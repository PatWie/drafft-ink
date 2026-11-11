//! Move, nudge, resize, rotate and set-bounds of a set of elements, and the shared machinery that
//! [`crate::arrange`] (align, distribute, flip) and `lifecycle` use.
//!
//! The transform rule (`docs/commands.md` section 2.4), in one place:
//! 1. **Companions follow.** Bound text moves, scales and rotates with its container. A frame brings its children
//!    (for move, nudge and flip; a frame resized alone does not scale its children).
//! 2. **Bindings.** An arrow end bound to a target that is in the transformed set keeps its binding and moves
//!    with the target. An end bound to a target outside the set is unbound first, so an arrow moved, resized,
//!    rotated or flipped alone detaches. After a mirror or a non-uniform scale the fixed points of the kept
//!    bindings are recomputed against the new target box.
//! 3. **Followers.** [`crate::binding::update_bound_arrows`] runs on every changed id before the function returns,
//!    so arrows bound to a transformed target follow it.
//! 4. **All or nothing.** Locked elements (companions included) refuse the whole call; every new value is
//!    validated before the first write, so an error leaves the transaction untouched.
//!
//! Groups have no transform of their own. Moving, resizing, rotating or flipping a group changes each member's own
//! box and angle, so what is drawn, hit-tested and selected always agree (the old app only rotated the picture).
//!
//! Text is never mirrored: a flip moves a text box and keeps its glyphs upright (its angle follows the mirrored
//! "up" direction). Images toggle their flip flags. Resizing scales a text's font size on a corner handle and
//! sets a fixed wrapping width on a side handle; math scales its font size. The box heights of text are measured
//! again by the text layer; this module only scales them.

use std::collections::{BTreeMap, BTreeSet};

use drafftink_geom::kurbo::{Affine, Point, Rect, Size, Vec2};
use drafftink_geom::select::{BoxHandle, Edge, MIN_RESIZE_SIZE, ResizeOpts, handle_point, resize_box};
use drafftink_geom::snap::snap_rotation;
use drafftink_geom::transform::{ElementFrame, map_box, normalize_angle, normalize_angle_signed, rotate_point};
use drafftink_geom::{OrientedBox, aabb};
use drafftink_scene::{
    Binding, Element, ElementError, ElementId, ElementKind, Kind, PathType, SceneRead, SceneTx, StoreError, limits,
    validate_element,
};

use crate::binding::{self, BindingError};
use crate::group::units::{Reverse, ReverseLookup, ensure_unlocked, primaries, with_companions};
use crate::target::TargetError;

mod common;
pub mod geometry;

pub use common::{CommonBounds, set_common_bounds, tidy};

/// Why elements cannot be rotated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotRotatableReason {
    /// A frame is among them.
    Frame,
    /// An elbow arrow is among them.
    ElbowArrow,
}

impl std::fmt::Display for NotRotatableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NotRotatableReason::Frame => "frames cannot be rotated",
            NotRotatableReason::ElbowArrow => {
                "elbow arrows cannot be rotated; change the arrow type to sharp or round first"
            }
        })
    }
}

/// Why a transform was refused. Nothing was changed.
#[derive(Debug, thiserror::Error)]
pub enum TransformError {
    /// An element does not exist, is deleted, or is locked.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// Frames and elbow arrows cannot be rotated.
    #[error("{reason} (elements: {})", crate::target::ids_text(.ids))]
    NotRotatable {
        /// The elements that cannot rotate.
        ids: Vec<ElementId>,
        /// Why not.
        reason: NotRotatableReason,
    },
    /// No element was given.
    #[error("no elements were given")]
    NoElements,
    /// The rotate handle was given to a resize.
    #[error("the rotate handle does not resize; use rotate")]
    RotateHandle,
    /// A requested box is not finite or has a negative size.
    #[error("bounds must be finite and the size 0 or more")]
    BadBounds,
    /// A requested size is zero or negative.
    #[error("the width and the height must be above 0")]
    NotPositive,
    /// An absolute angle needs one element.
    #[error("mode \"to\" sets an absolute angle and needs exactly one element; use mode \"by\" to rotate several")]
    RotateToNeedsOne,
    /// A number is not finite, or the result would leave the allowed range.
    #[error("the result is not a valid number; use smaller values")]
    NotFinite,
    /// The result would break a rule of the element.
    #[error("element {id} would become invalid: {error}")]
    Invalid {
        /// The element.
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
}

pub use crate::binding::End;

/// What a transform did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransformReport {
    /// Elements whose box, angle or points changed (companions included), in z-order.
    pub changed: Vec<ElementId>,
    /// Arrow ends that were unbound because their target was not transformed too.
    pub unbound: Vec<(ElementId, End)>,
    /// Arrows the binding solver re-routed.
    pub arrows: Vec<ElementId>,
    /// The common bounds of the transformed elements after the change.
    pub bounds: Option<Rect>,
}

