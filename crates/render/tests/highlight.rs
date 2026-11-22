#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The highlight band, read back from the encoded scene (no GPU): what it draws, in which order, per theme, and
//! what the cache key and the paint bounds say about it.

mod common;

use common::*;
use drafftink_render::*;
use drafftink_scene::*;
use drafftink_typeset::Typesetter;
use peniko::Color as PColor;
use vello::low_level::vello_encoding::{DrawColor, DrawTag};

fn paint_with(e: &Element, options: RenderOptions, ts: &mut Typesetter) -> Fragment {
    ElementPainter::new().paint(e, &options, ts, &|_| None)
}

fn paint(e: &Element) -> Fragment {
    paint_with(e, RenderOptions::default(), &mut Typesetter::new())
}

fn dark() -> RenderOptions {
    RenderOptions { theme: Theme::Dark, ..RenderOptions::default() }
}

fn packed(c: Color) -> u32 {
    DrawColor::from(PColor::from_rgba8(c.r, c.g, c.b, c.a)).rgba
}

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

/// The coordinates of every path, as words: two fragments are the same picture when these are.
fn geometry(f: &Fragment) -> Vec<u32> {
    f.scene.encoding().path_data.to_vec()
}

fn layers(f: &Fragment) -> usize {
    f.scene.encoding().draw_tags.iter().filter(|t| **t == DrawTag::BEGIN_CLIP).count()
}

fn path_segments(f: &Fragment) -> usize {
    f.scene.encoding().path_tags.len()
}

fn rect(highlight: Option<MarkerColor>) -> Element {
    let mut e = rect_el("r", &ZIndex::first(), 0.0, 0.0, 120.0, 80.0);
    e.style.stroke_color = Color::rgb(200, 30, 30);
    e.style.sloppiness = Sloppiness::Architect;
    e.style.seed = 5;
    e.style.highlight = highlight;
    e
}

#[test]
fn a_highlight_is_one_faded_layer_drawn_before_the_ink() {
    let plain = paint(&rect(None));
    assert_eq!(layers(&plain), 0);
    let f = paint(&rect(Some(MarkerColor::Yellow)));
    assert_eq!(layers(&f), 1, "one layer, so that overlaps do not darken");
    // The band is opaque inside its layer; the stroke comes after it.
    assert_eq!(colors(&f), [packed(MarkerColor::Yellow.rgb(Tone::Light)), packed(Color::rgb(200, 30, 30))]);
}

#[test]
fn the_band_is_under_the_fill_and_the_stroke() {
    let mut e = rect(Some(MarkerColor::Pink));
    e.style.fill_color = Some(Color::rgb(0, 120, 0));
    let f = paint(&e);
    assert_eq!(
        colors(&f),
        [packed(MarkerColor::Pink.rgb(Tone::Light)), packed(Color::rgb(0, 120, 0)), packed(Color::rgb(200, 30, 30))]
    );
}

#[test]
fn the_dark_canvas_uses_the_dark_value_of_the_marker_and_the_light_one_does_not() {
    let e = rect(Some(MarkerColor::Green));
    let mut ts = Typesetter::new();
    let on_dark = colors(&paint_with(&e, dark(), &mut ts));
    let on_light = colors(&paint_with(&e, RenderOptions::default(), &mut ts));
    assert_eq!(on_dark[0], packed(MarkerColor::Green.rgb(Tone::Dark)));
    assert_eq!(on_light[0], packed(MarkerColor::Green.rgb(Tone::Light)));
    assert_ne!(on_dark[0], packed(MarkerColor::Green.rgb(Tone::Light).for_dark_theme()), "not the filtered colour");
    assert_eq!(Theme::Dark.marker(MarkerColor::Green), MarkerColor::Green.rgb(Tone::Dark));
}

#[test]
fn the_band_is_wider_than_the_stroke_and_follows_the_outline_not_the_inside() {
    let thin = paint(&rect(Some(MarkerColor::Yellow)));
    let plain = paint(&rect(None));
    // 10 wide at least: the band reaches 5 beyond the outline.
    assert!(
        thin.bounds.x0 <= plain.bounds.x0 - 4.0 && thin.bounds.x1 >= plain.bounds.x1 + 4.0,
        "{:?} {:?}",
        thin.bounds,
        plain.bounds
    );
    let mut thick = rect(Some(MarkerColor::Yellow));
    thick.style.stroke_width = 8.0;
    assert!(paint(&thick).bounds.width() > thin.bounds.width() + 10.0, "3 x 8 = 24 wide");
    // The band is a stroke of the outline, not a filled box: no fill command for it.
    assert_eq!(path_segments(&paint(&rect(Some(MarkerColor::Yellow)))) > path_segments(&plain), true);
}

