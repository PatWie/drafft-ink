//! A [`Canvas`] over `vello_cpu`'s render context.

use std::collections::HashMap;
use std::sync::Arc;

use drafftink_geom::kurbo::{Affine, BezPath, Stroke};
use drafftink_render::Scene;
use drafftink_render::replay::{self, Brush, Canvas, GlyphDraw, LayerKind, Rgba};
use peniko::{Fill, ImageData, ImageFormat};
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::{Glyph, Image, ImageSource, Pixmap, RenderContext, Resources};

/// The pixels of each picture a scene draws, by the id of its pixel blob.
pub(super) type Pictures = HashMap<u64, Arc<Pixmap>>;

/// Decodes the pictures `scene` draws into pixmaps. A picture `vello_cpu` cannot hold (too large, or in a pixel format the
/// app never makes) is left out, and so is the shape it fills.
pub(super) fn pictures_of(scene: &Scene) -> Pictures {
    replay::pictures(scene).filter_map(|image| Some((image.data.id(), pixmap_of(image)?))).collect()
}

/// A premultiplied pixmap of the picture, `None` when it cannot be held.
fn pixmap_of(image: &ImageData) -> Option<Arc<Pixmap>> {
    let fits = |side: u32| (1..=u32::from(u16::MAX)).contains(&side);
    if !fits(image.width) || !fits(image.height) || !matches!(image.format, ImageFormat::Rgba8 | ImageFormat::Bgra8) {
        return None;
    }
    match ImageSource::from_peniko_image_data(image) {
        ImageSource::Pixmap(pixmap) => Some(pixmap),
        _ => None,
    }
}

/// Draws into a `vello_cpu` context.
pub(super) struct CpuCanvas<'a> {
    pub ctx: &'a mut RenderContext,
    pub resources: &'a mut Resources,
    pub pictures: &'a Pictures,
}

fn color(c: Rgba) -> AlphaColor<Srgb> {
    AlphaColor::new(c.0)
}

impl CpuCanvas<'_> {
    /// Sets the paint. False when there is nothing to draw (a picture that could not be held).
    fn paint(&mut self, transform: Affine, brush: Brush<'_>) -> bool {
        self.ctx.set_transform(transform);
        match brush {
            Brush::Solid(c) => self.ctx.set_paint(color(c)),
            Brush::Image { data, sampler, to_path } => {
                let Some(pixmap) = self.pictures.get(&data.data.id()) else { return false };
                self.ctx.set_paint(Image { image: ImageSource::Pixmap(Arc::clone(pixmap)), sampler });
                self.ctx.set_paint_transform(to_path);
            }
        }
        true
    }
}

impl Canvas for CpuCanvas<'_> {
    fn fill(&mut self, transform: Affine, path: &BezPath, rule: Fill, brush: Brush<'_>) {
        if self.paint(transform, brush) {
            self.ctx.set_fill_rule(rule);
            self.ctx.fill_path(path);
            self.ctx.reset_paint_transform();
        }
    }

    fn stroke(&mut self, transform: Affine, path: &BezPath, stroke: &Stroke, brush: Brush<'_>) {
        if self.paint(transform, brush) {
            self.ctx.set_stroke(stroke.clone());
            self.ctx.stroke_path(path);
            self.ctx.reset_paint_transform();
        }
    }

    fn push_layer(&mut self, transform: Affine, clip: &BezPath, kind: LayerKind) {
        self.ctx.set_transform(transform);
        self.ctx.set_fill_rule(Fill::NonZero);
        match kind {
            LayerKind::Clip => self.ctx.push_clip_layer(clip),
            LayerKind::Opacity(alpha) => self.ctx.push_layer(Some(clip), None, Some(alpha), None, None),
        }
    }

    fn pop_layer(&mut self) {
        self.ctx.pop_layer();
    }

    fn glyph_run(&mut self, draw: &GlyphDraw<'_>) {
        let run = draw.run;
        self.ctx.set_transform(run.transform.to_kurbo());
        self.ctx.set_paint(color(draw.color));
        let glyphs = draw.glyphs.iter().map(|g| Glyph { id: g.id, x: g.x, y: g.y });
        let mut builder = self.ctx.glyph_run(self.resources, &run.font).font_size(run.font_size).hint(run.hint);
        if !draw.coords.is_empty() {
            builder = builder.normalized_coords(draw.coords);
        }
        if let Some(t) = run.glyph_transform {
            builder = builder.glyph_transform(t.to_kurbo());
        }
        if let Err(e) = builder.fill_glyphs(glyphs) {
            log::debug!("glyph run not drawn: {e:?}");
        }
    }
}
