//! [`Canvas`]: the facade that ties a store, the spatial index, the viewport, the recognizer and the dispatcher
//! together.
//!
//! ```
//! use drafftink_canvas::{Canvas, Viewport, EventKind, Intent, InputEvent, PointerEvent, PointerPhase};
//! use drafftink_canvas::scene::{Element, ElementId, ElementKind, MemoryStore, SceneRead, SceneTx};
//! use drafftink_canvas::geom::kurbo::{Point, Size};
//!
//! let mut canvas = Canvas::new(MemoryStore::new(), Viewport::new(Size::new(800.0, 600.0), 1.0));
//! let id = ElementId::new("box").unwrap();
//! canvas
//!     .transact(|tx| {
//!         let z = tx.next_top_index().unwrap();
//!         tx.insert(Element::new(id.clone(), ElementKind::Rectangle, z).with_bounds(-50.0, -50.0, 100.0, 100.0))
//!     })
//!     .unwrap();
//!
//! // Hit testing in screen pixels: the centre of the viewport is the world origin.
//! assert_eq!(canvas.hit_at(Point::new(400.0, 300.0)), None, "an unfilled rectangle is hit on its outline only");
//! assert_eq!(canvas.hit_at(Point::new(350.0, 300.0)).map(|h| h.id), Some(id.clone()));
//!
//! // Input in, intents out.
//! let intents = canvas.handle_input(&InputEvent::Pointer(PointerEvent::mouse(PointerPhase::Move, 350.0, 300.0, 0.0)));
//! assert!(matches!(intents[0], Intent::Hover { .. }));
//! ```
//!
//! # Keeping the index in step
//!
//! The canvas subscribes to its store. Every commit, from [`Canvas::transact`], from [`Canvas::store_mut`], from a
//! remote update applied inside the store, queues its `ChangeSet`; the queue is applied to the index (incrementally,
//! see [`crate::SceneIndex`]) before the next query, input event or tick, and when a [`StoreMut`] guard drops.
//! Queries therefore always see the current scene.
//!
//! # Redraws
//!
//! [`Canvas::take_dirty`] reports what changed since the last call: the scene (re-encode changed elements), the
//! camera (re-composite) or the overlay (redraw handles and outlines only). An idle pointer over an unchanged
//! target changes nothing, so a host that redraws only when `take_dirty` says so draws zero frames while hovering.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use drafftink_geom::kurbo::{Point, Rect, Size, Vec2};
use drafftink_geom::{OrientedBox, aabb};
use drafftink_scene::{ChangeSet, Element, ElementId, GroupId, SceneStore, SceneTx, StoreError, SubscriptionId};

use crate::config::{CanvasConfig, PointerMode};
use crate::dispatch::{Dispatcher, ElementEvent, EventKind, Listener, ListenerId, Node, Selector};
use crate::index::{Hit, HitOptions, MarqueeMode, SceneIndex, unit_group};
use crate::input::{InputEvent, PointerKind};
use crate::intent::{Cursor, Handle, Intent, Meta, Pos, Target};
use crate::overlay::{
    BindCandidate, HoverOutline, MarqueeOverlay, Overlay, OverlayInputs, RemoteCursor, RemoteSelection, RemoteUser,
    SelectionBox, TransformerOverlay,
};
use crate::recognizer::{Picker, Recognizer};
use crate::shape::{element_box, shape_of};
use crate::transformer::SelectionFrame;
use crate::viewport::{Camera, Viewport};

/// What needs redrawing, as reported by [`Canvas::take_dirty`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dirty {
    /// Elements changed: re-encode them.
    pub scene: bool,
    /// The camera or the viewport size changed: re-composite.
    pub camera: bool,
    /// Something in the overlay changed: redraw the overlay.
    pub overlay: bool,
}

impl Dirty {
    /// True when any part needs a redraw.
    pub fn any(self) -> bool {
        self.scene || self.camera || self.overlay
    }
}

/// Mutable access to the store. Dropping the guard brings the index up to date.
pub struct StoreMut<'a, S: SceneStore> {
    canvas: &'a mut Canvas<S>,
}

