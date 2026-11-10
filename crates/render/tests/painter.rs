#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! What the painter draws for each kind, read back from the encoded scene (no GPU).

mod common;

use common::*;
use drafftink_render::*;
use drafftink_scene::*;
use drafftink_typeset::Typesetter;
use peniko::Color as PColor;
use vello::low_level::vello_encoding::{DrawColor, DrawTag};

fn paint(e: &Element) -> Fragment {
    paint_with(e, RenderOptions::default())
}

fn paint_with(e: &Element, options: RenderOptions) -> Fragment {
    let mut ts = Typesetter::new();
    ElementPainter::new().paint(e, &options, &mut ts, &|_| None)
}

/// The packed colour (premultiplied RGBA8) of a solid draw, as Vello stores it.
fn packed(c: PColor) -> u32 {
    DrawColor::from(c).rgba
}

/// Solid colours of the draws, in order, as packed words.
fn colors(f: &Fragment) -> Vec<u32> {
    let enc = f.scene.encoding();
    let mut out = Vec::new();
    let mut at = 0usize;
    for tag in &enc.draw_tags {
        let words = ((tag.0 >> 2) & 7) as usize;
        if *tag == DrawTag::COLOR {
            out.push(enc.draw_data[at]);
        }
        at += words;
    }
    out
}

fn n_draws(f: &Fragment, tag: DrawTag) -> usize {
    f.scene.encoding().draw_tags.iter().filter(|t| **t == tag).count()
}

fn rect(sloppy: Sloppiness, fill: Option<Color>, pattern: FillPattern) -> Element {
    let mut e = rect_el("r", &ZIndex::first(), 0.0, 0.0, 120.0, 80.0);
    e.style.stroke_color = Color::rgb(200, 30, 30);
    e.style.fill_color = fill;
    e.style.fill_pattern = pattern;
    e.style.sloppiness = sloppy;
    e.style.seed = 5;
    e
}

#[test]
fn architect_rectangle_is_a_fill_then_one_stroke() {
    let e = rect(Sloppiness::Architect, Some(Color::rgb(0, 120, 0)), FillPattern::Solid);
    let f = paint(&e);
    assert_eq!(colors(&f), [packed(PColor::from_rgba8(0, 120, 0, 255)), packed(PColor::from_rgba8(200, 30, 30, 255))]);
}

#[test]
fn sloppy_shapes_get_the_double_stroke() {
    let e = rect(Sloppiness::Artist, Some(Color::rgb(0, 120, 0)), FillPattern::Solid);
    let c = colors(&paint(&e));
    let stroke = packed(PColor::from_rgba8(200, 30, 30, 255));
    assert_eq!(c.len(), 3);
    assert_eq!(&c[1..], &[stroke, stroke]);
    // No fill: just the two strokes.
    assert_eq!(colors(&paint(&rect(Sloppiness::Artist, None, FillPattern::Solid))), [stroke, stroke]);
}

#[test]
fn a_pattern_is_a_tint_then_hatch_lines_clipped_to_the_fill() {
    let e = rect(Sloppiness::Architect, Some(Color::rgb(0, 100, 200)), FillPattern::Hachure);
    let f = paint(&e);
    assert_eq!(n_draws(&f, DrawTag::BEGIN_CLIP), 1);
    assert_eq!(n_draws(&f, DrawTag::END_CLIP), 1);
    let c = colors(&f);
    // The old tint: alpha 255 * 0.15 truncated to 38.
    assert_eq!(c[0], packed(PColor::from_rgba8(0, 100, 200, 38)));
    assert_eq!(c[1], packed(PColor::from_rgba8(0, 100, 200, 255)), "hatch lines use the fill colour");
    assert_eq!(*c.last().unwrap(), packed(PColor::from_rgba8(200, 30, 30, 255)));
}

#[test]
fn opacity_multiplies_every_paint_like_the_old_app() {
    let mut e = rect(Sloppiness::Architect, Some(Color::rgb(0, 120, 0)), FillPattern::Solid);
    e.style.opacity = 0.5;
    let c = colors(&paint(&e));
    // alpha = (255 * 0.5) as u8 = 127.
    assert_eq!(c, [packed(PColor::from_rgba8(0, 120, 0, 127)), packed(PColor::from_rgba8(200, 30, 30, 127))]);
}

#[test]
fn the_dark_theme_maps_colours_while_encoding() {
    let e = rect(Sloppiness::Architect, None, FillPattern::Solid);
    let light = colors(&paint(&e));
    let dark = colors(&paint_with(&e, RenderOptions { theme: Theme::Dark, ..RenderOptions::default() }));
    assert_ne!(light, dark);
    let want = Theme::Dark.map(Color::rgb(200, 30, 30));
    assert_eq!(dark, [packed(PColor::from_rgba8(want.r, want.g, want.b, 255))]);
}

