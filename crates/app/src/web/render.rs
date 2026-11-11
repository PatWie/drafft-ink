//! PNG export and screenshots in the browser.
//!
//! Reading a texture back from the GPU is asynchronous in a browser, and the engine's `Renderer` port is
//! synchronous ("nothing in a port may await", docs/engine.md). So the port works in two steps:
//!
//! 1. The first `render_png` for a request composes the scene, starts the drawing and answers
//!    [`PortError::NotReady`] (the command refuses with `render_pending`).
//! 2. When the pixels are back the port wakes the app. The app (UI) or the bridge (agents, `window.drafftink`)
//!    runs the same command again, and this time the call finds the finished picture and returns it.
//!
//! The same request means the same document snapshot and the same [`RenderRequest`]; only one picture is drawn at
//! a time, and a second request waits (it is answered `NotReady` until the first is finished). Finished pictures
//! stay on a small shelf, so a `batch` with several exports completes: each rerun finds the pictures drawn so far.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use drafftink_engine::PortError;
use drafftink_engine::ports::{RenderRequest, Renderer, SceneView};
use drafftink_engine::services::SharedTypesetter;
use drafftink_gpu::{Compositor, Gpu, GpuOptions, create_instance};
use drafftink_scene::Document;
use wasm_bindgen_futures::spawn_local;

use super::pure::Shelf;
use crate::offscreen::{Composed, compose, encode_png};

/// The device the pictures are drawn with: its own, made on first use, so the window's device is never blocked.
struct Device {
    gpu: Gpu,
    compositor: Compositor,
}

/// What was asked for, to recognise the same request again.
#[derive(Clone, PartialEq)]
struct Request {
    scene: Document,
    req: RenderRequest,
}

/// How many finished pictures wait for a caller that asks again (a `batch` may export several).
const SHELF_SIZE: usize = 4;

/// What is being drawn and what is finished.
struct Slot {
    /// The request that is being drawn. Only one is drawn at a time.
    drawing: Option<Request>,
    /// Finished pictures (or failures) that wait for the call that asked.
    done: Shelf<Request, Vec<u8>>,
}

/// The browser's [`Renderer`].
pub(crate) struct WebRenderer {
    typesetter: SharedTypesetter,
    wake: Arc<dyn Fn() + Send + Sync>,
    slot: Rc<RefCell<Slot>>,
    device: Rc<RefCell<Option<Device>>>,
}

impl WebRenderer {
    /// A renderer. `wake` wakes the app when a picture is ready, so that the command is run again.
    pub(crate) fn new(typesetter: SharedTypesetter, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let slot = Slot { drawing: None, done: Shelf::new(SHELF_SIZE) };
        Self { typesetter, wake, slot: Rc::new(RefCell::new(slot)), device: Rc::new(RefCell::new(None)) }
    }
}

impl Renderer for WebRenderer {
    fn render_png(&self, scene: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError> {
        let asked = Request { scene: scene.clone(), req: req.clone() };
        let mut slot = self.slot.borrow_mut();
        if let Some(result) = slot.done.get(&asked) {
            return result.map_err(PortError::Failed);
        }
        if slot.drawing.is_some() {
            return Err(PortError::NotReady);
        }
        let composed = compose(&self.typesetter, scene, req)?;
        slot.drawing = Some(asked.clone());
        let (slot_cell, device, wake) = (Rc::clone(&self.slot), Rc::clone(&self.device), Arc::clone(&self.wake));
        spawn_local(async move {
            let result = draw(&device, composed).await;
            let mut slot = slot_cell.borrow_mut();
            slot.drawing = None;
            slot.done.put(asked, result);
            drop(slot);
            wake();
        });
        Err(PortError::NotReady)
    }
}

/// Draws the scene and encodes the PNG. The device goes back into the cell when the drawing is done.
async fn draw(cell: &RefCell<Option<Device>>, composed: Composed) -> Result<Vec<u8>, String> {
    let taken = cell.borrow_mut().take();
    let mut device = match taken {
        Some(d) => d,
        None => {
            let gpu = Gpu::request(create_instance(None).await, None, &GpuOptions::default())
                .await
                .map_err(|e| e.to_string())?;
            let compositor = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).map_err(|e| e.to_string())?;
            Device { gpu, compositor }
        }
    };
    let image = device
        .compositor
        .render_to_image(&device.gpu, composed.size, Some((&composed.scene, composed.base)), None)
        .await
        .map_err(|e| e.to_string());
    let lost = device.gpu.lost().is_some();
    if !lost {
        *cell.borrow_mut() = Some(device);
    }
    let image = image?;
    encode_png(image.width, image.height, &image.rgba).map_err(|e| e.to_string())
}
