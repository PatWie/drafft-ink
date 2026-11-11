#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::field_reassign_with_default)]
//! Native format, sniffing, PNG metadata and SVG export.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use drafftink_io::measure::EstimateMeasure;
use drafftink_io::native::{self, Format};
use drafftink_io::png_meta::{self, KEYWORD};
use drafftink_io::svg::{self, FontFace, MathSvg, SvgOptions, SvgStyle, export_svg};
use drafftink_scene::{
    BindMode, Binding, Color, Document, Element, ElementId, ElementKind, FileEntry, FontFamily, FontWeight, FrameData,
    FreehandData, Head, ImageData, LinearData, MathData, TextData, TextRun, Transform, ZIndex,
};
use kurbo::Point;
use proptest::prelude::*;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn eid(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&vec![200u8; (w * h * 4) as usize]).unwrap();
    }
    out
}

/// One of every kind, with groups, a frame, bound text, bindings and an image.
fn sample() -> Document {
    let mut d = Document::new();
    d.name = "sample".into();
    let mut keys = ZIndex::rebalance(12).unwrap().into_iter();
    let mut push = |mut e: Element, d: &mut Document| {
        e.index = keys.next().unwrap();
        d.elements.push(e);
    };
    let mut frame = Element::new(
        eid("frame"),
        ElementKind::Frame(FrameData { name: Some("A <&> frame".into()), clip: true }),
        ZIndex::first(),
    )
    .with_bounds(-20.0, -20.0, 400.0, 300.0);
    frame.style.stroke_color = Color::rgb(0x11, 0x22, 0x33);
    push(frame, &mut d);
    let mut rect =
        Element::new(eid("rect"), ElementKind::Rectangle, ZIndex::first()).with_bounds(0.0, 0.0, 100.0, 60.0);
    rect.style.fill_color = Some(Color::rgba(255, 0, 0, 128));
    rect.style.fill_pattern = drafftink_scene::FillPattern::Hachure;
    rect.style.corner_radius = 8.0;
    rect.style.seed = 5;
    rect.transform.angle = 0.3;
    rect.frame_id = Some(eid("frame"));
    rect.group_ids.push(drafftink_scene::GroupId::new("g1").unwrap());
    push(rect, &mut d);
    let mut ell = Element::new(eid("ell"), ElementKind::Ellipse, ZIndex::first()).with_bounds(150.0, 0.0, 60.0, 60.0);
    ell.style.opacity = 0.5;
    ell.group_ids.push(drafftink_scene::GroupId::new("g1").unwrap());
    push(ell, &mut d);
    push(Element::new(eid("dia"), ElementKind::Diamond, ZIndex::first()).with_bounds(0.0, 100.0, 80.0, 80.0), &mut d);
    let mut arrow = Element::linear(
        eid("arrow"),
        drafftink_scene::LinearKind::Arrow,
        ZIndex::first(),
        &[Point::new(100.0, 30.0), Point::new(150.0, 30.0)],
    )
    .unwrap();
    if let ElementKind::Arrow(a) = &mut arrow.kind {
        a.end_head = Head::Triangle;
        a.start_head = Head::Circle;
        a.end_binding = Some(Binding { target: eid("ell"), mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
    }
    push(arrow, &mut d);
    let mut line = Element::linear(
        eid("line"),
        drafftink_scene::LinearKind::Line,
        ZIndex::first(),
        &[Point::new(0.0, 200.0), Point::new(50.0, 240.0), Point::new(100.0, 200.0)],
    )
    .unwrap();
    if let ElementKind::Line(l) = &mut line.kind {
        l.path = drafftink_scene::PathType::Round;
    }
    line.style.stroke_style = drafftink_scene::StrokeStyle::Dashed;
    push(line, &mut d);
    let mut fh = Element::new(
        eid("pen"),
        ElementKind::Freehand(FreehandData::new(
            (0..20).map(|i| Point::new(f64::from(i) * 5.0, f64::from(i % 3) * 4.0)).collect(),
        )),
        ZIndex::first(),
    );
    fh.refit_points().unwrap();
    fh.transform.x = 200.0;
    fh.transform.y = 200.0;
    push(fh, &mut d);
    let mut t = TextData::new("Hello <b>&\nsecond line");
    t.runs = vec![TextRun { start: 1, end: 3, color: Some(Color::rgb(0, 128, 0)), ..Default::default() }];
    t.font.family = FontFamily::GelPenSerif;
    let mut text =
        Element::new(eid("text"), ElementKind::Text(t), ZIndex::first()).with_bounds(0.0, 300.0, 140.0, 50.0);
    text.style.stroke_color = Color::rgb(10, 20, 30);
    push(text, &mut d);
    let mut label = TextData::new("label");
    label.container_id = Some(eid("rect"));
    push(
        Element::new(eid("label"), ElementKind::Text(label), ZIndex::first()).with_bounds(20.0, 20.0, 40.0, 20.0),
        &mut d,
    );
    push(
        Element::new(eid("math"), ElementKind::Math(MathData::new("\\frac{a}{b} < 1 & x")), ZIndex::first())
            .with_bounds(220.0, 300.0, 60.0, 30.0),
        &mut d,
    );
    let (fid, entry) = FileEntry::new("image/png", png_bytes(4, 3));
    d.files.insert(fid.clone(), entry);
    let mut img = Element::new(
        eid("img"),
        ElementKind::Image(ImageData { file_id: Some(fid), natural_width: 4, natural_height: 3, crop: None }),
        ZIndex::first(),
    )
    .with_bounds(300.0, 100.0, 40.0, 30.0);
    img.transform.flip_x = true;
    push(img, &mut d);
    let mut missing = Element::new(
        eid("gone"),
        ElementKind::Image(ImageData {
            file_id: Some(drafftink_scene::FileId::of(b"x")),
            natural_width: 1,
            natural_height: 1,
            crop: None,
        }),
        ZIndex::first(),
    )
    .with_bounds(300.0, 150.0, 40.0, 30.0);
    missing.deleted = false;
    push(missing, &mut d);
    d
}

#[test]
fn native_save_load_round_trip_and_errors() {
    let d = sample();
    // `sample` has a missing blob (a warning, not an error), so it saves.
    let text = native::save(&d).unwrap();
    assert_eq!(native::sniff(text.as_bytes()), Format::Native);
    assert_eq!(native::load(&text).unwrap(), d);
    assert_eq!(native::load(&native::save_pretty(&d).unwrap()).unwrap(), d);
    assert_eq!(native::load(&format!("\u{feff}{text}")).unwrap(), d, "a BOM is tolerated");
    assert!(
        native::load("{\"format\":\"drafftink\",\"version\":99}").unwrap_err().to_string().contains("Update the app")
    );
    assert!(native::load("{\"format\":\"other\"}").is_err());
    assert!(native::load("[1,2]").is_err());
    assert!(native::load("").is_err());
}

#[test]
fn sniff_tells_every_format_apart() {
    let fx = fixtures();
    let read = |p: &str| std::fs::read(fx.join(p)).unwrap();
    assert_eq!(native::sniff(&read("legacy/all_shapes.json")), Format::LegacyJson);
    assert_eq!(native::sniff(&read("legacy/large_1000.json.gz")), Format::Gzip);
    assert_eq!(native::sniff(&read("legacy/crdt/images.loro")), Format::LegacySnapshot);
    assert_eq!(native::sniff(&read("excalidraw/basic_shapes.excalidraw")), Format::ExcalidrawScene);
    assert_eq!(native::sniff(&read("excalidraw/v1_library.excalidrawlib")), Format::ExcalidrawLibrary);
    assert_eq!(native::sniff(&read("excalidraw/clipboard/basic.json")), Format::ExcalidrawClipboard);
    // A scene saved with the clipboard name is a scene: the type field decides.
    assert_eq!(native::sniff(&read("excalidraw/clipboard/not_clipboard.json")), Format::ExcalidrawScene);
    assert_eq!(native::sniff(&read("excalidraw/not_json.excalidraw")), Format::Unknown);
    assert_eq!(native::sniff(&read("mermaid/flowchart_comment_before_header.mmd")), Format::Mermaid);
    assert_eq!(native::sniff(&read("mermaid/flowchart_fenced.mmd")), Format::Mermaid);
    assert_eq!(native::sniff(&read("mermaid/sequence_basic.mmd")), Format::Mermaid);
    assert_eq!(native::sniff(&read("render/README.md")), Format::Unknown);
    assert_eq!(native::sniff(&png_bytes(2, 2)), Format::Png);
    assert_eq!(native::sniff(b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>"), Format::Svg);
    for junk in [&b""[..], b"\xff\xfe\x00", b"{", b"[", b"<", b"\x89PNG", b"loro", b"   ", b"{\"a\":1}"] {
        let _ = native::sniff(junk);
    }
    // Other diagram families and prose are not Mermaid input for this crate. (An empty `graph` is still a graph:
    // the parser, not the sniffer, decides that it has no content.)
    for name in ["class_diagram", "er_diagram", "gantt", "pie", "prose", "state_diagram", "empty"] {
        let b = read(&format!("mermaid/rejected/{name}.mmd"));
        assert_ne!(native::sniff(&b), Format::Mermaid, "{name}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// PNG metadata
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn png_scene_round_trip_with_unicode() {
    let mut d = sample();
    d.name = "日本語 ✓ — “curly”".into();
    d.elements[7].kind = ElementKind::Text(TextData::new("emoji 🎨 and 漢字 and ü"));
    let json = d.to_json().unwrap();
    let png = png_bytes(8, 8);
    let with = png_meta::embed_scene(&png, &json).unwrap();
    assert_eq!(png_meta::extract_scene(&png).unwrap(), None);
    let back = png_meta::extract_scene(&with).unwrap().unwrap();
    assert_eq!(back, json);
    assert_eq!(native::load(&back).unwrap(), d);
    // The result is still a PNG that a decoder accepts, and the pixels are untouched.
    let mut dec = png::Decoder::new(std::io::Cursor::new(&with)).read_info().unwrap();
    let mut buf = vec![0; dec.output_buffer_size().unwrap()];
    dec.next_frame(&mut buf).unwrap();
    assert!(buf.iter().all(|b| *b == 200));
    // Embedding twice replaces; there is exactly one chunk.
    let again = png_meta::embed_scene(&with, "{\"second\":true}").unwrap();
    assert_eq!(png_meta::extract_scene(&again).unwrap().as_deref(), Some("{\"second\":true}"));
    assert_eq!(again.windows(KEYWORD.len()).filter(|w| *w == KEYWORD.as_bytes()).count(), 1);
}

#[test]
fn png_reads_the_old_apps_ztxt_chunk() {
    let old = std::fs::read_to_string(fixtures().join("legacy/all_shapes.json")).unwrap();
    // The old app stored pretty JSON in a Latin-1 zTXt chunk. This fixture is ASCII, as the old limits required.
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 2, 2);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.add_ztxt_chunk(KEYWORD.to_owned(), old.clone()).unwrap();
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[0, 0, 0, 0]).unwrap();
    }
    let text = png_meta::extract_scene(&out).unwrap().unwrap();
    assert_eq!(text, old);
    assert_eq!(native::sniff(text.as_bytes()), Format::LegacyJson);
    // Other keywords are ignored.
    let mut other = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut other, 1, 1);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.add_text_chunk("Comment".to_owned(), "hi".to_owned()).unwrap();
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[0]).unwrap();
    }
    assert_eq!(png_meta::extract_scene(&other).unwrap(), None);
}

#[test]
fn png_hostile_input_is_an_error_or_none() {
    let good = png_meta::embed_scene(&png_bytes(4, 4), "{}").unwrap();
    assert!(png_meta::extract_scene(b"not a png").is_err());
    assert!(png_meta::embed_scene(b"not a png", "{}").is_err());
    assert!(png_meta::embed_scene(&good[..8], "{}").is_err(), "signature only: no IDAT/IEND to place the chunk before");
    for cut in 0..good.len() {
        let _ = png_meta::extract_scene(&good[..cut]);
        let _ = png_meta::embed_scene(&good[..cut], "{}");
    }
    // A chunk that claims 4 GiB.
    let mut huge = good[..8].to_vec();
    huge.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
    huge.extend_from_slice(b"iTXt");
    huge.extend_from_slice(&[0; 32]);
    assert!(png_meta::extract_scene(&huge).is_err());
    for i in 8..good.len() {
        let mut b = good.clone();
        b[i] ^= 0xff;
        let _ = png_meta::extract_scene(&b);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn png_extract_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512), keep_sig in any::<bool>()) {
        let mut b = Vec::new();
        if keep_sig { b.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]); }
        b.extend(bytes);
        let _ = png_meta::extract_scene(&b);
        let _ = png_meta::embed_scene(&b, "{}");
    }

    #[test]
    fn sniff_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = native::sniff(&bytes);
    }

    #[test]
    fn svg_extract_never_panics(s in "\\PC{0,400}") {
        let _ = svg::extract_scene(&s);
        let _ = svg::extract_scene(&format!("<drafftink-scene encoding=\"base64-deflate\">{s}</drafftink-scene>"));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// SVG
// ---------------------------------------------------------------------------------------------------------------

struct Dots;
impl MathSvg for Dots {
    fn math_svg(&self, latex: &str, _size: f64) -> Option<String> {
        (latex != "bad").then(|| "<circle r=\"3\" fill=\"currentColor\"/>".to_owned())
    }
}

fn parse(svg: &str) -> roxmltree::Document<'_> {
    roxmltree::Document::parse(svg).unwrap_or_else(|e| panic!("not well-formed: {e}\n{svg}"))
}

