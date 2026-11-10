//! Conformance suite for [`SceneStore`] implementations.
//!
//! Every store (the in-memory one here, the Loro-backed one in `doc`) must pass the same checks. Each check gets a
//! fresh, empty store and returns an error text instead of panicking, so the suite itself obeys the no-panic rule.
//!
//! ```ignore
//! #[test]
//! fn my_store_conforms() {
//!     if let Err(failures) = drafftink_scene::conformance::run(MyStore::new) {
//!         panic!("{}", failures.join("\n"));
//!     }
//! }
//! ```
//!
//! Available in this crate's own tests and behind the `conformance` feature.

use std::cell::RefCell;
use std::rc::Rc;

use smallvec::smallvec;

use crate::color::Color;
use crate::derived::DerivedIndex;
use crate::document::{Document, FileEntry};
use crate::element::{BindMode, Binding, Element, ElementKind, Kind, LinearData, TextData, Transform};
use crate::id::{ElementId, GroupId};
use crate::store::{
    ChangeKind, ChangeSet, Fields, SceneRead, SceneStore, SceneTx, StoreError, load_document, snapshot,
};
use crate::validate::validate_scene;
use crate::zindex::ZIndex;
use kurbo::Point;

/// One named check.
pub struct Check<S> {
    /// Name, for failure messages.
    pub name: &'static str,
    /// The check. It owns a fresh empty store.
    pub run: fn(S) -> Result<(), String>,
}

macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        if !($cond) {
            return Err(format!($($arg)*));
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr, $what:expr) => {{
        let (l, r) = (&$left, &$right);
        if l != r {
            return Err(format!("{}: expected {:?}, got {:?}", $what, r, l));
        }
    }};
}

fn e2s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn eid(s: &str) -> Result<ElementId, String> {
    ElementId::new(s).map_err(e2s)
}

fn gid(s: &str) -> Result<GroupId, String> {
    GroupId::new(s).map_err(e2s)
}

fn zs(n: usize) -> Result<Vec<ZIndex>, String> {
    ZIndex::rebalance(n).map_err(e2s)
}

fn rect(id: &str, z: &ZIndex) -> Result<Element, String> {
    let mut e = Element::new(eid(id)?, ElementKind::Rectangle, z.clone());
    e.transform = Transform::new(10.0, 20.0, 100.0, 50.0);
    Ok(e)
}

fn arrow(id: &str, z: &ZIndex, start: Option<&str>, end: Option<&str>) -> Result<Element, String> {
    let mut e = Element::linear(eid(id)?, Kind::Arrow, z.clone(), &[Point::new(0.0, 0.0), Point::new(50.0, 20.0)])
        .ok_or("could not build an arrow")?;
    if let ElementKind::Arrow(d) = &mut e.kind {
        let bind = |t: &str| -> Result<Binding, String> {
            Ok(Binding { target: eid(t)?, mode: BindMode::Orbit, fixed_point: [0.5, 0.5] })
        };
        d.start_binding = start.map(bind).transpose()?;
        d.end_binding = end.map(bind).transpose()?;
    }
    Ok(e)
}

fn label(id: &str, z: &ZIndex, container: &str) -> Result<Element, String> {
    let mut t = TextData::new("hi");
    t.container_id = Some(eid(container)?);
    Ok(Element::new(eid(id)?, ElementKind::Text(t), z.clone()))
}

fn commit_all<S: SceneStore>(s: &mut S, elements: Vec<Element>) -> Result<ChangeSet, String> {
    let mut tx = s.begin();
    for e in elements {
        tx.insert(e).map_err(e2s)?;
    }
    tx.commit().map_err(e2s)
}

fn order<R: SceneRead>(s: &R) -> Vec<String> {
    s.iter().map(|e| e.id.to_string()).collect()
}

