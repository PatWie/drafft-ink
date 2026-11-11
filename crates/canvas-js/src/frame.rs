//! The CPU half of a frame: the store's content as a Vello scene.
//!
//! [`Frame`] keeps what the render crate needs between frames (the paint index, the fragment cache, the scenes of
//! the world and of the overlay) and rebuilds only the part that changed. It never touches the GPU, so the tests
//! run everywhere; [`crate::target`] draws the scene it builds.

use std::cell::RefCell;
use std::rc::Rc;

use drafftink_canvas::{Canvas, Dirty, Viewport};
use drafftink_geom::kurbo::{Rect, Size, Vec2};
use drafftink_render::{
    FrameReport, GridSpec, OverlayStyle, PaintIndex, RenderOptions, Scene, SceneComposer, Theme, View,
    build_paint_index, combine, paint_overlay, update_paint_index,
};
use drafftink_scene::{ChangeSet, MemoryStore, SceneRead, SceneStore};

use crate::fonts::Fonts;
use crate::overlay;

/// A change set touching more entries than this rebuilds the paint index in bulk instead of one by one.
const BULK_REBUILD: usize = 2048;

/// The render crate's view for a canvas viewport.
pub(crate) fn view_of(vp: &Viewport) -> View {
    let size = vp.size();
    let zoom = vp.zoom();
    let scroll = vp.scroll();
    let offset = Vec2::new(size.width / 2.0 - scroll.x * zoom, size.height / 2.0 - scroll.y * zoom);
    View::new(zoom, offset, Size::new(size.width, size.height)).with_dpi(vp.dpr())
}

/// The scenes of one stage.
pub(crate) struct Frame {
    composer: SceneComposer,
    paint_index: PaintIndex,
    changes: Rc<RefCell<Vec<ChangeSet>>>,
    world: Scene,
    overlay: Scene,
    out: Scene,
    style: OverlayStyle,
    theme: Theme,
    report: FrameReport,
    /// Pictures were still waiting to be decoded in the last pass; another pass finishes them.
    more_work: bool,
}

impl Frame {
    /// A frame builder that follows `store`.
    pub(crate) fn new(store: &mut MemoryStore, theme: Theme, grid: GridSpec) -> Self {
        let changes: Rc<RefCell<Vec<ChangeSet>>> = Rc::default();
        let sink = Rc::clone(&changes);
        store.subscribe(Box::new(move |cs| sink.borrow_mut().push(cs.clone())));
        let mut composer = SceneComposer::new();
        composer.set_options(RenderOptions { theme, ..RenderOptions::default() });
        composer.set_grid(grid);
        Self {
            composer,
            paint_index: build_paint_index(store),
            changes,
            world: Scene::new(),
            overlay: Scene::new(),
            out: Scene::new(),
            style: OverlayStyle::default(),
            theme,
            report: FrameReport::default(),
            more_work: false,
        }
    }

    /// Whether another pass is needed without any new input (pictures still decoding).
    pub(crate) fn wants_frame(&self) -> bool {
        self.more_work
    }

    /// What the last pass did.
    pub(crate) fn report(&self) -> FrameReport {
        self.report
    }

    /// The theme in force.
    pub(crate) fn theme(&self) -> Theme {
        self.theme
    }

    /// Light or dark.
    pub(crate) fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.composer.set_options(RenderOptions { theme, ..*self.composer.options() });
        self.more_work = true;
    }

    /// The background grid.
    pub(crate) fn set_grid(&mut self, grid: GridSpec) {
        self.composer.set_grid(grid);
        self.more_work = true;
    }

    /// Brings the paint index and the fragment cache up to date with the commits since the last call.
    fn sync(&mut self, store: &MemoryStore) {
        let batches = std::mem::take(&mut *self.changes.borrow_mut());
        let touched: usize = batches.iter().map(|b| b.elements.len()).sum();
        if touched > BULK_REBUILD && touched > store.len() / 2 {
            self.paint_index = build_paint_index(store);
            batches.iter().for_each(|b| self.composer.apply_changes(b));
            return;
        }
        for cs in &batches {
            update_paint_index(&mut self.paint_index, store, cs);
            self.composer.apply_changes(cs);
        }
    }

    /// The scene to draw: the grid and the visible elements, then the overlay. Only the parts `dirty` names are
    /// built again. Returns the scene and the canvas colour.
    pub(crate) fn compose(
        &mut self,
        canvas: &Canvas<MemoryStore>,
        fonts: &mut Fonts,
        dirty: Dirty,
    ) -> (&Scene, peniko::Color) {
        let store = canvas.store();
        self.sync(store);
        let view = view_of(canvas.viewport());
        if dirty.scene || dirty.camera || self.more_work {
            self.report = self.composer.compose(store, &self.paint_index, &view, fonts.typesetter(), &mut self.world);
            self.more_work = self.report.images_pending > 0;
        }
        if dirty.any() || self.more_work {
            let described = overlay::to_render(&canvas.overlay());
            paint_overlay(&mut self.overlay, &described, &view, &self.style);
        }
        combine(&self.world, &self.overlay, &mut self.out);
        (&self.out, self.composer.background(store))
    }

    /// The bounds of everything the paint index holds, or of `ids` only.
    pub(crate) fn content_bounds(
        &self,
        store: &MemoryStore,
        ids: Option<&[drafftink_scene::ElementId]>,
    ) -> Option<Rect> {
        let bounds = |e: &drafftink_scene::Element| drafftink_render::paint_bounds(e);
        match ids {
            None => drafftink_geom::aabb::union_all(store.iter().map(bounds)),
            Some(ids) => drafftink_geom::aabb::union_all(
                ids.iter().filter_map(|id| store.get(id)).filter(|e| !e.deleted).map(bounds),
            ),
        }
    }
}