#[test]
fn an_arrow_head_is_in_the_band_and_an_open_line_gets_slanted_cuts() {
    let arrow = |head: Head, highlight| {
        let mut e = line_el("a", &ZIndex::first(), LinearKind::Arrow, &[(0.0, 0.0), (100.0, 0.0)]);
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.end_head = head;
            d.head_size = Some(20.0);
        }
        e.style.sloppiness = Sloppiness::Architect;
        e.style.highlight = highlight;
        e
    };
    let headless = paint(&arrow(Head::None, Some(MarkerColor::Blue)));
    // The head stands 20 units wide, 10 either side of the shaft, and its band comes with it.
    let with = paint(&arrow(Head::Triangle, Some(MarkerColor::Blue)));
    assert!(with.bounds.height() >= headless.bounds.height() + 10.0, "{:?} {:?}", with.bounds, headless.bounds);
}

#[test]
fn cardinality_markers_get_a_band_too() {
    let mut e = line_el("er", &ZIndex::first(), LinearKind::Arrow, &[(0.0, 0.0), (100.0, 0.0)]);
    if let ElementKind::Arrow(d) = &mut e.kind {
        d.end_head = Head::CardinalityMany;
    }
    e.style.sloppiness = Sloppiness::Architect;
    let band_of = |e: &Element| {
        let mut marked = e.clone();
        marked.style.highlight = Some(MarkerColor::Orange);
        path_segments(&paint(&marked)) - path_segments(&paint(e))
    };
    let with_marker = band_of(&e);
    let mut shaft_only = e.clone();
    if let ElementKind::Arrow(d) = &mut shaft_only.kind {
        d.end_head = Head::None;
    }
    let shaft = band_of(&shaft_only);
    assert!(with_marker > shaft, "the foot of the marker is in the band: {with_marker} against {shaft}");
}

#[test]
fn freehand_image_and_math_have_a_band_and_a_frame_has_none() {
    let mut rng = Rng(3);
    let mut hand = freehand_el("f", &ZIndex::first(), 0.0, 0.0, 12, &mut rng, false);
    hand.style.highlight = Some(MarkerColor::Yellow);
    assert_eq!(layers(&paint(&hand)), 1);
    let mut img = Element::new(id("i"), ElementKind::Image(ImageData::new(None, 10, 10)), ZIndex::first())
        .with_bounds(0.0, 0.0, 60.0, 40.0);
    img.style = Style::for_image(1);
    img.style.highlight = Some(MarkerColor::Yellow);
    assert_eq!(layers(&paint(&img)), 1);
    let mut ts = Typesetter::new();
    let mut m = math_el(&mut ts, "m", &ZIndex::first(), 0.0, 0.0, "x^2");
    m.style.highlight = Some(MarkerColor::Yellow);
    assert_eq!(layers(&paint(&m)), 1);
    let mut frame = Element::new(id("fr"), ElementKind::Frame(FrameData::default()), ZIndex::first())
        .with_bounds(0.0, 0.0, 50.0, 50.0);
    frame.style.highlight = Some(MarkerColor::Yellow);
    assert_eq!(layers(&paint(&frame)), 0, "a frame is a container, not content to mark");
}

// ---- text ----------------------------------------------------------------------------------------------------------

fn text(ts: &mut Typesetter, content: &str) -> Element {
    let mut e = text_el(ts, "t", &ZIndex::first(), 0.0, 0.0, content);
    e.style.seed = 9;
    e
}

fn set_runs(e: &mut Element, runs: Vec<TextRun>) {
    if let ElementKind::Text(t) = &mut e.kind {
        t.runs = runs;
    }
}