#[test]
fn svg_export_is_well_formed_and_complete() {
    let d = sample();
    let svg = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, Some(&Dots)).unwrap();
    let doc = parse(&svg);
    let root = doc.root_element();
    assert_eq!(root.tag_name().name(), "svg");
    let vb: Vec<f64> = root.attribute("viewBox").unwrap().split(' ').map(|v| v.parse().unwrap()).collect();
    assert_eq!(vb.len(), 4);
    // Frame spans -20..380, padding 20 on each side, and the frame stroke grows it a little.
    assert!(vb[0] <= -40.0 && vb[2] >= 440.0, "{vb:?}");
    for id in ["frame", "rect", "ell", "dia", "arrow", "line", "pen", "text", "label", "math", "img", "gone"] {
        assert!(doc.descendants().any(|n| n.attribute("id") == Some(&format!("e-{id}"))), "missing element {id}");
    }
    let text = doc.descendants().find(|n| n.attribute("id") == Some("e-text")).unwrap();
    let content: String = text.descendants().filter(|n| n.is_text()).filter_map(|n| n.text()).collect();
    assert_eq!(content, "Hello <b>&second line", "special characters survive escaping");
    assert!(text.descendants().any(|n| n.attribute("fill") == Some("#008000")), "the coloured run is a tspan");
    assert!(doc.descendants().any(|n| n.tag_name().name() == "image"
        && n.attribute("href").is_some_and(|h| h.starts_with("data:image/png;base64,"))));
    assert!(doc.descendants().any(|n| n.tag_name().name() == "clipPath"), "frame clip");
    assert!(svg.contains("A &lt;&amp;&gt; frame"));
    assert!(svg.contains("fill-opacity"), "alpha fills");
    assert!(svg.contains("stroke-dasharray"));
    // Rotation is a transform about the box centre.
    assert!(
        doc.descendants().any(|n| n.attribute("id") == Some("e-rect")
            && n.attribute("transform").is_some_and(|t| t.starts_with("rotate(")))
    );
}

