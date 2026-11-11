//! The text engine of the stage: a lazily built [`Typesetter`], sizing of text and formulas, and the adapters
//! the SVG exporter needs.
//!
//! Building the typesetter registers the bundled fonts, so a stage that never shows text never pays for it.

use std::cell::RefCell;
use std::fmt::Write as _;

use drafftink_io::MathBox;
use drafftink_io::measure::TextMeasure;
use drafftink_io::svg::MathSvg;
use drafftink_render::{math_spec, text_spec};
use drafftink_scene::{Color, Element, ElementKind, FontFamily, FontWeight, TextData};
use drafftink_typeset::{Rgba8, Typesetter};

/// The typesetter, built on first use.
#[derive(Default)]
pub(crate) struct Fonts {
    typesetter: Option<Typesetter>,
}

impl Fonts {
    /// The typesetter.
    pub(crate) fn typesetter(&mut self) -> &mut Typesetter {
        self.typesetter.get_or_insert_with(Typesetter::new)
    }

    /// Brings a new or patched element into the state the store expects: the points of a line, arrow or stroke are
    /// refitted to their box, and a text or formula gets the size its content needs.
    pub(crate) fn settle(&mut self, element: &mut Element) {
        if element.points().is_some() {
            // A false answer means no points or a non-finite one; the store's validation reports that.
            element.refit_points();
        }
        match &element.kind {
            ElementKind::Text(text) => {
                let spec = text_spec(text, element.style.stroke_color, element.transform.w);
                let size = self.typesetter().measure(&spec);
                if text.auto_resize {
                    element.transform.w = size.width;
                }
                element.transform.h = size.height;
            }
            ElementKind::Math(math) => {
                if let Ok(layout) = self.typesetter().math(&math_spec(math)) {
                    let size = layout.size();
                    element.transform.w = size.width;
                    element.transform.h = size.height;
                }
            }
            _ => {}
        }
    }
}

/// Measures text and formulas for the SVG exporter with the stage's typesetter.
pub(crate) struct SvgText<'a>(RefCell<&'a mut Typesetter>);

/// The colour handed to the outline for "the shape colour", which the SVG then writes as `currentColor`.
const SENTINEL: Rgba8 = Rgba8::new(1, 2, 3, 4);

impl<'a> SvgText<'a> {
    pub(crate) fn new(typesetter: &'a mut Typesetter) -> Self {
        Self(RefCell::new(typesetter))
    }
}

impl TextMeasure for SvgText<'_> {
    fn text_size(&self, text: &str, family: FontFamily, weight: FontWeight, size: f64, line_height: f64) -> (f64, f64) {
        let mut data = TextData::new(text);
        data.font.family = family;
        data.font.weight = weight;
        if size.is_finite() && size > 0.0 {
            data.font.size = size;
        }
        if line_height.is_finite() && line_height > 0.0 {
            data.font.line_height = line_height;
        }
        let spec = text_spec(&data, Color::BLACK, 0.0);
        let measured = self.0.borrow_mut().measure(&spec);
        (measured.width.max(0.0), measured.height.max(0.0))
    }

    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox> {
        let mut data = drafftink_scene::MathData::new(latex);
        if size.is_finite() && size > 0.0 {
            data.font_size = size;
        }
        let layout = self.0.borrow_mut().math(&math_spec(&data)).ok()?;
        Some(MathBox { width: layout.width.max(0.0), height: layout.height.max(0.0), depth: (-layout.depth).max(0.0) })
    }
}

impl MathSvg for SvgText<'_> {
    fn math_svg(&self, latex: &str, size: f64) -> Option<String> {
        let mut data = drafftink_scene::MathData::new(latex);
        if size.is_finite() && size > 0.0 {
            data.font_size = size;
        }
        let layout = self.0.borrow_mut().math(&math_spec(&data)).ok()?;
        let mut out = String::new();
        for (color, path) in layout.outline(SENTINEL) {
            let d = path.to_svg();
            if color == SENTINEL {
                let _ = write!(out, r#"<path d="{d}" fill="currentColor"/>"#);
            } else {
                let _ = write!(out, r##"<path d="{d}" fill="#{:02x}{:02x}{:02x}"/>"##, color.r, color.g, color.b);
            }
        }
        Some(out)
    }
}
