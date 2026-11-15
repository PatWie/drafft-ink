//! The paint editor: what the pointer and the keys do while an image is being painted on
//! (`docs/exceptions.md` E18, `docs/interaction.md`).
//!
//! `image.paint_begin` opens the editor (a mode recorded in the session's observed state). From then on this
//! machine owns the pointer:
//!
//! * A drag that starts on the image **paints a stroke** with the session brush (`image.paint_brush`). The points
//!   are converted into pixels of the stored picture as they come, so the stroke follows the picture through crop,
//!   rotation and mirroring. A click without a drag is a dab.
//! * Nothing in the document changes while the pointer moves (class B). The overlay shows the image with the draft
//!   stroke already in its paint, and the brush ring at the pointer.
//! * When the pointer is released the stroke is simplified and one `image.paint` stores it: one undo step per
//!   stroke. The draft and the stored stroke are the same stroke, so the picture does not change when it lands.
//! * `Esc` during a drag drops the stroke. `Enter`, `Esc` without a drag, and a click outside the image end the
//!   editor (`image.paint_end`). The strokes already painted are in the document.
//!
//! The geometry is in the element's own frame, where `drafftink_edit::retouch::PictureSpace` knows how the picture
//! lies, so a rotated, mirrored or cropped image is painted on the way the user sees it.

use std::borrow::Cow;

use drafftink_edit::brush::StrokeSampler;
use drafftink_edit::create::simplify_stroke;
use drafftink_edit::retouch::{PaintBrush, PictureSpace};
use drafftink_edit::style::ColorArg;
use drafftink_geom::kurbo::Point;
use drafftink_scene::paint::{PaintStroke, next_stroke_id};
use drafftink_scene::{Brush, Element, ElementKind, PressureMode};
use serde_json::json;

use super::overlay::{EraserRing, PaintView};
use super::world::PaintStage;
use crate::cmd::image_paint::{PaintArgs, PaintStrokeArg};
use crate::interact::ctx::Ctx;
use crate::interact::events::{Ev, Key};
use crate::session::{Ref, Session};

/// Samples closer than this to the last one (in screen pixels) are skipped, as for a freehand stroke.
const MIN_SPACING_PX: f64 = 0.5;
/// A stroke never holds more samples than this; once full, the newest sample replaces the last one.
const MAX_SAMPLES: usize = 5_000;
/// The simplification of a stroke when it is stored, as a share of the brush diameter. The result is not
/// told from the original on the picture.
const SIMPLIFY_SHARE: f64 = 0.05;
/// The least tolerance of the simplification, in picture pixels.
const SIMPLIFY_MIN: f64 = 0.25;
/// What the draft stroke of the overlay is called in the paint of the shown image.
const DRAFT_TAG: &str = "draft";

/// The image the editor is open on, as it was when the editor last looked.
#[derive(Debug, Clone)]
struct Editing {
    element: Element,
    space: PictureSpace,
}

impl Editing {
    fn open(e: &Element) -> Option<Self> {
        Some(Self { element: e.clone(), space: PictureSpace::of(e).ok()? })
    }

    /// The size of the stored picture in pixels.
    fn natural(&self) -> (u32, u32) {
        self.element.image_data().map_or((0, 0), |d| (d.natural_width, d.natural_height))
    }

    /// The image as the overlay shows it: with `draft` laid over its paint.
    fn shown(&self, draft: Option<&PaintStroke>) -> Element {
        let mut e = self.element.clone();
        if let (Some(stroke), ElementKind::Image(d)) = (draft, &mut e.kind)
            && let Some(id) = next_stroke_id(&d.paint, DRAFT_TAG)
        {
            d.paint.insert(id, stroke.clone());
        }
        e
    }

    fn view(&self, draft: Option<&PaintStroke>) -> PaintView {
        PaintView { element: self.element.id.clone(), shown: self.shown(draft), veil: true }
    }
}

/// The stroke being dragged.
#[derive(Debug)]
struct Stroke {
    space: PictureSpace,
    brush: PaintBrush,
    /// The diameter, resolved when the stroke began.
    size: f64,
    sampler: Option<StrokeSampler>,
    /// The last sample kept, in world units, for the spacing.
    last: Option<Point>,
    /// The samples in pixels of the stored picture.
    points: Vec<Point>,
    /// One pressure per point from a pen; empty for a mouse or a finger.
    pressures: Vec<f64>,
}

impl Stroke {
    fn start(editing: &Editing, brush: PaintBrush) -> Self {
        Self {
            space: editing.space,
            size: brush.size_for(editing.natural()),
            brush,
            sampler: None,
            last: None,
            points: Vec::new(),
            pressures: Vec::new(),
        }
    }

