//! `library`: the shape library (docs/commands.md §2.18).
//!
//! Items are saved groups of elements for reuse. The library lives in the `Storage` library namespace,
//! is not part of any document, and has no undo. It reads and writes `.excalidrawlib` (v1 and v2). The
//! file format is `drafftink-io`, taking an item out of a scene and putting it in is
//! `drafftink_edit::library`; this module keeps the list and the storage layout ([`store`]).

mod store;

use std::collections::HashSet;
use std::sync::Arc;

use drafftink_edit::library as frag;
use drafftink_io::excalidraw::{self, LibraryItem};
use drafftink_io::limits::MAX_LIBRARY_ITEMS;
use drafftink_io::{IoError, native};
use drafftink_scene::ElementId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use self::store::Item;
use super::doc::export::{Dest, ExportBytes};
use super::doc::files::{self, Loaded, Provenance, Source, base64_of};
use super::doc::open::{merge_into_active, open_in_new_tab};
use crate::cmd::tab::normalize_name;
use crate::cmd::view::Pt;
use crate::ports::{FileRef, PortKind};
use crate::refusal::{Refusal, echo};
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};

/// What `library.list` and `library.add` say about an item.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ItemInfo {
    /// The id to pass as `item`.
    pub id: String,
    /// The name, if it has one.
    pub name: Option<String>,
    /// Visible elements in the item.
    pub element_count: usize,
    /// Bounds `[x, y, w, h]` of the item (it starts at 0, 0).
    pub bounds: Option<[f64; 4]>,
    /// When it was added, milliseconds since the Unix epoch.
    pub created: u64,
}

fn info(i: &Item) -> ItemInfo {
    ItemInfo {
        id: i.id.clone(),
        name: i.name.clone(),
        element_count: i.document.elements.iter().filter(|e| !e.deleted).count(),
        bounds: frag::bounds(&i.document).map(|r| [round2(r.x0), round2(r.y0), round2(r.width()), round2(r.height())]),
        created: i.created_ms,
    }
}

fn round2(v: f64) -> f64 {
    if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 }
}

fn no_such_item(id: &str, item: &str) -> Refusal {
    Refusal::custom(
        "no_such_item",
        id,
        format!("There is no library item `{}`. List items with `library.list`.", echo(item)),
    )
    .suggesting("library.list", None)
}

/// Finds an item by id, else by name. Two items with the same name are ambiguous.
fn find(id: &str, items: &[Arc<Item>], wanted: &str) -> Result<usize, Refusal> {
    if let Some(i) = items.iter().position(|i| i.id == wanted) {
        return Ok(i);
    }
    let named: Vec<usize> = items.iter().enumerate().filter(|(_, i)| i.name.as_deref() == Some(wanted)).map(|(n, _)| n).collect();
    match named.as_slice() {
        [] => Err(no_such_item(id, wanted)),
        [one] => Ok(*one),
        _ => Err(Refusal::bad_arg(
            id,
            "item",
            format!("several items are named `{}`. Pass the item id from `library.list`", echo(wanted)),
        )),
    }
}

/// The state check of every library command.
fn needs_storage(s: &Session, id: &str) -> Result<(), Refusal> {
    if s.has_port(PortKind::Storage) {
        Ok(())
    } else {
        Err(Refusal::port_unavailable(id, "persistent storage", "storage port missing", "The shape library is kept in storage."))
    }
}

// ---- library.add ----

/// Arguments of `library.add`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AddArgs {
    /// The elements. Default: the selection.
    pub ids: Option<Vec<Ref>>,
    /// A name for the item (1 to 120 characters).
    pub name: Option<String>,
}

/// Output of `library.add`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AddOut {
    /// True.
    pub changed: bool,
    /// The new item.
    pub item: ItemInfo,
}

