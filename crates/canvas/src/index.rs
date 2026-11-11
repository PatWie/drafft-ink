//! [`SceneIndex`]: the spatial index of a scene, kept in step with a store from its change sets, plus exact hit
//! testing and marquee queries on top of it.
//!
//! The index holds one entry per live, drawable element: its world bounds including half the stroke and arrowheads
//! (see [`crate::shape::index_bounds`]). It is a cache of the scene, never a second source of truth. The scene
//! itself is passed to every call (as a [`SceneRead`]), so the same index works with any store.
//!
//! # Keeping it current
//!
//! Feed every [`ChangeSet`] a store emits to [`SceneIndex::apply`], in order. Per changed element the index
//! recomputes the bounds only when a geometry field changed (position, size, angle, flip, kind data, style or
//! deleted); a z-order, lock, group, name or custom change costs a field check and nothing else. One edit touches
//! O(changed) entries, each an O(log n) R-tree update. The index is rebuilt in bulk only when a single change set
//! touches more than half of the scene (loading a document), where bulk loading is faster than that many updates.
//! [`SceneIndex::verify`] compares the index with a full recomputation; tests and debug tools use it.
//!
//! # Hit testing
//!
//! A point query asks the R-tree for entries within the tolerance of the point, then runs the exact test of the
//! element's kind (see [`crate::shape`]) on those few candidates and returns the topmost one in z order. Costs
//! O(log n + candidates). Rules:
//!
//! * Deleted elements are not in the index. Locked elements are skipped unless [`HitOptions::include_locked`].
//! * The reach around an outline is `tolerance_px / zoom + stroke_width / 2` world units: a thin line stays
//!   clickable at any zoom, and the fat stroke of a marker is hit over its full width.
//! * A shape with a visible fill is hit inside; an unfilled one only near its outline. Text, math and images are
//!   hit anywhere in their box. A frame is hit near its border only.
//! * A child of a clipping frame cannot be hit outside the frame.

use std::cmp::Ordering;

use drafftink_geom::kurbo::{Point, Rect};
use drafftink_geom::select::{MarqueeMode as GeomMarquee, marquee_selects};
use drafftink_geom::{HitOpts, Outline, RectShape, SpatialIndex, aabb};
use drafftink_profile::{Counter, count, inc};
use drafftink_scene::{ChangeKind, ChangeSet, Element, ElementId, ElementKind, GroupId, SceneRead};
use serde::{Deserialize, Serialize};

use crate::shape::{element_box, hits_inside, index_bounds, shape_of, stroke_reach_width};

/// A change set that touches more entries than this, and more than half the index, rebuilds in bulk.
const BULK_REBUILD_MIN: usize = 2048;

/// Which part of an element a point hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HitPart {
    /// Near the outline.
    Stroke,
    /// Inside a filled shape, away from the outline.
    Fill,
    /// Inside the box of a text, math, image: the whole box counts as the element.
    Body,
}

/// A hit element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The element.
    pub id: ElementId,
    /// Which part of it.
    pub part: HitPart,
}

/// How a point query treats elements.
#[derive(Clone, Copy)]
pub struct HitOptions<'a> {
    /// Reach around outlines in logical screen pixels. Converted with the zoom.
    pub tolerance_px: f64,
    /// Locked elements can be hit (hover feedback and "what is here" queries want this; selection does not).
    pub include_locked: bool,
    /// Elements for which this returns false are invisible to the query (for example the dragged selection when
    /// looking for an arrow's binding target).
    pub filter: Option<&'a dyn Fn(&Element) -> bool>,
}

impl Default for HitOptions<'_> {
    fn default() -> Self {
        Self { tolerance_px: 5.0, include_locked: false, filter: None }
    }
}

impl<'a> HitOptions<'a> {
    /// Options with the given tolerance in screen pixels.
    pub fn with_tolerance(tolerance_px: f64) -> Self {
        Self { tolerance_px, ..Self::default() }
    }

    fn accepts(&self, e: &Element) -> bool {
        (self.include_locked || !e.locked) && self.filter.is_none_or(|f| f(e))
    }
}

