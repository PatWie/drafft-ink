//! [`Core`]: the stage without a browser. A canvas over an in-memory store, the typesetter, the frame builder,
//! the listeners and the built-in pointer behaviour. The wasm `Stage` is a thin shell around it, so everything
//! here is tested natively.

use std::collections::HashMap;

use drafftink_canvas::{
    Camera, Canvas, CanvasConfig, Dirty, ElementEvent, EventKind, HitOptions, InputEvent, Intent, Listener, ListenerId,
    MarqueeMode, Overlay, PointerKind, Selector, Target, Viewport,
};
use drafftink_geom::kurbo::{Point, Rect, Size, Vec2};
use drafftink_render::{GridSpec, Scene};
use drafftink_scene::{
    Color, Document, Element, ElementId, MemoryStore, SceneRead, SceneStore, StoreError, load_document, snapshot,
};
use serde_json::Value;

use crate::dto::{CameraDto, HitDto, OverlayDto};
use crate::error::ApiError;
use crate::fonts::Fonts;
use crate::frame::Frame;
use crate::image::{self, ImageSpec};
use crate::interact::Gesture;
use crate::json;
use crate::ops::{self, Ids, Op, OpCx, Outcome};
use crate::options::{InteractionOptions, StageOptions};
use crate::remote::RemoteUserSpec;

/// Everything about a stage except the browser and the GPU.
pub struct Core {
    pub(crate) canvas: Canvas<MemoryStore>,
    pub(crate) fonts: Fonts,
    pub(crate) frame: Frame,
    ids: Ids,
    default_style: json::Obj,
    pub(crate) interaction: InteractionOptions,
    pub(crate) gesture: Option<Gesture>,
    listeners: HashMap<u32, ListenerId>,
    next_listener: u32,
    /// Changes the canvas reported but nobody has drawn yet.
    pending: Dirty,
    /// A frame was asked for for a reason the canvas does not track (resize, theme, an export).
    forced: bool,
    selection_version: u32,
}

/// Parses `#rrggbb` or `#rrggbbaa`.
fn parse_color(text: &str) -> Result<Color, ApiError> {
    Color::parse(text).map_err(|e| ApiError::invalid(format!("background: {e}")))
}

fn finite(values: &[f64], what: &str) -> Result<(), ApiError> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(ApiError::invalid(format!("{what} must be finite numbers")))
    }
}

/// Parses element ids from JS strings.
pub(crate) fn parse_ids<S: AsRef<str>>(ids: &[S]) -> Result<Vec<ElementId>, ApiError> {
    ids.iter().map(|s| Ok(ElementId::new(s.as_ref())?)).collect()
}

impl Core {
    /// A stage of `size` CSS pixels at `dpr` device pixels per CSS pixel.
    pub fn new(size: Size, dpr: f64, options: &StageOptions) -> Result<Self, ApiError> {
        finite(&[size.width, size.height, dpr], "the size and the device pixel ratio")?;
        let mut store = MemoryStore::new();
        let frame = Frame::new(&mut store, options.theme.into(), options.grid.into());
        let mut config = CanvasConfig::default();
        if let Some(patch) = &options.config {
            let mut value = serde_json::to_value(&config)?;
            json::merge_patch(&mut value, patch.clone());
            config = serde_json::from_value(value)?;
        }
        let canvas = Canvas::with_config(store, Viewport::new(size, dpr.max(f64::MIN_POSITIVE)), config);
        let mut core = Self {
            canvas,
            fonts: Fonts::default(),
            frame,
            ids: Ids::default(),
            default_style: options.default_style.clone(),
            interaction: options.interaction,
            gesture: None,
            listeners: HashMap::new(),
            next_listener: 0,
            pending: Dirty { scene: true, camera: true, overlay: true },
            forced: true,
            selection_version: 0,
        };
        if let Some(color) = &options.background {
            core.set_background(parse_color(color)?);
        }
        if let Some(doc) = &options.document {
            core.load_value(doc.clone())?;
        }
        Ok(core)
    }

    /// The canvas, read only.
    pub fn canvas(&self) -> &Canvas<MemoryStore> {
        &self.canvas
    }

    // ---- writing ----------------------------------------------------------------------------------------------