fn add(s: &mut Session, a: AddArgs) -> Result<AddOut, Refusal> {
    let id = s.cmd();
    let name = match a.name.as_deref() {
        Some(n) => Some(normalize_name(id, n)?),
        None => None,
    };
    store::writable(s, id)?;
    let ids = s.targets(a.ids.as_deref())?;
    let members = frag::members(s.store(), &ids).map_err(|e| Refusal::internal(id, e.to_string()))?;
    let keep: HashSet<ElementId> = members.into_iter().collect();
    let mut doc = native::subset(&s.tab().doc.snapshot(), &keep);
    if !frag::normalize(&mut doc) {
        return Err(Refusal::nothing_to_do(id, "add", "the elements have no size or are deleted"));
    }
    let mut items = store::items(s);
    if items.len() >= MAX_LIBRARY_ITEMS {
        return Err(Refusal::limit_exceeded(id, items.len() + 1, "library items", MAX_LIBRARY_ITEMS));
    }
    let item = Arc::new(Item {
        id: store::fresh_id(s, &items),
        name,
        created_ms: s.now_ms(),
        hash: frag::content_hash(&doc),
        document: doc,
    });
    let out = AddOut { changed: true, item: info(&item) };
    let touched = vec![item.id.clone()];
    items.push(item);
    store::flush_notes(s);
    store::commit(s, items, &touched, &[])?;
    Ok(out)
}

// ---- library.insert ----

/// Arguments of `library.insert`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertArgs {
    /// The item id or name.
    pub item: String,
    /// The world point to centre the item on. Default: the centre of the viewport.
    #[serde(default)]
    pub at: Option<Pt>,
    /// Select the inserted elements. Default true.
    #[serde(default = "yes")]
    pub select: bool,
}

fn yes() -> bool {
    true
}

/// Output of `library.insert`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InsertOut {
    /// True.
    pub changed: bool,
    /// The item that was inserted.
    pub item: String,
    /// The new elements, back to front.
    pub ids: Vec<String>,
}

fn insert(s: &mut Session, a: InsertArgs) -> Result<InsertOut, Refusal> {
    let id = s.cmd();
    let items = store::items(s);
    let it = items.get(find(id, &items, &a.item)?).ok_or_else(|| no_such_item(id, &a.item))?;
    let at = a.at.unwrap_or_else(|| {
        let v = s.view();
        [v.scroll.x, v.scroll.y]
    });
    let r = merge_into_active(s, id, &it.document, Some(at), a.select)?;
    store::flush_notes(s);
    Ok(InsertOut { changed: true, item: it.id.clone(), ids: r.inserted.iter().map(|i| i.as_str().to_owned()).collect() })
}

// ---- library.remove ----

/// Arguments of `library.remove`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct RemoveArgs {
    /// Item ids or names to remove. Give either this or `all`.
    pub items: Option<Vec<String>>,
    /// Remove every item. The only way to empty the library.
    pub all: bool,
}

/// Output of `library.remove`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RemoveOut {
    /// False when there was nothing to remove.
    pub changed: bool,
    /// The ids that were removed.
    pub removed: Vec<String>,
}

fn remove(s: &mut Session, a: RemoveArgs) -> Result<RemoveOut, Refusal> {
    let id = s.cmd();
    if a.all == a.items.is_some() {
        return Err(Refusal::bad_args(id, "Give either `items` or all=true."));
    }
    store::writable(s, id)?;
    let items = store::items(s);
    let gone: Vec<String> = if a.all {
        items.iter().map(|i| i.id.clone()).collect()
    } else {
        let mut v: Vec<String> = Vec::new();
        for w in a.items.iter().flatten() {
            let n = find(id, &items, w)?;
            if let Some(i) = items.get(n)
                && !v.contains(&i.id)
            {
                v.push(i.id.clone());
            }
        }
        v
    };
    if gone.is_empty() {
        return Ok(RemoveOut { changed: false, removed: gone });
    }
    let kept: Vec<Arc<Item>> = items.into_iter().filter(|i| !gone.contains(&i.id)).collect();
    store::flush_notes(s);
    store::commit(s, kept, &[], &gone)?;
    Ok(RemoveOut { changed: true, removed: gone })
}

// ---- library.rename ----

/// Arguments of `library.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameArgs {
    /// The item id or name.
    pub item: String,
    /// The new name (1 to 120 characters).
    pub name: String,
}

/// Output of `library.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RenameOut {
    /// False when the item already had this name.
    pub changed: bool,
    /// The item after the command.
    pub item: ItemInfo,
}

