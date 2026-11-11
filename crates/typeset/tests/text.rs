//! Text layout: determinism, fonts, styles, spans, outlines.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_typeset::*;

#[test]
fn layout_is_deterministic_across_typesetters() {
    let spec = {
        let mut s = plain("The quick brown fox\njumps over the lazy dog", 22.0);
        s.spans.push(Span { range: 4..9, style: SpanStyle::color(Rgba8::rgb(200, 0, 0)) });
        s
    };
    let a = Typesetter::new().text(&spec);
    let b = Typesetter::new().text(&spec);
    assert_eq!(a.size, b.size);
    assert_eq!(a.lines, b.lines);
    assert_eq!(a.runs.len(), b.runs.len());
    for (x, y) in a.runs.iter().zip(&b.runs) {
        assert_eq!(x.glyphs, y.glyphs);
        assert_eq!(x.color, y.color);
        assert_eq!(x.style, y.style);
    }
}

#[test]
fn same_spec_after_clearing_caches_gives_equal_layout_new_id() {
    let mut ts = Typesetter::new();
    let spec = plain("hello", 20.0);
    let a = ts.text(&spec);
    let again = ts.text(&spec);
    assert_eq!(a.id, again.id, "a cache hit returns the same layout");
    ts.clear_caches();
    let b = ts.text(&spec);
    assert_ne!(a.id, b.id);
    assert_eq!(a.size, b.size);
    assert_eq!(a.runs[0].glyphs, b.runs[0].glyphs);
}

#[test]
fn width_grows_with_font_size_and_height_with_lines() {
    let mut ts = Typesetter::new();
    let s20 = ts.measure(&plain("Hello", 20.0));
    let s40 = ts.measure(&plain("Hello", 40.0));
    assert!((s40.width / s20.width - 2.0).abs() < 0.02, "{s20:?} {s40:?}");
    let two = ts.measure(&plain("Hello\nHello", 20.0));
    assert!((two.height / s20.height - 2.0).abs() < 0.01);
    assert!((two.width - s20.width).abs() < 0.01);
}

#[test]
fn line_height_option_changes_height() {
    let mut ts = Typesetter::new();
    let mut a = plain("a\nb\nc", 20.0);
    let normal = ts.measure(&a).height;
    a.line_height = LineHeight::FontSize(1.25);
    let h = ts.measure(&a).height;
    assert!((h - 3.0 * 25.0).abs() < 0.01, "{h}");
    assert!((normal - h).abs() > 1.0);
}

#[test]
fn empty_text_has_one_line_height_and_no_width() {
    let mut ts = Typesetter::new();
    let l = ts.text(&plain("", 20.0));
    assert_eq!(l.lines.len(), 1);
    assert_eq!(l.size.width, 0.0);
    assert!(l.size.height > 15.0);
    assert!(l.runs.is_empty());
    assert_eq!(l.text_len, 0);
}

#[test]
fn trailing_newline_adds_an_empty_line() {
    let mut ts = Typesetter::new();
    let one = ts.text(&plain("abc", 20.0));
    let two = ts.text(&plain("abc\n", 20.0));
    assert_eq!(two.lines.len(), 2);
    assert!((two.size.height - 2.0 * one.size.height).abs() < 0.01);
}

#[test]
fn every_bundled_family_and_weight_picks_its_own_face() {
    let mut ts = Typesetter::new();
    let mut seen = std::collections::BTreeSet::new();
    for family in [FontFamily::GelPen, FontFamily::GelPenSerif] {
        for weight in [FontWeight::Light, FontWeight::Regular, FontWeight::Heavy] {
            let mut s = plain("Sphinx", 24.0);
            s.style.family = family.clone();
            s.style.weight = weight;
            let l = ts.text(&s);
            let font = &l.runs[0].font;
            assert!(seen.insert(font.data.as_ref().len()), "{family:?} {weight:?} reused a face");
        }
    }
    for family in [FontFamily::VanillaExtract, FontFamily::NotoSans] {
        let mut s = plain("Sphinx", 24.0);
        s.style.family = family.clone();
        let l = ts.text(&s);
        assert!(seen.insert(l.runs[0].font.data.as_ref().len()), "{family:?}");
    }
    assert_eq!(seen.len(), 8);
}

