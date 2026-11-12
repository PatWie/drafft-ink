//! This module defines the main layout functions that will place the parse nodes in space, given the geometrical information provided in the font. 
//! The most important function here is [`layout`](crate::layout::engine::layout). 
//! Given a slice of [`ParseNode`s](crate::parser::nodes::ParseNode) and some [`LayoutSettings`](crate::layout::LayoutSettings), 
//! this function returns a layout. The layout can then be sent to the renderer (cf [`render`](crate::render)) to create a graphical output.


use std::unimplemented;

use super::convert::AsLayoutNode;
use super::kern::{subscript_kern, superscript_kern};
use super::{builders, Alignment, Layout, LayoutNode, LayoutVariant, Style};

use crate::font::{FontMetricsCache, MathFont};
use crate::font::{
    VariantGlyph,
    TexSymbolType
};
use crate::layout::builders::VBox;
use crate::layout::constants::{BASELINE_SKIP, COLUMN_SEP, DOUBLE_RULE_SEP, JOT, LINE_SKIP_ARRAY, LINE_SKIP_LIMIT_ARRAY, RULE_WIDTH, STRUT_DEPTH, STRUT_HEIGHT};
use super::convert::ToPx;
use super::spacing::{atom_space, Spacing};
use crate::parser::nodes::{Accent, Array, ArrayColumnAlign, BarThickness, ColSeparator, Delimited, ExtendedDelimiter, FontEffect, GenFraction, MathStyle, ParseNode, PlainText, Radical, Scripts, Stack};
use crate::parser::symbols::Symbol;
use crate::dimensions::Unit;
use crate::dimensions::units::{Em, FontSize, Px, Ratio, FUnit};
use crate::layout;
use crate::error::{FontError, LayoutError, LayoutResult};



/// Default font size used when none is provided: 12 pt.em-1
pub const DEFAULT_FONT_SIZE : Unit<FontSize> = Unit::new(12.);

/// This struct will hold all parameters needed for layout, namely:
///   - a font
///   - a font size
///   - layout style (e.g. [`Style::Text`] for `$..$` or [`Style::Display`] for `$$..$$`)
/// 
/// Font size and layout style have default values so only the font is required to create this struct.
pub struct LayoutBuilder<'f, F> {
    font      : & 'f F,
    font_size : Unit<FontSize>,
    style     : Style,
}

impl<'f, F> LayoutBuilder<'f, F> {
    /// Creates a new [`LayoutEngineBuilder`]. 
    pub fn new(font : & 'f F) -> Self {
        Self {
            font,
            font_size: DEFAULT_FONT_SIZE,
            style:     Style::default(),
        }
    }

    /// Sets the font size (in pt . em -1) 
    pub fn font_size(mut self, font_size : f64) -> Self {
        self.font_size = Unit::new(font_size);
        self
    }

    /// Sets the formula's display style (e.g. [`Style::Text`] for `$..$` or [`Style::Display`] for `$$..$$`)
    pub fn style(mut self, style : Style) -> Self {
        self.style = style;
        self
    }


}

impl<'f, F : MathFont> LayoutBuilder<'f, F> {

    /// Creates a new LayoutEngine. The [`LayoutEngine`] may be used to lay out multiple formula with the same format.
    pub fn build(self) -> LayoutEngine<'f, F> {
        let Self { font, font_size, style } = self;

        LayoutEngine::new(
            font, 
            LayoutContext { 
                style:       style, 
                font_size:   font_size * Unit::standard_pt_to_px().lift(), 
                parent_next: TexSymbolType::Transparent,
            }
        )
    }


    /// Lays out the nodes given, according to the font, font size and style specified in `self`.  
    /// If you need to lay out multiple formulas (i.e. slices of nodes) with the same settings, 
    /// consider building using the layout engine, cf [`LayoutBuilder::build`]
    pub fn layout(self, nodes : &[ParseNode]) -> LayoutResult<Layout<'f, F>> {
        let layout_engine = self.build();
        layout_engine.layout(nodes)
    }
}

// NOTE: A limitation on derive(Clone) forces us to implement clone ourselves.
// cf discussion here: https://stegosaurusdormant.com/understanding-derive-clone/
/// Defines the math font to use, the desired font size and whether to use Roman or Italic or various other scripts
pub struct LayoutEngine<'f, F> {
    /// Maths font
    font: & 'f F,

    /// The context at beginning of layout as defined by the user
    starting_context : LayoutContext,


    /// Contains frequently used constants from the font
    metrics_cache: FontMetricsCache,
}


/// Contains every element of state that may change during the layout, i.e. font size or display style
#[derive(Debug, Clone, Copy)]
pub struct LayoutContext {
    /// Sizes of glyphs : normal, subscript size, superscript size
    pub style: Style,
    /// Font size in pixels per em (this is private: all user-facing interfaces should use a more conventional pt . em-1 unit)
    pub font_size: Unit<Ratio<Px, Em>>,
    /// Parent's next symbol type
    pub parent_next : TexSymbolType,
}


impl<'f, F> Clone for LayoutEngine<'f, F> {
    fn clone(&self) -> Self {
        let Self { font, metrics_cache, starting_context } = self;
        Self {
            font :         font,
            metrics_cache: metrics_cache.clone(),
            starting_context: starting_context.clone(),
        }
    }
}


impl<'f, F> LayoutEngine<'f, F> {

    /// Access constants associated with font
    pub fn metrics_cache(&self) -> &FontMetricsCache {
        &self.metrics_cache
    }
}


impl<'f, F : MathFont> LayoutEngine<'f, F> {
    /// Creates new layout engine from font and starting context
    fn new(font: & 'f F, starting_context : LayoutContext) -> Self {
        Self {
            font,
            metrics_cache: FontMetricsCache::new(font),
            starting_context,
        }
    }

