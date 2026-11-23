#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Highlight: the model, the files, the marker colours and the reading of files from a newer version.

use drafftink_scene::{
    CharStyle, Color, Document, DocumentError, Element, ElementId, ElementKind, MARKER_ALPHA, MarkerColor, Shade,
    SpanHighlight, Style, TextData, TextRun, Tone, ZIndex, palette, unknown::UnknownFields,
};
use serde_json::json;

/// The marker of a name.
fn m(name: &str) -> MarkerColor {
    MarkerColor::from_name(name).unwrap()
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn rect(name: &str, highlight: Option<MarkerColor>) -> Element {
    let mut e = Element::new(id(name), ElementKind::Rectangle, ZIndex::first()).with_bounds(0.0, 0.0, 40.0, 20.0);
    e.style.highlight = highlight;
    e
}

#[test]
fn a_highlight_round_trips_and_no_highlight_writes_nothing() {
    let custom = [MarkerColor::custom(Color::rgb(0x12, 0xab, 0xef)), MarkerColor::custom(Color::rgb(0, 0, 0))];
    for color in MarkerColor::palette().chain(custom) {
        let mut doc = Document::new();
        doc.elements = vec![rect("r", Some(color))];
        let json = doc.to_json().unwrap();
        assert!(json.contains(&format!("\"highlight\":\"{}\"", color.name())), "{json}");
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }
    let mut doc = Document::new();
    doc.elements = vec![rect("r", None)];
    assert!(!doc.to_json().unwrap().contains("highlight"), "a default is not written");
}

#[test]
fn a_file_from_before_highlights_loads_with_none() {
    let mut doc = Document::new();
    doc.elements = vec![rect("r", None)];
    let loaded = Document::from_json_lenient(&doc.to_json().unwrap()).unwrap();
    assert!(loaded.unknown.is_empty());
    assert_eq!(loaded.document.elements[0].style.highlight, None);
    assert_eq!(Style::default().highlight, None);
}

#[test]
fn a_bad_highlight_name_is_an_error_not_an_unknown_field() {
    let text = json!({"format": "drafftink", "elements": [{
        "id": "r", "kind": {"type": "rectangle"}, "transform": {"x": 0.0, "y": 0.0, "w": 5.0, "h": 5.0},
        "style": {"highlight": "mauve"}, "index": "a0"
    }]})
    .to_string();
    assert!(matches!(Document::from_json_lenient(&text), Err(DocumentError::Parse(_))));
}

/// A document of a newer version: a style field, a field of a text's data, a field of a run and a top-level one.
fn newer_text() -> String {
    json!({
        "format": "drafftink", "version": 1, "future_top": 1,
        "elements": [
            {"id": "r", "kind": {"type": "rectangle"}, "transform": {"x": 0.0, "y": 0.0, "w": 5.0, "h": 5.0},
             "style": {"highlight": "pink", "glow": 3}, "index": "a0"},
            {"id": "t", "kind": {"type": "text", "content": "hello", "spellcheck": true,
                                 "runs": [{"start": 0, "end": 2, "bold": true, "wavy": 1}]},
             "transform": {"x": 0.0, "y": 20.0, "w": 50.0, "h": 20.0}, "index": "a1"}
        ]
    })
    .to_string()
}

#[test]
fn unknown_fields_are_ignored_collected_and_named() {
    let text = newer_text();
    assert!(Document::from_json(&text).is_err(), "the strict reader still refuses them");
    let loaded = Document::from_json_lenient(&text).unwrap();
    let mut paths = loaded.unknown.paths().to_vec();
    paths.sort();
    assert_eq!(
        paths,
        ["elements[0].style.glow", "elements[1].kind.runs[0].wavy", "elements[1].kind.spellcheck", "future_top"]
    );
    assert_eq!(loaded.unknown.count(), 4);
    let doc = loaded.document;
    assert_eq!(doc.elements.len(), 2);
    assert_eq!(
        doc.elements[0].style.highlight,
        Some(m("pink-400")),
        "what is known is kept (the old name pink is pink-400)"
    );
    let ElementKind::Text(t) = &doc.elements[1].kind else { panic!("a text") };
    assert_eq!((t.content.as_str(), t.runs.len(), t.runs[0].bold), ("hello", 1, true));
    assert!(UnknownFields::WARNING.contains("newer version of Drafft.ink"));
}

#[test]
fn other_errors_stay_errors_next_to_unknown_fields() {
    let text = newer_text().replace("\"hello\"", "7");
    assert!(Document::from_json_lenient(&text).is_err());
    assert!(matches!(Document::from_json_lenient("[]"), Err(DocumentError::NotAnObject)));
    let newer = json!({"format": "drafftink", "version": 99}).to_string();
    assert!(matches!(Document::from_json_lenient(&newer), Err(DocumentError::NewerVersion { .. })));
}

#[test]
fn many_elements_with_unknown_fields_load_without_reparsing_the_document() {
    let keys = ZIndex::rebalance(3000).unwrap();
    let elements: Vec<_> = (0..3000)
        .map(|i| {
            json!({"id": format!("e{i}"), "kind": {"type": "rectangle"},
                   "transform": {"x": 0.0, "y": 0.0, "w": 5.0, "h": 5.0}, "style": {"glow": i}, "index": keys[i].as_str()})
        })
        .collect();
    let text = json!({"format": "drafftink", "elements": elements}).to_string();
    let loaded = Document::from_json_lenient(&text).unwrap();
    assert_eq!(loaded.document.elements.len(), 3000);
    assert_eq!(loaded.unknown.count(), 3000);
    assert_eq!(loaded.unknown.paths().len(), drafftink_scene::unknown::MAX_RECORDED);
}

#[test]
fn a_run_highlight_round_trips_and_inherit_is_not_written() {
    let mut text = TextData::new("one two three");
    text.runs = vec![
        TextRun::new(0, 3, CharStyle { highlight: SpanHighlight::On(m("green-400")), ..CharStyle::default() }),
        TextRun::new(4, 7, CharStyle { highlight: SpanHighlight::Off, bold: true, ..CharStyle::default() }),
        TextRun { start: 8, end: 13, bold: true, ..Default::default() },
    ];
    let mut doc = Document::new();
    doc.elements = vec![Element::new(id("t"), ElementKind::Text(text), ZIndex::first())];
    let json = doc.to_json().unwrap();
    assert!(json.contains("\"highlight\":\"green-400\"") && json.contains("\"highlight\":\"none\""), "{json}");
    assert_eq!(json.matches("highlight").count(), 2, "the third run has none to write: {json}");
    assert_eq!(Document::from_json(&json).unwrap(), doc);
}

#[test]
fn a_run_with_only_a_highlight_is_not_plain_and_does_not_touch_glyphs() {
    let run = TextRun { start: 0, end: 2, highlight: SpanHighlight::On(m("blue-400")), ..Default::default() };
    assert!(!run.style().is_plain());
    assert!(!run.styles_glyphs());
    assert!(TextRun { bold: true, ..run.clone() }.styles_glyphs());
}

#[test]
fn the_span_rule_is_span_over_element() {
    let (pink, yellow) = (m("pink-300"), m("yellow-400"));
    use SpanHighlight::{Inherit, Off, On};
    assert_eq!(Inherit.resolve(Some(yellow)), Some(yellow));
    assert_eq!(Inherit.resolve(None), None);
    assert_eq!(Off.resolve(Some(yellow)), None);
    assert_eq!(On(pink).resolve(Some(yellow)), Some(pink));
    assert_eq!(On(pink).resolve(None), Some(pink));
    for element in [None, Some(yellow), Some(pink)] {
        for wanted in [None, Some(yellow), Some(pink)] {
            assert_eq!(SpanHighlight::for_result(wanted, element).resolve(element), wanted);
        }
    }
    assert_eq!(SpanHighlight::for_result(Some(yellow), Some(yellow)), Inherit, "no value of its own when not needed");
}

#[test]
fn span_highlight_names_parse_and_bad_ones_are_refused() {
    for name in ["none", "yellow-400", "slate-300", "emerald-500", "blue-25", "rose-950", "#12abef"] {
        let h = SpanHighlight::from_name(name).unwrap();
        assert_eq!(h.name().as_deref(), Some(name));
    }
    assert_eq!(SpanHighlight::from_name("yellow"), Some(SpanHighlight::On(m("yellow-400"))), "an old name");
    assert!(serde_json::from_value::<SpanHighlight>(json!("purple")).is_err(), "a family needs a shade");
    assert!(
        serde_json::from_value::<SpanHighlight>(json!("purple-150")).is_err(),
        "and the shade must be a palette one"
    );
    assert!(serde_json::from_value::<SpanHighlight>(json!("#12")).is_err());
    assert_eq!(serde_json::from_value::<SpanHighlight>(json!(null)).unwrap(), SpanHighlight::Inherit);
}

// ---- names ----------------------------------------------------------------------------------------------------------

#[test]
fn every_family_and_shade_has_a_name_that_round_trips() {
    let all: Vec<MarkerColor> = MarkerColor::palette().collect();
    assert_eq!(palette::FAMILIES.len(), 22);
    assert_eq!(Shade::ALL.map(Shade::number), palette::SHADES);
    assert_eq!(all.len(), palette::FAMILIES.len() * palette::SHADES.len(), "22 families by 12 shades");
    for marker in &all {
        let name = marker.name();
        let (family, shade) = (marker.family_name().unwrap(), marker.shade().unwrap());
        assert_eq!(name, format!("{family}-{}", shade.number()));
        assert_eq!(MarkerColor::from_name(&name), Some(*marker), "{name}");
        assert_eq!(MarkerColor::from_name(&name.to_uppercase()), Some(*marker), "case is ignored: {name}");
        assert_eq!(serde_json::to_value(marker).unwrap(), json!(name));
        assert_eq!(serde_json::from_value::<MarkerColor>(json!(name)).unwrap(), *marker);
        assert_eq!(marker.to_string(), name);
        // The light ink is the palette colour of the same name: a marker is a palette colour, not an invention.
        assert_eq!(Some(marker.rgb(Tone::Light)), palette::lookup(&name), "{name}");
    }
    let mut names: Vec<String> = all.iter().map(|x| x.name()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), all.len(), "no two markers share a name");
    assert_eq!(MarkerColor::YELLOW, m("yellow-400"), "the default marker");
}

