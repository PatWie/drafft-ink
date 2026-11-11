//! The loading banner: shown in the middle of the canvas while a joined room is not usable yet, with the percentage
//! of the import once the room's content is in (`collab.status` `loading`). It has no controls and no animation of
//! its own: the app repaints it when the engine's poll reports progress.

use egui::{Align2, Area, Context, Id, Order, ProgressBar, RichText, vec2};

use crate::state::UiState;
use crate::widgets::{Cx, panel_frame};

/// Width of the progress bar.
const BAR_WIDTH: f32 = 240.0;

/// Draws the banner when the room is loading.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, _st: &mut UiState) {
    let Some(loading) = cx.model.collab.loading else { return };
    let pal = cx.pal;
    Area::new(Id::unique("room_loading")).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).order(Order::Tooltip).show(
        ctx,
        |ui| {
            panel_frame(&pal).show(ui, |ui| {
                ui.set_width(BAR_WIDTH);
                ui.label(RichText::new(loading.text()).size(14.0).color(pal.text));
                if let Some(fraction) = loading.fraction() {
                    ui.add_space(6.0);
                    ui.add(ProgressBar::new(fraction).desired_width(BAR_WIDTH));
                }
            });
        },
    );
}
