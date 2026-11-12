//! SVG export, with the scene embedded so the file can be opened again.
//!
//! # Decisions
//!
//! * **Geometry** comes from `drafftink-sketch`, the same generator the renderer uses, so the SVG looks like the
//!   canvas. [`SvgStyle::Sketch`] honours each element's `sloppiness` and `seed`; [`SvgStyle::Clean`] draws every
//!   element with roughness 0.
//! * **Text is `<text>`.** It keeps its family, size, weight, alignment and per-character colours and stays
//!   selectable. To make the file look the same on a machine without the fonts, pass the font files in
//!   [`SvgOptions::fonts`]: each used family is then embedded as a base64 `@font-face`. Without them the SVG names
//!   the family and a generic fallback. Two kinds of text are glyph outlines instead, written by
//!   [`MathSvg::text_svg`] because outlines need the font files and the layout engine, which belong to `typeset`,
//!   not to this pure crate: text with a formula, and text with a character that only a glyph-fallback font draws
//!   (Kannada, symbols, CJK punctuation: `drafftink_typeset::needs_glyph_fallback`).
//! * **Formulas** are drawn by a [`MathSvg`] the engine supplies (it owns the formula engine). Without one the
//!   LaTeX source is written as italic text.
//! * **Images** are embedded as data URIs (cropping and mirroring included). An image whose blob is missing
//!   is drawn as a dashed placeholder. Image borders are not drawn.
//! * **Selection export** takes the given ids plus the labels bound to them. Only those elements, and only the
//!   blobs they use, go into the embedded scene, so exporting a selection never leaks the rest of the document.
//! * **Embedded scene**: a `<metadata><drafftink-scene encoding="base64-deflate">` element holding the native JSON,
//!   read back by [`extract_scene`].

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// Appends formatted text to a `String`. Writing to a `String` cannot fail, so there is no result to handle.
macro_rules! put {
    ($out:expr, $($arg:tt)*) => {{
        let _ = write!($out, $($arg)*);
    }};
}

/// [`put!`] followed by a newline.
macro_rules! putln {
    ($out:expr, $($arg:tt)*) => {{
        let _ = writeln!($out, $($arg)*);
    }};
}
use std::io::Write as _;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use drafftink_scene::{
    Color, Document, Element, ElementId, ElementKind, FillPattern, FontFamily, FontWeight, Head, ImageData, InlineMath,
    PressureMode, StrokeStyle, Style, TextAlign, TextData,
};
use drafftink_sketch::{
    Arrowhead, Curve, FillKind, FillStyle, LineDash, PressureProfile, Roundness, SketchInput, SketchParams,
    SketchShape, sketch,
};
use kurbo::Rect;

use crate::error::{FormatName, IoError};
use crate::limits::MAX_EMBEDDED_SCENE_BYTES;
use crate::measure::TextMeasure;

/// Element name of the embedded scene.
pub const SCENE_TAG: &str = "drafftink-scene";

/// How elements are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SvgStyle {
    /// Hand-drawn, per the element's sloppiness.
    #[default]
    Sketch,
    /// Precise lines (roughness 0) for every element.
    Clean,
}

/// A font file to embed.
#[derive(Debug, Clone)]
pub struct FontFace {
    /// Family the file belongs to.
    pub family: FontFamily,
    /// The weight this file is. Text of a weight with no face of its own uses the family's regular face.
    pub weight: FontWeight,
    /// Media type, for example `font/ttf` or `font/woff2`.
    pub mime: String,
    /// The font file.
    pub data: Arc<[u8]>,
}

/// Draws a formula as SVG. The engine implements this over the formula engine.
pub trait MathSvg {
    /// SVG elements for `latex` at `size`, with the baseline origin at (0, 0) and y pointing down. Colours must be
    /// `currentColor`; the exporter sets `color` on the wrapper. `None` when the formula does not parse.
    fn math_svg(&self, latex: &str, size: f64) -> Option<String>;

