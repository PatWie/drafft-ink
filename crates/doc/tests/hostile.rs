#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Hostile input never panics: random bytes, mutated updates, and valid Loro data with nonsense content.

mod common;

use common::*;
use drafftink_doc::{DocError, LoroConfig, LoroStore};
use drafftink_scene::*;
use loro::{ExportMode, LoroDoc, LoroValue};
use proptest::prelude::*;

/// A second writer that shares `store`'s history but writes raw Loro, bypassing every rule of the model.
struct Hostile {
    doc: LoroDoc,
}

impl Hostile {
    fn of(store: &LoroStore) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(666).unwrap();
        doc.import(&store.export_snapshot().unwrap()).unwrap();
        Self { doc }
    }

    fn element(&self, id: &str) -> loro::LoroMap {
        self.doc.get_map("elements").ensure_mergeable_map(id).unwrap()
    }

    /// A valid rectangle written by hand, to be spoiled by the test.
    fn good_rect(&self, id: &str) -> loro::LoroMap {
        let m = self.element(id);
        m.insert("kind", "rectangle").unwrap();
        for k in ["x", "y", "w", "h"] {
            m.insert(k, 5.0).unwrap();
        }
        m.insert("index", "a5").unwrap();
        m
    }

    fn send(&self, to: &mut LoroStore) -> Result<ChangeSet, DocError> {
        self.doc.commit();
        let from = loro::VersionVector::decode(&to.version_vector().unwrap()).unwrap();
        let bytes = self.doc.export(ExportMode::updates(&from)).unwrap();
        to.apply_remote(&bytes)
    }
}

fn base() -> LoroStore {
    let mut s = peer(1);
    add(&mut s, vec![rect("fine", &zs(1)[0])]);
    s
}

fn assert_hidden(s: &LoroStore, id: &str) {
    assert!(s.get(&eid(id)).is_none(), "{id} should be hidden");
    assert!(s.invalid_elements().any(|i| i.as_str() == id), "{id} should be reported as invalid");
    assert!(s.get(&eid("fine")).is_some(), "other elements are unaffected");
}

#[test]
fn a_well_formed_handwritten_element_is_accepted() {
    let mut s = base();
    let h = Hostile::of(&s);
    h.good_rect("hand");
    let cs = h.send(&mut s).unwrap();
    assert_eq!(cs.get(&eid("hand")).unwrap().kind, ChangeKind::Added);
    assert_eq!(s.get(&eid("hand")).unwrap().transform.w, 5.0);
}

#[test]
fn wrong_types_out_of_range_numbers_and_unknown_tags_hide_the_element() {
    type Spoil = fn(&loro::LoroMap);
    let spoilers: Vec<(&str, Spoil)> = vec![
        ("string_x", |m| m.insert("x", "oops").unwrap()),
        ("nan_x", |m| m.insert("x", f64::NAN).unwrap()),
        ("inf_w", |m| m.insert("w", f64::INFINITY).unwrap()),
        ("negative_w", |m| m.insert("w", -4.0).unwrap()),
        ("huge_x", |m| m.insert("x", 1.0e30).unwrap()),
        ("unknown_kind", |m| m.insert("kind", "hexagon").unwrap()),
        ("kind_is_number", |m| m.insert("kind", 7i64).unwrap()),
        ("bad_index", |m| m.insert("index", "").unwrap()),
        ("index_is_list", |m| m.insert("index", vec![1i64, 2]).unwrap()),
        ("negative_stroke", |m| m.insert("style.stroke_width", -3.0).unwrap()),
        ("bad_color", |m| m.insert("style.stroke_color", "not-a-colour").unwrap()),
        ("bad_enum", |m| m.insert("style.fill_pattern", "plaid").unwrap()),
        ("unknown_style_key", |m| m.insert("style.future_thing", 1i64).unwrap()),
        ("bad_group", |m| m.insert("group_ids", vec!["fine", "no spaces allowed"]).unwrap()),
        ("name_control_char", |m| m.insert("name", "a\u{0007}b").unwrap()),
        ("opacity_big", |m| m.insert("style.opacity", 7.0).unwrap()),
        ("value_and_group_clash", |m| {
            m.insert("style.stroke_color", "#ffffff").unwrap();
            m.insert("style.stroke_color.deeper", 1i64).unwrap();
        }),
        ("deep_custom", |m| {
            let mut v = LoroValue::from(1i64);
            for _ in 0..200 {
                v = LoroValue::from(vec![v]);
            }
            m.insert("custom", v).unwrap();
        }),
        ("container_where_value_expected", |m| {
            m.insert_container("x", loro::LoroMap::new()).unwrap();
        }),
    ];
    for (name, spoil) in spoilers {
        let mut s = base();
        let h = Hostile::of(&s);
        let m = h.good_rect("bad");
        spoil(&m);
        h.send(&mut s).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(s.get(&eid("bad")).is_none(), "{name}: element should be hidden");
        assert!(s.invalid_elements().any(|i| i.as_str() == "bad"), "{name}: reported");
        assert!(s.get(&eid("fine")).is_some(), "{name}");
        assert_view_matches_loro_ignoring_hidden(&s);
    }
}

