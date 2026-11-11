//! What a call into [`Interaction`](super::Interaction) tells the shell.

use drafftink_canvas::Cursor;
use drafftink_geom::kurbo::Point;
use drafftink_scene::ElementId;

use crate::refusal::Refusal;
use crate::spec::Redraw;

/// What the shell has to do after an input event.
///
/// * `redraw` is the strongest reason the interaction layer itself has to draw again (overlay preview, hover
///   outline, camera). A command that ran raises the session's own dirty reason as well: the shell draws when
///   either says so (`Session::take_dirty`).
/// * `cursor` is set only when the mouse cursor changed.
/// * `ui_request` asks the shell for a piece of UI that is not a command (a menu, an editor).
/// * `refusals` are the refusals of the commands the interaction ran for the user, in plain words. A shell shows
///   them as toasts (`QUIET_FROM_KEYBOARD` codes aside).
/// * `handled` is true when a key press was consumed (see [`Interaction::key`](super::Interaction::key)).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// The strongest redraw the interaction state needs, if any.
    pub redraw: Option<Redraw>,
    /// The new mouse cursor, when it changed.
    pub cursor: Option<Cursor>,
    /// UI to open.
    pub ui_request: Option<UiRequest>,
    /// Refusals of the commands that ran.
    pub refusals: Vec<Refusal>,
    /// A key press was used by the interaction layer; the shell must not run its own binding for it.
    pub handled: bool,
}

impl Outcome {
    /// Asks for at least `level`.
    pub(crate) fn redraw_at_least(&mut self, level: Redraw) {
        if level > Redraw::None && self.redraw.is_none_or(|r| r < level) {
            self.redraw = Some(level);
        }
    }

    /// Folds `later` into this outcome: the stronger redraw, the newest cursor and request, all refusals.
    pub(crate) fn absorb(&mut self, later: Outcome) {
        if let Some(r) = later.redraw {
            self.redraw_at_least(r);
        }
        if later.cursor.is_some() {
            self.cursor = later.cursor;
        }
        if later.ui_request.is_some() {
            self.ui_request = later.ui_request;
        }
        self.refusals.extend(later.refusals);
        self.handled |= later.handled;
    }
}

/// UI the shell should open because of a pointer event. None of these change the document.
#[derive(Debug, Clone, PartialEq)]
pub enum UiRequest {
    /// A text editor over `element` (an existing text, math, shape or arrow label), or over a new draft at
    /// `at` when `element` is `None`. Sent after `text.edit_begin` succeeded.
    OpenTextEditor {
        /// The element being edited, `None` for a draft that does not exist yet.
        element: Option<ElementId>,
        /// World position of the click (the top-left of a draft).
        at: Point,
    },
    /// The equation dialog for a math element (`text.edit_begin` ran first).
    OpenEquationEditor {
        /// The math element.
        element: ElementId,
    },
    /// The context menu (`ui.open_context_menu`) at a screen position.
    OpenContextMenu {
        /// Logical pixels from the top-left of the canvas.
        at: Point,
        /// What the menu is about.
        target: ContextTarget,
    },
}

/// What a context menu was opened on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextTarget {
    /// Empty canvas.
    Canvas,
    /// The current selection (the element under the pointer was selected first when it was not).
    Selection,
}