    /// SVG elements for a whole text whose content has inline math (`$...$`): glyph outlines of the text and the
    /// formulas, laid out as the canvas draws them, with the top-left of the box at (0, 0). The shape colour is
    /// `currentColor`, colours of the text's own runs stay. `None` makes the exporter write the source as `<text>`.
    fn text_svg(&self, _text: &TextData, _box_width: f64) -> Option<String> {
        None
    }
}

/// Options of [`export_svg`].
#[derive(Debug, Clone)]
pub struct SvgOptions {
    /// Sketch or clean.
    pub style: SvgStyle,
    /// Paint the document background as a rectangle behind everything.
    pub background: bool,
    /// Space around the content, in world units.
    pub padding: f64,
    /// Scale of the `width` and `height` attributes. The `viewBox` is unscaled.
    pub scale: f64,
    /// Export only these elements (and the labels bound to them). `None` exports everything.
    pub selection: Option<Vec<ElementId>>,
    /// Embed the exported scene as native JSON.
    pub embed_scene: bool,
    /// Fonts to embed: a face is written (once) when a text uses its family and weight.
    pub fonts: Vec<FontFace>,
    /// Draw with the dark palette: every colour goes through [`Color::for_dark_theme`], as on the canvas. The
    /// embedded scene keeps the document's own colours.
    pub dark: bool,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            style: SvgStyle::Sketch,
            background: true,
            padding: 20.0,
            scale: 1.0,
            selection: None,
            embed_scene: true,
            fonts: Vec::new(),
            dark: false,
        }
    }
}

fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_owned();
    }
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".to_owned() } else { s.to_owned() }
}

/// Escapes text for XML content and attribute values, and drops characters XML 1.0 cannot carry.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}

fn paint(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

fn alpha(c: Color) -> Option<String> {
    (c.a != 255).then(|| num(f64::from(c.a) / 255.0))
}

fn css_family(f: FontFamily) -> &'static str {
    match f {
        FontFamily::GelPen => "GelPen, 'Segoe Print', 'Comic Sans MS', cursive",
        FontFamily::NotoSans => "'Noto Sans', 'Helvetica Neue', Arial, sans-serif",
        FontFamily::GelPenSerif => "'GelPen Serif', Georgia, serif",
        FontFamily::VanillaExtract => "'Vanilla Extract', 'Segoe Print', cursive",
        FontFamily::PlexSans => "'IBM Plex Sans', 'Helvetica Neue', Arial, sans-serif",
    }
}

fn weight(w: FontWeight) -> u16 {
    match w {
        FontWeight::Light => 300,
        FontWeight::Regular => 400,
        FontWeight::Heavy => 700,
    }
}

fn arrowhead(h: Head) -> Arrowhead {
    match h {
        Head::None => Arrowhead::NONE,
        Head::Arrow => Arrowhead::ARROW,
        Head::Bar => Arrowhead::BAR,
        Head::Circle => Arrowhead::CIRCLE,
        Head::CircleOutline => Arrowhead::CIRCLE_OUTLINE,
        Head::Triangle => Arrowhead::TRIANGLE,
        Head::TriangleOutline => Arrowhead::TRIANGLE_OUTLINE,
        Head::Diamond => Arrowhead::DIAMOND,
        Head::DiamondOutline => Arrowhead::DIAMOND_OUTLINE,
        // The entity-relationship markers have no sketch form yet: a bar for the "one" family, an arrow for "many".
        Head::CardinalityOne | Head::CardinalityExactlyOne | Head::CardinalityZeroOrOne => Arrowhead::BAR,
        Head::CardinalityMany | Head::CardinalityOneOrMany | Head::CardinalityZeroOrMany => Arrowhead::ARROW,
    }
}

fn fill_style(style: &Style) -> FillStyle {
    if style.fill_color.is_none() {
        return FillStyle::None;
    }
    match style.fill_pattern {
        FillPattern::Solid => FillStyle::Solid,
        FillPattern::Hachure => FillStyle::Hachure,
        FillPattern::ZigZag => FillStyle::ZigZag,
        FillPattern::CrossHatch => FillStyle::CrossHatch,
        FillPattern::Dots => FillStyle::Dots,
        FillPattern::Dashed => FillStyle::Dashed,
        FillPattern::ZigZagLine => FillStyle::ZigZagLine,
    }
}

