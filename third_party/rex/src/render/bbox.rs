//! Defines a renderer that does not draw anything but computes the "real bbox", the one the encloses all areas actually drawn to.
//! Character (typically in italic style) routinely go beyond the bounding box as defined in the font. 
//! To determine the real bounding box, you can perform a renderer with the backend defined in this module.


use crate::{dimensions::{units::{Em, FUnit, Px, Ratio}, Unit}, font::{common::GlyphId, MathFont}, geometry::BBox};

use super::{Backend, FontBackend, GraphicsBackend};



/// A rendering backend that does not draw but simply records the bouding box being drawn to
#[derive(Debug, Clone)]
pub struct BBoxBackend {
    /// The current bounding box
    /// Is None when nothing has been drawn yet
    bbox: Option<BBox<Px>>,
}

impl Default for BBoxBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl BBoxBackend {
    /// Creates a new bbox rendering backend.
    pub fn new() -> Self {
        Self { 
            bbox: None,
        }
    }

    /// Returns the bounding box computed by the backend.
    /// Return None when nothing has be drawn
    pub fn finish(self) -> Option<BBox<Px>> {
        self.bbox
    }

    fn enclose(&mut self, mut bbox: BBox<Px>) {
        if let Some(other) = self.bbox.as_ref() {
            bbox = bbox.union(other.clone());
        }
        self.bbox = Some(bbox)
    }
}

impl GraphicsBackend for BBoxBackend {
    fn rule(&mut self, pos: super::Cursor, width: f64, height: f64) {
        self.enclose(BBox::from_dims(
            Unit::new(pos.x), Unit::new(pos.y),
            Unit::new(width), Unit::new(height),
        ));
    }

    fn begin_color(&mut self, _color: super::RGBA) {} 
    fn end_color(&mut self) {}
}

impl<F : MathFont> FontBackend<F> for BBoxBackend {
    fn symbol(&mut self, pos: crate::Cursor, gid: GlyphId, scale: f64, ctx: &F) {

        let scale = Unit::<Ratio<Px, Em>>::new(scale); 
        let em_per_funits = ctx.font_units_to_em();
        let (x_min, y_min, x_max, y_max) = ctx.glyph_from_gid(gid).unwrap().bbox;

        let font_bbox = BBox::<FUnit>::new(x_min, -y_max, x_max, -y_min);

        let x = Unit::<Px>::new(pos.x);
        let y = Unit::<Px>::new(pos.y);

        let funits_to_px: Unit<Ratio<Px, FUnit>> = em_per_funits * scale.lift();
        let bbox : BBox<Px> = 
            font_bbox
            .scale(funits_to_px)
            .translate(x, y)
        ;

        self.enclose(bbox);
    }
}

impl<F : MathFont> Backend<F> for BBoxBackend {}
