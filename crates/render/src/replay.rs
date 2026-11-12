//! Replays a Vello scene into another renderer.
//!
//! The render crate encodes every frame into a `vello::Scene`, which is a list of streams meant for Vello's compute
//! shaders: path tags and coordinates, transforms, styles, draw tags and draw data, and late-bound glyph runs and
//! images. `vello_gpu` (WebGL2) and `vello_cpu` take immediate-mode calls instead. This module reads the streams back
//! in drawing order and turns each draw object into one call on a [`Canvas`]. The composer does not know about it, so
//! the scenes are the same whichever renderer draws them.
//!
//! What is replayed: solid fills (both fill rules) and strokes (width, joins, caps, miter limit; dashes were already
//! cut into subpaths when the scene was built), image fills, glyph runs, clip layers and opacity layers. The app draws
//! nothing else (no gradients, blurs or blend modes); a draw object of another kind is counted in
//! [`ReplayStats::skipped`] and left out.

use vello::kurbo::{Affine, BezPath, Join, Point, Stroke};
use vello::low_level::vello_encoding::math::f16_to_f32;
use vello::low_level::vello_encoding::{DrawBeginClip, DrawTag, Encoding, GlyphRun, Patch, PathTag, Style, Transform};
use vello::peniko::{BlendMode, Compose, Fill, ImageData, ImageSampler, Mix};
use vello::{Glyph, Scene};

/// How much of the scene a replay draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detail {
    /// Everything.
    #[default]
    Full,
    /// While the user moves the view or a shape: text is drawn as grey bars, which costs a tenth of the glyphs.
    Moving,
}

/// A solid colour with straight (not premultiplied) alpha, components 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub [f32; 4]);

impl Rgba {
    /// The packed premultiplied RGBA8 word of a `DrawColor` (red in the low byte).
    fn from_packed(word: u32) -> Self {
        let [r, g, b, a] = word.to_le_bytes().map(f32::from);
        if a <= 0.0 {
            return Self([0.0; 4]);
        }
        Self([(r / a).min(1.0), (g / a).min(1.0), (b / a).min(1.0), a / 255.0])
    }

    fn with_alpha_scaled(self, k: f32) -> Self {
        let [r, g, b, a] = self.0;
        Self([r, g, b, a * k])
    }
}

/// What fills a shape.
pub enum Brush<'a> {
    /// One colour.
    Solid(Rgba),
    /// A picture.
    Image {
        /// The pixels.
        data: &'a ImageData,
        /// How they are sampled.
        sampler: ImageSampler,
        /// Maps the picture's pixel space into the space of the shape.
        to_path: Affine,
    },
}

/// What a layer does to what is drawn inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerKind {
    /// Only keeps what is inside the shape.
    Clip,
    /// Keeps what is inside the shape and draws the group with this opacity.
    Opacity(f32),
}

/// A run of glyphs of one font, size and colour.
pub struct GlyphDraw<'a> {
    /// The font, size and transform of the run.
    pub run: &'a GlyphRun,
    /// The glyphs, in the space of the run.
    pub glyphs: &'a [Glyph],
    /// Normalised variation coordinates of the font.
    pub coords: &'a [i16],
    /// The colour.
    pub color: Rgba,
}

