#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Named versions in the CRDT: they survive every export, merge when two peers save at once, are not undoable, and a
//! hostile entry is hidden without touching the others (`docs/crdt.md` section 7a).

mod common;

use common::*;
use drafftink_doc::{LoroConfig, LoroStore};
use drafftink_scene::*;
use loro::{ExportMode, LoroDoc, LoroValue};

/// Saves a version of the current scene in one commit that undo does not see.
fn save(s: &mut LoroStore, id: &str, name: &str, at: u64) -> NamedVersion {
    let v = NamedVersion::capture(VersionId::new(id).unwrap(), name, at, Some("Ana"), &*s).unwrap();
    let mut tx = s.begin();
    tx.not_undoable();
    tx.put_version(v.clone()).unwrap();
    tx.commit().unwrap();
    v
}

fn names(s: &impl SceneRead) -> Vec<String> {
    s.versions().map(|v| v.name.clone()).collect()
}

fn scene_with(n: usize) -> LoroStore {
    let mut s = peer(1);
    let z = zs(n);
    add(&mut s, (0..n).map(|i| rect(&format!("r{i}"), &z[i])).collect());
    s
}

#[test]
fn versions_survive_every_kind_of_export() {
    let mut a = scene_with(3);
    let first = save(&mut a, "v1", "First", 10);
    edit(&mut a, "r0", |e| e.transform.x = 500.0);
    let second = save(&mut a, "v2", "Second", 20);
    let expect = vec![first.clone(), second.clone()];
    let list = |s: &LoroStore| s.versions().cloned().collect::<Vec<_>>();

    assert_eq!(list(&LoroStore::from_snapshot(&a.export_snapshot().unwrap()).unwrap()), expect, "full snapshot");
    assert_eq!(list(&LoroStore::from_snapshot(&a.export_state_only().unwrap()).unwrap()), expect, "state only");
    assert_eq!(
        list(&LoroStore::from_snapshot(&a.export_shallow_snapshot(0).unwrap()).unwrap()),
        expect,
        "a trimmed snapshot: what a room keeps after compaction"
    );
    let mut late = LoroStore::new();
    late.apply_remote(&a.export_updates_since(&[]).unwrap()).unwrap();
    assert_eq!(list(&late), expect, "an update from nothing");
    let from_doc = LoroStore::from_document(&snapshot(&a)).unwrap();
    assert_eq!(list(&from_doc), expect, "a document: what a saved file holds");
}

#[test]
fn a_version_is_a_value_not_a_pointer_into_history() {
    let mut a = scene_with(2);
    let v = save(&mut a, "v1", "Two shapes", 1);
    // Rewrite history: nothing of the first commits is kept, yet the version opens.
    let trimmed = LoroStore::from_snapshot(&a.export_state_only().unwrap()).unwrap();
    let scene = trimmed.version(&v.id).unwrap().open().unwrap();
    assert_eq!(scene.elements.len(), 2);
}

#[test]
fn two_peers_that_save_at_once_both_keep_their_versions() {
    let mut a = scene_with(1);
    let mut b = LoroStore::from_snapshot_with(
        &a.export_snapshot().unwrap(),
        LoroConfig { peer_id: Some(2), ..LoroConfig::default() },
    )
    .unwrap();
    save(&mut a, "va", "From Ana", 100);
    save(&mut b, "vb", "From Ben", 101);
    save(&mut b, "vb2", "Also Ben", 102);
    sync(&mut a, &mut b);
    assert_eq!(names(&a), vec!["From Ana", "From Ben", "Also Ben"]);
    assert_eq!(names(&a), names(&b));
    assert_eq!(snapshot(&a).versions, snapshot(&b).versions);
}

