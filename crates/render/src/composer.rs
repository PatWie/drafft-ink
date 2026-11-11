//! [`SceneComposer`]: a frame from cached fragments.
//!
//! Per frame the composer asks the spatial index which elements are in view, puts them in z order, and for each
//! one either appends its cached fragment with the view transform or, when it is too small to matter, draws a
//! cheap stand-in. Nothing is measured or laid out here. The cost of a frame follows the number of visible
//! elements, not the size of the document.

use std::collections::HashMap;
use std::sync::Arc;

use drafftink_geom::{SpatialIndex, aabb};
use drafftink_profile::{Counter, Gauge, count, inc, set_gauge};
use drafftink_scene::{ChangeKind, ChangeSet, Element, ElementId, ElementKind, Fields, FileId, SceneRead};
use drafftink_typeset::Typesetter;
use kurbo::{Affine, Rect};
use peniko::Fill;
use vello::Scene;

use crate::cache::{FragmentCache, Outcome};
use crate::grid::{GridSpec, GridStats, paint_grid};
use crate::options::{LodConfig, RenderOptions};
use crate::painter::ElementPainter;
use crate::spec::{element_to_world, world_aabb};
use crate::theme::{Theme, scale_alpha};
use crate::view::View;

/// What one call to [`SceneComposer::compose`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameReport {
    /// Live elements in the store.
    pub total: usize,
    /// Elements whose paint bounds touch the view.
    pub in_view: usize,
    /// Elements not drawn because they are outside the view or outside their clipping frame.
    pub culled: usize,
    /// Elements drawn from a fragment.
    pub drawn: usize,
    /// Elements drawn as a flat box.
    pub lod_boxes: usize,
    /// Texts drawn as grey bars.
    pub lod_greeked: usize,
    /// Elements too small to draw.
    pub lod_skipped: usize,
    /// Cheap stand-ins dropped because the frame hit `max_cheap_per_frame`.
    pub lod_dropped: usize,
    /// Fragments served with the same version.
    pub cache_hits: usize,
    /// Fragments kept across a move, a rotation or a change that does not draw.
    pub cache_reuses: usize,
    /// Elements encoded this frame.
    pub cache_misses: usize,
    /// Clip layers pushed for frames.
    pub clips: usize,
    /// Pictures still waiting to be decoded.
    pub images_pending: usize,
    /// The grid that was drawn.
    pub grid: GridStats,
}

/// Where a visible element is, with what it needs.
struct Candidate<'a> {
    el: &'a Element,
    bounds: Rect,
}

/// A frame element's clip, worked out once per compose call.
#[derive(Clone, Copy)]
struct FrameClip {
    to_world: Affine,
    local: Rect,
    world: Rect,
}

/// Every indexed element in z order with its paint bounds, built at one version of the store. A frame that shows
/// most of a large scene walks this list instead of the store and the index (two hash lookups per element).
struct ZOrder {
    /// `SceneRead::version_stamp` of the store and `SpatialIndex::generation` of the index it was built from.
    stamp: (u64, u64),
    slots: Vec<(ElementId, Rect)>,
}

/// Turns a store, a spatial index and a view into a Vello scene.
pub struct SceneComposer {
    painter: ElementPainter,
    cache: FragmentCache,
    options: RenderOptions,
    grid: GridSpec,
    hidden: Vec<ElementId>,
    trace: Option<Vec<ElementId>>,
    /// The z order of the whole scene, kept between frames while the store does not change (a pan or a zoom).
    order: Option<ZOrder>,
}

impl Default for SceneComposer {
    fn default() -> Self {
        Self::new()
    }
}

/// Elements this many times fewer than the store switch the composer to sorting the hits instead of walking the
/// whole store in order.
const SORT_FRACTION: usize = 4;

impl SceneComposer {
    /// A composer with default options and cache sizes.
    pub fn new() -> Self {
        Self {
            painter: ElementPainter::new(),
            cache: FragmentCache::default(),
            options: RenderOptions::default(),
            grid: GridSpec::default(),
            hidden: Vec::new(),
            trace: None,
            order: None,
        }
    }

