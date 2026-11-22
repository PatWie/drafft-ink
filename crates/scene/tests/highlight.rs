#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Highlight: the model, the files, the marker colours and the reading of files from a newer version.

use drafftink_scene::{
    CharStyle, Color, Document, DocumentError, Element, ElementId, ElementKind, MARKER_ALPHA, MarkerColor, MarkerShade,
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
    for color in MarkerColor::all() {
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
    for name in ["none", "yellow-400", "slate-300", "emerald-500"] {
        let h = SpanHighlight::from_name(name).unwrap();
        assert_eq!(h.name().as_deref(), Some(name));
    }
    assert_eq!(SpanHighlight::from_name("yellow"), Some(SpanHighlight::On(m("yellow-400"))), "an old name");
    assert!(serde_json::from_value::<SpanHighlight>(json!("purple")).is_err(), "a family needs a shade");
    assert!(serde_json::from_value::<SpanHighlight>(json!("purple-100")).is_err(), "so does a marker shade");
    assert_eq!(serde_json::from_value::<SpanHighlight>(json!(null)).unwrap(), SpanHighlight::Inherit);
}

// ---- names ----------------------------------------------------------------------------------------------------------

#[test]
fn every_family_and_marker_shade_has_a_name_that_round_trips() {
    let all: Vec<MarkerColor> = MarkerColor::all().collect();
    assert_eq!(all.len(), palette::FAMILIES.len() * MarkerShade::ALL.len(), "22 families by 3 shades");
    assert_eq!(palette::FAMILIES.len(), 22);
    for marker in &all {
        let name = marker.name();
        assert_eq!(name, format!("{}-{}", marker.family_name(), marker.shade().number()));
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
fn the_shades_are_300_400_and_500_and_nothing_else_is_a_marker() {
    assert_eq!(MarkerShade::ALL.map(MarkerShade::number), [300, 400, 500]);
    for shade in palette::SHADES.into_iter().filter(|s| ![300, 400, 500].contains(s)) {
        assert_eq!(MarkerColor::from_name(&format!("blue-{shade}")), None, "blue-{shade}");
    }
    for bad in ["", "-", "blue-", "blue-4x0", "mauve-400", "blue 400", "#facc15", "yellow-400-400", "none"] {
        assert_eq!(MarkerColor::from_name(bad), None, "{bad:?}");
    }
    let err = serde_json::from_value::<MarkerColor>(json!("blue-100")).unwrap_err().to_string();
    assert!(err.contains("shade 300, 400 or 500") && err.contains("yellow-400") && err.contains("emerald"), "{err}");
}

#[test]
fn the_five_old_names_still_load_as_the_standard_shade_of_their_family() {
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
fn the_json_schema_names_the_pattern_and_accepts_the_old_names() {
    let schema = serde_json::to_value(schemars::schema_for!(MarkerColor)).unwrap();
    let pattern = schema["pattern"].as_str().unwrap();
    assert!(pattern.contains("emerald") && pattern.contains("(300|400|500)"), "{pattern}");
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

/// The least contrast a band must have against the canvas, and a text on it, per marker shade and canvas (the
/// measured least over the 22 families is in the comment of each line). Light: the wash shows (1.14; 1.153 measured,
/// for lime and yellow) and keeps black text at 12 or more (15.2); the marker (1.2; 1.244, 12.7) and the strong marker
/// (1.35; 1.39, with 10; 10.6 measured) follow. Dark: every band shows against the page (1.4 for the wash, 1.5 for the
/// marker, 1.8 for the strong one; 1.417, 1.585, 1.917 measured) and the light text on it stays above 8, 6 and the
/// WCAG AA 4.5 (8.8, 6.5, 4.9).
fn limits(tone: Tone, shade: MarkerShade) -> (f64, f64) {
    match (tone, shade) {
        (Tone::Light, MarkerShade::Soft) => (1.14, 12.0),
        (Tone::Light, MarkerShade::Standard) => (1.2, 12.0),
        (Tone::Light, MarkerShade::Strong) => (1.35, 10.0),
        (Tone::Dark, MarkerShade::Soft) => (1.4, 8.0),
        (Tone::Dark, MarkerShade::Standard) => (1.5, 6.0),
        (Tone::Dark, MarkerShade::Strong) => (1.8, 4.5),
    }
}

#[test]
fn every_family_and_shade_reads_as_a_marker_and_keeps_the_text_readable_on_both_canvases() {
    let canvases = [
        (Color::WHITE, Color::BLACK, Tone::Light),
        (Color::WHITE.for_dark_theme(), Color::BLACK.for_dark_theme(), Tone::Dark),
    ];
    for (canvas, text, tone) in canvases {
        for marker in MarkerColor::all() {
            let (band_min, text_min) = limits(tone, marker.shade());
            let band = over(marker.rgb(tone), MARKER_ALPHA, canvas);
            let (against_canvas, against_text) = (contrast(band, canvas), contrast(band, text));
            assert!(against_canvas >= band_min, "{marker} on {tone:?}: band against canvas {against_canvas:.2}");
            assert!(against_text >= text_min, "{marker} on {tone:?}: text on band {against_text:.2}");
        }
    }
}

#[test]
fn the_dark_value_is_the_same_family_at_the_mirrored_shade_and_the_greys_one_step_lighter() {
    let dark_name = |marker: MarkerColor| {
        let ink = marker.rgb(Tone::Dark);
        palette::SHADES
            .into_iter()
            .find(|s| palette::lookup(&format!("{}-{s}", marker.family_name())) == Some(ink))
            .unwrap()
    };
    for marker in MarkerColor::all() {
        let grey = ["slate", "gray", "zinc", "neutral", "stone"].contains(&marker.family_name());
        let expected = match (marker.shade(), grey) {
            (MarkerShade::Soft, false) => 700,
            (MarkerShade::Standard, false) => 600,
            (MarkerShade::Strong, false) => 500,
            (MarkerShade::Soft, true) => 600,
            (MarkerShade::Standard, true) => 500,
            (MarkerShade::Strong, true) => 400,
        };
        assert_eq!(dark_name(marker), expected, "{marker}");
        // The strong marker of a coloured family is the palette 500 on both canvases (the mirror of 500 is 500).
        if marker.shade() == MarkerShade::Strong && !grey {
            assert_eq!(marker.rgb(Tone::Light), marker.rgb(Tone::Dark), "{marker}");
        } else {
            assert_ne!(marker.rgb(Tone::Light), marker.rgb(Tone::Dark), "{marker} resolves differently per theme");
            // A grey at the strong shade is the one exception: its dark value is the palette 400, so that its band shows.
            let exception = marker.shade() == MarkerShade::Strong;
            assert!(
                exception || luminance(marker.rgb(Tone::Dark)) < luminance(marker.rgb(Tone::Light)),
                "{marker}: deeper on dark"
            );
        }
        assert_eq!(marker.rgb(Tone::Light).a, 255, "the alpha is the layer's, not the colour's");
    }
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

// ---- any colour as a marker ----

#[test]
fn a_marker_name_is_itself_and_a_palette_colour_takes_the_nearest_marker_shade() {
    for marker in MarkerColor::all() {
        assert_eq!(MarkerColor::nearest(&marker.name()), Some(marker));
    }
    assert_eq!(MarkerColor::nearest("yellow"), Some(m("yellow-400")), "the old names still mean the standard shade");
    for (picked, want) in [
        ("blue-25", "blue-300"),
        ("blue-50", "blue-300"),
        ("blue-200", "blue-300"),
        ("blue-300", "blue-300"),
        ("blue-400", "blue-400"),
        ("blue-500", "blue-500"),
        ("blue-600", "blue-500"),
        ("blue-950", "blue-500"),
        ("rose-100", "rose-300"),
        ("Rose-900", "rose-500"),
    ] {
        assert_eq!(MarkerColor::nearest(picked), Some(m(want)), "{picked}");
    }
}

#[test]
fn a_custom_colour_takes_the_nearest_marker_and_a_non_colour_takes_none() {
    // The ink of a marker is the marker's own nearest; a colour a little off still lands on it.
    for marker in MarkerColor::all() {
        let c = marker.rgb(Tone::Light);
        let near = Color::rgb(c.r.saturating_add(2), c.g.saturating_sub(2), c.b).to_hex();
        assert_eq!(MarkerColor::nearest(&near), Some(marker), "{near}");
    }
    assert_eq!(MarkerColor::nearest("#ff0000").map(|x| x.family_name()), Some("red"));
    assert_eq!(MarkerColor::nearest("#0000ff80"), MarkerColor::nearest("#0000ff"), "an alpha is ignored");
    assert_eq!(MarkerColor::nearest("#0000ff").map(|x| x.family_name()), Some("indigo"), "the palette's nearest blue");
    assert_eq!(MarkerColor::nearest("#fff").map(|x| x.shade()), Some(MarkerShade::Soft), "white is the palest marker");
    for bad in ["", "banana", "blue-", "blue-7", "nope-500", "#12"] {
        assert_eq!(MarkerColor::nearest(bad), None, "{bad:?}");
    }
}
