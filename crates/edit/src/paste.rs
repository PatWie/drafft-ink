//! Paste: put elements that come from outside the scene (the clipboard) into it.
//!
//! The structure rules are the ones [`crate::lifecycle::duplicate`] applies to a copy, applied to elements that are
//! not in the scene yet:
//!
//! * Every element, and every group that still has two or more members among the pasted elements, gets a **new id**.
//!   A pasted element never joins a group that exists in the scene. A group with fewer than two pasted members
//!   counts as no group. Group names (kept in `custom`) follow their group.
//! * Bound text keeps its container when the container was pasted, and becomes free text otherwise. Frame
//!   membership is kept only when the frame was pasted too.
//! * Bindings of arrows are remapped to the pasted targets with [`crate::binding::remap_bindings`]; a binding to
//!   something that was not pasted is dropped, so a paste is bound only to pasted elements.
//! * The block keeps its internal z-order and goes on top of the scene. Names and versions are reset, seeds are
//!   new, and the pictures that image elements use are stored by content hash (the same bytes are stored once).
//! * With `center`, the block is moved so that the middle of its bounds (labels excluded) is at that point.
//!
//! [`text_element`] and [`insert_text`] make a free text from plain text.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use kurbo::{Point, Vec2};

use drafftink_scene::{
    Element, ElementError, ElementId, ElementKind, FileEntry, FileId, GroupId, IdSource, IndexError, SceneTx,
    StoreError, Style, TextStyle, Transform, ZIndex, limits, validate_element,
};

use crate::binding::{self, BindingError};
use crate::create::{self, TextAnchor, TextSpec};
use crate::group::name_key;
use crate::text_bind::TextMeasure;
use crate::transform::geometry::union_bounds;
use crate::zorder::{self, REBALANCE_LEN, ZOrderError};

/// Why a paste was refused.
#[derive(Debug, thiserror::Error)]
pub enum PasteError {
    /// There was nothing to paste.
    #[error("there are no elements to paste")]
    Empty,
    /// The scene would hold more elements than the model allows.
    #[error("pasting {n} elements would pass the limit of {max} elements per document")]
    TooMany {
        /// Elements to paste.
        n: usize,
        /// Elements the document can still take.
        max: usize,
    },
    /// The text is empty or only white space.
    #[error("the text is empty")]
    EmptyText,
    /// The text is longer than a text element can be.
    #[error("the text has {n} characters, and a text element holds at most {max}")]
    TextTooLong {
        /// Characters given.
        n: usize,
        /// The limit.
        max: usize,
    },
    /// A position or size is not a usable number.
    #[error("the position or size of the pasted elements is not a finite number within the document limits")]
    NotFinite,
    /// The id source kept returning ids that are in use.
    #[error("could not make a fresh id; the id source keeps returning used ids")]
    IdsExhausted,
    /// A pasted element breaks a rule of the model.
    #[error("the pasted element {id} is invalid: {error}")]
    Invalid {
        /// The original id.
        id: ElementId,
        /// The broken rule.
        error: ElementError,
    },
    /// The store refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Bound arrows could not be updated.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// Z-order keys could not be made.
    #[error(transparent)]
    ZOrder(#[from] ZOrderError),
    /// A z-order key could not be made.
    #[error(transparent)]
    Index(#[from] IndexError),
}

/// Options of [`insert_elements`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PasteOpts {
    /// Move the block so that the middle of its bounds is here. `None` keeps the positions.
    pub center: Option<Point>,
}

/// What [`insert_elements`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PasteReport {
    /// Original and new id of every pasted element, in z-order.
    pub copies: Vec<(ElementId, ElementId)>,
    /// The new elements in z-order, bound text included.
    pub created: Vec<ElementId>,
    /// The new elements that make the selection: everything except bound text.
    pub selection: Vec<ElementId>,
    /// Old and new id of every regenerated group.
    pub groups: Vec<(GroupId, GroupId)>,
    /// Arrows the binding module re-solved.
    pub arrows: Vec<ElementId>,
    /// How far the block was moved.
    pub offset: (f64, f64),
}