fn assert_view_matches_loro_ignoring_hidden(s: &LoroStore) {
    let reopened = LoroStore::from_snapshot(&s.export_snapshot().unwrap()).unwrap();
    assert_same(s, &reopened);
}

#[test]
fn bad_point_lists_hide_the_line() {
    for (name, bytes) in
        [("odd_length", vec![0u8; 17]), ("not_pairs", vec![0u8; 24]), ("empty", vec![]), ("one_point", vec![0u8; 16])]
    {
        let mut s = base();
        let h = Hostile::of(&s);
        let m = h.good_rect("bad");
        m.insert("kind", "line").unwrap();
        m.insert("kind.points", bytes).unwrap();
        h.send(&mut s).unwrap();
        assert!(s.get(&eid("bad")).is_none(), "{name}");
    }
    // NaN coordinates.
    let mut s = base();
    let h = Hostile::of(&s);
    let m = h.good_rect("nan");
    m.insert("kind", "freehand").unwrap();
    let mut b = Vec::new();
    for v in [f64::NAN, 0.0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    m.insert("kind.points", b).unwrap();
    h.send(&mut s).unwrap();
    assert_hidden(&s, "nan");
}

#[test]
fn a_huge_point_list_is_refused_before_it_is_decoded() {
    let mut s = base();
    let h = Hostile::of(&s);
    let m = h.good_rect("huge");
    m.insert("kind", "freehand").unwrap();
    m.insert("kind.points", vec![0u8; 16 * 250_000]).unwrap();
    h.send(&mut s).unwrap();
    assert_hidden(&s, "huge");
}

#[test]
fn a_hidden_element_can_be_repaired_by_its_author() {
    let mut s = base();
    let h = Hostile::of(&s);
    let m = h.good_rect("fixme");
    m.insert("x", "oops").unwrap();
    h.send(&mut s).unwrap();
    assert_hidden(&s, "fixme");
    m.insert("x", 1.0).unwrap();
    let cs = h.send(&mut s).unwrap();
    assert_eq!(cs.get(&eid("fixme")).unwrap().kind, ChangeKind::Added);
    assert_eq!(s.invalid_elements().count(), 0);
}

#[test]
fn locally_inserting_over_a_hidden_element_replaces_it_cleanly() {
    let mut s = base();
    let h = Hostile::of(&s);
    let m = h.good_rect("clash");
    m.insert("x", "oops").unwrap();
    m.insert("junk", 1i64).unwrap();
    h.send(&mut s).unwrap();
    assert_hidden(&s, "clash");
    add(&mut s, vec![rect("clash", &zs(1)[0])]);
    assert!(s.get(&eid("clash")).is_some());
    assert_eq!(s.invalid_elements().count(), 0);
    assert!(s.debug_json()["elements"]["clash"].get("junk").is_none());
    assert_view_matches_loro(&s);
}

#[test]
fn bad_element_keys_and_non_container_entries_are_ignored() {
    let mut s = base();
    let h = Hostile::of(&s);
    let els = h.doc.get_map("elements");
    els.insert("plain_value", "just a string").unwrap();
    els.insert("not a valid id!", "x").unwrap();
    let long = "x".repeat(200);
    h.element(&long).insert("kind", "rectangle").unwrap();
    h.element("sp ace").insert("kind", "rectangle").unwrap();
    h.send(&mut s).unwrap();
    assert_eq!(s.len(), 1);
    // A local insert with an id that the plain value occupies still works.
    add(&mut s, vec![rect("plain_value", &zs(1)[0])]);
    assert!(s.get(&eid("plain_value")).is_some());
    assert_view_matches_loro(&s);
}

#[test]
fn fields_this_version_does_not_know_survive_local_edits() {
    let mut s = base();
    let h = Hostile::of(&s);
    h.good_rect("future").insert("future_field", "from a newer minor version").unwrap();
    h.send(&mut s).unwrap();
    edit(&mut s, "future", |e| e.transform.x = 1.0);
    edit(&mut s, "future", |e| e.deleted = true);
    assert_eq!(s.debug_json()["elements"]["future"]["future_field"], "from a newer minor version");
}

#[test]
fn files_must_hash_to_their_key() {
    let mut s = base();
    let h = Hostile::of(&s);
    let files = h.doc.get_map("files");
    let (good_id, good) = FileEntry::new("image/png", vec![1u8, 2, 3]);
    let entry = |mime: &str, bytes: Vec<u8>| {
        let mut m = std::collections::HashMap::new();
        m.insert("mime".to_owned(), LoroValue::from(mime));
        m.insert("bytes".to_owned(), LoroValue::from(bytes));
        LoroValue::from(m)
    };
    files.insert(good_id.as_str(), entry(&good.mime, good.bytes.to_vec())).unwrap();
    files.insert(&"a".repeat(64), entry("image/png", vec![9u8; 10])).unwrap(); // hash mismatch
    files.insert("short", entry("image/png", vec![9u8; 10])).unwrap(); // not a hash
    let (evil_id, _) = FileEntry::new("text/html", vec![4u8]);
    files.insert(evil_id.as_str(), entry("text/html", vec![4u8])).unwrap(); // not an image
    files.insert(&"b".repeat(64), LoroValue::from("not a map")).unwrap();
    h.send(&mut s).unwrap();
    assert_eq!(s.files().count(), 1);
    assert!(s.file(&good_id).is_some());
}

#[test]
fn a_bad_name_keeps_the_old_name() {
    let mut s = base();
    let h = Hostile::of(&s);
    h.doc.get_map("meta").insert("name", "x\u{0000}y").unwrap();
    h.doc.get_map("meta").insert("background", "javascript:alert(1)").unwrap();
    h.send(&mut s).unwrap();
    assert_eq!(s.meta().background, Color::WHITE);
    assert!(!s.meta().name.contains('\0'));
}

#[test]
fn a_newer_schema_is_refused_and_blocks_writes() {
    let mut s = base();
    let h = Hostile::of(&s);
    h.doc.get_map("meta").insert("schema", 99i64).unwrap();
    h.good_rect("from_the_future");
    let err = h.send(&mut s).unwrap_err();
    assert_eq!(err, DocError::NewerSchema { found: 99, max: 1 });
    assert_eq!(s.blocked_by_schema(), Some(99));
    assert!(err.to_string().contains("Update the app"));
    assert!(s.get(&eid("from_the_future")).is_none());
    // Further edits are refused instead of being written into a document we do not understand.
    let mut tx = s.begin();
    tx.update(&eid("fine"), |e| e.transform.x = 1.0).unwrap();
    assert!(tx.commit().is_err());
    assert_eq!(s.get(&eid("fine")).unwrap().transform.x, 10.0, "refused commit leaves the view as it was");
    // Opening such a document is refused too.
    assert!(matches!(
        LoroStore::from_snapshot(&h.doc.export(ExportMode::Snapshot).unwrap()),
        Err(DocError::NewerSchema { .. })
    ));
}

#[test]
fn input_over_the_size_limit_is_not_looked_at() {
    let mut s = LoroStore::with_config(LoroConfig { max_import_bytes: 64, ..LoroConfig::default() }).unwrap();
    let err = s.apply_remote(&[0u8; 65]).unwrap_err();
    assert!(matches!(err, DocError::TooLarge { size: 65, limit: 64 }));
    assert!(matches!(
        LoroStore::from_snapshot_with(&[0u8; 65], LoroConfig { max_import_bytes: 64, ..LoroConfig::default() }),
        Err(DocError::TooLarge { .. })
    ));
}

#[test]
fn snapshots_that_are_not_drafftink_documents_are_refused() {
    // The old app's collaboration snapshots use different roots and have no schema version.
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/legacy/crdt")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "loro") {
            let bytes = std::fs::read(&path).unwrap();
            let err = LoroStore::from_snapshot(&bytes).unwrap_err();
            assert!(matches!(err, DocError::NotADocument(_)), "{}: {err}", path.display());
            // Merged into a live store they add nothing visible and break nothing.
            let mut s = base();
            let _ = s.apply_remote(&bytes);
            assert_eq!(s.len(), 1);
            edit(&mut s, "fine", |e| e.transform.x = 3.0);
        }
    }
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    let mut s = base();
    for bytes in [&b""[..], b"x", b"loro", b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0", &[0xff; 300][..], b"{\"json\": true}"] {
        assert!(s.apply_remote(bytes).is_err(), "{bytes:?}");
        assert!(LoroStore::from_snapshot(bytes).is_err(), "{bytes:?}");
    }
    assert_eq!(s.len(), 1);
    edit(&mut s, "fine", |e| e.transform.x = 2.0);
    assert!(s.export_updates_since(b"nonsense").is_err());
    assert!(s.export_updates_since(&[0xff; 40]).is_err());
}

