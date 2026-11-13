//! The wasm-bindgen surface: `Stage`, a canvas element and everything the JS package calls.
//!
//! The pointer, wheel and resize listeners, the frame scheduler and the typed wrapper are plain JS in
//! `packages/drafftink-canvas`. This module is the engine they drive: it mounts a WebGPU surface on the canvas,
//! owns the scene, answers queries, runs listeners, and draws a frame when asked and one is due.

mod convert;
mod shared;

use drafftink_canvas::{ElementEvent, InputEvent, Listener, MarqueeMode, PointerMode, Selector};
use drafftink_geom::kurbo::Size;
use drafftink_scene::Color;
use js_sys::{Function, Uint8Array};
use serde::Deserialize;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use self::convert::{console_error, fail, from_js, options_from_js, to_js};
use self::shared::{Shared, State};
use crate::core::Core;
use crate::dto::{EventDto, parse_event_kind};
use crate::error::ApiError;
use crate::export::{PngOptions, SvgExport};
use crate::image::ImageSpec;
use crate::options::{GridOptions, StageOptions, ThemeOption};
use crate::remote::RemoteUserSpec;
use crate::target::Renderer;

/// A stage: a canvas element showing a scene that the page can read and change.
#[wasm_bindgen]
pub struct Stage {
    shared: Shared,
}

/// Options that only the browser glue needs.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct MountOptions {
    width: Option<f64>,
    height: Option<f64>,
    device_pixel_ratio: Option<f64>,
}

/// True when the browser offers WebGPU (`navigator.gpu` exists). An adapter can still be missing.
#[wasm_bindgen(js_name = webgpuAvailable)]
pub fn webgpu_available() -> bool {
    drafftink_gpu::web::webgpu_available()
}

/// Replaces the page with a plain explanation of why WebGPU is needed and which browsers have it.
#[wasm_bindgen(js_name = showErrorPage)]
pub fn show_error_page(title: &str, details: &str) {
    drafftink_gpu::web::show_error_page(title, details);
}

fn wrap_listener(callback: Function) -> Listener {
    Box::new(move |event: &mut ElementEvent| {
        let arg = match to_js(&EventDto::from(&*event)) {
            Ok(v) => v,
            Err(_) => return,
        };
        match callback.call1(&JsValue::NULL, &arg) {
            // The package's wrapper answers `true` after `event.stopPropagation()`.
            Ok(answer) if answer.as_bool() == Some(true) => event.stop_propagation(),
            Ok(_) => {}
            Err(e) => console_error(&format!("a listener threw: {e:?}")),
        }
    })
}

fn parse_mode(mode: &str) -> Result<PointerMode, ApiError> {
    match mode {
        "select" => Ok(PointerMode::Select),
        "hand" => Ok(PointerMode::Hand),
        "draw" => Ok(PointerMode::Draw),
        other => {
            Err(ApiError::invalid(format!("{other:?} is not a pointer mode; use \"select\", \"hand\" or \"draw\"")))
        }
    }
}

fn parse_marquee(mode: &str) -> Result<MarqueeMode, ApiError> {
    match mode {
        "contain" => Ok(MarqueeMode::Contain),
        "intersect" => Ok(MarqueeMode::Intersect),
        other => Err(ApiError::invalid(format!("{other:?} is not a marquee mode; use \"contain\" or \"intersect\""))),
    }
}

/// Gives every shape in `value` (one shape, or the `add` steps of a batch) an id.
fn name_shape(shared: &Shared, shape: &mut Value) -> Option<String> {
    let obj = shape.as_object_mut()?;
    if let Some(Value::String(id)) = obj.get("id") {
        return Some(id.clone());
    }
    let id = shared.fresh_id();
    obj.insert("id".to_owned(), Value::String(id.clone()));
    Some(id)
}