impl TransformReport {
    /// Adds what `other` did to this report: ids once each, bounds of the later step.
    pub fn merge(&mut self, other: TransformReport) {
        for id in other.changed {
            if !self.changed.contains(&id) {
                self.changed.push(id);
            }
        }
        self.unbound.extend(other.unbound);
        for id in other.arrows {
            if !self.arrows.contains(&id) {
                self.arrows.push(id);
            }
        }
        self.bounds = other.bounds.or(self.bounds);
    }
}

/// How one element moves.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Motion {
    Translate(Vec2),
    Rotate {
        pivot: Point,
        delta: f64,
    },
    Affine(Affine),
    /// The box is set exactly (no rounding noise from the affine); `via` maps the old box onto the new one for
    /// everything that follows it.
    Exact {
        to: drafftink_scene::Transform,
        via: Affine,
    },
}

impl Motion {
    fn affine(self) -> Affine {
        match self {
            Motion::Translate(d) => Affine::translate(d),
            Motion::Rotate { pivot, delta } => drafftink_geom::transform::about(pivot, Affine::rotate(delta)),
            Motion::Affine(a) | Motion::Exact { via: a, .. } => a,
        }
    }

    /// True when ratios inside a box survive: no mirror, and equal scale on both axes.
    fn keeps_ratios(self) -> bool {
        match self {
            Motion::Translate(_) | Motion::Rotate { .. } => true,
            Motion::Affine(a) | Motion::Exact { via: a, .. } => {
                let [m00, m10, m01, m11, ..] = a.as_coeffs();
                let det = m00 * m11 - m01 * m10;
                let (sx, sy) = (m00.hypot(m10), m01.hypot(m11));
                det > 0.0 && (sx - sy).abs() <= 1e-9 * sx.max(sy).max(1.0)
            }
        }
    }
}

pub(crate) struct Item {
    pub id: ElementId,
    pub motion: Motion,
}

fn clean_angle(a: f64) -> f64 {
    let n = normalize_angle(a);
    if (1e-12..=std::f64::consts::TAU - 1e-12).contains(&n) { n } else { 0.0 }
}

/// Angle of a box that looks the same when turned by a half turn, reduced to `[-π/2, π/2)` and then to `[0, 2π)`.
fn symmetric_angle(a: f64) -> f64 {
    let mut s = normalize_angle_signed(a);
    if s >= std::f64::consts::FRAC_PI_2 {
        s -= std::f64::consts::PI;
    } else if s < -std::f64::consts::FRAC_PI_2 {
        s += std::f64::consts::PI;
    }
    clean_angle(s)
}

/// The angle that keeps the "up" direction of a box pointing where the affine sends it (glyphs stay upright).
fn upright_angle(a: Affine, angle: f64) -> f64 {
    let [m00, m10, m01, m11, ..] = a.as_coeffs();
    let (ux, uy) = (angle.sin(), -angle.cos());
    let (vx, vy) = (m00 * ux + m01 * uy, m10 * ux + m11 * uy);
    if vx.hypot(vy) <= 0.0 || !vx.is_finite() || !vy.is_finite() {
        return clean_angle(angle);
    }
    clean_angle(vx.atan2(-vy))
}

fn ratio(new: f64, old: f64) -> f64 {
    if old > 0.0 { new / old } else { 1.0 }
}

fn set_box(e: &mut Element, center: Point, size: Size, angle: f64) {
    e.transform.x = center.x - size.width / 2.0;
    e.transform.y = center.y - size.height / 2.0;
    e.transform.w = size.width;
    e.transform.h = size.height;
    e.transform.angle = angle;
}