/// The renderer a scene is replayed into.
pub trait Canvas {
    /// Fills `path` under `transform`.
    fn fill(&mut self, transform: Affine, path: &BezPath, rule: Fill, brush: Brush<'_>);
    /// Strokes `path` under `transform`.
    fn stroke(&mut self, transform: Affine, path: &BezPath, stroke: &Stroke, brush: Brush<'_>);
    /// Starts a layer clipped to `clip`; everything drawn until the matching [`Canvas::pop_layer`] goes into it.
    fn push_layer(&mut self, transform: Affine, clip: &BezPath, kind: LayerKind);
    /// Ends the innermost layer.
    fn pop_layer(&mut self);
    /// Draws a run of glyphs.
    fn glyph_run(&mut self, draw: &GlyphDraw<'_>);
}

/// What a replay drew, for tests and the frame log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayStats {
    /// Filled shapes (not counting images and glyphs).
    pub fills: u32,
    /// Stroked shapes.
    pub strokes: u32,
    /// Shapes filled or stroked with a picture.
    pub images: u32,
    /// Glyph runs drawn as glyphs.
    pub glyph_runs: u32,
    /// Glyph runs drawn as bars ([`Detail::Moving`]).
    pub glyph_bars: u32,
    /// Layers pushed.
    pub layers: u32,
    /// Draw objects of a kind the app never makes (gradients, blurs), left out.
    pub skipped: u32,
}

/// Replay `encoding` into `canvas`. Layers left open by the scene are closed.
pub fn replay(encoding: &Encoding, canvas: &mut dyn Canvas, detail: Detail) -> ReplayStats {
    let mut stats = ReplayStats::default();
    let mut reader = PathReader::new(encoding);
    let mut path = BezPath::new();
    let mut runs = encoding.resources.glyph_runs.iter().peekable();
    let mut data_pos = 0usize;
    let mut open_layers = 0u32;
    let normal = DrawBeginClip::new(BlendMode::new(Mix::Normal, Compose::SrcOver), 1.0).blend_mode;
    for (index, &tag) in encoding.draw_tags.iter().enumerate() {
        let words = draw_words(tag);
        let word = |i: usize| encoding.draw_data.get(data_pos + i).copied();
        if let Some(run) = runs.next_if(|r| r.stream_offsets.draw_tags == index) {
            let color = word(0).map_or(Rgba([0.0, 0.0, 0.0, 1.0]), Rgba::from_packed);
            glyph_run(encoding, run, color, canvas, detail, &mut stats);
        } else if tag == DrawTag::END_CLIP {
            reader.next_path(&mut path);
            if open_layers > 0 {
                open_layers -= 1;
                canvas.pop_layer();
            }
        } else if let Some(end) = reader.next_path(&mut path) {
            match tag {
                DrawTag::COLOR => {
                    let brush = Brush::Solid(word(0).map_or(Rgba([0.0; 4]), Rgba::from_packed));
                    draw_shape(canvas, &end, &path, brush, &mut stats);
                }
                DrawTag::IMAGE => {
                    let patch = encoding.resources.patches.iter().find_map(|p| match p {
                        Patch::Image { image, draw_data_offset } if *draw_data_offset == data_pos => Some(image),
                        _ => None,
                    });
                    if let (Some(data), Some(sample_alpha), Some(to_path)) =
                        (patch, word(2), end.path_transform.inverse_checked().map(|i| i * end.brush_transform))
                    {
                        let brush = Brush::Image { data, sampler: sampler_of(sample_alpha), to_path };
                        draw_shape(canvas, &end, &path, brush, &mut stats);
                    } else {
                        stats.skipped += 1;
                    }
                }
                DrawTag::BEGIN_CLIP => {
                    let mode = word(0).unwrap_or(DrawBeginClip::CLIP_BLEND_MODE);
                    let alpha = word(1).map_or(1.0, f32::from_bits);
                    let kind = if mode == normal { LayerKind::Opacity(alpha) } else { LayerKind::Clip };
                    canvas.push_layer(end.path_transform, &path, kind);
                    open_layers += 1;
                    stats.layers += 1;
                }
                _ => stats.skipped += 1,
            }
        } else {
            break;
        }
        data_pos += words;
    }
    for _ in 0..open_layers {
        canvas.pop_layer();
    }
    stats
}

/// The pictures a scene draws.
pub fn pictures(scene: &Scene) -> impl Iterator<Item = &ImageData> {
    scene.encoding().resources.patches.iter().filter_map(|p| match p {
        Patch::Image { image, .. } => Some(image),
        _ => None,
    })
}

/// Words of draw data a draw tag owns (bits 2 to 4 of the tag).
fn draw_words(tag: DrawTag) -> usize {
    ((tag.0 >> 2) & 0x7) as usize
}

fn draw_shape(canvas: &mut dyn Canvas, end: &PathEnd, path: &BezPath, brush: Brush<'_>, stats: &mut ReplayStats) {
    let counter = match (&brush, end.style.flags_and_miter_limit & Style::FLAGS_STYLE_BIT != 0) {
        (Brush::Image { .. }, _) => &mut stats.images,
        (Brush::Solid(_), true) => &mut stats.strokes,
        (Brush::Solid(_), false) => &mut stats.fills,
    };
    *counter += 1;
    if end.style.flags_and_miter_limit & Style::FLAGS_STYLE_BIT != 0 {
        canvas.stroke(end.path_transform, path, &stroke_of(&end.style), brush);
    } else {
        let rule =
            if end.style.flags_and_miter_limit & Style::FLAGS_FILL_BIT != 0 { Fill::EvenOdd } else { Fill::NonZero };
        canvas.fill(end.path_transform, path, rule, brush);
    }
}

fn sampler_of(sample_alpha: u32) -> ImageSampler {
    use vello::peniko::{Extend, ImageQuality};
    let extend = |bits: u32| match bits & 3 {
        1 => Extend::Repeat,
        2 => Extend::Reflect,
        _ => Extend::Pad,
    };
    let quality = match (sample_alpha >> 12) & 3 {
        0 => ImageQuality::Low,
        1 => ImageQuality::Medium,
        _ => ImageQuality::High,
    };
    ImageSampler::new()
        .with_x_extend(extend(sample_alpha >> 10))
        .with_y_extend(extend(sample_alpha >> 8))
        .with_quality(quality)
        .with_alpha((sample_alpha & 0xff) as f32 / 255.0)
}

/// The stroke a style describes. A zero-width style never reaches a scene (Vello refuses to encode it).
fn stroke_of(style: &Style) -> Stroke {
    use vello::kurbo::Cap;
    let f = style.flags_and_miter_limit;
    let join = match f & Style::FLAGS_JOIN_MASK {
        Style::FLAGS_JOIN_BITS_MITER => Join::Miter,
        Style::FLAGS_JOIN_BITS_ROUND => Join::Round,
        _ => Join::Bevel,
    };
    let start = match f & Style::FLAGS_START_CAP_MASK {
        Style::FLAGS_START_CAP_BITS_SQUARE => Cap::Square,
        Style::FLAGS_START_CAP_BITS_ROUND => Cap::Round,
        _ => Cap::Butt,
    };
    let end = match f & Style::FLAGS_END_CAP_MASK {
        Style::FLAGS_END_CAP_BITS_SQUARE => Cap::Square,
        Style::FLAGS_END_CAP_BITS_ROUND => Cap::Round,
        _ => Cap::Butt,
    };
    // The low 16 bits are the miter limit as a half float.
    let miter = f16_to_f32((f & Style::MITER_LIMIT_MASK) as u16);
    Stroke::new(f64::from(style.line_width))
        .with_join(join)
        .with_start_cap(start)
        .with_end_cap(end)
        .with_miter_limit(f64::from(miter))
}

fn glyph_run(
    encoding: &Encoding,
    run: &GlyphRun,
    color: Rgba,
    canvas: &mut dyn Canvas,
    detail: Detail,
    stats: &mut ReplayStats,
) {
    let res = &encoding.resources;
    let (Some(glyphs), Some(coords)) =
        (res.glyphs.get(run.glyphs.clone()), res.normalized_coords.get(run.normalized_coords.clone()))
    else {
        stats.skipped += 1;
        return;
    };
    match detail {
        Detail::Full => {
            canvas.glyph_run(&GlyphDraw { run, glyphs, coords, color });
            stats.glyph_runs += 1;
        }
        Detail::Moving => {
            if let Some(bar) = greek_bar(run, glyphs) {
                canvas.fill(run.transform.to_kurbo(), &bar, Fill::NonZero, Brush::Solid(color.with_alpha_scaled(0.35)));
                stats.glyph_bars += 1;
            }
        }
    }
}

/// A bar where a run of glyphs is: from the first to the last glyph, at the height of lower-case letters.
fn greek_bar(run: &GlyphRun, glyphs: &[Glyph]) -> Option<BezPath> {
    let (first, last) = (glyphs.first()?, glyphs.last()?);
    let size = f64::from(run.font_size);
    let (x0, x1) = (f64::from(first.x.min(last.x)), f64::from(first.x.max(last.x)) + size * 0.5);
    let base = f64::from(first.y);
    let mut bar = BezPath::new();
    bar.move_to(Point::new(x0, base - size * 0.5));
    bar.line_to(Point::new(x1, base - size * 0.5));
    bar.line_to(Point::new(x1, base));
    bar.line_to(Point::new(x0, base));
    bar.close_path();
    Some(bar)
}

/// How a path ended: the transforms in force, and the style.
struct PathEnd {
    /// The transform of the path's segments.
    path_transform: Affine,
    /// The transform at the end marker. It differs from the path's when the draw has a brush transform.
    brush_transform: Affine,
    style: Style,
}

/// Reads path tags and coordinates, one path at a time.
struct PathReader<'a> {
    enc: &'a Encoding,
    tag: usize,
    word: usize,
    transform_index: usize,
    style_index: usize,
    transform: Affine,
    style: Style,
}

impl<'a> PathReader<'a> {
    fn new(enc: &'a Encoding) -> Self {
        Self {
            enc,
            tag: 0,
            word: 0,
            transform_index: 0,
            style_index: 0,
            transform: Affine::IDENTITY,
            style: Style::from_fill(Fill::NonZero),
        }
    }

