//! [`ElementPainter`]: one element in, one [`Fragment`] out.
//!
//! The look is the old app's: the sketch generator's output drawn the way `vello_impl.rs` drew it (fills first,
//! then the strokes; hatch lines clipped to the fill outline; a 15% tint under patterns; opacity multiplied into the
//! alpha of every paint), text and math through glyph runs, images as a textured rectangle. What is new is that the
//! result is a scene in the element's local frame that can be cached and appended, and that nothing here measures
//! anything: sizes come from the layouts the typesetter cached when the content last changed.

use std::sync::Arc;

use drafftink_profile::{Counter, count};
use drafftink_scene::{
    Color, Element, ElementKind, FileId, FillPattern, FrameData, FreehandData, Head, ImageData as ImageElem, MathData,
    StrokeStyle, TextData,
};
use drafftink_sketch::{FillKind, SketchCache, SketchGeometry};
use drafftink_typeset::{MathFontSlot, Typesetter};
use kurbo::{Affine, BezPath, Cap, Circle, Join, Point, Rect, RoundedRect, Shape, Stroke, Vec2};
use peniko::{Fill, ImageBrush, Mix};
use vello::{Glyph, Scene};

use crate::fragment::Fragment;
use crate::images::{ImageCache, Lookup};
use crate::options::RenderOptions;
use crate::spec::{
    FRAME_LABEL_GAP, FRAME_LABEL_MIN_WIDTH, FRAME_LABEL_ROOM, frame_label_spec, head_size, math_spec, sketch_input,
    text_spec,
};
use crate::theme::Theme;

/// A dashed stroke that would have more dashes than this is drawn solid.
const MAX_DASHES: f64 = 20_000.0;

/// Where encoded bytes of a picture come from: the store's blob table.
pub type FileSource<'a> = &'a dyn Fn(&FileId) -> Option<Arc<[u8]>>;

/// Encodes elements into fragments. Owns the sketch cache and the decoded-picture cache; borrows the typesetter
/// (shared with the editor, so text is laid out once for measuring and drawing).
pub struct ElementPainter {
    sketch: SketchCache,
    images: ImageCache,
    sketch_misses_seen: u64,
}

impl Default for ElementPainter {
    fn default() -> Self {
        Self::new()
    }
}

impl ElementPainter {
    /// A painter with default cache sizes.
    pub fn new() -> Self {
        Self { sketch: SketchCache::new(4096), images: ImageCache::default(), sketch_misses_seen: 0 }
    }

    /// The decoded-picture cache.
    pub fn images(&self) -> &ImageCache {
        &self.images
    }

    /// The decoded-picture cache, for the host that decodes in the background.
    pub fn images_mut(&mut self) -> &mut ImageCache {
        &mut self.images
    }

    /// Sketch cache statistics.
    pub fn sketch_stats(&self) -> drafftink_sketch::CacheStats {
        self.sketch.stats()
    }

    /// Encodes `e` in its local frame.
    pub fn paint(
        &mut self,
        e: &Element,
        options: &RenderOptions,
        typesetter: &mut Typesetter,
        files: FileSource<'_>,
    ) -> Fragment {
        let _span = drafftink_profile::span("paint_element");
        let mut ink = Ink::new(options.theme, e.style.opacity);
        let mut awaiting = None;
        match &e.kind {
            ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse | ElementKind::Line(_) => {
                self.paint_sketch(&mut ink, e);
            }
            ElementKind::Arrow(d) => {
                self.paint_sketch(&mut ink, e);
                paint_cardinality(&mut ink, e, d);
            }
            ElementKind::Freehand(f) if f.highlighter => paint_highlighter(&mut ink, e, f),
            ElementKind::Freehand(_) => self.paint_sketch(&mut ink, e),
            ElementKind::Text(t) => paint_text(&mut ink, e, t, typesetter),
            ElementKind::Math(m) => paint_math(&mut ink, e, m, typesetter),
            ElementKind::Image(i) => awaiting = self.paint_image(&mut ink, e, i, files),
            ElementKind::Frame(f) => paint_frame(&mut ink, e, f, typesetter),
        }
        let misses = self.sketch.stats().misses;
        count(Counter::SketchRegens, misses.saturating_sub(self.sketch_misses_seen));
        self.sketch_misses_seen = misses;
        Fragment::new(ink.scene, ink.bounds, awaiting)
    }