fn arrow(head: Head) -> Element {
    let mut e = line_el("a", &ZIndex::first(), Kind::Arrow, &[(0.0, 0.0), (100.0, 40.0)]);
    if let ElementKind::Arrow(d) = &mut e.kind {
        d.end_head = head;
    }
    e.style.sloppiness = Sloppiness::Architect;
    e
}

#[test]
fn arrow_heads_are_drawn_and_filled_ones_are_inked() {
    let none = paint(&arrow(Head::None));
    let open = paint(&arrow(Head::Arrow));
    let filled = paint(&arrow(Head::Triangle));
    let hollow = paint(&arrow(Head::TriangleOutline));
    assert_eq!(colors(&none).len(), 1);
    assert_eq!(colors(&open).len(), 1, "the open head is part of the stroke");
    assert!(open.scene.encoding().path_data.len() > none.scene.encoding().path_data.len());
    assert_eq!(colors(&filled).len(), 2, "a filled head adds an ink fill");
    assert_eq!(colors(&hollow).len(), 1);
}

#[test]
fn cardinality_markers_add_their_own_strokes() {
    let base = paint(&arrow(Head::None)).scene.encoding().path_data.len();
    for h in [
        Head::CardinalityOne,
        Head::CardinalityMany,
        Head::CardinalityOneOrMany,
        Head::CardinalityExactlyOne,
        Head::CardinalityZeroOrOne,
        Head::CardinalityZeroOrMany,
    ] {
        let f = paint(&arrow(h));
        assert!(f.scene.encoding().path_data.len() > base, "{h:?}");
        assert!(f.bounds.x1 > 99.0, "{h:?} is drawn at the tip");
    }
}

#[test]
fn a_highlighter_is_one_layer_with_the_alpha_on_the_layer() {
    let mut rng = Rng(1);
    let mut e = freehand_el("h", &ZIndex::first(), 0.0, 0.0, 20, &mut rng, false);
    if let ElementKind::Freehand(d) = &mut e.kind {
        d.highlighter = true;
    }
    e.style.stroke_width = 14.0;
    e.style.stroke_color = Color::rgba(255, 230, 0, 128);
    let f = paint(&e);
    assert_eq!(n_draws(&f, DrawTag::BEGIN_CLIP), 1);
    // The stroke itself is opaque; a self-crossing highlighter does not darken.
    assert_eq!(colors(&f), [packed(PColor::from_rgba8(255, 230, 0, 255))]);
    assert!(f.bounds.height() >= 14.0);
}

#[test]
fn pressure_strokes_are_one_ink_fill() {
    let mut rng = Rng(1);
    let e = freehand_el("p", &ZIndex::first(), 0.0, 0.0, 30, &mut rng, true);
    let f = paint(&e);
    let ink = packed(PColor::from_rgba8(0x63, 0x66, 0xf1, 255));
    assert_eq!(colors(&f), [ink]);
}

#[test]
fn text_is_glyph_runs_one_per_colour_and_nothing_when_empty() {
    let mut ts = Typesetter::new();
    let mut e = text_el(&mut ts, "t", &ZIndex::first(), 0.0, 0.0, "red green blue");
    let plain = paint(&e);
    assert_eq!(plain.scene.encoding().resources.glyph_runs.len(), 1);
    assert_eq!(plain.bounds.min_x(), 0.0);
    assert!(plain.bounds.width() > 20.0);
    if let ElementKind::Text(t) = &mut e.kind {
        t.runs = vec![
            TextRun { start: 0, end: 3, color: Some(Color::rgb(255, 0, 0)) },
            TextRun { start: 4, end: 9, color: Some(Color::rgb(0, 255, 0)) },
        ];
    }
    let coloured = paint(&e);
    assert!(
        coloured.scene.encoding().resources.glyph_runs.len() >= 3,
        "{}",
        coloured.scene.encoding().resources.glyph_runs.len()
    );
    let c = colors(&coloured);
    assert!(c.contains(&packed(PColor::from_rgba8(255, 0, 0, 255))));
    assert!(c.contains(&packed(PColor::from_rgba8(0, 255, 0, 255))));

    if let ElementKind::Text(t) = &mut e.kind {
        t.content.clear();
        t.runs.clear();
    }
    assert!(paint(&e).is_empty());
}

