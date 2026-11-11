//! Toasts: a refused command shows its plain text for a few seconds, with a button for the first
//! suggestion the refusal carries. The text is the refusal's, unchanged.

use egui::{Align2, Area, Context, Id, Order, vec2};

use crate::action::UiAction;
use crate::state::UiState;
use crate::theme::MARGIN;
use crate::widgets::{Cx, panel_frame};

/// Draws the toasts above the bottom bar.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if st.toasts.is_empty() {
        return;
    }
    let now = ctx.input(|i| i.time);
    let toasts = st.toasts.clone();
    Area::new(Id::unique("toasts"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -(MARGIN + 56.0)))
        .order(Order::Tooltip)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 6.0);
            for t in toasts.iter().rev() {
                let Some(alpha) = t.alpha(now) else {
                    continue;
                };
                ui.scope(|ui| {
                    ui.set_opacity(alpha);
                    let frame = panel_frame(&cx.pal).stroke(egui::Stroke::new(1.5, cx.pal.danger));
                    frame.show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(&t.text).size(13.0).color(cx.pal.text));
                            if let Some(call) = &t.suggestion {
                                let title = cx.model.title(&call.id).to_owned();
                                if ui.button(title).clicked() {
                                    cx.emit(UiAction::Command(call.clone()));
                                }
                            }
                        });
                    });
                });
            }
        });
}
