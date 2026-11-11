//! The interaction layer's picture of the scene: viewport, spatial index and selection frame.
//!
//! The session owns the document. This module keeps a [`SceneIndex`] of it for hit testing, and the
//! [`Viewport`] the recognizer converts positions with. Both are caches:
//!
//! * The index follows the working store by a scan that compares each element's `version` and transform with
//!   what was indexed. The scan runs only when the store revision moved, and never while a gesture runs (live
//!   frames rewrite elements many times, and nothing hit-tests during a drag).
//! * The viewport follows the session camera (`view.set`, `view.fit`, ...) unless the interaction layer moved
//!   it itself and has not reported it yet.

use std::collections::HashMap;

use drafftink_canvas::{
    Camera, CanvasConfig, Cursor, Handle, Hit, HitOptions, MarqueeMode, Picker, PointerKind, SceneIndex,
    SelectionFrame, Target, Viewport, unit_group,
};
use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_scene::{Change, ChangeKind, ChangeSet, Element, ElementId, Fields, GroupId, SceneRead, SceneStore};

use crate::session::Session;
use crate::state::{BASE_ZOOM, Tool};

/// What the index knew about an element when it last looked.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Seen {
    version: u64,
    transform: [f64; 5],
}

impl Seen {
    fn of(e: &Element) -> Seen {
        let t = &e.transform;
        Seen { version: e.version, transform: [t.x, t.y, t.w, t.h, t.angle] }
    }
}

/// How the tool decides what a pointer drag means to the recognizer.
pub(super) fn pointer_mode(tool: Tool) -> drafftink_canvas::PointerMode {
    use drafftink_canvas::PointerMode;
    match tool {
        Tool::Select | Tool::Text | Tool::Math => PointerMode::Select,
        Tool::Hand => PointerMode::Hand,
        Tool::Rectangle
        | Tool::Diamond
        | Tool::Ellipse
        | Tool::Arrow
        | Tool::Line
        | Tool::Freehand
        | Tool::Highlighter
        | Tool::Frame
        | Tool::Eraser
        | Tool::Laser => PointerMode::Draw,
    }
}

/// Index, viewport and selection frame for one tab.
pub(crate) struct World {
    /// The viewport the recognizer uses. The interaction layer moves its camera directly (class C).
    pub(crate) viewport: Viewport,
    /// Thresholds and handle sizes.
    pub(crate) config: CanvasConfig,
    index: SceneIndex,
    seen: HashMap<ElementId, Seen>,
    tab: Option<crate::state::TabId>,
    store_revision: Option<u64>,
    frame: SelectionFrame,
    frame_key: Option<(u64, Vec<ElementId>)>,
    /// The group whose members are picked one by one.
    entered: Option<GroupId>,
    /// The tool, as the picker needs it.
    tool: Tool,
}

impl World {
    pub(crate) fn new() -> Self {
        Self {
            viewport: Viewport::new(Size::new(1280.0, 720.0), 1.0),
            config: CanvasConfig::default(),
            index: SceneIndex::new(),
            seen: HashMap::new(),
            tab: None,
            store_revision: None,
            frame: SelectionFrame::None,
            frame_key: None,
            entered: None,
            tool: Tool::Select,
        }
    }

    /// Brings the index, frame, entered group and tool up to date with the session. The camera is the
    /// caller's business ([`super::view::ViewSync`]).
    pub(crate) fn sync(&mut self, s: &Session) {
        self.tool = s.tool.effective();
        self.entered = s.tab().observed.editing_group.clone();
        if self.tab != Some(s.tab().id) {
            self.tab = Some(s.tab().id);
            self.store_revision = None;
            self.frame_key = None;
        }
        // Nothing hit-tests during a gesture, and a live frame may rewrite an element many times: wait.
        if !s.gesture_running() {
            self.sync_index(s);
        }
        self.sync_frame(s);
    }

    fn sync_index(&mut self, s: &Session) {
        let store = s.store();
        let revision = store.revision();
        if self.store_revision == Some(revision) {
            return;
        }
        if self.store_revision.is_none() {
            self.index.rebuild(store);
            self.seen = store.iter_all().map(|e| (e.id.clone(), Seen::of(e))).collect();
        } else {
            let mut changes = Vec::new();
            let mut now: HashMap<ElementId, Seen> = HashMap::with_capacity(self.seen.len());
            for e in store.iter_all() {
                let seen = Seen::of(e);
                if self.seen.get(&e.id) != Some(&seen) {
                    changes.push(Change {
                        id: e.id.clone(),
                        kind: ChangeKind::Updated,
                        fields: Fields::all(),
                        version: e.version,
                    });
                }
                now.insert(e.id.clone(), seen);
            }
            for id in self.seen.keys().filter(|id| !now.contains_key(*id)) {
                changes.push(Change { id: id.clone(), kind: ChangeKind::Removed, fields: Fields::all(), version: 0 });
            }
            if !changes.is_empty() {
                changes.sort_by(|a, b| a.id.cmp(&b.id));
                self.index.apply(store, &ChangeSet { revision, elements: changes, meta: false, files: Vec::new() });
            }
            self.seen = now;
        }
        self.store_revision = Some(revision);
    }

