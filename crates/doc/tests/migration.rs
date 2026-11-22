#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The migration hook: opening an older document runs the steps, then the document reads normally.

mod common;

use common::*;
use drafftink_doc::{DocError, LoroConfig, LoroStore, MIGRATIONS, Migration, SCHEMA_VERSION};
use drafftink_scene::*;
use loro::{ExportMode, LoroDoc, LoroValue};

/// A pretend version-0 document: the schema number is 0 and the name lives under an old key.
fn version_zero() -> Vec<u8> {
    let doc = LoroDoc::new();
    let meta = doc.get_map("meta");
    meta.insert("schema", 0i64).unwrap();
    meta.insert("title", "Old title").unwrap();
    let m = doc.get_map("elements").ensure_mergeable_map("r").unwrap();
    m.insert("kind", "rectangle").unwrap();
    for k in ["x", "y", "w", "h"] {
        m.insert(k, 3.0).unwrap();
    }
    m.insert("index", "a0").unwrap();
    doc.commit();
    doc.export(ExportMode::Snapshot).unwrap()
}

fn rename_title(doc: &LoroDoc) -> Result<(), DocError> {
    let meta = doc.get_map("meta");
    if let Some(loro::ValueOrContainer::Value(v)) = meta.get("title") {
        meta.insert("name", v)?;
        meta.delete("title")?;
    }
    Ok(())
}

/// The step of the pretend version 0, then the built-in steps.
fn table_from_zero() -> Vec<Migration> {
    std::iter::once(Migration { from: 0, apply: rename_title }).chain(MIGRATIONS.iter().copied()).collect()
}

#[test]
fn an_older_document_is_migrated_on_open() {
    let s = LoroStore::open_with_migrations(&version_zero(), LoroConfig::default(), &table_from_zero()).unwrap();
    assert_eq!(s.schema_version(), SCHEMA_VERSION);
    assert_eq!(s.meta().name, "Old title");
    assert_eq!(s.len(), 1);
    assert!(!s.can_undo(), "migration is not an undo step");
    // The migrated document is a normal current document from now on, for everybody.
    let again = LoroStore::from_snapshot(&s.export_snapshot().unwrap()).unwrap();
    assert_same(&s, &again);
    assert_eq!(again.schema_version(), SCHEMA_VERSION);
}

#[test]
fn two_peers_migrating_the_same_old_document_still_converge() {
    let old = version_zero();
    let table = table_from_zero();
    let cfg = |p| LoroConfig { peer_id: Some(p), ..LoroConfig::default() };
    let mut a = LoroStore::open_with_migrations(&old, cfg(1), &table).unwrap();
    let mut b = LoroStore::open_with_migrations(&old, cfg(2), &table).unwrap();
    edit(&mut a, "r", |e| e.transform.x = 9.0);
    sync(&mut a, &mut b);
    assert_same(&a, &b);
    assert_eq!(a.meta().name, "Old title");
}

#[test]
fn without_a_step_for_the_old_version_the_open_fails_with_words() {
    let err = LoroStore::from_snapshot(&version_zero()).unwrap_err();
    assert!(matches!(err, DocError::Migration { from: 0, .. }), "{err}");
    assert!(err.to_string().contains("version 0"));
}

/// A schema-1 document written by hand, as the previous build stored it: `files[id] = {mime, bytes}`, no size.
fn schema_one(files: &[&FileEntry]) -> Vec<u8> {
    let doc = LoroDoc::new();
    doc.get_map("meta").insert("schema", 1i64).unwrap();
    let m = doc.get_map("elements").ensure_mergeable_map("r").unwrap();
    m.insert("kind", "rectangle").unwrap();
    for k in ["x", "y", "w", "h"] {
        m.insert(k, 3.0).unwrap();
    }
    m.insert("index", "a0").unwrap();
    let map = doc.get_map("files");
    for f in files {
        let value = std::collections::HashMap::from([
            ("mime".to_owned(), LoroValue::from(f.mime.as_str())),
            ("bytes".to_owned(), LoroValue::from(f.bytes.to_vec())),
        ]);
        map.insert(FileId::of(&f.bytes).as_str(), LoroValue::from(value)).unwrap();
    }
    doc.commit();
    doc.export(ExportMode::Snapshot).unwrap()
}