/// How a marquee selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarqueeMode {
    /// Only elements whose bounds lie completely inside the rectangle.
    Contain,
    /// Every element the rectangle touches. An unfilled shape counts only when the rectangle crosses its outline
    /// or contains it, so a marquee inside an empty rectangle selects nothing. Frames always need `Contain`.
    Intersect,
}

/// How the index differs from the scene, as reported by [`SceneIndex::verify`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IndexMismatch {
    /// An element is indexed with other bounds than the scene gives it, or is missing.
    #[error("{id}: index has {have:?}, scene says {want:?}")]
    Bounds {
        /// The element.
        id: ElementId,
        /// What the index holds.
        have: Option<Rect>,
        /// What a recomputation gives.
        want: Rect,
    },
    /// An element without geometry is indexed.
    #[error("{id}: index has {have:?}, element has no geometry")]
    NoGeometry {
        /// The element.
        id: ElementId,
        /// What the index holds.
        have: Rect,
    },
    /// The index holds entries for elements the scene no longer has.
    #[error("index holds {indexed} entries, scene has {expected} drawable elements")]
    Count {
        /// Entries in the index.
        indexed: usize,
        /// Drawable elements in the scene.
        expected: usize,
    },
}

/// What [`SceneIndex::apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ApplyStats {
    /// Entries added or moved.
    pub updated: usize,
    /// Entries removed.
    pub removed: usize,
    /// Changes that did not alter geometry and cost nothing.
    pub skipped: usize,
    /// The index was rebuilt in bulk.
    pub rebuilt: bool,
}

/// The z order of two elements: back to front by `(index, id)`.
pub fn z_cmp(a: &Element, b: &Element) -> Ordering {
    a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id))
}

/// The group a click on `e` selects, given the group the user has entered (if any). `groups` stack is innermost
/// first. Without an entered group this is the outermost group. With one, it is the next group inside it, or
/// `None` when `e` is a direct member of the entered group (then the element itself is the unit). An element
/// outside the entered group resolves as if nothing was entered. `is_group` filters out groups that no longer
/// count (fewer than two members).
pub fn unit_group(e: &Element, entered: Option<&GroupId>, is_group: impl Fn(&GroupId) -> bool) -> Option<GroupId> {
    let ids = &e.group_ids;
    let start = match entered.and_then(|g| ids.iter().position(|x| x == g)) {
        Some(0) => return None,
        Some(k) => k - 1,
        None => ids.len().checked_sub(1)?,
    };
    // Walk outward from the chosen level until a group that still counts is found.
    let mut i = start;
    loop {
        match ids.get(i) {
            Some(g) if is_group(g) => return Some(g.clone()),
            Some(_) if i > 0 => i -= 1,
            _ => return None,
        }
    }
}

/// The spatial index of a scene. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct SceneIndex {
    index: SpatialIndex<ElementId>,
    bulk_rebuilds: u64,
}

impl SceneIndex {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// An index of everything in `scene`, built in bulk.
    pub fn build<R: SceneRead>(scene: &R) -> Self {
        let mut ix = Self::new();
        ix.rebuild(scene);
        ix
    }

    /// Replaces the content with a bulk-loaded index of `scene`.
    pub fn rebuild<R: SceneRead>(&mut self, scene: &R) {
        count(Counter::ElementsScanned, u64::try_from(scene.len_all()).unwrap_or(u64::MAX));
        self.index = SpatialIndex::bulk_load(scene.iter().filter_map(|e| index_bounds(e).map(|b| (e.id.clone(), b))));
        self.bulk_rebuilds += 1;
    }

    /// How many times the index was rebuilt in bulk (the first build counts). A test can assert that edits do not
    /// raise it.
    pub fn bulk_rebuilds(&self) -> u64 {
        self.bulk_rebuilds
    }

    /// Number of indexed elements.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// True when nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// True when the element is indexed.
    pub fn contains(&self, id: &ElementId) -> bool {
        self.index.contains(id)
    }

