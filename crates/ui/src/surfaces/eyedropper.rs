//! The eyedropper's pick mode on screen: a hint under the strip, and a swatch with the colour's hex beside the
//! pointer (a loupe of one pixel: the colour that a click picks). The state and the rules are in `eyedropper.rs`.

use egui::{Align2, Area, Context, Id, Order, Sense, vec2};

use crate::chrome::GAP;
use crate::colors;
use crate::eyedropper::Preview;
use crate::state::{ColorTarget, UiState};
use crate::theme::{TextSize, space};
use crate::widgets::{Cx, paint_swatch, panel_frame};

/// Where the swatch hangs from the pointer, so the pointer does not cover it.
const PREVIEW_OFFSET: egui::Vec2 = vec2(18.0, 18.0);
/// Radius of the swatch.
const SWATCH_RADIUS: f32 = 10.0;

/// The words of the hint for a target.
fn hint(target: ColorTarget) -> &'static str {
    match target {
        ColorTarget::Stroke => "Click a colour on the canvas to use it for the stroke. Esc cancels.",
        ColorTarget::Fill => "Click a colour on the canvas to use it for the fill. Esc cancels.",
        ColorTarget::Background => "Click a colour on the canvas. Esc cancels.",
    }
}

/// Draws the hint and the swatch while pick mode is on.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(mode) = st.eyedropper().copied() else { return };
    let pal = cx.pal;
    let screen = ctx.content_rect();
    let below_strip = st.chrome.below_strip(screen) + GAP;
    Area::new(Id::unique("eyedropper_hint"))
        .order(Order::Tooltip)
        .interactable(false)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(egui::pos2(screen.center().x, below_strip))
        .show(ctx, |ui| {
            panel_frame(&pal).show(ui, |ui| {
                ui.label(egui::RichText::new(hint(mode.target)).size(TextSize::Body.points()).color(pal.text));
            });
        });
    let Some(Preview { at, shown }) = mode.preview else { return };
    let flip_x = at.x + PREVIEW_OFFSET.x + 120.0 > screen.right();
    let flip_y = at.y + PREVIEW_OFFSET.y + 40.0 > screen.bottom();
    let pivot = match (flip_x, flip_y) {
        (false, false) => Align2::LEFT_TOP,
        (true, false) => Align2::RIGHT_TOP,
        (false, true) => Align2::LEFT_BOTTOM,
        (true, true) => Align2::RIGHT_BOTTOM,
    };
    let offset = vec2(
        if flip_x { -PREVIEW_OFFSET.x } else { PREVIEW_OFFSET.x },
        if flip_y { -PREVIEW_OFFSET.y } else { PREVIEW_OFFSET.y },
    );
    Area::new(Id::unique("eyedropper_preview"))
        .order(Order::Tooltip)
        .interactable(false)
        .pivot(pivot)
        .fixed_pos(at + offset)
        .show(ctx, |ui| {
            panel_frame(&pal).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::MD;
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(2.0 * SWATCH_RADIUS, 2.0 * SWATCH_RADIUS), Sense::hover());
                    paint_swatch(
                        ui.painter(),
                        rect.center(),
                        SWATCH_RADIUS,
                        Some(colors::to_egui(shown)),
                        false,
                        false,
                        &pal,
                        true,
                    );
                    ui.label(egui::RichText::new(shown.to_hex()).size(TextSize::Body.points()).color(pal.text));
                });
            });
        });
}
