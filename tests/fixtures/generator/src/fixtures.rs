//! The documents. Every shape is built through the old public API, so the old serializer decides
//! the bytes. Seeds are set explicitly so the output does not depend on the old seed counter.

use crate::util::{fnv32, Res, Rng};
use drafftink_core::canvas::CanvasDocument;
use drafftink_core::shapes::*;
use kurbo::Point;

pub fn col(r: u8, g: u8, b: u8, a: u8) -> SerializableColor {
    SerializableColor::new(r, g, b, a)
}

const PALETTE: [(u8, u8, u8); 8] = [
    (0, 0, 0),
    (220, 38, 38),
    (234, 88, 12),
    (202, 138, 4),
    (22, 163, 74),
    (37, 99, 235),
    (147, 51, 234),
    (236, 72, 153),
];

pub fn palette(i: usize) -> SerializableColor {
    let (r, g, b) = PALETTE[i % PALETTE.len()];
    col(r, g, b, 255)
}

pub const SLOPS: [Sloppiness; 4] = [
    Sloppiness::Architect,
    Sloppiness::Artist,
    Sloppiness::Cartoonist,
    Sloppiness::Drunk,
];
pub const PATTERNS: [FillPattern; 7] = [
    FillPattern::Solid,
    FillPattern::Hachure,
    FillPattern::ZigZag,
    FillPattern::CrossHatch,
    FillPattern::Dots,
    FillPattern::Dashed,
    FillPattern::ZigZagLine,
];
pub const PATHS: [PathStyle; 3] = [PathStyle::Direct, PathStyle::Flowing, PathStyle::Angular];
pub const STROKES: [StrokeStyle; 3] = [StrokeStyle::Solid, StrokeStyle::Dashed, StrokeStyle::Dotted];
pub const FAMILIES: [FontFamily; 4] = [
    FontFamily::GelPen,
    FontFamily::NotoSans,
    FontFamily::GelPenSerif,
    FontFamily::VanillaExtract,
];
pub const WEIGHTS: [FontWeight; 3] = [FontWeight::Light, FontWeight::Regular, FontWeight::Heavy];

/// Builds styles with a running deterministic seed.
pub struct Sty(pub u32);

impl Sty {
    pub fn new(base: &str) -> Self {
        Sty(fnv32(base))
    }
    pub fn make(
        &mut self,
        stroke: SerializableColor,
        width: f64,
        fill: Option<SerializableColor>,
        pattern: FillPattern,
        slop: Sloppiness,
        opacity: f64,
    ) -> ShapeStyle {
        self.0 = self.0.wrapping_add(0x9E37_79B9);
        ShapeStyle {
            stroke_color: stroke,
            stroke_width: width,
            fill_color: fill,
            fill_pattern: pattern,
            sloppiness: slop,
            seed: self.0,
            opacity,
        }
    }
    pub fn plain(&mut self) -> ShapeStyle {
        self.make(col(0, 0, 0, 255), 2.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0)
    }
}

fn doc(name: &str) -> CanvasDocument {
    let mut d = CanvasDocument::new();
    d.name = name.to_string();
    d
}

pub fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

/// A grid cell origin for the style-matrix documents.
fn cell(i: usize, cols: usize, w: f64, h: f64) -> (f64, f64) {
    ((i % cols) as f64 * w, (i / cols) as f64 * h)
}

pub fn styles_rectangle() -> CanvasDocument {
    let mut d = doc("styles_rectangle");
    let mut s = Sty::new("rect");
    let mut i = 0;
    // sloppiness x fill pattern (28), filled with a colour and a stroke colour that cycles.
    for (si, slop) in SLOPS.iter().enumerate() {
        for (pi, pat) in PATTERNS.iter().enumerate() {
            let (x, y) = cell(i, 7, 160.0, 130.0);
            let mut r = Rectangle::new(p(x, y), 120.0, 90.0);
            r.style = s.make(palette(si + pi), 2.0, Some(palette(pi + 3)), *pat, *slop, 1.0);
            d.add_shape(Shape::Rectangle(r));
            i += 1;
        }
    }
    // stroke widths, opacity, corner radii, rotation, alpha colours, no fill, zero-size, negative.
    let extras: [(f64, f64, f64, f64, Option<SerializableColor>, f64); 6] = [
        (0.5, 0.0, 0.0, 1.0, None, 1.0),
        (1.0, 8.0, 0.0, 1.0, Some(col(255, 200, 0, 255)), 1.0),
        (4.0, 24.0, 0.3, 0.5, Some(col(0, 120, 255, 255)), 1.0),
        (8.0, 45.0, -0.7, 0.25, Some(col(0, 0, 0, 255)), 1.0),
        (3.0, 0.0, 1.5707963267948966, 1.0, Some(col(255, 0, 0, 128)), 1.0),
        (2.0, 100.0, 3.141592653589793, 0.0, Some(col(0, 255, 0, 255)), 1.0),
    ];
    for (k, (w, radius, rot, opacity, fill, _)) in extras.iter().enumerate() {
        let (x, y) = cell(i, 7, 160.0, 130.0);
        let mut r = Rectangle::new(p(x, y), 120.0, 90.0);
        r.corner_radius = *radius;
        r.rotation = *rot;
        r.style = s.make(palette(k), *w, *fill, FillPattern::Hachure, Sloppiness::Artist, *opacity);
        d.add_shape(Shape::Rectangle(r));
        i += 1;
    }
    // degenerate sizes and rectangles with the same seed (seed collisions are real in old files)
    for (w, h) in [(0.0, 0.0), (0.0, 50.0), (-80.0, -40.0)] {
        let (x, y) = cell(i, 7, 160.0, 130.0);
        let mut r = Rectangle::new(p(x + 60.0, y + 40.0), w, h);
        r.style = s.plain();
        d.add_shape(Shape::Rectangle(r));
        i += 1;
    }
    d
}

