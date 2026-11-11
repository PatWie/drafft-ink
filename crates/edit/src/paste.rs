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
//! [`insert_text`] makes a free text from plain text with `create::text_element`.

use std::collections::{BTreeMap, BTreeSet};

use drafftink_geom::kurbo::{Point, Vec2};

use drafftink_scene::{
    Element, ElementId, ElementKind, FileEntry, FileId, GroupId, IdSource, IndexError, SceneTx, StoreError, Style,
    TextStyle, limits,
};

use crate::copy::{self, CopyError, Placement};
use crate::create::{FreeTextError, TextPlace, TextSpec, text_element};
use crate::text_bind::TextMeasure;
use crate::transform::geometry::union_bounds;

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
    /// A position or size is not a usable number.
    #[error("the position or size of the pasted elements is not a finite number within the document limits")]
    NotFinite,
    /// The copies could not be made or written.
    #[error(transparent)]
    Copy(#[from] CopyError),
    /// The store refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The free text could not be built.
    #[error(transparent)]
    Text(#[from] FreeTextError),
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

    // Every group with two or more pasted members is regenerated; the others are dropped.
    let params = copy::Params { offset, keep_groups: &BTreeSet::new(), is_group: &|_| true };
    let mut copies = copy::build(&*tx, &src, &params, ids)?;

    // Pictures first, then the elements.
    let mut stored: BTreeSet<&FileId> = BTreeSet::new();
    for c in &copies.elements {
        if let ElementKind::Image(i) = &c.kind
            && let Some(f) = &i.file_id
            && let Some(entry) = files.get(f)
            && stored.insert(f)
        {
            tx.put_file(entry.clone())?;
        }
    }
    let created = copies.new_ids();
    let selection: Vec<ElementId> =
        copies.elements.iter().filter(|c| c.container_id().is_none()).map(|c| c.id.clone()).collect();
    let pairs = copies.pairs(&src);
    let groups = copies.regenerated_groups();
    let inserted = copy::insert(tx, &mut copies, Placement::OnTop)?;
    Ok(PasteReport { copies: pairs, created, selection, groups, arrows: inserted.arrows, offset: (offset.x, offset.y) })
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
    let id = crate::ids::unused_element_id(&*tx, ids);
    let z = tx.next_top_index()?;
    style.seed = ids.seed();
    let spec = TextSpec { font, place: TextPlace::Center(center), width: None };
    let e = text_element(m, id.clone(), z, content, spec, style)?;
    tx.insert(e)?;
    Ok(id)
}