fn derived_matches_rebuild<S: SceneStore>(s: &S) -> Result<(), String> {
    let mut fresh = DerivedIndex::new();
    fresh.rebuild(s);
    ensure!(s.derived() == &fresh, "the incrementally maintained derived index differs from a rebuild");
    Ok(())
}

fn empty_store<S: SceneStore>(s: S) -> Result<(), String> {
    ensure_eq!(s.len(), 0, "len");
    ensure_eq!(s.len_all(), 0, "len_all");
    ensure!(s.is_empty(), "is_empty");
    ensure!(s.iter().next().is_none(), "iter is empty");
    ensure!(s.get(&eid("nope")?).is_none(), "get of an unknown id");
    ensure_eq!(s.revision(), 0, "revision");
    ensure!(s.top_z().is_none() && s.bottom_z().is_none(), "no z keys");
    ensure_eq!(s.meta().background, Color::WHITE, "default background");
    ensure!(!s.meta().name.is_empty(), "default name");
    ensure!(s.files().next().is_none(), "no files");
    Ok(())
}

fn insert_commit_and_order<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(3)?;
    // Insert out of z-order; iteration must be back to front.
    let mut tx = s.begin();
    tx.insert(rect("c", &z[2])?).map_err(e2s)?;
    tx.insert(rect("a", &z[0])?).map_err(e2s)?;
    tx.insert(rect("b", &z[1])?).map_err(e2s)?;
    ensure_eq!(order(&tx), vec!["a", "b", "c"], "order inside the transaction (read your writes)");
    ensure_eq!(tx.len(), 3, "len inside the transaction");
    let cs = tx.commit().map_err(e2s)?;
    ensure_eq!(order(&s), vec!["a", "b", "c"], "order after commit");
    ensure_eq!(cs.revision, 1, "revision in the change set");
    ensure_eq!(s.revision(), 1, "store revision");
    ensure_eq!(cs.elements.len(), 3, "changes");
    ensure!(cs.elements.iter().all(|c| c.kind == ChangeKind::Added && c.version == 1), "all added at version 1");
    ensure!(cs.elements.windows(2).all(|w| w[0].id < w[1].id), "changes sorted by id");
    ensure_eq!(s.top_z(), Some(z[2].clone()), "top_z");
    ensure_eq!(s.bottom_z(), Some(z[0].clone()), "bottom_z");
    ensure_eq!(s.z_neighbors(&eid("b")?), (Some(z[0].clone()), Some(z[2].clone())), "z_neighbors of the middle");
    ensure_eq!(s.z_neighbors(&eid("a")?), (None, Some(z[1].clone())), "z_neighbors of the bottom");
    ensure_eq!(s.z_neighbors(&eid("zz")?), (None, None), "z_neighbors of an unknown id");
    let top = s.next_top_index().map_err(e2s)?;
    ensure!(top > z[2], "next_top_index is above everything");
    derived_matches_rebuild(&s)
}

fn ties_break_by_id<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(1)?;
    commit_all(&mut s, vec![rect("b", &z[0])?, rect("a", &z[0])?, rect("c", &z[0])?])?;
    ensure_eq!(order(&s), vec!["a", "b", "c"], "equal keys sort by id");
    Ok(())
}

fn rollback_restores_everything<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(3)?;
    commit_all(&mut s, vec![rect("a", &z[0])?, rect("b", &z[1])?])?;
    let before = snapshot(&s);
    let rev = s.revision();
    {
        let mut tx = s.begin();
        tx.insert(rect("c", &z[2])?).map_err(e2s)?;
        tx.update(&eid("a")?, |e| e.transform.x = 99.0).map_err(e2s)?;
        tx.remove(&eid("b")?).map_err(e2s)?;
        tx.set_name("renamed").map_err(e2s)?;
        tx.set_background(Color::BLACK);
        tx.put_file(FileEntry::new("image/png", vec![1, 2, 3]).1).map_err(e2s)?;
        ensure_eq!(order(&tx), vec!["a", "c"], "writes visible inside the transaction");
    }
    ensure_eq!(snapshot(&s), before, "dropping a transaction restores the scene");
    ensure_eq!(s.revision(), rev, "dropping a transaction keeps the revision");
    let tx = s.begin();
    tx.rollback();
    ensure_eq!(snapshot(&s), before, "explicit rollback");
    derived_matches_rebuild(&s)
}

