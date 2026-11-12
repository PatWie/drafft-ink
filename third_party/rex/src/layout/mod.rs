//! Converts [`ParseNode`](crate::parser::ParseNode)s to [`Layout`] boxes which are ready to be rendered.
//! 
//! The layout boxes follow a similar model as those found in HTML and TeX in that they both
//! have horizontal and vertical boxes.  One difference will be how glue is handled.  HTML/CSS
//! does not have anything similar to how glue is handled in TeX and so aboslute size will be
//! necessary for these scnarios.  It's unclear if we will be able to induce alignments from
//! glue, such as something being centered, aligned left/right, etc.  These questions may
//! also be useful to answer in SVG.
//!
//! Layout boxes will contain a minimal representation of what will be rendered.
//! This includes the container types: Horizontal/Vertical boxes,
//! and primitive types: Symbols, lines, spacing.
//!
//! While rendering in mathmode, most types require an atomtype to determine the kerning
//! between symbols.  This information must also be present with layout boxes.
//!
//! The units used in layout boxes must be in FontUnit (as defined in CSS).

#[macro_use]
mod builders;
mod convert;
pub mod engine;
pub mod spacing;
pub mod kern;
pub mod constants;

use crate::bbox::BBoxBackend;
use crate::font::common::{GlyphId, ScriptLevel};
use crate::font::{FontMetricsCache, MathFont};
use crate::parser::color::RGBA;
use crate::Renderer;
use std::ops::Deref;
use std::fmt;
use std::collections::BTreeMap;
use crate::dimensions::Unit;
use crate::dimensions::units::{Px, Em};



/// Contains a set of [`LayoutNode`s](crate::layout::LayoutNode) that defines the position of glyphs and rules (i.e. filled rectangles) and certain measurements useful for rendering.
/// It serves as input to [`Renderer::render`](crate::render::Renderer::render).
#[derive(Debug)]
pub struct Layout<'f, F> {
    /// The children nodes contained in the layout
    /// By default, they are laid out, horizontally as a horizontal box.
    pub contents:  Vec<LayoutNode<'f, F>>,
    /// Width of content
    pub width:     Unit<Px>,
    /// Height of content ; distance from baseline to the top of the layout
    pub height:    Unit<Px>,
    /// Depth of content ; distance from baseline to the bottom of the layout
    pub depth:     Unit<Px>,
    /// Offset from the baseline 
    // (NB: does not seem used at the moment)
    pub offset:    Unit<Px>,
    /// How to horizontally lay out children nodes
    pub alignment: Alignment,
}

impl<'f, F> Clone for Layout<'f, F> {
    fn clone(&self) -> Self {
        Self { 
            contents:  self.contents.clone(),
            width:     self.width,
            height:    self.height,
            depth:     self.depth,
            offset:    self.offset,
            alignment: self.alignment 
        }
    }
}


impl<'f, F> Default for Layout<'f, F> {
    fn default() -> Self {
        Self { 
            contents:  Vec::default(),
            width:     Unit::ZERO,
            height:    Unit::ZERO,
            depth:     Unit::ZERO,
            offset:    Unit::ZERO,
            alignment: Alignment::default(),
        }
    }
}