fn head_size(d: &drafftink_scene::LinearData, style: &Style) -> f64 {
    d.head_size.unwrap_or_else(|| (style.stroke_width * 5.0).max(15.0))
}

struct Ctx<'a> {
    doc: &'a Document,
    opts: &'a SvgOptions,
    measure: &'a dyn TextMeasure,
    math: Option<&'a dyn MathSvg>,
    out: String,
    defs: String,
    next_id: u32,
    used_faces: Vec<(FontFamily, FontWeight)>,
    frame_clips: HashMap<ElementId, String>,
}

/// Draws the document (or the selection) as an SVG string.
///
/// Fails with a plain-words error when there is nothing to draw, or when the document breaks a rule and so cannot
/// be embedded.
pub fn export_svg(
    doc: &Document,
    opts: &SvgOptions,
    measure: &dyn TextMeasure,
    math: Option<&dyn MathSvg>,
) -> Result<String, IoError> {
    // A dark export draws a recoloured copy; the scene embedded below is the original.
    let themed = opts.dark.then(|| {
        let mut copy = doc.clone();
        copy.map_colors(Color::for_dark_theme);
        copy
    });
    let drawn = themed.as_ref().unwrap_or(doc);
    let selected: Option<HashSet<ElementId>> = opts.selection.as_ref().map(|s| s.iter().cloned().collect());
    let live: Vec<&Element> = drawn.elements.iter().filter(|e| !e.deleted).collect();
    let chosen: Vec<&Element> = match &selected {
        None => live,
        Some(sel) => live
            .into_iter()
            .filter(|e| sel.contains(&e.id) || e.container_id().is_some_and(|c| sel.contains(c)))
            .collect(),
    };
    let Some(bounds) = content_bounds(&chosen) else {
        return Err(IoError::invalid(FormatName::Svg, "there is nothing to export: no visible elements"));
    };
    let pad = if opts.padding.is_finite() { opts.padding.clamp(0.0, 10_000.0) } else { 20.0 };
    let scale = if opts.scale.is_finite() { opts.scale.clamp(0.01, 100.0) } else { 1.0 };
    let view = Rect::new(bounds.x0 - pad, bounds.y0 - pad, bounds.x1 + pad, bounds.y1 + pad);

    let mut cx = Ctx {
        doc: drawn,
        opts,
        measure,
        math,
        out: String::new(),
        defs: String::new(),
        next_id: 0,
        used_faces: Vec::new(),
        frame_clips: HashMap::new(),
    };
    if opts.background {
        putln!(
            cx.out,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"{}/>",
            num(view.x0),
            num(view.y0),
            num(view.width()),
            num(view.height()),
            paint(drawn.background),
            alpha(drawn.background).map(|a| format!(" fill-opacity=\"{a}\"")).unwrap_or_default()
        );
    }
    let chosen_ids: HashSet<&ElementId> = chosen.iter().map(|e| &e.id).collect();
    for e in &chosen {
        if matches!(e.kind, ElementKind::Frame(_)) {
            cx.frame_clip(e);
        }
    }
    for e in &chosen {
        cx.element(e, &chosen_ids);
    }

    let mut svg = String::with_capacity(cx.out.len() + 1024);
    put!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\">",
        num(view.width() * scale),
        num(view.height() * scale),
        num(view.x0),
        num(view.y0),
        num(view.width()),
        num(view.height())
    );
    svg.push('\n');
    if opts.embed_scene {
        let ids: HashSet<ElementId> = chosen.iter().map(|e| e.id.clone()).collect();
        let sub = crate::native::subset(doc, &ids);
        let json = sub.to_json()?;
        putln!(svg, "<metadata>{}</metadata>", embed_payload(&json)?);
    }
    let fonts = cx.font_css();
    if !cx.defs.is_empty() || !fonts.is_empty() {
        svg.push_str("<defs>");
        if !fonts.is_empty() {
            put!(svg, "<style>{fonts}</style>");
        }
        svg.push_str(&cx.defs);
        svg.push_str("</defs>\n");
    }
    svg.push_str(&cx.out);
    svg.push_str("</svg>\n");
    Ok(svg)
}

