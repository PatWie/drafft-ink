//! Domain rules the `element` commands need that no other module of this crate owns: setting the common
//! bounds of several elements, and the elements hit by an eraser path.

use std::collections::BTreeSet;

use drafftink_geom::kurbo::{Point, Rect, Vec2};
use drafftink_geom::select::{BoxHandle, Corner};
use drafftink_scene::{ElementId, SceneRead, SceneTx};

use crate::group::units::{Relations, ensure_unlocked, primaries, sort_z, with_companions};
use crate::query::{AtOpts, elements_at};
use crate::transform::{
    BoundsSpec, ResizeSpec, TransformError, TransformReport, common_bounds, move_by, resize, set_bounds,
};

/// What `element.set_bounds` asks for. `None` keeps the current value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CommonBounds {
    /// Left edge (the centre x with `center`).
    pub x: Option<f64>,
    /// Top edge (the centre y with `center`).
    pub y: Option<f64>,
    /// Width, above 0.
    pub w: Option<f64>,
    /// Height, above 0.
    pub h: Option<f64>,
    /// `x` and `y` name the centre of the box instead of its top-left corner.
    pub center: bool,
    /// When only one of `w` and `h` is given, the other follows the current ratio.
    pub keep_aspect: bool,
}

/// Sets the position and/or size of the common bounds of `ids`. One element gets exactly that box (its
/// unrotated box, see [`set_bounds`]). Several elements scale as a unit with the rules of
/// [`crate::transform::resize`] (the size can end up larger than asked when a rule forces the ratio), then move so
/// that the result sits at the requested place.
pub fn set_common_bounds<T: SceneTx>(
    tx: &mut T,
    ids: &[ElementId],
    spec: CommonBounds,
) -> Result<TransformReport, TransformError> {
    let base = primaries(&*tx, ids)?;
    if [spec.x, spec.y, spec.w, spec.h].iter().flatten().any(|v| !v.is_finite()) {
        return Err(TransformError::NotFinite);
    }
    if spec.w.is_some_and(|w| w <= 0.0) || spec.h.is_some_and(|h| h <= 0.0) {
        return Err(TransformError::BadArgs("the width and the height must be above 0"));
    }
    if let [only] = base.as_slice() {
        let t = tx.get(only).map(|e| e.transform).ok_or(TransformError::NotFinite)?;
        let (w, h) = target_size(t.w, t.h, &spec);
        let (x, y) = if spec.center {
            (spec.x.unwrap_or(t.x + t.w / 2.0) - w / 2.0, spec.y.unwrap_or(t.y + t.h / 2.0) - h / 2.0)
        } else {
            (spec.x.unwrap_or(t.x), spec.y.unwrap_or(t.y))
        };
        return set_bounds(tx, only, BoundsSpec { x: Some(x), y: Some(y), w: Some(w), h: Some(h) });
    }
    let old = common_bounds(&*tx, &base).ok_or(TransformError::NotFinite)?;
    let (ow, oh) = (old.width(), old.height());
    let (w, h) = target_size(ow, oh, &spec);
    let mut report = TransformReport::default();
    if (w - ow).abs() > 1e-9 || (h - oh).abs() > 1e-9 {
        let r = resize(
            tx,
            &base,
            ResizeSpec {
                handle: BoxHandle::Corner(Corner::BottomRight),
                delta: Vec2::new(w - ow, h - oh),
                keep_aspect: spec.keep_aspect,
                from_center: false,
            },
        )?;
        merge(&mut report, r);
    }
    let now = common_bounds(&*tx, &base).ok_or(TransformError::NotFinite)?;
    let (cx, cy) = (
        spec.x.unwrap_or(if spec.center { old.center().x } else { old.x0 }),
        spec.y.unwrap_or(if spec.center { old.center().y } else { old.y0 }),
    );
    let (x0, y0) = if spec.center { (cx - now.width() / 2.0, cy - now.height() / 2.0) } else { (cx, cy) };
    let d = Vec2::new(x0 - now.x0, y0 - now.y0);
    if d.x != 0.0 || d.y != 0.0 {
        let r = move_by(tx, &base, d)?;
        merge(&mut report, r);
    }
    report.bounds = common_bounds(&*tx, &base);
    Ok(report)
}

fn target_size(ow: f64, oh: f64, spec: &CommonBounds) -> (f64, f64) {
    match (spec.w, spec.h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) if spec.keep_aspect && ow > 0.0 => (w, oh * w / ow),
        (None, Some(h)) if spec.keep_aspect && oh > 0.0 => (ow * h / oh, h),
        (w, h) => (w.unwrap_or(ow), h.unwrap_or(oh)),
    }
}

