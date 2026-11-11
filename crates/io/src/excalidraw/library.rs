//! `.excalidrawlib` import and export, and the grid layout that shows a library as one document.

use std::collections::{BTreeMap, HashMap, HashSet};

use drafftink_scene::limits::{MAX_COORD, MAX_ELEMENTS, MAX_NAME_CHARS};
use drafftink_scene::{Document, Element, ElementId, ElementKind, GroupId, IdSource, ZIndex};
use serde_json::{Map, Value, json};

use super::export::{SOURCE, export_elements};
use super::import::{Source, convert, fresh_element_id as fresh_element, fresh_group_id as fresh_group};
use super::read::warn;
use super::{Library, LibraryItem, parse_json};
use crate::limits::MAX_LIBRARY_ITEMS;
use crate::{Expected, FormatName, ImportReport, IoError};

/// Space on each side of an item in the grid, in world units.
pub const GRID_CELL_PADDING: f64 = 24.0;
/// Most columns the grid has.
pub const GRID_MAX_COLUMNS: usize = 10;

pub(super) fn import(text: &str, ids: &mut dyn IdSource) -> Result<(Library, ImportReport), IoError> {
    let v = parse_json(text, FormatName::ExcalidrawLibrary)?;
    let Some(obj) = v.as_object() else {
        return Err(IoError::WrongFormat {
            expected: Expected::ExcalidrawLibrary,
            hint: "the top level of a library file is a JSON object".to_owned(),
        });
    };
    if obj.get("type").and_then(Value::as_str) != Some("excalidrawlib") {
        return Err(IoError::WrongFormat {
            expected: Expected::ExcalidrawLibrary,
            hint: "an Excalidraw library file says \"type\": \"excalidrawlib\". To open a drawing, use Open instead"
                .to_owned(),
        });
    }
    // Version 2 keeps the items in `libraryItems`; version 1 in `library`. If both exist the newer one wins.
    let list =
        obj.get("libraryItems").and_then(Value::as_array).or_else(|| obj.get("library").and_then(Value::as_array));
    let mut rep = ImportReport::default();
    let mut lib = Library::default();
    let Some(list) = list else {
        warn(&mut rep, || "the library has no `libraryItems` or `library` list, so it is empty".to_owned());
        return Ok((lib, rep));
    };
    if list.len() > MAX_LIBRARY_ITEMS {
        warn(&mut rep, || format!("the library has {} items; only the first {MAX_LIBRARY_ITEMS} are read", list.len()));
    }
    let mut total = 0usize;
    for (i, item) in list.iter().enumerate().take(MAX_LIBRARY_ITEMS) {
        let n = i + 1;
        let prefix = format!("item {n}: ");
        // v2: an object with `elements` (and maybe `name`). v1, and some v2 files: the element array itself.
        let (elements, name) = match item {
            Value::Array(a) => (Some(a), None),
            Value::Object(o) => (o.get("elements").and_then(Value::as_array), o.get("name").and_then(Value::as_str)),
            _ => (None, None),
        };
        let Some(elements) = elements else {
            warn(&mut rep, || format!("{prefix}is not a list of elements and is skipped"));
            continue;
        };
        if total + elements.len() > MAX_ELEMENTS {
            warn(&mut rep, || {
                format!(
                    "{prefix}the library would pass {MAX_ELEMENTS} elements in total; this and the following items are skipped"
                )
            });
            break;
        }
        let conv = convert(Source { elements, files: None, prefix: &prefix }, ids, &mut rep)?;
        if conv.elements.is_empty() {
            warn(&mut rep, || format!("{prefix}has nothing this app can import and is skipped"));
            continue;
        }
        total += conv.elements.len();
        let name = name.map(|s| s.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect::<String>());
        let name = name.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let document = Document {
            name: name.clone().unwrap_or_else(|| "Untitled".to_owned()),
            elements: conv.elements,
            files: conv.files,
            ..Document::new()
        };
        lib.items.push(LibraryItem { name, document });
    }
    Ok((lib, rep))
}

/// `(min_x, min_y, max_x, max_y)`.
type Bounds = (f64, f64, f64, f64);

/// The bounding box of an item: `(min_x, min_y, max_x, max_y)` over the rotated boxes of its elements.
fn item_bounds(elements: &[Element]) -> Option<Bounds> {
    let mut b: Option<Bounds> = None;
    for e in elements.iter().filter(|e| !e.deleted) {
        let t = &e.transform;
        for (lx, ly) in [(0.0, 0.0), (t.w, 0.0), (0.0, t.h), (t.w, t.h)] {
            let p = t.local_to_world(kurbo::Point::new(lx, ly));
            b = Some(match b {
                None => (p.x, p.y, p.x, p.y),
                Some((x0, y0, x1, y1)) => (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)),
            });
        }
    }
    b
}

