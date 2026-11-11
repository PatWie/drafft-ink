//! Math layout and typed errors.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_typeset::*;

fn ok(ts: &mut Typesetter, latex: &str) -> std::sync::Arc<MathLayout> {
    ts.math(&MathSpec::new(latex)).unwrap_or_else(|e| panic!("{latex}: {e}"))
}

#[test]
fn simple_formula_has_glyphs_and_sane_bounds() {
    let mut ts = Typesetter::new();
    let m = ok(&mut ts, "E = mc^2");
    assert_eq!(m.glyphs.len(), 5);
    let b = m.bounds();
    assert!(b.x0 == 0.0 && b.x1 > 60.0 && b.x1 < 140.0, "{b:?}");
    assert!(b.y0 < -15.0 && b.y1 < 2.0, "{b:?}");
    assert!((m.baseline() - m.height).abs() < f64::EPSILON);
    assert!((m.size().height - (m.height - m.depth)).abs() < 1e-9 && m.size().height > 15.0);
}

#[test]
fn size_scales_the_layout() {
    let mut ts = Typesetter::new();
    let a = ts.math(&MathSpec::new("\\frac{a}{b}").with_size(20.0)).expect("a");
    let b = ts.math(&MathSpec::new("\\frac{a}{b}").with_size(40.0)).expect("b");
    assert!((b.width / a.width - 2.0).abs() < 0.05);
    assert!((b.size().height / a.size().height - 2.0).abs() < 0.05);
}

#[test]
fn fractions_roots_and_matrices_produce_rules_or_extra_glyphs() {
    let mut ts = Typesetter::new();
    assert!(!ok(&mut ts, "\\frac{a}{b}").rules.is_empty(), "a fraction bar is a rule");
    assert!(ok(&mut ts, "\\sqrt{x^2 + y^2}").glyphs.len() > 4);
    let m = ok(&mut ts, "\\begin{matrix} a & b \\\\ c & d \\end{matrix}");
    assert_eq!(m.glyphs.len(), 4);
    let ys: std::collections::BTreeSet<i64> = m.glyphs.iter().map(|g| (g.y * 100.0) as i64).collect();
    assert_eq!(ys.len(), 2, "two rows");
}

#[test]
fn colour_command_colours_only_its_glyphs() {
    let mut ts = Typesetter::new();
    let m = ok(&mut ts, "a + \\color{red}{b} + c");
    let coloured: Vec<_> = m.glyphs.iter().filter(|g| g.color.is_some()).collect();
    assert_eq!(coloured.len(), 1);
    assert_eq!(m.glyphs.iter().filter(|g| g.color.is_none()).count(), m.glyphs.len() - 1);
    let paths = m.outline(Rgba8::rgb(1, 2, 3));
    assert!(paths.iter().any(|p| p.color == Rgba8::rgb(1, 2, 3)));
    assert!(
        paths.iter().any(|p| p.color.r == 255 && p.color.g == 0),
        "{:?}",
        paths.iter().map(|p| p.color).collect::<Vec<_>>()
    );
}

#[test]
fn text_font_fallback_uses_gelpen_for_letters_and_stix_for_symbols() {
    let mut ts = Typesetter::new();
    let with = ts.math(&MathSpec::new("x + \\sum")).expect("with");
    let mut spec = MathSpec::new("x + \\sum");
    spec.text_font_fallback = false;
    let without = ts.math(&spec).expect("without");
    assert!(with.glyphs.iter().any(|g| g.font == MathFontSlot::Text));
    assert!(with.glyphs.iter().any(|g| g.font == MathFontSlot::Math), "\\sum is not in GelPen");
    assert!(without.glyphs.iter().all(|g| g.font == MathFontSlot::Math));
    // Text-font glyphs are drawn at 0.75 of the math size, like the old app.
    let t = with.glyphs.iter().find(|g| g.font == MathFontSlot::Text).expect("text glyph");
    let m = without.glyphs.iter().find(|g| g.size > 0.0).expect("glyph");
    assert!((t.size / m.size - 0.75).abs() < 1e-9);
    assert_ne!(with.font(MathFontSlot::Text).data.as_ref().len(), with.font(MathFontSlot::Math).data.as_ref().len());
}