    fn float(&mut self) -> Option<f64> {
        let bits = *self.enc.path_data.get(self.word)?;
        self.word += 1;
        Some(f64::from(f32::from_bits(bits)))
    }

    fn point(&mut self) -> Option<Point> {
        Some(Point::new(self.float()?, self.float()?))
    }

    /// Reads tags up to and including the next path marker into `path` (cleared first). `None` when the streams end
    /// before a marker or hold a segment kind the encoder never writes.
    fn next_path(&mut self, path: &mut BezPath) -> Option<PathEnd> {
        path.truncate(0);
        let mut path_transform = self.transform;
        let mut need_move = true;
        loop {
            let tag = *self.enc.path_tags.get(self.tag)?;
            self.tag += 1;
            if tag == PathTag::TRANSFORM {
                if let Some(t) = self.enc.transforms.get(self.transform_index) {
                    self.transform = Transform::to_kurbo(t);
                }
                self.transform_index += 1;
            } else if tag == PathTag::STYLE {
                if let Some(s) = self.enc.styles.get(self.style_index) {
                    self.style = *s;
                }
                self.style_index += 1;
            } else if tag == PathTag::PATH {
                return Some(PathEnd { path_transform, brush_transform: self.transform, style: self.style });
            } else if tag.is_path_segment() {
                if !tag.is_f32() {
                    return None;
                }
                if need_move {
                    path.move_to(self.point()?);
                }
                path_transform = self.transform;
                // The next segment starts a new subpath after the one that just ended.
                need_move = self.segment(tag, path)?;
            }
        }
    }