#[wasm_bindgen]
impl Stage {
    /// Mounts a stage on `canvas`. Rejects when WebGPU is unavailable.
    #[wasm_bindgen(js_name = create)]
    pub async fn create(canvas: HtmlCanvasElement, options: JsValue) -> Result<Stage, JsError> {
        console_error_panic_hook::set_once();
        // Already set when a second stage is created; that is fine.
        let _ = console_log::init_with_level(log::Level::Warn);
        let mount: MountOptions = options_from_js(options.clone())?;
        let options: StageOptions = options_from_js(options_without_mount(&options)?)?;
        let window = web_sys::window().ok_or_else(|| fail("there is no window"))?;
        let dpr = mount.device_pixel_ratio.unwrap_or_else(|| window.device_pixel_ratio());
        let width = mount.width.unwrap_or_else(|| f64::from(canvas.client_width()));
        let height = mount.height.unwrap_or_else(|| f64::from(canvas.client_height()));
        let (width, height) = (width.max(1.0), height.max(1.0));
        let core = Core::new(Size::new(width, height), dpr, &options).map_err(fail)?;
        let probe = State { core, canvas: canvas.clone(), renderer: None, frames: 0 };
        let size = probe.device_size();
        let renderer = Renderer::for_canvas(canvas, size).await.map_err(fail)?;
        let mut state = probe;
        state.renderer = Some(renderer);
        Ok(Stage { shared: Shared::new(state) })
    }

    // ---- scene ------------------------------------------------------------------------------------------------

    /// Adds a shape on top of everything. Returns its id.
    pub fn add(&self, shape: JsValue) -> Result<String, JsError> {
        let mut value: Value = from_js(shape)?;
        let id = name_shape(&self.shared, &mut value).ok_or_else(|| fail("a shape must be an object"))?;
        self.shared.mutate(move |st| st.core.add(value).map(|_| ()))?;
        Ok(id)
    }

    /// Adds a picture from the bytes of a PNG, JPEG, WebP, GIF, BMP, ICO or SVG file. `options` may hold `id`, `x`, `y`, `width`,
    /// `height`, `maxSide` and `name`; without them the picture is at the origin in its natural size. Returns
    /// the id of the new shape.
    #[wasm_bindgen(js_name = addImage)]
    pub fn add_image(&self, bytes: Vec<u8>, options: JsValue) -> Result<String, JsError> {
        let spec: ImageSpec = options_from_js(options)?;
        let id = self.shared.write(|st| st.core.add_image(&bytes, &spec))?;
        Ok(id.as_str().to_owned())
    }

    /// Merge-patches a shape.
    pub fn update(&self, id: String, patch: JsValue) -> Result<(), JsError> {
        let patch: Value = from_js(patch)?;
        self.shared.mutate(move |st| st.core.update(&id, patch))
    }

