//! Painting on an image: the rules behind `image.paint`, `image.reset_paint` and the paint editor.
//!
//! Paint is non-destructive. The picture stays whole and untouched in the document; `ImageData::paint` holds the
//! brush strokes, in pixels of the stored picture (`natural_width` by `natural_height`). Because the strokes are in
//! picture pixels and not in canvas units, they follow the picture through crop, resize, rotation and mirroring.
//! `drafftink_scene::paint::bake` turns them into pixels.
//!
//! This module answers four questions:
//!
//! * What is a valid stroke, and how many may a picture carry ([`PaintProblem`], [`validate_stroke`], the limits
//!   of `drafftink_scene::paint`)?
//! * How are strokes added and removed ([`paint_image`], [`reset_paint`])? Each call is one write, so one undo step.
//! * Where is a point of the canvas on the picture ([`PictureSpace`])? It reuses [`crate::crop::Picture`], which
//!   already knows crop, flip and scale, so the paint editor and the commands agree with the renderer.
//! * What is the brush a session paints with ([`PaintBrush`]), and how big is it when nobody chose a size
//!   ([`default_brush_size`])?

use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_scene::limits::MAX_COORD;
use drafftink_scene::paint::{
    MAX_PAINT_POINTS, MAX_PAINT_SIZE, MAX_PAINT_STROKES, MIN_PAINT_SIZE, Paint, PaintMode, PaintStroke, next_stroke_id,
    paint_points,
};
use drafftink_scene::{Color, Element, ElementId, ElementKind, SceneRead, SceneTx, StoreError, StrokeId, Transform};

use crate::crop::{self, Picture};
use crate::target::TargetError;

/// Most strokes one call adds.
pub const MAX_STROKES_PER_CALL: usize = 64;
/// Most points in one stroke.
pub const MAX_STROKE_POINTS: usize = 10_000;

/// The smallest brush the editor picks for a picture, in picture pixels.
const DEFAULT_SIZE_MIN: f64 = 4.0;
/// The largest brush the editor picks for a picture, in picture pixels.
const DEFAULT_SIZE_MAX: f64 = 200.0;
/// The share of the longer side of the picture that the editor's first brush covers.
const DEFAULT_SIZE_SHARE: f64 = 0.015;

/// What is wrong with a stroke or a brush. The text names the allowed range.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PaintProblem {
    /// The brush diameter is not a number in range.
    #[error("the size is {0}, and it must be from {MIN_PAINT_SIZE} to {MAX_PAINT_SIZE} picture pixels")]
    Size(f64),
    /// The hardness is not a number in range.
    #[error("the hardness is {0}, and it must be from 0 (soft edge) to 1 (hard edge)")]
    Hardness(f64),
    /// The opacity is not a number in range.
    #[error("the opacity is {0}, and it must be from 0 (clear) to 1 (solid)")]
    Opacity(f64),
    /// The stroke has no points.
    #[error("the stroke has no points. Give at least one point; a single point is a dab")]
    NoPoints,
    /// The stroke has too many points.
    #[error("the stroke has {have} points, and the most is {MAX_STROKE_POINTS}. Split it into several strokes")]
    TooManyPoints {
        /// Points given.
        have: usize,
    },
    /// A coordinate is not a number or is absurdly large.
    #[error("a point is not a finite number within ±{MAX_COORD:e}. Give x and y in picture pixels")]
    Point,
    /// The pressures do not match the points.
    #[error("there are {pressures} pressures for {points} points. Give none, or one per point")]
    PressureCount {
        /// Points in the stroke.
        points: usize,
        /// Pressures given.
        pressures: usize,
    },
    /// A pressure is not a number from 0 to 1.
    #[error("a pressure is {0}, and it must be from 0 to 1")]
    Pressure(f64),
}

