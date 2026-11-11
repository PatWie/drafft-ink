//! What a frame carries to a back end: egui's output, and the image a read-back returns.

use egui::epaint::ClippedPrimitive;

/// What egui produced for one frame, ready to paint.
pub struct EguiFrame {
    pub primitives: Vec<ClippedPrimitive>,
    pub textures_delta: egui::TexturesDelta,
    pub pixels_per_point: f32,
}

impl EguiFrame {
    /// Nothing from egui (scene only).
    pub fn empty() -> Self {
        Self { primitives: Vec::new(), textures_delta: egui::TexturesDelta::default(), pixels_per_point: 1.0 }
    }

    /// Tessellate the output of `ctx.run_ui` / `ctx.run`. Platform output stays with the caller.
    pub fn from_output(ctx: &egui::Context, output: egui::FullOutput) -> Self {
        let pixels_per_point = output.pixels_per_point;
        Self {
            primitives: ctx.tessellate(output.shapes, pixels_per_point),
            textures_delta: output.textures_delta,
            pixels_per_point,
        }
    }
}

/// An RGBA8 image read back from the GPU. Rows are tightly packed, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// The pixel at (x, y), or `None` outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.rgba.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }
}
