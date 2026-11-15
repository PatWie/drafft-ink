//! Paint on a picture: the strokes an image carries and the pixels they make.
//!
//! A painted image keeps its original bytes. The paint is a list of brush strokes in pixels of the stored picture
//! (`ImageData::natural_width` by `natural_height`), so it follows the picture through crop, rotation, flip, resize
//! and the corner radius. [`bake`] turns the strokes into pixels; the renderer, the exporters and the command line
//! all call it, so every output shows the same picture. It works on plain straight-alpha RGBA8 and needs no image
//! decoder.
//!
//! * A stroke is a polyline with a brush: a colour, a diameter, a hardness, an opacity, and optionally a pen
//!   pressure per point. An *erase* stroke gives the original pixels back, in proportion to how much the brush
//!   covers them (it removes paint, it never cuts a hole in the picture).
//! * A stroke has a uniform opacity: where it crosses itself the paint does not get darker.
//! * Strokes are applied in the order of their [`StrokeId`], which starts with a sequence number. They live in a
//!   map, so two people painting the same picture at once both keep their strokes.

use std::collections::BTreeMap;

use kurbo::Point;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::id::StrokeId;
use crate::points::pts;

/// Most strokes one picture carries.
pub const MAX_PAINT_STROKES: usize = 1000;
/// Most points in all the strokes of one picture together.
pub const MAX_PAINT_POINTS: usize = 100_000;
/// Largest brush diameter, in pixels of the picture.
pub const MAX_PAINT_SIZE: f64 = 4096.0;
/// Smallest brush diameter, in pixels of the picture.
pub const MIN_PAINT_SIZE: f64 = 0.5;

/// The share of the brush a pen with no pressure at all still draws.
const MIN_PRESSURE_SCALE: f64 = 0.2;
/// The shortest piece a stroke segment is cut into when it is rasterised, in pixels.
const MIN_PIECE: f64 = 4.0;
/// The most pieces one segment is cut into, so that a hostile segment cannot keep the loop busy.
const MAX_PIECES: usize = 4096;

/// What a stroke does to the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaintMode {
    /// Lays the colour over the picture.
    #[default]
    Paint,
    /// Gives the original pixels back where the brush passes (removes paint made before it).
    Erase,
}

impl PaintMode {
    fn is_paint(&self) -> bool {
        *self == PaintMode::Paint
    }
}

/// One brush stroke on a picture. Coordinates and sizes are in pixels of the stored picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PaintStroke {
    /// Paint or erase.
    #[serde(default, skip_serializing_if = "PaintMode::is_paint")]
    pub mode: PaintMode,
    /// The colour of a paint stroke. Its alpha multiplies `opacity`. An erase stroke ignores it.
    pub color: Color,
    /// Brush diameter, in picture pixels, at full pressure.
    pub size: f64,
    /// 1 is a hard edge; towards 0 the edge fades over more of the radius.
    pub hardness: f64,
    /// How much of the picture is covered, 1 (all) to 0 (none). The whole stroke has this opacity.
    pub opacity: f64,
    /// Where the brush went, in picture pixels. At least 1 (a single point is a dab).
    #[serde(with = "pts")]
    #[schemars(with = "Vec<[f64; 2]>")]
    pub points: Vec<Point>,
    /// Pen pressure per point, 0 to 1. Empty, or exactly one per point.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pressures: Vec<f64>,
}

impl PaintStroke {
    /// The radius of the brush at point `i`, in picture pixels.
    fn radius_at(&self, i: usize) -> f64 {
        let scale =
            self.pressures.get(i).map_or(1.0, |p| MIN_PRESSURE_SCALE + (1.0 - MIN_PRESSURE_SCALE) * p.clamp(0.0, 1.0));
        self.size / 2.0 * scale
    }
}

/// The strokes of a picture, in the order they were made.
pub type Paint = BTreeMap<StrokeId, PaintStroke>;