    /// Lays out the nodes
    pub fn layout(&self, nodes : &[ParseNode]) -> LayoutResult<Layout<'f, F>> {
        self.layout_with(nodes, self.starting_context)
    }

    /// The recursive auxiliary function that lays out node
    fn layout_with(&self, nodes: &[ParseNode], mut context: LayoutContext) -> LayoutResult<Layout<'f, F>> {
        let mut layout = Layout::new();
        let mut prev = None;
        let mut italic_correction = None;

        for idx in 0..nodes.len() {
            let node = &nodes[idx];

            // To determine spacing between glyphs, we look at each pair and their types.
            // Obtain the atom_type from the next node,  if we are the last in the node
            // list then we obtain the atomtype from the next node in parent's list.
            let next = match nodes.get(idx + 1) {
                Some(node) => node.atom_type(),
                None => context.parent_next,
            };

            let mut current = node.atom_type();
            if current == TexSymbolType::Binary {
                if let 
                      None 
                    | Some(TexSymbolType::Binary) 
                    | Some(TexSymbolType::Relation) 
                    | Some(TexSymbolType::Open) 
                    | Some(TexSymbolType::Punctuation) 
                    | Some(TexSymbolType::Operator(_)) = prev {
                    current = TexSymbolType::Alpha;
                } else if let 
                      TexSymbolType::Relation 
                    | TexSymbolType::Close 
                    | TexSymbolType::Punctuation = next {
                    current = TexSymbolType::Alpha;
                }
            }

            let sp = 
                if let Some(prev) = prev 
                { atom_space(prev, current, context.style) }
                else 
                { Spacing::None }
            ;

            let italic_correction_to_apply = italic_correction.take();
            if sp != Spacing::None {
                let kern = sp.to_length().to_px(self, context);
                layout.add_node(LayoutNode::horiz_kern(kern));
            }
            // if there is already no space between consecutive nodes
            // we check whether one should apply italic correction
            else if let Some(italic_correction) = italic_correction_to_apply {
                // Discharge italic correction
                if must_apply_italic_correction_before(node) {
                    layout.add_node(LayoutNode::horiz_kern(italic_correction));
                }
            }



            let nodes = match *node {
                ParseNode::Style(sty) => {
                    context.style = sty;
                    Vec::new()
                },
                ParseNode::Symbol(symbol) => {
                    let node = self.symbol(symbol, context)?;
                    italic_correction = node.is_symbol().map(|s| s.italics);
                    if !unicode_math::is_italic(symbol.codepoint) {
                        italic_correction = None;
                    }

                    vec![node]
                },
                _ => self.dispatch(node, context)?,
            };
            for node in nodes {
                layout.add_node(node);
            }

            // Transparent items should be ignored for parsing rules
            if current != TexSymbolType::Transparent {
                prev = Some(current);
            }
        }

        Ok(layout.finalize())
    }


    /// Access the underlying font used
    pub fn font(&self) -> & 'f F {
        self.font
    }

}




impl LayoutContext {
    /// Sets starting font size for layout, unit is pt / em.  
    /// See module [`rex::dimensions::units`](crate::dimensions::units) for an explanation of the different dimensions used in font rendering.
    pub fn font_size(mut self, font_size: f64) -> Self {
        self.font_size = Unit::<FontSize>::new(font_size) * Unit::standard_pt_to_px().lift();
        self
    }

    /// Sets the starting style of the layout (e.g. text style, display style). Cf [`Style`] for explanation of what a style is.
    pub fn layout_style(mut self, style : Style) -> Self {
        self.style = style;
        self
    }


    fn cramped(mut self) -> Self {
        self.style = self.style.cramped();
        self
    }

    fn superscript_variant(mut self) -> Self {
        self.style = self.style.superscript_variant();
        self
    }

    fn subscript_variant(mut self) -> Self {
        self.style = self.style.subscript_variant();
        self
    }

    fn numerator(mut self) -> Self {
        self.style = self.style.numerator();
        self
    }

    fn denominator(mut self) -> Self {
        self.style = self.style.denominator();
        self
    }

    fn with_display(mut self) -> Self {
        self.style = Style::Display;
        self
    }

    fn with_text(mut self) -> Self {
        self.style = Style::Text;
        self
    }

    fn no_next(mut self) -> Self {
        self.parent_next = TexSymbolType::Transparent;
        self
    }
}










fn must_apply_italic_correction_before(node: &ParseNode) -> bool {
    if let Some(symbol) = node.is_symbol() {
        if unicode_math::is_italic(symbol.codepoint) {
            return false;
        }
    }
    true
}


impl<'f, F : MathFont> LayoutEngine<'f, F> {

    fn dispatch<'a>(&self, node: &ParseNode, context: LayoutContext) -> LayoutResult<Vec<LayoutNode<'f, F>>> {
        Ok(match *node {
            ParseNode::Symbol(symbol) => vec![self.symbol(symbol, context)?],
            ParseNode::Scripts(ref script) => self.scripts(script, context)?,
            ParseNode::Radical(ref rad) => self.radical(rad, context)?,
            ParseNode::Delimited(ref delim) => self.delimited(delim, context)?,
            ParseNode::ExtendedDelimiter(ref delim) => vec![self.extended_delimiter(delim, context)?],
            ParseNode::Accent(ref acc) => vec![self.accent(acc, context)?],
            ParseNode::GenFraction(ref f) => self.frac(f, context)?,
            ParseNode::Stack(ref stack) => self.substack(stack, context)?,
            ParseNode::Array(ref arr) => self.array(arr, context)?,

            // TODO: check that `{1+}1` is not rendered same as `1+1`
            ParseNode::AtomChange(ref ac) => vec![self.layout_with(&ac.inner, context.no_next())?.as_node()],
            ParseNode::Group(ref gp) => vec![self.layout_with(gp, context.no_next())?.as_node()],
            ParseNode::Rule(rule) => vec![rule.as_layout(self, context)?],
            ParseNode::Kerning(kern) => vec![LayoutNode::horiz_kern(kern.to_px(self, context))],

            ParseNode::Color(ref clr) => {
                let inner = self.layout_with(&clr.inner, context.no_next())?;
                vec![layout::builders::color(inner, clr)]
            }

            ParseNode::DummyNode(_) => Vec::new(),
            ParseNode::FontEffect(FontEffect { inner: ref children }) => vec![self.underline(
                self.layout_with(children, context.no_next())?.as_node(), 
                context.no_next()
            )],

            ParseNode::PlainText(PlainText {ref text}) => {
                let mut to_return = Vec::new();
                for character in text.chars() {
                    if character.is_ascii_whitespace() {
                        to_return.push(LayoutNode::horiz_kern(Spacing::Medium.to_length().to_px(self, context)));
                    }
                    else {
                        to_return.push(self.font.glyph(character)?.as_layout(self, context)?);
                    }
                }
                to_return
            },

            // TODO: understand whether this is needed anywhere
            ParseNode::Style(_)     => unimplemented!(),
        })
    }

    fn symbol(&self, sym: Symbol, context: LayoutContext) -> LayoutResult<LayoutNode<'f, F>> {
        // Operators are handled specially.  We may need to find a larger
        // symbol and vertical center it.
        if matches!(sym.atom_type, TexSymbolType::Operator(_),) {
            return self.largeop(sym, context);
        }

        let glyph_gid = self.font().glyph_index(sym.codepoint).ok_or_else(|| FontError::MissingGlyphCodepoint(sym.codepoint))?;
        
        let substitute_gid = 
            context.style
                // if in script mode, we perform substitutions
                .script_level() 
                // only if said substitutions exist for glyph
                .and_then(|script_level| self.font.glyph_script_alternate(glyph_gid, script_level))
                // otherwise, use glyph_id from previous step
                .unwrap_or(glyph_gid)
        ;
        