/// The element after `motion`. `follower` is bound text that follows its container: it moves and turns but its
/// size is not scaled.
fn moved(e: &Element, motion: Motion, follower: bool) -> Result<Element, TransformError> {
    let mut n = e.clone();
    match motion {
        Motion::Translate(d) => {
            n.transform.x += d.x;
            n.transform.y += d.y;
        }
        Motion::Rotate { pivot, delta } => {
            let c = rotate_point(e.transform.center(), pivot, delta);
            n.transform.x = c.x - e.transform.w / 2.0;
            n.transform.y = c.y - e.transform.h / 2.0;
            n.transform.angle = clean_angle(e.transform.angle + delta);
        }
        Motion::Affine(a) => affine_element(&mut n, a, follower)?,
        Motion::Exact { to, via } => {
            if e.points().is_some() {
                affine_element(&mut n, via, follower)?;
            } else {
                n.transform = to;
            }
        }
    }
    Ok(n)
}

fn affine_element(n: &mut Element, a: Affine, follower: bool) -> Result<(), TransformError> {
    let old = geometry::obox(n);
    let (nb, mirrored) = map_box(&old, a).ok_or(TransformError::NotFinite)?;
    let tag = n.tag();
    match tag {
        Kind::Rectangle | Kind::Diamond | Kind::Ellipse => {
            let angle = if mirrored { symmetric_angle(nb.angle) } else { clean_angle(nb.angle) };
            set_box(n, nb.center, nb.size, angle);
        }
        Kind::Frame => set_box(n, nb.center, nb.size, 0.0),
        Kind::Text | Kind::Math => {
            let (kx, ky) = (ratio(nb.size.width, old.size.width), ratio(nb.size.height, old.size.height));
            let angle = upright_angle(a, n.transform.angle);
            let mut size = old.size;
            if !follower {
                let uniform = (kx - ky).abs() <= 1e-6 * kx.max(ky).max(1.0);
                let width_only = n.tag() == Kind::Text && (ky - 1.0).abs() <= 1e-9;
                if uniform || n.tag() == Kind::Math || !width_only {
                    let k = if uniform { (kx + ky) / 2.0 } else { (kx * ky).sqrt() };
                    scale_font(n, k);
                    size = Size::new(old.size.width * k, old.size.height * k);
                } else {
                    size = Size::new(nb.size.width, old.size.height);
                    if let ElementKind::Text(t) = &mut n.kind {
                        t.auto_resize = false;
                    }
                }
            }
            set_box(n, nb.center, size, angle);
        }
        Kind::Image => {
            let [m00, m10, m01, m11, ..] = a.as_coeffs();
            let (s, c) = n.transform.angle.sin_cos();
            let (fx, fy) = (if n.transform.flip_x { -1.0 } else { 1.0 }, if n.transform.flip_y { -1.0 } else { 1.0 });
            // L = A * R(angle) * F, the linear part of the whole image-to-world map.
            let (r00, r10, r01, r11) = (c * fx, s * fx, -s * fy, c * fy);
            let (l00, l10) = (m00 * r00 + m01 * r10, m10 * r00 + m11 * r10);
            let (l01, l11) = (m00 * r01 + m01 * r11, m10 * r01 + m11 * r11);
            let det = l00 * l11 - l01 * l10;
            let theta = l10.atan2(l00);
            let options: [(f64, bool, bool); 2] = if det >= 0.0 {
                [(theta, false, false), (theta + std::f64::consts::PI, true, true)]
            } else {
                [(theta, false, true), (theta + std::f64::consts::PI, true, false)]
            };
            let best = options
                .into_iter()
                .min_by(|x, y| normalize_angle_signed(x.0).abs().total_cmp(&normalize_angle_signed(y.0).abs()))
                .unwrap_or((theta, false, false));
            set_box(n, nb.center, nb.size, clean_angle(best.0));
            n.transform.flip_x = best.1;
            n.transform.flip_y = best.2;
        }
        Kind::Line | Kind::Arrow | Kind::Freehand => {
            let angle = if mirrored { symmetric_angle(nb.angle) } else { clean_angle(nb.angle) };
            let to_world = geometry::frame_of(n).to_world();
            let new_frame = ElementFrame::new(
                Point::new(nb.center.x - nb.size.width / 2.0, nb.center.y - nb.size.height / 2.0),
                nb.size,
                angle,
            );
            let to_local = new_frame.to_local();
            let map = |p: Point| to_local * (a * (to_world * p));
            match &mut n.kind {
                ElementKind::Line(d) | ElementKind::Arrow(d) => {
                    for p in &mut d.points {
                        *p = map(*p);
                    }
                    for s in &mut d.fixed_segments {
                        s.start = map(s.start);
                        s.end = map(s.end);
                    }
                }
                ElementKind::Freehand(d) => {
                    for p in &mut d.points {
                        *p = map(*p);
                    }
                }
                _ => {}
            }
            set_box(n, nb.center, nb.size, angle);
            if !n.refit_points() {
                return Err(TransformError::NotFinite);
            }
        }
    }
    Ok(())
}