/// The id for the next stroke of `paint`: the next sequence number, then `tag` (what makes two peers' strokes with
/// the same number differ).
pub fn next_stroke_id(paint: &Paint, tag: &str) -> Option<StrokeId> {
    let next = paint
        .keys()
        .filter_map(|k| k.as_str().split_once('-').and_then(|(n, _)| n.parse::<u32>().ok()))
        .max()
        .map_or(1, |n| n.saturating_add(1));
    let tag: String = tag.chars().filter(char::is_ascii_alphanumeric).take(12).collect();
    StrokeId::new(&format!("{next:06}-{tag}")).ok()
}

/// The number of points in all strokes of `paint`.
pub fn paint_points(paint: &Paint) -> usize {
    paint.values().map(|s| s.points.len()).sum()
}

// ---------------------------------------------------------------------------------------------
// Pixels
// ---------------------------------------------------------------------------------------------

/// Paints `strokes` over `original`, straight-alpha RGBA8 of `width` by `height` pixels, and returns the result.
///
/// Stroke coordinates are in picture pixels; `scale` is how many pixels of `original` one picture pixel is (`1, 1`
/// when the decoded picture has the size of the stored one). Returns `None` when `original` is not
/// `width * height * 4` bytes. Strokes with numbers that are not finite or are absurdly large are skipped.
pub fn bake<'a>(
    original: &[u8],
    width: u32,
    height: u32,
    scale: (f64, f64),
    strokes: impl IntoIterator<Item = &'a PaintStroke>,
) -> Option<Vec<u8>> {
    bake_over(original, original, width, height, scale, strokes)
}

/// Like [`bake`], for strokes that come after others that are already painted: `base` is `original` with those
/// earlier strokes on it, and `strokes` are only the later ones. Erase strokes still give back pixels of
/// `original`. Painting the first strokes with [`bake`] and the rest with this gives the same pixels as painting
/// them all at once, so a picture that gains one stroke at a time need not paint the old ones again. Returns `None`
/// when either buffer is not `width * height * 4` bytes.
pub fn bake_over<'a>(
    base: &[u8],
    original: &[u8],
    width: u32,
    height: u32,
    scale: (f64, f64),
    strokes: impl IntoIterator<Item = &'a PaintStroke>,
) -> Option<Vec<u8>> {
    let (w, h) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    let len = w.checked_mul(h)?.checked_mul(4)?;
    if original.len() != len || base.len() != len {
        return None;
    }
    let mut out = base.to_vec();
    let canvas = Canvas { w, h, scale };
    for stroke in strokes {
        canvas.apply(&mut out, original, stroke);
    }
    Some(out)
}

/// The size of the pixel grid a stroke is drawn on, and how stroke coordinates map onto it.
struct Canvas {
    w: usize,
    h: usize,
    scale: (f64, f64),
}

/// A disc of the brush: centre and radius in pixels of the grid.
#[derive(Clone, Copy)]
struct Disc {
    at: Point,
    r: f64,
}

/// Coverage of one stroke over a rectangle of the grid, 0 to 255 per pixel.
struct Mask {
    x0: usize,
    y0: usize,
    w: usize,
    data: Vec<u8>,
}

impl Canvas {
    fn apply(&self, pixels: &mut [u8], original: &[u8], stroke: &PaintStroke) {
        let Some(mask) = self.coverage(&self.discs(stroke), stroke.hardness) else { return };
        let opacity = stroke.opacity.clamp(0.0, 1.0) as f32;
        let colour = stroke.color;
        for (row, y) in mask.data.chunks_exact(mask.w).zip(mask.y0..) {
            let start = (y * self.w + mask.x0) * 4;
            let end = start + mask.w * 4;
            let (Some(dst), Some(src)) = (pixels.get_mut(start..end), original.get(start..end)) else { continue };
            for ((cover, px), orig) in row.iter().zip(dst.chunks_exact_mut(4)).zip(src.chunks_exact(4)) {
                if *cover == 0 {
                    continue;
                }
                let amount = f32::from(*cover) / 255.0 * opacity;
                match stroke.mode {
                    PaintMode::Paint => over(px, colour, amount * f32::from(colour.a) / 255.0),
                    PaintMode::Erase => restore(px, orig, amount),
                }
            }
        }
    }