    /// A composer with a given fragment cache budget in bytes.
    pub fn with_cache_bytes(max_bytes: usize) -> Self {
        let mut c = Self::new();
        c.cache = FragmentCache::new(max_bytes);
        c
    }

    /// The options in force.
    pub fn options(&self) -> &RenderOptions {
        &self.options
    }

    /// Changes the options. A new theme encodes elements again (each theme keeps its own fragments).
    pub fn set_options(&mut self, options: RenderOptions) {
        self.options = options;
    }

    /// The grid drawn under the elements.
    pub fn set_grid(&mut self, grid: GridSpec) {
        self.grid = grid;
    }

    /// Elements that are not drawn, for example the text being edited (the editor draws it).
    pub fn set_hidden(&mut self, ids: Vec<ElementId>) {
        self.hidden = ids;
    }

    /// Starts or stops recording the ids drawn by each [`Self::compose`], in draw order (for tests and tools).
    pub fn set_trace(&mut self, on: bool) {
        self.trace = on.then(Vec::new);
    }

    /// The ids drawn by the last [`Self::compose`], back to front, when tracing is on. Elements drawn as boxes
    /// or bars are included.
    pub fn drawn_order(&self) -> &[ElementId] {
        self.trace.as_deref().unwrap_or(&[])
    }

    /// The fragment cache.
    pub fn cache(&self) -> &FragmentCache {
        &self.cache
    }

    /// The fragment cache, to change its budget or clear it.
    pub fn cache_mut(&mut self) -> &mut FragmentCache {
        &mut self.cache
    }

    /// The painter (sketch and picture caches).
    pub fn painter(&self) -> &ElementPainter {
        &self.painter
    }

    /// The painter, to reach the picture cache from a decoding task.
    pub fn painter_mut(&mut self) -> &mut ElementPainter {
        &mut self.painter
    }

    /// Drops fragments of elements a commit removed or hid. Call it with each change set; skipping it only costs
    /// memory until the cache budget evicts them.
    pub fn apply_changes(&mut self, changes: &ChangeSet) {
        for c in &changes.elements {
            let gone =
                c.kind == ChangeKind::Removed || (c.kind == ChangeKind::Updated && c.fields.contains(Fields::DELETED));
            if gone {
                self.cache.remove(&c.id);
            }
        }
    }

    /// The canvas background for the store's document, in the current theme.
    pub fn background<S: SceneRead>(&self, store: &S) -> peniko::Color {
        peniko::Color::from(self.options.theme.map(store.meta().background))
    }