fn scale_font(e: &mut Element, k: f64) {
    match &mut e.kind {
        ElementKind::Text(t) => t.font.size = (t.font.size * k).clamp(1.0, limits::MAX_FONT_SIZE),
        ElementKind::Math(m) => m.font_size = (m.font_size * k).clamp(1.0, limits::MAX_FONT_SIZE),
        _ => {}
    }
}

fn ratio_in_box(b: &OrientedBox, p: Point) -> [f64; 2] {
    let l = b.to_local(p);
    [ratio(l.x + b.size.width / 2.0, b.size.width), ratio(l.y + b.size.height / 2.0, b.size.height)]
}

/// Bindings whose target is transformed too are kept. After a mirror or a non-uniform scale their fixed point is
/// carried over to the new target box; the ends whose target stays are freed afterwards by the binding module.
fn carry_fixed_points(
    news: &mut BTreeMap<ElementId, Element>,
    olds: &BTreeMap<ElementId, Element>,
    motions: &BTreeMap<&ElementId, Motion>,
) {
    let carried = |b: &Binding, news: &BTreeMap<ElementId, Element>| -> Option<[f64; 2]> {
        let motion = motions.get(&b.target).filter(|m| !m.keeps_ratios())?;
        let (old_t, new_t) = (olds.get(&b.target)?, news.get(&b.target)?);
        let old_box = geometry::obox(old_t);
        let g = old_box.to_world(Point::new(
            (b.fixed_point[0] - 0.5) * old_box.size.width,
            (b.fixed_point[1] - 0.5) * old_box.size.height,
        ));
        Some(ratio_in_box(&geometry::obox(new_t), motion.affine() * g))
    };
    let fixes: Vec<(ElementId, [Option<[f64; 2]>; 2])> = news
        .iter()
        .filter_map(|(id, n)| {
            let d = n.linear_data()?;
            let fixes = [d.start_binding.as_ref(), d.end_binding.as_ref()].map(|b| carried(b?, news));
            fixes.iter().any(Option::is_some).then(|| (id.clone(), fixes))
        })
        .collect();
    for (id, [start, end]) in fixes {
        let Some(ElementKind::Line(d) | ElementKind::Arrow(d)) = news.get_mut(&id).map(|n| &mut n.kind) else {
            continue;
        };
        for (fixed, binding) in [(start, d.start_binding.as_mut()), (end, d.end_binding.as_mut())] {
            if let (Some(fp), Some(b)) = (fixed, binding) {
                b.fixed_point = fp;
            }
        }
    }
}