impl<'f, F> Layout<'f, F> {
    /// Make layout into layout node, which can then be inserted in another layout
    pub fn as_node(self) -> LayoutNode<'f, F> {
        LayoutNode {
            width: self.width,
            height: self.height,
            depth: self.depth,
            node: LayoutVariant::HorizontalBox(
                HorizontalBox {
                   contents: self.contents,
                   offset: self.offset,
                   alignment: self.alignment,
                }
            ),
        }
    }

    /// Create new [`Layout`](crate::layout::Layout) ; equivalent to [Default::default]
    pub fn new() -> Layout<'f, F> {
        Layout::default()
    }

    /// Append node at end of layout (i.e. right of layout)
    pub fn add_node(&mut self, node: LayoutNode<'f, F>) {
        self.width += node.width;
        self.height = Unit::max(self.height, node.height);
        self.depth  = Unit::min(self.depth, node.depth);
        self.contents.push(node);
    }

    /// Sets offset of layout
    // TODO: not used at the moment figure out why (perhaps lack of alignement in array?)
    pub fn set_offset(&mut self, offset: Unit<Px>) {
        self.offset = offset;
    }

    /// Not clear what this does.
    pub fn finalize(mut self) -> Layout<'f, F> {
        self.depth -= self.offset;
        self.height -= self.offset;
        self
    }

    /// Makes layout's width equal to given arguments, and centers children within that width
    pub fn centered(mut self, new_width: Unit<Px>) -> Layout<'f, F> {
        self.alignment = Alignment::Centered(self.width);
        self.width = new_width;
        self
    }

    /// Returns [`LayoutDimensions`] dimensions for the given layout, in pixels. 
    pub fn size(&self) -> LayoutDimensions {
        LayoutDimensions {
            width  : self.width.unitless(Px),
            height : self.height.unitless(Px),
            depth  : self.depth.unitless(Px),
        }
    }


}

impl<'f, F> Layout<'f, F> {
    fn is_symbol(&self) -> Option<LayoutGlyph<'f, F>> {
        if self.contents.len() != 1 {
            return None;
        }
        self.contents[0].is_symbol()
    }
}

impl<'f, F : MathFont> Layout<'f, F> {
    /// Returns the true bounding box of the formula, the one that encloses all glyphs and all rules.
    /// The coordinates are given taking the origin to be in (0, 0).
    /// Returns `None` if the layout contains no glyphs or rules
    /// Note: it may still contain whitespaces! 
    pub fn bounding_box(&self) -> Option<LayoutBBox> {
        let mut bbox_renderer = BBoxBackend::new();
        Renderer::new().render(self, &mut bbox_renderer);
        let bbox = bbox_renderer.finish()?;

        Some(LayoutBBox { 
            x_min: bbox.x_min.unitless(Px), 
            x_max: bbox.x_max.unitless(Px), 
            y_min: bbox.y_min.unitless(Px), 
            y_max: bbox.y_max.unitless(Px), 
        })        
    }

    /// Returns the union of the bounding box that encloses all glyphs and rules (as returned by [`Layout::bounding_box`])
    /// and the 'typographical' bounding box (containing white spaces)
    pub fn full_bounding_box(&self) -> LayoutBBox {
        let content_bbox = self.bounding_box().unwrap_or_else(|| LayoutBBox { x_min: 0., x_max: 0., y_min: 0., y_max: 0. });
        let LayoutDimensions { width, height, depth } = self.size();
        content_bbox.union(LayoutBBox {
            x_min: 0.,
            x_max: width,
            y_min: height,
            y_max: depth,
        })
    }
}

/// A struct containing various measures for a [`Layout`] in pixel units.
// Should not be used internally, the unitless types are "unsafe"
#[derive(Debug, Clone)]
pub struct LayoutDimensions {
    /// distance from baseline to top of the formula (positive if top of formula above baseline, typically positive)
    pub width  : f64,
    /// distance from baseline to bottom of the formula (negative if bottom of formula below baseline, typically negative)
    pub height : f64,
    /// width of formula
    pub depth  : f64,
}

/// A struct representing the true bounding box of the formula, enclosing all glyphs and all rules.
/// The coordinates are given taking the origin to be in (0, 0).
// Should not be used internally, the unitless types are "unsafe"
#[derive(Debug, Clone)]
pub struct LayoutBBox {
    /// minimum x-coordinate
    pub x_min : f64,
    /// maximum x-coordinate
    pub x_max : f64,
    /// minimum y-coordinate
    pub y_min : f64,
    /// maximum y-coordinate
    pub y_max : f64,
}

impl LayoutBBox {
    /// Returns the width of the bounding box
    pub fn width(&self) -> f64 {
        self.x_max - self.x_min
    }