    /// Builds the world scene of one frame into `scene` (reset first): the grid, then the visible elements in
    /// z order.
    ///
    /// `index` must hold [`crate::paint_bounds`] of the live elements of `store` (see [`crate::build_paint_index`]).
    ///
    /// A frame that shows most of the scene needs the z order of all of it. The composer keeps that order between
    /// frames, tied to the store's [`SceneRead::version_stamp`] and the index's generation, so a pan or a zoom of
    /// a large scene does not rebuild it. Use one composer with one store; for another store, make another
    /// composer.
    pub fn compose<S: SceneRead>(
        &mut self,
        store: &S,
        index: &SpatialIndex<ElementId>,
        view: &View,
        typesetter: &mut Typesetter,
        scene: &mut Scene,
    ) -> FrameReport {
        let _span = drafftink_profile::span("compose");
        scene.reset();
        inc(Counter::SceneBuilds);
        self.painter.images_mut().begin_frame();
        if let Some(t) = &mut self.trace {
            t.clear();
        }
        let total = store.len();
        let mut rep = FrameReport { total, ..FrameReport::default() };
        set_gauge(Gauge::ElementsTotal, total as u64);
        if !view.is_valid() {
            return rep;
        }
        let theme = self.options.theme;
        let lod = self.options.lod;
        rep.grid = paint_grid(scene, view, &self.grid, theme);

        let to_dev = view.world_to_device();
        let scale = view.scale();
        let world = view.world_rect();

        // 1. Who is in view, in z order.
        let candidates = collect(store, index, &world, total, &self.hidden, &mut self.order);
        rep.in_view = candidates.len();
        rep.culled = total.saturating_sub(candidates.len());

        // 2. Draw.
        let files = |id: &FileId| store.file(id).map(|f| Arc::clone(&f.bytes));
        let mut clips = FrameClips::default();
        let mut cheap = 0usize;
        let SceneComposer { painter, cache, options, trace, .. } = self;
        for c in &candidates {
            let el = c.el;

            // Frame clipping: cull what lies outside the frame, clip what crosses its edge.
            let verdict = clips.verdict(store, el, &c.bounds);
            if matches!(verdict, FrameVerdict::Outside) {
                rep.culled += 1;
                continue;
            }
            clips.close_if_left(scene, el);

            match level_of_detail(&lod, el, &c.bounds, scale) {
                Detail::Skip => rep.lod_skipped += 1,
                Detail::Cheap(_) if cheap >= lod.max_cheap_per_frame => rep.lod_dropped += 1,
                Detail::Cheap(kind) => {
                    cheap += 1;
                    if let Some(t) = trace {
                        t.push(el.id.clone());
                    }
                    draw_cheap(scene, kind, to_dev * element_to_world(el), el, scale, options.theme, &mut rep);
                }
                Detail::Full => {
                    if clips.open_for(scene, to_dev, verdict) {
                        rep.clips += 1;
                    }
                    // A placeholder whose picture has since arrived is encoded again.
                    let stale = cache
                        .awaiting(&el.id, theme)
                        .is_some_and(|fid| painter.images().changed_since_placeholder(fid, || files(fid).is_some()));
                    let (fragment, outcome) =
                        cache.get_or_paint(el, theme, stale, || painter.paint(el, options, typesetter, &files));
                    match outcome {
                        Outcome::Hit => rep.cache_hits += 1,
                        Outcome::Reuse => rep.cache_reuses += 1,
                        Outcome::Miss => rep.cache_misses += 1,
                    }
                    if fragment.awaiting_image.is_some() {
                        rep.images_pending += 1;
                    }
                    if !fragment.is_empty() {
                        scene.append(&fragment.scene, Some(to_dev * element_to_world(el)));
                    }
                    rep.drawn += 1;
                    if let Some(t) = trace {
                        t.push(el.id.clone());
                    }
                }
            }
        }
        clips.close(scene);

        count(Counter::ElementsEncoded, (rep.drawn + rep.lod_boxes + rep.lod_greeked) as u64);
        count(Counter::ElementsCulled, rep.culled as u64);
        set_gauge(Gauge::ElementsVisible, rep.in_view as u64);
        set_gauge(Gauge::CacheEntries, self.cache.len() as u64);
        rep
    }

    /// Appends elements that are not in the store (a shape being drawn) to `scene`, encoded without the cache,
    /// each on top of what is there. Call it after [`Self::compose`].
    pub fn compose_transient<S: SceneRead>(
        &mut self,
        store: &S,
        elements: &[Element],
        view: &View,
        typesetter: &mut Typesetter,
        scene: &mut Scene,
    ) {
        let files = |id: &FileId| store.file(id).map(|f| Arc::clone(&f.bytes));
        let to_dev = view.world_to_device();
        for el in elements {
            let fragment = self.painter.paint(el, &self.options, typesetter, &files);
            if !fragment.is_empty() {
                scene.append(&fragment.scene, Some(to_dev * element_to_world(el)));
            }
        }
    }
}

/// A cheap stand-in for an element too small to draw properly.
#[derive(Clone, Copy)]
enum Cheap {
    /// A flat box.
    Box,
    /// Grey bars for text.
    Greek,
}

/// How much work an element gets at this zoom.
enum Detail {
    /// Too small to see.
    Skip,
    /// A stand-in.
    Cheap(Cheap),
    /// The cached fragment.
    Full,
}

fn level_of_detail(lod: &LodConfig, el: &Element, bounds: &Rect, scale: f64) -> Detail {
    let px = bounds.width().max(bounds.height()) * scale;
    if px < lod.skip_below_px {
        Detail::Skip
    } else if px < lod.box_below_px {
        Detail::Cheap(Cheap::Box)
    } else if let ElementKind::Text(t) = &el.kind
        && t.font.size * scale < lod.greek_text_below_px
    {
        Detail::Cheap(Cheap::Greek)
    } else {
        Detail::Full
    }
}