#[test]
fn the_same_name_twice_is_two_versions() {
    let mut a = scene_with(1);
    let mut b = LoroStore::from_snapshot_with(
        &a.export_snapshot().unwrap(),
        LoroConfig { peer_id: Some(2), ..LoroConfig::default() },
    )
    .unwrap();
    save(&mut a, "va", "Draft", 5);
    save(&mut b, "vb", "Draft", 6);
    sync(&mut a, &mut b);
    assert_eq!(names(&a), vec!["Draft", "Draft"]);
    assert_eq!(names(&b), names(&a));
}

#[test]
fn rename_and_delete_merge_with_saves() {
    let mut a = scene_with(1);
    save(&mut a, "v1", "Old name", 1);
    save(&mut a, "v2", "Doomed", 2);
    let mut b = LoroStore::from_snapshot_with(
        &a.export_snapshot().unwrap(),
        LoroConfig { peer_id: Some(2), ..LoroConfig::default() },
    )
    .unwrap();
    // Ana renames v1 and deletes v2; Ben renames v2 and saves a third.
    let mut tx = a.begin();
    tx.not_undoable();
    tx.rename_version(&VersionId::new("v1").unwrap(), "New name").unwrap();
    tx.remove_version(&VersionId::new("v2").unwrap());
    tx.commit().unwrap();
    let mut tx = b.begin();
    tx.not_undoable();
    tx.rename_version(&VersionId::new("v2").unwrap(), "Ben's name for it").unwrap();
    tx.commit().unwrap();
    save(&mut b, "v3", "Third", 3);
    sync(&mut a, &mut b);
    assert_eq!(names(&a), vec!["New name", "Third"], "the delete wins over a rename of the same version");
    assert_eq!(names(&b), names(&a));
}

#[test]
fn a_rename_writes_the_name_and_not_the_scene() {
    let mut a = scene_with(200);
    let v = save(&mut a, "v1", "Before", 1);
    let rev_bytes = a.export_updates_since(&[]).unwrap().len();
    let vv = a.version_vector().unwrap();
    let mut tx = a.begin();
    tx.not_undoable();
    tx.rename_version(&v.id, "After").unwrap();
    tx.commit().unwrap();
    let update = a.export_updates_since(&vv).unwrap();
    assert!(update.len() < 300, "a rename is {} bytes while the whole history is {rev_bytes}", update.len());
}

#[test]
fn saving_and_naming_are_not_undo_steps_and_undo_leaves_versions_alone() {
    let mut a = scene_with(1);
    let before = a.undo_depth();
    save(&mut a, "v1", "Named", 1);
    assert_eq!(a.undo_depth(), before, "a version is no undo step");
    edit(&mut a, "r0", |e| e.transform.x = 77.0);
    a.undo().unwrap();
    assert_eq!(names(&a), vec!["Named"], "undo of an edit keeps the version");
    a.undo().unwrap();
    assert_eq!(names(&a), vec!["Named"], "so does undo of the step that made the scene");
}

#[test]
fn the_limit_on_stored_versions_is_enforced_by_the_store() {
    let mut a = scene_with(1);
    let base = save(&mut a, "v0", "base", 0);
    let mut tx = a.begin();
    for i in 1..limits::MAX_VERSIONS_STORED {
        tx.put_version(NamedVersion { id: VersionId::new(&format!("v{i}")).unwrap(), ..base.clone() }).unwrap();
    }
    let over = NamedVersion { id: VersionId::new("over").unwrap(), ..base };
    assert!(matches!(tx.put_version(over), Err(StoreError::BadVersion(VersionRejection::TooMany { .. }))));
}

/// A second writer that bypasses every rule of the model.
fn hostile_entry(store: &LoroStore, write: impl FnOnce(&loro::LoroMap)) -> LoroStore {
    let doc = LoroDoc::new();
    doc.set_peer_id(666).unwrap();
    doc.import(&store.export_snapshot().unwrap()).unwrap();
    write(&doc.get_map("versions").ensure_mergeable_map("evil").unwrap());
    doc.commit();
    let mut to = LoroStore::from_snapshot(&store.export_snapshot().unwrap()).unwrap();
    let from = loro::VersionVector::decode(&to.version_vector().unwrap()).unwrap();
    to.apply_remote(&doc.export(ExportMode::updates(&from)).unwrap()).unwrap();
    to
}