fn update_reports_changed_fields<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(2)?;
    commit_all(&mut s, vec![rect("a", &z[0])?])?;
    let id = eid("a")?;

    let mut tx = s.begin();
    tx.update(&id, |e| e.transform.x += 5.0).map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    let c = cs.get(&id).ok_or("no change for a")?;
    ensure_eq!((c.kind, c.fields, c.version), (ChangeKind::Updated, Fields::POSITION, 2), "position change");

    let mut tx = s.begin();
    tx.update(&id, |e| {
        e.style.opacity = 0.5;
        e.locked = true;
        e.name = Some("box".into());
        e.index = z[1].clone();
    })
    .map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    let c = cs.get(&id).ok_or("no change for a")?;
    ensure_eq!(c.fields, Fields::STYLE | Fields::LOCKED | Fields::NAME | Fields::INDEX, "several fields");
    ensure_eq!(c.version, 3, "version counts changes, not fields");
    ensure!(cs.order_changed(), "order_changed after an index change");

    // A no-op update is not a change: no revision, no version bump, no notification.
    let rev = s.revision();
    let mut tx = s.begin();
    tx.update(&id, |e| e.transform.x += 0.0).map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    ensure!(cs.is_empty(), "an update to the same value reports nothing");
    ensure_eq!(s.revision(), rev, "revision unchanged");
    ensure_eq!(s.get(&id).map(|e| e.version), Some(3), "version unchanged");

    let mut tx = s.begin();
    tx.update(&id, |e| e.kind = ElementKind::Diamond).map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    ensure_eq!(cs.get(&id).map(|c| c.fields), Some(Fields::KIND), "kind change");
    ensure!(Fields::KIND.affects_paint() && Fields::KIND.affects_geometry(), "kind affects paint and geometry");
    ensure!(!Fields::NAME.affects_paint(), "a name does not affect paint");
    Ok(())
}

fn changes_are_net<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(3)?;
    commit_all(&mut s, vec![rect("keep", &z[0])?])?;
    let rev = s.revision();

    // insert + update in one transaction is one Added.
    let mut tx = s.begin();
    tx.insert(rect("n", &z[1])?).map_err(e2s)?;
    tx.update(&eid("n")?, |e| e.transform.w = 7.0).map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    ensure_eq!(cs.get(&eid("n")?).map(|c| c.kind), Some(ChangeKind::Added), "insert then update");
    ensure_eq!(cs.elements.len(), 1, "only n changed");

    // insert + remove vanishes.
    let mut tx = s.begin();
    tx.insert(rect("tmp", &z[2])?).map_err(e2s)?;
    tx.remove(&eid("tmp")?).map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    ensure!(cs.is_empty(), "insert then remove is no change");

    // update and revert vanishes.
    let rev2 = s.revision();
    let mut tx = s.begin();
    tx.update(&eid("keep")?, |e| e.transform.x = 1234.0).map_err(e2s)?;
    tx.update(&eid("keep")?, |e| e.transform.x = 10.0).map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    ensure!(cs.is_empty(), "update then revert is no change");
    ensure_eq!(s.revision(), rev2, "no revision for a net-empty commit");

    // remove and re-insert the same id is an update.
    let mut tx = s.begin();
    tx.remove(&eid("keep")?).map_err(e2s)?;
    let mut again = rect("keep", &z[0])?;
    again.transform.y = 77.0;
    tx.insert(again).map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    let c = cs.get(&eid("keep")?).ok_or("no change for keep")?;
    ensure_eq!((c.kind, c.fields), (ChangeKind::Updated, Fields::POSITION), "remove then insert");
    ensure!(rev2 > rev, "revision moved earlier");
    derived_matches_rebuild(&s)
}

