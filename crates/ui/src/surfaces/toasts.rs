//! Toasts: a refused command shows its plain text for a few seconds, with a button for the first
//! suggestion the refusal carries and a Copy button that puts the whole error (code, command, arguments,
//! message, version, platform) on the clipboard. The text is the refusal's, unchanged; a long one is cut
//! with a "Show more" button.

use egui::{Align2, Area, Context, Id, Order, Ui, vec2};

use crate::action::{HostRequest, UiAction};
use crate::chrome::{GAP, lift_to};
use crate::error_report::ErrorReport;
use crate::state::{Toast, UiState};
use crate::surfaces::dialogs::plain_button;
use crate::theme::{TextSize, space};
use crate::widgets::{Cx, panel_frame};

/// The widest a toast gets.
const MAX_WIDTH: f32 = 420.0;
/// A message longer than this many characters is cut until "Show more" is pressed.
const SHORT_LEN: usize = 220;

/// Draws the toasts above the bottom chrome (the bottom bar and the tabs).
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if st.toasts.is_empty() {
        return;
    }
    let now = ctx.input(|i| i.time);
    let toasts = st.toasts.clone();
    let screen = ctx.content_rect();
    let lift = lift_to(screen, st.chrome.bottom_floor(screen), GAP);
    Area::new(Id::unique("toasts")).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -lift)).order(Order::Tooltip).show(
        ctx,
        |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, space::MD);
            for t in toasts.iter().rev() {
                let Some(alpha) = t.alpha(now) else {
                    continue;
                };
                ui.scope(|ui| {
                    ui.set_opacity(alpha);
                    let frame = panel_frame(&cx.pal).stroke(egui::Stroke::new(1.5, cx.pal.danger));
                    frame.show(ui, |ui| {
                        ui.set_max_width(MAX_WIDTH);
                        toast(ui, cx, t);
                    });
                });
            }
        },
    );
}

fn toast(ui: &mut Ui, cx: &mut Cx<'_>, t: &Toast) {
    let expanded_id = Id::unique(("toast_expanded", &t.code, &t.text));
    let expanded = ui.data(|d| d.get_temp::<bool>(expanded_id)).unwrap_or(false);
    let long = t.text.chars().count() > SHORT_LEN;
    let shown: String = if long && !expanded {
        t.text.chars().take(SHORT_LEN).chain(std::iter::once('\u{2026}')).collect()
    } else {
        t.text.clone()
    };
    ui.vertical(|ui| {
        ui.add(egui::Label::new(egui::RichText::new(&shown).size(TextSize::Body.points()).color(cx.pal.text)).wrap());
        ui.add_space(space::SM);
        ui.horizontal_wrapped(|ui| {
            if let Some(call) = &t.suggestion {
                let title = cx.model.title(&call.id).to_owned();
                if plain_button(ui, &cx.pal, &title, false).clicked() {
                    cx.emit(UiAction::Command(call.clone()));
                }
            }
            if long && plain_button(ui, &cx.pal, if expanded { "Show less" } else { "Show more" }, false).clicked() {
                ui.data_mut(|d| d.insert_temp(expanded_id, !expanded));
            }
            if plain_button(ui, &cx.pal, "Copy", false).on_hover_text("Copy the error details").clicked() {
                cx.emit(UiAction::Host(HostRequest::CopyText(report(cx, t))));
            }
        });
    });
}

/// The text of the Copy button.
fn report(cx: &Cx<'_>, t: &Toast) -> String {
    let about = &cx.model.about;
    let version = format!("{} ({})", about.version, about.commit);
    ErrorReport { code: &t.code, command: &t.command, args: &t.args, message: &t.text, version: &version }.text()
}
