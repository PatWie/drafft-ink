//! Vello backend for ReX math rendering.

use kurbo::{Affine, BezPath, Point};
use peniko::Color;
use rex::font::backend::ttf_parser::TtfMathFont;
use rex::font::common::GlyphId;
use rex::render::{Backend, Cursor, FontBackend, GraphicsBackend, RGBA};
use vello::Scene;

/// Vello backend for ReX rendering.
pub struct VelloBackend<'a, 'f> {
    scene: &'a mut Scene,
    font: &'f TtfMathFont<'f>,
    transform: Affine,
    color_stack: Vec<Color>,
    current_color: Color,
}

impl<'a, 'f> VelloBackend<'a, 'f> {
    pub fn new(scene: &'a mut Scene, font: &'f TtfMathFont<'f>, transform: Affine, color: Color) -> Self {
        Self {
            scene,
            font,
            transform,
            color_stack: Vec::new(),
            current_color: color,
        }
    }
}

impl<'f> FontBackend<TtfMathFont<'f>> for VelloBackend<'_, 'f> {
    fn symbol(&mut self, pos: Cursor, gid: GlyphId, scale: f64, _ctx: &TtfMathFont<'f>) {
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

        let ttf_parser::cff::Matrix { sx, ky, kx, sy, tx, ty } = self.font.font_matrix();
        let font_matrix = Affine::new([sx as f64, ky as f64, kx as f64, sy as f64, tx as f64, ty as f64]);
        
        // Build glyph transform: font matrix -> scale + flip Y -> translate to position -> canvas transform
        let glyph_transform = self.transform
            * Affine::translate(kurbo::Vec2::new(pos.x, pos.y))
            * Affine::scale_non_uniform(scale, -scale)
            * font_matrix;

        let mut builder = PathBuilder(BezPath::new());
        self.font.font().outline_glyph(gid.into(), &mut builder);

        self.scene.fill(
            vello::peniko::Fill::NonZero,
            glyph_transform,
            self.current_color,
            None,
            &builder.0,
        );
    }
}

impl GraphicsBackend for VelloBackend<'_, '_> {
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

impl<'f> Backend<TtfMathFont<'f>> for VelloBackend<'_, 'f> {}