fn rename(s: &mut Session, a: RenameArgs) -> Result<RenameOut, Refusal> {
    let id = s.cmd();
    let name = normalize_name(id, &a.name)?;
    store::writable(s, id)?;
    let mut items = store::items(s);
    let n = find(id, &items, &a.item)?;
    let Some(slot) = items.get_mut(n) else { return Err(no_such_item(id, &a.item)) };
    if slot.name.as_deref() == Some(name.as_str()) {
        return Ok(RenameOut { changed: false, item: info(slot) });
    }
    let it = Arc::make_mut(slot);
    it.name = Some(name);
    let (touched, item) = (vec![it.id.clone()], info(it));
    store::flush_notes(s);
    store::commit(s, items, &touched, &[])?;
    Ok(RenameOut { changed: true, item })
}

// ---- library.list ----

/// Arguments of `library.list`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ListArgs {
    /// Only items whose name or id contains this text (case does not matter).
    pub search: Option<String>,
}

/// Output of `library.list`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ListOut {
    /// Always false.
    pub changed: bool,
    /// The items, in library order.
    pub items: Vec<ItemInfo>,
}

fn list(s: &mut Session, a: ListArgs) -> Result<ListOut, Refusal> {
    let items = store::items(s);
    store::flush_notes(s);
    let q = a.search.as_deref().map(str::to_lowercase);
    let items = items
        .iter()
        .filter(|i| match &q {
            Some(q) => i.id.to_lowercase().contains(q) || i.name.as_deref().is_some_and(|n| n.to_lowercase().contains(q)),
            None => true,
        })
        .map(|i| info(i))
        .collect();
    Ok(ListOut { changed: false, items })
}

// ---- library.import ----

/// How an imported library meets the existing one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// Add the items and skip duplicates (same content).
    #[default]
    Merge,
    /// Replace the whole library. Not undoable.
    Replace,
    /// Lay the items out on a grid in a new tab and leave the library alone (the old app's behaviour).
    OpenAsTab,
}

/// Arguments of `library.import`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportArgs {
    /// The `.excalidrawlib` file: `{path}`, `{stored}`, `{handle}` or `{base64, name?}`.
    pub source: Source,
    /// `merge` (default), `replace` or `open_as_tab`.
    #[serde(default)]
    pub mode: ImportMode,
}

/// Output of `library.import`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ImportOut {
    /// True when the library or the tabs changed.
    pub changed: bool,
    /// Items added to the library.
    pub added: usize,
    /// Items skipped because the library already has them (or the file has them twice).
    pub skipped: usize,
    /// Items in the library now.
    pub total: usize,
    /// The new tab, for `open_as_tab`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
}

fn not_a_library(id: &str, e: &IoError) -> Refusal {
    let why = match e {
        IoError::WrongFormat { hint, .. } => hint.clone(),
        other => other.to_string(),
    };
    Refusal::custom(
        "parse_failed",
        id,
        format!("The file is not an Excalidraw library: {}. Libraries are .excalidrawlib files.", files::cut(&why)),
    )
}

fn parse_library(s: &mut Session, id: &str, loaded: &Loaded) -> Result<excalidraw::Library, Refusal> {
    let text = std::str::from_utf8(&loaded.bytes)
        .map_err(|_| not_a_library(id, &IoError::parse("excalidraw library", "the data is not UTF-8 text")))?;
    let (lib, report) = excalidraw::import_library(text, &mut *s.ports.ids).map_err(|e| not_a_library(id, &e))?;
    for w in report.warnings {
        s.warn(w);
    }
    if report.omitted > 0 {
        s.warn(format!("{} more import notes were left out", report.omitted));
    }
    Ok(lib)
}

/// Adds the items of a library file to the library; items that exist (same content) are skipped. Returns
/// `(added, skipped)`.
fn merge_items(s: &mut Session, id: &str, lib: excalidraw::Library, replace: bool) -> Result<(usize, usize, usize), Refusal> {
    store::writable(s, id)?;
    let old = store::items(s);
    let mut items: Vec<Arc<Item>> = if replace { Vec::new() } else { old.clone() };
    let mut touched: Vec<String> = Vec::new();
    let (mut added, mut skipped) = (0usize, 0usize);
    let now = s.now_ms();
    for LibraryItem { name, mut document } in lib.items {
        if !frag::normalize(&mut document) {
            skipped += 1;
            continue;
        }
        let hash = frag::content_hash(&document);
        if items.iter().any(|i| i.hash == hash) {
            skipped += 1;
            continue;
        }
        if items.len() >= MAX_LIBRARY_ITEMS {
            return Err(Refusal::limit_exceeded(id, items.len() + 1, "library items", MAX_LIBRARY_ITEMS));
        }
        let name = name.and_then(|n| normalize_name(id, &n).ok());
        let item = Arc::new(Item { id: store::fresh_id(s, &items), name, created_ms: now, hash, document });
        touched.push(item.id.clone());
        items.push(item);
        added += 1;
    }
    let removed: Vec<String> = if replace { old.iter().map(|i| i.id.clone()).collect() } else { Vec::new() };
    let total = items.len();
    if added > 0 || !removed.is_empty() {
        store::commit(s, items, &touched, &removed)?;
    }
    Ok((added, skipped, total))
}

