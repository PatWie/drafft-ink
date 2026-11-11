//! The text engine of the stage: a lazily built [`Typesetter`], sizing of text and formulas, and the adapters
//! the SVG exporter needs.
//!
//! Building the typesetter registers the bundled fonts, so a stage that never shows text never pays for it.

use std::cell::RefCell;

use drafftink_io::TypesetterMeasure;
use drafftink_render::{math_spec, text_spec};
use drafftink_scene::{Element, ElementKind};
use drafftink_typeset::Typesetter;

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

    /// Measures text and formulas for the SVG exporter with the stage's typesetter.
    pub(crate) fn svg_text(&mut self) -> TypesetterMeasure<RefCell<&mut Typesetter>> {
        TypesetterMeasure(RefCell::new(self.typesetter()))
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
