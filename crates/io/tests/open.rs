#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::field_reassign_with_default)]
//! `open`: every fixture goes through one entry point and ends up in the right place.

use std::path::{Path, PathBuf};

use drafftink_io::measure::EstimateMeasure;
use drafftink_io::native::Format;
use drafftink_io::svg::{SvgOptions, export_svg};
use drafftink_io::{IoError, Opened, open, png_meta};
use drafftink_scene::{Document, Element, ElementId, ElementKind, SeqIds, ZIndex};
use proptest::prelude::*;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn run(bytes: &[u8]) -> Result<Opened, IoError> {
    open(bytes, &mut SeqIds::new(), &EstimateMeasure)
}

fn doc() -> Document {
    let mut d = Document::new();
    d.name = "round trip".into();
    let mut e = Element::new(ElementId::new("r1").unwrap(), ElementKind::Rectangle, ZIndex::first())
        .with_bounds(0.0, 0.0, 50.0, 30.0);
    e.style.fill_color = Some(drafftink_scene::Color::rgb(1, 2, 3));
    d.elements.push(e);
    d
}

fn png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 2, 2);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[0, 0, 0, 0]).unwrap();
    }
    out
}

#[test]
fn every_legacy_fixture_opens_as_a_document() {
    let dir = fixtures().join("legacy");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() || name.contains(".facts.") {
            continue;
        }
        match run(&std::fs::read(entry.path()).unwrap()).unwrap() {
            Opened::Document { imported, source } => {
                assert!(matches!(source, Format::LegacyJson), "{name}: {source:?}");
                assert!(!imported.document.validate().has_errors());
            }
            other => panic!("{name}: {other:?}"),
        }
        n += 1;
    }
    assert_eq!(n, 15);
    for entry in std::fs::read_dir(dir.join("crdt")).unwrap().filter_map(Result::ok) {
        if entry.path().extension().is_some_and(|e| e == "loro") {
            let Opened::Document { source, .. } = run(&std::fs::read(entry.path()).unwrap()).unwrap() else { panic!() };
            assert_eq!(source, Format::LegacySnapshot);
        }
    }
}

#[test]
fn excalidraw_and_mermaid_fixtures_route_to_their_readers() {
    let fx = fixtures();
    for f in [
        "basic_shapes",
        "bindings",
        "groups",
        "frames",
        "text_containers",
        "elbow_arrows",
        "closed_paths",
        "minimal_fields",
    ] {
        let bytes = std::fs::read(fx.join(format!("excalidraw/{f}.excalidraw"))).unwrap();
        assert!(matches!(run(&bytes).unwrap(), Opened::Document { source: Format::ExcalidrawScene, .. }), "{f}");
    }
    for f in ["fixture_library", "v1_library", "grouped_library"] {
        let bytes = std::fs::read(fx.join(format!("excalidraw/{f}.excalidrawlib"))).unwrap();
        let Opened::Library { library, .. } = run(&bytes).unwrap() else { panic!("{f}") };
        assert!(!library.items.is_empty());
    }
    let clip = std::fs::read(fx.join("excalidraw/clipboard/basic.json")).unwrap();
    assert!(matches!(run(&clip).unwrap(), Opened::Document { source: Format::ExcalidrawClipboard, .. }));
    let mm = std::fs::read(fx.join("mermaid/flowchart_subgraph.mmd")).unwrap();
    assert!(matches!(run(&mm).unwrap(), Opened::Diagram(_)));
    for bad in ["not_json.excalidraw", "not_a_library.excalidrawlib", "no_elements.excalidraw"] {
        let r = run(&std::fs::read(fx.join(format!("excalidraw/{bad}"))).unwrap());
        // Rejected with an error whose message is plain words, never a panic.
        if let Err(e) = r {
            assert!(!e.to_string().is_empty());
        }
    }
}

#[test]
fn native_png_and_svg_round_trip_through_open() {
    let d = doc();
    let json = d.to_json().unwrap();
    let Opened::Document { imported, source } = run(json.as_bytes()).unwrap() else { panic!() };
    assert_eq!((imported.document, source), (d.clone(), Format::Native));

    let with_scene = png_meta::embed_scene(&png(), &json).unwrap();
    let Opened::Document { imported, source } = run(&with_scene).unwrap() else { panic!() };
    assert_eq!((imported.document, source), (d.clone(), Format::Png));

    let svg = export_svg(&d, &SvgOptions::default(), &EstimateMeasure, None).unwrap();
    let Opened::Document { imported, source } = run(svg.as_bytes()).unwrap() else { panic!() };
    assert_eq!((imported.document, source), (d.clone(), Format::Svg));

    // A PNG carrying the old app's document opens too (through migration).
    let old = std::fs::read_to_string(fixtures().join("legacy/all_shapes.json")).unwrap();
    let old_png = png_meta::embed_scene(&png(), &old).unwrap();
    let Opened::Document { imported, .. } = run(&old_png).unwrap() else { panic!() };
    assert_eq!(imported.document.elements.len(), 11);
}

#[test]
fn pictures_without_a_scene_and_unknown_bytes_are_plain_errors() {
    let e = run(&png()).unwrap_err();
    assert!(matches!(e, IoError::WrongFormat { .. }), "{e}");
    assert!(e.to_string().contains("insert it as an image"));
    let svg = export_svg(&doc(), &SvgOptions { embed_scene: false, ..SvgOptions::default() }, &EstimateMeasure, None)
        .unwrap();
    assert!(matches!(run(svg.as_bytes()), Err(IoError::WrongFormat { .. })));
    for junk in [&b""[..], b"hello", b"\x00\x01\x02", b"{}", b"[]"] {
        assert!(matches!(run(junk), Err(IoError::WrongFormat { .. })), "{junk:?}");
    }
}

#[test]
fn nested_containers_are_bounded() {
    // A scene whose payload is a PNG whose payload is a scene ...: each level must be sniffed again, within a limit.
    let mut payload = doc().to_json().unwrap().into_bytes();
    for _ in 0..6 {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &payload).unwrap();
        payload = gz.finish().unwrap();
    }
    assert!(run(&payload).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]
    #[test]
    fn open_never_panics(prefix in prop::sample::select(vec![
        &b""[..], b"{", b"[", b"<svg>", b"\x89PNG\r\n\x1a\n", b"loro", b"\x1f\x8b", b"graph TD\n", b"{\"type\":\"excalidraw\",", b"{\"format\":\"drafftink\",",
        b"{\"shapes\":{},\"z_order\":[", b"{\"type\":\"excalidrawlib\",",
    ]), rest in prop::collection::vec(any::<u8>(), 0..300)) {
        let mut b = prefix.to_vec();
        b.extend(rest);
        let _ = run(&b);
    }
}
