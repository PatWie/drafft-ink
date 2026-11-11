//! The select tool: click, marquee, move, resize, rotate, line points, double-click. (Filled in by the
//! selection and transform part of the interaction layer.)

use crate::interact::ctx::Ctx;
use crate::interact::events::{Ev, Key};

/// State of the select tool between pointer events.
#[derive(Debug, Default)]
pub(crate) struct SelectTool {}

#[allow(clippy::unused_self)]
impl SelectTool {
    pub(crate) fn press(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn click(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn double_click(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn drag_start(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn drag_update(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}
    pub(crate) fn drag_end(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>, _cancelled: bool) {}
    /// Drops whatever is running without a trace (gesture cancelled, marquee dropped).
    pub(crate) fn cancel(&mut self, _cx: &mut Ctx<'_>) {}
    /// Returns true when the key was used.
    pub(crate) fn key(&mut self, _cx: &mut Ctx<'_>, _key: Key) -> bool {
        false
    }
    /// True while a drag of the select tool is running.
    pub(crate) fn is_active(&self) -> bool {
        false
    }
}
