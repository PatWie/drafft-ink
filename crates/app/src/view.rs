//! The drawing state of the canvas: the render caches, the paint index, and the three Vello scenes of a
//! frame (the document, the drawing preview, the overlay).
//!
//! The document scene is rebuilt only when the document or the camera changed. The overlay scenes are cheap
//! and are rebuilt on their own, so a hover outline or a marquee never re-encodes an element.

use drafftink_engine::interact::InteractionOverlay;
use drafftink_engine::services::SharedTypesetter;
use drafftink_engine::state::TabId;
use drafftink_geom::kurbo::{Affine, Rect, Size, Vec2};
use drafftink_gpu::BackendKind;
use drafftink_render::{
    FrameReport, GridSpec, GridStyle, LodConfig, Overlay, OverlayStyle, PaintIndex, RenderOptions, Scene,
    SceneComposer, Theme, View, build_paint_index, paint_crop_dim, paint_overlay,
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
    lod: LodConfig,
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
            lod: LodConfig::default(),
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
            self.composer.set_options(RenderOptions { theme, lod: self.lod });
            self.composer.set_grid(GridSpec::new(grid));
        }
        changed
    }

    /// Tells the view which renderer draws it. The compatibility renderer gets a coarser level of detail.
    pub(crate) fn set_backend(&mut self, backend: BackendKind) {
        self.lod = if backend.is_compat() { LodConfig::COMPAT } else { LodConfig::default() };
        self.composer.set_options(RenderOptions { theme: self.theme, lod: self.lod });
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
        let Self { composer, preview, overlay, typesetter, theme, .. } = self;
        preview.reset();
        if let Some(edit) = edited {
            typesetter.with(|ts| {
                if let Some(container) = &edit.grown_container {
                    composer.compose_transient(store, std::slice::from_ref(container), view, ts, preview);
                    composer.compose_transient(store, &edit.following_arrows, view, ts, preview);
                }
                composer.compose_edited_text(&edit.text, edit.reveal, view, ts, preview);
            });
        }
        // The crop editor draws the whole picture dimmed under a dark scrim, and the draft of the crop over it.
        if let Some(crop) = &interaction.crop {
            let mut whole = Scene::new();
            typesetter
                .with(|ts| composer.compose_transient(store, std::slice::from_ref(&crop.whole), view, ts, &mut whole));
            paint_crop_dim(preview, view, *theme, &whole, &crop.picture, &crop.window);
            typesetter
                .with(|ts| composer.compose_transient(store, std::slice::from_ref(&crop.shown), view, ts, preview));
        }
        if let Some(el) = &interaction.preview {
            typesetter.with(|ts| composer.compose_transient(store, std::slice::from_ref(el), view, ts, preview));
        }
        let faded: Vec<_> = interaction.faded.iter().filter_map(|id| store.get(id)).cloned().collect();
        if !faded.is_empty() {
            Self::compose_faded(composer, typesetter, store, &faded, view, preview);
        }
        typesetter.with(|ts| paint_overlay(overlay, render_overlay, view, &OverlayStyle::for_theme(*theme), ts));
    }

    /// Draws `elements` at 30% opacity on top of `scene` (the eraser's soft delete).
    fn compose_faded<S: SceneRead>(
        composer: &mut SceneComposer,
        typesetter: &SharedTypesetter,
        store: &S,
        elements: &[drafftink_scene::Element],
        view: &View,
        scene: &mut Scene,
    ) {
        let mut layer = Scene::new();
        typesetter.with(|ts| composer.compose_transient(store, elements, view, ts, &mut layer));
        let dev = view.device_size();
        let clip = Rect::new(0.0, 0.0, dev.width, dev.height);
        scene.push_layer(Fill::NonZero, BlendMode::new(Mix::Normal, Compose::SrcOver), 0.3, Affine::IDENTITY, &clip);
        scene.append(&layer, None);
        scene.pop_layer();
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

/// Draws the head of the local laser at `at`: a glow, a red disc and a white centre, in logical pixels as in the old
/// app (12, 6 and 2).
pub(crate) fn paint_laser_head(scene: &mut Scene, at: drafftink_geom::kurbo::Point, view: &View) {
    use drafftink_geom::kurbo::Circle;
    let centre = view.world_to_device() * at;
    let px = view.logical_to_device().as_coeffs()[0];
    for (radius, color) in
        [(12.0, Color::from_rgba8(255, 0, 0, 100)), (6.0, Color::from_rgba8(255, 50, 50, 255)), (2.0, Color::WHITE)]
    {
        scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &Circle::new(centre, radius * px));
    }
}

impl CanvasView {
    /// Adds the head of the local laser to the overlay scene.
    pub(crate) fn add_laser_head(&mut self, at: drafftink_geom::kurbo::Point, view: &View) {
        paint_laser_head(&mut self.overlay, at, view);
    }

    /// Adds trails to the overlay scene (the local laser, other people's lasers).
    pub(crate) fn add_trails(&mut self, trails: &[(Color, Vec<TrailPoint>)], view: &View) {
        for (color, points) in trails {
            paint_trail(&mut self.overlay, points, *color, view);
        }
    }
}

#[cfg(test)]
mod tests {
    use drafftink_geom::kurbo::{Point, Size, Vec2};
    use drafftink_render::View;
    use vello::Scene;

    use super::paint_laser_head;

    #[test]
    fn the_laser_head_is_a_glow_a_disc_and_a_centre_at_the_pointer() {
        let view = View::new(2.0, Vec2::new(0.0, 0.0), Size::new(400.0, 300.0));
        let mut scene = Scene::new();
        paint_laser_head(&mut scene, Point::new(50.0, 50.0), &view);
        assert_eq!(scene.encoding().n_paths, 3);
        let mut other = Scene::new();
        paint_laser_head(
            &mut other,
            Point::new(50.0, 50.0),
            &View::new(0.1, Vec2::new(0.0, 0.0), Size::new(400.0, 300.0)),
        );
        assert_eq!(
            other.encoding().path_data.len(),
            scene.encoding().path_data.len(),
            "the head keeps its screen size at every zoom, so it costs the same"
        );
    }
}