    /// One segment. In a stroke the last segment of a subpath is the encoder's cap marker: its points only tell the GPU
    /// stroker about the start tangent, and its kind says whether the subpath is closed (a line) or open (a quad).
    /// Returns whether the segment ended its subpath.
    fn segment(&mut self, tag: PathTag, path: &mut BezPath) -> Option<bool> {
        let is_stroke = self.style.flags_and_miter_limit & Style::FLAGS_STYLE_BIT != 0;
        let end = tag.is_subpath_end();
        let kind = tag.path_segment_type().0;
        let (p1, p2, p3) = match kind {
            1 => (self.point()?, None, None),
            2 => (self.point()?, Some(self.point()?), None),
            3 => (self.point()?, Some(self.point()?), Some(self.point()?)),
            _ => return None,
        };
        if is_stroke && end {
            if kind == 1 {
                path.close_path();
            }
            return Some(true);
        }
        match (p2, p3) {
            (None, _) => path.line_to(p1),
            (Some(p2), None) => path.quad_to(p1, p2),
            (Some(p2), Some(p3)) => path.curve_to(p1, p2, p3),
        }
        if end {
            // A fill's subpaths are closed.
            path.close_path();
        }
        Some(end)
    }
}

/// `Affine::inverse` returns NaNs for a singular matrix; this returns `None`.
trait InverseChecked {
    fn inverse_checked(&self) -> Option<Affine>;
}

impl InverseChecked for Affine {
    fn inverse_checked(&self) -> Option<Affine> {
        let inverse = self.inverse();
        inverse.as_coeffs().iter().all(|c| c.is_finite()).then_some(inverse)
    }
}

#[cfg(test)]
mod tests {
    use vello::Scene;
    use vello::kurbo::{Cap, Circle, PathEl, Rect, RoundedRect, Shape};
    use vello::peniko::{Blob, Color, FontData, ImageBrush, ImageFormat};

    use super::*;

