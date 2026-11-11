//! The pointer events the tool machines receive, as one borrowed shape.

use drafftink_canvas::{Meta, Pos, Target};
use drafftink_geom::kurbo::Vec2;

/// One pointer event as the tool machines see it.
///
/// For `press`, `click` and `double_click` the origin equals `at` and the deltas are zero. For the drag methods
/// `origin` is where the pointer went down, `at` is where it is now, and the deltas are totals since the press.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ev<'a> {
    /// Where the press happened.
    pub(crate) origin: Pos,
    /// Where the pointer is.
    pub(crate) at: Pos,
    /// Total movement in screen pixels since the press.
    pub(crate) delta_screen: Vec2,
    /// Total movement in world units since the press.
    pub(crate) delta_world: Vec2,
    /// What was under the press.
    pub(crate) target: &'a Target,
    /// Pointer details (modifiers, pressure, time).
    pub(crate) meta: &'a Meta,
}

/// A key the interaction layer reacts to. Other keys are commands (`docs/commands.md` section 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Enter: ends a multi-point line.
    Enter,
    /// Escape: cancels the running pointer gesture, or ends a multi-point line.
    Escape,
}
