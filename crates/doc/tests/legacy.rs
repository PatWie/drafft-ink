#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Every legacy fixture (documents from the old app) loads into a `LoroStore` and survives every export form.

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};

use common::legacy::convert;
use common::*;
use drafftink_doc::LoroStore;
use drafftink_scene::*;
use serde_json::Value;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/legacy")
}

fn read_fixture(path: &Path) -> Value {
    let raw = std::fs::read(path).unwrap();
    let text = if path.extension().is_some_and(|e| e == "gz") {
        let mut s = String::new();
        flate2::read::GzDecoder::new(&raw[..]).read_to_string(&mut s).unwrap();
        s
    } else {
        String::from_utf8(raw).unwrap()
    };
    serde_json::from_str(&text).unwrap()
}

fn fixtures(include_large: bool) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            !n.contains(".facts.") && (n.ends_with(".json") || (include_large && n.ends_with(".json.gz")))
        })
        .collect();
    v.sort();
    v
}

fn check_round_trips(name: &str, doc: &Document) {
    let mut s = peer(1);
    let cs = load_document(&mut s, doc).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(cs.elements.len(), doc.elements.len(), "{name}");
    assert_eq!(s.len_all(), doc.elements.len(), "{name}");
    assert_eq!(
        content(&s).elements,
        doc.elements
            .iter()
            .cloned()
            .map(|mut e| {
                e.version = 0;
                e
            })
            .collect::<Vec<_>>(),
        "{name}: view"
    );
    assert_eq!(s.meta().name, doc.name, "{name}");
    assert_eq!(s.files().count(), doc.files.len(), "{name}");
    assert_eq!(s.invalid_elements().count(), 0, "{name}");

    // Snapshot, state-only, shallow snapshot and plain updates all reproduce it.
    let from_snapshot = LoroStore::from_snapshot(&s.export_snapshot().unwrap()).unwrap();
    assert_same(&s, &from_snapshot);
    assert_same(&s, &LoroStore::from_snapshot(&s.export_state_only().unwrap()).unwrap());
    assert_same(&s, &LoroStore::from_snapshot(&s.export_shallow_snapshot(0).unwrap()).unwrap());
    let mut replica = peer(2);
    push(&s, &mut replica);
    assert_same(&s, &replica);
    assert_eq!(s.derived(), from_snapshot.derived(), "{name}: derived index");

    // And back to the native file format, equal to the input.
    let mut back = snapshot(&from_snapshot);
    for e in &mut back.elements {
        e.version = 0;
    }
    let mut want = doc.clone();
    for e in &mut want.elements {
        e.version = 0;
    }
    assert_eq!(back, want, "{name}: native document");
    assert!(!validate_scene(&from_snapshot).has_errors(), "{name}");
}

#[test]
fn every_legacy_fixture_round_trips_through_the_loro_store() {
    let files = fixtures(false);
    assert!(files.len() >= 12, "fixtures found: {files:?}");
    for path in files {
        let doc = convert(&read_fixture(&path));
        assert!(!doc.validate().has_errors(), "{}", path.display());
        check_round_trips(&path.file_name().unwrap().to_string_lossy(), &doc);
    }
}

#[test]
fn the_large_generated_fixtures_round_trip_too() {
    let large: Vec<PathBuf> =
        fixtures(true).into_iter().filter(|p| p.to_string_lossy().ends_with("large_1000.json.gz")).collect();
    assert_eq!(large.len(), 1);
    let doc = convert(&read_fixture(&large[0]));
    assert!(doc.elements.len() >= 1000);
    check_round_trips("large_1000", &doc);
}

#[test]
fn the_ten_thousand_element_fixture_loads_and_reopens() {
    let path = dir().join("large_10000.json.gz");
    let doc = convert(&read_fixture(&path));
    let mut s = peer(1);
    load_document(&mut s, &doc).unwrap();
    let reopened = LoroStore::from_snapshot(&s.export_snapshot().unwrap()).unwrap();
    assert_eq!(reopened.len_all(), doc.elements.len());
    assert_same(&s, &reopened);
}

#[test]
fn the_shipped_intro_document_loads_and_edits() {
    let doc = convert(&read_fixture(&dir().join("welcome.json")));
    let mut s = LoroStore::from_document(&doc).unwrap();
    assert_eq!(s.derived().group_count(), 2);
    let first = s.iter().next().unwrap().id.clone();
    edit(&mut s, first.as_str(), |e| e.transform.x += 10.0);
    s.undo().unwrap().unwrap();
    assert_eq!(s.get(&first).unwrap().transform.x, doc.elements.iter().find(|e| e.id == first).unwrap().transform.x);
    assert_view_matches_loro(&s);
}