    /// The brush discs of a stroke in pixels of the grid. A point or a size that is not usable leaves its disc out.
    fn discs(&self, stroke: &PaintStroke) -> Vec<Disc> {
        let radius_scale = (self.scale.0 * self.scale.1).abs().sqrt();
        let usable = |v: f64| v.is_finite() && v.abs() <= crate::limits::MAX_COORD;
        stroke
            .points
            .iter()
            .enumerate()
            .filter(|(_, p)| usable(p.x) && usable(p.y))
            .map(|(i, p)| Disc {
                at: Point::new(p.x * self.scale.0, p.y * self.scale.1),
                r: stroke.radius_at(i) * radius_scale,
            })
            .filter(|d| d.r.is_finite() && d.r >= 0.0 && d.at.x.is_finite() && d.at.y.is_finite())
            .collect()
    }

    /// How much of each pixel the brush covers, over the part of the grid it reaches. `None` when it reaches none.
    fn coverage(&self, discs: &[Disc], hardness: f64) -> Option<Mask> {
        let hardness = if hardness.is_finite() { hardness.clamp(0.0, 1.0) } else { 1.0 };
        let (mut lo, mut hi) = (Point::new(f64::MAX, f64::MAX), Point::new(f64::MIN, f64::MIN));
        for d in discs {
            lo = Point::new(lo.x.min(d.at.x - d.r), lo.y.min(d.at.y - d.r));
            hi = Point::new(hi.x.max(d.at.x + d.r), hi.y.max(d.at.y + d.r));
        }
        let (x0, x1) = span(lo.x - 1.0, hi.x + 1.0, self.w)?;
        let (y0, y1) = span(lo.y - 1.0, hi.y + 1.0, self.h)?;
        let mut mask = Mask { x0, y0, w: x1 - x0, data: vec![0; (x1 - x0) * (y1 - y0)] };
        match discs {
            [] => return None,
            [one] => mask.stamp(*one, *one, hardness),
            many => many.windows(2).for_each(|pair| {
                if let [a, b] = pair {
                    mask.capsule(*a, *b, hardness);
                }
            }),
        }
        Some(mask)
    }
}

/// The pixels `lo..hi` covers, limited to `0..len`. `None` when there are none.
fn span(lo: f64, hi: f64, len: usize) -> Option<(usize, usize)> {
    if !(lo.is_finite() && hi.is_finite()) {
        return None;
    }
    let start = lo.floor().max(0.0) as usize;
    let end = (hi.ceil().max(0.0) as usize).min(len);
    (start < end).then_some((start, end))
}

impl Mask {
    fn height(&self) -> usize {
        self.data.len().checked_div(self.w).unwrap_or(0)
    }

    /// Adds the brush moving from `a` to `b`, in pieces so that the box of each piece stays close to the brush.
    fn capsule(&mut self, a: Disc, b: Disc, hardness: f64) {
        let piece = MIN_PIECE.max(a.r.max(b.r));
        let n = ((a.at.distance(b.at) / piece).ceil().max(1.0) as usize).min(MAX_PIECES);
        let at = |t: f64| Disc { at: a.at.lerp(b.at, t), r: a.r + (b.r - a.r) * t };
        for i in 0..n {
            self.stamp(at(i as f64 / n as f64), at((i + 1) as f64 / n as f64), hardness);
        }
    }