    fn paint_sketch(&mut self, ink: &mut Ink, e: &Element) {
        let Some(input) = sketch_input(e) else { return };
        let geometry = self.sketch.get(&input);
        paint_geometry(ink, e, &geometry);
    }

    fn paint_image(&mut self, ink: &mut Ink, e: &Element, img: &ImageElem, files: FileSource<'_>) -> Option<FileId> {
        let t = &e.transform;
        let local = Rect::new(0.0, 0.0, t.w.max(0.0), t.h.max(0.0));
        let Some(file) = img.file_id.as_ref() else {
            paint_image_placeholder(ink, local, false);
            return None;
        };
        let lookup = self.images.lookup(file, || files(file));
        match lookup {
            Lookup::Ready(data) => {
                paint_picture(ink, img, local, data);
                paint_border(ink, e, local);
                None
            }
            Lookup::Failed => {
                paint_image_placeholder(ink, local, true);
                None
            }
            Lookup::Waiting | Lookup::Missing => {
                paint_image_placeholder(ink, local, false);
                Some(file.clone())
            }
        }
    }
}

// ----------------------------------------------------------------------------------------------
// Drawing helpers

/// A scene under construction with the colour rules of one element.
pub(crate) struct Ink {
    pub scene: Scene,
    pub bounds: Option<Rect>,
    theme: Theme,
    opacity: f64,
}

impl Ink {
    pub fn new(theme: Theme, opacity: f64) -> Self {
        let opacity = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 1.0 };
        Self { scene: Scene::new(), bounds: None, theme, opacity }
    }

    /// The colour as drawn: theme mapping, then the element opacity multiplied into the alpha (truncating, like
    /// the old app's `stroke_with_opacity`).
    pub fn color(&self, c: Color) -> peniko::Color {
        let m = self.theme.map(c);
        let a = (f64::from(m.a) * self.opacity) as u8;
        peniko::Color::from_rgba8(m.r, m.g, m.b, a)
    }

    pub fn grow(&mut self, r: Rect) {
        if r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite() {
            self.bounds = Some(self.bounds.map_or(r, |b| b.union(r)));
        }
    }

    pub fn fill(&mut self, path: &BezPath, color: peniko::Color) {
        self.scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, path);
        self.grow(path.control_box());
    }

    pub fn stroke(&mut self, path: &BezPath, width: f64, dash: Option<[f64; 2]>, color: peniko::Color) {
        if width <= 0.0 || !width.is_finite() {
            return;
        }
        let mut s = Stroke::new(width);
        if let Some(d) = dash {
            // Dashes are expanded into segments when encoded, so a hostile length (a rectangle a billion units
            // wide) would cost without bound. Past MAX_DASHES the line is drawn solid.
            let period = d[0] + d[1];
            if period > 0.0 && path.perimeter(1.0) / period <= MAX_DASHES {
                s = s.with_dashes(0.0, d);
            }
        }
        self.scene.stroke(&s, Affine::IDENTITY, color, None, path);
        self.grow(path.control_box().inflate(width / 2.0, width / 2.0));
    }
}

fn stroke_dash(style: StrokeStyle, w: f64) -> Option<[f64; 2]> {
    match style {
        StrokeStyle::Solid => None,
        StrokeStyle::Dashed => Some([w * 4.0, w * 2.0]),
        StrokeStyle::Dotted => Some([w, w * 2.0]),
    }
}