pub(super) fn grid(lib: &Library, ids: &mut dyn IdSource) -> Document {
    let mut doc = Document { name: "Library".to_owned(), ..Document::new() };
    let items: Vec<(&LibraryItem, Bounds)> =
        lib.items.iter().filter_map(|i| Some((i, item_bounds(&i.document.elements)?))).collect();
    if items.is_empty() {
        return doc;
    }
    let n = items.len();
    // Truncation is fine: the root of a count is small, and the value is clamped.
    let columns = ((n as f64).sqrt().ceil() as usize).clamp(1, GRID_MAX_COLUMNS);
    let cell_w = items.iter().map(|(_, b)| b.2 - b.0).fold(0.0, f64::max) + 2.0 * GRID_CELL_PADDING;
    let cell_h = items.iter().map(|(_, b)| b.3 - b.1).fold(0.0, f64::max) + 2.0 * GRID_CELL_PADDING;

    let mut used_elements: HashSet<ElementId> = HashSet::new();
    let mut used_groups: HashSet<GroupId> = HashSet::new();
    let mut elements: Vec<Element> = Vec::new();
    for (k, (item, b)) in items.iter().enumerate() {
        let live: Vec<&Element> = item.document.elements.iter().filter(|e| !e.deleted).collect();
        if elements.len() + live.len() > MAX_ELEMENTS {
            break;
        }
        let (row, col) = (k / columns, k % columns);
        let cx = col as f64 * cell_w + cell_w / 2.0;
        let cy = row as f64 * cell_h + cell_h / 2.0;
        let (dx, dy) = (cx - (b.0 + b.2) / 2.0, cy - (b.1 + b.3) / 2.0);

        // Ids from different items may clash ("A" in every item of a hand-made file). Rename only on a clash.
        let mut id_map: HashMap<&ElementId, ElementId> = HashMap::new();
        for e in &live {
            let id = if used_elements.contains(&e.id) { fresh_element(ids, &used_elements) } else { e.id.clone() };
            used_elements.insert(id.clone());
            id_map.insert(&e.id, id);
        }
        let mut group_map: HashMap<&GroupId, GroupId> = HashMap::new();
        for g in live.iter().flat_map(|e| e.group_ids.iter()) {
            if group_map.contains_key(g) {
                continue;
            }
            let id = if used_groups.contains(g) { fresh_group(ids, &used_groups) } else { g.clone() };
            used_groups.insert(id.clone());
            group_map.insert(g, id);
        }
        // One new group on top, so a click selects the whole item. A single element needs none.
        let item_group = if live.len() >= 2 {
            let g = fresh_group(ids, &used_groups);
            used_groups.insert(g.clone());
            Some(g)
        } else {
            None
        };
        for e in &live {
            let mut c = (*e).clone();
            c.id = id_map.get(&e.id).cloned().unwrap_or_else(|| e.id.clone());
            // A hostile item can be so large that the grid runs past the coordinate limit. Clamp: the layout is then
            // wrong, but the document stays valid.
            c.transform.x = (c.transform.x + dx).clamp(-MAX_COORD, MAX_COORD);
            c.transform.y = (c.transform.y + dy).clamp(-MAX_COORD, MAX_COORD);
            c.frame_id = e.frame_id.as_ref().and_then(|f| id_map.get(f).cloned());
            c.group_ids = e.group_ids.iter().filter_map(|g| group_map.get(g).cloned()).collect();
            if let Some(g) = &item_group {
                c.group_ids.push(g.clone());
            }
            match &mut c.kind {
                ElementKind::Text(t) => t.container_id = t.container_id.as_ref().and_then(|x| id_map.get(x).cloned()),
                ElementKind::Line(d) | ElementKind::Arrow(d) => {
                    for b in [&mut d.start_binding, &mut d.end_binding] {
                        match b.as_mut().and_then(|bb| id_map.get(&bb.target).cloned().map(|t| (bb, t))) {
                            Some((bb, t)) => bb.target = t,
                            None => *b = None,
                        }
                    }
                }
                _ => {}
            }
            elements.push(c);
        }
        for (id, file) in &item.document.files {
            doc.files.entry(id.clone()).or_insert_with(|| file.clone());
        }
    }
    match ZIndex::rebalance(elements.len()) {
        Ok(keys) => {
            for (e, k) in elements.iter_mut().zip(keys) {
                e.index = k;
            }
            doc.elements = elements;
        }
        // Not reachable for a count within MAX_ELEMENTS; an empty document is the safe answer.
        Err(_) => doc.files.clear(),
    }
    doc
}

pub(super) fn export(lib: &Library) -> Result<(String, ImportReport), IoError> {
    if lib.items.len() > MAX_LIBRARY_ITEMS {
        return Err(IoError::Limit { what: "library", max: format!("{MAX_LIBRARY_ITEMS} items") });
    }
    let mut rep = ImportReport::default();
    let mut items: Vec<Value> = Vec::with_capacity(lib.items.len());
    for (i, item) in lib.items.iter().enumerate() {
        let report = item.document.validate();
        if report.has_errors() {
            return Err(IoError::Document(drafftink_scene::DocumentError::Invalid(report)));
        }
        let out = export_elements(&item.document.elements, &BTreeMap::new(), false, &mut rep);
        let mut m = Map::new();
        m.insert("id".to_owned(), json!(format!("item-{}", i + 1)));
        m.insert("status".to_owned(), json!("unpublished"));
        m.insert("created".to_owned(), json!(0));
        if let Some(name) = &item.name {
            m.insert("name".to_owned(), json!(name));
        }
        m.insert("elements".to_owned(), Value::Array(out.elements));
        items.push(Value::Object(m));
    }
    let root = json!({"type": "excalidrawlib", "version": 2, "source": SOURCE, "libraryItems": items});
    let text = serde_json::to_string_pretty(&root)
        .map_err(|e| IoError::invalid(FormatName::ExcalidrawLibrary, e.to_string()))?;
    Ok((text, rep))
}
