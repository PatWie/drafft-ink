//! Glyph outlines as filled paths, for exports (SVG, PNG without a glyph renderer) and tests.

use kurbo::{Affine, BezPath, Point, Rect, Shape};

use crate::layout::TextLayout;
use crate::math::MathLayout;
use crate::style::Rgba8;

struct PathBuilder(BezPath);

impl ttf_parser::OutlineBuilder for PathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(Point::new(f64::from(x), f64::from(y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(Point::new(f64::from(x), f64::from(y)));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(Point::new(f64::from(x1), f64::from(y1)), Point::new(f64::from(x), f64::from(y)));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.curve_to(
            Point::new(f64::from(x1), f64::from(y1)),
            Point::new(f64::from(x2), f64::from(y2)),
            Point::new(f64::from(x), f64::from(y)),
        );
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

/// Outline of glyph `id` in font units, y up. `None` when the font has no outline for it.
fn glyph_path(face: &ttf_parser::Face<'_>, id: u32) -> Option<(BezPath, f64)> {
    let gid = ttf_parser::GlyphId(u16::try_from(id).ok()?);
    let mut b = PathBuilder(BezPath::new());
    face.outline_glyph(gid, &mut b)?;
    Some((b.0, f64::from(face.units_per_em().max(1))))
}

/// One path of an outline with the colour it is filled with.
#[derive(Clone, Debug, PartialEq)]
pub struct ColouredPath {
    /// The fill colour.
    pub color: Rgba8,
    /// The glyph outlines, rules and decorations of this colour, in layout space.
    pub path: BezPath,
}

/// Add colour runs: consecutive paths of the same colour share one `BezPath`.
fn push(out: &mut Vec<ColouredPath>, color: Rgba8, path: &BezPath) {
    match out.last_mut() {
        Some(last) if last.color == color => last.path.extend(path.elements().iter().copied()),
        _ => out.push(ColouredPath { color, path: path.clone() }),
    }
}

pub(crate) fn text_outline(l: &TextLayout) -> Vec<ColouredPath> {
    let mut out = Vec::new();
    // Paint order: each run's glyphs, then decorations on top.
    for run in &l.runs {
        let Ok(mut face) = ttf_parser::Face::parse(run.font.data.as_ref(), run.font.index) else { continue };
        for (axis, value) in &run.variations {
            // A font without the axis ignores the setting.
            let _ = face.set_variation(ttf_parser::Tag::from_bytes(axis), *value);
        }
        let mut acc = BezPath::new();
        for g in &run.glyphs {
            let Some((path, upem)) = glyph_path(&face, g.id) else { continue };
            let s = f64::from(run.font_size) / upem;
            let skew = run.skew.map_or(Affine::IDENTITY, |k| Affine::skew(f64::from(k), 0.0));
            // font units (y up) -> layout space (y down): scale, flip, shear about the baseline, translate.
            let t = Affine::translate((f64::from(g.x), f64::from(g.y))) * skew * Affine::scale_non_uniform(s, -s);
            acc.extend((t * path).elements().iter().copied());
        }
        if !acc.elements().is_empty() {
            push(&mut out, run.color, &acc);
        }
    }
    for f in &l.formulas {
        let to = Affine::translate(f.origin.to_vec2());
        for ColouredPath { color, path } in math_outline(&f.math, f.color) {
            push(&mut out, color, &(to * path));
        }
    }
    for d in &l.decorations {
        push(&mut out, d.color, &d.rect.to_path(0.1));
    }
    out
}

pub(crate) fn math_outline(l: &MathLayout, base: Rgba8) -> Vec<ColouredPath> {
    let mut out = Vec::new();
    let faces = (
        ttf_parser::Face::parse(l.math_font.data.as_ref(), l.math_font.index).ok(),
        ttf_parser::Face::parse(l.text_font.data.as_ref(), l.text_font.index).ok(),
    );
    for g in &l.glyphs {
        let face = match g.font {
            crate::math::MathFontSlot::Math => faces.0.as_ref(),
            crate::math::MathFontSlot::Text => faces.1.as_ref(),
        };
        let Some(face) = face else { continue };
        let Some((path, upem)) = glyph_path(face, u32::from(g.id)) else { continue };
        let s = g.size / upem;
        let t = Affine::translate((g.x, g.y)) * Affine::scale_non_uniform(s, -s);
        push(&mut out, g.color.unwrap_or(base), &(t * path));
    }
    for r in &l.rules {
        let rect: Rect = r.rect;
        push(&mut out, r.color.unwrap_or(base), &rect.to_path(0.1));
    }
    out
}
