//! The drawing state of the canvas: the render caches, the paint index, and the three Vello scenes of a
//! frame (the document, the drawing preview, the overlay).
//!
//! The document scene is rebuilt only when the document or the camera changed. The overlay scenes are cheap
//! and are rebuilt on their own, so a hover outline or a marquee never re-encodes an element.

use drafftink_engine::interact::InteractionOverlay;
use drafftink_engine::services::SharedTypesetter;
use drafftink_engine::state::TabId;
use drafftink_geom::kurbo::{Affine, Rect, Size, Vec2};
use drafftink_render::{
    FrameReport, GridSpec, GridStyle, Overlay, OverlayStyle, PaintIndex, RenderOptions, Scene, SceneComposer, Theme,
    View, build_paint_index, paint_overlay,
};
use drafftink_scene::{ElementId, SceneRead};

use crate::editor::Arranged;
use vello::peniko::{BlendMode, Color, Compose, Fill, Mix};

/// The document store this state follows: which tab, and how far the scan got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Synced {
    tab: TabId,
    revision: u64,
}

/// Render caches and scenes.
pub(crate) struct CanvasView {
    composer: SceneComposer,
    index: PaintIndex,
    synced: Option<Synced>,
    world: Scene,
    preview: Scene,
    overlay: Scene,
    typesetter: SharedTypesetter,
    theme: Theme,
    grid: GridStyle,
    hidden: Vec<ElementId>,
    last_report: FrameReport,
    /// The font set the cached fragments were encoded with (`Typesetter::font_set_version`).
    fonts: Option<u64>,
}

impl CanvasView {
    pub(crate) fn new(typesetter: SharedTypesetter) -> Self {
        Self {
            composer: SceneComposer::new(),
            index: build_paint_index(&drafftink_scene::MemoryStore::new()),
            synced: None,
            world: Scene::new(),
            preview: Scene::new(),
            overlay: Scene::new(),
            typesetter,
            theme: Theme::Light,
            grid: GridStyle::None,
            hidden: Vec::new(),
            last_report: FrameReport::default(),
            fonts: None,
        }
    }

    /// The report of the last document composition.
    pub(crate) fn report(&self) -> FrameReport {
        self.last_report
    }

    /// Light or dark elements and a grid style. Returns true when either changed (the document scene is stale).
    pub(crate) fn set_look(&mut self, theme: Theme, grid: GridStyle) -> bool {
        let changed = theme != self.theme || grid != self.grid;
        if changed {
            self.theme = theme;
            self.grid = grid;
            self.composer.set_options(RenderOptions { theme, ..RenderOptions::default() });
            self.composer.set_grid(GridSpec::new(grid));
        }
        changed
    }

    /// Drops the cached fragments when the typesetter's font set changed (a font arrived), because glyphs that
    /// were boxes before may be letters now. Returns true then. Does not build the typesetter.
    pub(crate) fn sync_fonts(&mut self) -> bool {
        let Some(now) = self.typesetter.peek(|ts| ts.font_set_version()) else { return false };
        let previous = self.fonts.replace(now);
        let changed = previous.is_some_and(|p| p != now);
        if changed {
            self.composer.cache_mut().clear();
        }
        changed
    }

    /// Elements the composer leaves out (the text being edited, the eraser's soft deletes). Returns true when
    /// the list changed.
    pub(crate) fn set_hidden(&mut self, ids: Vec<ElementId>) -> bool {
        if ids == self.hidden {
            return false;
        }
        self.composer.set_hidden(ids.clone());
        self.hidden = ids;
        true
    }

    /// Forgets which revision the paint index follows, so the next sync rebuilds it (the document was replaced).
    #[cfg(feature = "scenarios")]
    pub(crate) fn forget_index(&mut self) {
        self.synced = None;
    }

    /// Does the paint index already describe this revision of this tab?
    pub(crate) fn index_is_current(&self, tab: TabId, revision: u64) -> bool {
        self.synced == Some(Synced { tab, revision })
    }

    /// Brings the paint index up to date with the store. Cheap when nothing changed: one comparison. After an
    /// edit it re-reads the elements the commits touched (`touched`, from `TabDoc::take_touched`), so a drag
    /// frame costs what it moves and not the size of the document.
    pub(crate) fn sync_index<S: SceneRead>(&mut self, tab: TabId, revision: u64, store: &S, touched: &[ElementId]) {
        let now = Synced { tab, revision };
        if self.synced == Some(now) {
            return;
        }
        let _g = drafftink_profile::span("sync_index");
        if self.synced.is_none_or(|s| s.tab != tab) {
            self.index = build_paint_index(store);
            self.composer.forget_order();
        } else {
            self.composer.follow_touched(&mut self.index, store, touched);
        }
        self.synced = Some(now);
    }