#[test]
fn a_custom_colour_is_written_as_hex_without_its_alpha_and_reads_back() {
    let c = MarkerColor::custom(Color::rgba(0x12, 0xab, 0xef, 0x40));
    assert_eq!(c, MarkerColor::Custom(Color::rgb(0x12, 0xab, 0xef)), "the alpha is not part of a marker");
    assert_eq!((c.name(), c.to_string()), ("#12abef".to_owned(), "#12abef".to_owned()));
    assert_eq!(serde_json::to_value(c).unwrap(), json!("#12abef"));
    for text in ["#12abef", "#12ABEF", "  #12abef ", "#12abef80"] {
        assert_eq!(MarkerColor::from_name(text), Some(c), "{text:?}");
        assert_eq!(serde_json::from_value::<MarkerColor>(json!(text)).unwrap(), c, "{text:?}");
    }
    assert_eq!(MarkerColor::from_name("#fa0"), Some(MarkerColor::custom(Color::rgb(0xff, 0xaa, 0x00))));
    assert_eq!((c.family_name(), c.shade(), c.family().is_none()), (None, None, true));
    // A custom colour that equals a palette ink stays custom: the name says what was chosen.
    let ink = m("blue-500").rgb(Tone::Light);
    assert_ne!(MarkerColor::custom(ink), m("blue-500"));
    assert_eq!(MarkerColor::custom(ink).name(), ink.to_hex());
}

