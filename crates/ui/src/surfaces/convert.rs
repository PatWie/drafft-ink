//! The convert-type popup. Tab (or Shift+Tab) runs `element.convert`, which turns the selection into the
//! next type of its family; the popup then lists the types the selection can become, with the current one
//! marked. A click converts to that type (`element.convert` with `to`). More Tab presses keep cycling.
//! Esc, or an empty selection, closes it.

use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::Value;

use crate::chrome::{GAP, lift_to};
use crate::state::UiState;
use crate::surfaces::context_menu::convert_targets;
use crate::theme::{TextSize, space};
use crate::widgets::{Ctl, Cx, panel_frame};

/// The short name on the button of a row such as "Convert to sharp arrow".
fn short(text: &str) -> &str {
    text.strip_prefix("Convert to ").unwrap_or(text)
}

/// True when the row converts to the type `current` (the `to` the last conversion reported: a shape name,
/// `line`, or `sharp arrow`, `round arrow`, `elbow arrow`).
fn is_current(args: &Value, current: Option<&str>) -> bool {
    let Some(current) = current else {
        return false;
    };
    match (args.get("to").and_then(Value::as_str), args.get("arrow_type").and_then(Value::as_str)) {
        (Some("arrow"), Some(kind)) => current == format!("{kind} arrow"),
        (Some(to), _) => to == current,
        (None, _) => false,
    }
}

/// How far the popup sits above the toasts' line, so that a toast does not hide it.
const TOAST_CLEARANCE: f32 = 28.0;

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
    // Above the bottom chrome, and above the toasts that appear there.
    let screen = ctx.content_rect();
    let lift = lift_to(screen, st.chrome.bottom_floor(screen), GAP) + TOAST_CLEARANCE;
    Area::new(Id::unique("convert_popup")).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -lift)).order(Order::Middle).show(
        ctx,
        |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(space::SM, space::SM);
                ui.label(egui::RichText::new("Convert to").font(TextSize::Label.hand()).color(cx.pal.text));
                ui.horizontal_wrapped(|ui| {
                    for args in &targets {
                        let text = cx.model.label("element.convert", args);
                        let ctl = Ctl::new("element.convert")
                            .args(args.clone())
                            .label(text.clone())
                            .selected(is_current(args, popup.current.as_deref()));
                        cx.toggle_button(ui, &ctl, short(&text));
                    }
                });
                ui.label(
                    egui::RichText::new("Tab picks the next type").size(TextSize::Caption.points()).color(cx.pal.muted),
                );
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_current_row_is_found_by_the_words_the_command_reports() {
        assert!(is_current(&json!({ "to": "diamond" }), Some("diamond")));
        assert!(!is_current(&json!({ "to": "diamond" }), Some("ellipse")));
        assert!(is_current(&json!({ "to": "line" }), Some("line")));
        assert!(is_current(&json!({ "to": "arrow", "arrow_type": "elbow" }), Some("elbow arrow")));
        assert!(!is_current(&json!({ "to": "arrow", "arrow_type": "sharp" }), Some("elbow arrow")));
        assert!(!is_current(&json!({ "to": "line" }), None));
    }
}