/// Applies the motions to the scene: companion text follows, bindings are kept or dropped, bound arrows are
/// re-solved. All new values are checked before the first write.
pub(crate) fn apply<T: SceneTx>(tx: &mut T, items: Vec<Item>) -> Result<TransformReport, TransformError> {
    let set: BTreeSet<ElementId> = items.iter().map(|i| i.id.clone()).collect();
    // Taken before the first write, while a fresh transaction can still answer from the committed index.
    let all_ids: Vec<ElementId> = set.iter().cloned().collect();
    let followers = binding::arrows_bound_to(&*tx, &all_ids);
    let mut olds: BTreeMap<ElementId, Element> = BTreeMap::new();
    for i in &items {
        let e = tx
            .get(&i.id)
            .filter(|e| !e.deleted)
            .ok_or_else(|| TransformError::Target(TargetError::NotFound(i.id.clone())))?;
        olds.insert(i.id.clone(), e.clone());
    }
    let motions: BTreeMap<&ElementId, Motion> = items.iter().map(|i| (&i.id, i.motion)).collect();
    let mut news: BTreeMap<ElementId, Element> = BTreeMap::new();
    for i in &items {
        let Some(old) = olds.get(&i.id) else { continue };
        let follower = old.container_id().is_some_and(|c| set.contains(c));
        news.insert(i.id.clone(), moved(old, i.motion, follower)?);
    }

    carry_fixed_points(&mut news, &olds, &motions);

    for (id, n) in &news {
        validate_element(n).map_err(|error| TransformError::Invalid { id: id.clone(), error })?;
    }
    let mut changed: Vec<ElementId> = Vec::new();
    for (id, n) in news {
        if olds.get(&id).is_some_and(|o| o.transform == n.transform && o.kind == n.kind) {
            continue;
        }
        let (t, k) = (n.transform, n.kind);
        tx.update(&id, |e| {
            e.transform = t;
            e.kind = k;
        })?;
        changed.push(id);
    }
    // The ends whose target is not part of the transform are freed where they are.
    let freed = binding::unbind_detached(tx, &all_ids)?;
    let mut unbound: Vec<(ElementId, End)> = Vec::new();
    for id in freed {
        let (Some(old), Some(now)) =
            (olds.get(&id).and_then(Element::linear_data), tx.get(&id).and_then(Element::linear_data))
        else {
            continue;
        };
        if old.start_binding.is_some() && now.start_binding.is_none() {
            unbound.push((id.clone(), End::Start));
        }
        if old.end_binding.is_some() && now.end_binding.is_none() {
            unbound.push((id.clone(), End::End));
        }
        if !changed.contains(&id) {
            changed.push(id);
        }
    }
    crate::group::units::sort_z(&*tx, &mut changed);
    let arrows =
        if changed.is_empty() { Vec::new() } else { binding::update_bound_arrows_from(tx, &changed, followers)? };
    let bounds = geometry::union_bounds(items.iter().filter_map(|i| tx.get(&i.id)));
    Ok(TransformReport { changed, unbound, arrows, bounds })
}

fn finite_vec(v: Vec2) -> Result<(), TransformError> {
    if v.x.is_finite() && v.y.is_finite() { Ok(()) } else { Err(TransformError::NotFinite) }
}

/// The primaries of `ids` and everything that travels with them.
struct Prepared {
    primary: Vec<ElementId>,
    all: Vec<ElementId>,
    /// Some primary element belongs to a group that counts (two live members).
    grouped: bool,
}

fn prepare<T: SceneTx>(tx: &T, ids: &[ElementId], frame_children: bool) -> Result<Prepared, TransformError> {
    if ids.is_empty() {
        return Err(TransformError::NoElements);
    }
    let rel = Reverse::of(tx);
    let base = primaries(tx, ids)?;
    let all = with_companions(tx, &rel, &base, frame_children);
    ensure_unlocked(tx, &all)?;
    // The primaries now include frame children (they are members, not followers).
    let primary: Vec<ElementId> =
        all.iter().filter(|i| tx.get(i).is_some_and(|e| !crate::group::units::is_label(tx, e))).cloned().collect();
    let grouped = primary.iter().filter_map(|i| tx.get(i)).any(|e| e.group_ids.iter().any(|g| rel.is_group(g)));
    Ok(Prepared { primary, all, grouped })
}

fn items(all: &[ElementId], motion: Motion) -> Vec<Item> {
    all.iter().map(|id| Item { id: id.clone(), motion }).collect()
}

/// Moves the elements by `delta`. Frames bring their children, text its container, groups all members.
pub fn move_by<T: SceneTx>(tx: &mut T, ids: &[ElementId], delta: Vec2) -> Result<TransformReport, TransformError> {
    finite_vec(delta)?;
    let p = prepare(&*tx, ids, true)?;
    apply(tx, items(&p.all, Motion::Translate(delta)))
}

/// A small move for the arrow keys. The same rules as [`move_by`]: an arrow nudged without its targets unbinds,
/// and the bindings that remain keep their fixed points.
pub fn nudge<T: SceneTx>(tx: &mut T, ids: &[ElementId], dx: f64, dy: f64) -> Result<TransformReport, TransformError> {
    move_by(tx, ids, Vec2::new(dx, dy))
}

