#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// One test prints how long a capture takes. The time is not asserted.
#![allow(clippy::disallowed_methods)]
//! Named versions in files: a round trip, an old file, what an export leaves out, and what a version costs in size.
//! The sizes asserted here are the ones `docs/design/versions.md` reports.

use std::collections::HashSet;

use drafftink_io::native::{self, subset};
use drafftink_io::{EstimateMeasure, excalidraw, png_meta, svg};
use drafftink_scene::scenegen;
use drafftink_scene::{
    Document, Element, ElementKind, FileEntry, IdSource, ImageData, MemoryStore, NamedVersion, SceneStore, SceneTx,
    SeqIds, Transform, ZIndex, snapshot,
};

const INTRO: &str = include_str!("../../engine/assets/intro/intro.json");

/// `doc` with a version saved after each name, the way `version.save` does it.
fn with_versions(doc: &Document, names: &[&str]) -> Document {
    let mut store = MemoryStore::from_document(doc).unwrap();
    let mut ids = SeqIds::new();
    for (i, name) in names.iter().enumerate() {
        let v =
            NamedVersion::capture(ids.version_id(), name, 1_700_000_000_000 + i as u64, Some("Ana"), &store).unwrap();
        let mut tx = store.begin();
        tx.put_version(v).unwrap();
        tx.commit().unwrap();
    }
    snapshot(&store)
}

fn intro() -> Document {
    native::load(INTRO).unwrap()
}

fn thousand() -> Document {
    scenegen::generate(1_000, 1).unwrap()
}

fn picture(len: usize) -> (Document, drafftink_scene::FileId) {
    let mut rng = drafftink_scene::SplitMix64::new(3);
    let bytes: Vec<u8> = (0..len).map(|_| drafftink_scene::Rng64::next_u64(&mut rng).to_le_bytes()[0]).collect();
    let (id, entry) = FileEntry::new("image/png", bytes);
    let mut e = Element::new(
        SeqIds::new().element_id(),
        ElementKind::Image(ImageData {
            file_id: Some(id.clone()),
            natural_width: 100,
            natural_height: 100,
            crop: None,
            paint: Default::default(),
        }),
        ZIndex::first(),
    );
    e.transform = Transform::new(0.0, 0.0, 100.0, 100.0);
    let mut doc = Document::new();
    doc.elements.push(e);
    doc.files.insert(id.clone(), entry);
    (doc, id)
}

#[test]
fn versions_round_trip_through_a_native_file() {
    let doc = with_versions(&intro(), &["Welcome", "Welcome again"]);
    assert_eq!(doc.versions.len(), 2);
    for text in [native::save(&doc).unwrap(), native::save_pretty(&doc).unwrap()] {
        let back = native::load(&text).unwrap();
        assert_eq!(back, doc);
        let scene = back.versions[0].open().unwrap();
        assert_eq!(scene.elements.len(), doc.elements.len());
        assert_eq!(scene.background, doc.background);
    }
    assert!(native::save(&doc).unwrap().contains(r#""version":2"#));
}

#[test]
fn an_old_file_without_versions_loads_and_is_written_back_as_version_one() {
    let doc = intro();
    assert!(doc.versions.is_empty());
    let text = native::save(&doc).unwrap();
    assert!(text.starts_with(r#"{"format":"drafftink","version":1,"#), "{}", &text[..60]);
    assert!(!text.contains("versions"));
    // A file from before named versions, byte for byte: it has no `versions` key at all.
    let old = r##"{"format":"drafftink","version":1,"name":"Old","background":"#fafafa","elements":[]}"##;
    let doc = native::load(old).unwrap();
    assert_eq!((doc.name.as_str(), doc.versions.len(), doc.schema_version), ("Old", 0, 1));
    assert_eq!(native::sniff(old.as_bytes()), native::Format::Native);
}

#[test]
fn open_reads_a_file_with_versions_like_any_native_file() {
    let doc = with_versions(&intro(), &["A"]);
    let text = native::save(&doc).unwrap();
    let opened = drafftink_io::open(text.as_bytes(), &mut SeqIds::new(), &EstimateMeasure).unwrap();
    let drafftink_io::Opened::Document { imported, source } = opened else { panic!("a document") };
    assert_eq!(source, native::Format::Native);
    assert_eq!(imported.document.versions, doc.versions);
}

#[test]
fn excalidraw_export_drops_versions_and_says_so() {
    let doc = with_versions(&intro(), &["One", "Two", "Three"]);
    let (text, report) = excalidraw::export_scene_with_report(&doc).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.starts_with("3 named versions were left out: an Excalidraw file has no named versions.")),
        "{:?}",
        report.warnings
    );
    assert!(!text.contains("One") || !text.contains("versions"), "the file carries no versions");
    let back = excalidraw::import_scene(&text, &mut SeqIds::new()).unwrap();
    assert!(back.document.versions.is_empty());
    let one = with_versions(&intro(), &["Only"]);
    let (_, report) = excalidraw::export_scene_with_report(&one).unwrap();
    assert!(report.warnings.iter().any(|w| w.starts_with("1 named version was left out:")), "{:?}", report.warnings);
    let (_, report) = excalidraw::export_scene_with_report(&intro()).unwrap();
    assert!(!report.warnings.iter().any(|w| w.contains("named version")), "no versions, no warning");
}

#[test]
fn a_scene_embedded_in_a_png_or_svg_or_copied_has_no_versions() {
    let doc = with_versions(&thousand(), &["Big"]);
    let keep: HashSet<_> = doc.elements.iter().map(|e| e.id.clone()).collect();
    let sub = subset(&doc, &keep);
    assert!(sub.versions.is_empty() && sub.schema_version == 1);
    let embedded = native::save(&sub).unwrap();
    assert!(!embedded.contains("versions"));
    let full = native::save(&doc).unwrap().len();
    assert!(full - embedded.len() >= doc.versions[0].stored_bytes(), "the version is what the embedded scene saves");

    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, 2, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&[255; 16]).unwrap();
    }
    let with_scene = png_meta::embed_scene(&png_bytes, &embedded).unwrap();
    let out = png_meta::extract_scene(&with_scene).unwrap().unwrap();
    assert!(!out.contains("versions"));

    let svg_text = svg::export_svg(&doc, &svg::SvgOptions::default(), &EstimateMeasure, None).unwrap();
    let scene = svg::extract_scene(&svg_text).unwrap().unwrap();
    assert!(!scene.contains("versions"), "the SVG carries the scene and no versions");
    assert!(native::load(&scene).unwrap().versions.is_empty());
}