fn tombstones<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(2)?;
    commit_all(&mut s, vec![rect("a", &z[0])?, rect("b", &z[1])?])?;
    let mut tx = s.begin();
    tx.update(&eid("a")?, |e| e.deleted = true).map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    ensure_eq!(cs.get(&eid("a")?).map(|c| c.fields), Some(Fields::DELETED), "tombstone is an update of `deleted`");
    ensure_eq!(order(&s), vec!["b"], "iter skips tombstones");
    ensure_eq!(s.iter_all().count(), 2, "iter_all keeps tombstones");
    ensure_eq!((s.len(), s.len_all()), (1, 2), "len counts live, len_all counts all");
    ensure!(s.get(&eid("a")?).is_some_and(|e| e.deleted), "get returns the tombstone");
    ensure_eq!(s.top_z(), Some(z[1].clone()), "tombstones take part in z keys");
    let mut tx = s.begin();
    tx.update(&eid("a")?, |e| e.deleted = false).map_err(e2s)?;
    tx.commit().map_err(e2s)?;
    ensure_eq!(order(&s), vec!["a", "b"], "undelete");
    Ok(())
}

fn refusals_leave_the_store_unchanged<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(2)?;
    commit_all(&mut s, vec![rect("a", &z[0])?])?;
    let before = snapshot(&s);
    let mut tx = s.begin();
    ensure!(matches!(tx.insert(rect("a", &z[1])?), Err(StoreError::AlreadyExists(_))), "duplicate id");
    ensure!(matches!(tx.update(&eid("zz")?, |_| {}), Err(StoreError::NotFound(_))), "update of unknown id");
    ensure!(matches!(tx.remove(&eid("zz")?), Err(StoreError::NotFound(_))), "remove of unknown id");
    ensure!(
        matches!(
            tx.update(&eid("a")?, |e| e.id = ElementId::new("other").unwrap_or_else(|_| e.id.clone())),
            Err(StoreError::IdChanged(_))
        ),
        "id change"
    );
    ensure!(
        matches!(tx.update(&eid("a")?, |e| e.transform.x = f64::NAN), Err(StoreError::Invalid { .. })),
        "non-finite update"
    );
    let mut bad = rect("b", &z[1])?;
    bad.style.opacity = 7.0;
    ensure!(matches!(tx.insert(bad), Err(StoreError::Invalid { .. })), "invalid insert");
    ensure!(matches!(tx.set_name(""), Err(StoreError::BadName)), "empty name");
    ensure!(matches!(tx.put_file(FileEntry::new("text/html", vec![1]).1), Err(StoreError::BadFile(_))), "bad mime");
    ensure_eq!(tx.get(&eid("a")?).map(|e| e.transform.x), Some(10.0), "a keeps its value after refused updates");
    let cs = tx.commit().map_err(e2s)?;
    ensure!(cs.is_empty(), "refused operations leave nothing to commit");
    ensure_eq!(snapshot(&s), before, "store unchanged");
    Ok(())
}

fn subscribers<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(2)?;
    let seen: Rc<RefCell<Vec<ChangeSet>>> = Rc::default();
    let sink = Rc::clone(&seen);
    let sub = s.subscribe(Box::new(move |cs| sink.borrow_mut().push(cs.clone())));
    let cs1 = commit_all(&mut s, vec![rect("a", &z[0])?])?;
    let empty = s.begin().commit().map_err(e2s)?;
    ensure!(empty.is_empty(), "an empty commit is empty");
    ensure_eq!(seen.borrow().len(), 1, "one notification, none for the empty commit");
    let first = seen.borrow().first().cloned();
    ensure_eq!(first, Some(cs1), "the subscriber receives the returned change set");
    {
        let mut tx = s.begin();
        tx.insert(rect("dropped", &z[1])?).map_err(e2s)?;
    }
    ensure_eq!(seen.borrow().len(), 1, "no notification for a rollback");
    ensure!(s.unsubscribe(sub), "unsubscribe");
    ensure!(!s.unsubscribe(sub), "unsubscribe twice");
    commit_all(&mut s, vec![rect("b", &z[1])?])?;
    ensure_eq!(seen.borrow().len(), 1, "no notification after unsubscribe");
    Ok(())
}