fn sample_updates() -> Vec<Vec<u8>> {
    let mut a = peer(5);
    let z = zs(4);
    add(&mut a, vec![rect("r1", &z[0]), text("t1", &z[1], "hello"), arrow("ar", &z[2])]);
    let first = a.export_updates_since(&[]).unwrap();
    edit(&mut a, "t1", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.content = "hello wörld".into();
            t.runs =
                vec![TextRun { start: 0, end: 3, color: Some(Color::parse("#ff0000").unwrap()), ..Default::default() }];
        }
    });
    let second = a.export_updates_since(&[]).unwrap();
    vec![
        first,
        second,
        a.export_snapshot().unwrap(),
        a.export_state_only().unwrap(),
        a.export_shallow_snapshot(0).unwrap(),
    ]
}

/// After any hostile input the store must still answer. If the CRDT library panicked on the input, the store says
/// so (`is_poisoned`), refuses edits with an error, keeps serving reads, and recovers on `replace_from_snapshot`.
fn assert_usable_or_cleanly_poisoned(s: &mut LoroStore, good_snapshot: &[u8]) {
    // A mutated update can raise `meta.schema`. That is a newer app talking: the store goes read-only with a clear
    // status, shows the last scene it understood, and recovers by replacing the document.
    if let Some(found) = s.blocked_by_schema() {
        assert!(found > 1);
        assert!(s.get(&eid("fine")).is_some(), "reads still work");
        let mut tx = s.begin();
        let _ = tx.update(&eid("fine"), |e| e.transform.x = 8.0);
        assert!(tx.commit().unwrap_err().to_string().contains("Update the app"));
        assert!(matches!(s.apply_remote(&[1, 2, 3]).unwrap_err(), DocError::NewerSchema { .. }));
        assert!(matches!(s.undo().unwrap_err(), DocError::NewerSchema { .. }));
        assert!(!s.is_poisoned());
        s.replace_from_snapshot(good_snapshot).unwrap();
        assert_eq!(s.blocked_by_schema(), None);
    }
    if s.is_poisoned() {
        assert!(s.get(&eid("fine")).is_some(), "reads still work");
        let mut tx = s.begin();
        let _ = tx.update(&eid("fine"), |e| e.transform.x = 8.0);
        assert!(tx.commit().unwrap_err().to_string().contains("internal error"));
        assert_eq!(s.apply_remote(&[1, 2, 3]).unwrap_err(), DocError::Poisoned);
        assert_eq!(s.undo().unwrap_err(), DocError::Poisoned);
        assert!(s.export_snapshot().is_err());
        // An empty vector would make a peer send the whole room to a store that cannot take it.
        assert_eq!(s.version_vector().unwrap_err(), DocError::Poisoned);
        s.replace_from_snapshot(good_snapshot).unwrap();
        assert!(!s.is_poisoned());
    }
    edit(s, "fine", |e| e.transform.x = 8.0);
    assert_eq!(s.get(&eid("fine")).unwrap().transform.x, 8.0);
    s.export_snapshot().unwrap();
    assert!(s.undo().unwrap().is_some());
}

