//! The WebGL2 back end: `vello_gpu` draws the scene into the canvas, the scene is copied into a texture, and every
//! frame blits that texture and paints egui over it.
//!
//! A WebGL canvas loses its pixels after each frame is shown, so the retained scene cannot stay in the default
//! framebuffer the way the wgpu back end keeps it in a texture of its own. The copy (`blitFramebuffer`, no shader)
//! costs nothing next to the strip generation, and it is what lets a frame that only changed egui skip the scene.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use drafftink_profile::{Counter, Phase, in_phase, inc};
use vello::Scene;
use vello::peniko::Color;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use web_sys::{HtmlCanvasElement, WebGl2RenderingContext as Gl, WebGlFramebuffer, WebGlTexture};
use winit::window::Window;

use super::renderer::GlRenderer;
use crate::backend::{Backend, BackendKind, Detail, Presented};
use crate::error::{GpuError, LostInfo};
use crate::frame::EguiFrame;

fn webgl(message: impl Into<String>) -> GpuError {
    GpuError::WebGl(message.into())
}

/// The retained scene: a texture the size of the canvas, with a framebuffer to blit from and to.
struct SceneImage {
    texture: WebGlTexture,
    framebuffer: WebGlFramebuffer,
    size: [u32; 2],
}

impl SceneImage {
    fn create(gl: &Gl, size: [u32; 2]) -> Result<Self, GpuError> {
        let to_i32 = |v: u32| i32::try_from(v).map_err(|_| webgl("the canvas is too large"));
        let (w, h) = (to_i32(size[0])?, to_i32(size[1])?);
        let texture = gl.create_texture().ok_or_else(|| webgl("no texture for the scene"))?;
        let framebuffer = gl.create_framebuffer().ok_or_else(|| webgl("no framebuffer for the scene"))?;
        gl.bind_texture(Gl::TEXTURE_2D, Some(&texture));
        gl.tex_storage_2d(Gl::TEXTURE_2D, 1, Gl::RGBA8, w, h);
        gl.bind_framebuffer(Gl::FRAMEBUFFER, Some(&framebuffer));
        gl.framebuffer_texture_2d(Gl::FRAMEBUFFER, Gl::COLOR_ATTACHMENT0, Gl::TEXTURE_2D, Some(&texture), 0);
        let complete = gl.check_framebuffer_status(Gl::FRAMEBUFFER) == Gl::FRAMEBUFFER_COMPLETE;
        gl.bind_framebuffer(Gl::FRAMEBUFFER, None);
        gl.bind_texture(Gl::TEXTURE_2D, None);
        if !complete {
            gl.delete_framebuffer(Some(&framebuffer));
            gl.delete_texture(Some(&texture));
            return Err(webgl("the scene framebuffer is incomplete"));
        }
        Ok(Self { texture, framebuffer, size })
    }

    fn delete(&self, gl: &Gl) {
        gl.delete_framebuffer(Some(&self.framebuffer));
        gl.delete_texture(Some(&self.texture));
    }
}

/// Copies the whole `size` rectangle from one framebuffer to another (`None` is the canvas).
fn blit(gl: &Gl, from: Option<&WebGlFramebuffer>, to: Option<&WebGlFramebuffer>, size: [u32; 2]) {
    let (w, h) = (i32::try_from(size[0]).unwrap_or(i32::MAX), i32::try_from(size[1]).unwrap_or(i32::MAX));
    gl.bind_framebuffer(Gl::READ_FRAMEBUFFER, from);
    gl.bind_framebuffer(Gl::DRAW_FRAMEBUFFER, to);
    gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, Gl::COLOR_BUFFER_BIT, Gl::NEAREST);
    gl.bind_framebuffer(Gl::FRAMEBUFFER, None);
}

/// Copies the canvas into the scene texture.
fn capture(gl: &Gl, texture: &WebGlTexture, size: [u32; 2]) {
    let (w, h) = (i32::try_from(size[0]).unwrap_or(i32::MAX), i32::try_from(size[1]).unwrap_or(i32::MAX));
    gl.bind_framebuffer(Gl::FRAMEBUFFER, None);
    gl.bind_texture(Gl::TEXTURE_2D, Some(texture));
    gl.copy_tex_sub_image_2d(Gl::TEXTURE_2D, 0, 0, 0, 0, 0, w, h);
    gl.bind_texture(Gl::TEXTURE_2D, None);
}

/// The drawing back end of browsers without WebGPU.
pub(crate) struct GlBackend {
    draw: GlRenderer,
    painter: egui_glow::Painter,
    canvas: HtmlCanvasElement,
    scene_image: Option<SceneImage>,
    has_scene: bool,
    base: Color,
    size: [u32; 2],
    max_texture: u32,
    lost: Rc<Cell<bool>>,
    on_lost: Closure<dyn FnMut(web_sys::Event)>,
}

