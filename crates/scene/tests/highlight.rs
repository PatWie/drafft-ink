#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Highlight: the model, the files, the marker colours and the reading of files from a newer version.

use drafftink_scene::{
    CharStyle, Color, Document, DocumentError, Element, ElementId, ElementKind, MARKER_ALPHA, MarkerColor,
    SpanHighlight, Style, TextData, TextRun, Tone, ZIndex, unknown::UnknownFields,
};
use serde_json::json;

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
    for color in MarkerColor::ALL {
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
    assert_eq!(doc.elements[0].style.highlight, Some(MarkerColor::Pink), "what is known is kept");
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
        TextRun::new(0, 3, CharStyle { highlight: SpanHighlight::On(MarkerColor::Green), ..CharStyle::default() }),
        TextRun::new(4, 7, CharStyle { highlight: SpanHighlight::Off, bold: true, ..CharStyle::default() }),
        TextRun { start: 8, end: 13, bold: true, ..Default::default() },
    ];
    let mut doc = Document::new();
    doc.elements = vec![Element::new(id("t"), ElementKind::Text(text), ZIndex::first())];
    let json = doc.to_json().unwrap();
    assert!(json.contains("\"highlight\":\"green\"") && json.contains("\"highlight\":\"none\""), "{json}");
    assert_eq!(json.matches("highlight").count(), 2, "the third run has none to write: {json}");
    assert_eq!(Document::from_json(&json).unwrap(), doc);
}

#[test]
fn a_run_with_only_a_highlight_is_not_plain_and_does_not_touch_glyphs() {
    let run = TextRun { start: 0, end: 2, highlight: SpanHighlight::On(MarkerColor::Blue), ..Default::default() };
    assert!(!run.style().is_plain());
    assert!(!run.styles_glyphs());
    assert!(TextRun { bold: true, ..run.clone() }.styles_glyphs());
}

#[test]
fn the_span_rule_is_span_over_element() {
    use MarkerColor::{Pink, Yellow};
    use SpanHighlight::{Inherit, Off, On};
    assert_eq!(Inherit.resolve(Some(Yellow)), Some(Yellow));
    assert_eq!(Inherit.resolve(None), None);
    assert_eq!(Off.resolve(Some(Yellow)), None);
    assert_eq!(On(Pink).resolve(Some(Yellow)), Some(Pink));
    assert_eq!(On(Pink).resolve(None), Some(Pink));
    for element in [None, Some(Yellow), Some(Pink)] {
        for wanted in [None, Some(Yellow), Some(Pink)] {
            assert_eq!(SpanHighlight::for_result(wanted, element).resolve(element), wanted);
        }
    }
    assert_eq!(SpanHighlight::for_result(Some(Yellow), Some(Yellow)), Inherit, "no value of its own when not needed");
}

#[test]
fn span_highlight_names_parse_and_bad_ones_are_refused() {
    for name in ["none", "yellow", "green", "pink", "blue", "orange"] {
        let h = SpanHighlight::from_name(name).unwrap();
        assert_eq!(h.name(), Some(name));
    }
    assert!(serde_json::from_value::<SpanHighlight>(json!("purple")).is_err());
    assert_eq!(serde_json::from_value::<SpanHighlight>(json!(null)).unwrap(), SpanHighlight::Inherit);
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

#[test]
fn every_marker_reads_as_a_marker_and_keeps_the_text_readable_on_both_canvases() {
    // (canvas, text colour as the canvas shows it, least contrast of band against canvas, least of text on band).
    let light = (Color::WHITE, Color::BLACK, Tone::Light, 1.15, 12.0);
    let dark = (Color::WHITE.for_dark_theme(), Color::BLACK.for_dark_theme(), Tone::Dark, 1.7, 5.5);
    for (canvas, text, tone, band_min, text_min) in [light, dark] {
        for m in MarkerColor::ALL {
            let band = over(m.rgb(tone), MARKER_ALPHA, canvas);
            let (against_canvas, against_text) = (contrast(band, canvas), contrast(band, text));
            assert!(against_canvas >= band_min, "{m} on {tone:?}: band against canvas {against_canvas:.2}");
            assert!(against_text >= text_min, "{m} on {tone:?}: text on band {against_text:.2}");
        }
    }
}

#[test]
fn the_dark_canvas_is_not_the_light_one_with_a_filter() {
    let dark_canvas = Color::WHITE.for_dark_theme();
    assert_eq!((dark_canvas.r, dark_canvas.g, dark_canvas.b), (18, 18, 18), "the premise of the numbers above");
    for m in MarkerColor::ALL {
        assert_ne!(m.rgb(Tone::Light), m.rgb(Tone::Dark), "{m} resolves differently per theme");
        assert!(luminance(m.rgb(Tone::Dark)) < luminance(m.rgb(Tone::Light)), "{m}: deeper on the dark canvas");
        assert_eq!(m.rgb(Tone::Light).a, 255, "the alpha is the layer's, not the colour's");
    }
    // Yellow at 50% over black must still read as a marker, the case the marker pair exists for.
    let band = over(MarkerColor::Yellow.rgb(Tone::Dark), MARKER_ALPHA, Color::BLACK);
    assert!(contrast(band, Color::BLACK) >= 2.0, "{band:?}");
}