    /// Removes elements for good.
    pub fn remove(&self, ids: Vec<String>) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.remove(&ids))
    }

    /// Runs a batch, `[{ op: "add", shape }, { op: "update", id, patch }, ...]`, in one transaction. Returns what
    /// each step produced: the id of an added shape or a new group, else `null`.
    pub fn transact(&self, ops: JsValue) -> Result<JsValue, JsError> {
        let mut batch: Value = from_js(ops)?;
        let mut planned = Vec::new();
        for op in batch.as_array_mut().into_iter().flatten() {
            let named = match op.get_mut("shape") {
                Some(shape) => name_shape(&self.shared, shape),
                None => None,
            };
            planned.push(named.map_or(Value::Null, Value::String));
        }
        let busy = self.shared.cell().try_borrow_mut().is_err();
        if busy {
            self.shared.mutate(move |st| st.core.transact(batch).map(|_| ()))?;
            return to_js(&planned);
        }
        let results = self.shared.write(|st| st.core.transact(batch))?;
        to_js(&results)
    }

    /// A fresh element id.
    #[wasm_bindgen(js_name = newId)]
    pub fn new_id(&self) -> String {
        self.shared.fresh_id()
    }

    /// One shape, in the element layout.
    pub fn get(&self, id: String) -> Result<JsValue, JsError> {
        let value = self.shared.read(|st| Ok(serde_json::to_value(st.core.get(&id)?)?))?;
        to_js(&value)
    }

    /// All shapes, back to front.
    pub fn elements(&self) -> Result<JsValue, JsError> {
        let value = self.shared.read(|st| Ok(serde_json::to_value(st.core.elements())?))?;
        to_js(&value)
    }

    /// The ids a selector (`#id`, `.name`, `rectangle`, `group:g1`, `*`) matches, back to front.
    pub fn find(&self, selector: String) -> Result<Vec<String>, JsError> {
        self.shared.read(|st| st.core.find(&selector))
    }

    /// Number of live shapes.
    pub fn count(&self) -> Result<usize, JsError> {
        self.shared.read(|st| Ok(st.core.len()))
    }

    /// Changes whenever the scene does.
    pub fn revision(&self) -> Result<f64, JsError> {
        self.shared.read(|st| Ok(st.core.revision() as f64))
    }

    /// Removes everything.
    pub fn clear(&self) -> Result<(), JsError> {
        self.shared.mutate(|st| st.core.clear())
    }

    // ---- documents --------------------------------------------------------------------------------------------

    /// The scene as native JSON text.
    #[wasm_bindgen(js_name = exportJson)]
    pub fn export_json(&self, pretty: Option<bool>) -> Result<String, JsError> {
        self.shared.read(|st| st.core.to_json(pretty.unwrap_or(false)))
    }

    /// Replaces the scene with a file: the native JSON, a PNG or SVG exported from here, `.excalidraw`, or a file
    /// of the old app.
    #[wasm_bindgen(js_name = loadBytes)]
    pub fn load_bytes(&self, bytes: Vec<u8>) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.load_bytes(&bytes))
    }

    /// The scene as SVG text.
    #[wasm_bindgen(js_name = toSVG)]
    pub fn to_svg(&self, options: JsValue) -> Result<String, JsError> {
        let options: SvgExport = options_from_js(options)?;
        self.shared.write(|st| st.core.to_svg(&options))
    }

    /// The scene as PNG bytes. Draws offscreen, so it works while the canvas is hidden.
    #[wasm_bindgen(js_name = toPNG)]
    pub async fn to_png(&self, options: JsValue) -> Result<Uint8Array, JsError> {
        let options: PngOptions = options_from_js(options)?;
        let (plan, mut renderer) = {
            let mut st = self
                .shared
                .cell()
                .try_borrow_mut()
                .map_err(|_| fail("the stage is busy; export after the listener returns"))?;
            let renderer = st.renderer.take().ok_or_else(|| {
                fail("the stage cannot draw now (another export runs, the GPU was lost or the stage was destroyed)")
            })?;
            match st.core.plan_png(&options, renderer.max_side()) {
                Ok(plan) => (plan, renderer),
                Err(e) => {
                    st.renderer = Some(renderer);
                    return Err(fail(e));
                }
            }
        };
        let png = plan.render(&mut renderer).await;
        if let Ok(mut st) = self.shared.cell().try_borrow_mut() {
            st.renderer = Some(renderer);
            // The scene texture now has the size of the export.
            st.core.request_frame();
        }
        let png = png.map_err(fail)?;
        Ok(Uint8Array::from(png.as_slice()))
    }

    // ---- camera -----------------------------------------------------------------------------------------------

    /// `{ scroll: [x, y], zoom }`: the world point at the centre of the canvas and the pixels per world unit.
    pub fn camera(&self) -> Result<JsValue, JsError> {
        let camera = self.shared.read(|st| Ok(st.core.camera()))?;
        to_js(&camera)
    }

    /// Sets the centre and/or the zoom.
    #[wasm_bindgen(js_name = setCamera)]
    pub fn set_camera(&self, scroll: Option<Vec<f64>>, zoom: Option<f64>) -> Result<(), JsError> {
        let scroll = match scroll.as_deref() {
            None => None,
            Some([x, y]) => Some([*x, *y]),
            Some(_) => return Err(fail("scroll is [x, y]")),
        };
        self.shared.mutate(move |st| st.core.set_camera(scroll, zoom))
    }

    /// Moves the content by `dx`, `dy` pixels.
    #[wasm_bindgen(js_name = panBy)]
    pub fn pan_by(&self, dx: f64, dy: f64) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.pan_by(dx, dy))
    }

    /// Zooms by `factor` around the screen point `(x, y)`.
    #[wasm_bindgen(js_name = zoomAt)]
    pub fn zoom_at(&self, x: f64, y: f64, factor: f64) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.zoom_at(x, y, factor))
    }

    /// Fits all content in the canvas.
    #[wasm_bindgen(js_name = fitToContent)]
    pub fn fit_to_content(&self, padding: Option<f64>, max_zoom: Option<f64>) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.fit_to_content(padding.unwrap_or(40.0), max_zoom).map(|_| ()))
    }

    /// Fits the given shapes in the canvas.
    #[wasm_bindgen(js_name = fitTo)]
    pub fn fit_to(&self, ids: Vec<String>, padding: Option<f64>, max_zoom: Option<f64>) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.fit_to(&ids, padding.unwrap_or(40.0), max_zoom).map(|_| ()))
    }

    /// Sets the size in CSS pixels and the device pixel ratio.
    pub fn resize(&self, width: f64, height: f64, device_pixel_ratio: f64) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.resize(width, height, device_pixel_ratio))
    }

    /// World units to screen pixels: `[x, y]`.
    #[wasm_bindgen(js_name = worldToScreen)]
    pub fn world_to_screen(&self, x: f64, y: f64) -> Result<Vec<f64>, JsError> {
        self.shared.read(|st| Ok(st.core.world_to_screen(x, y).to_vec()))
    }

    /// Screen pixels to world units: `[x, y]`.
    #[wasm_bindgen(js_name = screenToWorld)]
    pub fn screen_to_world(&self, x: f64, y: f64) -> Result<Vec<f64>, JsError> {
        self.shared.read(|st| Ok(st.core.screen_to_world(x, y).to_vec()))
    }

    // ---- queries ----------------------------------------------------------------------------------------------

    /// The topmost shape at a screen point: `{ id, part }` or `null`.
    pub fn hit(&self, x: f64, y: f64, tolerance_px: Option<f64>) -> Result<JsValue, JsError> {
        let hit = self.shared.read(|st| Ok(st.core.hit(x, y, tolerance_px)))?;
        to_js(&hit)
    }

    /// Every shape at a screen point, topmost first.
    #[wasm_bindgen(js_name = hitAll)]
    pub fn hit_all(&self, x: f64, y: f64) -> Result<JsValue, JsError> {
        let hits = self.shared.read(|st| Ok(st.core.hit_all(x, y)))?;
        to_js(&hits)
    }

    /// What a pointer at a screen point would act on: a handle, a shape or nothing.
    pub fn pick(&self, x: f64, y: f64) -> Result<JsValue, JsError> {
        let target = self.shared.read(|st| Ok(st.core.pick(x, y)))?;
        to_js(&target)
    }

    /// The shapes a world rectangle `[x, y, w, h]` selects, `"contain"` or `"intersect"`.
    pub fn marquee(&self, rect: Vec<f64>, mode: Option<String>) -> Result<Vec<String>, JsError> {
        let rect: [f64; 4] = rect.try_into().map_err(|_| fail("a rectangle is [x, y, w, h]"))?;
        let mode = parse_marquee(mode.as_deref().unwrap_or("contain")).map_err(fail)?;
        self.shared.read(|st| st.core.marquee(rect, mode))
    }

    /// The shapes whose bounds touch a world rectangle `[x, y, w, h]`.
    #[wasm_bindgen(js_name = inRect)]
    pub fn in_rect(&self, rect: Vec<f64>) -> Result<Vec<String>, JsError> {
        let rect: [f64; 4] = rect.try_into().map_err(|_| fail("a rectangle is [x, y, w, h]"))?;
        self.shared.read(|st| st.core.in_rect(rect))
    }

    // ---- selection and transformer ----------------------------------------------------------------------------

    /// Tells the transformer which shapes to wrap.
    #[wasm_bindgen(js_name = setSelection)]
    pub fn set_selection(&self, ids: Vec<String>) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.set_selection(&ids))
    }

    /// The selected ids.
    pub fn selection(&self) -> Result<Vec<String>, JsError> {
        self.shared.read(|st| Ok(st.core.selection()))
    }

    /// Changes whenever the selection does.
    #[wasm_bindgen(js_name = selectionVersion)]
    pub fn selection_version(&self) -> Result<u32, JsError> {
        self.shared.read(|st| Ok(st.core.selection_version()))
    }

    /// The group whose members a click selects one by one, or `null`.
    #[wasm_bindgen(js_name = setEnteredGroup)]
    pub fn set_entered_group(&self, group: Option<String>) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.set_entered_group(group.as_deref()))
    }

    /// What a drag with the primary button does: `"select"`, `"hand"` or `"draw"`.
    #[wasm_bindgen(js_name = setPointerMode)]
    pub fn set_pointer_mode(&self, mode: String) -> Result<(), JsError> {
        let mode = parse_mode(&mode).map_err(fail)?;
        self.shared.mutate(move |st| {
            st.core.set_pointer_mode(mode);
            Ok(())
        })
    }

    /// Shapes to outline as targets.
    #[wasm_bindgen(js_name = setHighlights)]
    pub fn set_highlights(&self, ids: Vec<String>) -> Result<(), JsError> {
        self.shared.mutate(move |st| st.core.set_highlights(&ids))
    }

    /// The people in the room: `[{ id, name?, color, cursor?, selection?, trail? }]`. The stage draws each one's
    /// pointer with a name label, an outline round their selection and their laser trail. Replaces the list.
    #[wasm_bindgen(js_name = setRemoteUsers)]
    pub fn set_remote_users(&self, users: JsValue) -> Result<(), JsError> {
        let users: Vec<RemoteUserSpec> = from_js(users)?;
        self.shared.mutate(move |st| st.core.set_remote_users(users))
    }

    /// What is drawn above the scene, for hosts that draw it themselves.
    pub fn overlay(&self) -> Result<JsValue, JsError> {
        let overlay = self.shared.read(|st| Ok(st.core.overlay()))?;
        to_js(&overlay)
    }

    // ---- events and input -------------------------------------------------------------------------------------

    /// Subscribes to an event. Returns a handle for `off`.
    pub fn on(&self, event: String, selector: String, callback: Function) -> Result<u32, JsError> {
        let kind = parse_event_kind(&event).map_err(fail)?;
        let selector: Selector = selector.parse().map_err(fail)?;
        let handle = self.shared.next_handle();
        self.shared.mutate(move |st| {
            st.core.on_as(handle, kind, selector, wrap_listener(callback));
            Ok(())
        })?;
        Ok(handle)
    }

    /// Removes a subscription.
    pub fn off(&self, handle: u32) -> Result<(), JsError> {
        self.shared.mutate(move |st| {
            st.core.off(handle);
            Ok(())
        })
    }

    /// Feeds one input event (`{ Pointer: {...} }`, `{ Wheel: {...} }`, ...). Listeners run, the built-in behaviour
    /// acts, and the intents come back.
    #[wasm_bindgen(js_name = handleInput)]
    pub fn handle_input(&self, event: JsValue) -> Result<JsValue, JsError> {
        let event: InputEvent = from_js(event)?;
        let intents = self.shared.write(|st| Ok(st.core.handle_input(&event)));
        self.shared.flush();
        to_js(&intents?)
    }

    /// Advances time to `now_ms` (a long press may be due).
    pub fn tick(&self, now_ms: f64) -> Result<JsValue, JsError> {
        let intents = self.shared.write(|st| Ok(st.core.tick(now_ms)));
        self.shared.flush();
        to_js(&intents?)
    }

    /// When `tick` next has something to do, in the clock of the events, or `undefined`.
    #[wasm_bindgen(js_name = nextDeadline)]
    pub fn next_deadline(&self) -> Result<Option<f64>, JsError> {
        self.shared.read(|st| Ok(st.core.next_deadline()))
    }

    // ---- drawing ----------------------------------------------------------------------------------------------

    /// True when a frame is due: something changed since the last one.
    #[wasm_bindgen(js_name = needsFrame)]
    pub fn needs_frame(&self) -> Result<bool, JsError> {
        self.shared.write(|st| Ok(st.core.needs_frame()))
    }

    /// Draws a frame if one is due. True when pixels were presented.
    pub fn render(&self) -> Result<bool, JsError> {
        self.shared.write(State::draw)
    }

    /// Frames drawn so far.
    #[wasm_bindgen(js_name = framesDrawn)]
    pub fn frames_drawn(&self) -> Result<f64, JsError> {
        self.shared.read(|st| Ok(st.frames as f64))
    }

    /// Light or dark rendering.
    #[wasm_bindgen(js_name = setTheme)]
    pub fn set_theme(&self, theme: String) -> Result<(), JsError> {
        let theme = match theme.as_str() {
            "light" => ThemeOption::Light,
            "dark" => ThemeOption::Dark,
            other => return Err(fail(format!("{other:?} is not a theme; use \"light\" or \"dark\""))),
        };
        self.shared.mutate(move |st| {
            st.core.set_theme(theme);
            Ok(())
        })
    }

    /// The background grid: `{ kind: "none" | "lines" | "rules" | "crosses" | "dots", spacing }`.
    #[wasm_bindgen(js_name = setGrid)]
    pub fn set_grid(&self, grid: JsValue) -> Result<(), JsError> {
        let grid: GridOptions = options_from_js(grid)?;
        self.shared.mutate(move |st| {
            st.core.set_grid(grid.into());
            Ok(())
        })
    }

    /// The canvas colour, `#rrggbb` or `#rrggbbaa`.
    #[wasm_bindgen(js_name = setBackground)]
    pub fn set_background(&self, color: String) -> Result<(), JsError> {
        let color = Color::parse(&color).map_err(fail)?;
        self.shared.mutate(move |st| {
            st.core.set_background(color);
            Ok(())
        })
    }

    // ---- device -----------------------------------------------------------------------------------------------

    /// The adapter, the sizes and the counters of the last frame, for bug reports.
    pub fn stats(&self) -> Result<JsValue, JsError> {
        let value = self.shared.read(|st| {
            let report = st.core.frame_report();
            Ok(serde_json::json!({
                "elements": st.core.len(),
                "framesDrawn": st.frames,
                "adapter": st.renderer.as_ref().map(Renderer::adapter),
                "lost": st.renderer.as_ref().and_then(Renderer::lost).map(|l| l.message),
                "lastFrame": {
                    "inView": report.in_view,
                    "culled": report.culled,
                    "drawn": report.drawn,
                    "encoded": report.cache_misses,
                    "reused": report.cache_hits + report.cache_reuses,
                },
            }))
        })?;
        to_js(&value)
    }

    /// The GPU device was lost (a driver reset). Call `recover`.
    #[wasm_bindgen(js_name = isLost)]
    pub fn is_lost(&self) -> Result<bool, JsError> {
        self.shared.read(|st| Ok(st.renderer.as_ref().and_then(Renderer::lost).is_some()))
    }

    /// Starts a new device after a loss and draws again.
    pub async fn recover(&self) -> Result<(), JsError> {
        let (canvas, size) = {
            let mut st = self.shared.cell().try_borrow_mut().map_err(|_| fail("the stage is busy"))?;
            st.renderer = None;
            (st.canvas.clone(), st.device_size())
        };
        let renderer = Renderer::for_canvas(canvas, size).await.map_err(fail)?;
        let mut st = self.shared.cell().try_borrow_mut().map_err(|_| fail("the stage is busy"))?;
        st.renderer = Some(renderer);
        st.core.request_frame();
        Ok(())
    }

    /// Releases the GPU device. The stage cannot draw afterwards.
    pub fn destroy(&self) -> Result<(), JsError> {
        self.shared.write(|st| {
            st.renderer = None;
            Ok(())
        })
    }
}

/// The options without the keys the browser glue reads (`StageOptions` rejects unknown keys).
fn options_without_mount(options: &JsValue) -> Result<JsValue, JsError> {
    if options.is_undefined() || options.is_null() {
        return Ok(JsValue::UNDEFINED);
    }
    let copy = js_sys::Object::assign(&js_sys::Object::new(), &js_sys::Object::from(options.clone()));
    for key in ["width", "height", "devicePixelRatio"] {
        let _ = js_sys::Reflect::delete_property(&copy, &JsValue::from_str(key));
    }
    Ok(copy.into())
}
