//! The interaction layer's picture of the scene: viewport, spatial index and selection frame.
//!
//! The session owns the document. This module keeps a [`SceneIndex`] of it for hit testing, and the
//! [`Viewport`] the recognizer converts positions with. Both are caches:
//!
//! * The index belongs to the tab's document ([`crate::store::TabDoc::scene_index`]), which keeps it current from
//!   the ids its commits touched: it is built from the whole scene once, and a query re-reads only what changed
//!   since the last one. Commands that need "what is near here" use the same index.
//! * The viewport follows the session camera (`view.set`, `view.fit`, ...) unless the interaction layer moved
//!   it itself and has not reported it yet.

use drafftink_canvas::{
    Camera, CanvasConfig, Cursor, Hit, HitOptions, Picker, PointerKind, SelectionFrame, Target, Viewport, unit_group,
};
use drafftink_geom::OrientedBox;
use drafftink_geom::kurbo::{Point, Size};
use drafftink_scene::{Element, ElementId, GroupId, Kind, SceneRead, SceneStore};

use crate::session::Session;
use crate::state::{BASE_ZOOM, Tool};

/// How the tool decides what a pointer drag means to the recognizer.
pub(super) fn pointer_mode(tool: Tool) -> drafftink_canvas::PointerMode {
    use drafftink_canvas::PointerMode;
    match tool {
        Tool::Select | Tool::Lasso | Tool::Text | Tool::Math => PointerMode::Select,
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

/// What the crop editor lends the picker while it is open.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CropStage {
    /// The image being cropped.
    pub(crate) element: ElementId,
    /// The crop window with its eight handles. It stands in for the selection's frame.
    pub(crate) frame: SelectionFrame,
    /// The whole picture as the draft has it. A drag that starts on it slides it, so it is what the pointer grabs.
    pub(crate) picture: OrientedBox,
}

/// Index, viewport and selection frame for one tab.
pub(crate) struct World {
    /// The viewport the recognizer uses. The interaction layer moves its camera directly (class C).
    pub(crate) viewport: Viewport,
    /// Thresholds and handle sizes.
    pub(crate) config: CanvasConfig,
    tab: Option<crate::state::TabId>,
    frame: SelectionFrame,
    frame_key: Option<(u64, Vec<ElementId>)>,
    /// The group whose members are picked one by one.
    entered: Option<GroupId>,
    /// The tool, as the picker needs it.
    tool: Tool,
    /// While the crop editor is open, its window stands in for the selection's frame and its picture is grabbed.
    crop: Option<CropStage>,
}

impl World {
    pub(crate) fn new() -> Self {
        Self {
            viewport: Viewport::new(Size::new(1280.0, 720.0), 1.0),
            config: CanvasConfig::default(),
            tab: None,
            frame: SelectionFrame::None,
            frame_key: None,
            entered: None,
            tool: Tool::Select,
            crop: None,
        }
    }

    /// Brings the frame, entered group and tool up to date with the session. The camera is the
    /// caller's business ([`super::view::ViewSync`]).
    pub(crate) fn sync(&mut self, s: &Session) {
        self.tool = s.tool.effective();
        self.entered = s.tab().observed.editing_group.clone();
        if self.tab != Some(s.tab().id) {
            self.tab = Some(s.tab().id);
            self.frame_key = None;
        }
        self.sync_frame(s);
    }

    /// Opens the crop editor's stage (`Some`): its window takes the place of the selection's frame. `None` takes it
    /// away.
    pub(crate) fn set_crop(&mut self, stage: Option<CropStage>) {
        if self.crop != stage {
            // The cached selection frame is stale after either change.
            self.frame_key = None;
            self.crop = stage;
        }
    }

    fn sync_frame(&mut self, s: &Session) {
        if let Some(stage) = &self.crop {
            self.frame = stage.frame.clone();
            return;
        }
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

    /// Ids of the indexed elements whose bounds are within `reach` world units of `p`, in no particular order.
    /// The exact rules run on these only.
    pub(crate) fn near(&self, s: &Session, p: Point, reach: f64) -> Vec<ElementId> {
        s.tab().doc.near(p, reach)
    }

    /// Every element at a world point, topmost first.
    pub(crate) fn hit_all(&self, s: &Session, world: Point, tolerance_px: f64) -> Vec<Hit> {
        s.tab().doc.scene_index().hit_all(
            s.store(),
            world,
            self.viewport.zoom(),
            &HitOptions::with_tolerance(tolerance_px),
        )
    }

    /// The topmost element under a world point when it is locked (locked elements are skipped by the normal hit
    /// test, so this is what a right click needs to offer Unlock). A label counts as its container. `None` when
    /// the topmost element is unlocked or nothing is there.
    pub(crate) fn locked_on_top(&self, s: &Session, world: Point, tolerance_px: f64) -> Option<ElementId> {
        let opts = HitOptions { include_locked: true, ..HitOptions::with_tolerance(tolerance_px) };
        let top = s.tab().doc.scene_index().hit(s.store(), world, self.viewport.zoom(), &opts)?;
        let e = s.store().get(&top.id)?;
        let owner = e.container_id().and_then(|c| s.store().get(c)).filter(|c| !c.deleted).unwrap_or(e);
        owner.locked.then(|| owner.id.clone())
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
        // The crop editor draws its handles large for a pen as well as a finger, so a pen grabs them from as far.
        let coarse = touch || (w.crop.is_some() && kind == PointerKind::Pen);
        // Handles exist for the select tool, and for text and math, whose handles resize and move the text.
        if matches!(w.tool, Tool::Select | Tool::Lasso | Tool::Text | Tool::Math) {
            let radius =
                if coarse { w.config.transformer.hit_radius_touch_px } else { w.config.transformer.hit_radius_px };
            if let Some(h) = w.frame.hit(&w.viewport, screen, radius, &w.config.transformer) {
                return Target::Handle(h);
            }
        }
        let tol = if touch { w.config.hit_tolerance_touch_px } else { w.config.hit_tolerance_px };
        let world = w.viewport.screen_to_world(screen);
        let store = self.session.store();
        // The icon of a link (select tool only: the other tools have no use for a click on it) lies outside its
        // element and wins over what is under it. A selected element shows its handles there instead.
        if matches!(w.tool, Tool::Select | Tool::Lasso) {
            let selection = self.session.selection();
            let icon = self
                .session
                .tab()
                .doc
                .scene_index()
                .link_icon_at(store, world, w.viewport.zoom(), |e| selection.contains(&e.id));
            if let Some(id) = icon {
                return Target::LinkIcon { id };
            }
        }
        match self.session.tab().doc.scene_index().hit(
            store,
            world,
            w.viewport.zoom(),
            &HitOptions::with_tolerance(tol),
        ) {
            Some(hit) => {
                let group =
                    store.get(&hit.id).and_then(|e| unit_group(e, w.entered.as_ref(), |g| store.derived().is_group(g)));
                Target::Element { id: hit.id, unit_group: group }
            }
            // While the crop editor is open the dimmed part of the picture is the picture too: a drag there slides it.
            None => match &w.crop {
                Some(stage) if stage.picture.contains(world) => {
                    Target::Element { id: stage.element.clone(), unit_group: None }
                }
                _ => Target::Empty,
            },
        }
    }

    fn cursor(&self, target: &Target) -> Cursor {
        let w = self.world;
        if let Some(stage) = &w.crop {
            // The crop editor owns the pointer: the handles resize the window, the picture is slid, nothing else
            // answers.
            return match target {
                Target::Handle(h) => w.frame.cursor_for(h),
                Target::Element { id, .. } if *id == stage.element => Cursor::Grab,
                _ => Cursor::Default,
            };
        }
        match w.tool {
            Tool::Hand => Cursor::Grab,
            Tool::Select | Tool::Lasso | Tool::Text | Tool::Math => match target {
                Target::Handle(h) => w.frame.cursor_for(h),
                Target::Element { id, .. } if self.invites_typing(id) => Cursor::Text,
                Target::Element { .. } => Cursor::Move,
                Target::Empty | Target::None if w.tool == Tool::Lasso => Cursor::Crosshair,
                // The text tool places a new text on empty canvas.
                Target::Empty | Target::None if w.tool == Tool::Text => Cursor::Text,
                Target::LinkIcon { .. } => Cursor::Pointer,
                Target::Empty | Target::None => Cursor::Default,
            },
            _ => Cursor::Crosshair,
        }
    }
}

impl Pick<'_> {
    /// Whether a click on this element starts a text edit. Only the text tool does: a text, or a shape or an arrow
    /// that gets a label. The select tool moves a text like any other element, so its cursor is the move cursor.
    fn invites_typing(&self, id: &ElementId) -> bool {
        let Some(kind) = self.session.element(id).map(Element::tag) else { return false };
        self.world.tool == Tool::Text
            && matches!(kind, Kind::Text | Kind::Rectangle | Kind::Diamond | Kind::Ellipse | Kind::Arrow)
    }
}
