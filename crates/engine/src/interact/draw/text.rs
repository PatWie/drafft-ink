//! The text and math tools: a click starts or opens an edit.

use drafftink_geom::kurbo::Point;
use drafftink_scene::{ElementId, Kind};
use serde_json::json;

use crate::interact::ctx::Ctx;
use crate::interact::events::Ev;
use crate::interact::outcome::UiRequest;
use crate::state::Tool;
use drafftink_canvas::Target;

/// A click with the text or math tool.
pub(super) fn click(cx: &mut Ctx<'_>, ev: &Ev<'_>) {
    let at = ev.at.world;
    match ev.target {
        // A handle belongs to the selection; the select machine has already used the press.
        Target::Handle(_) | Target::None => {}
        Target::Element { id, .. } => match edit_kind(cx, id) {
            Some(kind) => edit(cx, id, kind, at),
            None => place(cx, at),
        },
        Target::Empty => place(cx, at),
    }
}

/// How an existing element is edited with the text or math tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Editor {
    /// The text editor over the element.
    Text,
    /// The equation dialog.
    Equation,
}

/// The editor an element opens, if the tool edits it. The text tool edits text, shapes and arrows; the math
/// tool edits formulas. Anything else is "not editable here", and the click places something new.
fn edit_kind(cx: &Ctx<'_>, id: &ElementId) -> Option<Editor> {
    let kind = cx.s.element(id)?.tag();
    match (cx.tool(), kind) {
        (Tool::Text, Kind::Text | Kind::Rectangle | Kind::Diamond | Kind::Ellipse | Kind::Arrow) => Some(Editor::Text),
        (Tool::Text | Tool::Math, Kind::Math) => Some(Editor::Equation),
        _ => None,
    }
}

/// `text.edit_begin` on an existing element, then the editor.
fn edit(cx: &mut Ctx<'_>, id: &ElementId, editor: Editor, at: Point) {
    if cx.exec("text.edit_begin", json!({ "id": id.as_str() })).is_none() {
        return;
    }
    cx.out.ui_request = Some(match editor {
        Editor::Text => UiRequest::OpenTextEditor { element: Some(id.clone()), at },
        Editor::Equation => UiRequest::OpenEquationEditor { element: id.clone() },
    });
}

/// A new text draft (text tool) or a new formula (math tool) at `at`.
pub(super) fn place(cx: &mut Ctx<'_>, at: Point) {
    match cx.tool() {
        Tool::Math => {
            // The placeholder formula is the command's default; no editor opens (the old behaviour).
            cx.exec("element.create", json!({ "kind": "math", "at": [at.x, at.y], "from_tool": true }));
        }
        _ => {
            if cx.exec("text.edit_begin", json!({ "at": [at.x, at.y] })).is_some() {
                cx.out.ui_request = Some(UiRequest::OpenTextEditor { element: None, at });
            }
        }
    }
}
