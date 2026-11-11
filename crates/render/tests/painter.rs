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
    let mut e = line_el("a", &ZIndex::first(), LinearKind::Arrow, &[(0.0, 0.0), (100.0, 40.0)]);
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

/// A stroke with one pressure per point, in the given mode and brush.
fn pressure_stroke(mode: PressureMode, brush: Brush, pressures: bool) -> Element {
    let mut rng = Rng(1);
    let mut e = freehand_el("m", &ZIndex::first(), 0.0, 0.0, 30, &mut rng, pressures);
    if let ElementKind::Freehand(d) = &mut e.kind {
        d.pressure_mode = mode;
        d.brush = brush;
    }
    e
}

/// The encoded geometry of a fragment: what the GPU would fill.
fn geometry(f: &Fragment) -> Vec<u32> {
    f.scene.encoding().path_data.clone()
}

#[test]
fn the_pressure_mode_decides_whether_the_stored_pressures_shape_the_stroke() {
    let uniform = paint(&pressure_stroke(PressureMode::Uniform, Brush::Normal, true));
    let device = paint(&pressure_stroke(PressureMode::Device, Brush::Normal, true));
    let simulated = paint(&pressure_stroke(PressureMode::Simulated, Brush::Normal, true));
    assert_ne!(geometry(&uniform), geometry(&device), "pressure changes the outline");
    assert_eq!(geometry(&simulated), geometry(&device), "a simulated pressure is stored like a pen's");
    // The uniform mode draws the stroke as if it had no pressures.
    let bare = paint(&pressure_stroke(PressureMode::Uniform, Brush::Normal, false));
    assert_eq!(geometry(&uniform), geometry(&bare));
    assert_eq!(colors(&uniform), colors(&bare));
}

#[test]
fn a_pressure_stroke_with_the_wrong_number_of_pressures_is_drawn_uniform() {
    let mut e = pressure_stroke(PressureMode::Device, Brush::Normal, true);
    if let ElementKind::Freehand(d) = &mut e.kind {
        d.pressures.pop();
    }
    let bare = paint(&pressure_stroke(PressureMode::Uniform, Brush::Normal, false));
    assert_eq!(geometry(&paint(&e)), geometry(&bare));
}