#[test]
fn text_fills_a_fixed_width_box_when_it_wraps() {
    let mut ts = Typesetter::new();
    let mut e =
        text_el(&mut ts, "t", &ZIndex::first(), 0.0, 0.0, "a fairly long line that must wrap inside a narrow box");
    if let ElementKind::Text(t) = &mut e.kind {
        t.auto_resize = false;
    }
    e.transform.w = 90.0;
    let f = paint(&e);
    assert!(f.bounds.width() <= 90.0 + 1.0, "{:?}", f.bounds);
    assert!(f.bounds.height() > 40.0, "wrapped into several lines: {:?}", f.bounds);
}

#[test]
fn math_draws_glyphs_and_rules_and_a_bad_formula_draws_the_old_error_box() {
    let mut ts = Typesetter::new();
    let ok = math_el(&mut ts, "m", &ZIndex::first(), 0.0, 0.0, "\\frac{a}{b}+x^2");
    let f = paint(&ok);
    assert!(!f.scene.encoding().resources.glyph_runs.is_empty());
    assert!(n_draws(&f, DrawTag::COLOR) >= 2, "glyph runs and the fraction bar");
    assert!(f.bounds.width() > 10.0);
    // The formula follows the shape colour.
    assert!(colors(&f).contains(&packed(PColor::from_rgba8(0x63, 0x66, 0xf1, 255))));

    let bad = math_el(&mut ts, "m2", &ZIndex::first(), 5.0, 5.0, "\\notacommand{");
    let f = paint(&bad);
    assert_eq!(f.scene.encoding().resources.glyph_runs.len(), 0);
    assert_eq!(colors(&f).len(), 2, "pale red fill and red border: {:?}", colors(&f));
}

#[test]
fn math_with_its_own_colour_keeps_it() {
    let mut ts = Typesetter::new();
    let e = math_el(&mut ts, "m", &ZIndex::first(), 0.0, 0.0, "{\\color{red} x} + y");
    let c = colors(&paint(&e));
    let base = packed(PColor::from_rgba8(0x63, 0x66, 0xf1, 255));
    assert!(c.contains(&base), "y uses the shape colour");
    assert!(c.iter().any(|w| *w != base), "x keeps its own colour: {c:?}");
}

#[test]
fn frames_draw_a_border_and_a_clipped_name() {
    let mut f = frame_el("f", &ZIndex::first(), 0.0, 0.0, 300.0, 200.0);
    let default_name = paint(&f);
    assert_eq!(default_name.scene.encoding().resources.glyph_runs.len(), 1, "the default label is Frame");
    assert_eq!(n_draws(&default_name, DrawTag::BEGIN_CLIP), 1);
    assert!(default_name.bounds.y0 < 0.0, "the label sits above the box");
    assert!(default_name.bounds.y0 >= -FRAME_LABEL_ROOM - 1.0);

    f.kind = ElementKind::Frame(FrameData { name: Some(String::new()), clip: true });
    let unnamed = paint(&f);
    assert_eq!(unnamed.scene.encoding().resources.glyph_runs.len(), 0);

    f.kind = ElementKind::Frame(FrameData { name: Some("Wireframes".into()), clip: true });
    f.style.fill_color = Some(Color::rgb(240, 240, 255));
    let named = paint(&f);
    assert!(colors(&named).len() >= 3, "fill, border, label");
}

#[test]
fn dashes_change_the_stroke_style() {
    let mut e = rect(Sloppiness::Architect, None, FillPattern::Solid);
    let solid = paint(&e);
    e.style.stroke_style = StrokeStyle::Dashed;
    let dashed = paint(&e);
    assert!(
        dashed.scene.encoding().path_data.len() > solid.scene.encoding().path_data.len(),
        "dashing splits the path"
    );
}

#[test]
fn fragments_are_in_the_local_frame() {
    // Where the element sits does not matter to its fragment.
    let a = paint(&{
        let mut e = rect(Sloppiness::Artist, None, FillPattern::Solid);
        e.transform.x = 1000.0;
        e.transform.angle = 0.5;
        e
    });
    let b = paint(&rect(Sloppiness::Artist, None, FillPattern::Solid));
    assert_eq!(a.scene.encoding().path_data, b.scene.encoding().path_data);
    assert_eq!(a.bounds, b.bounds);
}

#[test]
fn bytes_are_estimated_and_grow_with_content() {
    let small = paint(&rect(Sloppiness::Architect, None, FillPattern::Solid));
    let big = paint(&rect(Sloppiness::Architect, Some(Color::rgb(1, 2, 3)), FillPattern::CrossHatch));
    assert!(small.bytes > 0 && big.bytes > small.bytes * 3, "{} vs {}", small.bytes, big.bytes);
}