#[test]
fn svg_is_deterministic_and_style_changes_the_geometry() {
    let d = sample();
    let a = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    let b = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    assert_eq!(a, b);
    let clean = export_svg(&d, &SvgOptions { style: SvgStyle::Clean, ..SvgOptions::default() }, &EstimateMeasure, None)
        .unwrap();
    assert_ne!(a, clean);
    parse(&clean);
    // Clean geometry draws one stroke where the sketch style draws two.
    let strokes = |s: &str| {
        let doc = parse(s);
        let n = doc.descendants().find(|n| n.attribute("id") == Some("e-ell")).unwrap();
        n.children().filter(|c| c.tag_name().name() == "path" && c.attribute("stroke-width").is_some()).count()
    };
    assert_eq!((strokes(&a), strokes(&clean)), (2, 1));
}

#[test]
fn svg_background_scale_padding_options() {
    let d = sample();
    let none =
        export_svg(&d, &SvgOptions { background: false, ..SvgOptions::default() }, &EstimateMeasure, None).unwrap();
    let with = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    let first_child = |s: &str| {
        parse(s)
            .root_element()
            .children()
            .find(|c| c.is_element() && !matches!(c.tag_name().name(), "metadata" | "defs"))
            .map(|c| c.tag_name().name().to_owned())
    };
    assert_eq!(first_child(&with).as_deref(), Some("rect"));
    assert_ne!(first_child(&none).as_deref(), Some("rect"));
    let size = |s: &str| {
        let doc = parse(s);
        let r = doc.root_element();
        let f = |a: &str| r.attribute(a).unwrap().parse::<f64>().unwrap();
        (f("width"), f("height"))
    };
    let (w1, h1) = size(&with);
    let (w2, h2) =
        size(&export_svg(&d, &SvgOptions { scale: 2.0, ..SvgOptions::default() }, &EstimateMeasure, None).unwrap());
    assert!((w2 - 2.0 * w1).abs() < 0.01 && (h2 - 2.0 * h1).abs() < 0.01);
    let (w3, _) =
        size(&export_svg(&d, &SvgOptions { padding: 100.0, ..SvgOptions::default() }, &EstimateMeasure, None).unwrap());
    assert!((w3 - (w1 + 160.0)).abs() < 0.01, "{w1} {w3}");
}