    /// Runs `f` in one transaction. An error from `f` leaves the scene as it was.
    pub(crate) fn run<T>(
        &mut self,
        f: impl FnOnce(&mut drafftink_scene::MemoryTx<'_>, &mut OpCx<'_>) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let Self { canvas, ids, fonts, default_style, .. } = self;
        let mut failure = None;
        let result = canvas.transact(|tx| {
            let mut cx = OpCx { ids, fonts, default_style };
            f(tx, &mut cx).map_err(|e| {
                failure = Some(e);
                StoreError::backend("the transaction was aborted")
            })
        });
        match (result, failure) {
            (Ok((value, _)), _) => Ok(value),
            (Err(_), Some(e)) => Err(e),
            (Err(e), None) => Err(e.into()),
        }
    }

    /// Runs a batch of operations in one transaction: one commit, or none when any step fails. Returns what each
    /// step produced, in order.
    pub(crate) fn apply(&mut self, ops: Vec<Op>) -> Result<Vec<Outcome>, ApiError> {
        self.run(|tx, cx| ops.into_iter().map(|op| ops::apply(tx, cx, op)).collect())
    }

    /// Parses a batch from JSON (`[{ op: "add", shape }, { op: "update", id, patch }, ...]`) and runs it.
    pub fn transact(&mut self, batch: Value) -> Result<Vec<Value>, ApiError> {
        let ops: Vec<Op> = serde_json::from_value(batch)?;
        Ok(self.apply(ops)?.into_iter().map(Outcome::into_json).collect())
    }

    /// A fresh id for a shape the caller wants to name before adding it.
    pub fn new_id(&mut self) -> String {
        self.ids.element(self.canvas.store()).as_str().to_owned()
    }

    /// Adds a shape on top of everything and returns its id.
    pub fn add(&mut self, shape: Value) -> Result<ElementId, ApiError> {
        match self.apply(vec![Op::Add { shape }])?.pop() {
            Some(Outcome::Added(id)) => Ok(id),
            _ => Err(ApiError::invalid("the shape was not added")),
        }
    }

    /// Adds a picture from the bytes of a PNG, JPEG, WebP, GIF, BMP or ICO file, on top of everything. The bytes are identified by
    /// their content, stored once in the scene and the shape gets the natural size of the picture unless `spec`
    /// says otherwise (`ImageSpec`). A damaged or unsupported file is refused and changes nothing.
    pub fn add_image(&mut self, bytes: &[u8], spec: &ImageSpec) -> Result<ElementId, ApiError> {
        let prepared = image::prepare(bytes, spec)?;
        self.run(|tx, cx| image::insert(tx, cx, prepared))
    }

    /// Merge-patches a shape.
    pub fn update(&mut self, id: &str, patch: Value) -> Result<(), ApiError> {
        self.apply(vec![Op::Update { id: ElementId::new(id)?, patch }]).map(|_| ())
    }

    /// Removes elements for good.
    pub fn remove(&mut self, ids: &[String]) -> Result<(), ApiError> {
        self.apply(vec![Op::Remove { ids: parse_ids(ids)? }]).map(|_| ())
    }

    /// Removes every element and file, and the selection.
    pub fn clear(&mut self) -> Result<(), ApiError> {
        self.load_document(&Document::default())
    }

    // ---- reading ----------------------------------------------------------------------------------------------

    /// One live element.
    pub fn get(&self, id: &str) -> Result<&Element, ApiError> {
        let id = ElementId::new(id)?;
        self.canvas.store().get(&id).filter(|e| !e.deleted).ok_or(ApiError::NotFound(id))
    }

    /// Live elements back to front.
    pub fn elements(&self) -> Vec<&Element> {
        self.canvas.store().iter().collect()
    }

    /// Number of live elements.
    pub fn len(&self) -> usize {
        self.canvas.store().len()
    }

    /// True when the scene has no live element.
    pub fn is_empty(&self) -> bool {
        self.canvas.store().is_empty()
    }

    /// The ids of the live elements a selector matches, back to front. See [`Selector`] for the forms.
    pub fn find(&self, selector: &str) -> Result<Vec<String>, ApiError> {
        let selector: Selector = selector.parse()?;
        Ok(self
            .canvas
            .store()
            .iter()
            .filter(|e| matches_element(&selector, e))
            .map(|e| e.id.as_str().to_owned())
            .collect())
    }

    /// The store's commit counter: changes whenever the scene does.
    pub fn revision(&self) -> u64 {
        self.canvas.store().revision()
    }

    // ---- documents --------------------------------------------------------------------------------------------

    /// The whole scene as a document, tombstones included.
    pub fn document(&self) -> Document {
        snapshot(self.canvas.store())
    }

