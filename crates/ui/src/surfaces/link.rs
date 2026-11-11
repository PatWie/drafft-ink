//! The link dialog (`Mod+K`, `ui.open_link_editor`): one field for the single selected element. Save runs
//! `element.set_link` with the text, Remove runs it with `null`. A refusal keeps the dialog open next to its toast,
//! so the text can be fixed.

use egui::{Context, Id, Key, Modal, RichText};
use serde_json::json;

use crate::state::UiState;
use crate::surfaces::dialogs::{plain_button, show_modal, text_field, Close};
use crate::widgets::{Ctl, Cx};

/// The hint under the field.
const HINT: &str = "A web address (https://example.com), an email (mailto:name@example.com), or an element: #id or @name";
/// What the dialog says when the selection is not one element.
const NEEDS_ONE: &str = "Select one element to give it a link.";

/// The call that saves the text (an empty text removes the link).
pub(crate) fn save_args(text: &str) -> serde_json::Value {
    let trimmed = text.trim();
    json!({ "link": if trimmed.is_empty() { serde_json::Value::Null } else { json!(trimmed) } })
}

/// Draws the dialog.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(editor) = st.link_editor.as_mut() else {
        return;
    };
    let one = cx.model.selection_count == 1;
    let current = cx.model.selection_link.clone();
    let text = editor.text.get_or_insert_with(|| current.clone().unwrap_or_default());
    let pal = cx.pal;
    let focused = &mut editor.focused;
    show_modal(ctx, cx, Modal::new(Id::unique("link_dialog")), "Link", 440.0, &Close::Overlays, |ui, cx| {
        if !one {
            ui.label(RichText::new(NEEDS_ONE).size(13.0).color(pal.text));
            return;
        }
        let field = text_field(ui, text, "https://…", None, None, "Link");
        if !*focused {
            field.request_focus();
            *focused = true;
        }
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        ui.add_space(4.0);
        ui.label(RichText::new(HINT).size(12.0).color(pal.muted));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let remove = Ctl::new("element.set_link").args(json!({ "link": null })).label("Remove");
            if current.is_some() {
                cx.text_button(ui, &remove, "Remove", false);
            }
            ui.add_space((ui.available_width() - 2.0 * 80.0 - 2.0 * ui.spacing().item_spacing.x).max(0.0));
            if plain_button(ui, &pal, "Cancel", false).clicked() {
                cx.verb(crate::action::Verb::CloseOverlays);
            }
            let save = Ctl::new("element.set_link").args(save_args(text)).label("Save");
            let clicked = cx.text_button(ui, &save, "Save", true).clicked();
            if enter && !clicked && cx.model.is_available(save.id) {
                cx.run(save.id, save.args.clone());
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_field_removes_the_link_and_the_text_is_trimmed() {
        assert_eq!(save_args("  https://example.com "), json!({"link": "https://example.com"}));
        assert_eq!(save_args("   "), json!({"link": null}));
    }
}