#[test]
fn hostile_version_entries_are_hidden_and_the_others_stay() {
    let mut a = scene_with(1);
    let good = save(&mut a, "v1", "Good", 1);
    let state = LoroValue::from(good.state.to_vec());
    let cases: [(&str, Box<dyn FnOnce(&loro::LoroMap)>); 5] = [
        ("empty", Box::new(|_| {})),
        (
            "bad name",
            Box::new({
                let s = state.clone();
                move |m| {
                    m.insert("name", "two\nlines").unwrap();
                    m.insert("at", 1i64).unwrap();
                    m.insert("elements", 1i64).unwrap();
                    m.insert("state", s).unwrap();
                }
            }),
        ),
        (
            "negative time",
            Box::new({
                let s = state.clone();
                move |m| {
                    m.insert("name", "x").unwrap();
                    m.insert("at", -5i64).unwrap();
                    m.insert("elements", 1i64).unwrap();
                    m.insert("state", s).unwrap();
                }
            }),
        ),
        (
            "bad file id",
            Box::new({
                let s = state.clone();
                move |m| {
                    m.insert("name", "x").unwrap();
                    m.insert("at", 1i64).unwrap();
                    m.insert("elements", 1i64).unwrap();
                    m.insert("files", "not-a-hash").unwrap();
                    m.insert("state", s).unwrap();
                }
            }),
        ),
        (
            "state of the wrong type",
            Box::new(|m| {
                m.insert("name", "x").unwrap();
                m.insert("at", 1i64).unwrap();
                m.insert("elements", 1i64).unwrap();
                m.insert("state", "text").unwrap();
            }),
        ),
    ];
    for (what, write) in cases {
        let to = hostile_entry(&a, write);
        assert_eq!(names(&to), vec!["Good"], "{what}: only the valid version is shown");
        assert!(to.get(&eid("r0")).is_some(), "{what}: the scene is untouched");
    }
}

#[test]
fn an_unreadable_scene_inside_a_valid_entry_fails_when_opened_and_not_before() {
    let mut a = scene_with(1);
    let good = save(&mut a, "v1", "Good", 1);
    let to = hostile_entry(&a, |m| {
        m.insert("name", "Garbage inside").unwrap();
        m.insert("at", 2i64).unwrap();
        m.insert("elements", 1i64).unwrap();
        m.insert("state", LoroValue::from(vec![1u8, 2, 3, 4])).unwrap();
    });
    assert_eq!(names(&to), vec!["Good", "Garbage inside"]);
    assert!(to.version(&VersionId::new("evil").unwrap()).unwrap().open().is_err());
    assert!(to.version(&good.id).unwrap().open().is_ok());
}

#[test]
fn a_staged_import_brings_the_versions() {
    let mut a = scene_with(50);
    save(&mut a, "v1", "Staged", 1);
    let mut b = LoroStore::new();
    b.set_stage_import_bytes(1);
    b.apply_remote(&a.export_snapshot().unwrap()).unwrap();
    while b.import_progress().is_some() {
        b.import_step(7).unwrap();
    }
    assert_eq!(names(&b), vec!["Staged"]);
    assert_eq!(b.len(), 50);
}

#[test]
fn replacing_a_document_from_a_snapshot_replaces_its_versions() {
    let mut a = scene_with(1);
    save(&mut a, "v1", "Ana's", 1);
    let mut b = scene_with(1);
    save(&mut b, "vb", "Ben's", 2);
    let cs = b.replace_from_snapshot(&a.export_snapshot().unwrap()).unwrap();
    assert_eq!(names(&b), vec!["Ana's"]);
    assert_eq!(cs.versions.len(), 2, "one added and one removed");
}