fn derived_indexes<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(8)?;
    let (g1, g2) = (gid("g1")?, gid("g2")?);
    let mut a = rect("a", &z[0])?;
    let mut b = rect("b", &z[1])?;
    let mut c = rect("c", &z[2])?;
    a.group_ids = smallvec![g1.clone(), g2.clone()];
    b.group_ids = smallvec![g1.clone(), g2.clone()];
    c.group_ids = smallvec![g2.clone()];
    let mut frame = Element::new(eid("f")?, ElementKind::Frame(Default::default()), z[7].clone());
    frame.transform = Transform::new(0.0, 0.0, 500.0, 500.0);
    a.frame_id = Some(eid("f")?);
    let ar = arrow("ar", &z[3], Some("a"), Some("b"))?;
    let lb = label("lb", &z[4], "a")?;
    commit_all(&mut s, vec![a, b, c, frame, ar, lb])?;

    let d = s.derived();
    let ar = eid("ar")?;
    ensure_eq!(d.bound_arrows(&eid("a")?).collect::<Vec<_>>(), vec![&ar], "arrows bound to a");
    ensure_eq!(d.bound_arrows(&eid("b")?).count(), 1, "arrows bound to b");
    ensure_eq!(d.bound_arrows(&eid("c")?).count(), 0, "arrows bound to c");
    ensure_eq!(d.group_members(&g1).count(), 2, "members of g1");
    ensure_eq!(d.group_members(&g2).count(), 3, "members of g2");
    ensure!(d.is_group(&g1) && d.is_group(&g2), "both are groups");
    let a_id = eid("a")?;
    ensure_eq!(d.frame_children(&eid("f")?).collect::<Vec<_>>(), vec![&a_id], "children of f");
    ensure_eq!(d.label_of(&eid("a")?), Some(&eid("lb")?), "label of a");
    ensure_eq!(d.label_of(&eid("b")?), None, "label of b");
    derived_matches_rebuild(&s)?;

    // Rebind, regroup, move to no frame, delete the label: the index follows.
    let mut tx = s.begin();
    tx.update(&eid("ar")?, |e| {
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.start_binding = None;
            if let Some(b) = &mut d.end_binding {
                b.target = ElementId::new("c").unwrap_or_else(|_| b.target.clone());
            }
        }
    })
    .map_err(e2s)?;
    tx.update(&eid("b")?, |e| e.group_ids.clear()).map_err(e2s)?;
    tx.update(&eid("a")?, |e| e.frame_id = None).map_err(e2s)?;
    tx.update(&eid("lb")?, |e| e.deleted = true).map_err(e2s)?;
    tx.commit().map_err(e2s)?;
    let d = s.derived();
    ensure_eq!(d.bound_arrows(&eid("a")?).count(), 0, "a no longer has an arrow");
    ensure_eq!(d.bound_arrows(&eid("b")?).count(), 0, "b no longer has an arrow");
    ensure_eq!(d.bound_arrows(&eid("c")?).count(), 1, "c has the arrow now");
    ensure!(!d.is_group(&g1), "g1 shrank to one member and counts as no group");
    ensure_eq!(d.group_members(&g2).count(), 2, "g2 lost b");
    ensure_eq!(d.frame_children(&eid("f")?).count(), 0, "f has no children");
    ensure_eq!(d.label_of(&eid("a")?), None, "a deleted label is not indexed");
    derived_matches_rebuild(&s)?;

    // Deleting the arrow, a hard remove of a member.
    let mut tx = s.begin();
    tx.update(&eid("ar")?, |e| e.deleted = true).map_err(e2s)?;
    tx.remove(&eid("c")?).map_err(e2s)?;
    tx.commit().map_err(e2s)?;
    ensure_eq!(s.derived().bound_arrows(&eid("c")?).count(), 0, "a deleted arrow is not indexed");
    ensure_eq!(s.derived().group_members(&g2).count(), 1, "a removed member leaves its groups");
    derived_matches_rebuild(&s)
}