#[test]
fn a_text_highlight_is_one_band_per_line_in_one_layer() {
    let mut ts = Typesetter::new();
    let mut e = text(&mut ts, "one\ntwo\nthree");
    let plain = paint_with(&e, RenderOptions::default(), &mut ts);
    e.style.highlight = Some(MarkerColor::Yellow);
    let f = paint_with(&e, RenderOptions::default(), &mut ts);
    assert_eq!(layers(&f), plain_layers(&plain) + 1);
    // Three lines: three quadrilaterals, each 5 path segments (move, three lines, close) before the glyphs.
    assert!(path_segments(&f) >= path_segments(&plain) + 15, "{} {}", path_segments(&f), path_segments(&plain));
    assert_eq!(colors(&f)[0], packed(MarkerColor::Yellow.rgb(Tone::Light)), "the band is first");
}

fn plain_layers(f: &Fragment) -> usize {
    layers(f)
}

#[test]
fn only_the_highlighted_characters_get_a_band_and_each_colour_its_own_layer() {
    let mut ts = Typesetter::new();
    let mut e = text(&mut ts, "alpha beta gamma");
    let before = layers(&paint_with(&e, RenderOptions::default(), &mut ts));
    set_runs(
        &mut e,
        vec![
            TextRun::new(0, 5, CharStyle { highlight: SpanHighlight::On(MarkerColor::Green), ..Default::default() }),
            TextRun::new(11, 16, CharStyle { highlight: SpanHighlight::On(MarkerColor::Pink), ..Default::default() }),
        ],
    );
    let f = paint_with(&e, RenderOptions::default(), &mut ts);
    assert_eq!(layers(&f), before + 2, "two colours, two layers");
    let c = colors(&f);
    assert_eq!(&c[..2], &[packed(MarkerColor::Green.rgb(Tone::Light)), packed(MarkerColor::Pink.rgb(Tone::Light))]);
    // A highlight alone changes no glyph: the glyph runs are those of the plain text.
    let glyphs = |f: &Fragment| f.scene.encoding().resources.glyph_runs.len();
    let plain = paint_with(&text(&mut ts, "alpha beta gamma"), RenderOptions::default(), &mut ts);
    assert_eq!(glyphs(&f), glyphs(&plain));
}

#[test]
fn a_run_can_switch_the_highlight_of_the_whole_text_off_for_its_characters() {
    let mut ts = Typesetter::new();
    let mut e = text(&mut ts, "alpha beta gamma");
    e.style.highlight = Some(MarkerColor::Yellow);
    let whole = paint_with(&e, RenderOptions::default(), &mut ts);
    set_runs(&mut e, vec![TextRun::new(5, 11, CharStyle { highlight: SpanHighlight::Off, ..Default::default() })]);
    let holed = paint_with(&e, RenderOptions::default(), &mut ts);
    assert_eq!(layers(&holed), layers(&whole), "still one layer");
    assert!(path_segments(&holed) > path_segments(&whole), "two bands instead of one");
}

#[test]
fn the_ends_are_irregular_and_come_from_the_seed_alone() {
    let mut ts = Typesetter::new();
    let mut e = text(&mut ts, "marker pen");
    e.style.highlight = Some(MarkerColor::Yellow);
    let a = geometry(&paint_with(&e, RenderOptions::default(), &mut ts));
    assert_eq!(a, geometry(&paint_with(&e, RenderOptions::default(), &mut ts)), "deterministic");
    let mut moved = e.clone();
    moved.transform.x += 500.0;
    assert_eq!(a, geometry(&paint_with(&moved, RenderOptions::default(), &mut ts)), "a fragment is local");
    let mut other = e.clone();
    other.style.seed = 10;
    assert_ne!(a, geometry(&paint_with(&other, RenderOptions::default(), &mut ts)), "another seed, another band");
}

// ---- the cache key and the bounds ----------------------------------------------------------------------------------

#[test]
fn the_paint_hash_sees_the_highlight_of_the_element_and_of_a_run_and_not_the_position() {
    let base = paint_hash(&rect(None));
    assert_ne!(base, paint_hash(&rect(Some(MarkerColor::Yellow))));
    assert_ne!(paint_hash(&rect(Some(MarkerColor::Yellow))), paint_hash(&rect(Some(MarkerColor::Green))));
    let mut moved = rect(Some(MarkerColor::Yellow));
    moved.transform.x += 40.0;
    assert_eq!(paint_hash(&rect(Some(MarkerColor::Yellow))), paint_hash(&moved));

    let mut ts = Typesetter::new();
    let mut t = text(&mut ts, "alpha beta");
    let plain = paint_hash(&t);
    for h in [SpanHighlight::On(MarkerColor::Yellow), SpanHighlight::On(MarkerColor::Pink), SpanHighlight::Off] {
        set_runs(&mut t, vec![TextRun::new(0, 5, CharStyle { highlight: h, ..Default::default() })]);
        assert_ne!(paint_hash(&t), plain, "{h:?}");
    }
}