    /// Returns the height of the bounding box
    pub fn height(&self) -> f64 {
        self.y_max - self.y_min
    }

    /// Returns the smallest bbox enclosing both `şelf` and `other`
    pub fn union(&self, other: Self) -> Self {
        Self { 
            x_min: f64::min(self.x_min, other.x_min), 
            y_min: f64::min(self.y_min, other.y_min), 
            x_max: f64::max(self.x_max, other.x_max), 
            y_max: f64::max(self.y_max, other.y_max), 
        }
    }
}

/// A sub-part of the layout hierarchy: can contain other nodes and may be contained in other nodes.
pub struct LayoutNode<'f, F> {
    /// Type of node
    pub node: LayoutVariant<'f, F>,
    /// Width
    pub width: Unit<Px>,
    /// Height: distance from base line to top of the node
    pub height: Unit<Px>,
    /// Height: distance from base line to bottom of the node
    pub depth: Unit<Px>,
}

impl<'f, F> Clone for LayoutNode<'f, F> {
    fn clone(&self) -> Self {
        Self {
            node:   self.node.clone(),
            width:  self.width.clone(),
            height: self.height.clone(),
            depth:  self.depth.clone(),
        }
    }
}

/// Different types of layout nodes
pub enum LayoutVariant<'f, F> {
    /// A grid
    Grid(Grid<'f, F>),
    /// A horizontal box
    HorizontalBox(HorizontalBox<'f, F>),
    /// A vertical box
    VerticalBox(VerticalBox<'f, F>),
    /// A symbol (aka glyph) from the font
    Glyph(LayoutGlyph<'f, F>),
    /// A scope within which the main color is changed
    Color(ColorChange<'f, F>),
    /// A filled rectangle
    Rule,
    /// Some (possibly negative) spacing
    Kern,
}

impl<'f, F> Clone for LayoutVariant<'f, F> {
    fn clone(&self) -> Self {
        match self {
            LayoutVariant::Grid(grid)             => LayoutVariant::Grid(grid.clone()),
            LayoutVariant::HorizontalBox(hbox)    => LayoutVariant::HorizontalBox(hbox.clone()),
            LayoutVariant::VerticalBox(vbox)      => LayoutVariant::VerticalBox(vbox.clone()),
            LayoutVariant::Glyph(glyph)           => LayoutVariant::Glyph(glyph.clone()),
            LayoutVariant::Color(color_change)    => LayoutVariant::Color(color_change.clone()),
            LayoutVariant::Rule                   => LayoutVariant::Rule,
            LayoutVariant::Kern                   => LayoutVariant::Kern,
        }
    }
}

/// All children of this node will use the [ColorChange::color] as a fill color
pub struct ColorChange<'f, F> {
    /// Color to use
    pub color: RGBA,
    /// Children of the given node
    pub inner: Vec<LayoutNode<'f, F>>,
}

impl<'f, F> Clone for ColorChange<'f, F> {
    fn clone(&self) -> Self {
        Self { 
            color: self.color, 
            inner: self.inner.clone(),
        }
    }
}

/// Place nodes in a grid-like pattern. 
/// The number of rows and columns is determined automatically
pub struct Grid<'f, F> {
    /// Children nodes and their position in the grid
    pub contents: BTreeMap<(usize, usize), LayoutNode<'f, F>>,
    /// max length of each column
    pub columns: Vec<Unit<Px>>,
    /// (max height, max depth) of each row
    pub rows: Vec<(Unit<Px>, Unit<Px>)>,
}

impl<'f, F> Clone for Grid<'f, F> {
    fn clone(&self) -> Self {
        Self {
            contents: self.contents.clone(),
            columns:  self.columns.clone(),
            rows:     self.rows.clone(),
        }
    }
}

/// A horizontal box : children are placed side by side.
pub struct HorizontalBox<'f, F> {
    /// Children nodes
    pub contents: Vec<LayoutNode<'f, F>>,
    /// Offset
    // Unclear what this does
    pub offset: Unit<Px>,
    /// How to align Children nodes
    pub alignment: Alignment,
}

impl<'f, F> Clone for HorizontalBox<'f, F> {
    fn clone(&self) -> Self {
        Self {
            contents:  self.contents.clone(),
            offset:    self.offset.clone(),
            alignment: self.alignment.clone(),
        }
    }
}


// NOTE: A limitation on derive(Clone, Default) forces us to implement clone ourselves.
// cf discussion here: https://stegosaurusdormant.com/understanding-derive-clone/
impl<'f, F> Default for HorizontalBox<'f, F> {
    fn default() -> Self {
        Self { 
            contents:  Vec::default(), 
            offset:    Unit::ZERO, 
            alignment: Alignment::default(), 
        }
    }
}

/// Vertical box: children are placed on top of each other
pub struct VerticalBox<'f, F> {
    /// Children nodes
    pub contents: Vec<LayoutNode<'f, F>>,
    /// Offset from baseline
    pub offset: Unit<Px>,
    /// Horizontal alignment
    pub alignment: Alignment,
}

