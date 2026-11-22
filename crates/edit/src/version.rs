//! Restoring a named version: the scene of the version becomes the current scene.
//!
//! A version holds every element it had, tombstones included (`drafftink_scene::NamedVersion`). A restore is an
//! ordinary edit in the open document, so it undoes in one step and, in a room, syncs like any other edit:
//!
//! * an element the version holds takes the value the version has, and comes back when it is deleted now;
//! * a live element the version does not hold is deleted (a tombstone, as `element.delete` makes);
//! * the background is the version's.
//!
//! Nothing else changes: not the document name, the picture blobs, the other versions or the selection (the command
//! drops selected ids that are gone). The elements of the version are consistent among themselves, and everything
//! outside it ends up deleted, so the restored scene needs no repair.

use std::collections::HashSet;

use drafftink_scene::{Element, ElementId, ElementKind, FileId, SceneTx, StoreError, VersionScene};

/// What a restore changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restored {
    /// Elements of the version that did not exist, or were deleted, and are live again.
    pub brought_back: usize,
    /// Elements of the version that changed to the value the version has.
    pub changed: usize,
    /// Live elements the version does not have, now deleted.
    pub deleted: usize,
    /// True when the background changed.
    pub background: bool,
    /// Pictures the version shows whose blobs the document no longer holds. The elements are restored, and show
    /// as missing pictures.
    pub missing_files: Vec<FileId>,
}

impl Restored {
    /// True when the scene already was the scene of the version.
    pub fn is_empty(&self) -> bool {
        self.brought_back == 0 && self.changed == 0 && self.deleted == 0 && !self.background
    }
}

/// `a` and `b` are the same element, apart from the store's change counter.
fn same(a: &Element, b: &Element) -> bool {
    let mut b = b.clone();
    b.version = a.version;
    *a == b
}

/// Makes the scene of `tx` the scene of `version`. Writes nothing for what already matches.
///
/// # Errors
/// A store refusal. The caller rolls the transaction back, so a failed restore leaves the document as it was.
pub fn restore<T: SceneTx>(tx: &mut T, version: &VersionScene) -> Result<Restored, StoreError> {
    let mut out = Restored::default();
    if tx.meta().background != version.background {
        tx.set_background(version.background);
        out.background = true;
    }
    let keep: HashSet<&ElementId> = version.elements.iter().map(|e| &e.id).collect();
    let live_now: Vec<ElementId> = tx.iter().filter(|e| !keep.contains(&e.id)).map(|e| e.id.clone()).collect();
    for id in &live_now {
        tx.update(id, |e| e.deleted = true)?;
        out.deleted += 1;
    }
    for wanted in &version.elements {
        match tx.get(&wanted.id) {
            Some(now) if same(now, wanted) => continue,
            Some(now) => {
                if now.deleted && !wanted.deleted {
                    out.brought_back += 1;
                } else {
                    out.changed += 1;
                }
                let wanted = wanted.clone();
                tx.update(&wanted.id.clone(), |e| {
                    let counter = e.version;
                    *e = wanted;
                    e.version = counter;
                })?;
            }
            None => {
                if !wanted.deleted {
                    out.brought_back += 1;
                }
                tx.insert(wanted.clone())?;
            }
        }
    }
    let mut missing: Vec<FileId> = version
        .elements
        .iter()
        .filter(|e| !e.deleted)
        .filter_map(|e| match &e.kind {
            ElementKind::Image(i) => i.file_id.clone(),
            _ => None,
        })
        .filter(|f| tx.file(f).is_none())
        .collect();
    missing.sort();
    missing.dedup();
    out.missing_files = missing;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use drafftink_scene::{
        Color, IdSource, MemoryStore, NamedVersion, SceneStore, SeqIds, Transform, ZIndex, snapshot,
    };

    use super::*;

    fn rect(ids: &mut SeqIds, z: &ZIndex, x: f64) -> Element {
        let mut e = Element::new(ids.element_id(), ElementKind::Rectangle, z.clone());
        e.transform = Transform::new(x, 0.0, 10.0, 10.0);
        e
    }

    fn add(store: &mut MemoryStore, e: Element) {
        let mut tx = store.begin();
        tx.insert(e).unwrap();
        tx.commit().unwrap();
    }

    fn capture(store: &MemoryStore, name: &str) -> VersionScene {
        NamedVersion::capture(SeqIds::new().version_id(), name, 0, None, store).unwrap().open().unwrap()
    }

    fn run(store: &mut MemoryStore, scene: &VersionScene) -> Restored {
        let mut tx = store.begin();
        let r = restore(&mut tx, scene).unwrap();
        tx.commit().unwrap();
        r
    }

    fn live_ids(store: &MemoryStore) -> Vec<String> {
        store.iter().map(|e| e.id.to_string()).collect()
    }

    #[test]
    fn a_restore_puts_back_what_was_deleted_undoes_what_was_added_and_resets_what_moved() {
        let mut ids = SeqIds::new();
        let z = ZIndex::rebalance(3).unwrap();
        let mut s = MemoryStore::new();
        add(&mut s, rect(&mut ids, &z[0], 0.0));
        add(&mut s, rect(&mut ids, &z[1], 20.0));
        let scene = capture(&s, "two");
        // Later: e1 moves, e2 is deleted, e3 is added, the background changes.
        let mut tx = s.begin();
        tx.update(&ElementId::new("e1").unwrap(), |e| e.transform.x = 500.0).unwrap();
        tx.update(&ElementId::new("e2").unwrap(), |e| e.deleted = true).unwrap();
        tx.insert(rect(&mut ids, &z[2], 40.0)).unwrap();
        tx.set_background(Color::rgb(1, 2, 3));
        tx.commit().unwrap();

        let r = run(&mut s, &scene);
        assert_eq!((r.brought_back, r.changed, r.deleted, r.background), (1, 1, 1, true), "{r:?}");
        assert_eq!(live_ids(&s), vec!["e1", "e2"]);
        assert_eq!(s.get(&ElementId::new("e1").unwrap()).unwrap().transform.x, 0.0);
        assert!(
            s.get(&ElementId::new("e3").unwrap()).unwrap().deleted,
            "an element made later is deleted, not removed"
        );
        assert_eq!(s.meta().background, scene.background);
    }

    #[test]
    fn restoring_the_current_scene_changes_nothing() {
        let mut ids = SeqIds::new();
        let mut s = MemoryStore::new();
        add(&mut s, rect(&mut ids, &ZIndex::first(), 0.0));
        let scene = capture(&s, "same");
        let rev = s.revision();
        let r = run(&mut s, &scene);
        assert!(r.is_empty(), "{r:?}");
        assert_eq!(s.revision(), rev, "no commit, no revision");
    }

    #[test]
    fn a_restore_leaves_the_name_the_other_versions_and_the_blobs_alone() {
        let mut ids = SeqIds::new();
        let mut s = MemoryStore::new();
        add(&mut s, rect(&mut ids, &ZIndex::first(), 0.0));
        let scene = capture(&s, "a");
        let keep = NamedVersion::capture(ids.version_id(), "keep", 5, None, &s).unwrap();
        let mut tx = s.begin();
        tx.put_version(keep.clone()).unwrap();
        tx.set_name("Renamed document").unwrap();
        tx.commit().unwrap();
        run(&mut s, &scene);
        let doc = snapshot(&s);
        assert_eq!((doc.name.as_str(), doc.versions), ("Renamed document", vec![keep]));
    }
}
