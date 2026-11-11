//! A click on the link icon of an element opens its link (docs/interaction.md, Excalidraw `hitLinkElement`). The
//! icon sits beside the top-right corner of an element that has a link (`drafftink_geom::link_icon`). The canvas
//! reports it as its own pick target, [`Target::LinkIcon`], when the element is not selected (a selected element
//! shows its handles there). The click runs `element.open_link`; the shell opens the address.

use drafftink_canvas::Target;

use super::ctx::Ctx;
use super::events::Ev;
use super::outcome::UiRequest;

/// Handles a click that may be on a link icon. True when it was (the click is used up).
pub(super) fn click(cx: &mut Ctx<'_>, ev: &Ev<'_>) -> bool {
    if ev.meta.button != drafftink_canvas::Button::Primary {
        return false;
    }
    let Target::LinkIcon { id } = ev.target else { return false };
    let opened = cx.exec("element.open_link", serde_json::json!({ "id": id.as_str() }));
    if let Some(url) = opened.as_ref().and_then(|v| v.get("url")).and_then(serde_json::Value::as_str) {
        cx.out.ui_request = Some(UiRequest::OpenUrl { url: url.to_owned() });
    }
    true
}