/// The union of the elements' rotated boxes, grown by half the stroke and the arrow heads.
fn content_bounds(elements: &[&Element]) -> Option<Rect> {
    let mut out: Option<Rect> = None;
    for e in elements {
        let t = e.transform;
        let grow = match &e.kind {
            ElementKind::Text(_) | ElementKind::Math(_) => 0.0,
            ElementKind::Image(_) if e.style.stroke_color.a == 0 => 0.0,
            ElementKind::Arrow(d) => e.style.stroke_width / 2.0 + head_size(d, &e.style),
            ElementKind::Freehand(_) => e.style.stroke_width * 1.2,
            _ => e.style.stroke_width / 2.0,
        };
        let c = t.center();
        let (s, co) = t.angle.sin_cos();
        let (hw, hh) = (t.w / 2.0, t.h / 2.0);
        // Half extents of the rotated box.
        let ex = hw * co.abs() + hh * s.abs() + grow;
        let ey = hw * s.abs() + hh * co.abs() + grow;
        let r = Rect::new(c.x - ex, c.y - ey, c.x + ex, c.y + ey);
        out = Some(out.map_or(r, |o| o.union(r)));
    }
    out
}

impl Ctx<'_> {
    fn fresh_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn frame_clip(&mut self, frame: &Element) {
        if let ElementKind::Frame(f) = &frame.kind
            && f.clip
        {
            let id = self.fresh_id("clip");
            let t = frame.transform;
            put!(
                self.defs,
                "<clipPath id=\"{id}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath>",
                num(t.x),
                num(t.y),
                num(t.w),
                num(t.h)
            );
            self.frame_clips.insert(frame.id.clone(), id);
        }
    }

    /// The `@font-face` rules of the faces the texts use.
    fn font_css(&self) -> String {
        let mut css = String::new();
        for &(family, wt) in &self.used_faces {
            let find = |w: FontWeight| self.opts.fonts.iter().find(|f| f.family == family && f.weight == w);
            let Some(face) = find(wt).or_else(|| find(FontWeight::Regular)) else { continue };
            let mime: String =
                face.mime.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '+' | '.' | '-')).collect();
            put!(
                css,
                "@font-face{{font-family:'{}';font-weight:{};src:url(data:{mime};base64,{});}}",
                family.display_name(),
                weight(wt),
                STANDARD.encode(&face.data)
            );
        }
        css
    }

    fn element(&mut self, e: &Element, chosen: &HashSet<&ElementId>) {
        let t = e.transform;
        let mut attrs = String::new();
        if e.style.opacity < 1.0 {
            put!(attrs, " opacity=\"{}\"", num(e.style.opacity));
        }
        if t.angle != 0.0 {
            let c = t.center();
            put!(
                attrs,
                " transform=\"rotate({} {} {}) translate({} {})\"",
                num(t.angle.to_degrees()),
                num(c.x),
                num(c.y),
                num(t.x),
                num(t.y)
            );
        } else {
            put!(attrs, " transform=\"translate({} {})\"", num(t.x), num(t.y));
        }
        let clip = e.frame_id.as_ref().filter(|f| chosen.contains(f)).and_then(|f| self.frame_clips.get(f)).cloned();
        if let Some(c) = &clip {
            put!(self.out, "<g clip-path=\"url(#{c})\">");
        }
        put!(self.out, "<g id=\"e-{}\"{attrs}>", e.id);
        match &e.kind {
            ElementKind::Rectangle => {
                let r = Roundness::Radius(e.style.corner_radius);
                self.sketched(e, SketchShape::Rect { width: t.w, height: t.h, roundness: r }, true);
            }
            ElementKind::Diamond => {
                let r = Roundness::Radius(e.style.corner_radius);
                self.sketched(e, SketchShape::Diamond { width: t.w, height: t.h, roundness: r }, true);
            }
            ElementKind::Ellipse => self.sketched(e, SketchShape::Ellipse { width: t.w, height: t.h }, true),
            ElementKind::Line(d) => {
                let curve = if d.path == drafftink_scene::PathType::Round { Curve::Smooth } else { Curve::Straight };
                self.sketched(e, SketchShape::Polyline { points: d.points.clone(), curve, closed: d.closed }, true);
            }
            ElementKind::Arrow(d) => {
                let curve = if d.path == drafftink_scene::PathType::Round { Curve::Smooth } else { Curve::Straight };
                let shape = SketchShape::Arrow {
                    points: d.points.clone(),
                    curve,
                    start: arrowhead(d.start_head),
                    end: arrowhead(d.end_head),
                    head_size: head_size(d, &e.style),
                };
                self.sketched(e, shape, false);
            }
            ElementKind::Freehand(d) => {
                let pressures = if d.pressure_mode == PressureMode::Device { d.pressures.clone() } else { Vec::new() };
                let shape = SketchShape::Freehand {
                    points: d.points.clone(),
                    pressures,
                    closed: d.closed,
                    profile: PressureProfile::default(),
                };
                self.sketched(e, shape, true);
            }
            ElementKind::Text(d) => self.text(e, d),
            ElementKind::Math(m) => self.math_el(e, &m.latex, m.font_size),
            ElementKind::Image(i) => self.image(e, i),
            ElementKind::Frame(f) => self.frame(e, f.name.as_deref()),
        }
        self.out.push_str("</g>");
        if clip.is_some() {
            self.out.push_str("</g>");
        }
        self.out.push('\n');
    }

    fn sketched(&mut self, e: &Element, shape: SketchShape, _fillable: bool) {
        let s = &e.style;
        let roughness = match self.opts.style {
            SvgStyle::Clean => 0.0,
            SvgStyle::Sketch => s.sloppiness.roughness(),
        };
        let params = SketchParams {
            roughness,
            seed: s.seed,
            stroke_width: s.stroke_width,
            line: match s.stroke_style {
                StrokeStyle::Solid => LineDash::Solid,
                StrokeStyle::Dashed => LineDash::Dashed,
                StrokeStyle::Dotted => LineDash::Dotted,
            },
            fill: fill_style(s),
            ..SketchParams::default()
        };
        let geometry = sketch(&SketchInput::new(shape, params));
        let stroke = paint(s.stroke_color);
        let stroke_op = alpha(s.stroke_color);
        let fill = s.fill_color.unwrap_or(s.stroke_color);
        let fill_hex = paint(fill);
        let fill_alpha = f64::from(fill.a) / 255.0;
        for (path, kind) in &geometry.fills {
            let d = esc(&path.to_svg());
            match kind {
                FillKind::Solid => {
                    put!(self.out, "<path d=\"{d}\" fill=\"{fill_hex}\"{}/>", opacity_attr("fill-opacity", fill_alpha));
                }
                FillKind::Tint { alpha: a } => {
                    put!(
                        self.out,
                        "<path d=\"{d}\" fill=\"{fill_hex}\"{}/>",
                        opacity_attr("fill-opacity", fill_alpha * a)
                    );
                }
                FillKind::Ink if s.stroke_color.a == 0 => {}
                FillKind::Ink => {
                    put!(
                        self.out,
                        "<path d=\"{d}\" fill=\"{stroke}\"{}/>",
                        stroke_op.as_ref().map(|a| format!(" fill-opacity=\"{a}\"")).unwrap_or_default()
                    );
                }
                FillKind::Hatch { line_width, clip } => {
                    let id = self.fresh_id("hatch");
                    put!(self.defs, "<clipPath id=\"{id}\"><path d=\"{}\"/></clipPath>", esc(&clip.to_svg()));
                    put!(
                        self.out,
                        "<path d=\"{d}\" fill=\"none\" stroke=\"{fill_hex}\" stroke-width=\"{}\" clip-path=\"url(#{id})\"{}/>",
                        num(*line_width),
                        opacity_attr("stroke-opacity", fill_alpha)
                    );
                }
            }
        }
        let strokes = if s.stroke_color.a == 0 { &[][..] } else { &geometry.strokes[..] };
        for (path, st) in strokes {
            let dash = st.dash.map(|[a, b]| format!(" stroke-dasharray=\"{} {}\"", num(a), num(b))).unwrap_or_default();
            put!(
                self.out,
                "<path d=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{}\" stroke-linejoin=\"round\"{dash}{}/>",
                esc(&path.to_svg()),
                num(st.width),
                stroke_op.as_ref().map(|a| format!(" stroke-opacity=\"{a}\"")).unwrap_or_default()
            );
        }
    }

    fn text(&mut self, e: &Element, d: &TextData) {
        if d.content.is_empty() {
            return;
        }
        // Formulas, and characters that only a fallback font draws, are written as glyph outlines: the file looks like the
        // canvas on a machine that has none of the fonts.
        let outlined = (d.font.inline_math == InlineMath::Auto && drafftink_typeset::has_inline_math(&d.content))
            || drafftink_typeset::needs_glyph_fallback(&d.content);
        if outlined && let Some(svg) = self.math.and_then(|m| m.text_svg(d, e.transform.w)) {
            let fill_opacity =
                alpha(e.style.stroke_color).map(|a| format!(" fill-opacity=\"{a}\"")).unwrap_or_default();
            put!(
                self.out,
                "<g color=\"{}\"{fill_opacity} data-text=\"{}\">{svg}</g>",
                paint(e.style.stroke_color),
                esc(&d.content)
            );
            return;
        }
        let face = (d.font.family, d.font.weight);
        if !self.used_faces.contains(&face) {
            self.used_faces.push(face);
        }
        let t = e.transform;
        let size = d.font.size;
        let line_h = size * d.font.line_height;
        let lines = self.wrap(d, t.w);
        let (anchor, x) = match d.font.align {
            TextAlign::Left => ("start", 0.0),
            TextAlign::Center => ("middle", t.w / 2.0),
            TextAlign::Right => ("end", t.w),
        };
        let colors = d.char_colors();
        put!(
            self.out,
            "<text font-family=\"{}\" font-size=\"{}\" font-weight=\"{}\" fill=\"{}\"{} text-anchor=\"{anchor}\" xml:space=\"preserve\">",
            esc(css_family(d.font.family)),
            num(size),
            weight(d.font.weight),
            paint(e.style.stroke_color),
            alpha(e.style.stroke_color).map(|a| format!(" fill-opacity=\"{a}\"")).unwrap_or_default()
        );
        // Baseline of the first line: centre the glyphs in the line box, ascent about 0.8 em.
        let first = (line_h - size) / 2.0 + size * 0.8;
        let mut index = 0usize; // scalar index into the content
        let mut rest = d.content.as_str();
        for (line_no, line) in lines.into_iter().enumerate() {
            // Map the wrapped line back onto content indexes: wrapped lines are consecutive slices of the text
            // with the break whitespace dropped, so advance until the line text is found.
            let skip_to = rest.find(line.as_str()).unwrap_or(0);
            index += rest.get(..skip_to).map_or(0, |s| s.chars().count());
            rest = rest.get(skip_to..).unwrap_or("");
            put!(self.out, "<tspan x=\"{}\" y=\"{}\">", num(x), num(first + line_h * line_no as f64));
            let mut seg = String::new();
            let mut seg_color: Option<Color> = None;
            for (i, ch) in line.chars().enumerate() {
                let c = colors.get(index + i).copied().flatten();
                if c != seg_color && !seg.is_empty() {
                    self.span(&seg, seg_color);
                    seg.clear();
                }
                seg_color = c;
                seg.push(ch);
            }
            if !seg.is_empty() {
                self.span(&seg, seg_color);
            }
            self.out.push_str("</tspan>");
            let n = line.chars().count();
            index += n;
            rest = rest.get(line.len()..).unwrap_or("");
        }
        self.out.push_str("</text>");
    }

    fn span(&mut self, text: &str, color: Option<Color>) {
        match color {
            Some(c) => {
                put!(
                    self.out,
                    "<tspan fill=\"{}\"{}>{}</tspan>",
                    paint(c),
                    alpha(c).map(|a| format!(" fill-opacity=\"{a}\"")).unwrap_or_default(),
                    esc(text)
                );
            }
            None => self.out.push_str(&esc(text)),
        }
    }

    /// The lines to draw: the content's own lines, wrapped greedily to the box width when the box is fixed-width.
    fn wrap(&self, d: &TextData, width: f64) -> Vec<String> {
        let mut out = Vec::new();
        for line in d.content.split('\n') {
            if (d.auto_resize && d.container_id.is_none()) || width <= 0.0 {
                out.push(line.to_owned());
                continue;
            }
            let mut cur = String::new();
            for word in line.split_inclusive(' ') {
                let probe = format!("{cur}{word}");
                let (w, _) = self.measure.text_size(
                    probe.trim_end(),
                    d.font.family,
                    d.font.weight,
                    d.font.size,
                    d.font.line_height,
                );
                if w > width && !cur.is_empty() {
                    out.push(cur.trim_end().to_owned());
                    cur = word.to_owned();
                } else {
                    cur = probe;
                }
            }
            out.push(cur.trim_end().to_owned());
        }
        out
    }

    fn math_el(&mut self, e: &Element, latex: &str, size: f64) {
        let color = paint(e.style.stroke_color);
        if let Some(svg) = self.math.and_then(|m| m.math_svg(latex, size)) {
            // The formula's baseline origin sits `height` below the top of the box; the exporter does not know the
            // split, so it places the baseline at the box bottom minus the depth estimate of 0.25 em.
            let base = (e.transform.h - size * 0.25).max(0.0);
            put!(self.out, "<g color=\"{color}\" transform=\"translate(0 {})\">{svg}</g>", num(base));
        } else {
            put!(
                self.out,
                "<text font-style=\"italic\" font-family=\"serif\" font-size=\"{}\" fill=\"{color}\" data-latex=\"{}\" y=\"{}\">{}</text>",
                num(size),
                esc(latex),
                num(size),
                esc(latex)
            );
        }
    }

    fn image(&mut self, e: &Element, i: &ImageData) {
        let t = e.transform;
        let blob = i.file_id.as_ref().and_then(|f| self.doc.files.get(f));
        let Some(blob) = blob else {
            put!(
                self.out,
                "<rect width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"#9ca3af\" stroke-dasharray=\"6 4\"/>",
                num(t.w),
                num(t.h)
            );
            return;
        };
        let mime: String =
            blob.mime.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '+' | '.' | '-')).collect();
        let href = format!("data:{mime};base64,{}", STANDARD.encode(&blob.bytes));
        let flip = match (t.flip_x, t.flip_y) {
            (false, false) => String::new(),
            (fx, fy) => format!(
                " transform=\"translate({} {}) scale({} {})\"",
                if fx { num(t.w) } else { "0".into() },
                if fy { num(t.h) } else { "0".into() },
                if fx { "-1" } else { "1" },
                if fy { "-1" } else { "1" }
            ),
        };
        put!(self.out, "<g{flip}>");
        match &i.crop {
            Some(c) => {
                put!(
                    self.out,
                    "<svg width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\" preserveAspectRatio=\"none\"><image href=\"{href}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\"/></svg>",
                    num(t.w),
                    num(t.h),
                    num(c.x),
                    num(c.y),
                    num(c.width),
                    num(c.height),
                    i.natural_width,
                    i.natural_height
                );
            }
            None => {
                put!(
                    self.out,
                    "<image href=\"{href}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\"/>",
                    num(t.w),
                    num(t.h)
                );
            }
        }
        self.out.push_str("</g>");
        self.image_outline(e);
    }

    /// The outline of a picture: its stroke, when it has one. A picture without a stroke (the default) writes
    /// nothing, so the file has no `stroke` attribute for it.
    fn image_outline(&mut self, e: &Element) {
        let s = &e.style;
        if s.stroke_color.a == 0 || !s.stroke_width.is_finite() || s.stroke_width <= 0.0 {
            return;
        }
        let dash = s
            .stroke_style
            .dash(s.stroke_width)
            .map(|[on, off]| format!(" stroke-dasharray=\"{} {}\"", num(on), num(off)))
            .unwrap_or_default();
        let opacity = alpha(s.stroke_color).map(|a| format!(" stroke-opacity=\"{a}\"")).unwrap_or_default();
        put!(
            self.out,
            "<rect width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"{dash}{opacity}/>",
            num(e.transform.w),
            num(e.transform.h),
            paint(s.stroke_color),
            num(s.stroke_width)
        );
    }

    fn frame(&mut self, e: &Element, name: Option<&str>) {
        let t = e.transform;
        put!(
            self.out,
            "<rect width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"/>",
            num(t.w),
            num(t.h),
            paint(e.style.stroke_color),
            num(e.style.stroke_width.min(2.0))
        );
        put!(
            self.out,
            "<text y=\"-6\" font-size=\"14\" font-family=\"sans-serif\" fill=\"#6b7280\">{}</text>",
            esc(name.unwrap_or("Frame"))
        );
    }
}

