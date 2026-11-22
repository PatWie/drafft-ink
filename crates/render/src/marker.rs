//! The highlight band: a wide, see-through marker stroke drawn behind an element.
//!
//! The band is encoded first, so it lies under the element's fill and stroke and above whatever the element
//! covers. It is one layer per marker colour, drawn opaque inside and faded as a whole, so two parts of a band that
//! overlap do not get darker. The ends are flat and slightly slanted, never round.
//!
//! * Rectangle, diamond, ellipse, line, arrow, frame-less outlines: the band follows the strokes the sketch
//!   generator made (arrow heads and entity-relationship markers included), `max(3 x stroke width, 10)` wide and
//!   centred on them. Inside a filled shape the fill covers the inner half.
//! * Open lines, arrows and freehand strokes get a slanted cut at the ends that have no head.
//! * Freehand: the band follows the points under the stroke.
//! * Image: the band follows the outline of the picture, like a rectangle.
//! * Text: one band behind each laid-out line, over the lower 85% of the line box, with ends that lean a little
//!   differently on each side. The lean comes from the element's seed, so every peer and every export draws the same
//!   band. Which characters get one is [`highlighted_ranges`].
//! * Math: one band behind the formula, drawn like a line of text.

use std::ops::Range;

use drafftink_scene::{
    Element, ElementKind, Head, LinearData, MarkerColor, MathData, TEXT_END_PAD, TEXT_END_WOBBLE, TextData, band_width,
    text_band_overhang,
};
use drafftink_sketch::fit_head_size;
use drafftink_typeset::Typesetter;
use kurbo::{Affine, BezPath, Cap, Join, Point, Rect, RoundedRect, Shape, Stroke, Vec2};
use peniko::{Fill, Mix};

use crate::painter::{ElementPainter, Ink, cardinality_path};
use crate::spec::{head_size, sketch_input, text_spec};

/// Share of a text line box the band leaves free at the top.
const TEXT_BAND_TOP: f64 = 0.15;
/// How far a slanted end sticks out, as a share of the band width.
const CAP_SLANT: f64 = 0.25;
const END_WOBBLE: f64 = TEXT_END_WOBBLE;
const END_PAD: f64 = TEXT_END_PAD;

/// A mitre longer than this many half-widths becomes a bevel: a right angle (1.41) stays sharp, a thin corner does not
/// grow a spike.
const MITRE_LIMIT: f64 = 1.5;

/// How far the band of `e` reaches past the paths the element draws, in local units. Stroked bands reach at most
/// `MITRE_LIMIT` half-widths (a slanted end stays inside that); the bands of text and math reach past the characters
/// by `END_PAD + END_WOBBLE` line heights.
pub(crate) fn reach(e: &Element) -> f64 {
    match (&e.kind, e.style.highlight) {
        (ElementKind::Frame(_), _) => 0.0,
        (ElementKind::Text(t), _) => {
            let lines = if t.font.line_height.is_finite() { t.font.line_height.clamp(1.0, 3.0) } else { 1.0 };
            // A line box can be taller than the line height says; 1.5 covers the fonts that ship.
            text_band_overhang(lines * if t.font.size.is_finite() { t.font.size.max(0.0) } else { 0.0 } * 1.5)
        }
        (ElementKind::Math(_), _) => text_band_overhang(e.transform.h),
        (_, None) => 0.0,
        _ => stroke_reach(band_width(e.style.stroke_width)),
    }
}

/// How far a stroked band of `width` reaches past its path.
fn stroke_reach(width: f64) -> f64 {
    width / 2.0 * MITRE_LIMIT + 1.0
}

/// The characters of `t` that show a highlight, as byte ranges with their colour, in text order. A character's
/// highlight is its run's when the run has one (a colour, or none), and the text's own `element` highlight when it
/// has not (`SpanHighlight::resolve`). Neighbours with the same colour join into one range.
pub(crate) fn highlighted_ranges(t: &TextData, element: Option<MarkerColor>) -> Vec<(MarkerColor, Range<usize>)> {
    let mut out: Vec<(MarkerColor, Range<usize>)> = Vec::new();
    let styles = t.char_styles();
    for ((at, c), style) in t.content.char_indices().zip(styles) {
        let Some(color) = style.highlight.resolve(element) else { continue };
        let end = at + c.len_utf8();
        match out.last_mut() {
            Some((last, range)) if *last == color && range.end == at => range.end = end,
            _ => out.push((color, at..end)),
        }
    }
    out
}