    /// One call on the canvas, with everything it was given.
    #[derive(Debug, PartialEq)]
    enum Op {
        Fill { transform: Affine, path: Vec<PathEl>, rule: Fill, color: Option<[f32; 4]> },
        Stroke { transform: Affine, path: Vec<PathEl>, stroke: Stroke, color: Option<[f32; 4]> },
        Image { transform: Affine, to_path: Affine, size: (u32, u32), alpha: f32 },
        Push { transform: Affine, kind: LayerKind },
        Pop,
        Glyphs { count: usize, size: f32, color: [f32; 4] },
    }

    #[derive(Default)]
    struct Recorder(Vec<Op>);

    impl Canvas for Recorder {
        /// Fills `path` under `transform`.
        fn fill(&mut self, transform: Affine, path: &BezPath, rule: Fill, brush: Brush<'_>) {
            self.0.push(match brush {
                Brush::Solid(c) => Op::Fill { transform, path: path.elements().to_vec(), rule, color: Some(c.0) },
                Brush::Image { data, sampler, to_path } => {
                    Op::Image { transform, to_path, size: (data.width, data.height), alpha: sampler.alpha }
                }
            });
        }

        /// Strokes `path` under `transform`.
        fn stroke(&mut self, transform: Affine, path: &BezPath, stroke: &Stroke, brush: Brush<'_>) {
            let color = match brush {
                Brush::Solid(c) => Some(c.0),
                Brush::Image { .. } => None,
            };
            self.0.push(Op::Stroke { transform, path: path.elements().to_vec(), stroke: stroke.clone(), color });
        }

        /// Starts a layer clipped to `clip`; everything drawn until the matching [`Canvas::pop_layer`] goes into it.
        fn push_layer(&mut self, transform: Affine, _clip: &BezPath, kind: LayerKind) {
            self.0.push(Op::Push { transform, kind });
        }

        fn pop_layer(&mut self) {
            self.0.push(Op::Pop);
        }

        /// Draws a run of glyphs.
        fn glyph_run(&mut self, draw: &GlyphDraw<'_>) {
            self.0.push(Op::Glyphs { count: draw.glyphs.len(), size: draw.run.font_size, color: draw.color.0 });
        }
    }

    fn replayed(scene: &Scene, detail: Detail) -> (Vec<Op>, ReplayStats) {
        let mut rec = Recorder::default();
        let stats = replay(scene.encoding(), &mut rec, detail);
        (rec.0, stats)
    }

    const RED: Color = Color::from_rgba8(255, 0, 0, 255);

    /// A rectangle as the replay gives it back: the line to the start is explicit, then the close.
    fn rect_elements(r: Rect) -> Vec<PathEl> {
        let mut path = BezPath::new();
        path.move_to((r.x0, r.y0));
        path.line_to((r.x1, r.y0));
        path.line_to((r.x1, r.y1));
        path.line_to((r.x0, r.y1));
        path.line_to((r.x0, r.y0));
        path.close_path();
        path.elements().to_vec()
    }