/// Draws sketch output: fills in order, then strokes in order.
fn paint_geometry(ink: &mut Ink, e: &Element, g: &SketchGeometry) {
    let style = &e.style;
    let stroke_color = ink.color(style.stroke_color);
    let fill = style.fill_color.map(|c| ink.color(c));
    for (path, kind) in &g.fills {
        match kind {
            FillKind::Solid => {
                if let Some(c) = fill {
                    ink.fill(path, c);
                }
            }
            FillKind::Tint { alpha } => {
                if let Some(c) = fill {
                    let rgba = c.to_rgba8();
                    let a = (f64::from(rgba.a) * alpha) as u8;
                    ink.fill(path, peniko::Color::from_rgba8(rgba.r, rgba.g, rgba.b, a));
                }
            }
            FillKind::Ink => ink.fill(path, stroke_color),
            FillKind::Hatch { line_width, clip } => {
                if let Some(c) = fill {
                    ink.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, clip);
                    if *line_width > 0.0 {
                        ink.scene.stroke(&Stroke::new(*line_width), Affine::IDENTITY, c, None, path);
                    }
                    ink.scene.pop_layer();
                    // The pattern overshoots the shape; what shows is limited to the clip.
                    ink.grow(clip.control_box());
                }
            }
        }
    }
    for (path, s) in &g.strokes {
        ink.stroke(path, s.width, s.dash, stroke_color);
    }
}

/// The entity-relationship markers, which the sketch generator does not draw.
fn paint_cardinality(ink: &mut Ink, e: &Element, d: &drafftink_scene::LinearData) {
    let pts = &d.points;
    let (Some(&first), Some(&second), Some(&last), Some(&penultimate)) =
        (pts.first(), pts.get(1), pts.last(), pts.iter().rev().nth(1))
    else {
        return;
    };
    let size = head_size(d, e.style.stroke_width);
    let ends = [(d.end_head, last, penultimate), (d.start_head, first, second)];
    let color = ink.color(e.style.stroke_color);
    for (head, tip, toward) in ends {
        if !matches!(
            head,
            Head::CardinalityOne
                | Head::CardinalityMany
                | Head::CardinalityOneOrMany
                | Head::CardinalityExactlyOne
                | Head::CardinalityZeroOrOne
                | Head::CardinalityZeroOrMany
        ) {
            continue;
        }
        let away = tip - toward;
        let len = away.hypot();
        let dir = if len > f64::EPSILON { away / len } else { Vec2::new(1.0, 0.0) };
        let path = cardinality_path(head, tip, dir, size);
        ink.stroke(&path, e.style.stroke_width, None, color);
    }
}

/// The strokes of a cardinality marker at `tip`; `dir` points away from the line, out of the tip.
pub(crate) fn cardinality_path(head: Head, tip: Point, dir: Vec2, size: f64) -> BezPath {
    let perp = Vec2::new(-dir.y, dir.x);
    let at = |back: f64| tip - dir * back;
    let mut p = BezPath::new();
    let bar = |p: &mut BezPath, back: f64| {
        p.move_to(at(back) + perp * (size * 0.4));
        p.line_to(at(back) - perp * (size * 0.4));
    };
    let foot = |p: &mut BezPath| {
        let root = at(size * 0.9);
        p.move_to(root);
        p.line_to(tip + perp * (size * 0.5));
        p.move_to(root);
        p.line_to(tip);
        p.move_to(root);
        p.line_to(tip - perp * (size * 0.5));
    };
    let ring = |p: &mut BezPath, back: f64| {
        for el in Circle::new(at(back), size * 0.28).path_elements(0.05) {
            p.push(el);
        }
    };
    match head {
        Head::CardinalityOne => bar(&mut p, size * 0.5),
        Head::CardinalityExactlyOne => {
            bar(&mut p, size * 0.35);
            bar(&mut p, size * 0.65);
        }
        Head::CardinalityMany => foot(&mut p),
        Head::CardinalityOneOrMany => {
            foot(&mut p);
            bar(&mut p, size * 1.15);
        }
        Head::CardinalityZeroOrOne => {
            bar(&mut p, size * 0.35);
            ring(&mut p, size * 0.85);
        }
        Head::CardinalityZeroOrMany => {
            foot(&mut p);
            ring(&mut p, size * 1.3);
        }
        _ => {}
    }
    p
}