#[test]
fn the_brush_is_applied_while_drawing_so_the_painter_draws_the_stored_points() {
    // The calligraphy brush smooths the samples as they arrive (`edit::brush`), as in the old app. The
    // stored points are the result, so the same points are drawn the same way in either brush.
    for pressures in [false, true] {
        let normal = paint(&pressure_stroke(PressureMode::Device, Brush::Normal, pressures));
        let calligraphy = paint(&pressure_stroke(PressureMode::Device, Brush::Calligraphy, pressures));
        assert_eq!(geometry(&normal), geometry(&calligraphy));
    }
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
            TextRun { start: 0, end: 3, color: Some(Color::rgb(255, 0, 0)), ..Default::default() },
            TextRun { start: 4, end: 9, color: Some(Color::rgb(0, 255, 0)), ..Default::default() },
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
fn bold_and_italic_runs_change_the_glyphs_the_width_and_the_cache_key() {
    let mut ts = Typesetter::new();
    let mut e = text_el(&mut ts, "t", &ZIndex::first(), 0.0, 0.0, "plain bold slanted");
    let plain = paint(&e);
    let key = |e: &Element| drafftink_render::paint_hash(e);
    let plain_key = key(&e);
    let widths = |e: &Element, runs: Vec<TextRun>| {
        let mut e = e.clone();
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = runs;
        }
        (paint(&e).bounds.width(), key(&e))
    };
    let (bold_width, bold_key) = widths(&e, vec![TextRun { start: 0, end: 18, bold: true, ..Default::default() }]);
    assert!(
        bold_width > plain.bounds.width() + 1.0,
        "bold letters are wider: {bold_width} against {}",
        plain.bounds.width()
    );
    assert_ne!(bold_key, plain_key, "a bold run is another picture: the cache must not hand out the plain one");
    let (italic_width, italic_key) =
        widths(&e, vec![TextRun { start: 0, end: 18, italic: true, ..Default::default() }]);
    assert_ne!(italic_key, plain_key);
    assert_ne!(italic_key, bold_key);
    assert!(italic_width > 0.0);
    // Only the run's characters change: a bold word in the middle widens the text by less than all of it bold.
    let (some_width, _) = widths(&e, vec![TextRun { start: 6, end: 10, bold: true, ..Default::default() }]);
    assert!(some_width > plain.bounds.width() && some_width < bold_width, "{some_width}");
    // A run with nothing in it is no run at all.
    let (same_width, _) = widths(&e, vec![TextRun { start: 6, end: 10, ..Default::default() }]);
    assert_eq!(same_width, plain.bounds.width());
    if let ElementKind::Text(t) = &mut e.kind {
        t.runs = vec![TextRun { start: 0, end: 5, color: Some(Color::rgb(255, 0, 0)), bold: true, italic: false }];
    }
    let red_bold = paint(&e);
    assert!(colors(&red_bold).contains(&packed(PColor::from_rgba8(255, 0, 0, 255))), "colour and bold together");
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
fn a_label_wraps_at_its_box_like_a_fixed_width_text_and_free_text_does_not() {
    let mut ts = Typesetter::new();
    let long = "a fairly long line that must wrap inside a narrow box";
    let mut label = text_el(&mut ts, "t", &ZIndex::first(), 0.0, 0.0, long);
    if let ElementKind::Text(t) = &mut label.kind {
        // How a label is stored: it follows its container, so `auto_resize` stays on.
        t.container_id = Some(ElementId::new("box").unwrap());
        assert!(t.auto_resize);
    }
    label.transform.w = 90.0;
    let f = paint(&label);
    assert!(f.bounds.width() <= 90.0 + 1.0, "{:?}", f.bounds);
    assert!(f.bounds.height() > 40.0, "wrapped into several lines: {:?}", f.bounds);

    // The same text without a container is free text that sizes itself: one line, however narrow its box.
    let mut free = label.clone();
    if let ElementKind::Text(t) = &mut free.kind {
        t.container_id = None;
    }
    let f = paint(&free);
    assert!(f.bounds.width() > 200.0 && f.bounds.height() < 40.0, "{:?}", f.bounds);
    assert_ne!(paint_hash(&label), paint_hash(&free), "a cached fragment of the free text must not serve the label");

    // A label whose box has no width yet (a file that never measured it) stays on its lines.
    label.transform.w = 0.0;
    assert!(paint(&label).bounds.height() < 40.0);
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

// ---------------------------------------------------------------- inline math

fn text_with_formula(ts: &mut Typesetter, content: &str) -> Element {
    text_el(ts, "t", &ZIndex::first(), 0.0, 0.0, content)
}

fn base_ink() -> u32 {
    // The default stroke colour, indigo.
    packed(PColor::from_rgba8(0x63, 0x66, 0xf1, 255))
}

#[test]
fn a_formula_inside_text_is_drawn_with_the_math_font_in_the_text_colour() {
    let mut ts = Typesetter::new();
    let plain = text_with_formula(&mut ts, "Energy  here");
    let e = text_with_formula(&mut ts, "Energy $\\frac{a}{b}$ here");
    let (p, f) = (paint(&plain), paint(&e));
    // Text on both sides, the formula's glyphs, and its fraction bar.
    assert!(f.scene.encoding().resources.glyph_runs.len() > p.scene.encoding().resources.glyph_runs.len());
    assert!(n_draws(&f, DrawTag::COLOR) >= 3, "{}", n_draws(&f, DrawTag::COLOR));
    // Everything follows the text colour.
    assert!(colors(&f).iter().all(|c| *c == base_ink()), "{:?}", colors(&f));
    // The fragment covers the formula, and the element box, measured the same way, is as large.
    assert!(f.bounds.height() > p.bounds.height() + 5.0, "{:?} {:?}", f.bounds, p.bounds);
    assert!(f.bounds.x1 >= e.transform.w - 1.0, "{:?} {}", f.bounds, e.transform.w);
    assert!(f.bounds.y1 <= e.transform.h + 1.0);
}

#[test]
fn inline_math_off_draws_the_dollar_signs() {
    let mut ts = Typesetter::new();
    let mut e = text_with_formula(&mut ts, "Energy $\\frac{a}{b}$ here");
    let with_math = paint(&e);
    if let ElementKind::Text(t) = &mut e.kind {
        t.font.inline_math = InlineMath::Off;
    }
    measure_text(&mut ts, &mut e);
    let raw = paint(&e);
    // One run of plain text, no fraction bar (a bar is a rectangle fill, plain text has none).
    assert_eq!(raw.scene.encoding().resources.glyph_runs.len(), 1);
    assert!(raw.bounds.width() > with_math.bounds.width());
    assert!(n_draws(&raw, DrawTag::COLOR) < n_draws(&with_math, DrawTag::COLOR));
}

#[test]
fn a_formula_keeps_a_colour_it_sets_and_follows_a_colour_run() {
    let mut ts = Typesetter::new();
    let mut e = text_with_formula(&mut ts, "a {$\\color{red} x$} b $y$");
    // "{$..$}" is text around a formula; colour the second formula green with a run.
    let n = if let ElementKind::Text(t) = &e.kind { t.content.chars().count() } else { 0 };
    if let ElementKind::Text(t) = &mut e.kind {
        t.runs = vec![TextRun {
            start: (n - 3) as u32,
            end: n as u32,
            color: Some(Color::rgb(0, 160, 0)),
            ..Default::default()
        }];
    }
    let c = colors(&paint(&e));
    assert!(c.contains(&packed(PColor::from_rgba8(0, 160, 0, 255))), "the run colours the formula: {c:?}");
    assert!(c.contains(&base_ink()), "text outside the run keeps the text colour: {c:?}");
    assert!(c.iter().any(|w| *w != base_ink() && *w != packed(PColor::from_rgba8(0, 160, 0, 255))), "\\color: {c:?}");
}

#[test]
fn a_broken_formula_is_its_source_in_red_and_does_not_fail() {
    let mut ts = Typesetter::new();
    let e = text_with_formula(&mut ts, "ok $\\notacommand{$ ok");
    let f = paint(&e);
    let red = packed(PColor::from_rgba8(0xe0, 0x31, 0x31, 255));
    assert!(colors(&f).contains(&red), "{:?}", colors(&f));
    assert!(colors(&f).contains(&base_ink()));
    assert!(!f.is_empty());
}

#[test]
fn inline_math_follows_the_theme_and_the_opacity() {
    let mut ts = Typesetter::new();
    let mut e = text_with_formula(&mut ts, "a $\\frac{x}{y}$ b");
    e.style.stroke_color = Color::BLACK;
    let dark = paint_with(&e, RenderOptions { theme: drafftink_render::Theme::Dark, ..RenderOptions::default() });
    let c = colors(&dark);
    assert!(!c.is_empty() && c.iter().all(|w| *w == c[0]), "text, glyphs and bar share one colour: {c:?}");
    assert_ne!(c[0], packed(PColor::from_rgba8(0, 0, 0, 255)), "black became light ink");
    e.style.opacity = 0.5;
    let faded = colors(&paint(&e));
    assert!(faded.iter().all(|w| *w == faded[0]) && faded[0] != packed(PColor::from_rgba8(0, 0, 0, 255)));
    let alpha = |w: u32| w >> 24;
    assert!((100..=140).contains(&alpha(faded[0])), "{:#x}", faded[0]);
}

#[test]
fn rotation_and_position_do_not_change_the_fragment() {
    let mut ts = Typesetter::new();
    let mut e = text_with_formula(&mut ts, "a $x^2$ b");
    let first = paint(&e);
    e.transform.x = 300.0;
    e.transform.angle = 0.7;
    let moved = paint(&e);
    assert_eq!(first.bounds, moved.bounds);
    assert_eq!(first.bytes, moved.bytes);
}

#[test]
fn a_frame_fragment_is_the_box_only_and_its_name_is_left_to_the_composer() {
    let mut f = frame_el("f", &ZIndex::first(), 0.0, 0.0, 300.0, 200.0);
    let default_name = paint(&f);
    assert_eq!(default_name.scene.encoding().resources.glyph_runs.len(), 0, "the label is drawn by the composer");
    assert_eq!(n_draws(&default_name, DrawTag::BEGIN_CLIP), 0);
    assert!(default_name.bounds.y0 >= -2.0, "nothing above the box: {:?}", default_name.bounds);

    f.kind = ElementKind::Frame(FrameData { name: Some("Wireframes".into()), clip: true });
    f.style.fill_color = Some(Color::rgb(240, 240, 255));
    let named = paint(&f);
    assert!(colors(&named).len() >= 2, "fill and border");
    assert_eq!(paint_hash(&f), {
        f.kind = ElementKind::Frame(FrameData { name: Some("Another".into()), clip: true });
        paint_hash(&f)
    });
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

#[test]
fn no_stroke_draws_the_fill_alone() {
    let green = packed(PColor::from_rgba8(0, 120, 0, 255));
    for sloppy in [Sloppiness::Architect, Sloppiness::Artist] {
        let mut e = rect(sloppy, Some(Color::rgb(0, 120, 0)), FillPattern::Solid);
        e.style.stroke_color = Color::TRANSPARENT;
        assert_eq!(colors(&paint(&e)), [green], "{sloppy:?}");
    }
    // Without a stroke the outline adds nothing to the fragment's bounds.
    let mut e = rect(Sloppiness::Architect, Some(Color::rgb(0, 120, 0)), FillPattern::Solid);
    e.style.stroke_width = 20.0;
    let with = paint(&e).bounds;
    e.style.stroke_color = Color::TRANSPARENT;
    let without = paint(&e).bounds;
    assert_ne!(with, without);
}

#[test]
fn no_stroke_skips_arrowheads_and_the_ink_of_an_open_stroke() {
    let mut a = arrow(Head::Triangle);
    a.style.stroke_color = Color::TRANSPARENT;
    assert!(colors(&paint(&a)).is_empty());
    let mut p = pressure_stroke(PressureMode::Device, Brush::Normal, true);
    p.style.stroke_color = Color::TRANSPARENT;
    assert!(colors(&paint(&p)).is_empty());
}