#[test]
fn legacy_style_mapping_matches_the_old_table() {
    let noto_light = TextStyle::from_legacy(FontFamily::NotoSans, FontWeight::Light, 24.0, Rgba8::BLACK);
    assert!(noto_light.italic, "old app drew Noto Sans Light as italic");
    let noto_heavy = TextStyle::from_legacy(FontFamily::NotoSans, FontWeight::Heavy, 24.0, Rgba8::BLACK);
    assert_eq!(noto_heavy.weight, FontWeight::Heavy);
    let vanilla = TextStyle::from_legacy(FontFamily::VanillaExtract, FontWeight::Heavy, 24.0, Rgba8::BLACK);
    assert_eq!(vanilla.weight, FontWeight::Regular, "old app ignored the weight for Vanilla Extract");
    let mut ts = Typesetter::new();
    let mut a = TextSpec::styled("Sphinx", noto_light);
    let mut b = a.clone();
    b.style.italic = false;
    a.style.family = FontFamily::NotoSans;
    // Italic and upright use different font files, and neither needs a synthetic slant.
    let (la, lb) = (ts.text(&a), ts.text(&b));
    assert_ne!(la.runs[0].font.data.as_ref().len(), lb.runs[0].font.data.as_ref().len());
    assert!(la.runs[0].skew.is_none());
}

#[test]
fn italic_without_an_italic_face_is_synthesised() {
    let mut ts = Typesetter::new();
    let mut s = plain("Slanted", 24.0);
    s.style.italic = true;
    let l = ts.text(&s);
    let skew = l.runs[0].skew.expect("GelPen has no italic, so the layout asks for a slant");
    assert!(skew > 0.1 && skew < 0.5, "{skew}");
}

#[test]
fn char_colours_become_runs_with_matching_colours() {
    let text = "red green";
    let red = Rgba8::rgb(220, 38, 38);
    let green = Rgba8::rgb(22, 163, 74);
    let mut colors = vec![None; 9];
    colors[0..3].fill(Some(red));
    colors[4..9].fill(Some(green));
    let spans = Span::from_char_colors(text, &colors);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].range, 0..3);
    assert_eq!(spans[1].range, 4..9);
    let mut spec = plain(text, 20.0);
    spec.spans = spans;
    let l = Typesetter::new().text(&spec);
    let colors: Vec<(Rgba8, usize)> = l.runs.iter().map(|r| (r.color, r.glyphs.len())).collect();
    assert_eq!(colors, vec![(red, 3), (Rgba8::BLACK, 1), (green, 5)], "{colors:?}");
    // Colour does not move glyphs.
    let plain_l = Typesetter::new().text(&plain(text, 20.0));
    assert_eq!(l.size, plain_l.size);
}

#[test]
fn char_colours_use_char_indices_not_bytes() {
    let text = "日本語 x";
    let colors = vec![Some(Rgba8::rgb(1, 2, 3)), None, Some(Rgba8::rgb(1, 2, 3))];
    let spans = Span::from_char_colors(text, &colors);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].range, 0..3);
    assert_eq!(spans[1].range, 6..9);
}

#[test]
fn later_spans_win_and_spans_compose() {
    let mut spec = plain("abcdef", 20.0);
    spec.spans.push(Span { range: 0..6, style: SpanStyle::color(Rgba8::rgb(255, 0, 0)) });
    spec.spans.push(Span {
        range: 2..4,
        style: SpanStyle {
            color: Some(Rgba8::rgb(0, 0, 255)),
            weight: Some(FontWeight::Heavy),
            ..SpanStyle::default()
        },
    });
    let l = Typesetter::new().text(&spec);
    let seen: Vec<_> = l.runs.iter().map(|r| (r.glyphs.len(), r.color, l.styles[r.style as usize].weight)).collect();
    assert_eq!(
        seen,
        vec![
            (2, Rgba8::rgb(255, 0, 0), FontWeight::Regular),
            (2, Rgba8::rgb(0, 0, 255), FontWeight::Heavy),
            (2, Rgba8::rgb(255, 0, 0), FontWeight::Regular),
        ]
    );
}

#[test]
fn per_span_size_changes_line_metrics() {
    let mut spec = plain("small BIG", 16.0);
    spec.spans.push(Span { range: 6..9, style: SpanStyle { size: Some(48.0), ..SpanStyle::default() } });
    let l = Typesetter::new().text(&spec);
    let base = Typesetter::new().text(&plain("small BIG", 16.0));
    assert!(l.size.height > base.size.height * 2.0);
    assert!(l.runs.iter().any(|r| r.font_size == 48.0));
}

