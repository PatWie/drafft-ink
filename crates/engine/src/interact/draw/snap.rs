//! Snapping a point that a drawing tool places: grid, smart guides and angle lock.

use drafftink_edit::arrow::{PointSnap, SnapFlags, snap_dragged_point};
use drafftink_geom::OrientedBox;
use drafftink_geom::aabb;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_geom::snap::{GRID_SIZE, MAX_SNAP_CANDIDATES, MULTI_MOVE_SNAP_RADIUS, snap_to_grid};

use crate::interact::ctx::Ctx;

/// The default angle step, in degrees, when the setting is missing.
const DEFAULT_ANGLE_STEP: f64 = 15.0;

/// Snapping that is on for this placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Snaps {
    /// The setting `snap.grid`.
    pub(super) grid: bool,
    /// The setting `snap.smart_guides`.
    pub(super) smart: bool,
    /// Shift is held, or the setting `snap.angle` is on.
    pub(super) angle: bool,
}

impl Snaps {
    /// The snapping for a box corner: grid and guides, no angle.
    pub(super) fn corner(cx: &Ctx<'_>) -> Self {
        Self { grid: cx.setting("snap.grid", false), smart: cx.setting("snap.smart_guides", false), angle: false }
    }

    /// The snapping for a line or arrow point: grid, guides and the angle lock.
    pub(super) fn line(cx: &Ctx<'_>) -> Self {
        Self { angle: cx.mods.shift || cx.setting("snap.angle", false), ..Self::corner(cx) }
    }
}

/// The start of a drag snaps to the grid only, as in the old app.
pub(super) fn start(cx: &Ctx<'_>, p: Point) -> Point {
    if cx.setting("snap.grid", false) { snap_to_grid(p, GRID_SIZE).point } else { p }
}

/// Snaps a placed point. `reference` is the neighbouring point that the angle lock turns about.
pub(super) fn point(cx: &Ctx<'_>, raw: Point, reference: Option<Point>, snaps: Snaps) -> PointSnap {
    if !(snaps.grid || snaps.smart || snaps.angle) {
        return PointSnap { point: raw, guides: Vec::new(), angle: None };
    }
    let others = if snaps.smart { candidates(cx, raw) } else { Vec::new() };
    let step = cx.s.settings.f64_or("snap.angle_step", DEFAULT_ANGLE_STEP);
    let flags = SnapFlags { grid: snaps.grid, smart: snaps.smart, angle: snaps.angle };
    snap_dragged_point(raw, reference, &others, flags, step)
}

/// The boxes of the elements near `around`, from the spatial index (never a scan of the document).
fn candidates(cx: &Ctx<'_>, around: Point) -> Vec<Rect> {
    if !drafftink_geom::point_is_finite(around) {
        return Vec::new();
    }
    let zone = Rect::from_center_size(around, (MULTI_MOVE_SNAP_RADIUS * 2.0, MULTI_MOVE_SNAP_RADIUS * 2.0));
    cx.world
        .index()
        .in_rect(cx.s.store(), &zone)
        .into_iter()
        .map(|e| {
            let t = &e.transform;
            OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle).aabb()
        })
        .filter(aabb::is_finite)
        .take(MAX_SNAP_CANDIDATES)
        .collect()
}