#[test]
fn the_built_in_table_has_the_step_from_schema_one() {
    assert_eq!(SCHEMA_VERSION, 2);
    assert_eq!(MIGRATIONS.iter().map(|m| m.from).collect::<Vec<_>>(), vec![1]);
    assert_eq!(peer(1).schema_version(), 2);
}

#[test]
fn schema_one_to_two_adds_the_size_of_every_file() {
    let (a_id, a) = picture(1, 300);
    let (b_id, b) = picture(2, 5000);
    let s = LoroStore::from_snapshot(&schema_one(&[&a, &b])).unwrap();
    assert_eq!(s.schema_version(), 2);
    assert!(!s.can_undo(), "migration is not an undo step");
    let json = s.debug_json();
    assert_eq!(json["meta"]["schema"], 2);
    assert_eq!(json["files"][a_id.as_str()]["size"], 300);
    assert_eq!(json["files"][b_id.as_str()]["size"], 5000);
    // The bytes and the media type stay, and the view reads them as before.
    assert_eq!(s.file(&a_id).unwrap(), &a);
    assert_eq!(s.file(&b_id).unwrap(), &b);
    assert_eq!(s.manifest(&a_id), Some(&FileManifest { mime: "image/png".into(), size: 300 }));
    assert_eq!(s.manifests().count(), 2);
    assert_view_matches_loro(&s);
}

#[test]
fn the_step_is_idempotent() {
    let (_, a) = picture(3, 64);
    let doc = LoroDoc::new();
    doc.import(&schema_one(&[&a])).unwrap();
    let step = MIGRATIONS[0].apply;
    step(&doc).unwrap();
    doc.commit();
    let after_one = (doc.oplog_vv(), doc.get_deep_value());
    step(&doc).unwrap();
    doc.commit();
    assert_eq!((doc.oplog_vv(), doc.get_deep_value()), after_one, "a second run writes nothing");
}

#[test]
fn two_peers_migrating_schema_one_converge_to_one_document() {
    let old = schema_one(&[&picture(4, 700).1, &picture(5, 9).1]);
    let cfg = |p| LoroConfig { peer_id: Some(p), ..LoroConfig::default() };
    let mut a = LoroStore::from_snapshot_with(&old, cfg(1)).unwrap();
    let mut b = LoroStore::from_snapshot_with(&old, cfg(2)).unwrap();
    edit(&mut a, "r", |e| e.transform.x = 9.0);
    sync(&mut a, &mut b);
    assert_same(&a, &b);
    assert_eq!(a.debug_json(), b.debug_json());
    assert_eq!(a.manifests().collect::<Vec<_>>(), b.manifests().collect::<Vec<_>>());
    assert_eq!(a.manifests().count(), 2);
    assert_view_matches_loro(&a);
    assert_view_matches_loro(&b);
}

#[test]
fn a_document_of_schema_two_opens_without_a_step_and_a_schema_one_build_would_refuse_it() {
    let mut s = peer(1);
    put_picture(&mut s, 6, 100);
    let bytes = s.export_snapshot().unwrap();
    assert_eq!(LoroStore::from_snapshot(&bytes).unwrap().schema_version(), 2);
    // Without any table a schema-2 document opens: nothing to migrate.
    assert_eq!(LoroStore::open_with_migrations(&bytes, LoroConfig::default(), &[]).unwrap().schema_version(), 2);
    // The refusal of a build whose newest schema is 1 is `drafftink_doc::schema::migrate_to` with that maximum
    // (a unit test in `schema.rs`); here the same rule from the outside: one number above this build's.
    let doc = LoroDoc::new();
    doc.import(&bytes).unwrap();
    doc.get_map("meta").insert("schema", i64::from(SCHEMA_VERSION) + 1).unwrap();
    doc.commit();
    let err = LoroStore::from_snapshot(&doc.export(ExportMode::Snapshot).unwrap()).unwrap_err();
    assert_eq!(err, DocError::NewerSchema { found: 3, max: 2 });
}
