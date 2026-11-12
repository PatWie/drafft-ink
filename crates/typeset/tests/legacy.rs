//! Comparison with the old app (commit 8ce40ab).
//!
//! The old documents carry bounds that are estimates (`bounds_estimated: true` in the facts: the
//! old app measured text only while rendering, and the fixture generator never rendered). So the
//! real reference is the old renderer's PNG (`tests/fixtures/render`). These tests lay out the
//! same documents, rasterise nothing, and compare the ink extent of the outlines with the ink
//! extent found in the old PNG. The tolerance is 1.5 px: anti-aliasing and the PNG's integer pixel grid.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_typeset::*;
use serde_json::Value;

struct Png {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

fn read_png(path: &std::path::Path) -> Png {
    let file = std::fs::File::open(path).expect("png fixture");
    let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().expect("png header");
    let mut buf = vec![0; reader.output_buffer_size().expect("png size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    assert_eq!(info.color_type, png::ColorType::Rgba, "fixture PNGs are RGBA");
    Png { w: info.width as usize, h: info.height as usize, rgba: buf }
}

impl Png {
    /// Ink bounds `(x0, y0, x1, y1)` inside the pixel rectangle, ink being clearly not white.
    fn ink(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Option<(usize, usize, usize, usize)> {
        let mut out: Option<(usize, usize, usize, usize)> = None;
        for y in y0..y1.min(self.h) {
            for x in x0..x1.min(self.w) {
                let p = &self.rgba[(y * self.w + x) * 4..][..4];
                let dist = 255 - p[0].min(p[1]).min(p[2]);
                if dist > 70 {
                    out = Some(match out {
                        None => (x, y, x, y),
                        Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x), d.max(y)),
                    });
                }
            }
        }
        out
    }
}

fn doc(name: &str) -> Value {
    let text = std::fs::read_to_string(fixture(&format!("legacy/{name}.json"))).expect("legacy doc");
    serde_json::from_str(&text).expect("json")
}

fn shapes(doc: &Value, kind: &str) -> Vec<Value> {
    let map = doc["shapes"].as_object().expect("shapes");
    let mut v: Vec<Value> = doc["z_order"]
        .as_array()
        .expect("z_order")
        .iter()
        .filter_map(|id| map.get(id.as_str()?))
        .filter_map(|s| s.get(kind).cloned())
        .collect();
    v.sort_by(|a, b| a["position"]["y"].as_f64().partial_cmp(&b["position"]["y"].as_f64()).expect("order"));
    v
}

fn legacy_family(name: &str) -> FontFamily {
    match name {
        "GelPen" => FontFamily::GelPen,
        "NotoSans" => FontFamily::NotoSans,
        "GelPenSerif" => FontFamily::GelPenSerif,
        _ => FontFamily::VanillaExtract,
    }
}

fn legacy_weight(name: &str) -> FontWeight {
    match name {
        "Light" => FontWeight::Light,
        "Heavy" => FontWeight::Heavy,
        _ => FontWeight::Regular,
    }
}

fn colour(v: &Value) -> Rgba8 {
    let c = |k: &str| v[k].as_u64().expect("channel") as u8;
    Rgba8::new(c("r"), c("g"), c("b"), c("a"))
}

fn spec_of(t: &Value) -> TextSpec {
    let style = TextStyle::from_legacy(
        legacy_family(t["font_family"].as_str().expect("family")),
        legacy_weight(t["font_weight"].as_str().expect("weight")),
        t["font_size"].as_f64().expect("size") as f32,
        colour(&t["style"]["stroke_color"]),
    );
    let content = t["content"].as_str().expect("content");
    let mut spec = TextSpec::styled(content, style);
    let colors: Vec<Option<Rgba8>> =
        t["char_colors"].as_array().expect("char_colors").iter().map(|c| (!c.is_null()).then(|| colour(c))).collect();
    spec.spans = Span::from_char_colors(content, &colors);
    spec
}

/// The twelve family x weight rows at x = 0 of `text_fonts`: every row's ink must end where the
/// old renderer's ink ended, and its vertical extent must match.
#[test]
#[cfg(feature = "hand-fonts")]
fn text_fonts_ink_matches_the_old_render() {
    let doc = doc("text_fonts");
    let png = read_png(&fixture("render/text_fonts.png"));
    // The export fits the (estimated) document bounds [0, 0, 1146.4, 834] and adds 20 px.
    let (ox, oy) = (20.0, 20.0);
    let mut ts = Typesetter::new();
    let rows: Vec<Value> = shapes(&doc, "Text")
        .into_iter()
        .filter(|t| t["position"]["x"].as_f64() == Some(0.0) && t["font_size"].as_f64() == Some(24.0))
        .collect();
    assert_eq!(rows.len(), 12);
    for t in rows {
        let spec = spec_of(&t);
        let layout = ts.text(&spec);
        let y = t["position"]["y"].as_f64().expect("y");
        let ink = ink_bbox(&layout.outline()).expect("ink");
        // Rows are 40 px apart; search the band of this row.
        let (px0, py0) = (ox as usize, (oy + y) as usize);
        let found = png.ink(px0, py0.saturating_sub(2), 900, py0 + 38).expect("old render has ink in this row");
        let label = t["content"].as_str().expect("content");
        let (old_x1, old_y0, old_y1) =
            (found.2 as f64 + 1.0 - ox, found.1 as f64 - oy - y, found.3 as f64 + 1.0 - oy - y);
        // The "Size N" labels at x = 900 share these rows, so the search stops at pixel 900. A row
        // that is wider than that can only be checked to reach the edge.
        if ink.x1 + ox < 899.0 {
            assert!((ink.x1 - old_x1).abs() < 1.5, "{label}: right ink edge new {} old {old_x1}", ink.x1);
        } else {
            assert!(old_x1 + ox > 898.0, "{label}: old ink stops at {old_x1} but new ink reaches {}", ink.x1);
        }
        // The fixture puts a three-line label at (0, 320), on top of the GelPen Serif Heavy row,
        // and the Vanilla Extract rows (y >= 360) reach into their neighbours (the font has tall
        // overshoot), so the old render's vertical ink there is not the row's own.
        if y < 320.0 {
            assert!((ink.y0 - old_y0).abs() < 1.5, "{label}: top ink edge new {} old {old_y0}", ink.y0);
            assert!((ink.y1 - old_y1).abs() < 1.5, "{label}: bottom ink edge new {} old {old_y1}", ink.y1);
        }
        // The advance box contains the ink, with at most a few pixels of side bearing.
        assert!(
            layout.size.width >= ink.x1 - 1.0 && layout.size.width - ink.x1 < 6.0,
            "{label}: width {} ink {}",
            layout.size.width,
            ink.x1
        );
    }
}

/// The old estimate was `chars * size * factor` with a guessed factor per family. Real widths are
/// 0.6 to 1.25 times it for the Latin rows (Vanilla Extract is the widest at 1.15), which is how
/// rough the estimate was: it is why bounds must come from layout.
#[test]
fn old_estimated_bounds_were_rough_but_in_the_same_range() {
    let doc = doc("text_fonts");
    let facts: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture("legacy/text_fonts.facts.json")).expect("facts"))
            .expect("json");
    let rows = facts["doc"]["shapes"].as_array().expect("shapes");
    let mut ts = Typesetter::new();
    let texts = shapes(&doc, "Text");
    let mut checked = 0;
    for t in texts.iter().filter(|t| t["position"]["x"].as_f64() == Some(0.0) && t["font_size"].as_f64() == Some(24.0))
    {
        let id = t["id"].as_str().expect("id");
        let fact = rows.iter().find(|r| r["id"] == id).expect("fact row");
        let est_w = fact["bounds"][2].as_f64().expect("x1") - fact["bounds"][0].as_f64().expect("x0");
        let w = ts.measure(&spec_of(t)).width;
        let ratio = w / est_w;
        assert!((0.6..=1.25).contains(&ratio), "{}: new {w} estimate {est_w}", t["content"]);
        checked += 1;
    }
    assert_eq!(checked, 12);
}