/// A number in -1..1 from `seed` and two counters. Splitmix64, so the band is the same on every machine.
fn wobble(seed: u32, a: u32, b: u32) -> f64 {
    let mut z = (u64::from(seed) << 32 | u64::from(a)).wrapping_add(u64::from(b).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// A marker stroke over the lower part of `r`: a quadrilateral whose ends lean a little each, so no two look alike.
/// `salt` tells the bands of one element apart.
fn marker_quad(r: Rect, seed: u32, salt: u32) -> BezPath {
    let h = r.height();
    let (top, bottom) = (r.y0 + h * TEXT_BAND_TOP, r.y1);
    let (x0, x1) = (r.x0 - h * END_PAD, r.x1 + h * END_PAD);
    let w = |k: u32| wobble(seed, salt, k) * h * END_WOBBLE;
    let mut p = BezPath::new();
    p.move_to((x0 + w(0), top));
    p.line_to((x1 + w(1), top));
    p.line_to((x1 + w(2), bottom));
    p.line_to((x0 + w(3), bottom));
    p.close_path();
    p
}

/// The slanted cut at the end of an open stroke: a triangle that completes the flat end of the stroked path (which
/// stops at `end`) to a cut that leans. `outward` is the unit direction of the stroke at the end, pointing away.
fn slanted_end(end: Point, outward: Vec2, width: f64, lean: f64) -> BezPath {
    let normal = Vec2::new(-outward.y, outward.x) * (width / 2.0);
    let mut p = BezPath::new();
    p.move_to(end + normal);
    p.line_to(end - normal);
    p.line_to(end + normal * lean.signum() + outward * (width * CAP_SLANT));
    p.close_path();
    p
}

/// The unit direction from `from` to `to`, `None` when they coincide.
fn direction(from: Point, to: Point) -> Option<Vec2> {
    let d = to - from;
    let len = d.hypot();
    (len > 1e-9 && len.is_finite()).then(|| d / len)
}

/// The ends of an open polyline that get a slanted cut: its first and last point with the way out, leaving out an end
/// that `skip` names (an arrow head covers it).
fn open_ends(points: &[Point], skip_start: bool, skip_end: bool) -> Vec<(Point, Vec2)> {
    let mut out = Vec::with_capacity(2);
    let finite: Vec<Point> = points.iter().copied().filter(|p| p.x.is_finite() && p.y.is_finite()).collect();
    if let (false, Some(&a)) = (skip_start, finite.first())
        && let Some(&b) = finite.iter().find(|p| **p != a)
    {
        out.extend(direction(b, a).map(|d| (a, d)));
    }
    if let (false, Some(&z)) = (skip_end, finite.last())
        && let Some(&y) = finite.iter().rev().find(|p| **p != z)
    {
        out.extend(direction(y, z).map(|d| (z, d)));
    }
    out
}

/// One layer of one marker colour: `paths` stroked at `width` and `fills` filled, drawn opaque and faded together.
struct Band {
    color: MarkerColor,
    width: f64,
    strokes: Vec<BezPath>,
    fills: Vec<BezPath>,
}

impl Band {
    fn new(color: MarkerColor, width: f64) -> Self {
        Self { color, width, strokes: Vec::new(), fills: Vec::new() }
    }

    fn area(&self) -> Option<Rect> {
        let boxes = self
            .strokes
            .iter()
            .map(|p| p.control_box().inflate(stroke_reach(self.width), stroke_reach(self.width)))
            .chain(self.fills.iter().map(|p| p.control_box().inflate(1.0, 1.0)));
        boxes
            .reduce(|a, b| a.union(b))
            .filter(|r| r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite())
    }

    /// Draws the band into `ink`.
    fn paint(self, ink: &mut Ink) {
        let Some(area) = self.area() else { return };
        let (rgb, alpha) = ink.marker(self.color);
        ink.scene.push_layer(Fill::NonZero, Mix::Normal, alpha, Affine::IDENTITY, &area);
        let stroke = Stroke::new(self.width).with_caps(Cap::Butt).with_join(Join::Miter).with_miter_limit(MITRE_LIMIT);
        for p in &self.strokes {
            ink.scene.stroke(&stroke, Affine::IDENTITY, rgb, None, p);
        }
        for p in &self.fills {
            ink.scene.fill(Fill::NonZero, Affine::IDENTITY, rgb, None, p);
        }
        ink.scene.pop_layer();
        ink.grow(area);
    }
}

/// The path through `points`, closed or not.
fn polyline(points: &[Point], closed: bool) -> BezPath {
    let mut path = BezPath::new();
    let mut it = points.iter().filter(|p| p.x.is_finite() && p.y.is_finite());
    if let Some(first) = it.next() {
        path.move_to(*first);
        it.for_each(|p| path.line_to(*p));
        if closed {
            path.close_path();
        }
    }
    path
}

/// The strokes of the entity-relationship markers at the ends of an arrow, which the sketch generator does not make.
fn cardinality_paths(e: &Element, d: &LinearData) -> Vec<BezPath> {
    let pts = &d.points;
    let (Some(&first), Some(&second), Some(&last), Some(&penultimate)) =
        (pts.first(), pts.get(1), pts.last(), pts.iter().rev().nth(1))
    else {
        return Vec::new();
    };
    let asked = head_size(d, e.style.stroke_width);
    [(d.end_head, last, penultimate), (d.start_head, first, second)]
        .into_iter()
        .filter(|(head, ..)| {
            matches!(
                head,
                Head::CardinalityOne
                    | Head::CardinalityMany
                    | Head::CardinalityOneOrMany
                    | Head::CardinalityExactlyOne
                    | Head::CardinalityZeroOrOne
                    | Head::CardinalityZeroOrMany
            )
        })
        .map(|(head, tip, toward)| {
            let away = tip - toward;
            let len = away.hypot();
            let dir = if len > f64::EPSILON { away / len } else { Vec2::new(1.0, 0.0) };
            cardinality_path(head, tip, dir, fit_head_size(asked, len))
        })
        .collect()
}

impl ElementPainter {
    /// Draws the highlight band of `e`, if it has one, in the element's local frame. Call it before the element's
    /// own ink.
    pub(crate) fn paint_highlight(&mut self, ink: &mut Ink, e: &Element, typesetter: &mut Typesetter) {
        match &e.kind {
            ElementKind::Text(t) => paint_text_highlight(ink, e, t, typesetter),
            ElementKind::Math(m) => self.paint_math_highlight(ink, e, m, typesetter),
            ElementKind::Frame(_) => {}
            _ => {
                if let Some(color) = e.style.highlight {
                    self.paint_outline_highlight(ink, e, color);
                }
            }
        }
    }

    fn paint_outline_highlight(&mut self, ink: &mut Ink, e: &Element, color: MarkerColor) {
        let mut band = Band::new(color, band_width(e.style.stroke_width));
        let t = &e.transform;
        match &e.kind {
            ElementKind::Image(_) => {
                let r = Rect::new(0.0, 0.0, t.w.max(0.0), t.h.max(0.0));
                let radius = e.style.corner_radius.min(r.width().min(r.height()) / 2.0).max(0.0);
                band.strokes.push(RoundedRect::from_rect(r, radius).to_path(0.05));
            }
            ElementKind::Freehand(f) => {
                band.strokes.push(polyline(&f.points, f.closed));
                if !f.closed {
                    band.fills.extend(self.caps(&f.points, e, false, false, band.width));
                }
            }
            kind => {
                if let Some(input) = sketch_input(e) {
                    band.strokes.extend(self.sketch.get(&input).strokes.iter().map(|(p, _)| p.clone()));
                }
                if let ElementKind::Line(d) | ElementKind::Arrow(d) = kind {
                    if let ElementKind::Arrow(_) = kind {
                        band.strokes.extend(cardinality_paths(e, d));
                    }
                    if !d.closed {
                        let (skip_start, skip_end) = (d.start_head != Head::None, d.end_head != Head::None);
                        let heads = matches!(kind, ElementKind::Arrow(_));
                        band.fills.extend(self.caps(&d.points, e, heads && skip_start, heads && skip_end, band.width));
                    }
                }
            }
        }
        band.paint(ink);
    }

    /// The slanted cuts of an open stroke, the lean of each from the element's seed.
    fn caps(&self, points: &[Point], e: &Element, skip_start: bool, skip_end: bool, width: f64) -> Vec<BezPath> {
        open_ends(points, skip_start, skip_end)
            .into_iter()
            .enumerate()
            .map(|(i, (end, outward))| {
                let lean = wobble(e.style.seed, 7, u32::try_from(i).unwrap_or(0));
                slanted_end(end, outward, width, if lean == 0.0 { 1.0 } else { lean })
            })
            .collect()
    }

    fn paint_math_highlight(&mut self, ink: &mut Ink, e: &Element, _m: &MathData, _ts: &mut Typesetter) {
        let Some(color) = e.style.highlight else { return };
        let t = &e.transform;
        let (w, h) = (t.w.max(0.0), t.h.max(0.0));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let mut band = Band::new(color, 0.0);
        band.fills.push(marker_quad(Rect::new(0.0, 0.0, w, h), e.style.seed, 0));
        band.paint(ink);
    }
}

/// The bands behind the lines of a text: for each marker colour one layer, over the characters that show it.
pub(crate) fn paint_text_highlight(ink: &mut Ink, e: &Element, t: &TextData, ts: &mut Typesetter) {
    let ranges = highlighted_ranges(t, e.style.highlight);
    if t.content.is_empty() || ranges.is_empty() {
        return;
    }
    let layout = ts.text(&text_spec(t, e.style.stroke_color, e.transform.w));
    let mut layers: Vec<Band> = Vec::new();
    for (i, (color, range)) in ranges.into_iter().enumerate() {
        let salt = u32::try_from(i).unwrap_or(u32::MAX).wrapping_mul(16);
        let selection = layout.selection_from_range(range.start, range.end);
        for (j, r) in layout.selection_rects(&selection).into_iter().enumerate() {
            if r.width() <= 0.0 || r.height() <= 0.0 {
                continue;
            }
            let index = match layers.iter().position(|b| b.color == color) {
                Some(k) => k,
                None => {
                    layers.push(Band::new(color, 0.0));
                    layers.len() - 1
                }
            };
            let quad = marker_quad(r, e.style.seed, salt.wrapping_add(u32::try_from(j).unwrap_or(0)));
            if let Some(layer) = layers.get_mut(index) {
                layer.fills.push(quad);
            }
        }
    }
    layers.into_iter().for_each(|b| b.paint(ink));
}

#[cfg(test)]
mod tests {
    use drafftink_scene::{CharStyle, SpanHighlight, TextRun};

    use super::*;

    const YELLOW: MarkerColor = MarkerColor::YELLOW;

    fn green() -> MarkerColor {
        MarkerColor::from_name("green-400").unwrap()
    }

    #[test]
    fn the_width_is_three_strokes_with_a_floor_of_ten() {
        assert_eq!(band_width(2.0), 10.0);
        assert_eq!(band_width(4.0), 12.0);
        assert_eq!(band_width(8.0), 24.0);
        assert_eq!(band_width(0.0), 10.0);
        assert_eq!(band_width(f64::NAN), 10.0);
    }

    #[test]
    fn ranges_follow_the_span_over_element_rule_in_bytes() {
        let mut t = TextData::new("aé bc");
        // The text is yellow; the run over "bc" says green, the run over "é" says none.
        t.runs = vec![
            TextRun::new(1, 2, CharStyle { highlight: SpanHighlight::Off, ..Default::default() }),
            TextRun::new(3, 5, CharStyle { highlight: SpanHighlight::On(green()), ..Default::default() }),
        ];
        assert_eq!(highlighted_ranges(&t, Some(YELLOW)), vec![(YELLOW, 0..1), (YELLOW, 3..4), (green(), 4..6)]);
        assert_eq!(highlighted_ranges(&t, None), vec![(green(), 4..6)]);
        assert!(highlighted_ranges(&TextData::new("plain"), None).is_empty());
        assert_eq!(highlighted_ranges(&TextData::new("ab"), Some(YELLOW)), vec![(YELLOW, 0..2)]);
    }

    #[test]
    fn the_wobble_is_deterministic_and_bounded_and_depends_on_the_seed() {
        for k in 0..64 {
            let v = wobble(5, 1, k);
            assert!((-1.0..=1.0).contains(&v), "{v}");
            assert_eq!(v, wobble(5, 1, k));
        }
        assert_ne!(wobble(5, 1, 0), wobble(6, 1, 0));
    }

    #[test]
    fn a_marker_quad_keeps_the_lower_85_percent_with_slightly_leaning_ends() {
        let r = Rect::new(10.0, 0.0, 110.0, 20.0);
        let q = marker_quad(r, 3, 0);
        let b = q.control_box();
        assert!((b.y0 - 3.0).abs() < 1e-9 && (b.y1 - 20.0).abs() < 1e-9, "{b:?}");
        // Reaches a little past the characters but never by more than pad plus wobble.
        let slack = 20.0 * (END_PAD + END_WOBBLE) + 1e-9;
        assert!(
            b.x0 >= 10.0 - slack && b.x0 <= 10.0 + slack && b.x1 >= 110.0 - slack && b.x1 <= 110.0 + slack,
            "{b:?}"
        );
        assert_eq!(marker_quad(r, 3, 0), q, "same seed, same band");
        assert_ne!(marker_quad(r, 4, 0), q);
    }

    #[test]
    fn open_ends_skip_the_end_with_a_head() {
        let pts = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0)];
        assert_eq!(open_ends(&pts, false, false).len(), 2);
        let ends = open_ends(&pts, true, false);
        assert_eq!(ends.len(), 1);
        assert_eq!(ends[0].0, Point::new(10.0, 10.0));
        assert!((ends[0].1.y - 1.0).abs() < 1e-9, "points away from the line");
        assert!(open_ends(&[Point::new(1.0, 1.0)], false, false).is_empty());
    }
}
