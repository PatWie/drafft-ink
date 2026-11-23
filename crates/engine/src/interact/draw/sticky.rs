//! The sticky tool: a click puts a note in the middle of the point and opens it for typing.

use drafftink_geom::kurbo::Point;
use serde_json::json;

use crate::interact::ctx::Ctx;

/// A new sticky note centred on `at`, open for typing. It is `element.create_sticky` with its defaults, so the
/// size, the paper colour and the frame the note joins are the command's; `from_tool` returns the tool to select
/// unless the tool is locked. The shell opens the editor from the text edit the command began.
pub(super) fn place(cx: &mut Ctx<'_>, at: Point) {
    cx.exec("element.create_sticky", json!({ "at": [at.x, at.y], "edit": true, "from_tool": true }));
}