    /// Raises the coverage of every pixel near the segment `a`..`b` (the radius changes from `a.r` to `b.r`).
    fn stamp(&mut self, a: Disc, b: Disc, hardness: f64) {
        let reach = a.r.max(b.r) + 1.0;
        let x_lo = (a.at.x.min(b.at.x) - reach).floor().max(self.x0 as f64) as usize;
        let x_hi = ((a.at.x.max(b.at.x) + reach).ceil().max(0.0) as usize).min(self.x0 + self.w);
        let y_lo = (a.at.y.min(b.at.y) - reach).floor().max(self.y0 as f64) as usize;
        let y_hi = ((a.at.y.max(b.at.y) + reach).ceil().max(0.0) as usize).min(self.y0 + self.height());
        let (dx, dy) = (b.at.x - a.at.x, b.at.y - a.at.y);
        let length2 = dx * dx + dy * dy;
        let (x0, w) = (self.x0, self.w);
        let rows = self.data.chunks_exact_mut(w).zip(self.y0..);
        for (row, y) in rows.skip(y_lo.saturating_sub(self.y0)).take(y_hi.saturating_sub(y_lo)) {
            let py = y as f64 + 0.5;
            let cells = row.iter_mut().zip(x0..);
            for (cell, x) in cells.skip(x_lo.saturating_sub(x0)).take(x_hi.saturating_sub(x_lo)) {
                let px = x as f64 + 0.5;
                let along = if length2 > 0.0 {
                    (((px - a.at.x) * dx + (py - a.at.y) * dy) / length2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (cx, cy) = (a.at.x + dx * along, a.at.y + dy * along);
                let distance = (px - cx).hypot(py - cy);
                let value = (edge(distance, a.r + (b.r - a.r) * along, hardness) * 255.0).round() as u8;
                *cell = (*cell).max(value);
            }
        }
    }
}

/// How much of a pixel at `distance` from the middle of a brush of radius `r` the brush covers. A hard brush has a
/// one pixel edge; a soft one fades from `hardness * r` out to `r`.
fn edge(distance: f64, r: f64, hardness: f64) -> f64 {
    let fade = (r - r * hardness).max(1.0);
    let t = ((r + 0.5 - distance) / fade).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `px` (straight alpha) with `colour` laid over it at `alpha`.
fn over(px: &mut [u8], colour: Color, alpha: f32) {
    let [r, g, b, a] = px else { return };
    let under = f32::from(*a) / 255.0;
    let out = alpha + under * (1.0 - alpha);
    if out <= 0.0 {
        return;
    }
    let mix = |src: u8, dst: u8| (f32::from(src) * alpha + f32::from(dst) * under * (1.0 - alpha)) / out;
    *r = to_byte(mix(colour.r, *r));
    *g = to_byte(mix(colour.g, *g));
    *b = to_byte(mix(colour.b, *b));
    *a = to_byte(out * 255.0);
}

/// `px` moved towards the original pixel `orig` by `amount` (0 to 1), in premultiplied colour so that a clear pixel
/// has no colour of its own.
fn restore(px: &mut [u8], orig: &[u8], amount: f32) {
    let [r, g, b, a] = px else { return };
    let &[or, og, ob, oa] = orig else { return };
    let (now, then) = (f32::from(*a) / 255.0, f32::from(oa) / 255.0);
    let out = now + (then - now) * amount;
    if out <= 0.0 {
        *a = 0;
        return;
    }
    let mix = |cur: u8, original: u8| {
        let (c, o) = (f32::from(cur) * now, f32::from(original) * then);
        (c + (o - c) * amount) / out
    };
    *r = to_byte(mix(*r, or));
    *g = to_byte(mix(*g, og));
    *b = to_byte(mix(*b, ob));
    *a = to_byte(out * 255.0);
}

fn to_byte(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 20;
    const H: u32 = 10;
    const GREY: [u8; 4] = [100, 100, 100, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];

    fn grey() -> Vec<u8> {
        GREY.repeat((W * H) as usize)
    }

    fn pixel(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * W as usize + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn stroke(points: &[(f64, f64)]) -> PaintStroke {
        PaintStroke {
            mode: PaintMode::Paint,
            color: Color::rgb(255, 0, 0),
            size: 4.0,
            hardness: 1.0,
            opacity: 1.0,
            points: points.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            pressures: Vec::new(),
        }
    }

    fn bake_one(original: &[u8], strokes: &[&PaintStroke]) -> Vec<u8> {
        bake(original, W, H, (1.0, 1.0), strokes.iter().copied()).unwrap()
    }

    #[test]
    fn a_hard_stroke_covers_its_width_and_leaves_the_rest() {
        let out = bake_one(&grey(), &[&stroke(&[(3.0, 5.0), (15.0, 5.0)])]);
        assert_eq!(pixel(&out, 9, 5), RED, "on the stroke");
        assert_eq!(pixel(&out, 9, 4), RED, "inside the diameter");
        assert_eq!(pixel(&out, 9, 9), GREY, "away from it");
        assert_eq!(pixel(&out, 18, 5), GREY, "past the end");
    }

    #[test]
    fn a_single_point_is_a_dab() {
        let out = bake_one(&grey(), &[&stroke(&[(10.0, 5.0)])]);
        assert_eq!(pixel(&out, 10, 5), RED);
        assert_eq!(pixel(&out, 14, 5), GREY);
    }

    #[test]
    fn opacity_is_uniform_where_a_stroke_crosses_itself() {
        let s = PaintStroke { opacity: 0.5, ..stroke(&[(2.0, 5.0), (16.0, 5.0), (2.0, 5.0)]) };
        let [r, g, ..] = pixel(&bake_one(&grey(), &[&s]), 9, 5);
        assert!((i32::from(r) - 178).abs() <= 1 && (i32::from(g) - 50).abs() <= 1, "one layer of half red: {r} {g}");
    }

    #[test]
    fn an_erase_stroke_gives_the_original_pixels_back_and_only_there() {
        let red = stroke(&[(1.0, 5.0), (18.0, 5.0)]);
        let erase = PaintStroke { mode: PaintMode::Erase, ..stroke(&[(8.0, 5.0), (11.0, 5.0)]) };
        let out = bake_one(&grey(), &[&red, &erase]);
        assert_eq!(pixel(&out, 9, 5), GREY, "restored");
        assert_eq!(pixel(&out, 3, 5), RED, "paint elsewhere stays");
    }

    #[test]
    fn an_erase_stroke_does_not_make_a_hole_in_the_picture() {
        let erase = PaintStroke { mode: PaintMode::Erase, ..stroke(&[(8.0, 5.0)]) };
        assert_eq!(bake_one(&grey(), &[&erase]), grey());
    }

    #[test]
    fn painting_over_a_clear_pixel_gives_the_colour_and_the_stroke_alpha() {
        let clear = [0u8; 4].repeat((W * H) as usize);
        let s = PaintStroke { opacity: 0.5, ..stroke(&[(10.0, 5.0)]) };
        let [r, g, b, a] = pixel(&bake_one(&clear, &[&s]), 10, 5);
        assert_eq!((r, g, b), (255, 0, 0), "the colour is not darkened by the clear pixel");
        assert!((i32::from(a) - 128).abs() <= 1, "{a}");
    }

    #[test]
    fn a_soft_brush_fades_towards_its_edge() {
        let s = PaintStroke { size: 8.0, hardness: 0.0, ..stroke(&[(10.0, 5.0)]) };
        let out = bake_one(&grey(), &[&s]);
        let (centre, rim) = (pixel(&out, 10, 5)[1], pixel(&out, 13, 5)[1]);
        assert!(centre < rim && rim <= 100, "centre {centre}, rim {rim}");
    }

    #[test]
    fn pressure_makes_the_stroke_thinner() {
        let s = PaintStroke { size: 8.0, pressures: vec![1.0, 0.0], ..stroke(&[(2.0, 5.0), (17.0, 5.0)]) };
        let out = bake_one(&grey(), &[&s]);
        assert_eq!(pixel(&out, 3, 7), RED, "thick at the start");
        assert_eq!(pixel(&out, 16, 7), GREY, "thin at the end");
    }

    #[test]
    fn coordinates_follow_the_scale_of_the_decoded_picture() {
        // The stored picture is half the size of the decoded one.
        let out = bake(&grey(), W, H, (2.0, 2.0), [&stroke(&[(5.0, 2.0)])]).unwrap();
        assert_eq!(pixel(&out, 10, 4), RED);
        assert_eq!(pixel(&out, 5, 2), GREY);
    }

    #[test]
    fn strokes_outside_the_picture_and_hostile_numbers_change_nothing_and_do_not_panic() {
        for s in [
            stroke(&[(-50.0, -50.0)]),
            stroke(&[(1e300, 1e300)]),
            stroke(&[(f64::NAN, 1.0)]),
            stroke(&[(f64::INFINITY, f64::NEG_INFINITY), (3.0, 3.0)]),
            stroke(&[(0.0, 0.0), (1e300, 1e300)]),
            PaintStroke { size: f64::NAN, ..stroke(&[(3.0, 3.0)]) },
            PaintStroke { size: 1e300, ..stroke(&[(3.0, 3.0)]) },
            PaintStroke { hardness: f64::NAN, opacity: f64::NAN, ..stroke(&[(3.0, 3.0)]) },
            stroke(&[]),
        ] {
            let _ = bake_one(&grey(), &[&s]);
        }
        assert_eq!(bake_one(&grey(), &[&stroke(&[(-50.0, -50.0)])]), grey());
    }

    #[test]
    fn a_buffer_of_the_wrong_length_is_refused() {
        assert!(bake(&[0; 7], W, H, (1.0, 1.0), []).is_none());
    }

    #[test]
    fn ids_count_up_and_sort_in_stroke_order() {
        let mut paint = Paint::new();
        for tag in ["a", "b", "c"] {
            let key = next_stroke_id(&paint, tag).unwrap();
            paint.insert(key, stroke(&[(1.0, 1.0)]));
        }
        let keys: Vec<&str> = paint.keys().map(StrokeId::as_str).collect();
        assert_eq!(keys, ["000001-a", "000002-b", "000003-c"]);
        assert_eq!(paint_points(&paint), 3);
    }

    #[test]
    fn stroke_json_is_compact_and_round_trips() {
        let s = stroke(&[(1.0, 2.0), (3.0, 4.0)]);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("mode") && !json.contains("pressures"), "{json}");
        assert_eq!(serde_json::from_str::<PaintStroke>(&json).unwrap(), s);
        assert!(serde_json::from_str::<PaintStroke>(&json.replace("\"size\"", "\"extra\":1,\"size\"")).is_err());
    }

    #[test]
    fn painting_the_later_strokes_over_an_earlier_bake_equals_painting_all_at_once() {
        let first = stroke(&[(3.0, 5.0), (15.0, 5.0)]);
        let wipe = PaintStroke { mode: PaintMode::Erase, size: 6.0, ..stroke(&[(8.0, 5.0)]) };
        let last = PaintStroke { opacity: 0.5, ..stroke(&[(10.0, 2.0), (10.0, 9.0)]) };
        let original = grey();
        let all = bake_one(&original, &[&first, &wipe, &last]);
        let head = bake_one(&original, &[&first]);
        let tail = bake_over(&head, &original, W, H, (1.0, 1.0), [&wipe, &last]).unwrap();
        assert_eq!(tail, all, "the erase still restores the original under the stroke before it");
        assert!(bake_over(&head[4..], &original, W, H, (1.0, 1.0), [&last]).is_none(), "a base of the wrong size");
    }
}
