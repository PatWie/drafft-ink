//! A click on the link icon of an element opens its link (docs/interaction.md, Excalidraw
//! `hitLinkElement`). The icon sits beside the top-right corner of an element that has a link
//! (`drafftink_edit::link::icon_rect`). A selected element shows its handles there instead, so only elements that
//! are not selected have a live icon. The click runs `element.open_link`; the shell opens the address.

use drafftink_edit::link::hits_icon;
use drafftink_scene::{ElementId, SceneRead};

use super::ctx::Ctx;
use super::events::Ev;
use super::outcome::UiRequest;

/// The topmost element whose link icon is under the pointer and that is not selected.
fn icon_under(cx: &Ctx<'_>, ev: &Ev<'_>) -> Option<ElementId> {
    let zoom = cx.zoom();
    let selection = cx.s.selection();
    // The store lists the elements back to front: the last match is the topmost.
    cx.s.store()
        .iter()
        .filter(|e| !selection.contains(&e.id) && hits_icon(e, ev.at.world, zoom))
        .last()
        .map(|e| e.id.clone())
}

/// Handles a click that may be on a link icon. True when it was (the click is used up).
pub(super) fn click(cx: &mut Ctx<'_>, ev: &Ev<'_>) -> bool {
    if ev.meta.button != drafftink_canvas::Button::Primary {
        return false;
    }
    let Some(id) = icon_under(cx, ev) else { return false };
    let opened = cx.exec("element.open_link", serde_json::json!({ "id": id.as_str() }));
    if let Some(url) = opened.as_ref().and_then(|v| v.get("url")).and_then(serde_json::Value::as_str) {
        cx.out.ui_request = Some(UiRequest::OpenUrl { url: url.to_owned() });
    }
    true
}
