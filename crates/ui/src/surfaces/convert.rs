//! The convert-type popup. Tab (or Shift+Tab) runs `element.convert`, which turns the selection into the
//! next type of its family; the popup then lists the types the selection can become, with the current one
//! marked. A click converts to that type (`element.convert` with `to`). More Tab presses keep cycling.
//! Esc, or an empty selection, closes it.

use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::Value;

use crate::state::UiState;
use crate::surfaces::context_menu::convert_targets;
use crate::theme::{self, MARGIN};
use crate::widgets::{Ctl, Cx, panel_frame};

/// The short name on the button of a row such as "Convert to sharp arrow".
fn short(text: &str) -> &str {
    text.strip_prefix("Convert to ").unwrap_or(text)
}

/// True when the row converts to the type `current` (the `to` the last conversion reported).
fn is_current(args: &Value, current: Option<&str>) -> bool {
    let Some(current) = current else {
        return false;
    };
    let to = args.get("to").and_then(Value::as_str);
    match args.get("arrow_type").and_then(Value::as_str) {
        // Arrows are told apart by their path type, which the command reports as `arrow_type`-prefixed names
        // (`sharp_arrow`) or as plain `arrow`; a bare `arrow` is the sharp one.
        Some(kind) => current == format!("{kind}_arrow") || (kind == "sharp" && current == "arrow"),
        None => to == Some(current),
    }
}

/// Draws the popup.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(popup) = st.convert_popup.clone() else {
        return;
    };
    let targets = convert_targets(&cx.model.selection);
    if cx.model.selection_count == 0 || targets.is_empty() {
        st.convert_popup = None;
        return;
    }
    let lift = MARGIN + cx.layout.small_side() + 56.0;
    Area::new(Id::unique("convert_popup")).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -lift)).order(Order::Middle).show(
        ctx,
        |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                ui.label(egui::RichText::new("Convert to").font(theme::hand(14.0)).color(cx.pal.text));
                ui.horizontal_wrapped(|ui| {
                    for (text, args) in &targets {
                        let ctl = Ctl::new("element.convert")
                            .args(args.clone())
                            .label(*text)
                            .selected(is_current(args, popup.current.as_deref()));
                        cx.toggle_button(ui, &ctl, short(text));
                    }
                });
                ui.label(egui::RichText::new("Tab picks the next type").size(11.0).color(cx.pal.muted));
            });
        },
    );
}