fn merge(into: &mut TransformReport, r: TransformReport) {
    for id in r.changed {
        if !into.changed.contains(&id) {
            into.changed.push(id);
        }
    }
    into.unbound.extend(r.unbound);
    for id in r.arrows {
        if !into.arrows.contains(&id) {
            into.arrows.push(id);
        }
    }
    into.bounds = r.bounds.or(into.bounds);
}

/// The smallest number of samples per radius along an eraser stroke. Two samples per radius keep a stroke from
/// skipping a thin element.
const SAMPLES_PER_RADIUS: f64 = 2.0;
/// No path is sampled with more points than this.
const MAX_SAMPLES: usize = 20_000;

/// The unlocked elements hit by a stroke of `radius` along `path` (world units), by exact hit testing. Group
/// members are hit as themselves; a bound text counts as its container. A hit that has a locked member in its
/// closure (its bound text, or the children of a frame) is left out. The result is in z-order.
pub fn hit_by_path<R: SceneRead>(scene: &R, path: &[Point], radius: f64) -> Vec<ElementId> {
    let radius = if radius.is_finite() { radius.max(0.0) } else { 0.0 };
    let opts = AtOpts { tolerance: radius, all: true, deep: true, include_locked: false };
    let mut hit: BTreeSet<ElementId> = BTreeSet::new();
    let step = (radius / SAMPLES_PER_RADIUS).max(0.5);
    let mut samples = 0usize;
    let probe = |p: Point, hit: &mut BTreeSet<ElementId>| {
        for h in elements_at(scene, p, &opts) {
            hit.insert(h.id);
        }
    };
    for (i, p) in path.iter().enumerate() {
        probe(*p, &mut hit);
        let Some(next) = path.get(i + 1) else { continue };
        let len = p.distance(*next);
        if !len.is_finite() {
            continue;
        }
        let n = (len / step).ceil();
        let n = if n.is_finite() { n.min(MAX_SAMPLES as f64) as usize } else { 0 };
        for k in 1..n {
            samples += 1;
            if samples > MAX_SAMPLES {
                break;
            }
            let t = k as f64 / n as f64;
            probe(p.lerp(*next, t), &mut hit);
        }
    }
    // A hit whose whole closure (bound text, frame children) is not unlocked is skipped, never half-deleted.
    let rel = Relations::build(scene);
    let mut out: Vec<ElementId> = hit
        .into_iter()
        .filter(|id| ensure_unlocked(scene, &with_companions(scene, &rel, std::slice::from_ref(id), true)).is_ok())
        .collect();
    sort_z(scene, &mut out);
    out
}

/// The rectangle of a point list, for tests and overlays.
pub fn path_bounds(path: &[Point]) -> Option<Rect> {
    drafftink_geom::aabb::from_points(path.iter().copied())
}

/// Smallest unit that [`tidy`] keeps: 1e-9 world units.
const TIDY: f64 = 1.0e9;

fn tidy_value(v: f64) -> f64 {
    let r = (v * TIDY).round() / TIDY;
    if r.is_finite() { r } else { v }
}

/// Rounds the geometry of `ids` (box and points) to 1e-9 world units. A resize composes several affine
/// maps, and the result of dragging a box to 140 wide would otherwise be 139.99999999999915, which shows up in
/// a numeric panel and breaks grid alignment. Nothing is rounded that is visible at any zoom.
pub fn tidy<T: SceneTx>(tx: &mut T, ids: &[ElementId]) -> Result<(), drafftink_scene::StoreError> {
    for id in ids {
        let Some(e) = tx.get(id).filter(|e| !e.deleted) else { continue };
        let mut n = e.clone();
        n.transform.x = tidy_value(n.transform.x);
        n.transform.y = tidy_value(n.transform.y);
        n.transform.w = tidy_value(n.transform.w);
        n.transform.h = tidy_value(n.transform.h);
        match &mut n.kind {
            drafftink_scene::ElementKind::Line(d) | drafftink_scene::ElementKind::Arrow(d) => {
                for p in &mut d.points {
                    *p = Point::new(tidy_value(p.x), tidy_value(p.y));
                }
            }
            drafftink_scene::ElementKind::Freehand(d) => {
                for p in &mut d.points {
                    *p = Point::new(tidy_value(p.x), tidy_value(p.y));
                }
            }
            _ => {}
        }
        if n.transform != e.transform || n.kind != e.kind {
            let (t, k) = (n.transform, n.kind);
            tx.update(id, |slot| {
                slot.transform = t;
                slot.kind = k;
            })?;
        }
    }
    Ok(())
}