    /// The indexed bounds of an element (outline plus half stroke plus arrowheads).
    pub fn bounds_of(&self, id: &ElementId) -> Option<Rect> {
        self.index.get(id)
    }

    /// The union of all indexed bounds.
    pub fn total_bounds(&self) -> Option<Rect> {
        self.index.total_bounds()
    }

    /// The underlying R-tree (read only), for callers that want raw rectangle or nearest queries.
    pub fn spatial(&self) -> &SpatialIndex<ElementId> {
        &self.index
    }

    /// Brings the index up to date after `changes`. `scene` must be in the state the change set produced (or a
    /// later one, when several change sets are applied together). Call it with every change set in order.
    pub fn apply<R: SceneRead>(&mut self, scene: &R, changes: &ChangeSet) -> ApplyStats {
        let n = changes.elements.len();
        if n > BULK_REBUILD_MIN && n * 2 > self.index.len() {
            self.rebuild(scene);
            return ApplyStats { rebuilt: true, ..ApplyStats::default() };
        }
        let mut stats = ApplyStats::default();
        for c in &changes.elements {
            let live = scene.get(&c.id).filter(|e| !e.deleted);
            let Some(e) = live else {
                if self.index.remove(&c.id).is_some() {
                    stats.removed += 1;
                }
                continue;
            };
            if c.kind == ChangeKind::Updated && !c.fields.affects_geometry() && self.index.contains(&c.id) {
                stats.skipped += 1;
                continue;
            }
            match index_bounds(e) {
                Some(b) => {
                    if self.index.insert(e.id.clone(), b).is_ok() {
                        stats.updated += 1;
                    }
                }
                None => {
                    if self.index.remove(&c.id).is_some() {
                        stats.removed += 1;
                    }
                }
            }
        }
        stats
    }

    /// Compares the index with a full recomputation from `scene`. `Ok` when every live drawable element is indexed
    /// with exactly the bounds [`index_bounds`] gives and nothing else is. The error says what differs first.
    pub fn verify<R: SceneRead>(&self, scene: &R) -> Result<(), IndexMismatch> {
        let mut expected = 0usize;
        for e in scene.iter() {
            match (index_bounds(e), self.index.get(&e.id)) {
                (Some(want), Some(have)) if want == have => expected += 1,
                (Some(want), have) => return Err(IndexMismatch::Bounds { id: e.id.clone(), have, want }),
                (None, Some(have)) => return Err(IndexMismatch::NoGeometry { id: e.id.clone(), have }),
                (None, None) => {}
            }
        }
        if self.index.len() != expected {
            return Err(IndexMismatch::Count { indexed: self.index.len(), expected });
        }
        Ok(())
    }

