//! Setting the common bounds of several elements (`element.set_bounds`), and rounding the noise a resize leaves.

use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_geom::select::{BoxHandle, Corner};
use drafftink_scene::{ElementId, ElementKind, SceneTx, StoreError};

use super::{BoundsSpec, ResizeSpec, TransformError, TransformReport, common_bounds, move_by, resize, set_bounds};
use crate::group::units::primaries;

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
        let t = tx.get(only).map(|e| e.transform).ok_or_else(|| TransformError::NotFound(only.clone()))?;
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
        report.merge(r);
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
        report.merge(r);
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

/// Smallest unit that [`tidy`] keeps: 1e-9 world units.
const TIDY: f64 = 1.0e9;

fn tidy_value(v: f64) -> f64 {
    let r = (v * TIDY).round() / TIDY;
    if r.is_finite() { r } else { v }
}

/// Rounds the geometry of `ids` (box and points) to 1e-9 world units. A resize composes several affine
/// maps, and the result of dragging a box to 140 wide would otherwise be 139.99999999999915, which shows up in
/// a numeric panel and breaks grid alignment. Nothing is rounded that is visible at any zoom.
pub fn tidy<T: SceneTx>(tx: &mut T, ids: &[ElementId]) -> Result<(), StoreError> {
    for id in ids {
        let Some(e) = tx.get(id).filter(|e| !e.deleted) else { continue };
        let mut n = e.clone();
        n.transform.x = tidy_value(n.transform.x);
        n.transform.y = tidy_value(n.transform.y);
        n.transform.w = tidy_value(n.transform.w);
        n.transform.h = tidy_value(n.transform.h);
        match &mut n.kind {
            ElementKind::Line(d) | ElementKind::Arrow(d) => {
                for p in &mut d.points {
                    *p = Point::new(tidy_value(p.x), tidy_value(p.y));
                }
            }
            ElementKind::Freehand(d) => {
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