    /// Replaces the scene with `doc` in one commit and clears the selection. A document with errors is refused.
    pub fn load_document(&mut self, doc: &Document) -> Result<(), ApiError> {
        self.set_selection_ids(Vec::new());
        self.gesture = None;
        let mut store = self.canvas.store_mut();
        load_document(&mut *store, doc)?;
        Ok(())
    }

    fn load_value(&mut self, value: Value) -> Result<(), ApiError> {
        let doc = Document::from_json(&value.to_string())?;
        self.load_document(&doc)
    }

    /// Replaces the scene with the content of a file; see [`crate::load`] for the formats.
    pub fn load_bytes(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        let mut ids = drafftink_scene::SeqIds::new();
        let text = self.fonts.svg_text();
        let doc = crate::load::read_document(bytes, &mut ids, &text)?;
        self.load_document(&doc)
    }

    /// The canvas colour.
    pub fn set_background(&mut self, color: Color) {
        let result = self.run(|tx, _| {
            drafftink_scene::SceneTx::set_background(tx, color);
            Ok(())
        });
        if let Err(e) = result {
            log::warn!("background not set: {e}");
        }
    }

    // ---- camera -----------------------------------------------------------------------------------------------

    /// The camera.
    pub fn camera(&self) -> CameraDto {
        self.canvas.camera().into()
    }

    /// Sets the world point at the centre and/or the zoom (clamped to 0.1 to 30).
    pub fn set_camera(&mut self, scroll: Option<[f64; 2]>, zoom: Option<f64>) -> Result<(), ApiError> {
        let now = self.canvas.camera();
        let [x, y] = scroll.unwrap_or([now.scroll.x, now.scroll.y]);
        let zoom = zoom.unwrap_or(now.zoom);
        finite(&[x, y, zoom], "the camera")?;
        self.canvas.set_camera(Camera { scroll: Point::new(x, y), zoom });
        Ok(())
    }

    /// Moves the content by `dx`, `dy` logical pixels.
    pub fn pan_by(&mut self, dx: f64, dy: f64) -> Result<(), ApiError> {
        finite(&[dx, dy], "the pan distance")?;
        self.canvas.pan_by_screen(Vec2::new(dx, dy));
        Ok(())
    }

    /// Zooms by `factor` keeping the world point under the screen point `(x, y)` where it is.
    pub fn zoom_at(&mut self, x: f64, y: f64, factor: f64) -> Result<(), ApiError> {
        finite(&[x, y, factor], "the zoom point and factor")?;
        self.canvas.zoom_at(Point::new(x, y), factor);
        Ok(())
    }

    /// Fits the content in the canvas. False for an empty scene.
    pub fn fit_to_content(&mut self, padding: f64, max_zoom: Option<f64>) -> Result<bool, ApiError> {
        finite(&[padding], "the padding")?;
        Ok(self.canvas.fit_to_content(padding.max(0.0), max_zoom))
    }

    /// Fits the given elements in the canvas. False when none of them exists.
    pub fn fit_to(&mut self, ids: &[String], padding: f64, max_zoom: Option<f64>) -> Result<bool, ApiError> {
        finite(&[padding], "the padding")?;
        let ids = parse_ids(ids)?;
        Ok(self.canvas.fit_to_elements(ids.iter(), padding.max(0.0), max_zoom))
    }

    /// Resizes the canvas (CSS pixels) and sets the device pixel ratio.
    pub fn resize(&mut self, width: f64, height: f64, dpr: f64) -> Result<(), ApiError> {
        finite(&[width, height, dpr], "the size and the device pixel ratio")?;
        if width <= 0.0 || height <= 0.0 || dpr <= 0.0 {
            return Err(ApiError::invalid("the size and the device pixel ratio must be above zero"));
        }
        self.canvas.set_viewport_size(Size::new(width, height), dpr);
        Ok(())
    }

    /// The viewport.
    pub fn viewport(&self) -> &Viewport {
        self.canvas.viewport()
    }

    /// Screen pixels to world units.
    pub fn screen_to_world(&self, x: f64, y: f64) -> [f64; 2] {
        let p = self.canvas.viewport().screen_to_world(Point::new(x, y));
        [p.x, p.y]
    }

    /// World units to screen pixels.
    pub fn world_to_screen(&self, x: f64, y: f64) -> [f64; 2] {
        let p = self.canvas.viewport().world_to_screen(Point::new(x, y));
        [p.x, p.y]
    }

    // ---- queries ----------------------------------------------------------------------------------------------