#[test]
fn svg_selection_exports_only_the_selection_and_its_labels() {
    let d = sample();
    let opts = SvgOptions { selection: Some(vec![eid("rect")]), ..SvgOptions::default() };
    let svg = export_svg(&d, &opts, &EstimateMeasure, None).unwrap();
    let doc = parse(&svg);
    let ids: HashSet<String> = doc.descendants().filter_map(|n| n.attribute("id")).map(str::to_owned).collect();
    assert!(ids.contains("e-rect") && ids.contains("e-label"), "the bound label comes along");
    assert!(!ids.contains("e-ell") && !ids.contains("e-img") && !ids.contains("e-text"));
    // The embedded scene holds only the selection and does not carry the image blob nor the frame.
    let scene = native::load(&svg::extract_scene(&svg).unwrap().unwrap()).unwrap();
    let kept: Vec<&str> = scene.elements.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(kept, vec!["rect", "label"]);
    assert!(scene.files.is_empty());
    assert!(scene.elements[0].frame_id.is_none(), "the frame is not exported, so the reference is cut");
    assert!(!svg.contains("data:image"));
    // The viewBox is far smaller than the whole document's.
    let w: f64 = doc.root_element().attribute("width").unwrap().parse().unwrap();
    assert!(w < 300.0, "{w}");
}