#[test]
fn underline_sits_below_the_baseline_and_strike_above() {
    let mut s = plain("under", 24.0);
    s.style.underline = true;
    s.style.strike = true;
    let l = Typesetter::new().text(&s);
    let base = f64::from(l.first_baseline());
    let under = l.decorations.iter().find(|d| d.kind == DecorationKind::Underline).expect("underline");
    let strike = l.decorations.iter().find(|d| d.kind == DecorationKind::Strike).expect("strike");
    assert!(under.rect.y0 > base);
    assert!(strike.rect.y1 < base && strike.rect.y0 < strike.rect.y1);
    assert!((under.rect.x1 - under.rect.x0 - l.size.width).abs() < 0.5);
}

#[test]
fn wrapping_respects_the_width_and_never_splits_short_words() {
    let mut ts = Typesetter::new();
    let text = "The quick brown fox jumps over the lazy dog";
    let l = ts.text(&plain(text, 20.0).wrapped(120.0));
    assert!(l.lines.len() > 2);
    assert!(l.size.width <= 120.01, "{}", l.size.width);
    assert_eq!(l.box_size.width, 120.0);
    for line in &l.lines {
        let t = &text[line.text_range.clone()];
        assert!(!t.trim().is_empty());
        assert!(!t.trim().starts_with(' '));
    }
    // A word wider than the box is split rather than overflowing.
    let long = ts.text(&plain("Supercalifragilisticexpialidocious", 20.0).wrapped(60.0));
    assert!(long.lines.len() > 1);
    assert!(long.size.width <= 60.01);
}

#[test]
fn alignment_moves_lines_inside_the_wrap_box() {
    let mut ts = Typesetter::new();
    let text = "aaaa bbbb\ncc";
    let left = ts.text(&plain(text, 20.0).wrapped(200.0));
    let centre = ts.text(&plain(text, 20.0).wrapped(200.0).aligned(Align::Center));
    let right = ts.text(&plain(text, 20.0).wrapped(200.0).aligned(Align::Right));
    assert_eq!(left.lines[1].x, 0.0);
    let w = left.lines[1].width;
    assert!((centre.lines[1].x - (200.0 - w) / 2.0).abs() < 0.5, "{:?}", centre.lines[1]);
    assert!((right.lines[1].x - (200.0 - w)).abs() < 0.5, "{:?}", right.lines[1]);
    let first = &right.runs[0];
    assert!(first.x > 100.0);
}

#[test]
fn outline_has_ink_inside_the_layout_box() {
    let mut ts = Typesetter::new();
    let l = ts.text(&plain("Hello, World", 32.0));
    let paths = l.outline();
    assert_eq!(paths.len(), 1);
    let bb = ink_bbox(&paths).expect("ink");
    assert!(bb.x0 > -4.0 && bb.x1 < l.size.width + 4.0, "{bb:?} {:?}", l.size);
    assert!(bb.y0 > -2.0 && bb.y1 < l.size.height + 8.0, "{bb:?} {:?}", l.size);
    assert!(bb.width() > l.size.width * 0.8);
    // Colours group paths in paint order.
    let mut s = plain("ab", 32.0);
    s.spans.push(Span { range: 1..2, style: SpanStyle::color(Rgba8::rgb(9, 9, 9)) });
    let two = ts.text(&s).outline();
    assert_eq!(two.len(), 2);
    assert_eq!(two[1].color, Rgba8::rgb(9, 9, 9));
}

#[test]
fn hit_test_uses_line_boxes() {
    let mut ts = Typesetter::new();
    let l = ts.text(&plain("a very long first line\nb", 20.0));
    assert!(l.hit_test(kurbo::Point::new(5.0, 5.0), 0.0));
    let second = &l.lines[1];
    // To the right of the short second line is empty space.
    assert!(!l.hit_test(kurbo::Point::new(l.size.width - 2.0, f64::from(second.top + second.bottom) / 2.0), 0.0));
    assert!(l.hit_test(kurbo::Point::new(l.size.width - 2.0, 5.0), 0.0));
    assert!(!l.hit_test(kurbo::Point::new(-10.0, 5.0), 0.0));
    assert!(l.hit_test(kurbo::Point::new(-10.0, 5.0), 11.0));
}