impl<S: SceneStore> Deref for StoreMut<'_, S> {
    type Target = S;
    fn deref(&self) -> &S {
        &self.canvas.store
    }
}

impl<S: SceneStore> DerefMut for StoreMut<'_, S> {
    fn deref_mut(&mut self) -> &mut S {
        &mut self.canvas.store
    }
}

impl<S: SceneStore> Drop for StoreMut<'_, S> {
    fn drop(&mut self) {
        self.canvas.sync_inner();
    }
}

/// The reusable canvas. See the module docs and `docs/canvas-api.md`.
pub struct Canvas<S: SceneStore> {
    store: S,
    index: SceneIndex,
    viewport: Viewport,
    recognizer: Recognizer,
    dispatcher: Dispatcher,
    config: CanvasConfig,
    selection: Vec<ElementId>,
    selection_set: HashSet<ElementId>,
    frame: SelectionFrame,
    entered_group: Option<GroupId>,
    inputs: OverlayInputs,
    marquee: Option<MarqueeOverlay>,
    active_handle: Option<Handle>,
    queue: Rc<RefCell<Vec<ChangeSet>>>,
    subscription: SubscriptionId,
    pending: Vec<Intent>,
    dirty: Dirty,
}

/// The picker the recognizer uses: borrows the canvas fields it needs, so the recognizer itself stays borrowable.
struct PickCtx<'a, S: SceneStore> {
    store: &'a S,
    index: &'a SceneIndex,
    viewport: &'a Viewport,
    frame: &'a SelectionFrame,
    config: &'a CanvasConfig,
    entered: Option<&'a GroupId>,
}

impl<S: SceneStore> Picker for PickCtx<'_, S> {
    fn viewport(&self) -> &Viewport {
        self.viewport
    }

    fn pick(&self, screen: Point, kind: PointerKind) -> Target {
        let touch = kind == PointerKind::Touch;
        let radius =
            if touch { self.config.transformer.hit_radius_touch_px } else { self.config.transformer.hit_radius_px };
        if let Some(h) = self.frame.hit(self.viewport, screen, radius, &self.config.transformer) {
            return Target::Handle(h);
        }
        let tol = if touch { self.config.hit_tolerance_touch_px } else { self.config.hit_tolerance_px };
        let world = self.viewport.screen_to_world(screen);
        match self.index.hit(self.store, world, self.viewport.zoom(), &HitOptions::with_tolerance(tol)) {
            Some(hit) => {
                let group = self
                    .store
                    .get(&hit.id)
                    .and_then(|e| unit_group(e, self.entered, |g| self.store.derived().is_group(g)));
                Target::Element { id: hit.id, unit_group: group }
            }
            None => Target::Empty,
        }
    }

    fn cursor(&self, target: &Target) -> Cursor {
        match target {
            Target::Handle(h) => self.frame.cursor_for(h),
            Target::Element { .. } => Cursor::Move,
            Target::Empty | Target::None => Cursor::Default,
        }
    }
}

macro_rules! picker {
    ($s:expr) => {
        PickCtx {
            store: &$s.store,
            index: &$s.index,
            viewport: &$s.viewport,
            frame: &$s.frame,
            config: &$s.config,
            entered: $s.entered_group.as_ref(),
        }
    };
}

impl<S: SceneStore> Canvas<S> {
    /// A canvas over `store` with default configuration.
    pub fn new(store: S, viewport: Viewport) -> Self {
        Self::with_config(store, viewport, CanvasConfig::default())
    }