#[test]
#[cfg(feature = "hand-fonts")]
fn multiline_text_height_is_lines_times_line_height() {
    let doc = doc("text_fonts");
    let mut ts = Typesetter::new();
    let texts = shapes(&doc, "Text");
    let three = texts.iter().find(|t| t["content"] == "line one\nline two\nline three").expect("fixture");
    let one = ts.measure(&spec_of(&serde_json::json!({
        "content": "line one", "font_size": 20.0, "font_family": "GelPen", "font_weight": "Regular",
        "style": {"stroke_color": {"r":0,"g":0,"b":0,"a":255}}, "char_colors": []
    })));
    let h = ts.measure(&spec_of(three)).height;
    assert!((h - 3.0 * one.height).abs() < 0.01);
    // The old estimate was 1.2 x size per line = 24; the font's own line height is 23.98 for GelPen.
    assert!((one.height - 24.0).abs() < 1.0, "{}", one.height);
}

#[test]
fn cjk_and_emoji_rows_lay_out_without_panicking_and_have_width() {
    let doc = doc("text_fonts");
    let mut ts = Typesetter::new();
    for t in shapes(&doc, "Text") {
        let c = t["content"].as_str().expect("content");
        if c.contains('日') || c.contains('😀') || c.contains('\t') {
            let l = ts.text(&spec_of(&t));
            assert!(l.size.width > 20.0, "{c}: {:?}", l.size);
            assert!(l.size.height >= 20.0);
        }
    }
}

