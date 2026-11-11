//! The LaTeX editor dialog. The canvas layer opens it with `UiState::open_equation(latex)` after
//! `text.edit_begin` on a math element. Apply, Cancel and the X end the text edit with `text.edit_end`;
//! the state is closed by `after_command` whatever that answers.

use drafftink_engine::Origin;
use egui::{Context, FontId, Id, Key, Modal, Modifiers, RichText, TextEdit};
use serde_json::json;

use crate::action::{Call, HostRequest, UiAction};
use crate::state::UiState;
use crate::surfaces::dialogs::{Close, plain_button, show_modal};
use crate::widgets::{Ctl, Cx};

/// The hint under the field.
const HINT: &str = r"Examples: x^2, \frac{a}{b}, \sqrt{x}, \sum_{i=1}^n";

/// `text.edit_end` that keeps the LaTeX.
pub(crate) fn apply_call(latex: &str) -> Call {
    Call::new(Origin::Ui, "text.edit_end", json!({ "commit": true, "latex": latex }))
}

/// `text.edit_end` that drops the edit.
pub(crate) fn cancel_call() -> Call {
    Call::new(Origin::Ui, "text.edit_end", json!({ "commit": false }))
}

fn cancel_close() -> Close {
    Close::Command { id: "text.edit_end", args: cancel_call().args }
}

/// Draws the dialog.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if st.equation.is_none() {
        return;
    }
    // Text the app pasted for us goes to the end of the field.
    let pasted = st.take_pasted_text();
    let Some(eq) = st.equation.as_mut() else {
        return;
    };
    if let Some(text) = pasted {
        eq.latex.push_str(&text);
    }
    let pal = cx.pal;
    let close = cancel_close();
    show_modal(ctx, cx, Modal::new(Id::unique("equation_dialog")), "Edit equation", 440.0, &close, |ui, cx| {
        // Mod+Enter applies. It is taken before the field sees it, so it does not add a line.
        let apply_key = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
        let field = ui.add(
            TextEdit::multiline(&mut eq.latex)
                .font(FontId::monospace(14.0))
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("LaTeX"),
        );
        field.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MultilineTextInput, true, "LaTeX"));
        if !eq.focused {
            field.request_focus();
            eq.focused = true;
        }
        ui.add_space(4.0);
        ui.label(RichText::new(HINT).size(12.0).color(pal.muted));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if plain_button(ui, &pal, "Copy", false).clicked() {
                cx.emit(UiAction::Host(HostRequest::CopyText(eq.latex.clone())));
            }
            if plain_button(ui, &pal, "Paste", false).clicked() {
                cx.emit(UiAction::Host(HostRequest::PasteText));
            }
            ui.add_space((ui.available_width() - 2.0 * 80.0 - 2.0 * ui.spacing().item_spacing.x).max(0.0));
            let cancel = cancel_call();
            cx.text_button(ui, &Ctl::new(&cancel.id).args(cancel.args.clone()), "Cancel", false);
            let apply = apply_call(&eq.latex);
            cx.text_button(ui, &Ctl::new(&apply.id).args(apply.args.clone()), "Apply", true);
            if apply_key && cx.model.is_available(&apply.id) {
                cx.run(&apply.id, apply.args);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_and_cancel_calls() {
        assert_eq!(apply_call(r"\frac{a}{b}").args, json!({"commit": true, "latex": r"\frac{a}{b}"}));
        assert_eq!(cancel_call().args, json!({"commit": false}));
        assert_eq!(cancel_call().id, "text.edit_end");
    }
}
