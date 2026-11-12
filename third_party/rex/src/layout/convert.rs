//! This is a collection of tools used for converting ParseNodes into LayoutNodes.

use crate::font::{Glyph, Direction, VariantGlyph, MathFont};
use crate::dimensions::{Unit, AnyUnit};
use crate::dimensions::units::{Em, FUnit, Px, Ratio};

use super::engine::{LayoutContext, LayoutEngine};
use super::Style;
use super::builders;
use super::{LayoutNode, LayoutVariant, LayoutGlyph};
use crate::parser::nodes::Rule;
use crate::error::LayoutResult;

pub trait AsLayoutNode<'f, F> {
    fn as_layout<'a>(&self, engine: &LayoutEngine<'f, F>, context: LayoutContext) -> LayoutResult<LayoutNode<'f, F>>;
}

impl<'f, F> AsLayoutNode<'f, F> for Glyph<'f, F> {
    fn as_layout<'a>(&self, engine: &LayoutEngine<'f, F>, context: LayoutContext) -> LayoutResult<LayoutNode<'f, F>> {
        Ok(LayoutNode {
            height: self.height().to_px(engine, context),
            width:  self.advance.to_px(engine, context),
            depth:  self.depth().to_px(engine, context),
            node:   LayoutVariant::Glyph(LayoutGlyph {
                font: self.font,
                gid: self.gid,
                size: Unit::<Em>::new(1.0).to_px(engine, context),
                attachment: self.attachment.to_px(engine, context),
                italics: self.italics.to_px(engine, context),
                offset:  Unit::ZERO,
            })
        })
    }
}

impl<'f, F> AsLayoutNode<'f, F> for Rule {
    fn as_layout<'a>(&self, engine: &LayoutEngine<'f, F>, context: LayoutContext) -> LayoutResult<LayoutNode<'f, F>> {
        Ok(LayoutNode {
            node:   LayoutVariant::Rule,
            width:  self.width.to_px(engine, context),
            height: self.height.to_px(engine, context),
            depth:  Unit::ZERO,
        })
    }
}

impl<'f, F : MathFont> AsLayoutNode<'f, F> for VariantGlyph {
    fn as_layout<'a>(&self, engine: &LayoutEngine<'f, F>, context: LayoutContext) -> LayoutResult<LayoutNode<'f, F>> {
        match *self {
            VariantGlyph::Replacement(gid) => {
                let glyph = engine.font().glyph_from_gid(gid)?;
                glyph.as_layout(engine, context)
            },

            VariantGlyph::Constructable(dir, ref parts) => {
                match dir {
                    Direction::Vertical => {
                        let mut contents = builders::VBox::new();
                        for instr in parts.into_iter().rev() {
                            let glyph = engine.font().glyph_from_gid(instr.gid)?;
                            contents.add_node(glyph.as_layout(engine, context)?);
                            if instr.overlap != 0 {
                                let overlap = Unit::<FUnit>::new(instr.overlap.into());
                                let kern = -(overlap + glyph.depth()).to_px(engine, context);
                                contents.add_node(LayoutNode::vert_kern(kern))
                            }
                        }

                        Ok(contents.build())
                    },

                    Direction::Horizontal => {
                        let mut contents = builders::HBox::new();
                        for instr in parts {
                            let glyph = engine.font().glyph_from_gid(instr.gid)?;
                            if instr.overlap != 0 {
                                let kern = -Unit::<FUnit>::new(instr.overlap.into()).to_px(engine, context);
                                contents.add_node(LayoutNode::horiz_kern(kern));
                            }
                            contents.add_node(glyph.as_layout(engine, context)?);
                        }

                        Ok(contents.build())
                    }
                }
            },
        }
    }
}

impl<'f, F> LayoutEngine<'f, F> {
    fn scale_factor(&self, style : Style) -> f64 {
        match style {
            Style::Display |
            Style::DisplayCramped |
            Style::Text |
            Style::TextCramped
                => 1.0,

            Style::Script |
            Style::ScriptCramped
                => self.metrics_cache().constants().script_percent_scale_down,

            Style::ScriptScript |
            Style::ScriptScriptCramped
                => self.metrics_cache().constants().script_script_percent_scale_down,
        }
    }
    fn scale_font_unit(&self, length: Unit<FUnit>, font_size : Unit<Ratio<Px, Em>>) -> Unit<Px> {
        length * (font_size / self.metrics_cache().units_per_em()).unlift()
    }

    /// Convert a length given in pixels to a length in font units. The resulting value depends on the selected font size.
    pub(super) fn to_font(&self, length: Unit<Px>, font_size : Unit<Ratio<Px, Em>>) -> Unit<FUnit> {
        length  * (self.metrics_cache().units_per_em() / font_size).unlift()
    }
}
pub trait ToPx {
    fn to_px<F>(self, engine: &LayoutEngine<F>, context : LayoutContext) -> Unit<Px>;
}

impl ToPx for Unit<FUnit> {
    fn to_px<F>(self, engine: &LayoutEngine<F>, context : LayoutContext) -> Unit<Px> {
        engine.scale_font_unit(self, context.font_size).scale(engine.scale_factor(context.style))
    }
}

impl ToPx for Unit<Px> {
    fn to_px<F>(self, engine: &LayoutEngine<F>, context : LayoutContext) -> Unit<Px> {
        self.scale(engine.scale_factor(context.style))
    }
}
impl ToPx for Unit<Em> {
    fn to_px<F>(self, engine: &LayoutEngine<F>, context : LayoutContext) -> Unit<Px> {
        (self * context.font_size).scale(engine.scale_factor(context.style))
    }
}
impl ToPx for AnyUnit {
    fn to_px<F>(self, engine: &LayoutEngine<F>, context : LayoutContext) -> Unit<Px> {
        let length = match self {
            AnyUnit::Em(em) => Unit::<Em>::new(em) * context.font_size,
            AnyUnit::Px(px) => Unit::<Px>::new(px)
        };
        length.scale(engine.scale_factor(context.style))
    }
}