    /// The topmost element at a screen point. `tolerance_px` defaults to the configured reach.
    pub fn hit(&self, x: f64, y: f64, tolerance_px: Option<f64>) -> Option<HitDto> {
        let opts = HitOptions::with_tolerance(tolerance_px.unwrap_or(self.canvas.config().hit_tolerance_px));
        self.canvas.hit_with(Point::new(x, y), &opts).as_ref().map(HitDto::from)
    }

    /// Every element at a screen point, topmost first.
    pub fn hit_all(&self, x: f64, y: f64) -> Vec<HitDto> {
        self.canvas.hit_all_at(Point::new(x, y)).iter().map(HitDto::from).collect()
    }

    /// What the pointer would act on at a screen point: a transformer handle, an element or nothing.
    pub fn pick(&self, x: f64, y: f64) -> Target {
        self.canvas.pick(Point::new(x, y), PointerKind::Mouse)
    }

    /// Elements selected by a world rectangle `[x, y, w, h]`, back to front.
    pub fn marquee(&self, rect: [f64; 4], mode: MarqueeMode) -> Result<Vec<String>, ApiError> {
        let rect = rect_of(rect)?;
        Ok(self.canvas.marquee(&rect, mode).iter().map(|id| id.as_str().to_owned()).collect())
    }

    /// Live elements whose bounds touch a world rectangle `[x, y, w, h]`, back to front.
    pub fn in_rect(&self, rect: [f64; 4]) -> Result<Vec<String>, ApiError> {
        let rect = rect_of(rect)?;
        Ok(self.canvas.elements_in_rect(&rect).iter().map(|e| e.id.as_str().to_owned()).collect())
    }

    // ---- selection --------------------------------------------------------------------------------------------

    /// Tells the transformer what to wrap. Every id must be a live element.
    pub fn set_selection(&mut self, ids: &[String]) -> Result<(), ApiError> {
        let ids = parse_ids(ids)?;
        if let Some(missing) = ids.iter().find(|id| self.canvas.store().get(id).is_none_or(|e| e.deleted)) {
            return Err(ApiError::NotFound(missing.clone()));
        }
        self.set_selection_ids(ids);
        Ok(())
    }

    pub(crate) fn set_selection_ids(&mut self, ids: Vec<ElementId>) {
        if ids != self.canvas.selection() {
            self.selection_version = self.selection_version.wrapping_add(1);
        }
        self.canvas.set_selection(ids);
    }

    /// The selected ids.
    pub fn selection(&self) -> Vec<String> {
        self.canvas.selection().iter().map(|id| id.as_str().to_owned()).collect()
    }

    /// Changes whenever the selection does.
    pub fn selection_version(&self) -> u32 {
        self.selection_version
    }

    /// What a drag with the primary button does.
    pub fn set_pointer_mode(&mut self, mode: drafftink_canvas::PointerMode) {
        self.canvas.set_pointer_mode(mode);
    }

    // ---- events and input -------------------------------------------------------------------------------------

    /// Subscribes to an event. Returns a handle for [`Core::off`].
    pub fn on(&mut self, kind: EventKind, selector: Selector, listener: Listener) -> u32 {
        self.next_listener += 1;
        let handle = self.next_listener;
        self.on_as(handle, kind, selector, listener);
        handle
    }

    /// Subscribes under a handle the caller chose (the wasm stage hands handles out before it can subscribe, when a
    /// listener adds a listener). A handle that is in use is replaced.
    pub fn on_as(&mut self, handle: u32, kind: EventKind, selector: Selector, listener: Listener) {
        let id = self.canvas.on(kind, selector, listener);
        if let Some(old) = self.listeners.insert(handle, id) {
            self.canvas.off(old);
        }
    }

    /// The group whose members a click selects one by one (set after a double click into a group).
    pub fn set_entered_group(&mut self, group: Option<&str>) -> Result<(), ApiError> {
        let group = group.map(drafftink_scene::GroupId::new).transpose()?;
        self.canvas.set_entered_group(group);
        Ok(())
    }

    /// Removes a subscription. False when the handle is unknown.
    pub fn off(&mut self, handle: u32) -> bool {
        self.listeners.remove(&handle).is_some_and(|id| self.canvas.off(id))
    }

    /// Delivers an event the host built itself, along the bubble path of its target.
    pub fn dispatch(&mut self, event: ElementEvent) -> ElementEvent {
        self.canvas.dispatch(event)
    }