impl<'f, F> Clone for VerticalBox<'f, F> {
    fn clone(&self) -> Self {
        Self {
            contents:  self.contents.clone(),
            offset:    self.offset.clone(),
            alignment: self.alignment.clone(),
        }
    }
}

// NOTE: A limitation on derive(Clone, Default) forces us to implement clone ourselves.
// cf discussion here: https://stegosaurusdormant.com/understanding-derive-clone/
impl<'f, F> Default for VerticalBox<'f, F> {
    fn default() -> Self {
        Self { 
            contents:  Vec::default(), 
            offset:    Unit::ZERO, 
            alignment: Alignment::default(), 
        }
    }
}


/// Glyph : this node has no children ; simply specify some glyph (i.e. symbol) to draw
pub struct LayoutGlyph<'f, F> {
    /// glyph id
    pub gid: GlyphId,
    /// width of the symbol
    pub size: Unit<Px>,
    /// offset from baseline
    pub offset: Unit<Px>,
    /// where to place accents
    pub attachment: Unit<Px>,
    /// italic correction: italic symbols, who lean, may come out of the bounding box towards the top ; when the next glyph is not as slanted (i.e. is not italic), the italic glyph may collide with next glyph.
    /// The "italic correction" tells one how much wider the glyph needs to be in order to avoid any collisions with subsequent glyphs.
    pub italics: Unit<Px>,
    /// font to render glyph with
    pub font: &'f F,
}


impl<'f, F> Clone for LayoutGlyph<'f, F> {
    fn clone(&self) -> Self {
        Self {
            gid:        self.gid,
            size:       self.size,
            offset:     self.offset,
            attachment: self.attachment,
            italics:    self.italics,
            font:       self.font,
        }
    }
}
impl<'f, F> Copy for LayoutGlyph<'f, F> {}

// NOTE: A limitation on derive(Clone) forces us to implement clone ourselves.
// cf discussion here: https://stegosaurusdormant.com/understanding-derive-clone/


#[allow(dead_code)]
#[derive(Copy, Clone, Debug, PartialEq)]
/// How to horizontally align certain elements
pub enum Alignment {
    /// Centered within the argument width
    Centered(Unit<Px>),
    /// Right-aligned within the argument width
    Right(Unit<Px>),
    /// Placed left to right, one after the other ; width is determined automatically
    Left,
    /// inherit from previous
    Inherit,
    /// default
    Default,
}

impl Default for Alignment {
    fn default() -> Alignment {
        Alignment::Default
    }
}

impl<'f, F> Deref for HorizontalBox<'f, F> {
    type Target = [LayoutNode<'f, F>];
    fn deref(&self) -> &Self::Target {
        &self.contents
    }
}

impl<'f, F> Deref for VerticalBox<'f, F> {
    type Target = [LayoutNode<'f, F>];
    fn deref(&self) -> &Self::Target {
        &self.contents
    }
}

impl<'f, F> fmt::Debug for VerticalBox<'f, F> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.offset.is_zero() {
            write!(f, "VerticalBox({:?})", self.contents)
        } else {
            write!(f,
                   "VerticalBox({:?}, offset: {})",
                   self.contents,
                   self.offset)
        }
    }
}