#[test]
fn outline_matches_glyph_positions() {
    let mut ts = Typesetter::new();
    let m = ok(&mut ts, "\\frac{1}{2}mv^2");
    let bb = ink_bbox(&m.outline(Rgba8::BLACK)).expect("ink");
    let b = m.bounds();
    assert!(bb.x0 >= b.x0 - 3.0 && bb.x1 <= b.x1 + 3.0, "{bb:?} {b:?}");
    assert!(bb.y0 >= b.y0 - 3.0 && bb.y1 <= b.y1 + 8.0, "{bb:?} {b:?}");
}

#[test]
fn empty_formula_is_an_empty_layout() {
    let m = ok(&mut Typesetter::new(), "");
    assert!(m.glyphs.is_empty() && m.rules.is_empty());
    assert_eq!((m.width, m.height, m.depth), (0.0, 0.0, 0.0));
}

fn err(latex: &str) -> MathError {
    Typesetter::new().math(&MathSpec::new(latex)).expect_err(latex)
}

#[test]
fn unknown_command_points_at_the_command() {
    let e = err("x + \\unknowncommand{y}");
    assert_eq!(e.kind, MathErrorKind::UnknownCommand("unknowncommand".into()));
    assert_eq!(&"x + \\unknowncommand{y}"[e.span.clone()], "\\unknowncommand");
    assert!(e.message.contains("unknowncommand"));
    assert_eq!(e.line_col("x + \\unknowncommand{y}"), (0, 4));
}

#[test]
fn unknown_command_prefix_of_a_known_one_is_found_correctly() {
    // `\fra` is unknown even though `\frac` appears earlier.
    let src = "\\frac{a}{b} + \\fra{c}";
    let e = err(src);
    assert_eq!(&src[e.span.clone()], "\\fra");
}

#[test]
fn unbalanced_braces_point_at_the_brace() {
    let src = "\\frac{a}{b";
    let e = err(src);
    assert_eq!(e.kind, MathErrorKind::Unbalanced);
    assert_eq!(&src[e.span.clone()], "{");
    assert_eq!(e.span.start, 8);
    let src2 = "a + b}";
    let e2 = err(src2);
    assert_eq!(e2.kind, MathErrorKind::Unbalanced);
    assert_eq!(&src2[e2.span.clone()], "}");
}

#[test]
fn single_open_brace_is_an_error_not_a_panic() {
    assert!(Typesetter::new().math(&MathSpec::new("{")).is_err());
}

#[test]
fn missing_arguments_and_scripts() {
    assert_eq!(err("\\frac").kind, MathErrorKind::MissingArgument);
    let src = "x^";
    let e = err(src);
    assert_eq!(e.kind, MathErrorKind::MissingArgument);
    assert_eq!(&src[e.span.clone()], "^");
}

#[test]
fn unknown_environment_colour_and_dimension() {
    let src = "\\begin{nonsense} a \\end{nonsense}";
    let e = err(src);
    assert_eq!(e.kind, MathErrorKind::UnknownEnvironment("nonsense".into()));
    assert_eq!(&src[e.span.clone()], "nonsense");
    let src = "\\color{notacolour}{x}";
    let e = err(src);
    assert!(matches!(e.kind, MathErrorKind::UnknownColor(_)), "{e:?}");
    assert_eq!(&src[e.span.clone()], "notacolour");
}

#[test]
fn error_spans_are_always_valid_char_ranges() {
    for src in ["é \\bad", "日本語 \\x{", "\\frac{é}{", "}{", "^_", "\\", "\\begin{", "😀 \\left( x", "x_{"] {
        let r = Typesetter::new().math(&MathSpec::new(src));
        if let Err(e) = r {
            assert!(e.span.start <= e.span.end && e.span.end <= src.len(), "{src}: {e:?}");
            assert!(src.is_char_boundary(e.span.start) && src.is_char_boundary(e.span.end), "{src}: {e:?}");
            assert!(!e.message.is_empty());
            let _ = e.line_col(src);
        }
    }
}