    /// Feeds one input event. Listeners run, the built-in behaviour acts, and the intents come back.
    pub fn handle_input(&mut self, event: &InputEvent) -> Vec<Intent> {
        let intents = self.canvas.handle_input(event);
        self.interact(&intents);
        intents
    }

    /// Advances time to `now_ms`: a long press may be due.
    pub fn tick(&mut self, now_ms: f64) -> Vec<Intent> {
        let intents = self.canvas.tick(now_ms);
        self.interact(&intents);
        intents
    }

    /// When [`Core::tick`] next has something to do.
    pub fn next_deadline(&self) -> Option<f64> {
        self.canvas.next_deadline()
    }

    // ---- drawing ----------------------------------------------------------------------------------------------

    /// What the canvas reported changed, together with what it reported earlier and was not drawn yet.
    fn absorb(&mut self) {
        let d = self.canvas.take_dirty();
        self.pending = Dirty {
            scene: self.pending.scene || d.scene,
            camera: self.pending.camera || d.camera,
            overlay: self.pending.overlay || d.overlay,
        };
    }

    /// Whether a frame is due. Nothing is due when nothing changed: an idle stage draws nothing.
    pub fn needs_frame(&mut self) -> bool {
        self.absorb();
        self.forced || self.pending.any() || self.frame.wants_frame()
    }

    /// Asks for a frame for a reason the canvas does not track.
    pub fn request_frame(&mut self) {
        self.forced = true;
    }

    /// The scene to draw now, and the canvas colour. `None` when nothing is due. The caller draws it and then
    /// calls [`Core::frame_done`].
    pub fn build_frame(&mut self) -> Option<(&Scene, peniko::Color)> {
        if !self.needs_frame() {
            return None;
        }
        let dirty = Dirty {
            scene: self.pending.scene || self.forced,
            camera: self.pending.camera || self.forced,
            overlay: self.pending.overlay || self.forced,
        };
        let Self { canvas, fonts, frame, .. } = self;
        Some(frame.compose(canvas, fonts, dirty))
    }

    /// Marks the frame built by [`Core::build_frame`] as drawn.
    pub fn frame_done(&mut self) {
        self.pending = Dirty::default();
        self.forced = false;
    }

    /// Light or dark rendering.
    pub fn set_theme(&mut self, theme: crate::options::ThemeOption) {
        self.frame.set_theme(theme.into());
        self.forced = true;
    }

    /// The background grid.
    pub fn set_grid(&mut self, grid: GridSpec) {
        self.frame.set_grid(grid);
        self.forced = true;
    }

    /// The overlay for hosts that draw it themselves.
    pub fn overlay(&self) -> OverlayDto {
        OverlayDto::new(&self.canvas.overlay(), self.canvas.viewport())
    }

    /// The canvas crate's own overlay description.
    pub fn raw_overlay(&self) -> Overlay {
        self.canvas.overlay()
    }

    /// Elements highlighted as targets (for example while dragging something over them).
    pub fn set_highlights(&mut self, ids: &[String]) -> Result<(), ApiError> {
        self.canvas.set_bind_candidates(parse_ids(ids)?);
        Ok(())
    }

    /// The collaborators to show: a pointer with a name, an outline round each selection, a laser trail. Replaces
    /// the previous list; an empty list shows nobody.
    pub fn set_remote_users(&mut self, users: Vec<RemoteUserSpec>) -> Result<(), ApiError> {
        let users = users.into_iter().map(RemoteUserSpec::into_user).collect::<Result<Vec<_>, _>>()?;
        self.canvas.set_remote_users(users);
        Ok(())
    }

    /// How many elements the picture of the last frame left out or drew, for tests and profiling.
    pub fn frame_report(&self) -> drafftink_render::FrameReport {
        self.frame.report()
    }
}

/// Whether a selector matches an element as a listener's selector would (`stage` matches no element).
fn matches_element(selector: &Selector, e: &Element) -> bool {
    match selector {
        Selector::Any => true,
        Selector::Stage => false,
        Selector::Id(id) => e.id == *id,
        Selector::Kind(kind) => e.tag() == *kind,
        Selector::Name(word) => e.name.as_deref().is_some_and(|n| n.split_whitespace().any(|w| w == word)),
        Selector::Group(group) => e.group_ids.contains(group),
    }
}

fn rect_of([x, y, w, h]: [f64; 4]) -> Result<Rect, ApiError> {
    finite(&[x, y, w, h], "the rectangle")?;
    Ok(Rect::new(x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)))
}