    #[test]
    fn a_filled_rectangle_comes_back_with_its_colour_transform_and_rule() {
        let mut scene = Scene::new();
        let t = Affine::translate((10.0, 20.0));
        scene.fill(Fill::NonZero, t, RED, None, &Rect::new(0.0, 0.0, 30.0, 40.0));
        scene.fill(
            Fill::EvenOdd,
            Affine::IDENTITY,
            Color::from_rgba8(0, 0, 255, 255),
            None,
            &Circle::new((5.0, 5.0), 4.0),
        );
        let (ops, stats) = replayed(&scene, Detail::Full);
        assert_eq!(stats, ReplayStats { fills: 2, ..ReplayStats::default() });
        let [Op::Fill { transform, path, rule, color }, Op::Fill { rule: second_rule, color: second_color, .. }] =
            &ops[..]
        else {
            panic!("{ops:?}")
        };
        assert_eq!(*transform, t);
        assert_eq!(*rule, Fill::NonZero);
        assert_eq!(*color, Some([1.0, 0.0, 0.0, 1.0]));
        assert!(path.starts_with(&[PathEl::MoveTo((0.0, 0.0).into())]), "{path:?}");
        // The encoder writes the line back to the start itself, so a rectangle has four.
        assert_eq!(path.iter().filter(|e| matches!(e, PathEl::LineTo(_))).count(), 4);
        assert_eq!(path.last(), Some(&PathEl::ClosePath));
        assert_eq!(*second_rule, Fill::EvenOdd);
        assert_eq!(*second_color, Some([0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn a_translucent_colour_is_unpremultiplied_again() {
        let mut scene = Scene::new();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgba8(200, 100, 50, 128),
            None,
            &Rect::new(0.0, 0.0, 4.0, 4.0),
        );
        let (ops, _) = replayed(&scene, Detail::Full);
        let Some(Op::Fill { color: Some([r, g, b, a]), .. }) = ops.first() else { panic!("{ops:?}") };
        // Premultiplying to 8 bits and back loses a little; the colour stays within two steps of the original.
        for (got, want) in [(*r, 200.0), (*g, 100.0), (*b, 50.0), (*a, 128.0)] {
            assert!((got * 255.0 - want).abs() <= 2.5, "{got} against {want}");
        }
    }

    #[test]
    fn an_open_stroke_drops_the_cap_marker_and_keeps_the_style() {
        let mut scene = Scene::new();
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((50.0, 0.0));
        line.line_to((50.0, 30.0));
        let style = Stroke::new(3.0).with_join(Join::Round).with_caps(Cap::Round).with_miter_limit(2.0);
        scene.stroke(&style, Affine::scale(2.0), RED, None, &line);
        let (ops, stats) = replayed(&scene, Detail::Full);
        assert_eq!(stats.strokes, 1);
        let [Op::Stroke { transform, path, stroke, .. }] = &ops[..] else { panic!("{ops:?}") };
        assert_eq!(*transform, Affine::scale(2.0));
        assert_eq!(path.as_slice(), line.elements(), "the marker segment is gone");
        assert_eq!(
            (stroke.width, stroke.join, stroke.start_cap, stroke.end_cap),
            (3.0, Join::Round, Cap::Round, Cap::Round)
        );
        assert!((stroke.miter_limit - 2.0).abs() < 1e-3);
    }

    #[test]
    fn a_closed_stroke_is_closed() {
        let mut scene = Scene::new();
        scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 20.0, 10.0));
        let (ops, _) = replayed(&scene, Detail::Full);
        let [Op::Stroke { path, .. }] = &ops[..] else { panic!("{ops:?}") };
        assert_eq!(path.last(), Some(&PathEl::ClosePath), "{path:?}");
        assert_eq!(path.iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count(), 1);
    }

    #[test]
    fn a_dashed_stroke_arrives_as_separate_subpaths() {
        let mut scene = Scene::new();
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((100.0, 0.0));
        let style = Stroke::new(2.0).with_dashes(0.0, [10.0, 10.0]);
        scene.stroke(&style, Affine::IDENTITY, RED, None, &line);
        let (ops, _) = replayed(&scene, Detail::Full);
        let [Op::Stroke { path, stroke, .. }] = &ops[..] else { panic!("{ops:?}") };
        assert_eq!(path.iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count(), 5, "{path:?}");
        assert!(stroke.dash_pattern.is_empty(), "the dashes are already cut");
    }

    #[test]
    fn curves_survive() {
        let mut scene = Scene::new();
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &RoundedRect::new(0.0, 0.0, 40.0, 20.0, 6.0));
        let (ops, _) = replayed(&scene, Detail::Full);
        let [Op::Fill { path, .. }] = &ops[..] else { panic!("{ops:?}") };
        assert!(path.iter().any(|e| matches!(e, PathEl::CurveTo(..) | PathEl::QuadTo(..))), "{path:?}");
    }