#[test]
fn custom_font_registration_changes_the_font_set_version() {
    let mut ts = Typesetter::new();
    let v0 = ts.font_set_version();
    let spec = plain("x", 20.0);
    let _ = ts.text(&spec);
    assert_eq!(ts.stats().text_entries, 1);
    assert!(matches!(ts.register_font(vec![1, 2, 3]), Err(FontError::Invalid(_))));
    assert_eq!(ts.font_set_version(), v0, "a failed registration changes nothing");
    let bytes =
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/NotoSans-Regular.ttf"))
            .expect("font file");
    let names = ts.register_font(bytes).expect("valid font");
    assert!(names.iter().any(|n| n == "Noto Sans"), "{names:?}");
    assert_eq!(ts.font_set_version(), v0 + 1);
    assert_eq!(ts.stats().text_entries, 0, "the text cache is cleared");
}

fn font_bytes(file: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts").join(file)).expect("font file")
}

#[test]
fn a_typesetter_without_the_extra_fonts_asks_for_noto_sans_when_a_glyph_is_missing() {
    let mut ts = Typesetter::without_extra_fonts();
    assert!(ts.wanted_fonts().is_empty(), "nothing is wanted before a layout needs it");
    // GelPen has no Cyrillic letters.
    let spec = plain("Привет", 24.0);
    let before = ts.text(&spec);
    assert_eq!(ts.wanted_fonts(), ExtraFont::NOTO_SANS);

    let version = ts.font_set_version();
    for font in ExtraFont::NOTO_SANS {
        ts.provide_font(font, font_bytes(font.file_name())).expect("a valid font");
    }
    assert!(ts.wanted_fonts().is_empty(), "provided fonts are no longer wanted");
    assert!(ts.font_set_version() > version, "layouts made without the font are not served again");
    let after = ts.text(&spec);
    assert_ne!(before.id, after.id);
    assert!(after.runs.iter().flat_map(|r| &r.glyphs).all(|g| g.id != 0), "every letter has a glyph now");
}

#[test]
fn text_in_the_noto_sans_family_wants_the_font_even_when_no_glyph_is_missing() {
    let mut ts = Typesetter::without_extra_fonts();
    let mut spec = plain("hello", 20.0);
    spec.style.family = FontFamily::NotoSans;
    let _ = ts.text(&spec);
    assert_eq!(ts.wanted_fonts(), ExtraFont::NOTO_SANS);
}

#[test]
fn plain_gelpen_text_wants_nothing() {
    let mut ts = Typesetter::without_extra_fonts();
    let _ = ts.text(&plain("hello world", 20.0));
    assert!(ts.wanted_fonts().is_empty());
}

#[test]
fn providing_a_font_twice_or_a_broken_one_changes_nothing_more() {
    let mut ts = Typesetter::without_extra_fonts();
    let font = ExtraFont::NotoSansBold;
    let v0 = ts.font_set_version();
    assert!(matches!(ts.provide_font(font, vec![1, 2, 3]), Err(FontError::Invalid(_))));
    assert_eq!(ts.font_set_version(), v0);
    ts.provide_font(font, font_bytes(font.file_name())).expect("valid");
    let v1 = ts.font_set_version();
    ts.provide_font(font, font_bytes(font.file_name())).expect("a second time is fine");
    assert_eq!(ts.font_set_version(), v1);
}

#[test]
fn the_bundled_font_files_are_exposed_once_for_every_host() {
    use FontFamily::{GelPen, GelPenSerif, NotoSans, VanillaExtract};
    use FontWeight::{Heavy, Light, Regular};
    let mut seen = Vec::new();
    for (family, weight) in [
        (GelPen, Light),
        (GelPen, Regular),
        (GelPen, Heavy),
        (GelPenSerif, Light),
        (GelPenSerif, Regular),
        (GelPenSerif, Heavy),
    ] {
        let bytes = font_file(&family, weight).unwrap_or_default();
        assert!(bytes.len() > 1000, "{family:?} {weight:?} has a font file");
        assert!(!seen.contains(&bytes.as_ptr()), "{family:?} {weight:?} is its own file");
        seen.push(bytes.as_ptr());
    }
    let vanilla = font_file(&VanillaExtract, Regular).unwrap_or_default();
    assert!(vanilla.len() > 1000);
    assert_eq!(
        font_file(&VanillaExtract, Heavy).map(<[u8]>::as_ptr),
        Some(vanilla.as_ptr()),
        "one face for every weight"
    );
    assert!(font_file(&FontFamily::Custom("Mine".into()), Regular).is_none(), "a custom family has no bundled file");
    // Noto Sans is a built-in extra font on this (embedded) test build; its bold face is a different file.
    let regular = font_file(&NotoSans, Regular).unwrap_or_default();
    let bold = font_file(&NotoSans, Heavy).unwrap_or_default();
    assert!(!regular.is_empty() && !bold.is_empty() && regular.as_ptr() != bold.as_ptr());
}

