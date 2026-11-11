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
        Target::Handle(_) | Target::None | Target::LinkIcon { .. } => {}
        Target::Element { id, .. } => match edit_kind(cx, id) {
            Some(kind) => edit(cx, id, kind, at),
            None => place(cx, at, None),
        },
        Target::Empty => place(cx, at, None),
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
        Editor::Text => UiRequest::OpenTextEditor { element: Some(id.clone()), at, width: None },
        Editor::Equation => UiRequest::OpenEquationEditor { element: id.clone() },
    });
}

/// A drag with the text tool ended: a text box as wide as the drag, with its top-left at the drag's top-left.
/// A drag narrower than [`MIN_BOX_PX`] screen pixels is a click that moved a little, and places at the origin.
pub(super) fn drag_box(cx: &mut Ctx<'_>, origin: Point, end: Point) {
    let width = (end.x - origin.x).abs();
    if cx.tool() != Tool::Text || width < cx.px(MIN_BOX_PX) {
        place(cx, origin, None);
        return;
    }
    place(cx, Point::new(origin.x.min(end.x), origin.y.min(end.y)), Some(width));
}

/// The narrowest text box a drag makes, in screen pixels.
const MIN_BOX_PX: f64 = 20.0;

/// A new text draft (text tool) or a new formula (math tool) at `at`; `width` fixes the width of the text.
pub(super) fn place(cx: &mut Ctx<'_>, at: Point, width: Option<f64>) {
    match cx.tool() {
        Tool::Math => {
            // The placeholder formula is the command's default; no editor opens (the old behaviour).
            cx.exec("element.create", json!({ "kind": "math", "at": [at.x, at.y], "from_tool": true }));
        }
        _ => {
            let mut args = json!({ "at": [at.x, at.y] });
            if let Some(w) = width {
                args["width"] = json!(w);
            }
            if cx.exec("text.edit_begin", args).is_some() {
                cx.out.ui_request = Some(UiRequest::OpenTextEditor { element: None, at, width });
            }
        }
    }
}
