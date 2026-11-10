#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The migration hook: opening an older document runs the steps, then the document reads normally.

mod common;

use common::*;
use drafftink_doc::{DocError, LoroConfig, LoroStore, Migration};
use drafftink_scene::*;
use loro::{ExportMode, LoroDoc};

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

#[test]
fn an_older_document_is_migrated_on_open() {
    let table = [Migration { from: 0, apply: rename_title }];
    let s = LoroStore::open_with_migrations(&version_zero(), LoroConfig::default(), &table).unwrap();
    assert_eq!(s.schema_version(), 1);
    assert_eq!(s.meta().name, "Old title");
    assert_eq!(s.len(), 1);
    assert!(!s.can_undo(), "migration is not an undo step");
    // The migrated document is a normal v1 document from now on, for everybody.
    let again = LoroStore::from_snapshot(&s.export_snapshot().unwrap()).unwrap();
    assert_same(&s, &again);
    assert_eq!(again.schema_version(), 1);
}

#[test]
fn two_peers_migrating_the_same_old_document_still_converge() {
    let old = version_zero();
    let table = [Migration { from: 0, apply: rename_title }];
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

#[test]
fn the_built_in_table_is_empty_because_version_one_is_the_first() {
    assert!(drafftink_doc::MIGRATIONS.is_empty());
    assert_eq!(drafftink_doc::SCHEMA_VERSION, 1);
    assert_eq!(peer(1).schema_version(), 1);
}