/// `doc.drop` hands a library file here: merge mode. Returns the number of items added.
pub(crate) fn import_loaded(s: &mut Session, id: &str, loaded: &Loaded) -> Result<usize, Refusal> {
    needs_storage(s, id)?;
    let lib = parse_library(s, id, loaded)?;
    merge_items(s, id, lib, false).map(|(added, _, _)| added)
}

fn stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = base.to_lowercase();
    let cut = [".excalidrawlib", ".json"].iter().find_map(|e| lower.ends_with(e).then(|| base.len() - e.len()));
    let s = cut.and_then(|n| base.get(..n)).unwrap_or(base).trim();
    if s.is_empty() || s == "the data" { "Library".to_owned() } else { s.to_owned() }
}

fn import(s: &mut Session, a: ImportArgs) -> Result<ImportOut, Refusal> {
    let id = s.cmd();
    let loaded = files::read_source(s, id, &a.source, "source")?;
    let lib = parse_library(s, id, &loaded)?;
    store::flush_notes(s);
    match a.mode {
        ImportMode::OpenAsTab => {
            let mut doc = excalidraw::library_to_grid_document(&lib, &mut *s.ports.ids);
            doc.name = stem(&loaded.name).chars().take(120).collect();
            open_in_new_tab(s, id, &doc, None, Provenance::Clean)?;
            let total = store::items(s).len();
            Ok(ImportOut { changed: true, added: 0, skipped: 0, total, tab: Some(s.tab().id.to_string()) })
        }
        ImportMode::Merge | ImportMode::Replace => {
            let (added, skipped, total) = merge_items(s, id, lib, a.mode == ImportMode::Replace)?;
            Ok(ImportOut { changed: added > 0 || a.mode == ImportMode::Replace, added, skipped, total, tab: None })
        }
    }
}

// ---- library.export ----

/// Arguments of `library.export`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ExportArgs {
    /// Item ids or names. Default: all items.
    pub items: Option<Vec<String>>,
    /// `inline` (default) returns the bytes; a file is written through the files port.
    pub to: Dest,
}

/// Output of `library.export`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ExportOut {
    /// Always false.
    pub changed: bool,
    /// Items written.
    pub items: usize,
    /// Size in bytes.
    pub size: usize,
    /// The content, for `to: inline`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<ExportBytes>,
    /// Where it was written, for a file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_to: Option<FileRef>,
}

const LIB_MIME: &str = "application/vnd.excalidrawlib+json";

fn export(s: &mut Session, a: ExportArgs) -> Result<ExportOut, Refusal> {
    let id = s.cmd();
    let all = store::items(s);
    store::flush_notes(s);
    if all.is_empty() {
        return Err(Refusal::custom("nothing_to_do", id, "The library is empty, so there is nothing to export."));
    }
    let chosen: Vec<&Arc<Item>> = match &a.items {
        Some(list) => {
            let mut v: Vec<&Arc<Item>> = Vec::new();
            for w in list {
                if let Some(i) = all.get(find(id, &all, w)?)
                    && !v.iter().any(|x| x.id == i.id)
                {
                    v.push(i);
                }
            }
            v
        }
        None => all.iter().collect(),
    };
    if chosen.is_empty() {
        return Err(Refusal::custom("nothing_to_do", id, "No items were chosen, so there is nothing to export."));
    }
    if let Dest::File(f) = &a.to {
        files::require_writable(s, id, f)?;
    }
    let lib = excalidraw::Library {
        items: chosen.iter().map(|i| LibraryItem { name: i.name.clone(), document: i.document.clone() }).collect(),
    };
    let (text, report) = excalidraw::export_library_with_report(&lib)
        .map_err(|e| Refusal::io_failed(id, "write", "the library", &e.to_string()))?;
    for w in report.warnings {
        s.warn(w);
    }
    let bytes = text.into_bytes();
    let mut out = ExportOut { changed: false, items: chosen.len(), size: bytes.len(), bytes: None, saved_to: None };
    match a.to {
        Dest::Inline(_) => out.bytes = Some(ExportBytes { base64: base64_of(&bytes), mime: LIB_MIME }),
        Dest::File(f) => {
            files::queue_file(s, id, &f, bytes, LIB_MIME)?;
            out.saved_to = Some(f);
        }
    }
    Ok(out)
}