#[test]
fn a_picture_is_in_the_file_once_however_many_versions_name_it() {
    let (doc, id) = picture(300_000);
    let plain = native::save(&doc).unwrap();
    let many = with_versions(&doc, &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]);
    let text = native::save(&many).unwrap();
    let b64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&doc.files[&id].bytes)
    };
    let probe = &b64[b64.len() / 2..b64.len() / 2 + 64];
    assert_eq!(text.matches(probe).count(), 1, "the picture bytes are written once");
    assert_eq!(many.versions.iter().filter(|v| v.files == vec![id.clone()]).count(), 10, "each version names the blob");
    let extra = text.len() - plain.len();
    assert!(
        extra < 10 * 2_000,
        "ten versions of a one-picture scene cost {extra} bytes, and the picture is {} bytes",
        b64.len()
    );
    // The picture still opens from the version.
    let back = native::load(&text).unwrap();
    assert!(back.files.contains_key(&id));
    assert!(back.validate().is_clean());
}

#[test]
fn a_version_is_a_small_fraction_of_the_scene_it_holds() {
    let mut report = String::new();
    for (name, doc) in [("intro", intro()), ("1k", thousand())] {
        let json = native::save(&doc).unwrap().len();
        let one = native::save(&with_versions(&doc, &["x"])).unwrap().len() - json;
        let five = native::save(&with_versions(&doc, &["a", "b", "c", "d", "e"])).unwrap().len() - json;
        let with_one = with_versions(&doc, &["x"]);
        let v = &with_one.versions[0];
        // Not asserted (a loaded machine): what saving and opening one version takes, for docs/design/versions.md.
        let store = MemoryStore::from_document(&doc).unwrap();
        let t = std::time::Instant::now();
        let made = NamedVersion::capture(SeqIds::new().version_id(), "t", 0, None, &store).unwrap();
        let (saved, t) = (t.elapsed(), std::time::Instant::now());
        let opened = made.open().unwrap();
        let opened_in = t.elapsed();
        assert_eq!(opened.elements.len(), doc.elements.len());
        report.push_str(&format!(
            "{name}: {} elements, scene json {json} B, one version {} B stored, {one} B in a file, five {five} B; capture {saved:?}, open {opened_in:?}\n",
            doc.elements.len(),
            v.stored_bytes()
        ));
        assert!(v.stored_bytes() * 4 < json, "{name}: a version takes {} of {json} bytes", v.stored_bytes());
        assert!(five < 6 * one, "{name}: versions add up linearly");
        assert!(v.stored_bytes() <= drafftink_scene::limits::MAX_VERSION_STATE_BYTES);
    }
    eprintln!("{report}");
}