impl<'f, F> fmt::Debug for HorizontalBox<'f, F> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "HorizontalBox({:?})", self.contents)
    }
}

impl<'f, F> fmt::Debug for LayoutGlyph<'f, F> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "LayoutGlyph({})", Into::<u16>::into(self.gid))
    }
}

impl<'f, F> fmt::Debug for LayoutNode<'f, F> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() {
            match self.node {
                LayoutVariant::Grid(ref _grid)       => write!(f, "Grid(..)"),
                LayoutVariant::HorizontalBox(ref hb) => write!(f, "HBox({:#?})", hb.contents),
                LayoutVariant::VerticalBox(ref vb)   => write!(f, "VBox({:#?})", vb.contents),
                LayoutVariant::Glyph(ref gly)        => write!(f, "Glyph({:#?})", gly),
                LayoutVariant::Rule                  => write!(f, "Rule()"),
                LayoutVariant::Kern                  => {
                    let kern = if self.width.is_zero() {
                        self.height
                    } else {
                        self.width
                    };

                    write!(f, "Kern({:.1})", kern)
                }
                LayoutVariant::Color(ref clr) => write!(f, "Color({:#?}, {:#?})", clr.color, clr.inner),
            }
        }
        else {
            match self.node {
                LayoutVariant::Grid(ref _grid)       => write!(f, "Grid(..)"),
                LayoutVariant::HorizontalBox(ref hb) => write!(f, "HBox({:?})", hb.contents),
                LayoutVariant::VerticalBox(ref vb)   => write!(f, "VBox({:?})", vb.contents),
                LayoutVariant::Glyph(ref gly)        => write!(f, "Glyph({:?})", gly),
                LayoutVariant::Rule                  => write!(f, "Rule()"),
                LayoutVariant::Kern                  => {
                    let kern = if self.width.is_zero() {
                        self.height
                    } else {
                        self.width
                    };

                    write!(f, "Kern({:.1})", kern)
                }
                LayoutVariant::Color(ref clr) => write!(f, "Color({:?}, {:?})", clr.color, clr.inner),
            }
        }
    }
}


impl<'f, F> LayoutNode<'f, F> {
    /// A node of the given height and zero width to be used as vertical kern 
    /// (aka vertical space) between nodes
    fn vert_kern(height : Unit<Px>) -> Self {
        LayoutNode {
            width:  Unit::ZERO,
            height,
            depth:  Unit::ZERO,
            node:   LayoutVariant::Kern,
        }
    }
    
    /// A node of the given width and zero height to be used as horizontal kern 
    /// (aka horizontal space) between nodes
    fn horiz_kern(width : Unit<Px>) -> Self {
        LayoutNode {
            width,
            height: Unit::ZERO,
            depth:  Unit::ZERO,
            node:   LayoutVariant::Kern,
        }
    }
    
    /// Center the vertical about the axis.
    /// For now this ignores offsets if already applied,
    /// and will break if there already are offsets.
    fn centered(mut self, axis: Unit<Px>) -> LayoutNode<'f, F> {
        let shift = (self.height + self.depth).scale(0.5) - axis;

        match self.node {
            LayoutVariant::VerticalBox(ref mut vb) => {
                vb.offset = shift;
                self.height -= shift;
                self.depth -= shift;
            }

            LayoutVariant::Glyph(_) => return vbox!(offset: shift; self),

            _ => (),
        }

        self
    }

    fn is_symbol(&self) -> Option<LayoutGlyph<'f, F>> {
        match self.node {
            LayoutVariant::Glyph(gly) => Some(gly),
            LayoutVariant::HorizontalBox(ref hb) => is_symbol(&hb.contents),
            LayoutVariant::VerticalBox(ref vb) => is_symbol(&vb.contents),
            LayoutVariant::Color(ref clr) => is_symbol(&clr.inner),
            _ => None,
        }
    }
}

