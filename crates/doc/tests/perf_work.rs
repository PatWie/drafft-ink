#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Work-counter tests of the Loro store at 1k, 10k and (in release builds) 100k elements: a local commit, a
//! remote update and opening a document do work in proportion to what changed (`WorkStats`), never to the size of
//! the scene. `cargo xtask perf` runs this file in release mode, which adds the 100k scene.

mod common;

use common::*;
use drafftink_doc::{LoroConfig, LoroStore};
use drafftink_scene::*;

/// Scene sizes. Debug builds stop at 10k: writing 100k elements into Loro unoptimized takes about a minute.
fn sizes() -> &'static [usize] {
    if cfg!(debug_assertions) { &[1_000, 10_000] } else { &[1_000, 10_000, 100_000] }
}

fn name(i: usize) -> String {
    format!("e{i:06}")
}

fn document(n: usize) -> Document {
    let z = zs(n);
    let mut doc = Document::new();
    doc.elements = (0..n).map(|i| rect(&name(i), &z[i])).collect();
    doc
}

fn replica(doc: &Document, peer_id: u64) -> LoroStore {
    let a = LoroStore::from_document(doc).unwrap();
    LoroStore::from_snapshot_with(
        &a.export_snapshot().unwrap(),
        LoroConfig { peer_id: Some(peer_id), ..LoroConfig::default() },
    )
    .unwrap()
}

#[test]
fn a_local_edit_writes_one_element_at_every_size() {
    let mut written = Vec::new();
    for &n in sizes() {
        let mut s = replica(&document(n), 1);
        s.reset_stats();
        edit(&mut s, &name(n / 2), |e| e.transform.x += 5.0);
        let st = s.stats();
        assert_eq!((st.commits, st.elements_written, st.elements_decoded), (1, 1, 0), "at {n}: {st:?}");
        assert_eq!(st.full_rebuilds, 0, "at {n}");
        written.push(st.fields_written);
    }
    assert!(written.windows(2).all(|w| w[0] == w[1]), "fields written by scene size: {written:?}");
}

#[test]
fn a_remote_update_of_one_element_decodes_one_element_at_every_size() {
    for &n in sizes() {
        let doc = document(n);
        let mut a = LoroStore::from_document(&doc).unwrap();
        let mut b = LoroStore::from_snapshot_with(
            &a.export_snapshot().unwrap(),
            LoroConfig { peer_id: Some(2), ..LoroConfig::default() },
        )
        .unwrap();
        edit(&mut a, &name(n / 3), |e| e.transform.y -= 7.0);
        let bytes = a.export_updates_since(&b.version_vector()).unwrap();
        b.reset_stats();
        let cs = b.apply_remote(&bytes).unwrap();
        let st = b.stats();
        assert_eq!(cs.elements.len(), 1, "at {n}");
        assert_eq!((st.elements_decoded, st.full_rebuilds, st.remote_updates), (1, 0, 1), "at {n}: {st:?}");
    }
}

#[test]
fn opening_a_snapshot_decodes_each_element_once() {
    for &n in sizes() {
        let bytes = LoroStore::from_document(&document(n)).unwrap().export_snapshot().unwrap();
        let s = LoroStore::from_snapshot(&bytes).unwrap();
        let st = s.stats();
        assert_eq!((st.elements_decoded, st.full_rebuilds, st.elements_written), (n as u64, 1, 0), "at {n}: {st:?}");
    }
}

#[test]
fn loading_a_document_writes_each_element_once_and_leaves_undo_working() {
    for &n in sizes() {
        let mut s = LoroStore::from_document(&document(n)).unwrap();
        let st = s.stats();
        assert_eq!((st.elements_written, st.commits, st.elements_decoded), (n as u64, 1, 0), "at {n}: {st:?}");
        assert!(!s.can_undo(), "the load is not an undo step at {n}");
        // The undo manager is attached after the load: the first edit is the first step.
        edit(&mut s, &name(1), |e| e.transform.x += 1.0);
        assert_eq!(s.undo_depth(), 1, "at {n}");
        s.undo().unwrap();
        assert_eq!(s.get(&eid(&name(1))).unwrap().transform.x, rect(&name(1), &zs(1)[0]).transform.x, "at {n}");
        assert!(!s.can_undo() && s.can_redo(), "at {n}");
    }
}