/// A highlighter: one wide round-capped stroke whose alpha applies to the stroke as a whole, so a self-crossing
/// stroke does not get darker where it overlaps.
fn paint_highlighter(ink: &mut Ink, e: &Element, f: &FreehandData) {
    let width = e.style.stroke_width.max(0.0);
    let pts: Vec<Point> = f.points.iter().copied().filter(|p| p.x.is_finite() && p.y.is_finite()).collect();
    let Some((&first, rest)) = pts.split_first() else { return };
    let paint = ink.color(e.style.stroke_color);
    let rgba = paint.to_rgba8();
    let opaque = peniko::Color::from_rgba8(rgba.r, rgba.g, rgba.b, 255);
    let alpha = f32::from(rgba.a) / 255.0;
    let mut path = BezPath::new();
    path.move_to(first);
    for p in rest {
        path.line_to(*p);
    }
    let area = path.control_box().inflate(width / 2.0 + 1.0, width / 2.0 + 1.0);
    ink.scene.push_layer(Fill::NonZero, Mix::Normal, alpha, Affine::IDENTITY, &area);
    if pts.len() == 1 {
        let dot = Circle::new(first, width / 2.0).to_path(0.05);
        ink.scene.fill(Fill::NonZero, Affine::IDENTITY, opaque, None, &dot);
    } else {
        let s = Stroke::new(width.max(0.0)).with_caps(Cap::Round).with_join(Join::Round);
        ink.scene.stroke(&s, Affine::IDENTITY, opaque, None, &path);
    }
    ink.scene.pop_layer();
    ink.grow(area);
}

fn paint_text(ink: &mut Ink, e: &Element, t: &TextData, ts: &mut Typesetter) {
    if t.content.is_empty() {
        return;
    }
    let spec = text_spec(t, e.style.stroke_color, e.transform.w);
    let layout = ts.text(&spec);
    draw_layout(ink, &layout, Vec2::ZERO);
}

/// Draws a text layout with its top-left at `origin`.
pub(crate) fn draw_layout(ink: &mut Ink, layout: &drafftink_typeset::TextLayout, origin: Vec2) {
    let to = Affine::translate(origin);
    draw_runs(ink, layout, to, |ink, run| {
        let c = run.color;
        ink.color(Color::rgba(c.r, c.g, c.b, c.a))
    });
    for d in &layout.decorations {
        let c = d.color;
        let color = ink.color(Color::rgba(c.r, c.g, c.b, c.a));
        ink.scene.fill(Fill::NonZero, to, color, None, &d.rect);
    }
    let b = layout.bounds();
    ink.grow(Rect::new(b.x0 + origin.x, b.y0 + origin.y, b.x1 + origin.x, b.y1 + origin.y));
}