#[test]
fn svg_selection_cuts_dangling_bindings() {
    let d = sample();
    let opts = SvgOptions { selection: Some(vec![eid("arrow")]), ..SvgOptions::default() };
    let svg = export_svg(&d, &opts, &EstimateMeasure, None).unwrap();
    let scene = native::load(&svg::extract_scene(&svg).unwrap().unwrap()).unwrap();
    assert_eq!(scene.elements.len(), 1);
    assert!(scene.elements[0].linear_data().unwrap().end_binding.is_none());
}

#[test]
fn svg_embedded_scene_round_trips_the_whole_document() {
    let d = sample();
    let svg = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    let json = svg::extract_scene(&svg).unwrap().unwrap();
    let back = native::load(&json).unwrap();
    // The export drops nothing visible; the missing-blob image is part of the scene.
    assert_eq!(back.elements.len(), d.elements.len());
    assert_eq!(back.files, d.files);
    assert_eq!(back, d);
    let off =
        export_svg(&d, &SvgOptions { embed_scene: false, ..SvgOptions::default() }, &EstimateMeasure, None).unwrap();
    assert_eq!(svg::extract_scene(&off).unwrap(), None);
    assert_eq!(svg::extract_scene("<svg/>").unwrap(), None);
}

fn face(family: FontFamily, weight: FontWeight, data: &[u8]) -> FontFace {
    FontFace { family, weight, mime: "font/ttf".into(), data: Arc::from(data) }
}

#[test]
fn svg_fonts_are_embedded_only_when_used_and_supplied() {
    let d = sample();
    let fonts = vec![
        face(FontFamily::GelPenSerif, FontWeight::Regular, &[1, 2, 3]),
        face(FontFamily::VanillaExtract, FontWeight::Regular, &[9]),
    ];
    let svg = export_svg(&d, &SvgOptions { fonts, ..SvgOptions::default() }, &EstimateMeasure, None).unwrap();
    assert!(svg.contains("@font-face{font-family:'GelPen Serif';font-weight:400;"), "{svg}");
    assert!(svg.contains("base64,AQID"), "the bytes of the face");
    assert!(!svg.contains("Vanilla Extract'"), "an unused family is not embedded");
    parse(&svg);
}