    /// Live elements whose indexed bounds touch `rect`, back to front. This is the culling query: pass the visible
    /// world rectangle (grown by a margin) to get what to draw.
    pub fn in_rect<'s, R: SceneRead>(&self, scene: &'s R, rect: &Rect) -> Vec<&'s Element> {
        inc(Counter::SpatialQueries);
        let mut out: Vec<&Element> = self.index.query_rect(rect).filter_map(|(id, _)| scene.get(id)).collect();
        out.retain(|e| !e.deleted);
        out.sort_by(|a, b| z_cmp(a, b));
        out
    }

    /// The topmost element hit by a world point. `zoom` converts the pixel tolerance of `opts` to world units.
    pub fn hit<R: SceneRead>(&self, scene: &R, world: Point, zoom: f64, opts: &HitOptions<'_>) -> Option<Hit> {
        inc(Counter::HitTests);
        let tol = tolerance_world(opts.tolerance_px, zoom);
        let mut best: Option<(&Element, HitPart)> = None;
        for (id, _) in self.index.query_point(world, tol) {
            let Some(e) = scene.get(id) else { continue };
            if e.deleted || !opts.accepts(e) {
                continue;
            }
            if best.is_some_and(|(b, _)| z_cmp(e, b) != Ordering::Greater) {
                continue;
            }
            if let Some(part) = hit_element(scene, e, world, tol) {
                best = Some((e, part));
            }
        }
        best.map(|(e, part)| Hit { id: e.id.clone(), part })
    }

    /// Every element hit by a world point, topmost first.
    pub fn hit_all<R: SceneRead>(&self, scene: &R, world: Point, zoom: f64, opts: &HitOptions<'_>) -> Vec<Hit> {
        inc(Counter::HitTests);
        let tol = tolerance_world(opts.tolerance_px, zoom);
        let mut found: Vec<(&Element, HitPart)> = Vec::new();
        for (id, _) in self.index.query_point(world, tol) {
            let Some(e) = scene.get(id) else { continue };
            if e.deleted || !opts.accepts(e) {
                continue;
            }
            if let Some(part) = hit_element(scene, e, world, tol) {
                found.push((e, part));
            }
        }
        found.sort_by(|a, b| z_cmp(b.0, a.0));
        found.into_iter().map(|(e, part)| Hit { id: e.id.clone(), part }).collect()
    }

    /// Elements selected by a marquee rectangle (world space), back to front. Locked elements are left out unless
    /// `include_locked`.
    pub fn marquee<R: SceneRead>(
        &self,
        scene: &R,
        rect: &Rect,
        mode: MarqueeMode,
        include_locked: bool,
    ) -> Vec<ElementId> {
        inc(Counter::SpatialQueries);
        if !aabb::is_finite(rect) {
            return Vec::new();
        }
        let rect = aabb::normalize(*rect);
        let mut found: Vec<&Element> = Vec::new();
        for (id, _) in self.index.query_rect(&rect) {
            let Some(e) = scene.get(id) else { continue };
            if e.deleted || (e.locked && !include_locked) {
                continue;
            }
            let Some(shape) = shape_of(e) else { continue };
            let selected = match (mode, matches!(e.kind, ElementKind::Frame(_))) {
                (MarqueeMode::Contain, _) | (MarqueeMode::Intersect, true) => {
                    marquee_selects(&shape, &rect, GeomMarquee::Contain, false)
                }
                (MarqueeMode::Intersect, false) => marquee_selects(&shape, &rect, GeomMarquee::Touch, hits_inside(e)),
            };
            if selected {
                found.push(e);
            }
        }
        found.sort_by(|a, b| z_cmp(a, b));
        found.into_iter().map(|e| e.id.clone()).collect()
    }
}

fn tolerance_world(tolerance_px: f64, zoom: f64) -> f64 {
    if tolerance_px.is_finite() && zoom.is_finite() && zoom > 0.0 { tolerance_px.max(0.0) / zoom } else { 0.0 }
}

/// The exact test of one element against a world point; the index only narrows down which elements to ask.
/// `tol` is the reach beyond the outline in world units. Ignores the locked flag (the caller's rule). Public so a
/// brute-force scan can serve as the reference in tests.
pub fn hit_element<R: SceneRead>(scene: &R, e: &Element, world: Point, tol: f64) -> Option<HitPart> {
    if clipped_out(scene, e, world) {
        return None;
    }
    let shape = shape_of(e)?;
    let width = stroke_reach_width(e);
    let near_outline = shape.hit_test(world, HitOpts::stroke(tol, width));
    let box_like = matches!(e.kind, ElementKind::Text(_) | ElementKind::Math(_) | ElementKind::Image(_));
    if box_like {
        // The whole box, plus the tolerance around its border.
        return shape.hit_test(world, HitOpts::filled(tol, 0.0)).then_some(HitPart::Body);
    }
    if near_outline {
        return Some(HitPart::Stroke);
    }
    (hits_inside(e) && shape.contains(world)).then_some(HitPart::Fill)
}

/// A child of a clipping frame is invisible outside the frame, so it cannot be hit there.
fn clipped_out<R: SceneRead>(scene: &R, e: &Element, world: Point) -> bool {
    let Some(frame) = e.frame_id.as_ref().and_then(|f| scene.get(f)) else { return false };
    if frame.deleted {
        return false;
    }
    match &frame.kind {
        ElementKind::Frame(d) if d.clip => !RectShape::new(element_box(frame)).contains(world),
        _ => false,
    }
}