/// Why painting on an image, or taking the paint off, did not happen. The text says how to proceed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PaintError {
    /// The element is missing, deleted or locked.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// The element is not an image.
    #[error("Only images can be painted on, and {id} is a {kind}. Select an image.")]
    NotAnImage {
        /// The element.
        id: ElementId,
        /// Its kind, in words.
        kind: &'static str,
    },
    /// The picture's bytes are not in the document (yet).
    #[error(
        "The picture of {id} has not arrived yet, so it cannot be painted on. Wait for it to load, then try again."
    )]
    NotLoaded {
        /// The element.
        id: ElementId,
    },
    /// The image has no usable size.
    #[error("{id} has no size to paint on. Give it a width and height first.")]
    NoSize {
        /// The element.
        id: ElementId,
    },
    /// The call carries no stroke.
    #[error("There is no stroke to paint. Give at least one.")]
    NoStrokes,
    /// A stroke is not valid.
    #[error("Stroke {number} is not valid: {problem}.")]
    BadStroke {
        /// Which stroke of the call, from 1.
        number: usize,
        /// What is wrong with it.
        problem: PaintProblem,
    },
    /// A limit of the call or of the picture is exceeded.
    #[error("{what}: {have} is over the limit of {max}. {advice}")]
    Limit {
        /// What is counted.
        what: &'static str,
        /// How many there would be.
        have: usize,
        /// The limit.
        max: usize,
        /// What to do about it.
        advice: &'static str,
    },
    /// The stroke tag cannot make an id.
    #[error("The strokes could not be given ids. Use a tag of letters and digits.")]
    NoStrokeId,
    /// The store refused the write.
    #[error("{0}")]
    Store(String),
}