// ---- registration ----

/// A library command: storage and the library buffer, allowed in view-only mode, refused during a drag.
const LIB: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::PORTS,
    needs_selection: Selection::No,
    ports: PortSet::STORAGE,
    batch: Batch::Allowed,
    view_only: Gate::Allowed,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::None,
    ring: RingMode::Always,
    palette: true,
    agent: true,
};

/// The library family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "library.add",
        "library",
        "Add to library",
        "Stores a copy of the selected (or given) elements as a new library item.",
        &Typed::new(add),
        Facts { needs_selection: Selection::Required, ..LIB },
    )
    .available(|s| needs_storage(s, "library.add"))
    .refusals(&["empty_name", "library_newer"])
    .example(r#"{"name":"Login box"}"#),
    CommandSpec::new(
        "library.insert",
        "library",
        "Insert from library",
        "Inserts a library item like a paste: new ids, centred on a point, selected.",
        &Typed::new(insert),
        Facts { undo: Undo::Undoable, mutates: Mutates::DOC.with(Mutates::OBSERVED), view_only: Gate::Refused, redraw: Redraw::Scene, ..LIB },
    )
    .available(|s| needs_storage(s, "library.insert"))
    .refusals(&["no_such_item"])
    .example(r#"{"item":"Login box","at":[400,300]}"#),
    CommandSpec::new(
        "library.remove",
        "library",
        "Remove from library",
        "Removes library items by id or name, or all of them with all=true. Not undoable.",
        &Typed::new(remove),
        LIB,
    )
    .available(|s| needs_storage(s, "library.remove"))
    .refusals(&["no_such_item", "library_newer"])
    .example(r#"{"items":["Login box"]}"#),
    CommandSpec::new(
        "library.rename",
        "library",
        "Rename library item",
        "Gives a library item a new name.",
        &Typed::new(rename),
        LIB,
    )
    .available(|s| needs_storage(s, "library.rename"))
    .refusals(&["no_such_item", "empty_name", "library_newer"])
    .example(r#"{"item":"lib_e1","name":"Login box"}"#),
    CommandSpec::new(
        "library.list",
        "library",
        "List library",
        "Lists the library items with id, name, element count, bounds and creation time.",
        &Typed::new(list),
        Facts { reads_only: true, mutates: Mutates::NONE, view_only: Gate::Allowed, while_gesture: Gate::Allowed, while_text_edit: TextGate::Allowed, ..LIB },
    )
    .available(|s| needs_storage(s, "library.list")),
    CommandSpec::new(
        "library.import",
        "library",
        "Import library",
        "Reads an .excalidrawlib file (v1 or v2) and merges it into the library, replaces the library, or opens it as a tab.",
        &Typed::new(import),
        Facts { mutates: Mutates::PORTS.with(Mutates::TABS), ports: PortSet::FILES.with(PortSet::STORAGE), ..LIB },
    )
    .available(|s| needs_storage(s, "library.import"))
    .refusals(&["library_newer", "too_many_tabs"])
    .example(r#"{"source":{"path":"/home/me/shapes.excalidrawlib"}}"#),
    CommandSpec::new(
        "library.export",
        "library",
        "Export library",
        "Writes the library (or some items) as an .excalidrawlib v2 file, inline or to a file.",
        &Typed::new(export),
        Facts { ports: PortSet::FILES.with(PortSet::STORAGE), ..LIB },
    )
    .available(|s| needs_storage(s, "library.export"))
    .refusals(&["no_such_item"]),
];

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
