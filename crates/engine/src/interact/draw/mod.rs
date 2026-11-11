//! The drawing tools: shapes, lines and arrows, freehand, eraser, laser, text and math. (Filled in by the
//! drawing part of the interaction layer.)

use drafftink_geom::kurbo::Point;

use crate::interact::ctx::Ctx;
use crate::interact::events::{Ev, Key};

/// State of the drawing tools between pointer events.
#[derive(Debug, Default)]
pub(crate) struct DrawTools {}

#[allow(clippy::unused_self)]
impl DrawTools {
    pub(crate) fn press(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn click(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn double_click(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn drag_start(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn drag_update(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn drag_end(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>, _cancelled: bool) {}
    /// The pointer moved with no button down: rubber band of a multi-point line, binding candidate.
    pub(crate) fn pointer_moved(&mut self, _cx: &mut Ctx<'_>, _screen: Point, _world: Point) {}
    /// Drops the preview and any draft without a trace.
    pub(crate) fn cancel(&mut self, _cx: &mut Ctx<'_>) {}
    /// Returns true when the key was used.
    pub(crate) fn key(&mut self, _cx: &mut Ctx<'_>, _key: Key) -> bool {
        false
    }
    /// Time passes: ages the laser trail.
    pub(crate) fn tick(&mut self, _cx: &mut Ctx<'_>) {}
    /// When `tick` next has something to do (the laser fade), if ever.
    pub(crate) fn deadline(&self) -> Option<f64> {
        None
    }
    /// True while a drawing is in progress (a drag, or a multi-point line draft).
    pub(crate) fn is_active(&self) -> bool {
        false
    }
}
