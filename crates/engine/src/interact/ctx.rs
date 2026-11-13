//! What the tool machines get to work with while they handle one event.

use std::any::Any;

use drafftink_canvas::Modifiers;
use serde_json::Value;

use super::bind;
use super::outcome::Outcome;
use super::overlay::OverlayParts;
use super::world::World;
use crate::cmd::arrow::BindChoice;
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::Redraw;
use crate::state::Tool;

/// The session, the world picture, the overlay state and the outcome, for the duration of one event.
///
/// A tool machine changes the document only through [`Ctx::exec`], [`Ctx::exec_typed`] and the gesture methods
/// of `Session` (`docs/exceptions.md`). It writes its previews into [`Ctx::parts`] and says so with
/// [`Ctx::overlay_changed`].
pub(crate) struct Ctx<'a> {
    /// The session.
    pub(crate) s: &'a mut Session,
    /// Viewport, index and selection frame.
    pub(crate) world: &'a World,
    /// The overlay state that machines own (marquee, preview, guides, ...).
    pub(crate) parts: &'a mut OverlayParts,
    /// The outcome being built.
    pub(crate) out: &'a mut Outcome,
    /// Time of the event on the caller's clock.
    pub(crate) now_ms: f64,
    /// Modifier keys at the event.
    pub(crate) mods: Modifiers,
}

impl Ctx<'_> {
    /// Runs a command for the user. A refusal goes to the outcome and `None` comes back.
    pub(crate) fn exec(&mut self, id: &str, args: Value) -> Option<Value> {
        match self.s.execute_from(Origin::Ui, id, args) {
            Ok(v) => Some(v),
            Err(r) => {
                self.out.refusals.push(r);
                None
            }
        }
    }

    /// Runs a command with its argument struct. A refusal goes to the outcome and `None` comes back.
    pub(crate) fn exec_typed<A: Any>(&mut self, id: &str, args: &A) -> Option<Value> {
        match self.s.execute_typed(Origin::Ui, id, args) {
            Ok(v) => Some(v),
            Err(r) => {
                self.out.refusals.push(r);
                None
            }
        }
    }

    /// Runs a command and returns its refusal instead of recording it, for callers that decide themselves.
    pub(crate) fn try_exec(&mut self, id: &str, args: Value) -> Result<Value, Refusal> {
        self.s.execute_from(Origin::Ui, id, args)
    }

    /// The overlay has to be drawn again.
    pub(crate) fn overlay_changed(&mut self) {
        self.out.redraw_at_least(Redraw::Overlay);
    }

    /// The tool in effect (the held one, else the active one).
    pub(crate) fn tool(&self) -> Tool {
        self.s.tool.effective()
    }

    /// A boolean setting.
    pub(crate) fn setting(&self, key: &str, default: bool) -> bool {
        self.s.settings.bool_or(key, default)
    }

    /// How a gesture with these keys held binds an arrow end: the setting `arrow.binding`, turned around by Ctrl
    /// or Cmd, and Alt for inside (see [`bind::choice`]).
    pub(crate) fn bind_choice(&self, mods: Modifiers) -> BindChoice {
        bind::choice(self.setting("arrow.binding", true), mods)
    }

    /// Pixels per world unit.
    pub(crate) fn zoom(&self) -> f64 {
        self.world.zoom()
    }

    /// World units for a length in screen pixels.
    pub(crate) fn px(&self, px: f64) -> f64 {
        self.world.px(px)
    }
}