fn meta_and_files<S: SceneStore>(mut s: S) -> Result<(), String> {
    let (id, entry) = FileEntry::new("image/png", vec![137, 80, 78, 71]);
    let mut tx = s.begin();
    tx.set_name("Plan").map_err(e2s)?;
    tx.set_background(Color::rgb(250, 250, 250));
    let put = tx.put_file(entry.clone()).map_err(e2s)?;
    ensure_eq!(put, id, "put_file returns the content hash");
    ensure_eq!(tx.put_file(entry.clone()).map_err(e2s)?, id, "putting the same bytes again");
    ensure_eq!(tx.meta().name.as_str(), "Plan", "meta visible inside the transaction");
    ensure!(tx.file(&id).is_some(), "file visible inside the transaction");
    let cs = tx.commit().map_err(e2s)?;
    ensure!(cs.meta && cs.files == vec![id.clone()] && cs.elements.is_empty(), "meta and file reported: {cs:?}");
    ensure_eq!(s.meta().name.as_str(), "Plan", "name");
    ensure_eq!(s.meta().background, Color::rgb(250, 250, 250), "background");
    ensure_eq!(s.file(&id), Some(&entry), "file");

    let mut tx = s.begin();
    tx.set_name("Plan").map_err(e2s)?;
    let cs = tx.commit().map_err(e2s)?;
    ensure!(cs.is_empty(), "setting the same name is not a change");

    let mut tx = s.begin();
    ensure!(tx.remove_file(&id), "remove_file");
    ensure!(!tx.remove_file(&id), "remove_file twice");
    let cs = tx.commit().map_err(e2s)?;
    ensure_eq!(cs.files, vec![id.clone()], "removal reported");
    ensure!(s.file(&id).is_none(), "file gone");
    Ok(())
}

fn document_round_trip<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(3)?;
    let (fid, entry) = FileEntry::new("image/png", vec![9, 9, 9]);
    let mut img = Element::new(
        eid("img")?,
        ElementKind::Image(crate::element::ImageData {
            file_id: Some(fid.clone()),
            natural_width: 4,
            natural_height: 3,
            crop: None,
        }),
        z[2].clone(),
    );
    img.transform = Transform::new(0.0, 0.0, 40.0, 30.0);
    let mut doc = Document::new();
    doc.name = "Round trip".into();
    doc.background = Color::rgb(1, 2, 3);
    doc.elements = vec![rect("a", &z[0])?, arrow("ar", &z[1], Some("a"), None)?, img];
    doc.files.insert(fid, entry);
    load_document(&mut s, &doc).map_err(e2s)?;
    ensure_eq!(order(&s), vec!["a", "ar", "img"], "loaded order");
    let mut back = snapshot(&s);
    for e in &mut back.elements {
        e.version = 0;
    }
    ensure_eq!(back, doc, "snapshot equals the loaded document");
    ensure!(!validate_scene(&s).has_errors(), "a loaded document validates");
    derived_matches_rebuild(&s)?;

    // Loading again replaces the content, keeps versions moving forward for unchanged elements, and an invalid
    // document is refused without touching the store.
    let before = snapshot(&s);
    let mut bad = doc.clone();
    bad.elements.push(rect("a", &z[2])?);
    ensure!(load_document(&mut s, &bad).is_err(), "duplicate ids are refused");
    ensure_eq!(snapshot(&s), before, "refused load leaves the store unchanged");

    let mut smaller = Document::new();
    smaller.elements = vec![rect("only", &z[0])?];
    load_document(&mut s, &smaller).map_err(e2s)?;
    ensure_eq!(order(&s), vec!["only"], "load replaces everything");
    ensure!(s.files().next().is_none(), "load replaces the files");
    derived_matches_rebuild(&s)
}