#[test]
fn legacy_fixture_formulas_all_behave_as_in_the_old_app() {
    let text = std::fs::read_to_string(fixture("legacy/math.json")).expect("fixture");
    let doc: serde_json::Value = serde_json::from_str(&text).expect("json");
    let mut ts = Typesetter::new();
    let mut valid = 0;
    let mut invalid = 0;
    for shape in doc["shapes"].as_object().expect("shapes").values() {
        let m = &shape["Math"];
        let latex = m["latex"].as_str().expect("latex");
        match ts.math(&MathSpec::new(latex).with_size(m["font_size"].as_f64().expect("size"))) {
            Ok(l) => {
                valid += 1;
                assert!(l.width.is_finite() && l.height.is_finite());
            }
            Err(_) => {
                invalid += 1;
                // The old app drew a pink box for these; they are exactly the two invalid fixtures.
                assert!(latex == "\\unknowncommand{x}" || latex == "{", "unexpected failure for {latex}");
            }
        }
    }
    assert_eq!(invalid, 2);
    assert!(valid >= 10);
}

#[test]
fn too_long_and_too_deep_input_is_refused_with_a_typed_error() {
    let long = "x+".repeat(MAX_LATEX_BYTES);
    assert_eq!(err(&long).kind, MathErrorKind::TooLong);
    let deep = format!("{}x{}", "{".repeat(MAX_NESTING + 1), "}".repeat(MAX_NESTING + 1));
    assert_eq!(err(&deep).kind, MathErrorKind::TooComplex);
    let many = "a^b ".repeat(MAX_TOKENS + 1);
    assert_eq!(err(&many).kind, MathErrorKind::TooComplex);
    // At the limit it still works.
    let at = format!("{}x{}", "{".repeat(MAX_NESTING), "}".repeat(MAX_NESTING));
    assert!(Typesetter::new().math(&MathSpec::new(at)).is_ok());
}

#[test]
fn huge_dimensions_are_a_too_large_error() {
    let e = err("\\rule{1000000000pt}{1pt}");
    assert!(matches!(e.kind, MathErrorKind::TooLarge | MathErrorKind::BadDimension(_)), "{e:?}");
}

#[test]
fn math_errors_are_cached_and_do_not_reparse() {
    let mut ts = Typesetter::new();
    let spec = MathSpec::new("\\nope");
    let a = ts.math(&spec).expect_err("error");
    let before = ts.stats();
    let b = ts.math(&spec).expect_err("error");
    assert_eq!(a, b);
    let after = ts.stats();
    assert_eq!(after.math_hits, before.math_hits + 1);
    assert_eq!(after.math_misses, before.math_misses);
}

#[test]
fn a_formula_before_the_math_font_arrives_is_a_loading_error_that_is_not_cached() {
    let mut ts = Typesetter::without_extra_fonts();
    let spec = MathSpec::new("E = mc^2");
    let e = ts.math(&spec).unwrap_err();
    assert_eq!(e.kind, MathErrorKind::FontLoading);
    assert_eq!(ts.wanted_fonts(), [ExtraFont::StixTwoMath]);
    assert_eq!(ts.stats().math_entries, 0, "a loading error is not cached");

    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts").join(ExtraFont::StixTwoMath.file_name()),
    )
    .expect("font file");
    ts.provide_font(ExtraFont::StixTwoMath, bytes).expect("a valid font");
    assert!(ts.wanted_fonts().is_empty());
    let m = ts.math(&spec).expect("the formula typesets once the font is there");
    assert_eq!(m.glyphs.len(), 5);
}

#[test]
fn inline_formulas_are_laid_out_again_when_the_math_font_arrives() {
    let mut ts = Typesetter::without_extra_fonts();
    let mut spec = plain("energy $E = mc^2$ here", 20.0);
    spec.math = InlineMath::Auto;
    let before = ts.text(&spec);
    assert_eq!(before.formula_errors.len(), 1, "{:?}", before.formula_errors);
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts").join(ExtraFont::StixTwoMath.file_name()),
    )
    .expect("font file");
    ts.provide_font(ExtraFont::StixTwoMath, bytes).expect("a valid font");
    let after = ts.text(&spec);
    assert!(after.formula_errors.is_empty() && after.formulas.len() == 1, "{:?}", after.formula_errors);
}
