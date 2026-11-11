//! Reusable fragments: shape library items, and the content of a file merged into a document.
//!
//! A fragment is a small [`Document`]: elements back to front plus the image blobs they use. Three rules live
//! here, so `library.add`, `library.insert`, `library.import` and `doc.open` (mode merge) share them:
//!
//! * **Taking a fragment out of a scene** ([`members`], [`normalize`]): the chosen elements, their bound text and
//!   the children of chosen frames, moved so that the top-left corner of the content is at the origin.
//! * **Putting a fragment into a scene** ([`insert_fragment`]): every element gets a new id and seed, groups get
//!   new ids (all members of one group share one new id), bound text, frame membership and arrow bindings follow
//!   the new ids (a reference to something outside the fragment is cut), names are dropped (a name addresses one
//!   element), and the copies go on top of the z-order in their old order. The block is centred on a point or
//!   keeps its coordinates.
//! * **Telling two fragments apart** ([`content_hash`]): equal when they draw the same, whatever the ids, seeds,
//!   names, z-order keys or position.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use drafftink_geom::kurbo::{Point, Rect, Vec2};
use drafftink_scene::{
    Document, Element, ElementError, ElementId, ElementKind, GroupId, IdSource, SceneRead, SceneTx, StoreError,
    validate_element,
};
use serde_json::Value;

use crate::binding::{self, BindingError};
use crate::group::name_key;
use crate::group::units::{Relations, primaries, with_companions};
use crate::query::element_bounds;
use crate::target::TargetError;

/// Why a fragment could not be taken or inserted.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// No element was given.
    #[error("no elements were given. Select some, or pass `ids`")]
    NoTarget,
    /// An element does not exist, is deleted, or is locked.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// The fragment has no visible element.
    #[error("the fragment has no elements")]
    Empty,
    /// A position is not a finite number.
    #[error("the position must be a finite number")]
    NotFinite,
    /// The id source kept returning ids that are in use.
    #[error("could not make a fresh id; the id source keeps returning used ids")]
    IdsExhausted,
    /// An element of the fragment breaks a rule.
    #[error("element {id} of the fragment is invalid: {error}")]
    Invalid {
        /// The element (its id in the fragment).
        id: ElementId,
        /// The broken rule.
        error: ElementError,
    },
    /// The store refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Bindings of the copies could not be repaired.
    #[error(transparent)]
    Binding(#[from] BindingError),
}

/// The elements a library item is made of: `ids` with every bound text replaced by its container, then the
/// bound text of each and the children of each frame. In z-order. Errors when `ids` is empty or names a
/// missing element.
pub fn members<R: SceneRead>(scene: &R, ids: &[ElementId]) -> Result<Vec<ElementId>, LibraryError> {
    if ids.is_empty() {
        return Err(LibraryError::NoTarget);
    }
    let given = primaries(scene, ids)?;
    let rel = Relations::build(scene);
    Ok(with_companions(scene, &rel, &given, true))
}

/// The union of the bounds of the visible elements, rotation included.
pub fn bounds(doc: &Document) -> Option<Rect> {
    doc.elements.iter().filter(|e| !e.deleted).map(element_bounds).reduce(|a, b| a.union(b))
}

/// Drops tombstones, names and change counters, and moves the content so that its top-left corner is at the
/// origin. Returns false (and changes nothing) when there is nothing visible.
pub fn normalize(doc: &mut Document) -> bool {
    let Some(b) = bounds(doc) else { return false };
    doc.elements.retain(|e| !e.deleted);
    for e in &mut doc.elements {
        e.transform.x -= b.x0;
        e.transform.y -= b.y0;
        e.name = None;
        e.version = 0;
        e.locked = false;
    }
    let used: BTreeSet<_> = doc
        .elements
        .iter()
        .filter_map(|e| match &e.kind {
            ElementKind::Image(i) => i.file_id.clone(),
            _ => None,
        })
        .collect();
    doc.files.retain(|id, _| used.contains(id));
    true
}

/// A number that is equal for two fragments that draw the same: ids, seeds, names, z-order keys, group names and
/// change counters are left out, references are compared by position, and so is everything else about the
/// elements. Fit for finding duplicates; not a security hash.
pub fn content_hash(doc: &Document) -> u64 {
    let live: Vec<&Element> = doc.elements.iter().filter(|e| !e.deleted).collect();
    let mut names: HashMap<String, String> = HashMap::new();
    for (i, e) in live.iter().enumerate() {
        names.insert(e.id.as_str().to_owned(), format!("#{i}"));
    }
    for e in &live {
        for g in &e.group_ids {
            let n = names.len();
            names.entry(g.as_str().to_owned()).or_insert_with(|| format!("@{n}"));
        }
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for e in live {
        let mut v = serde_json::to_value(e).unwrap_or(Value::Null);
        if let Value::Object(o) = &mut v {
            for k in ["id", "index", "name", "version", "custom", "locked"] {
                o.remove(k);
            }
            if let Some(Value::Object(s)) = o.get_mut("style") {
                s.remove("seed");
            }
        }
        rename(&mut v, &names);
        feed(v.to_string().as_bytes());
        feed(b"\n");
    }
    for (id, f) in &doc.files {
        feed(id.as_str().as_bytes());
        feed(f.mime.as_bytes());
    }
    h
}

fn rename(v: &mut Value, names: &HashMap<String, String>) {
    match v {
        Value::String(s) => {
            if let Some(n) = names.get(s.as_str()) {
                *s = n.clone();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rename(x, names)),
        Value::Object(o) => o.values_mut().for_each(|x| rename(x, names)),
        _ => {}
    }
}

/// Where an inserted fragment goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// Keep the coordinates the fragment has.
    AsIs,
    /// Move the centre of the fragment's bounds to this point.
    Centre(Point),
}

/// Outcome of [`insert_fragment`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InsertReport {
    /// The new elements, back to front.
    pub inserted: Vec<ElementId>,
    /// The outermost new group of each new element that has one (for selecting whole groups).
    pub outer_groups: Vec<GroupId>,
    /// Arrows the binding module repaired.
    pub arrows: Vec<ElementId>,
}