fn paint_math(ink: &mut Ink, e: &Element, m: &MathData, ts: &mut Typesetter) {
    let spec = math_spec(m);
    let layout = match ts.math(&spec) {
        Ok(l) => l,
        Err(_) => {
            // The old look: a pale red box with a red border where the formula would be.
            let t = &e.transform;
            let (w, h) = if t.w > 0.0 && t.h > 0.0 { (t.w, t.h) } else { (40.0, 24.0) };
            let r = Rect::new(0.0, 0.0, w, h);
            ink.scene.fill(Fill::NonZero, Affine::IDENTITY, ink.faded(255, 200, 200, 100), None, &r);
            ink.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, ink.faded(255, 100, 100, 255), None, &r);
            ink.grow(r.inflate(0.5, 0.5));
            return;
        }
    };
    // Layout coordinates are relative to the baseline origin; the element box starts `height` above it.
    let to = Affine::translate((0.0, layout.height));
    let base = ink.color(e.style.stroke_color);
    for run in layout.glyphs.chunk_by(same_run) {
        let Some(first) = run.first() else { continue };
        let color = match first.color {
            Some(c) => ink.color(Color::rgba(c.r, c.g, c.b, c.a)),
            None => base,
        };
        let slot: MathFontSlot = first.font;
        ink.scene
            .draw_glyphs(layout.font(slot))
            .brush(color)
            .hint(false)
            .transform(to)
            .font_size(first.size as f32)
            .draw(Fill::NonZero, run.iter().map(|g| Glyph { id: u32::from(g.id), x: g.x as f32, y: g.y as f32 }));
    }
    for r in &layout.rules {
        let color = match r.color {
            Some(c) => ink.color(Color::rgba(c.r, c.g, c.b, c.a)),
            None => base,
        };
        ink.scene.fill(Fill::NonZero, to, color, None, &r.rect);
    }
    let b = layout.bounds();
    ink.grow(Rect::new(b.x0, b.y0 + layout.height, b.x1, b.y1 + layout.height));
}

fn same_run(a: &drafftink_typeset::MathGlyph, b: &drafftink_typeset::MathGlyph) -> bool {
    a.font == b.font && a.size.to_bits() == b.size.to_bits() && a.color == b.color
}

impl Ink {
    /// A fixed colour (not themed) with only the element opacity applied.
    fn faded(&self, r: u8, g: u8, b: u8, a: u8) -> peniko::Color {
        peniko::Color::from_rgba8(r, g, b, (f64::from(a) * self.opacity) as u8)
    }
}

fn paint_frame(ink: &mut Ink, e: &Element, f: &FrameData, ts: &mut Typesetter) {
    let t = &e.transform;
    let (w, h) = (t.w.max(0.0), t.h.max(0.0));
    let style = &e.style;
    let path = if style.corner_radius > 0.0 {
        let r = style.corner_radius.min(w.min(h) / 2.0);
        RoundedRect::new(0.0, 0.0, w, h, r).to_path(0.05)
    } else {
        Rect::new(0.0, 0.0, w, h).to_path(0.05)
    };
    if let Some(fill) = style.fill_color
        && style.fill_pattern == FillPattern::Solid
    {
        let c = ink.color(fill);
        ink.fill(&path, c);
    }
    let sc = ink.color(style.stroke_color);
    ink.stroke(&path, style.stroke_width, stroke_dash(style.stroke_style, style.stroke_width), sc);

    let name = f.name.as_deref().unwrap_or("Frame");
    if name.is_empty() {
        return;
    }
    let layout = ts.text(&frame_label_spec(name));
    let clip = Rect::new(0.0, -FRAME_LABEL_ROOM, w.max(FRAME_LABEL_MIN_WIDTH), 0.0);
    // The label is a caption in the stroke colour.
    let c = style.stroke_color;
    let origin = Vec2::new(0.0, -(FRAME_LABEL_GAP + layout.size.height));
    ink.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &clip);
    draw_label(ink, &layout, origin, c);
    ink.scene.pop_layer();
    ink.grow(Rect::new(
        0.0,
        origin.y.max(-FRAME_LABEL_ROOM),
        layout.size.width.min(clip.x1),
        origin.y + layout.size.height,
    ));
}

/// Draws a layout in a single colour, whatever colours its runs carry.
fn draw_label(ink: &mut Ink, layout: &drafftink_typeset::TextLayout, origin: Vec2, c: Color) {
    let color = ink.color(c);
    draw_runs(ink, layout, Affine::translate(origin), |_, _| color);
}

