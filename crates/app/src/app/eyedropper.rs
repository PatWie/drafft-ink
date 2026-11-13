//! Pick mode of the eyedropper on the app's side: the pointer reads the canvas instead of drawing on it, the shell
//! answers with the colour of the pixel, and a release of the pointer picks it (`drafftink_ui::eyedropper`).
//!
//! The pixel is read from the scene the shell last drew, so it is what the person sees on the canvas, pictures
//! included, and never a part of the interface.

use drafftink_engine::Origin;
use drafftink_gpu::{EventCx, PixelSample, PixelUse, PointerPhase, PointerSample, RedrawReason, Repaint};

use super::App;

impl App {
    /// A pointer sample while pick mode is on. `None` when the sample is not for the canvas (it is over the
    /// interface, which keeps working: the button that ends the mode, a tool) or pick mode is off.
    ///
    /// A press and a move ask for the colour under the pointer, for the swatch; the release asks for the colour to
    /// pick. So a finger can touch down, slide to the colour and lift. Nothing of it reaches the canvas.
    pub(super) fn eyedropper_pointer(
        &mut self,
        cx: &mut EventCx<'_>,
        s: &PointerSample,
        over_egui: bool,
    ) -> Option<Repaint> {
        self.ui.state.eyedropper()?;
        if over_egui && !matches!(s.phase, PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Leave) {
            return None;
        }
        let at = s.point.pos;
        cx.set_cursor(egui::CursorIcon::Crosshair);
        match s.phase {
            PointerPhase::Hover | PointerPhase::Down | PointerPhase::Move => {
                cx.sample_scene(at, PixelUse::Preview);
                Some(Repaint::Idle)
            }
            PointerPhase::Up if !over_egui => {
                cx.sample_scene(at, PixelUse::Pick);
                Some(Repaint::Idle)
            }
            PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Leave => {
                // The pointer left the canvas: no colour is under it.
                let changed = self.ui.state.set_eyedropper_preview(at, None);
                Some(if changed { Repaint::Now(RedrawReason::HoverChange) } else { Repaint::Idle })
            }
        }
    }

    /// The colour of a pixel of the canvas arrived.
    pub(super) fn eyedropper_pixel(&mut self, sample: PixelSample) -> Repaint {
        match sample.purpose {
            PixelUse::Preview => {
                if self.ui.state.set_eyedropper_preview(sample.at, sample.rgba) {
                    Repaint::Now(RedrawReason::HoverChange)
                } else {
                    Repaint::Idle
                }
            }
            PixelUse::Pick => {
                // Esc may have ended the mode while the pixel was on its way: then there is nothing to pick for.
                let Some(target) = self.ui.state.eyedropper_target() else { return Repaint::Idle };
                let Some([r, g, b, _]) = sample.rgba else {
                    self.toast("The colour could not be read from the canvas. Try again.");
                    return Repaint::Now(RedrawReason::Ui);
                };
                if let Some(call) = self.ui.state.finish_eyedropper(target, [r, g, b]) {
                    self.run(Origin::Ui, &call.id, call.args);
                }
                Repaint::Now(RedrawReason::Command)
            }
        }
    }
}