#[test]
fn malformed_names_are_refused_with_the_way_to_write_one() {
    for bad in [
        "",
        "-",
        "blue-",
        "blue-4x0",
        "mauve-400",
        "blue 400",
        "yellow-400-400",
        "none",
        "blue-0",
        "blue-150",
        "blue-1000",
        "blue--400",
        "#",
        "#12",
        "#12345",
        "#1234567",
        "#gg0000",
        "#ff00ff0",
        "banana",
        "transparent",
        "purple",
    ] {
        assert_eq!(MarkerColor::from_name(bad), None, "{bad:?}");
    }
    let err = serde_json::from_value::<MarkerColor>(json!("blue-7")).unwrap_err().to_string();
    assert!(
        err.contains("blue-7") && err.contains("950") && err.contains("#rrggbb") && err.contains("emerald"),
        "{err}"
    );
    assert!(serde_json::from_value::<MarkerColor>(json!(5)).is_err());
    assert!(serde_json::from_value::<MarkerColor>(json!(null)).is_err());
    // A bad name is refused in a file as well, and the text is cut at 40 characters in the message.
    let long = serde_json::from_value::<MarkerColor>(json!("x".repeat(500))).unwrap_err().to_string();
    assert!(long.len() < 400, "{}", long.len());
}

#[test]
fn the_five_old_names_still_load_as_shade_400_of_their_family() {
    for (old, new) in [
        ("yellow", "yellow-400"),
        ("green", "green-400"),
        ("pink", "pink-400"),
        ("blue", "blue-400"),
        ("orange", "orange-400"),
    ] {
        assert_eq!(m(old), m(new), "{old}");
        assert_eq!(serde_json::from_value::<MarkerColor>(json!(old)).unwrap(), m(new));
        // They are written under the new name.
        assert_eq!(serde_json::to_value(m(old)).unwrap(), json!(new));
        // A file of the first builds loads, with the highlight of an element and of a run.
        let text = json!({"format": "drafftink", "elements": [{
            "id": "t", "kind": {"type": "text", "content": "abc", "runs": [{"start": 0, "end": 1, "highlight": old}]},
            "transform": {"x": 0.0, "y": 0.0, "w": 5.0, "h": 5.0},
            "style": {"highlight": old}, "index": "a0"
        }]})
        .to_string();
        let doc = Document::from_json(&text).unwrap();
        let e = &doc.elements[0];
        assert_eq!(e.style.highlight, Some(m(new)));
        let ElementKind::Text(t) = &e.kind else { panic!("a text") };
        assert_eq!(t.runs[0].highlight, SpanHighlight::On(m(new)));
        assert!(doc.to_json().unwrap().contains(&format!("\"{new}\"")));
    }
}