fn plex(text: &str, weight: FontWeight) -> TextSpec {
    let mut s = plain(text, 24.0);
    s.style.family = FontFamily::PlexSans;
    s.style.weight = weight;
    s
}

#[test]
fn plex_sans_lays_out_with_its_own_metrics_and_real_weights() {
    let mut ts = Typesetter::new();
    let text = "Handgloves and hamburgefonts";
    let regular = ts.text(&plex(text, FontWeight::Regular));
    let heavy = ts.text(&plex(text, FontWeight::Heavy));
    let light = ts.text(&plex(text, FontWeight::Light));
    let gelpen = ts.text(&plain(text, 24.0));
    assert_ne!(regular.size.width, gelpen.size.width, "Plex Sans is not GelPen");
    assert!(heavy.size.width > regular.size.width, "bold is wider than regular: {:?}", (heavy.size, regular.size));
    assert!(light.size.width < regular.size.width, "light is narrower than regular: {:?}", (light.size, regular.size));
    for l in [&regular, &heavy, &light] {
        assert!(l.runs.iter().flat_map(|r| &r.glyphs).all(|g| g.id != 0), "every letter has a Plex glyph");
        assert!(!l.runs.iter().any(|r| r.embolden), "the weight comes from the font, not from synthetic bold");
    }
    // The three weights are three settings of one file, so the runs carry different variation coordinates.
    let coords = |l: &TextLayout| l.runs[0].normalized_coords.clone();
    assert_ne!(coords(&regular), coords(&heavy));
    assert_ne!(coords(&regular), coords(&light));
    assert!(std::ptr::eq(
        font_file(&FontFamily::PlexSans, FontWeight::Regular).unwrap_or_default(),
        font_file(&FontFamily::PlexSans, FontWeight::Heavy).unwrap_or_default()
    ));
}

#[test]
fn plex_sans_outlines_follow_the_weight() {
    let mut ts = Typesetter::new();
    let mut ink = |w| {
        let l = ts.text(&plex("Hxl", w));
        let area: f64 = l.outline().iter().map(|p| kurbo::Shape::area(&p.path).abs()).sum();
        area
    };
    let (regular, heavy) = (ink(FontWeight::Regular), ink(FontWeight::Heavy));
    assert!(heavy > regular * 1.1, "the bold outline has more ink: {regular} -> {heavy}");
}

#[test]
fn a_typesetter_without_the_extra_fonts_asks_for_plex_sans_only_when_the_family_is_used() {
    let mut ts = Typesetter::without_extra_fonts();
    let _ = ts.text(&plain("hello", 20.0));
    assert!(ts.wanted_fonts().is_empty());
    let before = ts.text(&plex("hello", FontWeight::Regular));
    assert_eq!(ts.wanted_fonts(), [ExtraFont::PlexSans]);
    // A span in the family asks too.
    let mut ts = Typesetter::without_extra_fonts();
    let mut spec = plain("hello", 20.0);
    spec.spans
        .push(Span { range: 0..2, style: SpanStyle { family: Some(FontFamily::PlexSans), ..SpanStyle::default() } });
    let _ = ts.text(&spec);
    assert_eq!(ts.wanted_fonts(), [ExtraFont::PlexSans]);

    let mut ts = Typesetter::without_extra_fonts();
    let _ = ts.text(&plex("hello", FontWeight::Regular));
    ts.provide_font(ExtraFont::PlexSans, font_bytes(ExtraFont::PlexSans.file_name())).expect("a valid font");
    assert!(ts.wanted_fonts().is_empty());
    let after = ts.text(&plex("hello", FontWeight::Regular));
    assert_ne!(before.size.width, after.size.width, "the real face is narrower or wider than the fallback");
}
