//! Vello backend for ReX math rendering with font fallback.

use kurbo::{Affine, BezPath, Point};
use peniko::Color;
use rex::font::backend::ttf_parser::TtfMathFont;
use rex::font::common::GlyphId;
use rex::render::{Backend, Cursor, FontBackend, GraphicsBackend, RGBA};
use std::collections::HashMap;
use vello::Scene;

/// Map Unicode math alphanumeric symbols to ASCII equivalents.
fn math_to_ascii(c: char) -> Option<char> {
    let cp = c as u32;
    match cp {
        // Math Italic Capital A-Z (U+1D434-1D44D)
        0x1D434..=0x1D44D => Some((b'A' + (cp - 0x1D434) as u8) as char),
        // Math Italic Small a-z (U+1D44E-1D467, hole at U+1D455 for 'h')
        0x1D44E..=0x1D454 => Some((b'a' + (cp - 0x1D44E) as u8) as char), // a-g
        0x1D456..=0x1D467 => Some((b'a' + (cp - 0x1D456 + 8) as u8) as char), // i-z
        0x210E => Some('h'), // Planck constant (math italic h)
        // Math Bold Capital A-Z (U+1D400-1D419)
        0x1D400..=0x1D419 => Some((b'A' + (cp - 0x1D400) as u8) as char),
        // Math Bold Small a-z (U+1D41A-1D433)
        0x1D41A..=0x1D433 => Some((b'a' + (cp - 0x1D41A) as u8) as char),
        // Math Bold Italic Capital A-Z (U+1D468-1D481)
        0x1D468..=0x1D481 => Some((b'A' + (cp - 0x1D468) as u8) as char),
        // Math Bold Italic Small a-z (U+1D482-1D49B)
        0x1D482..=0x1D49B => Some((b'a' + (cp - 0x1D482) as u8) as char),
        // Math Sans Capital A-Z (U+1D5A0-1D5B9)
        0x1D5A0..=0x1D5B9 => Some((b'A' + (cp - 0x1D5A0) as u8) as char),
        // Math Sans Small a-z (U+1D5BA-1D5D3)
        0x1D5BA..=0x1D5D3 => Some((b'a' + (cp - 0x1D5BA) as u8) as char),
        // Math Monospace Capital A-Z (U+1D670-1D689)
        0x1D670..=0x1D689 => Some((b'A' + (cp - 0x1D670) as u8) as char),
        // Math Monospace Small a-z (U+1D68A-1D6A3)
        0x1D68A..=0x1D6A3 => Some((b'a' + (cp - 0x1D68A) as u8) as char),
        // Digits (various styles)
        0x1D7CE..=0x1D7D7 => Some((b'0' + (cp - 0x1D7CE) as u8) as char),
        0x1D7D8..=0x1D7E1 => Some((b'0' + (cp - 0x1D7D8) as u8) as char),
        0x1D7E2..=0x1D7EB => Some((b'0' + (cp - 0x1D7E2) as u8) as char),
        0x1D7EC..=0x1D7F5 => Some((b'0' + (cp - 0x1D7EC) as u8) as char),
        0x1D7F6..=0x1D7FF => Some((b'0' + (cp - 0x1D7F6) as u8) as char),
        _ => None,
    }
}

/// Vello backend for ReX rendering with primary font fallback.
pub struct VelloBackend<'a, 'f, 'p> {
    scene: &'a mut Scene,
    math_font: &'f TtfMathFont<'f>,
    primary_font: Option<&'p ttf_parser::Face<'p>>,
    /// Maps math font glyph IDs to codepoints for fallback lookup.
    glyph_to_codepoint: HashMap<u16, char>,
    transform: Affine,
    color_stack: Vec<Color>,
    current_color: Color,
}