fn draw_cheap(
    scene: &mut Scene,
    kind: Cheap,
    base: Affine,
    el: &Element,
    scale: f64,
    theme: Theme,
    rep: &mut FrameReport,
) {
    match kind {
        Cheap::Box => {
            draw_box(scene, base, el, scale, theme);
            rep.lod_boxes += 1;
        }
        Cheap::Greek => {
            draw_greek(scene, base, el, theme);
            rep.lod_greeked += 1;
        }
    }
}

/// What a frame says about an element's visibility.
enum FrameVerdict {
    /// The element has a clipping frame and lies entirely outside it.
    Outside,
    /// Nothing to clip: no frame, no clip, or the element is inside it.
    Whole,
    /// The element crosses the edge of its frame, which clips it.
    Crossing(ElementId, FrameClip),
}

/// The clip layers of frames during one compose call: one frame's clip is open at a time, and the clips of frames
/// already seen are remembered.
#[derive(Default)]
struct FrameClips {
    known: HashMap<ElementId, Option<FrameClip>>,
    open: Option<ElementId>,
}

impl FrameClips {
    fn verdict<S: SceneRead>(&mut self, store: &S, el: &Element, bounds: &Rect) -> FrameVerdict {
        let Some(fid) = &el.frame_id else { return FrameVerdict::Whole };
        let Some(clip) = *self.known.entry(fid.clone()).or_insert_with(|| frame_clip(store, fid)) else {
            return FrameVerdict::Whole;
        };
        if !aabb::intersects(&clip.world, bounds) {
            FrameVerdict::Outside
        } else if aabb::contains_rect(&clip.world, bounds) {
            FrameVerdict::Whole
        } else {
            FrameVerdict::Crossing(fid.clone(), clip)
        }
    }

    /// Ends the open clip when `el` is not a child of its frame.
    fn close_if_left(&mut self, scene: &mut Scene, el: &Element) {
        if let Some(open) = &self.open
            && el.frame_id.as_ref() != Some(open)
        {
            self.close(scene);
        }
    }

    /// Opens the clip a crossing element needs, unless one is open. True when a layer was pushed.
    fn open_for(&mut self, scene: &mut Scene, to_dev: Affine, verdict: FrameVerdict) -> bool {
        match verdict {
            FrameVerdict::Crossing(fid, clip) if self.open.is_none() => {
                scene.push_clip_layer(Fill::NonZero, to_dev * clip.to_world, &clip.local);
                self.open = Some(fid);
                true
            }
            _ => false,
        }
    }

    fn close(&mut self, scene: &mut Scene) {
        if self.open.take().is_some() {
            scene.pop_layer();
        }
    }
}

fn frame_clip<S: SceneRead>(store: &S, id: &ElementId) -> Option<FrameClip> {
    let f = store.get(id)?;
    let ElementKind::Frame(data) = &f.kind else { return None };
    if f.deleted || !data.clip {
        return None;
    }
    let to_world = element_to_world(f);
    let local = Rect::new(0.0, 0.0, f.transform.w, f.transform.h);
    Some(FrameClip { to_world, local, world: world_aabb(to_world, local) })
}

/// The elements in view, back to front.
fn collect<'a, S: SceneRead>(
    store: &'a S,
    index: &SpatialIndex<ElementId>,
    world: &Rect,
    total: usize,
    hidden: &[ElementId],
    order: &mut Option<ZOrder>,
) -> Vec<Candidate<'a>> {
    let visible = |e: &Element| !e.deleted && !hidden.contains(&e.id);
    let everything = index.total_bounds().is_some_and(|b| aabb::contains_rect(world, &b));
    if everything {
        return ordered(store, index, world, visible, order);
    }
    inc(Counter::SpatialQueries);
    let hits: Vec<(&ElementId, Rect)> = index.query_rect(world).collect();
    if total > 256 && hits.len() * SORT_FRACTION > total {
        return ordered(store, index, world, visible, order);
    }
    let mut out: Vec<Candidate<'a>> = Vec::with_capacity(hits.len());
    for (id, bounds) in hits {
        if let Some(el) = store.get(id)
            && visible(el)
        {
            out.push(Candidate { el, bounds });
        }
    }
    out.sort_unstable_by(|a, b| (&a.el.index, &a.el.id).cmp(&(&b.el.index, &b.el.id)));
    out
}