#[test]
fn svg_embeds_one_face_per_used_weight_and_reuses_the_regular_one_for_a_weight_it_lacks() {
    let mut d = Document::new();
    let mut keys = ZIndex::rebalance(3).unwrap().into_iter();
    for (id, weight) in [("a", FontWeight::Light), ("b", FontWeight::Heavy), ("c", FontWeight::Heavy)] {
        let mut t = TextData::new("hi");
        t.font.family = FontFamily::GelPen;
        t.font.weight = weight;
        let e = Element::new(eid(id), ElementKind::Text(t), keys.next().unwrap()).with_bounds(0.0, 0.0, 20.0, 20.0);
        d.elements.push(e);
    }
    let fonts = vec![
        face(FontFamily::GelPen, FontWeight::Light, &[1]),
        face(FontFamily::GelPen, FontWeight::Regular, &[2]),
        face(FontFamily::GelPen, FontWeight::Heavy, &[3]),
    ];
    let svg = export_svg(&d, &SvgOptions { fonts, ..SvgOptions::default() }, &EstimateMeasure, None).unwrap();
    assert_eq!(svg.matches("@font-face").count(), 2, "light and heavy, once each, not regular: {svg}");
    assert!(svg.contains("font-weight:300;src:url(data:font/ttf;base64,AQ=="), "{svg}");
    assert!(svg.contains("font-weight:700;src:url(data:font/ttf;base64,Aw=="), "{svg}");
    // A weight with no face of its own is drawn with the family's regular face.
    let only_regular = vec![face(FontFamily::GelPen, FontWeight::Regular, &[2])];
    let svg =
        export_svg(&d, &SvgOptions { fonts: only_regular, ..SvgOptions::default() }, &EstimateMeasure, None).unwrap();
    assert_eq!(svg.matches("@font-face").count(), 2);
    assert!(svg.contains("font-weight:700;src:url(data:font/ttf;base64,Ag=="), "{svg}");
}

#[test]
fn svg_dark_maps_every_colour_like_the_canvas_and_leaves_the_embedded_scene_alone() {
    let d = sample();
    let light = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    let dark = export_svg(&d, &SvgOptions { dark: true, ..SvgOptions::default() }, &EstimateMeasure, None).unwrap();
    let hex = |c: Color| c.to_hex()[..7].to_owned();
    // The background (#fafafa is not the default white), a stroke colour and a text run colour.
    let background = d.background;
    for c in [background, Color::rgb(10, 20, 30), Color::rgb(0, 128, 0)] {
        assert!(light.contains(&hex(c)), "light has {c:?}");
        assert!(dark.contains(&hex(c.for_dark_theme())), "dark has the mapped {c:?}");
    }
    assert!(!dark.contains("fill=\"#0a141e\""), "the light text colour is gone");
    assert_ne!(light, dark);
    // The scene inside is the document as it was, light colours included.
    let scene = svg::extract_scene(&dark).unwrap().unwrap();
    assert_eq!(scene, svg::extract_scene(&light).unwrap().unwrap());
}

#[test]
fn svg_math_uses_the_supplied_renderer_or_falls_back_to_text() {
    let d = sample();
    let with = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, Some(&Dots)).unwrap();
    assert!(with.contains("<circle r=\"3\""));
    let without = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    assert!(without.contains("data-latex"));
    parse(&without);
}

#[test]
fn svg_nothing_to_export_is_an_error() {
    assert!(export_svg(&Document::new(), &SvgOptions::default(), &EstimateMeasure, None).is_err());
    let mut d = sample();
    for e in &mut d.elements {
        e.deleted = true;
    }
    assert!(export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).is_err());
    let opts = SvgOptions { selection: Some(vec![eid("nothing")]), ..SvgOptions::default() };
    assert!(export_svg(&sample(), &opts, &EstimateMeasure, None).is_err());
}