    #[test]
    fn layers_nest_and_close() {
        let mut scene = Scene::new();
        let clip = Rect::new(0.0, 0.0, 100.0, 100.0);
        scene.push_clip_layer(Fill::NonZero, Affine::translate((5.0, 5.0)), &clip);
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 10.0, 10.0));
        scene.push_layer(Fill::NonZero, BlendMode::new(Mix::Normal, Compose::SrcOver), 0.25, Affine::IDENTITY, &clip);
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 10.0, 10.0));
        scene.pop_layer();
        scene.pop_layer();
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 10.0, 10.0));
        let (ops, stats) = replayed(&scene, Detail::Full);
        let kinds: Vec<&str> = ops
            .iter()
            .map(|o| match o {
                Op::Push { kind: LayerKind::Clip, .. } => "clip",
                Op::Push { kind: LayerKind::Opacity(_), .. } => "opacity",
                Op::Pop => "pop",
                Op::Fill { .. } => "fill",
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(kinds, ["clip", "fill", "opacity", "fill", "pop", "pop", "fill"]);
        assert_eq!(stats.layers, 2);
        assert!(matches!(&ops[0], Op::Push { transform, .. } if *transform == Affine::translate((5.0, 5.0))));
        assert!(matches!(&ops[2], Op::Push { kind: LayerKind::Opacity(a), .. } if (*a - 0.25).abs() < 1e-6));
    }

    #[test]
    fn a_layer_the_scene_leaves_open_is_closed() {
        let mut scene = Scene::new();
        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::new(0.0, 0.0, 9.0, 9.0));
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 3.0, 3.0));
        let (ops, _) = replayed(&scene, Detail::Full);
        assert!(matches!(ops.last(), Some(Op::Pop)), "{ops:?}");
    }

    #[test]
    fn appending_a_scene_with_a_transform_moves_its_shapes() {
        let mut part = Scene::new();
        part.fill(Fill::NonZero, Affine::translate((1.0, 2.0)), RED, None, &Rect::new(0.0, 0.0, 5.0, 5.0));
        part.stroke(&Stroke::new(1.0), Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 5.0, 5.0));
        let mut frame = Scene::new();
        frame.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 1.0, 1.0));
        let view = Affine::translate((100.0, 0.0)) * Affine::scale(2.0);
        frame.append(&part, Some(view));
        frame.append(&part, Some(Affine::translate((0.0, 50.0))));
        let (ops, stats) = replayed(&frame, Detail::Full);
        assert_eq!((stats.fills, stats.strokes), (3, 2));
        let transforms: Vec<Affine> = ops
            .iter()
            .map(|o| match o {
                Op::Fill { transform, .. } | Op::Stroke { transform, .. } => *transform,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(transforms[0], Affine::IDENTITY);
        assert_eq!(transforms[1], view * Affine::translate((1.0, 2.0)));
        assert_eq!(transforms[2], view);
        assert_eq!(transforms[3], Affine::translate((0.0, 50.0)) * Affine::translate((1.0, 2.0)));
        assert_eq!(transforms[4], Affine::translate((0.0, 50.0)));
    }

    #[test]
    fn a_picture_keeps_its_brush_transform_relative_to_the_shape() {
        let image = ImageData {
            data: Blob::from(vec![255u8; 4 * 2 * 2]),
            format: ImageFormat::Rgba8,
            alpha_type: vello::peniko::ImageAlphaType::Alpha,
            width: 2,
            height: 2,
        };
        let brush = ImageBrush::new(image).with_alpha(0.5);
        let to_box = Affine::scale(10.0);
        let mut part = Scene::new();
        part.fill(Fill::NonZero, Affine::IDENTITY, &brush, Some(to_box), &Rect::new(0.0, 0.0, 20.0, 20.0));
        let mut frame = Scene::new();
        let place = Affine::translate((7.0, 9.0)) * Affine::rotate(0.5);
        frame.append(&part, Some(place));
        let (ops, stats) = replayed(&frame, Detail::Full);
        assert_eq!(stats.images, 1);
        let [Op::Image { transform, to_path, size, alpha }] = &ops[..] else { panic!("{ops:?}") };
        let coeffs = to_path.as_coeffs();
        for (got, want) in coeffs.iter().zip(to_box.as_coeffs()) {
            assert!((got - want).abs() < 1e-4, "{coeffs:?}");
        }
        // The shape's own transform is exactly the placement (up to the f32 the stream holds).
        for (got, want) in transform.as_coeffs().iter().zip(place.as_coeffs()) {
            assert!((got - want).abs() < 1e-6, "{transform:?}");
        }
        assert_eq!(*size, (2, 2));
        assert!((alpha - 0.5).abs() < 0.01);
    }

    /// Noto Sans Regular, from the typesetter that owns the font files.
    fn noto() -> FontData {
        let bytes = drafftink_typeset::font_file(
            &drafftink_typeset::FontFamily::NotoSans,
            drafftink_typeset::FontWeight::Regular,
        )
        .expect("the dev-dependency on drafftink-typeset builds Noto Sans in");
        FontData::new(Blob::from(bytes.to_vec()), 0)
    }

    fn glyphs(n: u32) -> impl Iterator<Item = Glyph> + Clone {
        (0..n).map(|i| Glyph { id: 36 + i, x: i as f32 * 9.0, y: 0.0 })
    }

    #[test]
    fn a_glyph_run_arrives_in_order_with_its_colour_and_size() {
        let mut scene = Scene::new();
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 1.0, 1.0));
        scene
            .draw_glyphs(&noto())
            .font_size(18.0)
            .transform(Affine::translate((3.0, 4.0)))
            .brush(Color::from_rgba8(0, 128, 0, 255))
            .draw(Fill::NonZero, glyphs(5));
        scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 1.0, 1.0));
        let (ops, stats) = replayed(&scene, Detail::Full);
        assert_eq!((stats.fills, stats.glyph_runs, stats.strokes), (1, 1, 1));
        let kinds: Vec<&str> = ops
            .iter()
            .map(|o| match o {
                Op::Fill { .. } => "fill",
                Op::Glyphs { .. } => "glyphs",
                Op::Stroke { .. } => "stroke",
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(kinds, ["fill", "glyphs", "stroke"]);
        let Op::Glyphs { count, size, color } = &ops[1] else { panic!() };
        assert_eq!((*count, *size), (5, 18.0));
        assert!((color[1] - 128.0 / 255.0).abs() < 0.01 && color[0] == 0.0 && color[3] == 1.0, "{color:?}");
    }

    #[test]
    fn several_glyph_runs_and_shapes_keep_their_order_after_an_append() {
        let mut part = Scene::new();
        part.draw_glyphs(&noto()).font_size(10.0).draw(Fill::NonZero, glyphs(2));
        part.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 1.0, 1.0));
        part.draw_glyphs(&noto()).font_size(12.0).draw(Fill::NonZero, glyphs(3));
        let mut frame = Scene::new();
        frame.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 1.0, 1.0));
        frame.append(&part, Some(Affine::translate((5.0, 5.0))));
        frame.append(&part, None);
        let (ops, _) = replayed(&frame, Detail::Full);
        let seen: Vec<String> = ops
            .iter()
            .map(|o| match o {
                Op::Fill { .. } => "fill".to_owned(),
                Op::Glyphs { count, size, .. } => format!("g{count}@{size}"),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(seen, ["fill", "g2@10", "fill", "g3@12", "g2@10", "fill", "g3@12"]);
    }

    #[test]
    fn while_moving_text_is_a_bar() {
        let mut scene = Scene::new();
        scene.draw_glyphs(&noto()).font_size(20.0).brush(RED).draw(Fill::NonZero, glyphs(4));
        let (ops, stats) = replayed(&scene, Detail::Moving);
        assert_eq!((stats.glyph_runs, stats.glyph_bars), (0, 1));
        let [Op::Fill { path, color: Some(c), .. }] = &ops[..] else { panic!("{ops:?}") };
        assert!(c[3] > 0.0 && c[3] < 0.5, "a bar is a faint version of the text colour: {c:?}");
        let bounds = BezPath::from_vec(path.clone()).bounding_box();
        assert!((bounds.width() - (27.0 + 10.0)).abs() < 1e-3 && bounds.height() > 5.0, "{bounds:?}");
    }

    #[test]
    fn an_empty_scene_replays_to_nothing() {
        let (ops, stats) = replayed(&Scene::new(), Detail::Full);
        assert!(ops.is_empty());
        assert_eq!(stats, ReplayStats::default());
    }

    #[test]
    fn zero_length_and_degenerate_shapes_do_not_derail_the_streams() {
        let mut scene = Scene::new();
        let mut dot = BezPath::new();
        dot.move_to((3.0, 3.0));
        dot.line_to((3.0, 3.0));
        scene.stroke(&Stroke::new(2.0).with_caps(Cap::Round), Affine::IDENTITY, RED, None, &dot);
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(1.0, 1.0, 9.0, 9.0));
        let (ops, _) = replayed(&scene, Detail::Full);
        let fills: Vec<&Op> = ops.iter().filter(|o| matches!(o, Op::Fill { .. })).collect();
        assert_eq!(fills.len(), 1);
        let Op::Fill { path, .. } = fills[0] else { panic!() };
        assert_eq!(*path, rect_elements(Rect::new(1.0, 1.0, 9.0, 9.0)));
    }
}
