//! Whether an element shows anything: the one rule behind the refusal of a style that leaves nothing to see
//! (`drafftink-edit`) and the dotted hint that marks such an element on screen (`drafftink-canvas`).

use crate::{Element, ElementKind, Kind, Style};

/// The painter multiplies every colour's alpha by the element opacity and truncates to a byte, so an opacity
/// below one alpha step leaves even an opaque colour clear.
const ONE_ALPHA_STEP: f64 = 1.0 / 255.0;

/// True when a fill can paint something for an element of `kind`: shapes, and closed lines and pencil strokes.
pub fn fill_paints(kind: Kind, closed: bool) -> bool {
    match kind {
        Kind::Rectangle | Kind::Diamond | Kind::Ellipse => true,
        Kind::Line | Kind::Freehand => closed,
        _ => false,
    }
}

/// Whether an element of `kind` shows anything with `style`, colours only (opacity is [`Element::shows_nothing`]'s
/// business). A stroke colour with alpha 0 is "no stroke" (what Excalidraw calls `transparent`), and no fill is a
/// missing or fully clear fill colour.
///
/// Rectangles, diamonds, ellipses, closed lines and closed pencil strokes show with a fill alone. Everything else
/// is drawn in its stroke colour: text, formulas, open lines, arrows, open pencil strokes, and frames (outline and
/// name). Images always show: their stroke is an optional outline, so "no stroke" is allowed for them.
pub fn style_shows(kind: Kind, closed: bool, style: &Style) -> bool {
    if kind == Kind::Image {
        return true;
    }
    let fills = fill_paints(kind, closed) && style.fill_color.is_some_and(|c| c.a > 0);
    style.stroke_color.a > 0 || fills
}

impl Element {
    /// True for a closed line, arrow or pencil stroke: the kinds whose fill paints an inside.
    pub fn is_closed_outline(&self) -> bool {
        match &self.kind {
            ElementKind::Line(d) | ElementKind::Arrow(d) => d.closed,
            ElementKind::Freehand(f) => f.closed && !f.highlighter,
            _ => false,
        }
    }

    /// True when the element draws nothing: its opacity is so low that every colour ends up clear, or its colours
    /// show nothing ([`style_shows`]). Such an element still exists, is hit and selected like any other, and is
    /// kept by every export. Only the screen marks it, so that a person can find it.
    pub fn shows_nothing(&self) -> bool {
        // A non-finite opacity is drawn as fully opaque (the painter's rule), so it does not hide anything.
        let faded_out = self.style.opacity.is_finite() && self.style.opacity < ONE_ALPHA_STEP;
        faded_out || !style_shows(self.tag(), self.is_closed_outline(), &self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, ElementId, LinearData, MathData, TextData, ZIndex};
    use kurbo::Point;

    fn el(kind: ElementKind) -> Element {
        let id = ElementId::new("a").unwrap();
        Element::new(id, kind, ZIndex::first()).with_bounds(0.0, 0.0, 40.0, 20.0)
    }

    fn line() -> ElementKind {
        ElementKind::Line(LinearData::new(vec![Point::ZERO, Point::new(5.0, 5.0)]))
    }

    fn style(stroke: Color, fill: Option<Color>) -> Style {
        Style { stroke_color: stroke, fill_color: fill, ..Style::default() }
    }

    #[test]
    fn a_default_element_shows_something() {
        for kind in [ElementKind::Rectangle, ElementKind::Diamond, ElementKind::Ellipse, line()] {
            assert!(!el(kind).shows_nothing());
        }
    }

    #[test]
    fn opacity_below_one_alpha_step_shows_nothing_whatever_the_colours() {
        let mut e = el(ElementKind::Rectangle);
        e.style.fill_color = Some(Color::rgb(1, 2, 3));
        e.style.opacity = 0.0;
        assert!(e.shows_nothing());
        e.style.opacity = 0.003;
        assert!(e.shows_nothing(), "255 * 0.003 truncates to alpha 0");
        e.style.opacity = 0.005;
        assert!(!e.shows_nothing(), "255 * 0.005 is alpha 1");
        e.style.opacity = f64::NAN;
        assert!(!e.shows_nothing(), "the painter draws a NaN opacity as opaque");
    }

    #[test]
    fn a_picture_with_opacity_zero_shows_nothing_but_one_without_a_stroke_does() {
        let mut e =
            el(ElementKind::Image(crate::ImageData { file_id: None, natural_width: 4, natural_height: 3, crop: None }));
        e.style.stroke_color = Color::TRANSPARENT;
        assert!(!e.shows_nothing());
        e.style.opacity = 0.0;
        assert!(e.shows_nothing());
    }

    #[test]
    fn colours_decide_by_kind() {
        let blue = Some(Color::rgb(0, 0, 255));
        for kind in Kind::ALL {
            assert!(style_shows(kind, false, &style(Color::BLACK, None)), "{kind:?} with a stroke");
        }
        for kind in [Kind::Rectangle, Kind::Diamond, Kind::Ellipse] {
            assert!(style_shows(kind, false, &style(Color::BLACK, None)), "{kind:?} with a stroke");
            assert!(style_shows(kind, false, &style(Color::TRANSPARENT, blue)), "{kind:?}: a fill is enough");
            assert!(!style_shows(kind, false, &style(Color::TRANSPARENT, None)));
            assert!(!style_shows(kind, false, &style(Color::TRANSPARENT, Some(Color::TRANSPARENT))));
        }
        assert!(style_shows(Kind::Line, true, &style(Color::TRANSPARENT, blue)), "closed lines are shapes");
        for kind in [Kind::Line, Kind::Arrow, Kind::Freehand, Kind::Text, Kind::Math, Kind::Frame] {
            assert!(!style_shows(kind, false, &style(Color::TRANSPARENT, blue)), "{kind:?} is drawn in its stroke");
        }
        assert!(style_shows(Kind::Image, false, &style(Color::TRANSPARENT, None)));
    }

    #[test]
    fn a_closed_line_with_a_fill_and_no_stroke_shows_but_an_open_one_does_not() {
        let mut e = el(line());
        e.style = style(Color::TRANSPARENT, Some(Color::rgb(1, 2, 3)));
        assert!(e.shows_nothing(), "an open line has no inside to fill");
        if let ElementKind::Line(d) = &mut e.kind {
            d.closed = true;
        }
        assert!(e.is_closed_outline() && !e.shows_nothing());
    }

    #[test]
    fn text_and_formulas_are_drawn_in_the_stroke_colour() {
        for kind in [ElementKind::Text(TextData::new("hi")), ElementKind::Math(MathData::new("x"))] {
            let mut e = el(kind);
            assert!(!e.shows_nothing());
            e.style.stroke_color = Color::TRANSPARENT;
            assert!(e.shows_nothing());
        }
    }
}