#[test]
fn svg_hostile_options_and_content() {
    let mut d = Document::new();
    let mut t = TextData::new("a\u{0}b\u{1}c \u{fffe} </text><script>alert(1)</script> \"'&");
    t.font.size = 12.0;
    let mut e = Element::new(eid("t"), ElementKind::Text(t), ZIndex::first()).with_bounds(0.0, 0.0, 100.0, 20.0);
    e.name = Some("\"><x".into());
    d.elements.push(e);
    let svg = export_svg(
        &d,
        &SvgOptions { padding: f64::NAN, scale: f64::INFINITY, ..SvgOptions::default() },
        &EstimateMeasure,
        None,
    )
    .unwrap();
    let doc = parse(&svg);
    assert!(!doc.descendants().any(|n| n.tag_name().name() == "script"), "text never becomes markup");
    // Huge and tiny boxes still produce finite numbers.
    let mut big = Document::new();
    big.elements
        .push(Element::new(eid("r"), ElementKind::Rectangle, ZIndex::first()).with_bounds(-9e8, -9e8, 9e8, 9e8));
    let s = export_svg(&big, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    assert!(!s.contains("NaN") && !s.contains("inf"));
    parse(&s);
}

#[test]
fn svg_linear_data_in_the_sample_is_valid_input_for_the_sketch() {
    // Guards the fixture: the arrow of `sample` has two distinct points and heads.
    let d = sample();
    let a = d.elements.iter().find(|e| e.id.as_str() == "arrow").unwrap();
    let l: &LinearData = a.linear_data().unwrap();
    assert_eq!(l.points.len(), 2);
    assert_eq!(a.transform, Transform { x: 100.0, y: 30.0, w: 50.0, h: 0.0, angle: 0.0, flip_x: false, flip_y: false });
}

#[test]
fn subset_keeps_documents_valid() {
    let d = sample();
    let keep: HashSet<ElementId> = [eid("arrow"), eid("label"), eid("img")].into_iter().collect();
    let s = native::subset(&d, &keep);
    assert_eq!(s.elements.len(), 3);
    assert!(!s.validate().has_errors(), "{:?}", s.validate().errors().collect::<Vec<_>>());
    assert_eq!(s.files.len(), 1);
    let ElementKind::Text(t) = &s.elements[1].kind else { panic!() };
    assert!(t.container_id.is_none());
}

// ---------------------------------------------------------------------------------------------------------------
// SVG with inline math, through the real typesetter
// ---------------------------------------------------------------------------------------------------------------

fn inline_math_doc(content: &str, off: bool) -> Document {
    let mut d = Document::new();
    let mut t = drafftink_scene::TextData::new(content);
    if off {
        t.font.inline_math = drafftink_scene::InlineMath::Off;
    }
    let mut e = Element::new(
        drafftink_scene::ElementId::new("t").unwrap(),
        drafftink_scene::ElementKind::Text(t),
        ZIndex::first(),
    );
    e.transform = drafftink_scene::Transform::new(10.0, 10.0, 200.0, 30.0);
    d.elements.push(e);
    d
}

fn real_measure()
-> drafftink_io::typeset_measure::TypesetterMeasure<std::cell::RefCell<&'static mut drafftink_typeset::Typesetter>> {
    let ts: &'static mut drafftink_typeset::Typesetter = Box::leak(Box::new(drafftink_typeset::Typesetter::new()));
    drafftink_io::typeset_measure::TypesetterMeasure(std::cell::RefCell::new(ts))
}

#[test]
fn svg_draws_inline_math_as_outlines_in_the_text_colour() {
    let m = real_measure();
    let d = inline_math_doc("Energy $E = mc^2$ here", false);
    let svg = export_svg(&d, &SvgOptions::default(), &m, Some(&m)).unwrap();
    let doc = parse(&svg);
    let el = doc.descendants().find(|n| n.attribute("id") == Some("e-t")).unwrap();
    assert!(!el.descendants().any(|n| n.tag_name().name() == "text"), "no <text>: the glyphs are outlines");
    let g = el.descendants().find(|n| n.attribute("data-text").is_some()).unwrap();
    assert_eq!(g.attribute("data-text"), Some("Energy $E = mc^2$ here"), "the source stays readable");
    assert!(g.attribute("color").is_some());
    let paths = g.descendants().filter(|n| n.tag_name().name() == "path").count();
    assert!(paths >= 1);
    assert!(g.descendants().all(|n| n.tag_name().name() != "path" || n.attribute("fill") == Some("currentColor")));
    // Text without a formula stays <text>, and so does a text with inline math switched off.
    for (content, off) in [("Energy here", false), ("Energy $E$ here", true), ("costs $5 and $10", false)] {
        let svg = export_svg(&inline_math_doc(content, off), &SvgOptions::default(), &m, Some(&m)).unwrap();
        assert!(svg.contains("<text "), "{content:?}");
        assert!(!svg.contains("data-text"), "{content:?}");
    }
}

#[test]
fn svg_inline_math_without_a_formula_engine_writes_the_source() {
    let d = inline_math_doc("Energy $E = mc^2$ here", false);
    let svg = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    assert!(svg.contains("Energy $E = mc^2$ here"));
}

fn plex_document() -> Document {
    let mut d = Document::new();
    let mut keys = ZIndex::rebalance(3).unwrap().into_iter();
    for (id, weight) in [("light", FontWeight::Light), ("regular", FontWeight::Regular), ("heavy", FontWeight::Heavy)] {
        let mut t = TextData::new("Plex");
        t.font.family = FontFamily::PlexSans;
        t.font.weight = weight;
        d.elements
            .push(Element::new(eid(id), ElementKind::Text(t), keys.next().unwrap()).with_bounds(0.0, 0.0, 60.0, 25.0));
    }
    d
}

#[test]
fn a_document_in_plex_sans_round_trips_the_native_file_and_names_the_family_by_its_word() {
    let d = plex_document();
    let text = native::save(&d).unwrap();
    assert!(text.contains("\"plex_sans\""), "{text}");
    assert_eq!(native::load(&text).unwrap(), d);
    assert_eq!(native::load(&native::save_pretty(&d).unwrap()).unwrap(), d);
}

