#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Store behaviour beyond the conformance suite: scale, determinism, realistic edit sequences.

mod common;

use common::*;
use drafftink_scene::*;

#[test]
fn many_elements_in_one_transaction() {
    let n = 20_000;
    let z = zs(n);
    let mut s = MemoryStore::new();
    let mut tx = s.begin();
    for (i, k) in z.iter().enumerate().rev() {
        tx.insert(rect(&format!("e{i}"), k)).unwrap();
    }
    let cs = tx.commit().unwrap();
    assert_eq!(cs.elements.len(), n);
    assert_eq!(s.len(), n);
    let ids: Vec<String> = s.iter().map(|e| e.id.to_string()).collect();
    assert_eq!(ids.first().map(String::as_str), Some("e0"));
    assert_eq!(ids.last().map(String::as_str), Some("e19999"));
    assert!(s.iter().zip(s.iter().skip(1)).all(|(a, b)| a.index < b.index));
    assert!(!validate_scene(&s).has_errors());

    // Touch a few: the change set lists only those.
    let mut tx = s.begin();
    for i in [3usize, 4000, 19_999] {
        tx.update(&id(&format!("e{i}")), |e| e.transform.x += 1.0).unwrap();
    }
    let cs = tx.commit().unwrap();
    assert_eq!(cs.elements.len(), 3);
    assert!(cs.elements.iter().all(|c| c.fields == Fields::POSITION && c.version == 2));
}

#[test]
fn reorder_with_one_new_key_changes_one_element() {
    let z = zs(5);
    let mut s = MemoryStore::new();
    let mut tx = s.begin();
    for (i, k) in z.iter().enumerate() {
        tx.insert(rect(&format!("e{i}"), k)).unwrap();
    }
    tx.commit().unwrap();
    // Send e4 backward by one step: a key between e2 and e3.
    let (below, _) = s.z_neighbors(&id("e3"));
    let e3_key = s.get(&id("e3")).unwrap().index.clone();
    let new_key = ZIndex::between(below.as_ref(), Some(&e3_key)).unwrap();
    let mut tx = s.begin();
    tx.update(&id("e4"), |e| e.index = new_key.clone()).unwrap();
    let cs = tx.commit().unwrap();
    assert_eq!(cs.elements.len(), 1);
    assert!(cs.order_changed());
    let order: Vec<String> = s.iter().map(|e| e.id.to_string()).collect();
    assert_eq!(order, ["e0", "e1", "e2", "e4", "e3"]);
}

#[test]
fn group_then_ungroup_keeps_ids_and_updates_the_index() {
    let z = zs(3);
    let mut s = MemoryStore::new();
    let mut tx = s.begin();
    for (i, k) in z.iter().enumerate() {
        tx.insert(rect(&format!("e{i}"), k)).unwrap();
    }
    tx.commit().unwrap();
    let g = gid("g1");
    let mut tx = s.begin();
    for i in 0..2 {
        tx.update(&id(&format!("e{i}")), |e| e.group_ids.push(g.clone())).unwrap();
    }
    tx.commit().unwrap();
    assert!(s.derived().is_group(&g));
    assert!(!validate_scene(&s).has_errors());
    let mut tx = s.begin();
    for i in 0..2 {
        tx.update(&id(&format!("e{i}")), |e| e.group_ids.clear()).unwrap();
    }
    tx.commit().unwrap();
    assert_eq!(s.derived().group_len(&g), 0);
    assert_eq!(s.derived().group_count(), 0);
    assert_eq!(s.len(), 3, "ids and elements are untouched");
}