/// Loro blobs carry an xxh32 checksum of everything after byte 20 in bytes 16..20. Recomputing it after a
/// mutation lets the damage reach the decoder instead of being rejected up front.
fn refresh_checksum(bytes: &mut [u8]) {
    if bytes.len() > 20 {
        let sum = xxhash_rust::xxh32::xxh32(&bytes[20..], u32::from_le_bytes(*b"LORO"));
        bytes[16..20].copy_from_slice(&sum.to_le_bytes());
    }
}

/// Inputs found by the mutation tests on which Loro 1.17 itself asserts. They must come back as errors, a poisoned
/// store must say so, and the store must recover.
#[test]
fn inputs_on_which_loro_panics_come_back_as_errors() {
    let updates = sample_updates();
    let mutate = |which: usize, pos: usize, val: u8, fix: bool| {
        let mut bytes = updates[which].clone();
        let at = if fix { 22 + pos % (bytes.len() - 22) } else { pos % bytes.len() };
        bytes[at] ^= val | 1;
        if fix {
            refresh_checksum(&mut bytes);
        }
        bytes
    };
    let cases = [mutate(1, 11_787_218_173_712_677_415, 32, false), mutate(1, 1_413_006_951_391_554_047, 48, true)];
    for bytes in cases {
        let mut s = base();
        let good = s.export_snapshot().unwrap();
        assert!(s.apply_remote(&bytes).is_err(), "a corrupt update is an error");
        assert!(LoroStore::from_snapshot(&bytes).is_err());
        eprintln!("poisoned after the corrupt update: {}", s.is_poisoned());
        assert_usable_or_cleanly_poisoned(&mut s, &good);
        drop(s); // dropping a poisoned store must not panic either
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2000)) {
        let mut s = base();
        let good = s.export_snapshot().unwrap();
        let _ = s.apply_remote(&bytes);
        let _ = LoroStore::from_snapshot(&bytes);
        assert_usable_or_cleanly_poisoned(&mut s, &good);
    }

    #[test]
    fn mutated_updates_never_panic(which in 0usize..5, flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..4), cut in proptest::option::of(any::<usize>())) {
        let updates = sample_updates();
        let mut bytes = updates[which % updates.len()].clone();
        for (pos, val) in flips {
            let at = pos % bytes.len();
            bytes[at] ^= val | 1;
        }
        if let Some(c) = cut {
            bytes.truncate(c % (bytes.len() + 1));
        }
        let mut s = base();
        let good = s.export_snapshot().unwrap();
        let _ = s.apply_remote(&bytes);
        let _ = LoroStore::from_snapshot(&bytes);
        assert_usable_or_cleanly_poisoned(&mut s, &good);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 600, ..ProptestConfig::default() })]

    #[test]
    fn mutated_updates_with_a_valid_checksum_never_panic(which in 0usize..5, flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..4)) {
        let updates = sample_updates();
        let mut bytes = updates[which % updates.len()].clone();
        for (pos, val) in flips {
            if bytes.len() > 22 {
                let at = 22 + pos % (bytes.len() - 22);
                bytes[at] ^= val | 1;
            }
        }
        refresh_checksum(&mut bytes);
        let mut s = base();
        let good = s.export_snapshot().unwrap();
        let _ = s.apply_remote(&bytes);
        let _ = LoroStore::from_snapshot(&bytes);
        assert_usable_or_cleanly_poisoned(&mut s, &good);
    }
}

#[test]
fn a_peer_id_that_loro_reserves_is_refused_not_replaced() {
    let r = LoroStore::with_config(LoroConfig { peer_id: Some(u64::MAX), ..LoroConfig::default() });
    assert_eq!(r.err(), Some(DocError::BadPeerId(u64::MAX)));
    let ok = LoroStore::with_config(LoroConfig { peer_id: Some(7), ..LoroConfig::default() }).unwrap();
    assert_eq!(ok.peer_id(), Ok(7));
}