#[test]
fn svg_names_plex_sans_and_embeds_its_one_variable_file_for_the_weights_used() {
    let d = plex_document();
    let fonts = vec![face(FontFamily::PlexSans, FontWeight::Regular, &[7, 7])];
    let svg = export_svg(&d, &SvgOptions { fonts, ..SvgOptions::default() }, &EstimateMeasure, None).unwrap();
    assert!(
        svg.contains("<text font-family=\"&apos;IBM Plex Sans&apos;, &apos;Helvetica Neue&apos;, Arial, sans-serif\""),
        "{svg}"
    );
    // Light, regular and heavy are each declared, all from the one file.
    for weight in [300, 400, 700] {
        assert!(
            svg.contains(&format!(
                "@font-face{{font-family:'IBM Plex Sans';font-weight:{weight};src:url(data:font/ttf;base64,Bwc="
            )),
            "{weight}: {svg}"
        );
    }
    parse(&svg);
}

// ---------------------------------------------------------------------------------------------------------------
// Characters that only a glyph-fallback font draws
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn svg_draws_characters_of_the_fallback_fonts_as_outlines_and_keeps_other_text_selectable() {
    let m = real_measure();
    for content in ["ಠ_ಠ", "｡◕‿◕｡", "a → b"] {
        let svg = export_svg(&inline_math_doc(content, false), &SvgOptions::default(), &m, Some(&m)).unwrap();
        let doc = parse(&svg);
        let el = doc.descendants().find(|n| n.attribute("id") == Some("e-t")).unwrap();
        assert!(!el.descendants().any(|n| n.tag_name().name() == "text"), "{content:?}: the glyphs are outlines");
        let g = el.descendants().find(|n| n.attribute("data-text").is_some()).unwrap();
        assert_eq!(g.attribute("data-text"), Some(content), "the source stays readable");
        // One path per colour; each glyph contributes at least one closed contour (`M`).
        let contours: usize = g
            .descendants()
            .filter(|n| n.tag_name().name() == "path")
            .map(|n| n.attribute("d").unwrap_or_default().matches('M').count())
            .sum();
        let glyphs = content.chars().filter(|c| !c.is_whitespace()).count();
        assert!(contours >= glyphs.min(3), "{content:?}: {contours} contours for {glyphs} characters");
    }
    // Text the hand-drawn font can draw stays <text>.
    let svg = export_svg(&inline_math_doc("plain", false), &SvgOptions::default(), &m, Some(&m)).unwrap();
    assert!(svg.contains("<text "));
    // Without an outliner the exporter still writes the text, as before.
    let svg = export_svg(&inline_math_doc("ಠ_ಠ", false), &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    assert!(svg.contains("ಠ_ಠ"));
}

#[test]
fn a_document_with_kaomoji_round_trips_the_native_file_and_the_svg_scene() {
    let mut d = Document::new();
    let mut keys = ZIndex::rebalance(2).unwrap().into_iter();
    for (id, content) in [("a", "ಠ_ಠ"), ("b", "｡◕‿◕｡ ٩(◕‿◕｡)۶ (╯°□°）╯︵ ┻━┻")]
    {
        d.elements.push(
            Element::new(eid(id), ElementKind::Text(TextData::new(content)), keys.next().unwrap())
                .with_bounds(0.0, 0.0, 200.0, 40.0),
        );
    }
    assert_eq!(native::load(&native::save(&d).unwrap()).unwrap(), d);
    let m = real_measure();
    let svg = export_svg(&d, &SvgOptions::default(), &m, Some(&m)).unwrap();
    assert_eq!(drafftink_io::svg::extract_scene(&svg).unwrap().map(|s| native::load(&s).unwrap()), Some(d));
}

#[test]
fn svg_export_skips_a_clear_stroke() {
    let mut d = Document::new();
    let mut rect = Element::new(eid("r"), ElementKind::Rectangle, ZIndex::first()).with_bounds(0.0, 0.0, 100.0, 60.0);
    rect.style.fill_color = Some(Color::rgb(0, 0, 255));
    rect.style.sloppiness = drafftink_scene::Sloppiness::Architect;
    d.elements.push(rect);
    let with_stroke = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    assert!(with_stroke.contains("stroke-width"));
    d.elements[0].style.stroke_color = Color::TRANSPARENT;
    let svg = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    assert!(!svg.contains("stroke-width") && !svg.contains("stroke=\""), "{svg}");
    assert!(svg.contains("fill=\"#0000ff\""), "the fill stays: {svg}");
}