#[test]
fn deleting_a_target_leaves_the_arrow_binding_resolvable_as_a_warning() {
    let z = zs(2);
    let mut ar = arrow("ar", &z[1], &[(0.0, 0.0), (5.0, 5.0)]);
    set_end_binding(&mut ar, "a");
    let mut s = MemoryStore::from_document(&doc_of(vec![rect("a", &z[0]), ar])).unwrap();
    assert_eq!(s.derived().bound_arrows(&id("a")).count(), 1);
    let mut tx = s.begin();
    tx.update(&id("a"), |e| e.deleted = true).unwrap();
    tx.commit().unwrap();
    let report = validate_scene(&s);
    assert!(!report.has_errors());
    assert!(has_warning(&report, |e| matches!(e, DocError::BindingTargetDeleted { .. })));
    // The edit layer unbinds; afterwards the report is clean.
    let mut tx = s.begin();
    tx.update(&id("ar"), |e| {
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.end_binding = None;
        }
    })
    .unwrap();
    tx.commit().unwrap();
    assert!(validate_scene(&s).is_clean());
    assert_eq!(s.derived().bound_arrows(&id("a")).count(), 0);
}

#[test]
fn ids_come_from_the_injected_source_only() {
    let mut a = RandomIds(SplitMix64::new(42));
    let mut b = RandomIds(SplitMix64::new(42));
    for _ in 0..100 {
        assert_eq!(a.element_id(), b.element_id());
        assert_eq!(a.group_id(), b.group_id());
        assert_eq!(a.seed(), b.seed());
    }
    let mut seq = SeqIds::new();
    let z = ZIndex::first();
    let e1 = Element::new(seq.element_id(), ElementKind::Rectangle, z.clone());
    let e2 = Element::new(seq.element_id(), ElementKind::Rectangle, z);
    assert_eq!((e1.id.as_str(), e2.id.as_str()), ("e1", "e2"));
}

#[test]
fn versions_are_cache_keys() {
    let z = ZIndex::first();
    let mut s = MemoryStore::new();
    let mut tx = s.begin();
    tx.insert(rect("a", &z)).unwrap();
    tx.commit().unwrap();
    let mut seen = vec![s.get(&id("a")).unwrap().version];
    for i in 0..5 {
        let mut tx = s.begin();
        tx.update(&id("a"), |e| e.transform.y = i as f64 + 100.0).unwrap();
        tx.commit().unwrap();
        seen.push(s.get(&id("a")).unwrap().version);
    }
    assert_eq!(seen, [1, 2, 3, 4, 5, 6]);
}

#[test]
fn a_version_at_the_maximum_does_not_overflow_on_update() {
    // A hostile document can carry any `version`; the next edit must not panic or wrap to 0.
    let z = ZIndex::first();
    let mut s = MemoryStore::new();
    let mut e = rect("a", &z);
    e.version = u64::MAX;
    let mut tx = s.begin();
    tx.insert(e).unwrap();
    tx.commit().unwrap();
    let mut tx = s.begin();
    tx.update(&id("a"), |e| e.transform.x = 5.0).unwrap();
    let cs = tx.commit().unwrap();
    assert_eq!(cs.get(&id("a")).unwrap().version, u64::MAX);
    assert_eq!(s.get(&id("a")).unwrap().version, u64::MAX);
}

#[test]
fn a_rejected_file_says_which_rule_it_broke() {
    use drafftink_scene::limits::{MAX_FILE_BYTES, MAX_FILES};
    let put = |s: &mut MemoryStore, mime: &str, bytes: Vec<u8>| {
        let mut tx = s.begin();
        tx.put_file(FileEntry::new(mime, bytes).1)
    };
    let mut s = MemoryStore::new();
    assert_eq!(put(&mut s, "text/html", vec![1]), Err(FileRejection::MediaType("text/html".into()).into()));
    assert_eq!(
        put(&mut s, "image/png", vec![0; MAX_FILE_BYTES + 1]),
        Err(FileRejection::TooLarge { len: MAX_FILE_BYTES + 1, max: MAX_FILE_BYTES }.into())
    );
    for i in 0..MAX_FILES {
        let mut tx = s.begin();
        tx.put_file(FileEntry::new("image/png", i.to_le_bytes().to_vec()).1).unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        put(&mut s, "image/png", vec![9, 9, 9, 9, 9, 9, 9, 9, 9]),
        Err(FileRejection::TooMany { max: MAX_FILES }.into())
    );
}