/// Determines if a set of nodes is a singleton set containing a symbol node
pub fn is_symbol<'a, 'b: 'a, F>(contents: &'a [LayoutNode<'b, F>]) -> Option<LayoutGlyph<'b, F>> {
    if contents.len() != 1 {
        return None;
    }

    contents[0].is_symbol()
}

/// Display styles which are used in scaling glyphs.  The associated
/// methods are taken from pg.441 from the TeXBook
#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Style {
    /// sub-script of sub-script with no spacing
    ScriptScriptCramped,
    /// sub-script of sub-script
    ScriptScript,
    /// sub-script with no sapce
    ScriptCramped,
    /// sub-script 
    Script,
    /// TODO
    TextCramped,
    /// This is the default style when writing formulas within a text paragraph, i.e. what you get with `$...$` in LaTeX.  
    /// Among other things, it places limits on ∑ and ∫ as exponents not above and below the symbol to avoid making the text line too big.
    Text,
    /// TODO
    DisplayCramped,
    /// This is the default style when writing formulas in its own paragraph, i.e. what you get with `$$...$$` or `\[...\]` in LaTeX.  
    /// It places limits on ∑ and ∫ above and below the symbols, as would be standard when writing the formulas on paper.
    Display,
}

impl Default for Style {
    fn default() -> Style {
        Style::Display
    }
}

#[allow(dead_code)]
impl Style {
    fn cramped(self) -> Style {
        match self {
            Style::ScriptScriptCramped |
            Style::ScriptScript => Style::ScriptScriptCramped,
            Style::ScriptCramped | Style::Script => Style::ScriptCramped,
            Style::TextCramped | Style::Text => Style::TextCramped,
            Style::DisplayCramped | Style::Display => Style::DisplayCramped,
        }
    }

    fn superscript_variant(self) -> Style {
        match self {
            Style::Display | Style::Text => Style::Script,
            Style::DisplayCramped | Style::TextCramped => Style::ScriptCramped,
            Style::Script | Style::ScriptScript => Style::ScriptScript,
            Style::ScriptCramped |
            Style::ScriptScriptCramped => Style::ScriptScriptCramped,
        }
    }

    fn subscript_variant(self) -> Style {
        match self {
            Style::Display | Style::Text | Style::DisplayCramped | Style::TextCramped => {
                Style::ScriptCramped
            }
            Style::Script |
            Style::ScriptScript |
            Style::ScriptCramped |
            Style::ScriptScriptCramped => Style::ScriptScriptCramped,
        }
    }

    fn sup_shift_up<F>(self, font_metrics_cache: &FontMetricsCache) -> Unit<Em> {
        match self {
            Style::Display | Style::Text | Style::Script | Style::ScriptScript => {
                font_metrics_cache.constants().superscript_shift_up
            }
            _ => font_metrics_cache.constants().superscript_shift_up_cramped
        }
    }

    fn is_cramped(&self) -> bool {
        match *self {
            Style::Display | Style::Text | Style::Script | Style::ScriptScript => false,
            _ => true,
        }
    }

    fn numerator(self) -> Style {
        match self {
            Style::Display => Style::Text,
            Style::DisplayCramped => Style::TextCramped,
            _ => self.superscript_variant(),
        }
    }

    fn denominator(self) -> Style {
        match self {
            Style::Display | Style::DisplayCramped => Style::TextCramped,
            _ => self.subscript_variant(),
        }
    }

    fn script_level(self) -> Option<ScriptLevel> {
        match self {
            Style::ScriptScriptCramped 
            | Style::ScriptScript => Some(ScriptLevel::LevelTwo),
            
            Style::ScriptCramped 
            | Style::Script => Some(ScriptLevel::LevelOne),
            
            Style::TextCramped 
            | Style::Text 
            | Style::DisplayCramped 
            | Style::Display => None,
        }
    }
}