        self.font()
            // create the glyph
            .glyph_from_gid(substitute_gid)?
            // turn it into a `LayoutNode`
            .as_layout(self, context)
    }


    /// Adds an underline below a node
    fn underline<'a>(&self, node: LayoutNode<'f, F>, context : LayoutContext) -> LayoutNode<'f, F> {
        let width = node.width;
        let depth = node.depth;
        let clearance       = self.metrics_cache.constants().underbar_vertical_gap    * context.font_size;
        let thick           = self.metrics_cache.constants().underbar_rule_thickness  * context.font_size;
        let extra_descender = self.metrics_cache.constants().underbar_extra_descender * context.font_size;
        let mut vbox = layout::builders::VBox::new();
        vbox.add_node(node);
        vbox.add_node(LayoutNode::vert_kern(clearance - depth));
        vbox.add_node(rule!(width: width, height: thick));
        vbox.add_node(LayoutNode::vert_kern(extra_descender));
        vbox.set_offset(clearance - depth + thick + extra_descender);
        vbox.build()
    }


    fn largeop(&self, sym: Symbol, context: LayoutContext) -> LayoutResult<LayoutNode<'f, F>> {
        let glyph = self.font.glyph(sym.codepoint)?;
        if context.style > Style::Text {
            let axis_offset = self.metrics_cache.constants().axis_height.to_px(self, context);
            let largeop = 
                self.font
                .vert_variant(glyph.gid, self.metrics_cache.constants().display_operator_min_height * self.metrics_cache.units_per_em())
                .as_layout(self, context)?;
            let shift = (largeop.height + largeop.depth).scale(0.5) - axis_offset;
            Ok(vbox!(offset: shift; largeop))
        } else {
            glyph.as_layout(self, context)
        }
    }
    
    fn accent<'a>(&self, acc: &Accent, context: LayoutContext) -> LayoutResult<LayoutNode<'f, F>> {
        // [ ] The width of the selfing box is the width of the base.
        // [x] Bottom accents: vertical placement is directly below nucleus,
        //       no correction takes place.
        // [x] WideAccent vs Accent: Don't expand Accent types.
        let base = self.layout_with(&acc.nucleus, context.cramped())?;
        let symbol = &acc.symbol;
        let glyph = self.font.glyph(symbol.codepoint)?; 
        let accent_variant =
            if acc.extend 
                { self.font.horz_variant(glyph.gid, self.to_font(base.width, context.font_size)) }
            // to not extend, we consider the trivial variant glyph where the glyph itself is used as replacement
            else 
                { VariantGlyph::Replacement(glyph.gid) }
        ;
        let accent = accent_variant.as_layout(self, context)?;

        // Attachment points for accent & base are calculated by
        //   (a) Non-symbol: width / 2.0,
        //   (b) Symbol:
        //      1. Attachment point (if there is one)
        //      2. Otherwise: (width + ic) / 2.0
        let base_offset = match layout::is_symbol(&base.contents) {
            Some(sym) => {
                let glyph = self.font.glyph_from_gid(sym.gid)?;
                if !glyph.attachment.is_zero() {
                    glyph.attachment.to_px(self, context)
                } else {
                    let offset = (glyph.advance + glyph.italics).scale(0.5);
                    offset.to_px(self, context)
                }
            }
            None => base.width.scale(0.5),
        };

        let acc_offset = match accent_variant {
            VariantGlyph::Replacement(sym) => {
                let glyph = self.font.glyph_from_gid(sym)?;
                if !glyph.attachment.is_zero() {
                    glyph.attachment.to_px(self, context)
                } else {
                    // For glyphs without attachmens, we must
                    // also account for combining glyphs
                    let offset = (glyph.bbox.2 + glyph.bbox.0).scale(0.5);
                    offset.to_px(self, context)
                }
            }

            VariantGlyph::Constructable(_, _) => accent.width.scale(0.5),
        };

        // We want "accent_offset" and "base_offset" to be aligned
        //      accent_offset
        //      <->
        //      [-+------]
        // [------+-]
        // <------>
        // base_offset
        //
        // We compute which of the base and the accent extend the most (i) to the left, (ii) to the right
        // We add spaces accordingly the make both match in width

        let diff_offsets = base_offset - acc_offset;

        let accent_node; 
        let base_node;
        let base_depth  = base.depth;
        let base_height = base.height;
        if diff_offsets > Unit::ZERO {
            let mut hbox_accent = layout::builders::HBox::new();
            hbox_accent.add_node(LayoutNode::horiz_kern(diff_offsets));
            hbox_accent.add_node(accent);

            base_node = base.as_node();
            accent_node = hbox_accent.build();
        }
        else {
            let mut hbox_base = layout::builders::HBox::new();
            hbox_base.add_node(LayoutNode::horiz_kern(- diff_offsets));
            hbox_base.add_node(base.as_node());

            base_node = hbox_base.build();
            accent_node = accent;
        }




        let mut vbox = layout::builders::VBox::new();

        if acc.under {
            let accent_node_height = accent_node.height;


            vbox.add_node(base_node);
            vbox.add_node(LayoutNode::vert_kern(- base_depth));
            vbox.add_node(accent_node);

            // node must stand at the same height as basis
            vbox.set_offset(- base_depth + accent_node_height);
        }
        else {
            // Do not place the accent any further than you would if given
            // an `x` character in the current style.
            let delta = -Unit::min(base_height, self.metrics_cache.constants().accent_base_height.to_px(self, context));

            // By not placing an offset on this vbox, we are assured that the
            // baseline will match the baseline of `base.as_node()`
            vbox.add_node(accent_node);
            vbox.add_node(LayoutNode::vert_kern(delta));
            vbox.add_node(base_node);
        }
        Ok(vbox.build())
    }

    fn delimited<'a>(&self, delim: &Delimited, config: LayoutContext) -> LayoutResult<Vec<LayoutNode<'f, F>>> {
        // let inner = layout(&delim.inner, config)?.as_node();
        let mut to_return = Vec::with_capacity(3); // 3 is most typical case for delimiters
        let mut inners = Vec::with_capacity(delim.inners().len());
        let mut max_height = Unit::ZERO;
        let mut min_depth  = Unit::ZERO;
        for inner_parse_nodes in delim.inners() {
            let inner = self.layout_with(inner_parse_nodes.as_slice(), config)?.as_node();
            max_height = Unit::max(max_height, inner.height);
            min_depth  = Unit::min(min_depth,  inner.depth);
            inners.push(inner);
        }


        let delimiters = delim.delimiters();
        for (symbol, inner) in Iterator::zip(delimiters.iter(), inners)  {
            to_return.push(self.extend_delimiter(*symbol, max_height, min_depth, config)?);
            to_return.push(inner);
        }
        let right_symbol = delimiters.last().unwrap();
        to_return.push(self.extend_delimiter(*right_symbol, max_height, min_depth, config)?);

        Ok(to_return)
    }
    fn scripts(&self, scripts: &Scripts, context: LayoutContext) -> LayoutResult<Vec<LayoutNode<'f, F>>> {
        // For OpenType Math: https://learn.microsoft.com/en-us/typography/opentype/spec/math#mathkerninfo-table
        // See: https://tug.org/TUGboat/tb27-1/tb86jackowski.pdf
        //      https://www.tug.org/tugboat/tb30-1/tb94vieth.pdf
        let base = match scripts.base {
            Some(ref base) => self.layout_with(std::slice::from_ref(base), context)?,
            None => Layout::new(),
        };

        let mut sup = match scripts.superscript {
            Some(ref sup) => self.layout_with(sup, context.superscript_variant())?,
            None => Layout::new(),
        };

        let mut sub = match scripts.subscript {
            Some(ref sub) => self.layout_with(sub, context.subscript_variant())?,
            None => Layout::new(),
        };

        // We use a different algoirthm for handling scripts for operators with limits.
        // This is where he handle Operators with limits.
        if let Some(ref b) = scripts.base {
            if TexSymbolType::Operator(true) == b.atom_type() {
                let to_return = self.operator_limits(base, sup, sub, context)?;
                return Ok(vec![to_return]);
            }
        }

        // We calculate the vertical positions of the scripts.  The `adjust_up`
        // variable will describe how far we need to adjust the superscript up.
        let mut adjust_up = Unit::ZERO;
        let mut adjust_down = Unit::ZERO;
        let mut sup_kern = Unit::ZERO;
        let mut sub_kern = Unit::ZERO;

        if scripts.superscript.is_some() {
            // Use default font values for first iteration of vertical height.
            adjust_up = match context.style.is_cramped() {
                true =>  self.metrics_cache.constants().superscript_shift_up_cramped,
                false => self.metrics_cache.constants().superscript_shift_up,
            }
            .to_px(self, context);

            // TODO: These checks should be recursive?
            let mut height = base.height;
            if let Some(ref b) = scripts.base {
                if b.atom_type() != TexSymbolType::Operator(false) {
                    // For accents whose base is a simple symbol we do not take
                    // the accent into account while positioning the superscript.
                    if let ParseNode::Accent(ref acc) = **b {
                        use crate::parser::is_symbol;
                        if let Some(sym) = is_symbol(&acc.nucleus) {
                            height = self.font().glyph(sym.codepoint)?.height().to_px(self, context);
                        }
                    }
                    // Apply italics correction is base is a symbol
                    else if let Some(base_sym) = base.is_symbol() {
                        let base_height = self.to_font(base.height, context.font_size);
                        // TODO: shouldn't this be font_size of sperscript?
                        let script_depth = self.to_font(sup.depth + adjust_up, context.font_size);

                        // Lookup font kerning of superscript
                        let kern = superscript_kern(&base, &sup, base_height, script_depth)?.to_px(self, context);
                        sup_kern = base_sym.italics + kern;
                    }
                }
            }

            let drop_max = self.metrics_cache.constants().superscript_baseline_drop_max.to_px(self, context);
            adjust_up = max!(adjust_up,
                            height - drop_max,
                            self.metrics_cache.constants().superscript_bottom_min.to_px(self, context) - sup.depth);
        }

        // We calculate the vertical position of the subscripts.  The `adjust_down`
        // variable will describe how far we need to adjust the subscript down.
        if scripts.subscript.is_some() {
            // Use default font values for first iteration of vertical height.
            adjust_down = max!(self.metrics_cache.constants().subscript_shift_down.to_px(self, context),
                                sub.height - self.metrics_cache.constants().subscript_top_max.to_px(self, context),
                                self.metrics_cache.constants().subscript_baseline_drop_min.to_px(self, context) - base.depth);

            // Provided that the base and subscript are symbols, we apply
            // kerning values found in the kerning font table
            if let Some(ref b) = scripts.base {
                if let Some(base_sym) = base.is_symbol() {
                    if TexSymbolType::Operator(false) == b.atom_type() {
                        // This recently changed in LuaTeX.  See `nolimitsmode`.
                        // This needs to be the glyph information _after_ layout for base.
                        sub_kern = -self.font().glyph_from_gid(base_sym.gid)?.italics.to_px(self, context);
                    }
                }

                let base_depth = self.to_font(base.depth, context.font_size);
                // TODO: shouldn't this be font_size of subscript?
                let script_height = self.to_font(sub.height + adjust_down, context.font_size);
                sub_kern += subscript_kern(&base, &sub, base_depth, script_height)?.to_px(self, context);
            }
        }

        // TODO: lazy gap fix; see BottomMaxWithSubscript
        if scripts.subscript.is_some() && scripts.superscript.is_some() {
            let sup_bot = adjust_up + sup.depth;
            let sub_top = sub.height - adjust_down;
            let gap_min = self.metrics_cache.constants().sub_superscript_gap_min.to_px(self, context);
            if sup_bot - sub_top < gap_min {
                let adjust = (gap_min - sup_bot + sub_top).scale(0.5);
                adjust_up += adjust;
                adjust_down += adjust;
            }
        }

        let mut contents = layout::builders::VBox::new();
        if scripts.superscript.is_some() {
            if !sup_kern.is_zero() {
                sup.contents.insert(0, LayoutNode::horiz_kern(sup_kern));
                sup.width += sup_kern;
            }

            let corrected_adjust = adjust_up - sub.height + adjust_down;
            contents.add_node(sup.as_node());
            contents.add_node(LayoutNode::vert_kern(corrected_adjust));
        }

        contents.set_offset(adjust_down);
        if scripts.subscript.is_some() { 
            if !sub_kern.is_zero() {
                sub.contents.insert(0, LayoutNode::horiz_kern(sub_kern));
                sub.width += sub_kern;
            }
            contents.add_node(sub.as_node());
        }

        Ok(vec![
            base.as_node(),
            contents.build(),
        ])
    }

    fn operator_limits<'a>(&self, base: Layout<'f, F>, sup: Layout<'f, F>, sub: Layout<'f, F>, context: LayoutContext) -> LayoutResult<LayoutNode<'f, F>> {
        // Provided that the operator is a simple symbol, we need to account
        // for the italics correction of the symbol.  This how we "center"
        // the superscript and subscript of the limits.
        let delta = match base.is_symbol() {
            Some(gly) => gly.italics,
            None => Unit::ZERO
        };

        // Next we calculate the kerning required to separate the superscript
        // and subscript (respectively) from the base.
        let sup_kern = Unit::max(self.metrics_cache.constants().upper_limit_baseline_rise_min.to_px(self, context),
                        self.metrics_cache.constants().upper_limit_gap_min.to_px(self, context) - sup.depth);
        let sub_kern = Unit::max(self.metrics_cache.constants().lower_limit_gap_min.to_px(self, context),
                        self.metrics_cache.constants().lower_limit_baseline_drop_min.to_px(self, context) - sub.height) -
                    base.depth;

        // We need to preserve the baseline of the operator when
        // attaching the scripts.  Since the base should already
        // be aligned, we only need to offset by the addition of
        // subscripts.
        let offset = sub.height + sub_kern;

        // We will construct a vbox containing the superscript/base/subscript.
        // We will all of these nodes, so we widen each to the largest.
        let width = max!(base.width, sub.width + delta.scale(0.5), sup.width + delta.scale(0.5));

        Ok(vbox![
            offset: offset;
            hbox![align: Alignment::Centered(sup.width);
                width: width;
                LayoutNode::horiz_kern(delta.scale(0.5)),
                sup.as_node()
            ],

            LayoutNode::vert_kern(sup_kern),
            base.centered(width).as_node(),
            LayoutNode::vert_kern(sub_kern),

            hbox![align: Alignment::Centered(sub.width);
                width: width;
                LayoutNode::horiz_kern(-delta.scale(0.5)),
                sub.as_node()
            ]
        ])
    }

    fn frac(&self, frac: &GenFraction, context: LayoutContext) -> LayoutResult<Vec<LayoutNode<'f, F>>> {
        // The style used in fractions (\displaystyle, etc) does not necessarily inherit from context
        // So we create a new context
        let frac_context = match frac.style {
            MathStyle::NoChange => context.clone(),
            MathStyle::Display => context.with_display(),
            MathStyle::Text => context.with_text(),
        };

        let bar = match frac.bar_thickness {
            BarThickness::Default => self.metrics_cache.constants().fraction_rule_thickness.to_px(self, frac_context),
            BarThickness::None => Unit::ZERO,
            BarThickness::Unit(u) => u.to_px(self, frac_context),
        };

        let mut n = self.layout_with(&frac.numerator,   frac_context.numerator())?;
        let mut d = self.layout_with(&frac.denominator, frac_context.denominator())?;

        if n.width > d.width {
            d.alignment = Alignment::Centered(d.width);
            d.width = n.width;
        } else {
            n.alignment = Alignment::Centered(n.width);
            n.width = d.width;
        }

        let numer = n.as_node();
        let denom = d.as_node();

        let axis = self.metrics_cache.constants().axis_height.to_px(self, frac_context);
        let shift_up;
        let shift_down;
        let gap_num;
        let gap_denom;

        if frac_context.style > Style::Text {
            shift_up   = self.metrics_cache.constants().fraction_numerator_display_style_shift_up.to_px(self, frac_context);
            shift_down = self.metrics_cache.constants().fraction_denominator_display_style_shift_down.to_px(self, frac_context);
            gap_num    = self.metrics_cache.constants().fraction_num_display_style_gap_min.to_px(self, frac_context);
            gap_denom  = self.metrics_cache.constants().fraction_denom_display_style_gap_min.to_px(self, frac_context);
        } else {
            shift_up   = self.metrics_cache.constants().fraction_numerator_shift_up.to_px(self, frac_context);
            shift_down = self.metrics_cache.constants().fraction_denominator_shift_down.to_px(self, frac_context);
            gap_num    = self.metrics_cache.constants().fraction_numerator_gap_min.to_px(self, frac_context);
            gap_denom  = self.metrics_cache.constants().fraction_denominator_gap_min.to_px(self, frac_context);
        }

        let kern_num = Unit::max(shift_up - axis - bar.scale(0.5), gap_num - numer.depth);
        let kern_den = Unit::max(shift_down + axis - denom.height - bar.scale(0.5), gap_denom);
        let offset = denom.height + kern_den + bar.scale(0.5) - axis;

        let width = numer.width;
        let inner = vbox!(offset: offset;
            numer,
            LayoutNode::vert_kern(kern_num),
            rule!(width: width, height: bar),
            LayoutNode::vert_kern(kern_den),
            denom
        );

        let null_delimiter_space = self.metrics_cache.constants().null_delimiter_space * frac_context.font_size;
        let axis_height = self.metrics_cache.constants().axis_height * frac_context.font_size;
        // Enclose fraction with delimiters if provided, otherwise with a NULL_DELIMITER_SPACE.
        let left = match frac.left_delimiter {
            None => LayoutNode::horiz_kern(null_delimiter_space),
            Some(sym) => {
                let clearance = Unit::max(inner.height - axis_height, axis_height - inner.depth).scale(2.0);
                let clearance = Unit::max(clearance, self.metrics_cache.constants().delimited_sub_formula_min_height * frac_context.font_size);

                let glyph_id = self.font.glyph_index(sym.codepoint).ok_or(crate::error::FontError::MissingGlyphCodepoint(sym.codepoint))?;
                self.font
                    .vert_variant(glyph_id, self.to_font(clearance, frac_context.font_size))
                    .as_layout(self, frac_context)?
                    .centered(axis_height.to_px(self, frac_context))
            }
        };

        let right = match frac.right_delimiter {
            None => LayoutNode::horiz_kern(null_delimiter_space),
            Some(sym) => {
                let clearance = Unit::max(inner.height - axis_height, axis_height - inner.depth).scale(2.0);
                let clearance = Unit::max(clearance, self.metrics_cache.constants().delimited_sub_formula_min_height * frac_context.font_size);

                let glyph_id = self.font.glyph_index(sym.codepoint).ok_or(crate::error::FontError::MissingGlyphCodepoint(sym.codepoint))?;
                self.font
                    .vert_variant(glyph_id, self.to_font(clearance, frac_context.font_size))
                    .as_layout(self, frac_context)?
                    .centered(axis_height.to_px(self, frac_context))
            }
        };

        
        Ok(vec![
            left,
            inner,
            right,
        ])
    }

    fn radical<'a>(&self, rad: &Radical, context: LayoutContext) -> LayoutResult<Vec<LayoutNode<'f, F>>> {
        // reference rule 11 from pg 443 of TeXBook
        let contents = self.layout_with(&rad.inner, context.cramped())?.as_node();

        // Cache dimensions before contents is moved
        let contents_height = contents.height;
        let contents_depth = contents.depth;
        let contents_width = contents.width;

        // obtain minimum clearange between radicand and radical bar
        // and cache other sizes that will be needed
        let gap = match context.style >= Style::Display {
            true  => self.metrics_cache.constants().radical_display_style_vertical_gap.to_px(self, context),
            false => self.metrics_cache.constants().radical_vertical_gap.to_px(self, context),
        };

        let rule_thickness = self.metrics_cache.constants().radical_rule_thickness.to_px(self, context);
        let rule_ascender = self.metrics_cache.constants().radical_extra_ascender.to_px(self, context);

        // determine size of radical glyph
        let inner_height = (contents_height - contents_depth) + gap + rule_thickness;
        let sqrt = self.vert_variant_for_codepoint(rad.character, self.to_font(inner_height, context.font_size))?.as_layout(self, context)?;

        // pad between radicand and radical bar
        let delta = (sqrt.height - sqrt.depth - inner_height).scale(0.5) + rule_thickness;
        let gap = Unit::max(delta, gap);

        // offset radical symbol
        let offset = rule_thickness + gap + contents_height;
        let offset = sqrt.height - offset;

        
        // padding above sqrt
        // TODO: This is unclear
        let top_padding = rule_ascender - rule_thickness;

        // Build the radical body (sqrt glyph + bar + contents)
        let radical_symbol_vbox = vbox![offset: offset; sqrt];
        // For use in positioning degree
        let radical_symbol_depth = radical_symbol_vbox.depth;
        let radical_symbol_height = radical_symbol_vbox.height;
        let radical_body = vec![
            radical_symbol_vbox,
            vbox![
                LayoutNode::vert_kern(top_padding),
                rule!(width: contents_width, height: rule_thickness),
                LayoutNode::vert_kern(gap),
                contents
            ]
        ];

        // If there's an index (nth root), prepend it
        if let Some(ref index_nodes) = rad.index {
            if !index_nodes.is_empty() {
                // Layout the index in script-script style
                // NB: we're matching XeTeX here and it produces the most esthetically pleasing outcome 
                // but not sure where this is specified in the Unicode MATH spec.
                let index_context = context.superscript_variant().superscript_variant();
                let index_layout = self.layout_with(index_nodes, index_context)?.as_node();

                // Get radical degree positioning constants from font
                let kern_before = self.metrics_cache.constants().radical_kern_before_degree.to_px(self, context);
                let kern_after = self.metrics_cache.constants().radical_kern_after_degree.to_px(self, context);
                let raise_percent = self.metrics_cache.constants().radical_degree_bottom_raise_percent;

                // Calculate vertical raise: raise the degree so its bottom is at
                // raise_percent of the height (ascender + descender) of the radical sign
                // counting from the bottom of the radical sign
                //            
                //                                 ______  ∧
                // degree (to be raised) →     3  /        | 
                //                          ∧    /         |   total_radical height
                //  total_radical_height    |---/ ---------+--------- baseline
                //  × raise                 ⊥ \/           ⊥
                // 
                let raise_from_bottom_radical = (radical_symbol_height - radical_symbol_depth).scale(raise_percent as f64 / 100.0);
                let raise = raise_from_bottom_radical + radical_symbol_depth;

                let mut result = Vec::with_capacity(radical_body.len() + 3);
                result.push(LayoutNode::horiz_kern(kern_before));
                result.push(vbox![offset: -raise; index_layout]);
                result.push(LayoutNode::horiz_kern(kern_after));
                result.extend(radical_body);
                return Ok(result);
            }
        }

        Ok(radical_body)
    }

    fn substack<'a>(&self, stack: &Stack, context: LayoutContext) -> LayoutResult<Vec<LayoutNode<'f, F>>> {
        // Don't bother constructing a new node if there is nothing.
        if stack.lines.len() == 0 {
            return Ok(Vec::new());
        }

        // Layout each line in the substack, and track which line is the widest
        let mut lines: Vec<Layout<F>> = Vec::with_capacity(stack.lines.len());
        let mut widest = Unit::ZERO;
        let mut widest_idx = 0;
        for (n, line) in stack.lines.iter().enumerate() {
            let line = self.layout_with(line, context)?;
            if line.width > widest {
                widest = line.width;
                widest_idx = n;
            }
            lines.push(line);
        }

        // Center lines according to widest variant
        for (n, line) in lines.iter_mut().enumerate() {
            if n == widest_idx {
                continue;
            }
            line.alignment = Alignment::Centered(line.width);
            line.width = widest;
        }

        // The line gap will be taken from STACK_GAP constants
        let gap_min = if context.style > Style::Text {
            self.metrics_cache.constants().stack_display_style_gap_min.to_px(self, context)
        } else {
            self.metrics_cache.constants().stack_gap_min.to_px(self, context)
        };

        // No idea.
        let gap_try = if context.style > Style::Text {
            self.metrics_cache.constants().stack_top_display_style_shift_up
            - self.metrics_cache.constants().axis_height
            + self.metrics_cache.constants().stack_bottom_shift_down
            - self.metrics_cache.constants().accent_base_height.scale(2.0)
        } else {
            self.metrics_cache.constants().stack_top_shift_up
            - self.metrics_cache.constants().axis_height
            + self.metrics_cache.constants().stack_bottom_shift_down
            - self.metrics_cache.constants().accent_base_height.scale(2.0)
        }
        .to_px(self, context);

        // Join the lines with appropriate spacing inbetween
        let mut vbox = builders::VBox::new();
        let length = lines.len();
        for (idx, line) in lines.into_iter().enumerate() {
            let prev = line.depth;
            vbox.add_node(line.as_node());

            // Try for an ideal gap, otherwise use the minimum
            if idx < length {
                let gap = Unit::max(gap_min, gap_try - prev);
                vbox.add_node(LayoutNode::vert_kern(gap));
            }
        }

        // Vertically center the stack to the axis
        let offset = (vbox.height + vbox.depth).scale(0.5) - self.metrics_cache.constants().axis_height.to_px(self, context);
        vbox.set_offset(offset);
        
        Ok(vec![
            vbox.build()
        ])
    }

    fn extended_delimiter<'a>(&self, delim: &ExtendedDelimiter, context: LayoutContext) -> LayoutResult<LayoutNode<'f, F>> {
        let ExtendedDelimiter { symbol, height_enclosed_content }  = delim; 

        let height_enclosed_content = height_enclosed_content.to_px(self, context);

        self.extend_delimiter(*symbol, height_enclosed_content, Unit::ZERO, context)
    }


    fn array(&self, array: &Array, context: LayoutContext) -> LayoutResult<Vec<LayoutNode<'f, F>>> {
        let cell_layout_settings = context.layout_style(array.cell_layout_style);
        let normal_baseline_skip = BASELINE_SKIP; 
        let strut_height     = normal_baseline_skip.scale(STRUT_HEIGHT) * context.font_size; 
        let strut_depth      = - normal_baseline_skip.scale(STRUT_DEPTH)  * context.font_size; 

        let jot = if array.extra_row_sep { JOT } else { Unit::ZERO };
        let baseline_skip = normal_baseline_skip * context.font_size + jot * Unit::standard_pt_to_px();
        let line_skip = (LINE_SKIP_ARRAY + jot) * Unit::standard_pt_to_px();
        let line_skip_limit = (LINE_SKIP_LIMIT_ARRAY + jot)  * Unit::standard_pt_to_px();

        let half_col_sep     = COLUMN_SEP      * Unit::standard_pt_to_px(); 
        let rule_width       = RULE_WIDTH      * Unit::standard_pt_to_px();
        let double_rule_sep  = DOUBLE_RULE_SEP * Unit::standard_pt_to_px();

        let null_delimiter_space = self.metrics_cache.constants().null_delimiter_space * context.font_size;


        // Don't bother constructing a new node if there is nothing.
        let num_rows = array.rows.len();
        let num_columns = array.rows.iter().map(Vec::len).max().unwrap_or(0);
        if num_columns == 0 {
            return Ok(Vec::new());
        }

        // -- LAY OUT ALL NODES OF ARRAY
        // Columns of an array may be separated by @-expressions
        // We treat @-expressions are ordinary columns, except for the fact that
        // @-expression content is the same in every row
        // We compute how many columns there are, when taking into account @-expressions
        let all_separators = &array.col_format.separators;
        // We count the number of columns including @-expr columns
        let num_columns_at = num_columns + all_separators
            .iter().map(|separators| separators.iter()) 
            .flatten()
            .filter(|separator| matches!(separator, ColSeparator::AtExpression(_)))
            .count()
        ;
        // we store alignments information ; we can also use this array to check if a column was an @-expression or not
        let mut alignments : Vec<Option<ArrayColumnAlign>> = Vec::with_capacity(num_columns_at); 
        let mut columns : Vec<Vec<Layout<'f, F>>> = Vec::with_capacity(num_columns_at);
        let mut n_vertical_bars : Vec<u8> = Vec::with_capacity(num_columns_at + 1);
        let mut current_n_vertical_bars = 0;

        for separator in &all_separators[0] {
            match separator {
                ColSeparator::VerticalBars(n_bars) => 
                    current_n_vertical_bars += n_bars,
                ColSeparator::AtExpression(nodes) => {
                    let node = self.layout_with(&nodes, cell_layout_settings)?;
                    let mut column = Vec::with_capacity(num_rows);
                    for _ in 0 .. num_rows {
                        column.push(node.clone());
                    }
                    columns.push(column);
                    alignments.push(None);
                    n_vertical_bars.push(std::mem::replace(&mut current_n_vertical_bars, 0));
                },
            }
        }
        for (i, separators) in all_separators[1..].iter().enumerate() {
            // first comes the real column
            let mut column = Vec::with_capacity(num_rows);
            for j in 0 .. num_rows {
                let cell_node = array.rows
                    .get(j)
                    .and_then(|row| row.get(i))
                ;  
                let layout = match cell_node {
                    Some(cell_node) => self.layout_with(&cell_node, cell_layout_settings)?,
                    None => Layout::new(),
                };
                column.push(layout);
            }
            columns.push(column);
            alignments.push(Some(array.col_format.alignment[i]));
            n_vertical_bars.push(std::mem::replace(&mut current_n_vertical_bars, 0));

            // then comes the separators
            for separator in separators {
                match separator {
                    ColSeparator::VerticalBars(n_bars) => 
                        current_n_vertical_bars += n_bars,
                    ColSeparator::AtExpression(nodes) => {
                        let node = self.layout_with(&nodes, cell_layout_settings)?;
                        let mut column = Vec::with_capacity(num_rows);
                        for _ in 0 .. num_rows {
                            column.push(node.clone());
                        }
                        columns.push(column);
                        alignments.push(None);
                        n_vertical_bars.push(std::mem::replace(&mut current_n_vertical_bars, 0));
                    },
                }
            }
        }
        n_vertical_bars.push(std::mem::replace(&mut current_n_vertical_bars, 0));

        debug_assert_eq!(columns.len(), num_columns_at);
        debug_assert_eq!(alignments.len(), num_columns_at);
        debug_assert_eq!(n_vertical_bars.len(), num_columns_at + 1);


        // let mut columns = Vec::with_capacity(num_columns);
        // for _ in 0..num_columns {
        //     columns.push(Vec::with_capacity(num_rows));
        // }

        // -- COMPUTE COLUMN WIDTHS AND BASELINE DISTS
        // column width
        let mut col_widths = Vec::with_capacity(num_columns_at);

        for column in columns.iter() {
            // TODO: there is no need to do that if the column is an @-expr, since the width is expected to be the same
            let mut col_width = Unit::ZERO;
            for node in column {
                col_width = Unit::max(col_width, node.width);
            }
            col_widths.push(col_width);
        }
        debug_assert_eq!(col_widths.len(), num_columns_at);


        // baseline_dists[0] is dist from top of first line to first baseline (e.g. as though it was preceded by a line of zero-depth)
        // baseline_dists[i] is the dist from row indexed i and row indexed i+1
        let mut baseline_dists = Vec::with_capacity(num_rows);
        let mut prev_depth = Unit::ZERO;

        for i_row in 0 .. num_rows {
            let mut max_height = strut_height;
            let mut max_depth  = strut_depth;

            for column in columns.iter() {
                let cell = &column[i_row];

                max_height = Unit::max(max_height, cell.height);
                max_depth  = Unit::min(max_depth, cell.depth); // depth are negative
            }

            let box_separation = max_height - prev_depth;
            let baseline_dist = 
                if i_row == 0 {
                    box_separation
                }
                else if box_separation + line_skip_limit > baseline_skip && !baseline_dists.is_empty() {
                    box_separation + line_skip
                }
                else {
                    baseline_skip
                }
            ;
            baseline_dists.push(baseline_dist);
            prev_depth = max_depth;
        }
        let last_depth = prev_depth;
        debug_assert_eq!(baseline_dists.len(), num_rows);

        
        // -- CONSTRUCT COLUMNS
        // Each column is a VBox containing (1) the cell's content, (2) vertical space needed to align rows
        // We need to center cells on the horizontal axis (left, center or right)
        // We need to add vertical space between successive cells so that the baselines of all cells are aligned
        let mut column_vboxes = Vec::with_capacity(num_columns_at);
        for (i_col, column) in columns.into_iter().enumerate() {
            let mut vbox = builders::VBox::new();
            let col_width = col_widths[i_col];
            let alignment = alignments[i_col];


            for (i_row, mut cell) in column.into_iter().enumerate() {
                // add vertical space if necessary to align cells of the same row
                let kern = baseline_dists[i_row] - cell.height;
                if kern > Unit::ZERO {
                    vbox.add_node(LayoutNode::vert_kern(kern));
                }

                cell.alignment = match alignment {
                    Some(ArrayColumnAlign::Centered) => Alignment::Centered(cell.width),
                    Some(ArrayColumnAlign::Left)     => Alignment::Left,
                    Some(ArrayColumnAlign::Right)    => Alignment::Right(cell.width),
                    None => Alignment::Default,
                };
                cell.width = col_width;
                vbox.add_node(cell.as_node());

            }

            // add final space to align bottom of boxes
            let kern = - last_depth;
            if kern > Unit::ZERO {
                vbox.add_node(LayoutNode::vert_kern(kern));
            }
            column_vboxes.push(vbox);
        }
        debug_assert_eq!(column_vboxes.len(), num_columns_at);



        // -- CONSTRUCT ARRAY
        // Now columns have been constructed, we lay them out together
        // We insert space between columns and vertical bars between them

        // the body of the matrix is an hbox of column vectors.
        let mut hbox = builders::HBox::new();

        // If there are no delimiters, insert a null space.  Otherwise we insert
        // the delimiters _after_ we have laidout the body of the matrix.
        if array.left_delimiter.is_none() {
            hbox.add_node(LayoutNode::horiz_kern(null_delimiter_space));
        }


        let total_height : Unit<Px> = 
            baseline_dists.iter().cloned().sum::<Unit<Px>>()
            - last_depth
        ;

        let rule_measurements = RuleMeasurements {
            rule_width,
            total_height,
            double_rule_sep,
        };

        // add left vertical bars
        draw_vertical_bars(&mut hbox, n_vertical_bars[0], rule_measurements);


        for (i_col, vbox) in column_vboxes.into_iter().enumerate() {
            // insert half col sep before if:
            //   - vbox is not an @-expression
            //   - if this is the first col, there is no left delimiter
            //   - if not, preceding col is not @-expr
            if alignments[i_col].is_some() && i_col.checked_sub(1).map_or(array.left_delimiter.is_none(), |prec_i| alignments[prec_i].is_some()) {
                hbox.add_node(LayoutNode::horiz_kern(half_col_sep));
            }

            // insert column
            hbox.add_node(vbox.build());

            // insert half col sep before if:
            //   - vbox is not an @-expression
            //   - if this is the last col, there is no right delimiter
            if alignments[i_col].is_some() && alignments.get(i_col + 1).map_or(array.right_delimiter.is_none(), Option::is_some) {
                hbox.add_node(LayoutNode::horiz_kern(half_col_sep));
            }

            // draw vertical bars after
            draw_vertical_bars(&mut hbox, n_vertical_bars[i_col + 1], rule_measurements);
        }

        if array.right_delimiter.is_none() {
            hbox.add_node(LayoutNode::horiz_kern(null_delimiter_space));
        }


        // TODO: Reference array vertical alignment (optional [bt] arguments)
        // Vertically center the array on axis.
        // Note: hbox has no depth, so hbox.height is total height.
        let height = hbox.height;
        let mut vbox = builders::VBox::new();
        let offset = height.scale(0.5) - self.metrics_cache.constants().axis_height.to_px(self, context);
        vbox.set_offset(offset);
        vbox.add_node(hbox.build());
        let vbox = vbox.build();

        // Now that we know the layout of the matrix body we can place scaled delimiters
        // First check if there are any delimiters to add, if not just return.
        if array.left_delimiter.is_none() && array.right_delimiter.is_none() {
            return Ok(vec![vbox]);
        }

        // place delimiters in an hbox surrounding the matrix body
        let mut hbox = builders::HBox::new();
        let axis = self.metrics_cache.constants().axis_height.to_px(self, context);
        let clearance = Unit::max(height.scale(self.metrics_cache.constants().delimiter_factor),
                            height - self.metrics_cache.constants().delimiter_short_fall * context.font_size);

        if let Some(left) = array.left_delimiter {
            let left = self.vert_variant_for_codepoint(left.codepoint, self.to_font(clearance, context.font_size))?
                .as_layout(self, context)?
                .centered(axis);
            hbox.add_node(left);
        }

        hbox.add_node(vbox);
        if let Some(right) = array.right_delimiter {
            let right = self.vert_variant_for_codepoint(right.codepoint, self.to_font(clearance, context.font_size))?
                .as_layout(self, context)?
                .centered(axis);
            hbox.add_node(right);
        }
        Ok(vec![hbox.build()])
    } 


    fn vert_variant_for_codepoint(&self, codepoint: char, height: Unit<FUnit>) -> Result<VariantGlyph, FontError> {
        let glyph_id = self.font.glyph_index(codepoint).ok_or_else(|| FontError::MissingGlyphCodepoint(codepoint))?;
        Ok(self.font.vert_variant(glyph_id, height))
    }

    fn extend_delimiter(
        &self,
        symbol : Symbol, 
        height_content: Unit<Px>,
        depth_content:  Unit<Px>, 
        context: LayoutContext
    ) -> LayoutResult<LayoutNode<'f, F>> {
        let min_height = self.metrics_cache.constants().delimited_sub_formula_min_height * context.font_size;
        let null_delimiter_space = self.metrics_cache.constants().null_delimiter_space * context.font_size;

        if symbol.codepoint == '.' {
            return Ok(LayoutNode::horiz_kern(null_delimiter_space));
        }


        // Only extend if we meet a certain size
        // TODO: This quick height check doesn't seem to be strong enough,
        // reference: http://tug.org/pipermail/luatex/2010-July/001745.html
        if Unit::max(height_content, -depth_content) > min_height.scale(0.5) {
            let axis = self.metrics_cache.constants().axis_height * context.font_size;

            let inner_size = Unit::max(height_content - axis, axis - depth_content).scale(2.0);
            let clearance_px  = Unit::max(
                inner_size.scale(self.metrics_cache.constants().delimiter_factor),
                height_content - depth_content - self.metrics_cache.constants().delimiter_short_fall * context.font_size
            );
            let clearance = self.to_font(clearance_px, context.font_size);

            Ok(
                self
                .vert_variant_for_codepoint(symbol.codepoint, clearance)?
                .as_layout(self, context)?
                .centered(axis)
            )
        }
        else {
            self.font
                .glyph(symbol.codepoint)?
                .as_layout(self, context)
        }
    }
}


#[derive(Debug, Clone, Copy)]
struct RuleMeasurements {
    rule_width:      Unit<Px>, 
    total_height:    Unit<Px>, 
    double_rule_sep: Unit<Px>
}

#[inline]
fn draw_vertical_bars<F>(hbox: &mut builders::HBox<F>, n_bars: u8, rule_measurements: RuleMeasurements) {
    if n_bars != 0 {
        let RuleMeasurements { rule_width, total_height, double_rule_sep } = rule_measurements;
        let rule_width = rule_width;
        let total_height = total_height;
        let double_rule_sep = double_rule_sep;
        hbox.add_node(rule![width: rule_width, height: total_height]);
        for _ in 0 .. n_bars - 1 {
            hbox.add_node(LayoutNode::horiz_kern(double_rule_sep));
            hbox.add_node(rule![width: rule_width, height: total_height]);
        }
    }
}