impl GlBackend {
    /// Start WebGL2 on `canvas`.
    pub(crate) fn create(canvas: &HtmlCanvasElement, size: [u32; 2]) -> Result<Self, GpuError> {
        // winit sizes the canvas with CSS only; the drawing buffer follows these attributes (the wgpu surface sets them
        // when it is configured).
        canvas.set_width(size[0]);
        canvas.set_height(size[1]);
        let draw = GlRenderer::new(canvas)?;
        let gl = draw.gl().clone();
        let max_texture =
            gl.get_parameter(Gl::MAX_TEXTURE_SIZE).ok().and_then(|v| v.as_f64()).map_or(2048, |v| v as u32);
        #[expect(clippy::arc_with_non_send_sync, reason = "egui_glow takes an Arc; the web is single threaded")]
        let glow = Arc::new(glow::Context::from_webgl2_context(gl));
        let painter = egui_glow::Painter::new(glow, "", None, false).map_err(|e| webgl(e.to_string()))?;
        let lost = Rc::new(Cell::new(false));
        let flag = Rc::clone(&lost);
        // `preventDefault` tells the browser that this page wants a `webglcontextrestored` event.
        let on_lost = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            event.prevent_default();
            flag.set(true);
        });
        canvas
            .add_event_listener_with_callback("webglcontextlost", on_lost.as_ref().unchecked_ref())
            .map_err(|_| webgl("could not listen for a lost context"))?;
        Ok(Self {
            draw,
            painter,
            canvas: canvas.clone(),
            scene_image: None,
            has_scene: false,
            base: Color::BLACK,
            size,
            max_texture,
            lost,
            on_lost,
        })
    }
}

impl Drop for GlBackend {
    fn drop(&mut self) {
        self.painter.destroy();
        if let Some(image) = self.scene_image.take() {
            image.delete(self.draw.gl());
        }
        // The listener is removed with the closure; the canvas outlives the backend.
        let _ =
            self.canvas.remove_event_listener_with_callback("webglcontextlost", self.on_lost.as_ref().unchecked_ref());
    }
}

impl Backend for GlBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::WebGl
    }

    fn describe(&self) -> String {
        let gl = self.draw.gl();
        let text = |parameter: u32| gl.get_parameter(parameter).ok().and_then(|v| v.as_string()).unwrap_or_default();
        format!("WebGL2 ({}, {})", text(Gl::RENDERER), text(Gl::VENDOR))
    }

    fn max_texture_side(&self) -> u32 {
        self.max_texture
    }

    fn lost(&self) -> Option<LostInfo> {
        (self.lost.get() || self.draw.gl().is_context_lost())
            .then(|| LostInfo { destroyed: false, message: "the WebGL context was lost".into() })
    }

    fn destroy_for_test(&self) {
        self.lost.set(true);
    }

    fn size(&self) -> [u32; 2] {
        self.size
    }

    fn resize(&mut self, size: [u32; 2]) -> bool {
        if size[0] == 0 || size[1] == 0 || size == self.size {
            return false;
        }
        self.size = size;
        self.canvas.set_width(size[0]);
        self.canvas.set_height(size[1]);
        true
    }

    fn ensure_scene_target(&mut self, size: [u32; 2]) -> bool {
        let size = [size[0].max(1), size[1].max(1)];
        if self.scene_image.as_ref().is_some_and(|image| image.size == size) {
            return false;
        }
        if let Some(old) = self.scene_image.take() {
            old.delete(self.draw.gl());
        }
        self.has_scene = false;
        match SceneImage::create(self.draw.gl(), size) {
            Ok(image) => self.scene_image = Some(image),
            Err(e) => log::error!("{e}"),
        }
        true
    }

    fn render_scene(&mut self, scene: &Scene, base: Color, size: [u32; 2], detail: Detail) -> Result<(), GpuError> {
        self.ensure_scene_target(size);
        let Some(image) = self.scene_image.as_ref() else { return Err(webgl("no scene texture")) };
        let gl = self.draw.gl().clone();
        // egui leaves its scissor box on; the scene wants the whole canvas.
        gl.disable(Gl::SCISSOR_TEST);
        gl.bind_framebuffer(Gl::FRAMEBUFFER, None);
        let stats = self.draw.draw(scene, base, size, detail)?;
        capture(&gl, &image.texture, size);
        self.has_scene = true;
        self.base = base;
        inc(Counter::GpuSubmits);
        log::trace!("webgl scene: {stats:?}");
        Ok(())
    }

    fn present(&mut self, window: &Window, frame: &mut EguiFrame, size: [u32; 2]) -> Result<Presented, GpuError> {
        if self.lost().is_some() {
            return Ok(Presented::Skipped);
        }
        let gl = self.draw.gl().clone();
        gl.disable(Gl::SCISSOR_TEST);
        // A clear first: Chrome clears the canvas lazily after each frame, and a blit that is the first thing drawn
        // into a fresh canvas can be lost.
        let c = self.base.to_rgba8();
        gl.bind_framebuffer(Gl::FRAMEBUFFER, None);
        gl.clear_color(f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0, 1.0);
        gl.clear(Gl::COLOR_BUFFER_BIT);
        if let (true, Some(image)) = (self.has_scene, self.scene_image.as_ref()) {
            blit(&gl, Some(&image.framebuffer), None, size);
        }
        in_phase(Phase::GpuSubmit, || {
            self.painter.paint_and_update_textures(
                size,
                frame.pixels_per_point,
                &frame.primitives,
                &mut frame.textures_delta,
            );
        });
        frame.textures_delta.clear();
        window.pre_present_notify();
        inc(Counter::GpuSubmits);
        Ok(Presented::Shown)
    }
}