#[test]
fn the_json_schema_names_every_shade_the_hex_forms_and_the_old_names() {
    let schema = serde_json::to_value(schemars::schema_for!(MarkerColor)).unwrap();
    let pattern = schema["pattern"].as_str().unwrap();
    assert!(
        pattern.contains("emerald") && pattern.contains("(25|50|100|200|300|400|500|600|700|800|900|950)"),
        "{pattern}"
    );
    assert!(pattern.contains("#[0-9a-fA-F]{6}"), "{pattern}");
    for old in ["yellow", "green", "pink", "blue", "orange"] {
        assert!(pattern.contains(old), "{old}");
    }
}

// ---- theme resolution and contrast --------------------------------------------------------------------------------

fn lin(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn luminance(c: Color) -> f64 {
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

fn contrast(a: Color, b: Color) -> f64 {
    let (hi, lo) = (luminance(a).max(luminance(b)), luminance(a).min(luminance(b)));
    (hi + 0.05) / (lo + 0.05)
}

/// `ink` at `alpha` over `under`.
fn over(ink: Color, alpha: f64, under: Color) -> Color {
    let mix = |f: u8, b: u8| (f64::from(f) * alpha + f64::from(b) * (1.0 - alpha)).round() as u8;
    Color::rgb(mix(ink.r, under.r), mix(ink.g, under.g), mix(ink.b, under.b))
}

/// The least contrast a band must have against the canvas, and a text on it, per default shade and canvas (the
/// measured least over the 22 families is in the comment of each line). These are the shades the pickers offer
/// first; no colour is refused for missing them, they are what the defaults promise. Light: the wash (300) shows (1.14;
/// 1.153 measured, for lime and yellow) and keeps black text at 12 or more (15.2); the marker (400; 1.2; 1.244, 12.7)
/// and the strong marker (500; 1.35; 1.39, with 10; 10.6 measured) follow. Dark: every band shows against the page
/// (1.4 for the wash, 1.5 for the marker, 1.8 for the strong one; 1.417, 1.585, 1.917 measured) and the light text on it
/// stays above 8, 6 and the WCAG AA 4.5 (8.8, 6.5, 4.9).
fn limits(tone: Tone, shade: Shade) -> (f64, f64) {
    match (tone, shade.number()) {
        (Tone::Light, 300) => (1.14, 12.0),
        (Tone::Light, 400) => (1.2, 12.0),
        (Tone::Light, _) => (1.35, 10.0),
        (Tone::Dark, 300) => (1.4, 8.0),
        (Tone::Dark, 400) => (1.5, 6.0),
        (Tone::Dark, _) => (1.8, 4.5),
    }
}

#[test]
fn the_default_shades_300_400_and_500_read_as_markers_and_keep_the_text_readable_on_both_canvases() {
    assert_eq!(Shade::MARKER.map(Shade::number), [300, 400, 500]);
    let canvases = [
        (Color::WHITE, Color::BLACK, Tone::Light),
        (Color::WHITE.for_dark_theme(), Color::BLACK.for_dark_theme(), Tone::Dark),
    ];
    for (canvas, text, tone) in canvases {
        for marker in MarkerColor::palette().filter(|x| x.shade().is_some_and(|s| Shade::MARKER.contains(&s))) {
            let (band_min, text_min) = limits(tone, marker.shade().unwrap());
            let band = over(marker.rgb(tone), MARKER_ALPHA, canvas);
            let (against_canvas, against_text) = (contrast(band, canvas), contrast(band, text));
            assert!(against_canvas >= band_min, "{marker} on {tone:?}: band against canvas {against_canvas:.2}");
            assert!(against_text >= text_min, "{marker} on {tone:?}: text on band {against_text:.2}");
        }
    }
}

#[test]
fn no_shade_and_no_colour_is_refused_for_its_contrast() {
    // Readability is the person's choice: the palest wash, the deepest ink, white and black are all markers.
    for name in ["yellow-25", "blue-950", "slate-25", "#ffffff", "#000000", "#fefefe"] {
        let marker = m(name);
        assert_eq!(serde_json::from_value::<MarkerColor>(json!(name)).unwrap(), marker);
        assert_eq!(serde_json::from_value::<SpanHighlight>(json!(name)).unwrap(), SpanHighlight::On(marker));
    }
}

/// Whether `ink` is the palette colour of the family of `marker` at `shade` (some families repeat a colour in two
/// neighbouring shades, so the shade is asked for rather than searched).
fn is_shade(marker: MarkerColor, ink: Color, shade: u16) -> bool {
    palette::lookup(&format!("{}-{shade}", marker.family_name().unwrap())) == Some(ink)
}

#[test]
fn the_dark_shade_is_the_mirror_and_the_table_is_the_documented_one() {
    let table: [(u16, u16); 12] = [
        (25, 950),
        (50, 950),
        (100, 900),
        (200, 800),
        (300, 700),
        (400, 600),
        (500, 500),
        (600, 400),
        (700, 300),
        (800, 200),
        (900, 100),
        (950, 50),
    ];
    for (light, dark) in table {
        assert_eq!(Shade::from_number(light).unwrap().dark().number(), dark, "{light}");
    }
    // Everything but 25 is 1000 minus the shade, and from 50 to 950 the mapping is its own inverse.
    for shade in Shade::ALL.into_iter().skip(1) {
        assert_eq!(shade.dark().number(), 1000 - shade.number(), "{shade}");
        if shade.number() != 950 {
            assert_eq!(shade.dark().dark(), shade, "{shade}");
        }
    }
    assert_eq!(Shade::DEFAULT.number(), 400);
}

#[test]
fn a_palette_marker_resolves_to_the_mirror_shade_on_dark_and_the_greys_one_step_lighter() {
    const GREYS: [&str; 5] = ["slate", "gray", "zinc", "neutral", "stone"];
    for marker in MarkerColor::palette() {
        let (family, shade) = (marker.family_name().unwrap(), marker.shade().unwrap());
        let grey = GREYS.contains(&family);
        assert!(is_shade(marker, marker.rgb(Tone::Light), shade.number()), "{marker} on light");
        let mirror = shade.dark();
        let expected = if grey { palette::SHADES[mirror.index().saturating_sub(1)] } else { mirror.number() };
        assert!(is_shade(marker, marker.rgb(Tone::Dark), expected), "{marker} on dark, wanted {expected}");
        assert_eq!(marker.rgb(Tone::Light).a, 255, "the alpha is the layer's, not the colour's");
        assert_eq!(marker.rgb(Tone::Dark).a, 255);
    }
    // The shades that were markers before keep the value they had: 300 is 700, 400 is 600 and 500 stays (greys: 600, 500, 400).
    for (name, light, dark) in [
        ("amber-300", 300, 700),
        ("amber-400", 400, 600),
        ("amber-500", 500, 500),
        ("stone-300", 300, 600),
        ("stone-400", 400, 500),
        ("stone-500", 500, 400),
    ] {
        let marker = m(name);
        assert!(is_shade(marker, marker.rgb(Tone::Light), light), "{name}");
        assert!(is_shade(marker, marker.rgb(Tone::Dark), dark), "{name}");
    }
    // Mirrored pairs read as a deep value for a pale one: the coloured 300 to 400 are deeper on dark.
    for shade in [100, 200, 300, 400] {
        let marker = m(&format!("blue-{shade}"));
        assert!(luminance(marker.rgb(Tone::Dark)) < luminance(marker.rgb(Tone::Light)), "{marker}");
    }
}

#[test]
fn a_custom_colour_is_the_same_on_both_canvases_and_is_never_filtered() {
    for c in
        [Color::rgb(0x12, 0xab, 0xef), Color::WHITE, Color::BLACK, Color::rgb(255, 255, 0), Color::rgb(250, 204, 21)]
    {
        let marker = MarkerColor::custom(c);
        assert_eq!(marker.rgb(Tone::Light), c);
        assert_eq!(marker.rgb(Tone::Dark), c, "not mapped for the dark theme");
        assert_ne!(
            marker.rgb(Tone::Dark),
            c.for_dark_theme(),
            "{c:?}: the dark filter is for stored colours, not markers"
        );
    }
    // A palette marker, by contrast, resolves differently on the two canvases (but for 500, its own mirror).
    assert_ne!(m("yellow-400").rgb(Tone::Light), m("yellow-400").rgb(Tone::Dark));
    assert_eq!(m("yellow-500").rgb(Tone::Light), m("yellow-500").rgb(Tone::Dark));
}

#[test]
fn the_dark_canvas_is_not_the_light_one_with_a_filter() {
    let dark_canvas = Color::WHITE.for_dark_theme();
    assert_eq!((dark_canvas.r, dark_canvas.g, dark_canvas.b), (18, 18, 18), "the premise of the numbers above");
    // Yellow at 50% over black must still read as a marker, the case the dark values exist for.
    let band = over(MarkerColor::YELLOW.rgb(Tone::Dark), MARKER_ALPHA, Color::BLACK);
    assert!(contrast(band, Color::BLACK) >= 2.0, "{band:?}");
    // And the dark filter would not do: it maps the light yellow to something much duller than the dark value shows.
    let filtered = over(MarkerColor::YELLOW.rgb(Tone::Light).for_dark_theme(), MARKER_ALPHA, dark_canvas);
    let chosen = over(MarkerColor::YELLOW.rgb(Tone::Dark), MARKER_ALPHA, dark_canvas);
    assert!(contrast(chosen, dark_canvas) > contrast(filtered, dark_canvas), "{filtered:?} {chosen:?}");
}

#[test]
fn a_custom_highlight_round_trips_in_a_file_on_an_element_and_on_a_run() {
    let custom = MarkerColor::custom(Color::rgb(0x12, 0xab, 0xef));
    let mut text = TextData::new("one two");
    text.runs = vec![TextRun::new(0, 3, CharStyle { highlight: SpanHighlight::On(custom), ..CharStyle::default() })];
    let mut doc = Document::new();
    let keys = ZIndex::rebalance(2).unwrap();
    doc.elements =
        vec![rect("r", Some(m("blue-100"))), Element::new(id("t"), ElementKind::Text(text), keys[1].clone())];
    doc.elements[0].index = keys[0].clone();
    doc.elements[1].style.highlight = Some(m("slate-950"));
    let json = doc.to_json().unwrap();
    assert!(
        json.contains("\"highlight\":\"#12abef\"") && json.contains("blue-100") && json.contains("slate-950"),
        "{json}"
    );
    assert_eq!(Document::from_json(&json).unwrap(), doc);
    let loaded = Document::from_json_lenient(&json).unwrap();
    assert!(loaded.unknown.is_empty());
    assert_eq!(loaded.document, doc);
}