pub fn styles_ellipse() -> CanvasDocument {
    let mut d = doc("styles_ellipse");
    let mut s = Sty::new("ellipse");
    let mut i = 0;
    for (si, slop) in SLOPS.iter().enumerate() {
        for (pi, pat) in PATTERNS.iter().enumerate() {
            let (x, y) = cell(i, 7, 160.0, 130.0);
            let mut e = Ellipse::new(p(x + 60.0, y + 45.0), 60.0, 40.0);
            e.style = s.make(palette(si + pi), 2.0, Some(palette(pi + 5)), *pat, *slop, 1.0);
            d.add_shape(Shape::Ellipse(e));
            i += 1;
        }
    }
    let variants: [(f64, f64, f64, f64, Option<SerializableColor>); 6] = [
        (40.0, 40.0, 0.0, 1.0, None),
        (60.0, 20.0, 0.6, 1.0, Some(col(255, 200, 0, 255))),
        (20.0, 60.0, -1.2, 0.5, Some(col(0, 120, 255, 255))),
        (50.0, 30.0, 3.141592653589793, 0.2, Some(col(0, 0, 0, 255))),
        (1.0, 1.0, 0.0, 1.0, Some(col(255, 0, 0, 255))),
        (0.0, 0.0, 0.0, 1.0, None),
    ];
    for (k, (rx, ry, rot, op, fill)) in variants.iter().enumerate() {
        let (x, y) = cell(i, 7, 160.0, 130.0);
        let mut e = Ellipse::new(p(x + 60.0, y + 45.0), *rx, *ry);
        e.rotation = *rot;
        e.style = s.make(palette(k), 1.0 + k as f64, *fill, FillPattern::CrossHatch, Sloppiness::Artist, *op);
        d.add_shape(Shape::Ellipse(e));
        i += 1;
    }
    d
}

fn zigzag(n: usize, x: f64, y: f64, dx: f64, amp: f64) -> Vec<Point> {
    (0..n)
        .map(|k| p(x + k as f64 * dx, y + if k % 2 == 0 { 0.0 } else { amp }))
        .collect()
}

pub fn styles_line() -> CanvasDocument {
    let mut d = doc("styles_line");
    let mut s = Sty::new("line");
    let mut i = 0;
    for (pi, path) in PATHS.iter().enumerate() {
        for (ti, stroke) in STROKES.iter().enumerate() {
            for (k, npts) in [2usize, 5].iter().enumerate() {
                let (x, y) = cell(i, 6, 200.0, 120.0);
                let pts = if *npts == 2 {
                    vec![p(x, y + 10.0), p(x + 160.0, y + 90.0)]
                } else {
                    zigzag(5, x, y + 10.0, 40.0, 70.0)
                };
                let mut l = Line::from_points(pts, *path);
                l.stroke_style = *stroke;
                l.style = s.make(palette(pi + ti + k), 1.0 + ti as f64 * 2.0, None, FillPattern::Solid, SLOPS[(pi + ti) % 4], 1.0);
                d.add_shape(Shape::Line(l));
                i += 1;
            }
        }
    }
    // closed polygons, filled and not (the flag is lost through the CRDT)
    for (k, fill) in [None, Some(col(255, 220, 120, 255))].iter().enumerate() {
        let (x, y) = cell(i, 6, 200.0, 120.0);
        let mut l = Line::from_points(
            vec![p(x, y + 80.0), p(x + 80.0, y), p(x + 160.0, y + 80.0), p(x, y + 80.0)],
            PathStyle::Direct,
        );
        l.closed = true;
        l.style = s.make(palette(k + 1), 2.0, *fill, FillPattern::Hachure, Sloppiness::Artist, 1.0);
        d.add_shape(Shape::Line(l));
        i += 1;
    }
    // degenerate: start == end; two points only with opacity
    let (x, y) = cell(i, 6, 200.0, 120.0);
    let mut l = Line::new(p(x + 10.0, y + 10.0), p(x + 10.0, y + 10.0));
    l.style = s.plain();
    d.add_shape(Shape::Line(l));
    let (x, y) = cell(i + 1, 6, 200.0, 120.0);
    let mut l = Line::new(p(x, y), p(x + 150.0, y + 20.0));
    l.style = s.make(palette(2), 6.0, None, FillPattern::Solid, SLOPS[3], 0.4);
    d.add_shape(Shape::Line(l));
    d
}

