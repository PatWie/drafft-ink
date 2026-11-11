//! Drawing a scene into an image without WebGPU: PNG export and screenshots in the compatibility mode.
//!
//! WebGL reads pixels back synchronously, so unlike the WebGPU exporter this one needs no waiting.

use vello::Scene;
use vello::peniko::Color;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, WebGl2RenderingContext as Gl};

use super::renderer::GlRenderer;
use crate::backend::Detail;
use crate::error::GpuError;
use crate::frame::Image;

/// A canvas of its own, never shown, that pictures are drawn on.
pub struct GlOffscreen {
    canvas: HtmlCanvasElement,
    renderer: GlRenderer,
}

impl GlOffscreen {
    /// Make the canvas and its WebGL2 context.
    pub fn new() -> Result<Self, GpuError> {
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.create_element("canvas").ok())
            .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
            .ok_or_else(|| GpuError::WebGl("could not make a canvas".into()))?;
        let renderer = GlRenderer::new(&canvas)?;
        Ok(Self { canvas, renderer })
    }

    /// Draw `scene` over `base` at `size` pixels and read the result back as straight-alpha RGBA, top row first.
    pub fn render(&mut self, scene: &Scene, base: Color, size: [u32; 2]) -> Result<Image, GpuError> {
        let [w, h] = size.map(|s| s.max(1));
        self.canvas.set_width(w);
        self.canvas.set_height(h);
        self.renderer.draw(scene, base, [w, h], Detail::Full)?;
        let gl = self.renderer.gl();
        let (gw, gh) = (gl.drawing_buffer_width(), gl.drawing_buffer_height());
        if u32::try_from(gw) != Ok(w) || u32::try_from(gh) != Ok(h) {
            return Err(GpuError::Readback(format!("the browser gave a {gw}x{gh} canvas for {w}x{h}")));
        }
        let row = w as usize * 4;
        let mut bottom_up = vec![0u8; row * h as usize];
        gl.bind_framebuffer(Gl::FRAMEBUFFER, None);
        gl.read_pixels_with_opt_u8_array(0, 0, gw, gh, Gl::RGBA, Gl::UNSIGNED_BYTE, Some(&mut bottom_up))
            .map_err(|_| GpuError::Readback("readPixels failed".into()))?;
        let mut rgba = Vec::with_capacity(bottom_up.len());
        for line in bottom_up.chunks_exact(row).rev() {
            rgba.extend_from_slice(line);
        }
        unpremultiply(&mut rgba);
        Ok(Image { width: w, height: h, rgba })
    }
}

/// The canvas holds premultiplied alpha; a PNG holds straight alpha.
fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

impl Drop for GlOffscreen {
    fn drop(&mut self) {
        // Browsers allow about sixteen contexts; give this one back at once instead of waiting for the collector.
        if let Ok(Some(ext)) = self.renderer.gl().get_extension("WEBGL_lose_context")
            && let Ok(ext) = ext.dyn_into::<web_sys::WebglLoseContext>()
        {
            ext.lose_context();
        }
    }
}