fn same_document_loaded_twice_changes_nothing<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(2)?;
    let mut doc = Document::new();
    doc.elements = vec![rect("a", &z[0])?, rect("b", &z[1])?];
    load_document(&mut s, &doc).map_err(e2s)?;
    let versions: Vec<u64> = s.iter().map(|e| e.version).collect();
    let rev = s.revision();
    let cs = load_document(&mut s, &doc).map_err(e2s)?;
    ensure!(cs.is_empty(), "loading identical content reports nothing: {cs:?}");
    ensure_eq!(s.revision(), rev, "revision");
    ensure_eq!(s.iter().map(|e| e.version).collect::<Vec<_>>(), versions, "versions");
    Ok(())
}

fn validate_scene_sees_problems<S: SceneStore>(mut s: S) -> Result<(), String> {
    let z = zs(2)?;
    commit_all(&mut s, vec![rect("a", &z[0])?, arrow("ar", &z[1], Some("ghost"), None)?])?;
    let report = validate_scene(&s);
    ensure!(report.has_errors(), "a binding to a missing element is an error");
    let mut tx = s.begin();
    tx.update(&eid("ar")?, |e| {
        if let ElementKind::Arrow(LinearData { start_binding, .. }) = &mut e.kind {
            *start_binding = None;
        }
    })
    .map_err(e2s)?;
    tx.commit().map_err(e2s)?;
    ensure!(!validate_scene(&s).has_errors(), "fixed");
    Ok(())
}

/// Every check, in a stable order.
pub fn checks<S: SceneStore>() -> Vec<Check<S>> {
    vec![
        Check { name: "empty_store", run: empty_store },
        Check { name: "insert_commit_and_order", run: insert_commit_and_order },
        Check { name: "ties_break_by_id", run: ties_break_by_id },
        Check { name: "rollback_restores_everything", run: rollback_restores_everything },
        Check { name: "update_reports_changed_fields", run: update_reports_changed_fields },
        Check { name: "changes_are_net", run: changes_are_net },
        Check { name: "tombstones", run: tombstones },
        Check { name: "refusals_leave_the_store_unchanged", run: refusals_leave_the_store_unchanged },
        Check { name: "subscribers", run: subscribers },
        Check { name: "derived_indexes", run: derived_indexes },
        Check { name: "meta_and_files", run: meta_and_files },
        Check { name: "document_round_trip", run: document_round_trip },
        Check { name: "same_document_loaded_twice_changes_nothing", run: same_document_loaded_twice_changes_nothing },
        Check { name: "validate_scene_sees_problems", run: validate_scene_sees_problems },
    ]
}

/// Runs every check against a fresh store from `make`. Returns the failures as `name: message` lines.
pub fn run<S: SceneStore>(make: impl Fn() -> S) -> Result<(), Vec<String>> {
    let failures: Vec<String> =
        checks::<S>().into_iter().filter_map(|c| (c.run)(make()).err().map(|m| format!("{}: {m}", c.name))).collect();
    if failures.is_empty() { Ok(()) } else { Err(failures) }
}

#[cfg(test)]
mod tests {
    use crate::memory::MemoryStore;

    #[test]
    fn memory_store_conforms() {
        if let Err(failures) = super::run(MemoryStore::new) {
            panic!("{}", failures.join("\n"));
        }
    }
}