/// Draws the glyph runs of `layout` under `to`, each in the colour `color_of` gives for it.
fn draw_runs(
    ink: &mut Ink,
    layout: &drafftink_typeset::TextLayout,
    to: Affine,
    color_of: impl Fn(&Ink, &drafftink_typeset::GlyphRun) -> peniko::Color,
) {
    for run in layout.runs.iter().filter(|run| !run.glyphs.is_empty()) {
        let color = color_of(ink, run);
        ink.scene
            .draw_glyphs(&run.font)
            .brush(color)
            .hint(true)
            .transform(to)
            .glyph_transform(run.skew.map(|s| Affine::skew(f64::from(s), 0.0)))
            .font_size(run.font_size)
            .normalized_coords(&run.normalized_coords)
            .draw(Fill::NonZero, run.glyphs.iter().map(|g| Glyph { id: g.id, x: g.x, y: g.y }));
    }
}

fn paint_picture(ink: &mut Ink, img: &ImageElem, local: Rect, data: peniko::ImageData) {
    let (iw, ih) = (f64::from(data.width), f64::from(data.height));
    if iw <= 0.0 || ih <= 0.0 || local.width() <= 0.0 || local.height() <= 0.0 {
        return;
    }
    // The crop is in pixels of the encoded picture; the decoded one should have the same size, but scale if not.
    let (kx, ky) = (
        if img.natural_width > 0 { iw / f64::from(img.natural_width) } else { 1.0 },
        if img.natural_height > 0 { ih / f64::from(img.natural_height) } else { 1.0 },
    );
    let whole = Rect::new(0.0, 0.0, iw, ih);
    let src = match &img.crop {
        Some(c) if c.width > 0.0 && c.height > 0.0 => {
            let r = Rect::new(c.x * kx, c.y * ky, (c.x + c.width) * kx, (c.y + c.height) * ky);
            let r = r.intersect(whole);
            if r.width() > 0.0 && r.height() > 0.0 { r } else { whole }
        }
        _ => whole,
    };
    let to_box = Affine::scale_non_uniform(local.width() / src.width(), local.height() / src.height())
        * Affine::translate((-src.x0, -src.y0));
    let brush = ImageBrush::new(data).with_alpha(ink.opacity as f32);
    ink.scene.fill(Fill::NonZero, Affine::IDENTITY, &brush, Some(to_box), &local);
    ink.grow(local);
}

fn paint_border(ink: &mut Ink, e: &Element, local: Rect) {
    let s = &e.style;
    if s.stroke_width > 0.0 && s.stroke_color.a > 0 {
        let c = ink.color(s.stroke_color);
        ink.stroke(&local.to_path(0.05), s.stroke_width, stroke_dash(s.stroke_style, s.stroke_width), c);
    }
}

/// The grey box with a cross the old app drew for a picture it could not show. `broken` marks a decode failure.
fn paint_image_placeholder(ink: &mut Ink, r: Rect, broken: bool) {
    let r = if r.width() > 0.0 && r.height() > 0.0 { r } else { Rect::new(0.0, 0.0, 24.0, 24.0) };
    let fill = if broken { ink.faded(230, 190, 190, 255) } else { ink.faded(200, 200, 200, 255) };
    ink.scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &r);
    let mut x = BezPath::new();
    x.move_to(Point::new(r.x0, r.y0));
    x.line_to(Point::new(r.x1, r.y1));
    x.move_to(Point::new(r.x1, r.y0));
    x.line_to(Point::new(r.x0, r.y1));
    let stroke = Stroke::new(2.0);
    ink.scene.stroke(&stroke, Affine::IDENTITY, ink.faded(150, 150, 150, 255), None, &x);
    ink.scene.stroke(&stroke, Affine::IDENTITY, ink.faded(100, 100, 100, 255), None, &r);
    ink.grow(r.inflate(1.0, 1.0));
}
