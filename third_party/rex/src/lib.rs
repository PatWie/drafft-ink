/*! # A mathematical typesetting engine based on LuaTeX and XeTeX.

This is a Rust mathematical typesetting engine library. It takes a formula written in TeX syntax (e.g. `\cos\frac{\pi}{4}`) and renders it to a screen, an image, etc.

This is a fork of [ReX](https://github.com/ReTeX/ReX) incorporating modifications by s3bk.

## Basic usage

To render a formula, you need two ingredients: a mathematical font and a graphical backend. 

First, load and parse the font with e.g. the `ttf-parser` crate.

```no_run
// create font backend
let font_file = std::fs::read("font.otf").expect("Couldn't load font");
let font = ttf_parser::Face::parse(&font_file, 0).expect("Couldn't parse font.");
let math_font = rex::font::backend::ttf_parser::TtfMathFont::new(font).expect("The font likely lacks a MATH table"); // extracts math info from font
```

Second, create the graphical backend, e.g. with `cairo` here

```no_run
# // create font backend
# let font_file = std::fs::read("font.otf").expect("Couldn't load font");
# let font = ttf_parser::Face::parse(&font_file, 0).expect("Couldn't parse font.");
# let math_font = rex::font::backend::ttf_parser::TtfMathFont::new(font).expect("The font likely lacks a MATH table"); // extracts math info from font
# 
// create graphics backend
let svg_surface = cairo::SvgSurface::new(800., 600., Some("out.svg")).expect("Couldn't create SVG surface");
let context = cairo::Context::new(&svg_surface).expect("Couldn't get context for SVG surface");
// The (0, 0) point is the baseline of the first glyph we move it to a reasonable place
context.translate(0., 300.);
let mut backend = rex::cairo::CairoBackend::new(context);
```

With font and backend, a call to `render` will render the formula:


```no_run
# // create font backend
# let font_file = std::fs::read("font.otf").expect("Couldn't load font");
# let font = ttf_parser::Face::parse(&font_file, 0).expect("Couldn't parse font.");
# let math_font = rex::font::backend::ttf_parser::TtfMathFont::new(font).expect("The font likely lacks a MATH table"); // extracts math info from font
# 
# 
# // create graphics backend
# let svg_surface = cairo::SvgSurface::new(800., 600., Some("out.svg")).expect("Couldn't create SVG surface");
# let context = cairo::Context::new(&svg_surface).expect("Couldn't get context for SVG surface");
# // The (0, 0) point is the baseline of the first glyph we move it to a reasonable place
# context.translate(0., 300.);
# let mut backend = rex::cairo::CairoBackend::new(context);
#
#
rex::render(
  r"e = \lim_{n \to \infty} \left(1 + \frac{1}{n}\right)^n", 
  &mut backend,
  &math_font,
).expect("Error in rendering");
```

Notes:

 - `render` places the first glyph's origin at (0, 0), using a font size of 16 surface units per em.
 - The surface unit is whatever unit is used when making calls to the graphical backend directly. Here, with Cairo, these are pixels.
 - em is a unit with which the size of glyphs are expressed in the font file ; by convention, it is approximately, although not necessarily equal to, the size of upper case letter M.



## More complex cases

You sometimes need more control. For instance, you may want to get the dimensions of formula to be able to center it. 
The call to `render` is a wrapper around three operations and we may break these steps apart to get more control:

  1. Parsing the formula into [`ParseNode`](`crate::parser::ParseNode`)s, cf [`parse`](crate::parser::parse).
  2. Laying out the [`ParseNode`](`crate::parser::ParseNode`)s in space relative to each other, yielding a [`Layout`](crate::layout::Layout), cf [`layout`](crate::layout::engine::layout). 
     This step requires some mathematical OpenType font to provide various spacing parameters and some other info, like desired font size, cf [`LayoutSettings`](crate::layout::LayoutSettings).
  3. Drawing the nodes on a certain graphical backend (e.g. the screen, a SVG file, etc). [`Renderer`](crate::render::Renderer) is used to that end, and especially the method [Renderer::render](`crate::render::Renderer::render`).
     This step requires a certain graphical backend to be set up (e.g. [Cairo](https://gtk-rs.org/gtk-rs-core/stable/latest/docs/cairo/), [femtovg](https://docs.rs/femtovg/latest/femtovg/index.html), [pathfinder](https://github.com/servo/pathfinder)).
 

We can perform these steps manually. First, we parse the formula:

```
// Step 1: parsing formula into nodes
let parse_nodes = rex::parser::parse(r"e = \lim_{n \to \infty} \left(1 + \frac{1}{n}\right)^n").expect("Parse error");
```

Then, with the font backend loaded, we lay out the nodes in space:

```no_run
# // Step 1: parsing formula into nodes
# let parse_nodes = rex::parser::parse(r"e = \lim_{n \to \infty} \left(1 + \frac{1}{n}\right)^n").expect("Parse error");
# 
// create font backend
let font_file = std::fs::read("font.otf").expect("Couldn't load font");
let font = ttf_parser::Face::parse(&font_file, 0).expect("Couldn't parse font.");
let math_font = rex::font::backend::ttf_parser::TtfMathFont::new(font).expect("The font likely lacks a MATH table"); // extracts math info from font


// Step 2: lay out nodes in space
let font_size : f64 = 10.; // in surface units per em
// "display style" is the style used for typesetting $$...$$ formulas in LaTeX (as opposed to $...$ formulas)
let layout = rex::layout::engine::LayoutBuilder::new(&math_font).layout(&parse_nodes).expect("Font error"); // may fail if your font lacks some glyphs or does not contain some needed MATH info
```

The layout contains useful information like the dimension of the formulas. We use this to center:


```no_run
# // Step 1: parsing formula into nodes
# let parse_nodes = rex::parser::parse(r"e = \lim_{n \to \infty} \left(1 + \frac{1}{n}\right)^n").expect("Parse error");
# 
# // create font backend
# let font_file = std::fs::read("font.otf").expect("Couldn't load font");
# let font = ttf_parser::Face::parse(&font_file, 0).expect("Couldn't parse font.");
# let math_font = rex::font::backend::ttf_parser::TtfMathFont::new(font).expect("The font likely lacks a MATH table"); // extracts math info from font
# 
# // Step 2: lay out nodes in space
# let font_size : f64 = 10.; // in surface units per em
# // "display style" is the style used for typesetting $$...$$ formulas in LaTeX (as opposed to $...$ formulas)
# let layout = rex::layout::engine::LayoutBuilder::new(&math_font).layout(&parse_nodes).expect("Font error"); // may fail if your font lacks some glyphs or does not contain some needed MATH info
# 
let size   = layout.size();
let total_height = size.height + size.depth; // 'height' is dist from baseline to highest point and 'depth' is dist from baseline to lowest point
let total_width  = size.width;

let svg_surface = cairo::SvgSurface::new(total_width, total_height, Some("out.svg")).expect("Couldn't create SVG surface");
let context = cairo::Context::new(&svg_surface).expect("Couldn't get context for SVG surface");
context.translate(total_height / 2., 0.);
let mut backend = rex::cairo::CairoBackend::new(context);
```

Finally, we may render:

```no_run
# // Step 1: parsing formula into nodes
# let parse_nodes = rex::parser::parse(r"e = \lim_{n \to \infty} \left(1 + \frac{1}{n}\right)^n").expect("Parse error");
# 
# // create font backend
# let font_file = std::fs::read("font.otf").expect("Couldn't load font");
# let font = ttf_parser::Face::parse(&font_file, 0).expect("Couldn't parse font.");
# let math_font = rex::font::backend::ttf_parser::TtfMathFont::new(font).expect("The font likely lacks a MATH table"); // extracts math info from font
# 
# // Step 2: lay out nodes in space
# let font_size : f64 = 10.; // in surface units per em
# // "display style" is the style used for typesetting $$...$$ formulas in LaTeX (as opposed to $...$ formulas)
# let layout = rex::layout::engine::LayoutBuilder::new(&math_font).layout(&parse_nodes).expect("Font error"); // may fail if your font lacks some glyphs or does not contain some needed MATH info
# 
# let size   = layout.size();
# let total_height = size.height + size.depth; // 'height' is dist from baseline to highest point and 'depth' is dist from baseline to lowest point
# let total_width  = size.width;
# 
# let svg_surface = cairo::SvgSurface::new(total_width, total_height, Some("out.svg")).expect("Couldn't create SVG surface");
# let context = cairo::Context::new(&svg_surface).expect("Couldn't get context for SVG surface");
# context.translate(total_height / 2., 0.);
# let mut backend = rex::cairo::CairoBackend::new(context);
let renderer = rex::render::Renderer::new();
renderer.render(&layout, &mut backend);
```


 
## Implementing backends

The crate gives you freedom to use your favorite font parsing crate, by implementing the [`MathFont`](crate::font::MathFont) trait and your favourite graphical backend, by implementing [`Backend`](crate::render::Backend). 
Some features provide implementations for common font parsing crates and graphical backends.
 
### Font parser backend

The [`MathFont`](crate::font::MathFont) trait demands access to certain information from an otf font file, such as access to a certain list of mathematical parameters from the font table, how to construct
extended versions of certain glyphs

### Graphical backend

The [`Backend`](crate::render::Backend) trait consists of two traits: [`FontBackend<F>`](crate::render::FontBackend) and [`GraphicsBackend`](crate::render::GraphicsBackend).
The [`FontBackend<F>`](crate::render::FontBackend) consists in the method `symbol` to draw a glyph, given a particular `F` implementing [`MathFont`](crate::font::MathFont).
The [`GraphicsBackend`](crate::render::GraphicsBackend) only contains drawing methods that do not require a particular font: drawing boxes, drawing lines, pushing a certain color on the stack, etc.
*/



#[macro_use]
extern crate serde_derive;



extern crate log;

#[macro_use]
mod macros;

#[deny(missing_docs)]
mod geometry;
#[deny(missing_docs)]
pub mod error;
#[deny(missing_docs)]
pub mod dimensions;
#[deny(missing_docs)]
pub mod layout;
#[warn(missing_docs)]
pub mod parser;
#[deny(missing_docs)]
pub mod render;

pub mod font;

use font::MathFont;
use layout::engine::LayoutBuilder;
pub use render::*;

use crate::parser::parse;


/// Render a LateX formula to a given a surface `backend`, given a math font provided by `font_context`.
pub fn render<F : MathFont, B : Backend<F>>(formula : &str, backend : &mut B, font: &F) -> Result<(), crate::error::Error> {
    let parse_nodes = parse(formula)?;



    let layout = LayoutBuilder::new(font).layout(&parse_nodes)?;


    let renderer = Renderer::new();
    renderer.render(&layout, backend);
    Ok(())
}