#[test]
fn char_colour_fixture_gives_the_expected_colour_runs() {
    let doc = doc("text_char_colors");
    let mut ts = Typesetter::new();
    let texts = shapes(&doc, "Text");
    let rainbow = texts.iter().find(|t| t["content"] == "Rainbow text").expect("rainbow");
    let l = ts.text(&spec_of(rainbow));
    let distinct: std::collections::BTreeSet<_> = l.runs.iter().map(|r| r.color).collect();
    // 12 chars: the old file colours the first 7 with 7 colours, the rest default.
    let coloured = rainbow["char_colors"].as_array().expect("cc").iter().filter(|c| !c.is_null()).count();
    assert!(distinct.len() >= coloured.min(7), "{distinct:?}");
    // `char_colors` shorter or longer than the text: extra entries are ignored, missing ones are default.
    let short = texts.iter().find(|t| t["content"] == "short colors").expect("short");
    let l = ts.text(&spec_of(short));
    assert_eq!(l.glyph_count(), "short colors".len());
    let long = texts.iter().find(|t| t["content"] == "long").expect("long");
    assert!(ts.text(&spec_of(long)).glyph_count() == 4);
}

/// Equations at x = 0 in `math`: the ink of glyphs and rules must match the old render. The old
/// export fits bounds [0, -20, 800, 714], so a shape at (0, y) has its baseline at pixel row y + 40.
#[test]
fn math_ink_matches_the_old_render() {
    let doc = doc("math");
    let png = read_png(&fixture("render/math.png"));
    let mut ts = Typesetter::new();
    let rows: Vec<Value> =
        shapes(&doc, "Math").into_iter().filter(|m| m["position"]["x"].as_f64() == Some(0.0)).collect();
    let mut compared = 0;
    for m in rows {
        let latex = m["latex"].as_str().expect("latex");
        let size = m["font_size"].as_f64().expect("size");
        let y = m["position"]["y"].as_f64().expect("y");
        let Ok(layout) = ts.math(&MathSpec::new(latex).with_size(size)) else {
            continue; // error rows are checked in math.rs
        };
        if layout.glyphs.is_empty() && layout.rules.is_empty() {
            continue;
        }
        let paths = layout.outline(Rgba8::BLACK);
        let ink = ink_bbox(&paths).expect("ink");
        let base_row = y + 40.0;
        let found =
            png.ink(20, (base_row - 50.0) as usize, 510, (base_row + 30.0) as usize).expect("old render has ink");
        let (ox0, ox1) = (found.0 as f64 - 20.0, found.2 as f64 + 1.0 - 20.0);
        let (oy0, oy1) = (found.1 as f64 - base_row, found.3 as f64 + 1.0 - base_row);
        let tol = 1.5;
        if latex.contains(['^', '_']) {
            // The old render drew every script in STIX Two Math at 0.7 of the math size, next to a base in GelPen,
            // so a script was nearly as big as its base. Scripts are now drawn in the text font at the same
            // 0.7 ratio (`superscript_is_smaller_and_raised` in math.rs). The ink must therefore lie inside the
            // old ink and still cover most of it.
            assert!(
                ink.x0 >= ox0 - tol && ink.x1 <= ox1 + tol && ink.y0 >= oy0 - tol && ink.y1 <= oy1 + tol,
                "{latex}: new ink {ink:?} must lie inside old x {ox0}..{ox1} y {oy0}..{oy1}"
            );
            assert!(
                ink.width() > 0.85 * (ox1 - ox0) && ink.height() > 0.75 * (oy1 - oy0),
                "{latex}: new ink {ink:?} covers too little of old x {ox0}..{ox1} y {oy0}..{oy1}"
            );
        } else {
            assert!(
                (ink.x0 - ox0).abs() < tol && (ink.x1 - ox1).abs() < tol,
                "{latex}: x new {}..{} old {ox0}..{ox1}",
                ink.x0,
                ink.x1
            );
            assert!(
                (ink.y0 - oy0).abs() < tol && (ink.y1 - oy1).abs() < tol,
                "{latex}: y new {}..{} old {oy0}..{oy1}",
                ink.y0,
                ink.y1
            );
        }
        compared += 1;
    }
    assert!(compared >= 6, "compared {compared} equations");
}