    fn sync_frame(&mut self, s: &Session) {
        // A running gesture rewrites the selected elements every frame; the overlay builds its own frame, and
        // nothing picks handles during a drag.
        if s.gesture_running() {
            return;
        }
        let revision = s.store().revision();
        let selection = s.selection();
        let fresh = self.frame_key.as_ref().is_some_and(|(r, ids)| *r == revision && ids.iter().eq(selection.iter()));
        if fresh {
            return;
        }
        let ids: Vec<ElementId> = selection.iter().cloned().collect();
        self.frame = SelectionFrame::of(s.store(), ids.iter());
        self.frame_key = Some((revision, ids));
    }

    /// The frame of the selection (box or points), with its handles.
    pub(crate) fn frame(&self) -> &SelectionFrame {
        &self.frame
    }

    /// The group the user entered.
    pub(crate) fn entered_group(&self) -> Option<&GroupId> {
        self.entered.as_ref()
    }

    /// Reads the index against the store.
    pub(crate) fn index(&self) -> &SceneIndex {
        &self.index
    }

    /// The viewport size and device ratio follow the session; false when nothing changed.
    pub(crate) fn adopt_viewport(&mut self, s: &Session) -> bool {
        let v = &s.view().viewport;
        let a = self.viewport.set_size(Size::new(v.w, v.h));
        let b = self.viewport.set_dpr(v.scale_factor);
        a || b
    }

    /// Adopts the session camera.
    pub(crate) fn adopt_camera(&mut self, s: &Session) -> bool {
        let v = s.view();
        self.viewport.set_camera(Camera { scroll: v.scroll, zoom: v.zoom })
    }

    /// Pixels per world unit.
    pub(crate) fn zoom(&self) -> f64 {
        self.viewport.zoom()
    }

    /// The zoom as the UI displays it (1 is 100%), which binding reach is written for.
    pub(crate) fn displayed_zoom(&self) -> f64 {
        self.viewport.zoom() / BASE_ZOOM
    }

    /// World units for a length in screen pixels.
    pub(crate) fn px(&self, px: f64) -> f64 {
        self.viewport.px_to_world(px)
    }

    /// The topmost element at a world point, `tolerance_px` screen pixels around outlines.
    pub(crate) fn hit(&self, s: &Session, world: Point, tolerance_px: f64) -> Option<Hit> {
        self.index.hit(s.store(), world, self.viewport.zoom(), &HitOptions::with_tolerance(tolerance_px))
    }

    /// Every element at a world point, topmost first.
    pub(crate) fn hit_all(&self, s: &Session, world: Point, tolerance_px: f64) -> Vec<Hit> {
        self.index.hit_all(s.store(), world, self.viewport.zoom(), &HitOptions::with_tolerance(tolerance_px))
    }

    /// Elements a world rectangle selects, back to front, without locked ones.
    pub(crate) fn marquee(&self, s: &Session, rect: &Rect, mode: MarqueeMode) -> Vec<ElementId> {
        self.index.marquee(s.store(), rect, mode, false)
    }

    /// What the recognizer sees under a screen point.
    pub(crate) fn pick(&self, s: &Session, screen: Point, kind: PointerKind) -> Target {
        Pick { world: self, session: s }.pick(screen, kind)
    }

    /// The picker the recognizer runs against, borrowing the world and the session.
    pub(crate) fn picker<'a>(&'a self, s: &'a Session) -> impl Picker + 'a {
        Pick { world: self, session: s }
    }
}

struct Pick<'a> {
    world: &'a World,
    session: &'a Session,
}

impl Picker for Pick<'_> {
    fn viewport(&self) -> &Viewport {
        &self.world.viewport
    }

    fn pick(&self, screen: Point, kind: PointerKind) -> Target {
        let w = self.world;
        let touch = kind == PointerKind::Touch;
        // Handles exist for the select tool, and for text and math, whose handles resize and move the text.
        if matches!(w.tool, Tool::Select | Tool::Text | Tool::Math) {
            let radius =
                if touch { w.config.transformer.hit_radius_touch_px } else { w.config.transformer.hit_radius_px };
            if let Some(h) = w.frame.hit(&w.viewport, screen, radius, &w.config.transformer) {
                return Target::Handle(h);
            }
        }
        let tol = if touch { w.config.hit_tolerance_touch_px } else { w.config.hit_tolerance_px };
        let world = w.viewport.screen_to_world(screen);
        let store = self.session.store();
        match w.index.hit(store, world, w.viewport.zoom(), &HitOptions::with_tolerance(tol)) {
            Some(hit) => {
                let group =
                    store.get(&hit.id).and_then(|e| unit_group(e, w.entered.as_ref(), |g| store.derived().is_group(g)));
                Target::Element { id: hit.id, unit_group: group }
            }
            None => Target::Empty,
        }
    }

    fn cursor(&self, target: &Target) -> Cursor {
        let w = self.world;
        match w.tool {
            Tool::Hand => Cursor::Grab,
            Tool::Select | Tool::Text | Tool::Math => match target {
                Target::Handle(h) => w.frame.cursor_for(h),
                Target::Element { .. } => Cursor::Move,
                Target::Empty | Target::None => Cursor::Default,
            },
            _ => Cursor::Crosshair,
        }
    }
}

/// True for the handles that stretch or turn a box.
pub(crate) fn is_box_handle(h: &Handle) -> bool {
    h.to_box().is_some()
}
