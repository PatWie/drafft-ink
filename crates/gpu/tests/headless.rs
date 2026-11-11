//! The windowless shell draws the canvas under the UI, at the scale it was asked for.
//! Needs a Vulkan, Metal or DX12 adapter (`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` for Mesa
//! lavapipe); no adapter is a failure, as in `compositor.rs`.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use drafftink_gpu::headless::{Headless, HeadlessOptions};
use drafftink_gpu::{FrameCx, FrameOutput, ShellApp};
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Color, Fill};

const RED: Color = Color::from_rgb8(255, 0, 0);
const SKY: Color = Color::from_rgb8(100, 150, 250);

/// A canvas with a red square at (20, 20)-(60, 60) points and a panel along the top edge that egui paints.
struct Demo;

impl ShellApp for Demo {
    fn frame(&mut self, ui: &mut egui::Ui, cx: &mut FrameCx<'_>) -> FrameOutput {
        let scale = f64::from(cx.view.scale);
        let scene = cx.scene_mut(SKY);
        scene.fill(Fill::NonZero, Affine::scale(scale), RED, None, &Rect::new(20.0, 20.0, 60.0, 60.0));
        egui::Panel::top("bar").exact_size(10.0).frame(egui::Frame::new().fill(egui::Color32::BLACK)).show(ui, |_| {});
        FrameOutput::default()
    }
}

#[test]
fn the_canvas_is_composited_under_the_ui_at_the_asked_scale() {
    let options = HeadlessOptions { size_points: [100.0, 80.0], scale: 2.0, touch_first: false };
    let mut shell = Headless::new(Demo, options).expect("a headless shell");
    shell.settle(2);
    let image = shell.screenshot().expect("a picture");
    assert_eq!([image.width, image.height], [200, 160], "the picture is size * scale pixels");
    // The canvas: the clear colour and the red square, scaled by 2.
    assert_eq!(image.pixel(150, 120), Some([100, 150, 250, 255]));
    assert_eq!(image.pixel(80, 80), Some([255, 0, 0, 255]));
    // The UI over the canvas: the black panel covers the top 10 points.
    assert_eq!(image.pixel(150, 10), Some([0, 0, 0, 255]));
}