    /// The device-space view for a camera over a canvas of `size` logical pixels.
    pub(crate) fn view_of(scroll: drafftink_geom::kurbo::Point, zoom: f64, size: Size, dpi: f64) -> View {
        let offset = Vec2::new(size.width / 2.0 - scroll.x * zoom, size.height / 2.0 - scroll.y * zoom);
        View::new(zoom, offset, size).with_dpi(dpi)
    }

    /// Rebuilds the document scene.
    pub(crate) fn compose_world<S: SceneRead>(&mut self, store: &S, view: &View) {
        let Self { composer, index, world, typesetter, last_report, .. } = self;
        *last_report = typesetter.with(|ts| composer.compose(store, index, view, ts, world));
    }

    /// The canvas background under the scene, in the current theme.
    pub(crate) fn background<S: SceneRead>(&self, store: &S) -> Color {
        self.composer.background(store)
    }

    /// Rebuilds the preview scene (the element being drawn, the faded elements of an eraser sweep) and the
    /// overlay scene.
    pub(crate) fn compose_overlay<S: SceneRead>(
        &mut self,
        store: &S,
        interaction: &InteractionOverlay,
        render_overlay: &Overlay,
        edited: Option<&Arranged>,
        view: &View,
    ) {
        let Self { composer, preview, overlay, typesetter, .. } = self;
        preview.reset();
        if let Some(edit) = edited {
            typesetter.with(|ts| {
                if let Some(container) = &edit.grown_container {
                    composer.compose_transient(store, std::slice::from_ref(container), view, ts, preview);
                }
                composer.compose_edited_text(&edit.text, edit.reveal, view, ts, preview);
            });
        }
        if let Some(el) = &interaction.preview {
            typesetter.with(|ts| composer.compose_transient(store, std::slice::from_ref(el), view, ts, preview));
        }
        let faded: Vec<_> = interaction.faded.iter().filter_map(|id| store.get(id)).cloned().collect();
        if !faded.is_empty() {
            let mut layer = Scene::new();
            typesetter.with(|ts| composer.compose_transient(store, &faded, view, ts, &mut layer));
            let dev = view.device_size();
            let clip = Rect::new(0.0, 0.0, dev.width, dev.height);
            preview.push_layer(
                Fill::NonZero,
                BlendMode::new(Mix::Normal, Compose::SrcOver),
                0.3,
                Affine::IDENTITY,
                &clip,
            );
            preview.append(&layer, None);
            preview.pop_layer();
        }
        typesetter.with(|ts| paint_overlay(overlay, render_overlay, view, &OverlayStyle::default(), ts));
    }

    /// Writes the three scenes, back to front, into the shell's scene.
    pub(crate) fn write_to(&self, out: &mut Scene) {
        out.append(&self.world, None);
        out.append(&self.preview, None);
        out.append(&self.overlay, None);
    }
}

/// A point of a fading trail, in world units with its age.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrailPoint {
    pub(crate) at: drafftink_geom::kurbo::Point,
    pub(crate) age_ms: f64,
}

/// How long a laser point stays visible, as in the old app.
pub(crate) const LASER_FADE_MS: f64 = 330.0;

/// Draws a fading trail (the laser) over the scene: segments between neighbours, older ones fainter.
pub(crate) fn paint_trail(scene: &mut Scene, points: &[TrailPoint], color: Color, view: &View) {
    use drafftink_geom::kurbo::Stroke;
    let to_dev = view.world_to_device();
    let px = view.logical_to_device().as_coeffs()[0];
    for pair in points.windows(2) {
        let [a, b] = pair else { continue };
        let alpha = (1.0 - b.age_ms / LASER_FADE_MS).clamp(0.0, 1.0) as f32;
        if alpha <= 0.0 {
            continue;
        }
        let line = drafftink_geom::kurbo::Line::new(to_dev * a.at, to_dev * b.at);
        scene.stroke(
            &Stroke::new(4.0 * px).with_caps(drafftink_geom::kurbo::Cap::Round),
            Affine::IDENTITY,
            color.with_alpha(alpha),
            None,
            &line,
        );
    }
}

impl CanvasView {
    /// Adds trails to the overlay scene (the local laser, other people's lasers).
    pub(crate) fn add_trails(&mut self, trails: &[(Color, Vec<TrailPoint>)], view: &View) {
        for (color, points) in trails {
            paint_trail(&mut self.overlay, points, *color, view);
        }
    }
}