fn opacity_attr(name: &str, a: f64) -> String {
    if a >= 0.999 { String::new() } else { format!(" {name}=\"{}\"", num(a)) }
}

fn embed_payload(json: &str) -> Result<String, IoError> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(json.as_bytes()).map_err(|e| IoError::invalid(FormatName::Svg, e.to_string()))?;
    let z = enc.finish().map_err(|e| IoError::invalid(FormatName::Svg, e.to_string()))?;
    Ok(format!(
        "<{SCENE_TAG} xmlns=\"https://drafft.ink/ns\" type=\"application/vnd.drafftink+json\" encoding=\"base64-deflate\">{}</{SCENE_TAG}>",
        STANDARD.encode(z)
    ))
}

/// Reads the scene JSON an exported SVG carries. `Ok(None)` when the SVG has none (it was not exported by this
/// app, or was exported without the scene). The text is native JSON; read it with [`crate::native::load`].
pub fn extract_scene(svg: &str) -> Result<Option<String>, IoError> {
    use std::io::Read as _;
    let open = format!("<{SCENE_TAG}");
    let Some(start) = svg.find(&open) else { return Ok(None) };
    let rest = svg.get(start..).unwrap_or("");
    let Some(gt) = rest.find('>') else { return Ok(None) };
    let tag = rest.get(..gt).unwrap_or("");
    let close = format!("</{SCENE_TAG}>");
    let Some(end) = rest.find(&close) else { return Ok(None) };
    let body = rest.get(gt + 1..end).unwrap_or("").trim();
    if tag.contains("encoding=\"base64-deflate\"") {
        let raw = STANDARD
            .decode(body)
            .map_err(|e| IoError::parse(FormatName::Svg, format!("the embedded scene is not base64: {e}")))?;
        let mut out = Vec::new();
        let limit = u64::try_from(MAX_EMBEDDED_SCENE_BYTES).unwrap_or(u64::MAX);
        flate2::read::ZlibDecoder::new(raw.as_slice())
            .take(limit.saturating_add(1))
            .read_to_end(&mut out)
            .map_err(|e| IoError::parse(FormatName::Svg, format!("the embedded scene does not inflate: {e}")))?;
        if out.len() > MAX_EMBEDDED_SCENE_BYTES {
            return Err(IoError::Limit { what: "embedded scene", max: format!("{MAX_EMBEDDED_SCENE_BYTES} bytes") });
        }
        return String::from_utf8(out)
            .map(Some)
            .map_err(|_| IoError::parse(FormatName::Svg, "the embedded scene is not UTF-8"));
    }
    Ok(None)
}