/// Translates each group of elements by its own vector (align and distribute). Elements must be unlocked.
pub(crate) fn translate_each<T: SceneTx>(
    tx: &mut T,
    moves: &[(Vec<ElementId>, Vec2)],
) -> Result<TransformReport, TransformError> {
    let mut all = Vec::new();
    let mut any = false;
    for (ids, d) in moves {
        finite_vec(*d)?;
        any |= d.x != 0.0 || d.y != 0.0;
        // Units that stay still belong to the set too: arrows bound to them keep their bindings.
        all.extend(ids.iter().map(|id| Item { id: id.clone(), motion: Motion::Translate(*d) }));
    }
    if !any {
        return Ok(TransformReport::default());
    }
    apply(tx, all)
}

/// Applies one affine to the elements (flip). `frame_children` brings the children of selected frames.
pub(crate) fn affine_all<T: SceneTx>(
    tx: &mut T,
    ids: &[ElementId],
    make: impl FnOnce(Rect) -> Affine,
) -> Result<TransformReport, TransformError> {
    let p = prepare(&*tx, ids, true)?;
    let bounds = geometry::union_bounds(p.primary.iter().filter_map(|i| tx.get(i))).ok_or(TransformError::NotFinite)?;
    apply(tx, items(&p.all, Motion::Affine(make(bounds))))
}

/// Which handle of the selection box is dragged, and by how much.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeSpec {
    /// Corner or side handle (the rotate handle is refused).
    pub handle: BoxHandle,
    /// Total displacement of the handle since the start, in world axes.
    pub delta: Vec2,
    /// Keep the width to height ratio (forced for images, math, and for text on a corner).
    pub keep_aspect: bool,
    /// Resize about the centre.
    pub from_center: bool,
}

/// Resizes the common box of the elements. One element resizes in its own (rotated) frame. Several elements, and
/// groups, scale as a unit about the box of all of them; the ratio is kept when a rotated element, text, math, an
/// image or a group member takes part, because a non-uniform scale of those would shear them.
///
/// Lines, arrows and freehand scale their points. Dragging past the opposite side mirrors. The size never
/// drops below 1 unit. The membership of a resized frame is for [`crate::frame::recompute_after_resize`].
pub fn resize<T: SceneTx>(tx: &mut T, ids: &[ElementId], spec: ResizeSpec) -> Result<TransformReport, TransformError> {
    if spec.handle == BoxHandle::Rotate {
        return Err(TransformError::RotateHandle);
    }
    finite_vec(spec.delta)?;
    let base = primaries(&*tx, ids)?;
    let several = base.len() > 1;
    let p = prepare(&*tx, ids, several)?;
    let elems: Vec<&Element> = p.primary.iter().filter_map(|i| tx.get(i)).collect();
    let single = elems.len() == 1;
    let original = match (single, elems.first()) {
        (true, Some(e)) => geometry::obox(e),
        _ => OrientedBox::from_rect(geometry::union_bounds(elems.iter().copied()).ok_or(TransformError::NotFinite)?),
    };
    let forced = if single {
        elems.first().is_some_and(|e| match e.tag() {
            Kind::Image | Kind::Math => true,
            Kind::Text => matches!(spec.handle, BoxHandle::Corner(_) | BoxHandle::Edge(Edge::Top | Edge::Bottom)),
            _ => false,
        })
    } else {
        p.grouped
            || elems
                .iter()
                .any(|e| e.transform.angle != 0.0 || matches!(e.tag(), Kind::Text | Kind::Math | Kind::Image))
    };
    let target = handle_point(&original, spec.handle, 0.0) + spec.delta;
    let opts = ResizeOpts {
        keep_aspect: spec.keep_aspect || forced,
        from_center: spec.from_center,
        min_size: MIN_RESIZE_SIZE,
    };
    let res = resize_box(&original, spec.handle, target, opts).ok_or(TransformError::NotFinite)?;
    let mut report = apply(tx, items(&p.all, Motion::Affine(res.affine(&original))))?;
    report.bounds = Some(res.obox.aabb());
    Ok(report)
}

/// A target for [`set_bounds`]; `None` keeps the current value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoundsSpec {
    /// Left edge of the unrotated box.
    pub x: Option<f64>,
    /// Top edge of the unrotated box.
    pub y: Option<f64>,
    /// Width, 0 or more.
    pub w: Option<f64>,
    /// Height, 0 or more.
    pub h: Option<f64>,
}