impl<'a, 'f, 'p> VelloBackend<'a, 'f, 'p> {
    pub fn new(
        scene: &'a mut Scene,
        math_font: &'f TtfMathFont<'f>,
        primary_font: Option<&'p ttf_parser::Face<'p>>,
        transform: Affine,
        color: Color,
    ) -> Self {
        // Build reverse map from glyph ID to codepoint
        let mut glyph_to_codepoint = HashMap::new();
        if primary_font.is_some() {
            for subtable in math_font
                .font()
                .tables()
                .cmap
                .iter()
                .flat_map(|c| c.subtables)
            {
                if subtable.is_unicode() {
                    subtable.codepoints(|cp| {
                        if let Some(c) = char::from_u32(cp) {
                            if let Some(gid) = subtable.glyph_index(cp) {
                                glyph_to_codepoint.insert(gid.0, c);
                            }
                        }
                    });
                }
            }
        }

        Self {
            scene,
            math_font,
            primary_font,
            glyph_to_codepoint,
            transform,
            color_stack: Vec::new(),
            current_color: color,
        }
    }
}

struct PathBuilder(BezPath);

impl ttf_parser::OutlineBuilder for PathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(Point::new(x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(Point::new(x as f64, y as f64));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(
            Point::new(x1 as f64, y1 as f64),
            Point::new(x as f64, y as f64),
        );
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.curve_to(
            Point::new(x1 as f64, y1 as f64),
            Point::new(x2 as f64, y2 as f64),
            Point::new(x as f64, y as f64),
        );
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

impl<'f, 'p> FontBackend<TtfMathFont<'f>> for VelloBackend<'_, 'f, 'p> {
    fn symbol(&mut self, pos: Cursor, gid: GlyphId, scale: f64, _ctx: &TtfMathFont<'f>) {
        // Try primary font first if available
        if let Some(primary) = self.primary_font {
            if let Some(&codepoint) = self.glyph_to_codepoint.get(&gid.into()) {
                // Map math italic/bold Unicode to ASCII for primary font lookup
                let lookup_char = math_to_ascii(codepoint).unwrap_or(codepoint);
                if let Some(primary_gid) = primary.glyph_index(lookup_char) {
                    // Use primary font (slightly smaller to match text tool rendering)
                    let units_per_em = primary.units_per_em() as f64;
                    let adjusted_scale = scale * 0.75;
                    let glyph_transform = self.transform
                        * Affine::translate(kurbo::Vec2::new(pos.x, pos.y))
                        * Affine::scale_non_uniform(
                            adjusted_scale / units_per_em,
                            -adjusted_scale / units_per_em,
                        );

                    let mut builder = PathBuilder(BezPath::new());
                    if primary.outline_glyph(primary_gid, &mut builder).is_some() {
                        self.scene.fill(
                            vello::peniko::Fill::NonZero,
                            glyph_transform,
                            self.current_color,
                            None,
                            &builder.0,
                        );
                        return;
                    }
                }
            }
        }

        // Fallback to math font
        let ttf_parser::cff::Matrix {
            sx,
            ky,
            kx,
            sy,
            tx,
            ty,
        } = self.math_font.font_matrix();
        let font_matrix = Affine::new([
            sx as f64, ky as f64, kx as f64, sy as f64, tx as f64, ty as f64,
        ]);

        let glyph_transform = self.transform
            * Affine::translate(kurbo::Vec2::new(pos.x, pos.y))
            * Affine::scale_non_uniform(scale, -scale)
            * font_matrix;

        let mut builder = PathBuilder(BezPath::new());
        self.math_font
            .font()
            .outline_glyph(gid.into(), &mut builder);

        self.scene.fill(
            vello::peniko::Fill::NonZero,
            glyph_transform,
            self.current_color,
            None,
            &builder.0,
        );
    }
}

impl GraphicsBackend for VelloBackend<'_, '_, '_> {
    fn rule(&mut self, pos: Cursor, width: f64, height: f64) {
        let rect = kurbo::Rect::new(pos.x, pos.y, pos.x + width, pos.y + height);
        self.scene.fill(
            vello::peniko::Fill::NonZero,
            self.transform,
            self.current_color,
            None,
            &rect,
        );
    }

    fn begin_color(&mut self, RGBA(r, g, b, a): RGBA) {
        self.color_stack.push(self.current_color);
        self.current_color = Color::from_rgba8(r, g, b, a);
    }

    fn end_color(&mut self) {
        if let Some(color) = self.color_stack.pop() {
            self.current_color = color;
        }
    }
}

impl<'f, 'p> Backend<TtfMathFont<'f>> for VelloBackend<'_, 'f, 'p> {}