    /// Adds the press position (once) and the pointer's, as the freehand tool does: the first sample is the press,
    /// with what the pen did there; the drag update that follows brings the first move.
    fn update(&mut self, cx: &Ctx<'_>, ev: &Ev<'_>) {
        if self.sampler.is_none() {
            let (sampler, first) = StrokeSampler::start(
                Brush::Normal,
                PressureMode::Uniform,
                ev.origin.world,
                ev.meta.time_ms,
                ev.pen_pressure(),
            );
            self.sampler = Some(sampler);
            self.push(first.point, first.pressure);
        }
        let sample = self.sampler.as_mut().and_then(|s| s.push(ev.at.world, ev.meta.time_ms, ev.pen_pressure()));
        if let Some(sample) = sample {
            self.push_spaced(cx.px(MIN_SPACING_PX), sample.point, sample.pressure);
        }
    }

    fn push(&mut self, world: Point, pressure: Option<f64>) {
        self.last = Some(world);
        self.points.push(self.space.pixel(world));
        self.pressures.extend(pressure);
    }

    /// Adds a sample when it is more than `min_gap` (world units) from the last one.
    fn push_spaced(&mut self, min_gap: f64, world: Point, pressure: Option<f64>) {
        if self.last.is_some_and(|l| l.distance(world) < min_gap) {
            return;
        }
        if self.points.len() < MAX_SAMPLES {
            self.push(world, pressure);
            return;
        }
        // Full: the end of the stroke keeps following the pointer.
        self.last = Some(world);
        if let Some(slot) = self.points.last_mut() {
            *slot = self.space.pixel(world);
        }
        if let (Some(slot), Some(v)) = (self.pressures.last_mut(), pressure) {
            *slot = v;
        }
    }

    /// The stroke as it is stored: simplified, with the brush it was made with. The overlay shows this same
    /// stroke, so the picture does not change when the pointer is released.
    fn stroke(&self) -> PaintStroke {
        let tolerance = (self.size * SIMPLIFY_SHARE).max(SIMPLIFY_MIN);
        let (points, pressures) = simplify_stroke(&self.points, &self.pressures, tolerance);
        self.brush.stroke(self.size, points, pressures)
    }
}

/// The paint editor of the interaction layer.
#[derive(Debug, Default)]
pub(crate) struct PaintTool {
    editing: Option<Editing>,
    stroke: Option<Stroke>,
    /// The pointer when no button is down, in world units, for the brush ring. `None` after it left the canvas.
    pointer: Option<Point>,
}

impl PaintTool {
    /// Follows the session: the editor is open exactly when the session says so, on the image it names. A draft
    /// whose image changed under it (undo, a stroke just stored, a collaborator) is made again from the image as
    /// it is now, and a stroke that was running is dropped.
    pub(crate) fn sync(&mut self, s: &Session) {
        let open = s.tab().observed.paint_editor.as_ref().and_then(|id| s.element(id));
        let fresh = match (&self.editing, open) {
            (Some(d), Some(e)) => d.element.id == e.id && d.element.version == e.version,
            (None, None) => true,
            _ => false,
        };
        if fresh {
            return;
        }
        self.stroke = None;
        self.editing = open.and_then(Editing::open);
    }

    /// True while the editor is open.
    pub(crate) fn is_open(&self) -> bool {
        self.editing.is_some()
    }

    /// True while a stroke is being dragged.
    pub(crate) fn is_active(&self) -> bool {
        self.stroke.is_some()
    }

    /// What the picker needs while the editor is open: the image, the only thing the pointer can reach.
    pub(crate) fn stage(&self) -> Option<PaintStage> {
        self.editing.as_ref().map(|d| PaintStage { element: d.element.id.clone(), space: d.space })
    }