/// The elements in view, back to front, from the z order of the whole scene. When the store can say which version
/// it is at ([`SceneRead::version_stamp`]) the order is kept between frames and rebuilt only when the version
/// changes; otherwise every frame walks the store.
fn ordered<'a, S: SceneRead>(
    store: &'a S,
    index: &SpatialIndex<ElementId>,
    world: &Rect,
    visible: impl Fn(&Element) -> bool,
    order: &mut Option<ZOrder>,
) -> Vec<Candidate<'a>> {
    let Some(stamp) = store.version_stamp() else {
        *order = None;
        return walk(store, index, world, visible);
    };
    let stamp = (stamp, index.generation());
    if order.as_ref().is_none_or(|o| o.stamp != stamp) {
        count(Counter::ElementsScanned, u64::try_from(store.len_all()).unwrap_or(u64::MAX));
        let slots = store.iter().filter_map(|el| Some((el.id.clone(), index.get(&el.id)?))).collect();
        *order = Some(ZOrder { stamp, slots });
    }
    let Some(order) = order.as_ref() else { return Vec::new() };
    order
        .slots
        .iter()
        .filter(|(_, bounds)| aabb::intersects(world, bounds))
        .filter_map(|(id, bounds)| {
            let el = store.get(id)?;
            visible(el).then_some(Candidate { el, bounds: *bounds })
        })
        .collect()
}

/// Walks the whole store in z order and keeps what the index says is in view.
fn walk<'a, S: SceneRead>(
    store: &'a S,
    index: &SpatialIndex<ElementId>,
    world: &Rect,
    visible: impl Fn(&Element) -> bool,
) -> Vec<Candidate<'a>> {
    count(Counter::ElementsScanned, u64::try_from(store.len_all()).unwrap_or(u64::MAX));
    store
        .iter()
        .filter_map(|el| {
            let bounds = index.get(&el.id)?;
            (aabb::intersects(world, &bounds) && visible(el)).then_some(Candidate { el, bounds })
        })
        .collect()
}

/// A flat box the size of the element's box, at least one device pixel across.
fn draw_box(scene: &mut Scene, base: Affine, el: &Element, scale: f64, theme: crate::theme::Theme) {
    let t = &el.transform;
    let min = 1.0 / scale.max(1e-9);
    let (w, h) = (t.w.max(min), t.h.max(min));
    let color = el.style.fill_color.filter(|c| c.a > 0).unwrap_or(el.style.stroke_color);
    let mapped = theme.map(color);
    let a = scale_alpha(mapped.a, el.style.opacity.clamp(0.0, 1.0) * 0.8);
    let c = peniko::Color::from_rgba8(mapped.r, mapped.g, mapped.b, a);
    scene.fill(Fill::NonZero, base, c, None, &Rect::new(0.0, 0.0, w, h));
}

/// Grey bars where a text's lines would be.
fn draw_greek(scene: &mut Scene, base: Affine, el: &Element, theme: crate::theme::Theme) {
    let ElementKind::Text(t) = &el.kind else { return };
    let w = el.transform.w.max(0.0);
    let h = el.transform.h.max(0.0);
    let lines = (t.content.bytes().filter(|b| *b == b'\n').count() + 1).min(64);
    let lh = h / lines as f64;
    let mapped = theme.map(el.style.stroke_color);
    let a = scale_alpha(mapped.a, el.style.opacity.clamp(0.0, 1.0) * 0.45);
    let c = peniko::Color::from_rgba8(mapped.r, mapped.g, mapped.b, a);
    for i in 0..lines {
        let y = i as f64 * lh;
        scene.fill(Fill::NonZero, base, c, None, &Rect::new(0.0, y + lh * 0.3, w, y + lh * 0.7));
    }
}
