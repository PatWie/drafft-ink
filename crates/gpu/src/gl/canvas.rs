//! A [`Canvas`] over `vello_gpu`'s scene.

use std::collections::HashMap;

use glifo::Glyph as GpuGlyph;
use vello::kurbo::{Affine, BezPath, Stroke};
use vello::peniko::Fill;
use vello_gpu::color::{AlphaColor, Srgb};
use vello_gpu::peniko::ImageBrush;
use vello_gpu::{ImageId, ImageSource, Resources, Scene};

use super::replay::{Brush, Canvas, GlyphDraw, LayerKind, Rgba};

/// The atlas image of each picture, by the id of its pixel blob. `None`: left out.
pub(crate) type Images = HashMap<u64, Option<ImageId>>;

/// Draws into a `vello_gpu` scene.
pub(crate) struct GpuCanvas<'a> {
    pub scene: &'a mut Scene,
    pub resources: &'a mut Resources,
    pub images: &'a Images,
}

fn color(c: Rgba) -> AlphaColor<Srgb> {
    AlphaColor::new(c.0)
}

impl GpuCanvas<'_> {
    /// Sets the paint. False when there is nothing to draw (a picture that could not be uploaded).
    fn paint(&mut self, transform: Affine, brush: Brush<'_>) -> bool {
        self.scene.set_transform(transform);
        match brush {
            Brush::Solid(c) => self.scene.set_paint(color(c)),
            Brush::Image { data, sampler, to_path } => {
                let Some(&Some(id)) = self.images.get(&data.data.id()) else { return false };
                let brush = ImageBrush { image: ImageSource::opaque_id(id), sampler };
                self.scene.set_paint(brush);
                self.scene.set_paint_transform(to_path);
            }
        }
        true
    }
}

impl Canvas for GpuCanvas<'_> {
    fn fill(&mut self, transform: Affine, path: &BezPath, rule: Fill, brush: Brush<'_>) {
        if self.paint(transform, brush) {
            self.scene.set_fill_rule(rule);
            self.scene.fill_path(path);
            self.scene.reset_paint_transform();
        }
    }

    fn stroke(&mut self, transform: Affine, path: &BezPath, stroke: &Stroke, brush: Brush<'_>) {
        if self.paint(transform, brush) {
            self.scene.set_stroke(stroke.clone());
            self.scene.stroke_path(path);
            self.scene.reset_paint_transform();
        }
    }

    fn push_layer(&mut self, transform: Affine, clip: &BezPath, kind: LayerKind) {
        self.scene.set_transform(transform);
        self.scene.set_fill_rule(Fill::NonZero);
        match kind {
            LayerKind::Clip => self.scene.push_clip_layer(clip),
            LayerKind::Opacity(alpha) => self.scene.push_layer(Some(clip), None, Some(alpha), None, None),
        }
    }

    fn pop_layer(&mut self) {
        self.scene.pop_layer();
    }

    fn glyph_run(&mut self, draw: &GlyphDraw<'_>) {
        let run = draw.run;
        self.scene.set_transform(run.transform.to_kurbo());
        self.scene.set_paint(color(draw.color));
        let glyphs = draw.glyphs.iter().map(|g| GpuGlyph { id: g.id, x: g.x, y: g.y });
        let mut builder = self.scene.glyph_run(self.resources, &run.font).font_size(run.font_size).hint(run.hint);
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