fn fresh<I: IdSource + ?Sized, R: SceneRead>(
    src: &mut I,
    scene: &R,
    used: &BTreeSet<ElementId>,
) -> Result<ElementId, LibraryError> {
    for _ in 0..64 {
        let id = src.element_id();
        if scene.get(&id).is_none() && !used.contains(&id) {
            return Ok(id);
        }
    }
    Err(LibraryError::IdsExhausted)
}

/// Inserts a copy of the fragment on top of the scene. See the module docs for the rules. Nothing is written
/// when the fragment is empty or an element is invalid.
pub fn insert_fragment<T: SceneTx, I: IdSource + ?Sized>(
    tx: &mut T,
    fragment: &Document,
    place: Placement,
    ids: &mut I,
) -> Result<InsertReport, LibraryError> {
    let live: Vec<&Element> = fragment.elements.iter().filter(|e| !e.deleted).collect();
    if live.is_empty() {
        return Err(LibraryError::Empty);
    }
    let offset = match place {
        Placement::AsIs => Vec2::ZERO,
        Placement::Centre(p) => {
            if !p.x.is_finite() || !p.y.is_finite() {
                return Err(LibraryError::NotFinite);
            }
            let c = bounds(fragment).map(|b| b.center()).unwrap_or(Point::ZERO);
            Vec2::new(p.x - c.x, p.y - c.y)
        }
    };

    let mut id_map: BTreeMap<ElementId, ElementId> = BTreeMap::new();
    let mut used: BTreeSet<ElementId> = BTreeSet::new();
    for e in &live {
        let n = fresh(ids, &*tx, &used)?;
        used.insert(n.clone());
        id_map.insert(e.id.clone(), n);
    }
    let existing: BTreeSet<GroupId> = tx.iter_all().flat_map(|e| e.group_ids.iter().cloned()).collect();
    let mut group_map: BTreeMap<GroupId, GroupId> = BTreeMap::new();
    for g in live.iter().flat_map(|e| e.group_ids.iter()) {
        if group_map.contains_key(g) {
            continue;
        }
        let mut n = ids.group_id();
        for _ in 0..64 {
            if !existing.contains(&n) && !group_map.values().any(|v| *v == n) {
                break;
            }
            n = ids.group_id();
        }
        if existing.contains(&n) || group_map.values().any(|v| *v == n) {
            return Err(LibraryError::IdsExhausted);
        }
        group_map.insert(g.clone(), n);
    }

    let mut copies: Vec<Element> = Vec::with_capacity(live.len());
    for o in &live {
        let Some(new_id) = id_map.get(&o.id) else { continue };
        let mut c = (*o).clone();
        c.id = new_id.clone();
        c.version = 0;
        c.deleted = false;
        c.name = None;
        c.style.seed = ids.seed();
        c.transform.x += offset.x;
        c.transform.y += offset.y;
        c.group_ids = o.group_ids.iter().filter_map(|g| group_map.get(g).cloned()).collect();
        c.frame_id = o.frame_id.as_ref().and_then(|f| id_map.get(f).cloned());
        if let ElementKind::Text(t) = &mut c.kind {
            let container = t.container_id.as_ref().and_then(|x| id_map.get(x).cloned());
            if container.is_none() {
                t.label_position = None;
            }
            t.container_id = container;
        }
        if let Some(custom) = &mut c.custom {
            for (old, new) in &group_map {
                if let Some(v) = custom.remove(&name_key(old)) {
                    custom.insert(name_key(new), v);
                }
            }
            if custom.is_empty() {
                c.custom = None;
            }
        }
        validate_element(&c).map_err(|error| LibraryError::Invalid { id: o.id.clone(), error })?;
        copies.push(c);
    }

    for (fid, entry) in &fragment.files {
        if copies.iter().any(|c| matches!(&c.kind, ElementKind::Image(i) if i.file_id.as_ref() == Some(fid))) {
            tx.put_file(entry.clone())?;
        }
    }
    let new_ids: Vec<ElementId> = copies.iter().map(|c| c.id.clone()).collect();
    let mut outer: Vec<GroupId> = Vec::new();
    for mut c in copies {
        if let Some(g) = c.group_ids.last()
            && !outer.contains(g)
        {
            outer.push(g.clone());
        }
        c.index = tx.next_top_index().map_err(StoreError::backend)?;
        tx.insert(c)?;
    }
    // Bindings between copies point at copies; a binding to something outside the fragment is cut.
    let map: HashMap<ElementId, ElementId> = id_map.iter().map(|(a, b)| (a.clone(), b.clone())).collect();
    binding::remap_bindings(tx, &new_ids, &map)?;
    let arrows = binding::heal_bindings(tx, Some(&new_ids))?;
    Ok(InsertReport { inserted: new_ids, outer_groups: outer, arrows })
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