impl From<StoreError> for PaintError {
    fn from(e: StoreError) -> Self {
        Self::Store(e.to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// The brush
// ---------------------------------------------------------------------------------------------

/// The brush a session paints with: what a stroke gets for the settings it does not name.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct PaintBrush {
    /// The colour of a paint stroke. Its alpha multiplies `opacity`.
    pub color: Color,
    /// The diameter at full pressure, in picture pixels. While [`PaintBrush::size_chosen`] is false it is only the
    /// last default the editor picked.
    pub size: f64,
    /// 1 is a hard edge, 0 the softest.
    pub hardness: f64,
    /// 1 is solid, 0 is clear.
    pub opacity: f64,
    /// Paint or erase.
    pub mode: PaintMode,
    /// True once someone set the size. Until then the size follows the picture ([`default_brush_size`]).
    pub size_chosen: bool,
}

impl Default for PaintBrush {
    fn default() -> Self {
        Self {
            color: Color::rgb(220, 38, 38),
            size: 24.0,
            hardness: 0.8,
            opacity: 1.0,
            mode: PaintMode::Paint,
            size_chosen: false,
        }
    }
}

impl PaintBrush {
    /// Checks the numbers of the brush.
    pub fn validate(&self) -> Result<(), PaintProblem> {
        check_look(self.size, self.hardness, self.opacity)
    }

    /// The diameter for a picture of `natural` pixels: the chosen size, or the default for that picture.
    pub fn size_for(&self, natural: (u32, u32)) -> f64 {
        if self.size_chosen { self.size } else { default_brush_size(natural) }
    }

    /// A stroke of this brush over `points`, with the diameter `size` (see [`PaintBrush::size_for`]).
    pub fn stroke(&self, size: f64, points: Vec<Point>, pressures: Vec<f64>) -> PaintStroke {
        PaintStroke {
            mode: self.mode,
            color: self.color,
            size,
            hardness: self.hardness,
            opacity: self.opacity,
            points,
            pressures,
        }
    }
}

/// The brush diameter for a picture of `natural` pixels when nobody chose one: 1.5 % of its longer side, from 4
/// to 200 picture pixels. The one place that says so.
pub fn default_brush_size(natural: (u32, u32)) -> f64 {
    let longer = f64::from(natural.0.max(natural.1));
    (longer * DEFAULT_SIZE_SHARE).round().clamp(DEFAULT_SIZE_MIN, DEFAULT_SIZE_MAX)
}

fn in_range(v: f64, min: f64, max: f64) -> bool {
    v.is_finite() && (min..=max).contains(&v)
}

fn check_look(size: f64, hardness: f64, opacity: f64) -> Result<(), PaintProblem> {
    if !in_range(size, MIN_PAINT_SIZE, MAX_PAINT_SIZE) {
        return Err(PaintProblem::Size(size));
    }
    if !in_range(hardness, 0.0, 1.0) {
        return Err(PaintProblem::Hardness(hardness));
    }
    if !in_range(opacity, 0.0, 1.0) {
        return Err(PaintProblem::Opacity(opacity));
    }
    Ok(())
}

/// Checks one stroke against the rules of the model (`drafftink_scene::validate`) and the limit of points in a
/// stroke.
pub fn validate_stroke(s: &PaintStroke) -> Result<(), PaintProblem> {
    check_look(s.size, s.hardness, s.opacity)?;
    if s.points.is_empty() {
        return Err(PaintProblem::NoPoints);
    }
    if s.points.len() > MAX_STROKE_POINTS {
        return Err(PaintProblem::TooManyPoints { have: s.points.len() });
    }
    if !s.points.iter().all(|p| in_range(p.x, -MAX_COORD, MAX_COORD) && in_range(p.y, -MAX_COORD, MAX_COORD)) {
        return Err(PaintProblem::Point);
    }
    if !s.pressures.is_empty() && s.pressures.len() != s.points.len() {
        return Err(PaintProblem::PressureCount { points: s.points.len(), pressures: s.pressures.len() });
    }
    match s.pressures.iter().find(|p| !in_range(**p, 0.0, 1.0)) {
        Some(p) => Err(PaintProblem::Pressure(*p)),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------
// Where a point of the canvas is on the picture
// ---------------------------------------------------------------------------------------------

/// How the picture of one image lies on the canvas, so a canvas point can be named in picture pixels.
///
/// It is made from the image as it is shown now (box, crop, rotation, mirroring). The paint editor keeps one for
/// the length of a stroke.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PictureSpace {
    transform: Transform,
    picture: Picture,
}

impl PictureSpace {
    /// The space of an image element.
    pub fn of(e: &Element) -> Result<Self, PaintError> {
        if e.image_data().is_none() {
            return Err(PaintError::NotAnImage { id: e.id.clone(), kind: e.tag().name() });
        }
        // After the kind check the only way `Picture::of` fails is an image without a size.
        let picture = Picture::of(e).map_err(|_| PaintError::NoSize { id: e.id.clone() })?;
        Ok(Self { transform: e.transform, picture })
    }

    /// The point of the canvas `world` in pixels of the stored picture. It can lie outside the picture (a stroke
    /// that began inside and ran off the edge); the renderer clips.
    pub fn pixel(&self, world: Point) -> Point {
        let local = self.transform.world_to_local(world);
        self.picture.source_of(Rect::from_origin_size(local, Size::ZERO)).origin()
    }

    /// Whether the canvas point `world` is on the shown part of the image: inside its box, which turns with it.
    pub fn shows(&self, world: Point) -> bool {
        let local = self.transform.world_to_local(world);
        Rect::from_origin_size(Point::ZERO, Size::new(self.transform.w, self.transform.h)).contains(local)
    }

    /// Canvas units per picture pixel, the mean of across and down.
    pub fn units_per_pixel(&self) -> f64 {
        let (kx, ky) = self.picture.scale();
        (kx + ky) / 2.0
    }
}

// ---------------------------------------------------------------------------------------------
// Painting and resetting
// ---------------------------------------------------------------------------------------------

/// What [`paint_image`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Painted {
    /// The ids of the new strokes, in the order given.
    pub added: Vec<StrokeId>,
    /// The strokes the picture carries now.
    pub total: usize,
}

/// The image `id` as a target of paint: live, an image and unlocked.
fn paintable<'a, R: SceneRead>(scene: &'a R, id: &ElementId) -> Result<&'a Element, PaintError> {
    let e = scene.get(id).filter(|e| !e.deleted).ok_or_else(|| TargetError::NotFound(id.clone()))?;
    if e.image_data().is_none() {
        return Err(PaintError::NotAnImage { id: id.clone(), kind: e.tag().name() });
    }
    if e.locked {
        return Err(TargetError::Locked(vec![id.clone()]).into());
    }
    Ok(e)
}