/// The start of every `custom` key that holds a group name (see [`name_key`]).
const GROUP_NAME_PREFIX: &str = "drafftink.group.";

fn fresh(
    src: &mut dyn IdSource,
    taken: &dyn Fn(&ElementId) -> bool,
    used: &BTreeSet<ElementId>,
) -> Result<ElementId, PasteError> {
    for _ in 0..64 {
        let id = src.element_id();
        if !taken(&id) && !used.contains(&id) {
            return Ok(id);
        }
    }
    Err(PasteError::IdsExhausted)
}

/// Inserts `elements` (with the pictures they use from `files`) into `tx`. See the module docs for the rules.
/// Deleted elements are skipped; a repeated id is used once.
pub fn insert_elements<T: SceneTx>(
    tx: &mut T,
    elements: &[Element],
    files: &BTreeMap<FileId, FileEntry>,
    opts: &PasteOpts,
    ids: &mut dyn IdSource,
) -> Result<PasteReport, PasteError> {
    let mut seen: BTreeSet<&ElementId> = BTreeSet::new();
    let mut src: Vec<&Element> = elements.iter().filter(|e| !e.deleted && seen.insert(&e.id)).collect();
    if src.is_empty() {
        return Err(PasteError::Empty);
    }
    src.sort_by(|a, b| a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id)));
    let room = limits::MAX_ELEMENTS.saturating_sub(tx.len_all());
    if src.len() > room {
        return Err(PasteError::TooMany { n: src.len(), max: room });
    }

    // Where the block goes.
    let offset = match opts.center {
        None => Vec2::ZERO,
        Some(c) => {
            let top = src.iter().filter(|e| e.container_id().is_none()).copied();
            let b = union_bounds(top).or_else(|| union_bounds(src.iter().copied())).ok_or(PasteError::Empty)?;
            Vec2::new(c.x - b.center().x, c.y - b.center().y)
        }
    };
    let finite = |v: f64| v.is_finite() && v.abs() <= limits::MAX_COORD;
    if !finite(offset.x) || !finite(offset.y) {
        return Err(PasteError::NotFinite);
    }

    // New ids.
    let mut id_map: BTreeMap<ElementId, ElementId> = BTreeMap::new();
    let mut used: BTreeSet<ElementId> = BTreeSet::new();
    for e in &src {
        let n = fresh(ids, &|i| tx.contains(i), &used)?;
        used.insert(n.clone());
        id_map.insert(e.id.clone(), n);
    }

    // Groups: every group with two or more pasted members is regenerated; the others are dropped.
    let mut counts: BTreeMap<&GroupId, usize> = BTreeMap::new();
    for e in &src {
        for g in &e.group_ids {
            *counts.entry(g).or_default() += 1;
        }
    }
    let existing: BTreeSet<GroupId> = tx.iter_all().flat_map(|e| e.group_ids.iter().cloned()).collect();
    let mut group_map: BTreeMap<GroupId, GroupId> = BTreeMap::new();
    for (g, n) in counts {
        if n < 2 {
            continue;
        }
        let mut candidate = ids.group_id();
        for _ in 0..64 {
            if !existing.contains(&candidate) && !group_map.values().any(|v| *v == candidate) {
                break;
            }
            candidate = ids.group_id();
        }
        if existing.contains(&candidate) || group_map.values().any(|v| *v == candidate) {
            return Err(PasteError::IdsExhausted);
        }
        group_map.insert(g.clone(), candidate);
    }

    // Build and check every copy before writing anything.
    let mut copies: Vec<Element> = Vec::with_capacity(src.len());
    for o in &src {
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
            match t.container_id.as_ref().and_then(|x| id_map.get(x)) {
                Some(n) => t.container_id = Some(n.clone()),
                None => {
                    t.container_id = None;
                    t.label_position = None;
                }
            }
        }
        if let Some(custom) = &mut c.custom {
            let old_keys: Vec<(String, GroupId)> =
                o.group_ids.iter().filter_map(|g| group_map.get(g).map(|n| (name_key(g), n.clone()))).collect();
            let mut kept = BTreeMap::new();
            for (old, new) in old_keys {
                if let Some(v) = custom.remove(&old) {
                    kept.insert(name_key(&new), v);
                }
            }
            // Names of groups that were not carried over are dropped.
            custom.retain(|k, _| !k.starts_with(GROUP_NAME_PREFIX));
            custom.extend(kept);
            if custom.is_empty() {
                c.custom = None;
            }
        }
        validate_element(&c).map_err(|error| PasteError::Invalid { id: o.id.clone(), error })?;
        copies.push(c);
    }

    // Z-order keys: one block on top.
    let top = tx.top_z();
    let keys = match ZIndex::n_between(top.as_ref(), None, copies.len()) {
        Ok(k) => k,
        Err(_) => {
            zorder::rebalance(tx)?;
            ZIndex::n_between(tx.top_z().as_ref(), None, copies.len())?
        }
    };
    for (c, k) in copies.iter_mut().zip(keys) {
        c.index = k;
    }
    let long = copies.iter().any(|c| c.index.as_str().len() > REBALANCE_LEN);

    // Pictures first, then the elements.
    let mut stored: BTreeSet<&FileId> = BTreeSet::new();
    for c in &copies {
        if let ElementKind::Image(i) = &c.kind
            && let Some(f) = &i.file_id
            && let Some(entry) = files.get(f)
            && stored.insert(f)
        {
            tx.put_file(entry.clone())?;
        }
    }
    let new_ids: Vec<ElementId> = copies.iter().map(|c| c.id.clone()).collect();
    let selection: Vec<ElementId> =
        copies.iter().filter(|c| c.container_id().is_none()).map(|c| c.id.clone()).collect();
    for c in copies {
        tx.insert(c)?;
    }
    if long {
        zorder::rebalance(tx)?;
    }
    let map: HashMap<ElementId, ElementId> = id_map.iter().map(|(a, b)| (a.clone(), b.clone())).collect();
    binding::remap_bindings(tx, &new_ids, &map)?;
    let arrows = binding::update_bound_arrows(tx, &new_ids)?;
    let pairs = src.iter().filter_map(|o| id_map.get(&o.id).map(|n| (o.id.clone(), n.clone()))).collect();
    Ok(PasteReport {
        copies: pairs,
        created: new_ids,
        selection,
        groups: group_map.into_iter().collect(),
        arrows,
        offset: (offset.x, offset.y),
    })
}

