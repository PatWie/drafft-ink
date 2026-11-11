#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The adapter from the typesetter to the measure traits of `io`.

use std::cell::RefCell;

use drafftink_io::svg::MathSvg;
use drafftink_io::{TextMeasure, TypesetterMeasure};
use drafftink_scene::{FontFamily, FontWeight};
use drafftink_typeset::Typesetter;

#[test]
fn text_has_a_size_that_grows_with_its_content_and_never_goes_negative() {
    let mut ts = Typesetter::new();
    let m = TypesetterMeasure(RefCell::new(&mut ts));
    let (w1, h1) = m.text_size("ab", FontFamily::GelPen, FontWeight::Regular, 20.0, 1.25);
    let (w2, h2) = m.text_size("ab\nabcd", FontFamily::GelPen, FontWeight::Regular, 20.0, 1.25);
    assert!(w1 > 0.0 && h1 > 0.0);
    assert!(w2 > w1 && h2 > h1, "{w1}x{h1} then {w2}x{h2}");
    let (w, h) = m.text_size("x", FontFamily::GelPen, FontWeight::Regular, f64::NAN, f64::NAN);
    assert!(w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0);
}

#[test]
fn a_formula_has_a_box_and_a_svg_and_a_broken_one_has_neither() {
    let mut ts = Typesetter::new();
    let m = TypesetterMeasure(RefCell::new(&mut ts));
    let b = m.math_size("x^2", 20.0).expect("x^2 lays out");
    assert!(b.width > 0.0 && b.height > 0.0 && b.depth >= 0.0);
    let svg = m.math_svg("x^2", 20.0).expect("x^2 has an outline");
    assert!(svg.contains("<path") && svg.contains("currentColor"), "{svg}");
    assert!(m.math_size("\\notacommand{", 20.0).is_none());
    assert!(m.math_svg("\\notacommand{", 20.0).is_none());
}

#[test]
fn plex_sans_text_is_measured_with_the_plex_face_and_its_real_weights() {
    let mut ts = Typesetter::new();
    let m = TypesetterMeasure(RefCell::new(&mut ts));
    let text = "Handgloves and hamburgefonts";
    let size = |family, weight| m.text_size(text, family, weight, 24.0, 1.25).0;
    let (plex, gelpen) =
        (size(FontFamily::PlexSans, FontWeight::Regular), size(FontFamily::GelPen, FontWeight::Regular));
    assert!((plex - gelpen).abs() > 1.0, "Plex {plex} is not GelPen {gelpen}");
    assert!(size(FontFamily::PlexSans, FontWeight::Heavy) > plex);
    assert!(size(FontFamily::PlexSans, FontWeight::Light) < plex);
}

#[test]
fn the_bundled_svg_fonts_have_one_plex_face() {
    let faces = drafftink_io::typeset_measure::bundled_svg_fonts();
    let plex: Vec<_> = faces.iter().filter(|f| f.family == FontFamily::PlexSans).collect();
    assert_eq!(plex.len(), 1);
    assert!(plex[0].data.len() > 100_000, "the whole variable file is there");
}