    /// The image the session says is being painted on. The editor may have been opened by a command since the last
    /// pointer event (the strip's button), so what is not there yet is made from the image as it is.
    fn current(&self, s: &Session) -> Option<Cow<'_, Editing>> {
        let live = s.tab().observed.paint_editor.as_ref().and_then(|id| s.element(id))?;
        match &self.editing {
            Some(d) if d.element.id == live.id && d.element.version == live.version => Some(Cow::Borrowed(d)),
            _ => Editing::open(live).map(Cow::Owned),
        }
    }

    /// What the overlay shows.
    pub(crate) fn view(&self, s: &Session) -> Option<PaintView> {
        let editing = self.current(s)?;
        Some(editing.view(self.stroke.as_ref().map(Stroke::stroke).as_ref()))
    }

    /// The brush ring: round the pointer while it is over the image or a stroke runs. Its radius is a screen size.
    pub(crate) fn ring(&self, s: &Session, zoom: f64) -> Option<EraserRing> {
        let editing = self.current(s)?;
        let at = self.pointer?;
        if self.stroke.is_none() && !editing.space.shows(at) {
            return None;
        }
        let size = self.stroke.as_ref().map_or_else(|| s.tool.paint_brush.size_for(editing.natural()), |t| t.size);
        Some(EraserRing { at, radius_px: (size / 2.0 * editing.space.units_per_pixel() * zoom).max(1.0) })
    }

    // ---- pointer ----

    /// The pointer moved with no button down: the ring follows it.
    pub(crate) fn hover(&mut self, cx: &mut Ctx<'_>, world: Point) {
        let Some(editing) = &self.editing else { return };
        let was_over = self.pointer.is_some_and(|p| editing.space.shows(p));
        self.pointer = Some(world);
        if was_over || editing.space.shows(world) {
            cx.overlay_changed();
        }
    }

    /// The pointer left the canvas.
    pub(crate) fn leave(&mut self, cx: &mut Ctx<'_>) {
        if self.pointer.take().is_some() && self.is_open() {
            cx.overlay_changed();
        }
    }

    /// A drag starts. On the image it begins a stroke; anywhere else it is ignored.
    pub(crate) fn drag_start(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let Some(editing) = &self.editing else { return };
        if !editing.space.shows(ev.origin.world) {
            self.stroke = None;
            return;
        }
        let mut stroke = Stroke::start(editing, cx.s.tool.paint_brush);
        stroke.update(cx, ev);
        self.stroke = Some(stroke);
        self.pointer = Some(ev.at.world);
        cx.overlay_changed();
    }

    pub(crate) fn drag_update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let Some(stroke) = &mut self.stroke else { return };
        stroke.update(cx, ev);
        self.pointer = Some(ev.at.world);
        cx.overlay_changed();
    }

    /// The pointer was released: one `image.paint`. A cancelled drag drops the stroke.
    pub(crate) fn drag_end(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>, cancelled: bool) {
        if !cancelled {
            self.drag_update(cx, ev);
        }
        let Some(stroke) = self.stroke.take() else { return };
        if !cancelled {
            commit(cx, self.editing.as_ref(), &stroke.stroke());
        }
        cx.overlay_changed();
    }

    /// A click. On the image it is a dab; outside the image the editor ends, and the result says the click should
    /// go on to the select tool (which selects what is under it).
    pub(crate) fn click(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) -> bool {
        let Some(editing) = &self.editing else { return false };
        if !editing.space.shows(ev.at.world) {
            self.finish(cx);
            return true;
        }
        let mut dab = Stroke::start(editing, cx.s.tool.paint_brush);
        dab.push(ev.at.world, ev.pen_pressure());
        commit(cx, Some(editing), &dab.stroke());
        false
    }

    // ---- keys ----

    /// `Enter` and `Esc` end the editor. Returns whether the key was used.
    pub(crate) fn key(&mut self, cx: &mut Ctx<'_>, _key: Key) -> bool {
        if !self.is_open() {
            return false;
        }
        self.finish(cx);
        true
    }

    /// Ends the editor (`image.paint_end`) and drops a stroke that was running.
    fn finish(&mut self, cx: &mut Ctx<'_>) {
        self.stroke = None;
        if cx.s.tab().observed.paint_editor.is_some() {
            cx.exec("image.paint_end", json!({}));
        }
        cx.overlay_changed();
    }

    /// Drops a running stroke without storing it. The editor stays open; a tool change closes it through the
    /// session, and `sync` drops the draft.
    pub(crate) fn cancel(&mut self) {
        self.stroke = None;
    }
}

/// The one `image.paint` that stores `stroke` on the image of `editing`.
fn commit(cx: &mut Ctx<'_>, editing: Option<&Editing>, stroke: &PaintStroke) {
    let Some(editing) = editing else { return };
    let arg = PaintStrokeArg {
        mode: Some(stroke.mode),
        color: Some(ColorArg(stroke.color)),
        size: Some(stroke.size),
        hardness: Some(stroke.hardness),
        opacity: Some(stroke.opacity),
        points: stroke.points.iter().map(|p| [p.x, p.y]).collect(),
        pressures: stroke.pressures.clone(),
    };
    let args = PaintArgs { id: Some(Ref(editing.element.id.as_str().to_owned())), strokes: vec![arg] };
    cx.exec_typed("image.paint", &args);
}