/// A free text with its middle at `center`: the box follows the content (auto-resize on).
/// Blank text and text over the length limit are refused.
pub fn text_element<M: TextMeasure + ?Sized>(
    m: &M,
    id: ElementId,
    index: ZIndex,
    content: &str,
    font: TextStyle,
    style: Style,
    center: Point,
) -> Result<Element, PasteError> {
    if content.trim().is_empty() {
        return Err(PasteError::EmptyText);
    }
    let n = content.chars().count();
    if n > limits::MAX_TEXT_CHARS {
        return Err(PasteError::TextTooLong { n, max: limits::MAX_TEXT_CHARS });
    }
    let spec = TextSpec { font, at: center, width: None };
    let e = create::free_text(m, id, index, content, spec, TextAnchor::Center, style);
    if !(finite_box(&e.transform)) {
        return Err(PasteError::NotFinite);
    }
    validate_element(&e).map_err(|error| PasteError::Invalid { id: e.id.clone(), error })?;
    Ok(e)
}

fn finite_box(t: &Transform) -> bool {
    [t.x, t.y, t.w, t.h].iter().all(|v| v.is_finite() && v.abs() <= limits::MAX_COORD)
}

/// Makes a free text of `content` on top of the scene, centred on `center`. Returns its id.
pub fn insert_text<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &mut dyn IdSource,
    content: &str,
    font: TextStyle,
    mut style: Style,
    center: Point,
) -> Result<ElementId, PasteError> {
    let id = crate::text_bind::fresh_element_id(&*tx, ids);
    let z = tx.next_top_index()?;
    style.seed = ids.seed();
    let e = text_element(m, id.clone(), z, content, font, style, center)?;
    tx.insert(e)?;
    Ok(id)
}