/// Sets the unrotated box of one element (`element.set_bounds`). Points of lines and freehand scale with it, a
/// rotated element keeps its angle and scales in its own frame.
pub fn set_bounds<T: SceneTx>(tx: &mut T, id: &ElementId, spec: BoundsSpec) -> Result<TransformReport, TransformError> {
    let p = prepare(&*tx, std::slice::from_ref(id), false)?;
    let e =
        tx.get(id).filter(|e| !e.deleted).ok_or_else(|| TransformError::Target(TargetError::NotFound(id.clone())))?;
    let old = geometry::obox(e);
    let t = e.transform;
    let (x, y) = (spec.x.unwrap_or(t.x), spec.y.unwrap_or(t.y));
    let (w, h) = (spec.w.unwrap_or(t.w), spec.h.unwrap_or(t.h));
    if ![x, y, w, h].iter().all(|v| v.is_finite()) || w < 0.0 || h < 0.0 {
        return Err(TransformError::BadBounds);
    }
    let new = OrientedBox::from_xywh(x, y, w, h, t.angle);
    let a = new.to_world_affine() * Affine::scale_non_uniform(ratio(w, t.w), ratio(h, t.h)) * old.to_local_affine();
    let to = drafftink_scene::Transform { x, y, w, h, ..t };
    let mut moves = items(&p.all, Motion::Affine(a));
    for m in &mut moves {
        if m.id == *id {
            m.motion = Motion::Exact { to, via: a };
        }
    }
    let mut report = apply(tx, moves)?;
    report.bounds = Some(new.aabb());
    Ok(report)
}

/// Absolute or relative rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateMode {
    /// Turn by the angle.
    By,
    /// Set the angle (one element only).
    To,
}

/// Rotates the elements about the common centre of their bounds (one element about its own centre). Every
/// member's own `angle` changes, so a group rotates for real.
///
/// `snap_step` (radians) rounds the turn for `By`, and the final angle for `To`. Frames and elbow arrows refuse
/// ([`TransformError::NotRotatable`]); the whole call is refused, nothing is skipped.
pub fn rotate<T: SceneTx>(
    tx: &mut T,
    ids: &[ElementId],
    angle: f64,
    mode: RotateMode,
    snap_step: Option<f64>,
) -> Result<TransformReport, TransformError> {
    if !angle.is_finite() {
        return Err(TransformError::NotFinite);
    }
    let p = prepare(&*tx, ids, false)?;
    let blocked: Vec<ElementId> = p
        .primary
        .iter()
        .filter(|i| {
            tx.get(i)
                .is_some_and(|e| e.tag() == Kind::Frame || e.linear_data().is_some_and(|d| d.path == PathType::Elbow))
        })
        .cloned()
        .collect();
    if !blocked.is_empty() {
        let reason = if blocked.iter().any(|i| tx.get(i).is_some_and(|e| e.tag() == Kind::Frame)) {
            NotRotatableReason::Frame
        } else {
            NotRotatableReason::ElbowArrow
        };
        return Err(TransformError::NotRotatable { ids: blocked, reason });
    }
    let elems: Vec<&Element> = p.primary.iter().filter_map(|i| tx.get(i)).collect();
    let step = snap_step.filter(|s| s.is_finite() && *s > 0.0);
    let delta = match mode {
        RotateMode::By => match step {
            Some(s) => (angle / s).round() * s,
            None => angle,
        },
        RotateMode::To => {
            let [only] = elems.as_slice() else {
                return Err(TransformError::RotateToNeedsOne);
            };
            let target = match step {
                Some(s) => snap_rotation(angle, s),
                None => normalize_angle(angle),
            };
            normalize_angle_signed(target - only.transform.angle)
        }
    };
    let pivot = match elems.as_slice() {
        [only] => only.transform.center(),
        _ => aabb::center(&geometry::union_bounds(elems.iter().copied()).ok_or(TransformError::NotFinite)?),
    };
    if delta == 0.0 {
        return Ok(TransformReport::default());
    }
    apply(tx, items(&p.all, Motion::Rotate { pivot, delta }))
}

/// The common bounds of the elements in world space (rotation included), for overlays and badges.
pub fn common_bounds<R: SceneRead>(scene: &R, ids: &[ElementId]) -> Option<Rect> {
    geometry::union_bounds(ids.iter().filter_map(|i| scene.get(i)).filter(|e| !e.deleted))
}
