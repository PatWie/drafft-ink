//! The drawing back end behind the shell.
//!
//! The shell owns the window, the event flow and the repaint rules. A [`Backend`] owns everything that touches a
//! graphics API: it draws a Vello scene, paints egui over it and presents the result. Two exist:
//!
//! * `wgpu_backend` (Cargo feature `wgpu`): Vello's compute renderer on wgpu (Vulkan, Metal, DX12, WebGPU). The
//!   normal path on every platform.
//! * `gl::GlBackend` (feature `webgl`, web only): `vello_gpu` on WebGL2 with egui_glow, for browsers that have no
//!   WebGPU. See `docs/web-app.md`, "Browsers without WebGPU".

use vello::Scene;
use vello::peniko::Color;
use winit::window::Window;

use crate::error::{GpuError, LostInfo};
use crate::frame::EguiFrame;

/// Which renderer draws the canvas. The app reads it to choose how much to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    /// Vello on wgpu: the full renderer.
    Vello,
    /// `vello_gpu` on WebGL2: the compatibility renderer of browsers without WebGPU. Slower on large scenes, so the
    /// app draws less (`docs/web-app.md`).
    WebGl,
}

impl BackendKind {
    /// True for the compatibility renderer, which the app lightens.
    pub fn is_compat(self) -> bool {
        matches!(self, Self::WebGl)
    }
}

pub use drafftink_render::replay::Detail;

/// What [`Backend::present`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Presented {
    /// The frame is on screen.
    Shown,
    /// The surface was not ready (resized, lost); the frame is not drawn and the shell asks for another.
    Retry,
    /// The surface gave nothing this time (occluded, timed out); the frame is not drawn.
    Skipped,
}

/// A graphics API behind the shell. All methods run on the UI thread.
pub(crate) trait Backend {
    /// Which renderer this is.
    fn kind(&self) -> BackendKind;

    /// One line for logs: the adapter or the GL renderer.
    fn describe(&self) -> String;

    /// The largest texture side, which egui's atlas must stay under.
    fn max_texture_side(&self) -> u32;

    /// `Some` once the device was lost.
    fn lost(&self) -> Option<LostInfo>;

    /// Test hook: lose the device now.
    fn destroy_for_test(&self);

    /// The drawing surface size in physical pixels.
    fn size(&self) -> [u32; 2];

    /// Follow a new window size. Returns whether anything changed.
    fn resize(&mut self, size: [u32; 2]) -> bool;

    /// Make sure the retained scene image has this size. True when it was recreated, in which case the scene must be
    /// drawn again.
    fn ensure_scene_target(&mut self, size: [u32; 2]) -> bool;

    /// Draw `scene` over `base` into the retained scene image.
    fn render_scene(&mut self, scene: &Scene, base: Color, size: [u32; 2], detail: Detail) -> Result<(), GpuError>;

    /// Put the retained scene image and `frame` on screen. Applies and clears the frame's texture updates when it
    /// returns [`Presented::Shown`].
    fn present(&mut self, window: &Window, frame: &mut EguiFrame, size: [u32; 2]) -> Result<Presented, GpuError>;
}