    /// A canvas with explicit configuration. The index is built in bulk from the store's content.
    pub fn with_config(mut store: S, viewport: Viewport, config: CanvasConfig) -> Self {
        let queue: Rc<RefCell<Vec<ChangeSet>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&queue);
        let subscription = store.subscribe(Box::new(move |cs| sink.borrow_mut().push(cs.clone())));
        let index = SceneIndex::build(&store);
        Self {
            store,
            index,
            viewport,
            recognizer: Recognizer::new(config.input.clone()),
            dispatcher: Dispatcher::new(),
            config,
            selection: Vec::new(),
            selection_set: HashSet::new(),
            frame: SelectionFrame::None,
            entered_group: None,
            inputs: OverlayInputs::default(),
            marquee: None,
            active_handle: None,
            queue,
            subscription,
            pending: Vec::new(),
            dirty: Dirty { scene: true, camera: true, overlay: true },
        }
    }

    /// Takes the store back, unsubscribing from it.
    pub fn into_store(self) -> S {
        let Canvas { mut store, subscription, .. } = self;
        store.unsubscribe(subscription);
        store
    }

    // ---- store ----------------------------------------------------------------------------------------------

    /// Read access to the store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Write access to the store. The index follows when the guard drops.
    pub fn store_mut(&mut self) -> StoreMut<'_, S> {
        StoreMut { canvas: self }
    }

    /// Runs `f` in one transaction and commits it. On an error from `f` the transaction is dropped and nothing
    /// changes. Returns `f`'s value and the change set.
    pub fn transact<T>(
        &mut self,
        f: impl FnOnce(&mut S::Tx<'_>) -> Result<T, StoreError>,
    ) -> Result<(T, ChangeSet), StoreError> {
        let mut tx = self.store.begin();
        let value = f(&mut tx)?;
        let changes = tx.commit()?;
        self.sync_inner();
        Ok((value, changes))
    }

    /// The spatial index (read only).
    pub fn index(&self) -> &SceneIndex {
        &self.index
    }

    /// Applies queued store changes now and returns the intents they caused (a hover that changed because an
    /// element moved under the pointer). Normally unnecessary: every other call syncs first.
    pub fn sync(&mut self) -> Vec<Intent> {
        self.sync_inner();
        std::mem::take(&mut self.pending)
    }

    fn sync_inner(&mut self) {
        let batches = std::mem::take(&mut *self.queue.borrow_mut());
        if batches.is_empty() {
            return;
        }
        let mut touched = false;
        for cs in &batches {
            self.index.apply(&self.store, cs);
            if !cs.is_empty() {
                self.dirty.scene = true;
            }
            if !self.selection_set.is_empty() && cs.elements.iter().any(|c| self.selection_set.contains(&c.id)) {
                touched = true;
            }
            if !(self.inputs.remote.is_empty() && self.inputs.bind_candidates.is_empty()) && !cs.elements.is_empty() {
                self.dirty.overlay = true;
            }
            if let Some(h) = self.recognizer.hover().element()
                && cs.get(h).is_some()
            {
                self.dirty.overlay = true;
            }
        }
        if touched {
            self.frame = SelectionFrame::of(&self.store, self.selection.iter());
            self.dirty.overlay = true;
        }
        self.refresh_hover();
    }

    fn refresh_hover(&mut self) {
        let mut fresh = Vec::new();
        {
            let ctx = picker!(self);
            self.recognizer.refresh_hover(&ctx, &mut fresh);
        }
        self.absorb(fresh);
    }

    // ---- viewport -------------------------------------------------------------------------------------------

    /// The viewport.
    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    /// The camera.
    pub fn camera(&self) -> Camera {
        self.viewport.camera()
    }

    /// Sets the camera (zoom clamped). Returns true when it changed.
    pub fn set_camera(&mut self, camera: Camera) -> bool {
        let changed = self.viewport.set_camera(camera);
        self.viewport_changed(changed)
    }

    /// Resizes the canvas and sets the device pixel ratio. Returns true when either changed.
    pub fn set_viewport_size(&mut self, size: Size, dpr: f64) -> bool {
        let a = self.viewport.set_size(size);
        let b = self.viewport.set_dpr(dpr);
        self.viewport_changed(a || b)
    }

    /// Changes the viewport with `f`, which returns whether it changed anything.
    pub fn update_viewport(&mut self, f: impl FnOnce(&mut Viewport) -> bool) -> bool {
        let changed = f(&mut self.viewport);
        self.viewport_changed(changed)
    }

    /// Zooms around a screen point by a factor.
    pub fn zoom_at(&mut self, screen: Point, factor: f64) -> bool {
        self.update_viewport(|v| v.zoom_at(screen, factor))
    }

    /// Moves the content by `delta` logical pixels.
    pub fn pan_by_screen(&mut self, delta: Vec2) -> bool {
        self.update_viewport(|v| v.pan_by_screen(delta))
    }

    /// Fits the viewport to the bounds of everything in the scene. False for an empty scene.
    pub fn fit_to_content(&mut self, padding: f64, max_zoom: Option<f64>) -> bool {
        match self.index.total_bounds() {
            Some(b) => self.update_viewport(|v| v.fit_bounds(b, padding, max_zoom)),
            None => false,
        }
    }

    /// Fits the viewport to the given elements. False when none of them exists.
    pub fn fit_to_elements<'a>(
        &mut self,
        ids: impl IntoIterator<Item = &'a ElementId>,
        padding: f64,
        max_zoom: Option<f64>,
    ) -> bool {
        let bounds = aabb::union_all(ids.into_iter().filter_map(|id| self.index.bounds_of(id)));
        match bounds {
            Some(b) => self.update_viewport(|v| v.fit_bounds(b, padding, max_zoom)),
            None => false,
        }
    }

    fn viewport_changed(&mut self, changed: bool) -> bool {
        if changed {
            self.dirty.camera = true;
            self.dirty.overlay = true;
            self.sync_inner();
            self.refresh_hover();
        }
        changed
    }

    // ---- selection and context -------------------------------------------------------------------------------

    /// Sets the elements the transformer wraps (Konva's `transformer.nodes`). The canvas does not own selection
    /// rules: the editor decides what is selected and tells the canvas.
    pub fn set_selection(&mut self, ids: Vec<ElementId>) {
        if ids == self.selection {
            return;
        }
        self.selection_set = ids.iter().cloned().collect();
        self.selection = ids;
        self.frame = SelectionFrame::of(&self.store, self.selection.iter());
        self.dirty.overlay = true;
        self.refresh_hover();
    }

    /// The elements the transformer wraps.
    pub fn selection(&self) -> &[ElementId] {
        &self.selection
    }

    /// The frame and handles the selection gets.
    pub fn selection_frame(&self) -> &SelectionFrame {
        &self.frame
    }

    /// The group the user has entered (double-clicked into). Hits on its members resolve to the member, or to the
    /// next inner group, instead of the whole group.
    pub fn set_entered_group(&mut self, group: Option<GroupId>) {
        if group != self.entered_group {
            self.entered_group = group;
            self.refresh_hover();
        }
    }

    /// What a drag with the primary button means. See [`PointerMode`].
    pub fn set_pointer_mode(&mut self, mode: PointerMode) {
        self.recognizer.set_mode(mode);
    }

    /// The pointer mode.
    pub fn pointer_mode(&self) -> PointerMode {
        self.recognizer.mode()
    }

    /// The configuration.
    pub fn config(&self) -> &CanvasConfig {
        &self.config
    }

    /// Replaces the configuration. Takes effect at once.
    pub fn set_config(&mut self, config: CanvasConfig) {
        self.recognizer.set_config(config.input.clone());
        self.config = config;
        self.dirty.overlay = true;
    }

    // ---- queries ----------------------------------------------------------------------------------------------

    /// The topmost element at a screen point, with the default tolerance for a mouse.
    pub fn hit_at(&self, screen: Point) -> Option<Hit> {
        self.hit_with(screen, &HitOptions::with_tolerance(self.config.hit_tolerance_px))
    }

    /// The topmost element at a screen point with explicit options.
    pub fn hit_with(&self, screen: Point, opts: &HitOptions<'_>) -> Option<Hit> {
        let world = self.viewport.screen_to_world(screen);
        self.index.hit(&self.store, world, self.viewport.zoom(), opts)
    }

    /// Every element at a screen point, topmost first.
    pub fn hit_all_at(&self, screen: Point) -> Vec<Hit> {
        let world = self.viewport.screen_to_world(screen);
        let opts = HitOptions::with_tolerance(self.config.hit_tolerance_px);
        self.index.hit_all(&self.store, world, self.viewport.zoom(), &opts)
    }

    /// What the recognizer would report under a screen point: a handle, an element (with its unit group) or empty
    /// space.
    pub fn pick(&self, screen: Point, kind: PointerKind) -> Target {
        picker!(self).pick(screen, kind)
    }

    /// Elements selected by a world-space marquee, back to front, without locked ones.
    pub fn marquee(&self, rect: &Rect, mode: MarqueeMode) -> Vec<ElementId> {
        self.index.marquee(&self.store, rect, mode, false)
    }

    /// Live elements whose bounds touch a world rectangle, back to front.
    pub fn elements_in_rect(&self, rect: &Rect) -> Vec<&Element> {
        self.index.in_rect(&self.store, rect)
    }

    /// The elements to draw: those that touch the visible area (plus `margin_px` logical pixels), back to front.
    pub fn visible_elements(&self, margin_px: f64) -> Vec<&Element> {
        self.index.in_rect(&self.store, &self.viewport.visible_rect_padded(margin_px))
    }

    // ---- events (Konva-style) ---------------------------------------------------------------------------------

    /// Subscribes to element events. See [`crate::dispatch`] for the event model.
    pub fn on(&mut self, kind: EventKind, selector: Selector, callback: Listener) -> ListenerId {
        self.dispatcher.on(kind, selector, callback)
    }

    /// Removes a subscription.
    pub fn off(&mut self, id: ListenerId) -> bool {
        self.dispatcher.off(id)
    }

    /// Delivers an event the host built itself, along the bubble path of its target.
    pub fn dispatch(&mut self, mut event: ElementEvent) -> ElementEvent {
        self.dispatcher.dispatch(&self.store, &mut event);
        event
    }

    // ---- input ------------------------------------------------------------------------------------------------

    /// Feeds one input event. Returns the intents it caused (pans and zooms are already applied to the viewport),
    /// after delivering the matching events to the subscribers.
    pub fn handle_input(&mut self, ev: &InputEvent) -> Vec<Intent> {
        self.sync_inner();
        let mut fresh = Vec::new();
        {
            let ctx = picker!(self);
            self.recognizer.handle(ev, &ctx, &mut fresh);
        }
        self.absorb(fresh);
        std::mem::take(&mut self.pending)
    }

    /// Advances time: produces a long press when one is due. Call it at [`Canvas::next_deadline`].
    pub fn tick(&mut self, now_ms: f64) -> Vec<Intent> {
        self.sync_inner();
        let mut fresh = Vec::new();
        self.recognizer.tick(now_ms, &mut fresh);
        self.absorb(fresh);
        std::mem::take(&mut self.pending)
    }

    /// When [`Canvas::tick`] next has something to do, if ever. A host sleeps until then or until input.
    pub fn next_deadline(&self) -> Option<f64> {
        self.recognizer.next_deadline()
    }

    /// The current hover target.
    pub fn hover(&self) -> &Target {
        self.recognizer.hover()
    }

    /// Applies view intents to the viewport, tracks marquee and active handle, delivers events, sets dirty flags,
    /// and queues the intents in `pending`.
    fn absorb(&mut self, fresh: Vec<Intent>) {
        let mut view_changed = false;
        for intent in &fresh {
            match intent {
                Intent::Pan { delta, .. } => view_changed |= self.viewport.pan_by_screen(*delta),
                Intent::Zoom { at, factor, .. } => view_changed |= self.viewport.zoom_at(*at, *factor),
                Intent::DragStart { origin, at, target, .. } => match target {
                    Target::Handle(h) => {
                        self.active_handle = Some(h.clone());
                        self.dirty.overlay = true;
                    }
                    Target::Empty if self.recognizer.mode() == PointerMode::Select => {
                        self.marquee = Some(MarqueeOverlay { rect: world_rect(origin.world, at.world) });
                        self.dirty.overlay = true;
                    }
                    _ => {}
                },
                Intent::DragUpdate { origin, at, .. } => {
                    if self.marquee.is_some() {
                        self.marquee = Some(MarqueeOverlay { rect: world_rect(origin.world, at.world) });
                        self.dirty.overlay = true;
                    }
                }
                Intent::DragEnd { .. } => {
                    if self.marquee.take().is_some() || self.active_handle.take().is_some() {
                        self.dirty.overlay = true;
                    }
                }
                Intent::Hover { .. } => self.dirty.overlay = true,
                _ => {}
            }
            self.emit_events(intent);
        }
        self.pending.extend(fresh);
        if view_changed {
            self.dirty.camera = true;
            self.dirty.overlay = true;
            self.refresh_hover();
        }
    }

    fn emit_events(&mut self, intent: &Intent) {
        if self.dispatcher.is_empty() {
            return;
        }
        match intent {
            Intent::Press { at, target, meta } => {
                self.fire(EventKind::PointerDown, target, *at, meta, Vec2::ZERO, Vec2::ZERO, false)
            }
            Intent::Click { at, target, meta } => {
                self.fire(EventKind::Click, target, *at, meta, Vec2::ZERO, Vec2::ZERO, false)
            }
            Intent::DoubleClick { at, target, meta } => {
                self.fire(EventKind::DoubleClick, target, *at, meta, Vec2::ZERO, Vec2::ZERO, false)
            }
            Intent::LongPress { at, target, meta } => {
                self.fire(EventKind::LongPress, target, *at, meta, Vec2::ZERO, Vec2::ZERO, false)
            }
            Intent::ContextMenu { at, target, meta } => {
                self.fire(EventKind::ContextMenu, target, *at, meta, Vec2::ZERO, Vec2::ZERO, false)
            }
            Intent::DragStart { at, target, meta, .. } => {
                self.fire(EventKind::DragStart, target, *at, meta, Vec2::ZERO, Vec2::ZERO, false)
            }
            Intent::DragUpdate { at, delta_screen, delta_world, target, meta, .. } => {
                self.fire(EventKind::DragMove, target, *at, meta, *delta_screen, *delta_world, false);
            }
            Intent::DragEnd { at, delta_screen, delta_world, target, cancelled, meta, .. } => {
                self.fire(EventKind::DragEnd, target, *at, meta, *delta_screen, *delta_world, *cancelled);
            }
            Intent::Hover { from, to, at, .. } => self.fire_hover(from, to, *at),
            Intent::Pan { .. } | Intent::Zoom { .. } | Intent::ViewEnd => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fire(&mut self, kind: EventKind, target: &Target, at: Pos, meta: &Meta, ds: Vec2, dw: Vec2, cancelled: bool) {
        let (kind, element, handle) = match target {
            Target::Element { id, .. } => (kind, Some(id.clone()), None),
            Target::Handle(h) => {
                let k = match kind {
                    EventKind::DragStart => EventKind::TransformStart,
                    EventKind::DragMove => EventKind::Transform,
                    EventKind::DragEnd => EventKind::TransformEnd,
                    other => other,
                };
                (k, None, Some(h.clone()))
            }
            Target::Empty | Target::None => (kind, None, None),
        };
        if !self.dispatcher.wants(kind) {
            return;
        }
        let mut ev = ElementEvent::new(kind, element, at);
        ev.delta_screen = ds;
        ev.delta_world = dw;
        ev.modifiers = meta.modifiers;
        ev.pointer = meta.kind;
        ev.button = meta.button;
        ev.handle = handle;
        ev.cancelled = cancelled;
        self.dispatcher.dispatch(&self.store, &mut ev);
    }

    /// Leave events for the element and groups the pointer left, then enter events for those it entered. Neither
    /// bubbles.
    fn fire_hover(&mut self, from: &Target, to: &Target, at: Pos) {
        if !(self.dispatcher.wants(EventKind::PointerEnter) || self.dispatcher.wants(EventKind::PointerLeave)) {
            return;
        }
        let groups = |t: &Target| -> Vec<GroupId> {
            t.element()
                .and_then(|id| self.store.get(id))
                .map(|e| e.group_ids.iter().cloned().collect())
                .unwrap_or_default()
        };
        let (from_groups, to_groups) = (groups(from), groups(to));
        let leaving: Vec<GroupId> = from_groups.iter().filter(|g| !to_groups.contains(g)).cloned().collect();
        let entering: Vec<GroupId> = to_groups.iter().filter(|g| !from_groups.contains(g)).cloned().collect();
        for (kind, target, nodes) in [(EventKind::PointerLeave, from, leaving), (EventKind::PointerEnter, to, entering)]
        {
            let Some(id) = target.element() else { continue };
            let mut path = vec![Node::Element(id.clone())];
            path.extend(nodes.into_iter().map(Node::Group));
            let mut ev = ElementEvent::new(kind, Some(id.clone()), at);
            self.dispatcher.dispatch_along(&self.store, &mut ev, &path);
        }
    }

    // ---- overlay ----------------------------------------------------------------------------------------------

    /// Sets the smart guides to draw. Returns true when they changed.
    pub fn set_guides(&mut self, guides: Vec<drafftink_geom::guides::SmartGuide>) -> bool {
        if guides == self.inputs.guides {
            return false;
        }
        self.inputs.guides = guides;
        self.dirty.overlay = true;
        true
    }

    /// Sets the elements to highlight as binding candidates. Returns true when they changed.
    pub fn set_bind_candidates(&mut self, ids: Vec<ElementId>) -> bool {
        if ids == self.inputs.bind_candidates {
            return false;
        }
        self.inputs.bind_candidates = ids;
        self.dirty.overlay = true;
        true
    }

    /// Sets the collaborators to show. Returns true when they changed.
    pub fn set_remote_users(&mut self, users: Vec<RemoteUser>) -> bool {
        if users == self.inputs.remote {
            return false;
        }
        self.inputs.remote = users;
        self.dirty.overlay = true;
        true
    }

    /// Builds the overlay for the current state.
    pub fn overlay(&self) -> Overlay {
        let cfg = &self.config;
        let mut o = Overlay { zoom: self.viewport.zoom(), ..Overlay::default() };
        let linear = matches!(self.frame, SelectionFrame::Linear { .. });
        if !linear && self.selection.len() <= cfg.max_selection_boxes {
            o.selection = self
                .selection
                .iter()
                .filter_map(|id| self.store.get(id))
                .filter(|e| !e.deleted)
                .map(|e| SelectionBox { id: e.id.clone(), obox: element_box(e) })
                .collect();
        }
        if !self.frame.is_none() {
            o.transformer = Some(TransformerOverlay {
                frame: self.frame.clone(),
                handles: self.frame.handles(&self.viewport, &cfg.transformer),
                handle_size_px: cfg.transformer.handle_size_px,
                active: self.active_handle.clone(),
            });
        }
        if let Some(id) = self.recognizer.hover().element()
            && !self.selection_set.contains(id)
            && let Some(shape) = self.store.get(id).and_then(shape_of)
        {
            o.hover = Some(HoverOutline { id: id.clone(), shape });
        }
        o.marquee = self.marquee;
        o.guides = self.inputs.guides.clone();
        o.bind_candidates = self
            .inputs
            .bind_candidates
            .iter()
            .filter_map(|id| self.store.get(id).filter(|e| !e.deleted))
            .filter_map(|e| shape_of(e).map(|shape| BindCandidate { target: e.id.clone(), shape }))
            .collect();
        for u in &self.inputs.remote {
            if let Some(pos) = u.cursor {
                o.remote_cursors.push(RemoteCursor { user: u.id.clone(), name: u.name.clone(), color: u.color, pos });
            }
            if !u.selection.is_empty() {
                let cap = cfg.max_remote_selection_boxes;
                let boxes: Vec<OrientedBox> = u
                    .selection
                    .iter()
                    .filter_map(|id| self.store.get(id))
                    .filter(|e| !e.deleted)
                    .take(cap)
                    .map(element_box)
                    .collect();
                o.remote_selections.push(RemoteSelection {
                    user: u.id.clone(),
                    color: u.color,
                    truncated: u.selection.len() > cap,
                    boxes,
                });
            }
        }
        o
    }

    /// What changed since the last call, then resets. A host redraws only when [`Dirty::any`].
    pub fn take_dirty(&mut self) -> Dirty {
        self.sync_inner();
        std::mem::take(&mut self.dirty)
    }
}

fn world_rect(a: Point, b: Point) -> Rect {
    Rect::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}