/// Adds `strokes` to the image `id`, as one write (one undo step).
///
/// The image must be live, unlocked, and its picture must be in the document. Everything is checked before the
/// first write. The strokes get ids that order them after the strokes already there; `tag` (letters and digits)
/// tells two peers' strokes with the same number apart. A stroke that would show nothing is stored all the same.
pub fn paint_image<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    strokes: Vec<PaintStroke>,
    tag: &str,
) -> Result<Painted, PaintError> {
    let e = paintable(&*tx, id)?;
    let Some(data) = e.image_data() else {
        return Err(PaintError::NotAnImage { id: id.clone(), kind: e.tag().name() });
    };
    if !data.file_id.as_ref().is_some_and(|f| tx.file(f).is_some()) {
        return Err(PaintError::NotLoaded { id: id.clone() });
    }
    if strokes.is_empty() {
        return Err(PaintError::NoStrokes);
    }
    if strokes.len() > MAX_STROKES_PER_CALL {
        return Err(PaintError::Limit {
            what: "Strokes in one call",
            have: strokes.len(),
            max: MAX_STROKES_PER_CALL,
            advice: "Split them into several calls.",
        });
    }
    for (i, s) in strokes.iter().enumerate() {
        validate_stroke(s).map_err(|problem| PaintError::BadStroke { number: i + 1, problem })?;
    }
    let total = data.paint.len().saturating_add(strokes.len());
    if total > MAX_PAINT_STROKES {
        return Err(PaintError::Limit {
            what: "Strokes on the picture",
            have: total,
            max: MAX_PAINT_STROKES,
            advice: "Reset the paint, or erase and start again.",
        });
    }
    let points = paint_points(&data.paint).saturating_add(strokes.iter().map(|s| s.points.len()).sum());
    if points > MAX_PAINT_POINTS {
        return Err(PaintError::Limit {
            what: "Points in the strokes of the picture",
            have: points,
            max: MAX_PAINT_POINTS,
            advice: "Reset the paint, or paint with fewer points.",
        });
    }
    if next_stroke_id(&Paint::new(), tag).is_none() {
        return Err(PaintError::NoStrokeId);
    }

    let mut added = Vec::with_capacity(strokes.len());
    tx.update(id, |e| {
        let ElementKind::Image(d) = &mut e.kind else { return };
        for s in strokes {
            let Some(stroke_id) = next_stroke_id(&d.paint, tag) else { break };
            added.push(stroke_id.clone());
            d.paint.insert(stroke_id, s);
        }
    })?;
    let total = tx.get(id).and_then(Element::image_data).map_or(added.len(), |d| d.paint.len());
    Ok(Painted { added, total })
}

/// Takes all the paint off the image `id`, as one write. Returns how many strokes went; an image with no paint
/// is left alone (nothing is written).
pub fn reset_paint<T: SceneTx>(tx: &mut T, id: &ElementId) -> Result<usize, PaintError> {
    let removed = paintable(&*tx, id)?.image_data().map_or(0, |d| d.paint.len());
    if removed == 0 {
        return Ok(0);
    }
    tx.update(id, |e| {
        if let ElementKind::Image(d) = &mut e.kind {
            d.paint.clear();
        }
    })?;
    Ok(removed)
}

/// Whether an element is an unlocked image whose picture is in the scene: what painting needs. Used to offer
/// the paint commands; [`paint_image`] checks again. The same condition as for cropping.
pub fn can_paint<R: SceneRead>(scene: &R, e: &Element) -> bool {
    crop::can_crop(scene, e)
}

#[cfg(test)]
#[path = "retouch_tests.rs"]
mod tests;