pub fn styles_arrow() -> CanvasDocument {
    let mut d = doc("styles_arrow");
    let mut s = Sty::new("arrow");
    let mut i = 0;
    for (pi, path) in PATHS.iter().enumerate() {
        for (ti, stroke) in STROKES.iter().enumerate() {
            for (k, npts) in [2usize, 4].iter().enumerate() {
                let (x, y) = cell(i, 6, 200.0, 120.0);
                let pts = if *npts == 2 {
                    vec![p(x, y + 10.0), p(x + 160.0, y + 90.0)]
                } else {
                    zigzag(4, x, y + 10.0, 50.0, 70.0)
                };
                let mut a = Arrow::from_points(pts, *path);
                a.stroke_style = *stroke;
                a.head_size = [15.0, 30.0][k];
                a.style = s.make(palette(pi + ti + k), 1.0 + ti as f64 * 2.0, None, FillPattern::Solid, SLOPS[(pi + ti) % 4], 1.0);
                d.add_shape(Shape::Arrow(a));
                i += 1;
            }
        }
    }
    for head in [0.0, 8.0, 60.0] {
        let (x, y) = cell(i, 6, 200.0, 120.0);
        let mut a = Arrow::new(p(x, y + 60.0), p(x + 150.0, y + 60.0));
        a.head_size = head;
        a.style = s.plain();
        d.add_shape(Shape::Arrow(a));
        i += 1;
    }
    // leftward, vertical, zero-length
    for (dx, dy) in [(-150.0, 0.0), (0.0, 90.0), (0.0, 0.0)] {
        let (x, y) = cell(i, 6, 200.0, 120.0);
        let mut a = Arrow::new(p(x + 160.0, y + 10.0), p(x + 160.0 + dx, y + 10.0 + dy));
        a.style = s.make(palette(i), 3.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
        d.add_shape(Shape::Arrow(a));
        i += 1;
    }
    d
}

pub fn styles_freehand() -> CanvasDocument {
    let mut d = doc("styles_freehand");
    let mut s = Sty::new("freehand");
    let mut rng = Rng(7);
    let mut i = 0;
    let wave = |rng: &mut Rng, n: usize, x: f64, y: f64| -> Vec<Point> {
        (0..n)
            .map(|k| {
                let t = k as f64 / (n - 1) as f64;
                p(
                    ((x + t * 150.0 + rng.range(-1.0, 1.0)) * 100.0).round() / 100.0,
                    ((y + 40.0 + (t * 9.0).sin() * 30.0 + rng.range(-1.0, 1.0)) * 100.0).round() / 100.0,
                )
            })
            .collect()
    };
    for (wi, w) in [1.0, 2.0, 4.0, 10.0].iter().enumerate() {
        for pressure in [false, true] {
            let (x, y) = cell(i, 4, 200.0, 100.0);
            let pts = wave(&mut rng, 24, x, y);
            let mut f = if pressure {
                let pr: Vec<f64> = (0..pts.len())
                    .map(|k| {
                        let t = k as f64 / (pts.len() - 1) as f64;
                        ((0.15 + 0.85 * (t * std::f64::consts::PI).sin()) * 1000.0).round() / 1000.0
                    })
                    .collect();
                Freehand::from_points_with_pressure(pts, pr)
            } else {
                Freehand::from_points(pts)
            };
            f.style = s.make(palette(wi + pressure as usize), *w, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
            d.add_shape(Shape::Freehand(f));
            i += 1;
        }
    }
    // closed loops: plain and filled (closed is lost through the CRDT)
    for (k, fill) in [None, Some(col(255, 220, 120, 255))].iter().enumerate() {
        let (x, y) = cell(i, 4, 200.0, 100.0);
        let pts: Vec<Point> = (0..=20)
            .map(|j| {
                let a = j as f64 / 20.0 * std::f64::consts::TAU;
                p(((x + 60.0 + a.cos() * 50.0) * 100.0).round() / 100.0, ((y + 40.0 + a.sin() * 35.0) * 100.0).round() / 100.0)
            })
            .collect();
        let mut f = Freehand::from_points(pts);
        f.closed = true;
        f.style = s.make(palette(k + 4), 2.0, *fill, FillPattern::Hachure, Sloppiness::Artist, 1.0);
        d.add_shape(Shape::Freehand(f));
        i += 1;
    }
    // single point, two identical points, opacity
    let (x, y) = cell(i, 4, 200.0, 100.0);
    let mut f = Freehand::from_points(vec![p(x + 20.0, y + 20.0)]);
    f.style = s.make(palette(1), 8.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
    d.add_shape(Shape::Freehand(f));
    let (x, y) = cell(i + 1, 4, 200.0, 100.0);
    let mut f = Freehand::from_points(wave(&mut rng, 12, x, y));
    f.style = s.make(palette(2), 6.0, None, FillPattern::Solid, Sloppiness::Artist, 0.35);
    d.add_shape(Shape::Freehand(f));
    d
}

pub fn text_fonts() -> CanvasDocument {
    let mut d = doc("text_fonts");
    let mut s = Sty::new("text_fonts");
    let mut y = 0.0;
    for (fi, fam) in FAMILIES.iter().enumerate() {
        for wt in WEIGHTS.iter() {
            let content = format!("{} {:?}: Sphinx of black quartz, judge my vow 0123456789", fam.name(), wt);
            let mut t = Text::new(p(0.0, y), content)
                .with_font_family(*fam)
                .with_font_weight(*wt)
                .with_font_size(24.0);
            t.style = s.make(palette(fi), 2.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
            d.add_shape(Shape::Text(t));
            y += 40.0;
        }
    }
    // sizes
    for (k, size) in [8.0, 12.0, 20.0, 28.0, 36.0, 64.0].iter().enumerate() {
        let mut t = Text::new(p(900.0, k as f64 * 70.0), format!("Size {size}")).with_font_size(*size);
        t.style = s.plain();
        d.add_shape(Shape::Text(t));
    }
    // multi-line, trailing newline, empty, unicode (CJK, emoji, combining), tabs, rotation, opacity
    let cases: [(&str, f64, f64); 8] = [
        ("line one\nline two\nline three", 0.0, 1.0),
        ("trailing newline\n", 0.0, 1.0),
        ("", 0.0, 1.0),
        ("日本語のテキスト 你好 안녕하세요", 0.0, 1.0),
        ("emoji 😀🎉 and e\u{0301} combining", 0.0, 1.0),
        ("tab\tseparated\tvalues", 0.0, 1.0),
        ("rotated 30 degrees", 0.5235987755982988, 1.0),
        ("half transparent", 0.0, 0.5),
    ];
    for (k, (c, rot, op)) in cases.iter().enumerate() {
        let mut t = Text::new(p(0.0, 320.0 + k as f64 * 70.0), c.to_string());
        t.rotation = *rot;
        t.style = s.make(palette(k), 2.0, None, FillPattern::Solid, Sloppiness::Artist, *op);
        d.add_shape(Shape::Text(t));
    }
    d
}

pub fn text_char_colors() -> CanvasDocument {
    let mut d = doc("text_char_colors");
    let mut s = Sty::new("text_char_colors");
    let mut y = 0.0;
    let add = |d: &mut CanvasDocument, mut t: Text, y: &mut f64| {
        t.position = p(0.0, *y);
        *y += 60.0;
        d.add_shape(Shape::Text(t));
    };
    // all characters coloured, one colour per word, a single range, unicode (index = scalar value)
    let mut t = Text::new(p(0.0, 0.0), "Rainbow text".to_string()).with_font_size(32.0);
    t.style = s.plain();
    for (i, _) in "Rainbow text".chars().enumerate() {
        t.apply_color_to_range(i, i + 1, palette(i + 1));
    }
    add(&mut d, t, &mut y);

    let mut t = Text::new(p(0.0, 0.0), "red green blue plain".to_string()).with_font_size(28.0);
    t.style = s.plain();
    t.apply_color_to_range(0, 3, col(220, 38, 38, 255));
    t.apply_color_to_range(4, 9, col(22, 163, 74, 255));
    t.apply_color_to_range(10, 14, col(37, 99, 235, 255));
    add(&mut d, t, &mut y);

    let mut t = Text::new(p(0.0, 0.0), "multi\nline\ncolors".to_string()).with_font_size(24.0);
    t.style = s.make(palette(5), 2.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
    t.apply_color_to_range(2, 8, col(236, 72, 153, 255));
    add(&mut d, t, &mut y);
    y += 40.0;

    let mut t = Text::new(p(0.0, 0.0), "日本語 😀 mixed".to_string()).with_font_size(28.0);
    t.style = s.plain();
    t.apply_color_to_range(0, 3, col(147, 51, 234, 255));
    t.apply_color_to_range(4, 5, col(234, 88, 12, 255));
    add(&mut d, t, &mut y);

    // semi-transparent colour on one char, whole-string colour via range, other font families
    let mut t = Text::new(p(0.0, 0.0), "alpha and serif".to_string()).with_font_family(FontFamily::GelPenSerif).with_font_size(28.0);
    t.style = s.plain();
    t.apply_color_to_range(0, 5, col(0, 0, 0, 90));
    add(&mut d, t, &mut y);

    let mut t = Text::new(p(0.0, 0.0), "vanilla extract".to_string()).with_font_family(FontFamily::VanillaExtract).with_font_size(28.0);
    t.style = s.make(palette(2), 2.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
    t.apply_color_to_range(0, 7, col(0, 0, 255, 255));
    add(&mut d, t, &mut y);

    // inconsistent char_colors lengths (shorter and longer than the content) as the old loader accepts
    let mut t = Text::new(p(0.0, 0.0), "short colors".to_string()).with_font_size(24.0);
    t.style = s.plain();
    t.char_colors = vec![Some(col(255, 0, 0, 255)), Some(col(0, 255, 0, 255))];
    add(&mut d, t, &mut y);
    let mut t = Text::new(p(0.0, 0.0), "long".to_string()).with_font_size(24.0);
    t.style = s.plain();
    t.char_colors = vec![Some(col(255, 0, 0, 255)); 9];
    add(&mut d, t, &mut y);
    d
}

pub fn math() -> CanvasDocument {
    let mut d = doc("math");
    let mut s = Sty::new("math");
    let formulas = [
        r"E = mc^2",
        r"\frac{a}{b} + \sqrt{x^2 + y^2}",
        r"\sum_{i=0}^{n} i = \frac{n(n+1)}{2}",
        r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}",
        r"\alpha + \beta = \gamma",
        r"x_{1,2} = \frac{-b \pm \sqrt{b^2-4ac}}{2a}",
        r"\begin{matrix} a & b \\ c & d \end{matrix}",
        r"",
        r"\unknowncommand{x}",
        r"{",
    ];
    for (k, f) in formulas.iter().enumerate() {
        let mut m = Math::new(p(0.0, k as f64 * 80.0), f.to_string());
        m.style = s.make(palette(k), 2.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
        d.add_shape(Shape::Math(m));
    }
    for (k, (size, rot, op)) in [(12.0, 0.0, 1.0), (40.0, 0.0, 1.0), (24.0, 0.5, 1.0), (24.0, 3.141592653589793, 0.5)].iter().enumerate() {
        let mut m = Math::new(p(500.0, 30.0 + k as f64 * 100.0), r"\frac{1}{2}mv^2".to_string());
        m.font_size = *size;
        m.rotation = *rot;
        m.style = s.make(palette(k + 1), 2.0, None, FillPattern::Solid, Sloppiness::Artist, *op);
        d.add_shape(Shape::Math(m));
    }
    d
}

pub fn image_bytes(kind: &str, w: u32, h: u32) -> Res<Vec<u8>> {
    use image::{codecs, ExtendedColorType, ImageEncoder};
    let mut raw = Vec::new();
    for y in 0..h {
        for x in 0..w {
            raw.extend_from_slice(&[(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, ((x ^ y) * 8) as u8, 255]);
        }
    }
    let mut out = Vec::new();
    match kind {
        "png" => codecs::png::PngEncoder::new(&mut out).write_image(&raw, w, h, ExtendedColorType::Rgba8)?,
        "webp" => codecs::webp::WebPEncoder::new_lossless(&mut out).write_image(&raw, w, h, ExtendedColorType::Rgba8)?,
        "jpeg" => {
            let rgb: Vec<u8> = raw.chunks(4).flat_map(|c| [c[0], c[1], c[2]]).collect();
            codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).write_image(&rgb, w, h, ExtendedColorType::Rgb8)?
        }
        _ => return Err("unknown image kind".into()),
    }
    Ok(out)
}

pub fn images() -> Res<CanvasDocument> {
    let mut d = doc("images");
    let mut s = Sty::new("images");
    let specs: [(&str, ImageFormat, u32, u32, f64, f64, f64, f64); 6] = [
        ("png", ImageFormat::Png, 32, 24, 0.0, 0.0, 32.0, 24.0),
        ("jpeg", ImageFormat::Jpeg, 32, 24, 0.0, 0.0, 128.0, 96.0),
        ("webp", ImageFormat::WebP, 32, 24, 0.0, 0.0, 64.0, 48.0),
        ("png", ImageFormat::Png, 16, 16, 0.5, 0.0, 80.0, 80.0),
        ("png", ImageFormat::Png, 64, 16, 0.0, 0.0, 192.0, 24.0),
        ("png", ImageFormat::Png, 16, 64, 0.0, 0.0, 12.0, 48.0),
    ];
    let mut x = 0.0;
    for (kind, fmt, sw, sh, rot, _, w, h) in specs {
        let bytes = image_bytes(kind, sw, sh)?;
        let mut im = Image::new(p(x, 0.0), &bytes, sw, sh, fmt).with_size(w, h);
        im.rotation = rot;
        im.style = s.plain();
        d.add_shape(Shape::Image(im));
        x += w.max(40.0) + 40.0;
    }
    // fit_within as the app does on paste, an image with a stroke (optional border) and opacity
    let bytes = image_bytes("png", 100, 50)?;
    let mut im = Image::new(p(0.0, 200.0), &bytes, 100, 50, ImageFormat::Png).fit_within(400.0, 400.0);
    im.style = s.make(col(220, 38, 38, 255), 4.0, None, FillPattern::Solid, Sloppiness::Artist, 0.6);
    d.add_shape(Shape::Image(im));
    Ok(d)
}

fn mini_scene(s: &mut Sty, x: f64, y: f64) -> Vec<Shape> {
    let mut r = Rectangle::new(p(x, y), 80.0, 50.0);
    r.style = s.make(palette(1), 2.0, Some(palette(3)), FillPattern::Hachure, Sloppiness::Artist, 1.0);
    let mut e = Ellipse::new(p(x + 40.0, y + 25.0), 15.0, 15.0);
    e.style = s.make(palette(5), 2.0, Some(col(255, 255, 255, 255)), FillPattern::Solid, Sloppiness::Architect, 1.0);
    let mut t = Text::new(p(x + 5.0, y + 55.0), "label".to_string()).with_font_size(14.0);
    t.style = s.plain();
    vec![Shape::Rectangle(r), Shape::Ellipse(e), Shape::Text(t)]
}

pub fn groups_nested() -> CanvasDocument {
    let mut d = doc("groups_nested");
    let mut s = Sty::new("groups");
    // 1: flat group of three
    d.add_shape(Shape::Group(Group::new(mini_scene(&mut s, 0.0, 0.0))));
    // 2: group containing a group containing a group (depth 3), mixed with a line and an arrow
    let inner = Group::new(mini_scene(&mut s, 200.0, 0.0));
    let mut l = Line::new(p(190.0, 100.0), p(300.0, 110.0));
    l.style = s.plain();
    let mut mid_children = vec![Shape::Group(inner), Shape::Line(l)];
    mid_children.extend(mini_scene(&mut s, 200.0, 130.0).into_iter().take(1));
    let mid = Group::new(mid_children);
    let mut a = Arrow::new(p(180.0, 220.0), p(320.0, 240.0));
    a.style = s.plain();
    let outer = Group::new(vec![Shape::Group(mid), Shape::Arrow(a)]);
    d.add_shape(Shape::Group(outer));
    // 3: rotated group (rotation is lost by the CRDT and ignored by bounds)
    let mut g = Group::new(mini_scene(&mut s, 400.0, 0.0));
    g.rotation = 0.6;
    d.add_shape(Shape::Group(g));
    // 4: group with every shape type, including freehand, math, image
    let mut fh = Freehand::from_points((0..10).map(|k| p(600.0 + k as f64 * 8.0, 20.0 + (k % 3) as f64 * 10.0)).collect());
    fh.style = s.plain();
    let mut m = Math::new(p(600.0, 100.0), r"x^2".to_string());
    m.style = s.plain();
    let img = image_bytes("png", 8, 8).unwrap_or_default();
    let mut im = Image::new(p(680.0, 60.0), &img, 8, 8, ImageFormat::Png).with_size(40.0, 40.0);
    im.style = s.plain();
    let mut ar = Arrow::new(p(600.0, 150.0), p(720.0, 150.0));
    ar.style = s.plain();
    d.add_shape(Shape::Group(Group::new(vec![
        Shape::Freehand(fh),
        Shape::Math(m),
        Shape::Image(im),
        Shape::Arrow(ar),
    ])));
    // 5: single-child group and empty group (the old UI refuses these but the format allows them)
    d.add_shape(Shape::Group(Group::new(mini_scene(&mut s, 800.0, 0.0).into_iter().take(1).collect())));
    d.add_shape(Shape::Group(Group::new(vec![])));
    // 6: loose shapes above and below groups to pin z-order interleaving
    let mut r = Rectangle::new(p(30.0, 20.0), 60.0, 60.0);
    r.style = s.make(palette(4), 2.0, Some(col(22, 163, 74, 160)), FillPattern::Solid, Sloppiness::Artist, 1.0);
    d.add_shape(Shape::Rectangle(r));
    // 7: group-in-group where both groups contain the same child geometry twice (distinct ids)
    let g1 = Group::new(mini_scene(&mut s, 0.0, 350.0));
    let g2 = Group::new(mini_scene(&mut s, 0.0, 350.0));
    d.add_shape(Shape::Group(Group::new(vec![Shape::Group(g1), Shape::Group(g2)])));
    d
}

pub fn all_shapes() -> Res<CanvasDocument> {
    let mut d = doc("all_shapes");
    let mut s = Sty::new("all");
    let mut r = Rectangle::new(p(0.0, 0.0), 140.0, 90.0);
    r.corner_radius = 12.0;
    r.style = s.make(palette(1), 3.0, Some(palette(3)), FillPattern::Hachure, Sloppiness::Artist, 1.0);
    d.add_shape(Shape::Rectangle(r));
    let mut e = Ellipse::new(p(260.0, 45.0), 70.0, 45.0);
    e.style = s.make(palette(5), 2.0, Some(col(186, 230, 253, 255)), FillPattern::CrossHatch, Sloppiness::Cartoonist, 1.0);
    d.add_shape(Shape::Ellipse(e));
    let mut l = Line::from_points(vec![p(380.0, 0.0), p(430.0, 80.0), p(480.0, 10.0), p(530.0, 90.0)], PathStyle::Flowing);
    l.style = s.make(palette(4), 3.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
    d.add_shape(Shape::Line(l));
    let mut a = Arrow::from_points(vec![p(0.0, 140.0), p(150.0, 140.0), p(150.0, 220.0)], PathStyle::Angular);
    a.stroke_style = StrokeStyle::Dashed;
    a.style = s.make(palette(6), 2.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
    d.add_shape(Shape::Arrow(a));
    let pts: Vec<Point> = (0..30).map(|k| p(260.0 + k as f64 * 6.0, 190.0 + (k as f64 * 0.5).sin() * 25.0)).collect();
    let pts: Vec<Point> = pts.iter().map(|q| p((q.x * 100.0).round() / 100.0, (q.y * 100.0).round() / 100.0)).collect();
    let pr: Vec<f64> = (0..30).map(|k| (0.2 + 0.8 * (k as f64 / 29.0) * 1000.0).round() / 1000.0).map(|v: f64| v.min(1.0)).collect();
    let mut f = Freehand::from_points_with_pressure(pts, pr);
    f.style = s.make(palette(7), 4.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
    d.add_shape(Shape::Freehand(f));
    let mut t = Text::new(p(0.0, 280.0), "Hello, drafft".to_string()).with_font_size(28.0).with_font_family(FontFamily::GelPenSerif);
    t.style = s.make(palette(0), 2.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
    t.apply_color_to_range(0, 5, palette(1));
    d.add_shape(Shape::Text(t));
    let mut m = Math::new(p(300.0, 320.0), r"\frac{a}{b}".to_string());
    m.style = s.plain();
    d.add_shape(Shape::Math(m));
    let bytes = image_bytes("png", 24, 24)?;
    let mut im = Image::new(p(420.0, 270.0), &bytes, 24, 24, ImageFormat::Png).with_size(72.0, 72.0);
    im.style = s.plain();
    d.add_shape(Shape::Image(im));
    d.add_shape(Shape::Group(Group::new(mini_scene(&mut s, 560.0, 140.0))));
    Ok(d)
}

pub fn z_order() -> CanvasDocument {
    // Overlapping filled squares, then reordered with the old bring_to_front/send_to_back/forward/backward.
    let mut d = doc("z_order");
    let mut s = Sty::new("z");
    let mut ids = Vec::new();
    for k in 0..6 {
        let mut r = Rectangle::new(p(k as f64 * 40.0, k as f64 * 25.0), 120.0, 90.0);
        r.style = s.make(palette(k), 2.0, Some(palette(k + 2)), FillPattern::Solid, Sloppiness::Architect, 1.0);
        ids.push(r.id());
        d.add_shape(Shape::Rectangle(r));
    }
    d.bring_to_front(ids[0]);
    d.send_to_back(ids[5]);
    d.bring_forward(ids[2]);
    d.send_backward(ids[4]);
    d
}

/// Deterministic large documents: a mix of every cheap shape type.
pub fn large(n: usize, seed: u64) -> CanvasDocument {
    let name = format!("large_{n}");
    let mut d = doc(&name);
    let mut rng = Rng(seed);
    let mut s = Sty::new(&name);
    let side = (n as f64).sqrt() * 140.0;
    let words = ["alpha", "beta", "gamma", "delta", "kappa", "omega", "lorem", "ipsum", "draft", "canvas"];
    for _ in 0..n {
        let x = rng.range(0.0, side);
        let y = rng.range(0.0, side);
        let kind = rng.below(100);
        let slop = SLOPS[rng.below(4)];
        let pat = PATTERNS[rng.below(7)];
        let stroke = palette(rng.below(8));
        let fill = if rng.below(3) == 0 { Some(palette(rng.below(8))) } else { None };
        let width = [1.0, 2.0, 2.0, 3.0, 4.0][rng.below(5)];
        let op = if rng.below(10) == 0 { 0.5 } else { 1.0 };
        let st = s.make(stroke, width, fill, pat, slop, op);
        let sh = match kind {
            0..=29 => {
                let mut r = Rectangle::new(p(x, y), rng.range(10.0, 200.0), rng.range(10.0, 150.0));
                r.corner_radius = if rng.below(4) == 0 { rng.range(2.0, 20.0) } else { 0.0 };
                r.rotation = if rng.below(8) == 0 { rng.range(-3.0, 3.0) } else { 0.0 };
                r.style = st;
                Shape::Rectangle(r)
            }
            30..=44 => {
                let mut e = Ellipse::new(p(x, y), rng.range(5.0, 100.0), rng.range(5.0, 80.0));
                e.rotation = if rng.below(8) == 0 { rng.range(-3.0, 3.0) } else { 0.0 };
                e.style = st;
                Shape::Ellipse(e)
            }
            45..=54 => {
                let n = 2 + rng.below(4);
                let pts = (0..n).map(|k| p(x + k as f64 * rng.range(20.0, 80.0), y + rng.range(-60.0, 60.0))).collect();
                let mut l = Line::from_points(pts, PATHS[rng.below(3)]);
                l.stroke_style = STROKES[rng.below(3)];
                l.style = ShapeStyle { fill_color: None, ..st };
                Shape::Line(l)
            }
            55..=64 => {
                let n = 2 + rng.below(3);
                let pts = (0..n).map(|k| p(x + k as f64 * rng.range(30.0, 90.0), y + rng.range(-60.0, 60.0))).collect();
                let mut a = Arrow::from_points(pts, PATHS[rng.below(3)]);
                a.stroke_style = STROKES[rng.below(3)];
                a.style = ShapeStyle { fill_color: None, ..st };
                Shape::Arrow(a)
            }
            65..=74 => {
                let n = 10 + rng.below(40);
                let mut px = x;
                let mut py = y;
                let pts: Vec<Point> = (0..n)
                    .map(|_| {
                        px += rng.range(-6.0, 8.0);
                        py += rng.range(-6.0, 6.0);
                        p(px, py)
                    })
                    .collect();
                let mut f = if rng.below(2) == 0 {
                    let pr = (0..n).map(|_| rng.range(0.1, 1.0)).collect();
                    Freehand::from_points_with_pressure(pts, pr)
                } else {
                    Freehand::from_points(pts)
                };
                f.style = ShapeStyle { fill_color: None, ..st };
                Shape::Freehand(f)
            }
            75..=94 => {
                let nw = 1 + rng.below(4);
                let content: Vec<&str> = (0..nw).map(|_| words[rng.below(10)]).collect();
                let content = if rng.below(6) == 0 { content.join("\n") } else { content.join(" ") };
                let mut t = Text::new(p(x, y), content)
                    .with_font_family(FAMILIES[rng.below(4)])
                    .with_font_weight(WEIGHTS[rng.below(3)])
                    .with_font_size([12.0, 16.0, 20.0, 28.0][rng.below(4)]);
                if rng.below(5) == 0 {
                    let len = t.content.chars().count();
                    t.apply_color_to_range(0, len / 2 + 1, palette(rng.below(8)));
                }
                t.style = ShapeStyle { fill_color: None, ..st };
                Shape::Text(t)
            }
            95..=96 => {
                let mut m = Math::new(p(x, y), r"\frac{a+b}{c}".to_string());
                m.style = ShapeStyle { fill_color: None, ..st };
                Shape::Math(m)
            }
            _ => {
                let mut kids = Vec::new();
                for k in 0..(2 + rng.below(3)) {
                    let mut r = Rectangle::new(p(x + k as f64 * 30.0, y), rng.range(10.0, 60.0), rng.range(10.0, 60.0));
                    r.style = s.make(palette(k), 2.0, None, FillPattern::Solid, Sloppiness::Artist, 1.0);
                    kids.push(Shape::Rectangle(r));
                }
                Shape::Group(Group::new(kids))
            }
        };
        d.add_shape(sh);
    }
    d
}