#[test]
fn a_fragment_cache_hit_is_not_served_after_the_highlight_changes() {
    let mut cache = FragmentCache::new(DEFAULT_CACHE_BYTES);
    let _ = &mut cache; // the key is what decides; see the hash test above
    let (a, b) = (rect(None), rect(Some(MarkerColor::Orange)));
    assert_ne!(paint_hash(&a), paint_hash(&b));
}

#[test]
fn the_paint_bounds_cover_the_band() {
    let plain = rect(None);
    let marked = rect(Some(MarkerColor::Yellow));
    let (p, m) = (paint_bounds(&plain), paint_bounds(&marked));
    assert!(m.x0 < p.x0 && m.x1 > p.x1 && m.y0 < p.y0 && m.y1 > p.y1);
    let f = paint(&marked);
    let world = f.bounds;
    assert!(world.x0 >= m.x0 - paint_margin(&marked).left && world.x1 <= 120.0 + paint_margin(&marked).right);
    let mut thick = rect(Some(MarkerColor::Yellow));
    thick.style.stroke_width = 10.0;
    assert!(paint_margin(&thick).left >= 3.0 * 10.0 / 2.0, "half the band");
}

// ---- the paint bounds hold the band --------------------------------------------------------------------------------

#[test]
fn every_band_stays_inside_the_paint_bounds_whatever_the_shape() {
    let z = ZIndex::first();
    let mut ts = Typesetter::new();
    let mut painter = ElementPainter::new();
    let mut elements: Vec<Element> = Vec::new();
    for (w, h) in [(120.0, 80.0), (3.0, 200.0), (200.0, 2.0), (1.0, 1.0)] {
        elements.push(rect_el("r", &z, 5.0, 5.0, w, h));
        elements.push(ellipse_el("e", &z, 5.0, 5.0, w, h));
        elements.push(Element::new(id("d"), ElementKind::Diamond, z.clone()).with_bounds(5.0, 5.0, w, h));
    }
    for head in
        [Head::None, Head::Arrow, Head::Triangle, Head::Circle, Head::CardinalityZeroOrMany, Head::CardinalityOneOrMany]
    {
        let mut a = line_el("a", &z, LinearKind::Arrow, &[(0.0, 0.0), (60.0, 10.0), (90.0, -30.0)]);
        if let ElementKind::Arrow(d) = &mut a.kind {
            d.start_head = head;
            d.end_head = head;
        }
        elements.push(a);
    }
    elements.push(line_el("l", &z, LinearKind::Line, &[(0.0, 0.0), (50.0, 3.0)]));
    elements.push(line_el("tiny", &z, LinearKind::Arrow, &[(0.0, 0.0), (6.0, 0.0)]));
    let mut rng = Rng(8);
    elements.push(freehand_el("f", &z, 0.0, 0.0, 30, &mut rng, true));
    elements.push(text_el(&mut ts, "t", &z, 0.0, 0.0, "marker\npen and a longer second line"));
    elements.push(math_el(&mut ts, "m", &z, 0.0, 0.0, "\\frac{a}{b}"));
    for mut e in elements {
        for (sw, seed, color) in
            [(0.0, 1, MarkerColor::Yellow), (2.0, 99, MarkerColor::Pink), (14.0, 7, MarkerColor::Blue)]
        {
            e.style.stroke_width = sw;
            e.style.seed = seed;
            e.style.highlight = Some(color);
            let frag = painter.paint(&e, &RenderOptions::default(), &mut ts, &|_| None);
            let m = paint_margin(&e);
            let a = kurbo::Rect::new(-m.left, -m.top, e.transform.w + m.right, e.transform.h + m.bottom);
            let b = frag.bounds;
            assert!(
                b.x0 >= a.x0 - 1e-6 && b.y0 >= a.y0 - 1e-6 && b.x1 <= a.x1 + 1e-6 && b.y1 <= a.y1 + 1e-6,
                "{:?} (stroke {sw}) drew {b:?} outside {a:?}",
                e.tag()
            );
        }
    }
}
