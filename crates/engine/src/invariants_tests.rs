//! The invariant pass, driven by scripted store edits: synthetic commands move, resize, re-text and delete
//! elements, and the tests check the follow-ups (arrows, labels) and that they share the command's undo step.

use std::sync::Arc;

use drafftink_edit::binding::{End, stale_arrows};
use drafftink_edit::linear::world_points;
use drafftink_edit::text_bind::{create_label, label_of};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    BindMode, Binding, Element, ElementId, ElementKind, LinearKind, SceneRead, SceneTx, StoreError, TextData, Transform,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ports::fake::Fakes;
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::services::Services;
use crate::session::Session;
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, Redraw, RingMode, Selection, TextGate, Typed, Undo};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
struct EditArgs {
    id: String,
    dx: f64,
    dy: f64,
    w: f64,
    h: f64,
    text: String,
    lock: bool,
}

fn target(s: &Session, a: &EditArgs) -> Result<ElementId, Refusal> {
    s.resolve_ref(&crate::session::Ref(a.id.clone()))
}

fn run_edit(
    s: &mut Session,
    id: &ElementId,
    f: impl FnOnce(&mut Element) -> Result<(), Refusal>,
) -> Result<(), Refusal> {
    let cmd = s.cmd();
    let mut inner: Result<(), Refusal> = Ok(());
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| tx.update(id, |e| inner = f(e)))
        .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    inner
}

fn move_it(s: &mut Session, a: EditArgs) -> Result<Value, Refusal> {
    let id = target(s, &a)?;
    run_edit(s, &id, |e| {
        e.transform.x += a.dx;
        e.transform.y += a.dy;
        Ok(())
    })?;
    Ok(json!({}))
}

fn resize_it(s: &mut Session, a: EditArgs) -> Result<Value, Refusal> {
    let id = target(s, &a)?;
    run_edit(s, &id, |e| {
        e.transform.w = a.w;
        e.transform.h = a.h;
        Ok(())
    })?;
    Ok(json!({}))
}

fn retext(s: &mut Session, a: EditArgs) -> Result<Value, Refusal> {
    let id = target(s, &a)?;
    run_edit(s, &id, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.content = a.text.clone();
        }
        Ok(())
    })?;
    Ok(json!({}))
}

fn lock_it(s: &mut Session, a: EditArgs) -> Result<Value, Refusal> {
    let id = target(s, &a)?;
    run_edit(s, &id, |e| {
        e.locked = a.lock;
        Ok(())
    })?;
    Ok(json!({}))
}

fn delete_it(s: &mut Session, a: EditArgs) -> Result<Value, Refusal> {
    let id = target(s, &a)?;
    run_edit(s, &id, |e| {
        e.deleted = true;
        Ok(())
    })?;
    Ok(json!({}))
}

fn move_then_refuse(s: &mut Session, a: EditArgs) -> Result<Value, Refusal> {
    move_it(s, a)?;
    Err(Refusal::nothing_to_do(s.cmd(), "move", "this test command always refuses after moving"))
}

const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    needs_selection: Selection::No,
    ports: crate::spec::PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: true,
    doc_write: crate::spec::DocWrite::Edit,
};

const SCRIPT: &[CommandSpec] = &[
    CommandSpec::new("t.move", "t", "Move", "Moves an element.", &Typed::new(move_it), DOC),
    CommandSpec::new("t.resize", "t", "Resize", "Sets the size of an element.", &Typed::new(resize_it), DOC),
    CommandSpec::new("t.retext", "t", "Retext", "Sets the content of a text.", &Typed::new(retext), DOC),
    CommandSpec::new("t.lock", "t", "Lock", "Sets the lock flag.", &Typed::new(lock_it), DOC),
    CommandSpec::new("t.delete", "t", "Delete", "Marks an element deleted.", &Typed::new(delete_it), DOC),
    CommandSpec::new(
        "t.move_refuse",
        "t",
        "Move and refuse",
        "Moves, then refuses.",
        &Typed::new(move_then_refuse),
        DOC,
    ),
];

fn session() -> Session {
    let fakes = Fakes::new();
    let reg = Arc::new(Registry::from_families(&[SCRIPT]));
    let mut s = Session::with_registry(fakes.ports(), reg);
    s.services = Services::fake();
    s
}

fn put(s: &mut Session, e: Element) {
    s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.insert(e)).unwrap();
}

fn rect(s: &mut Session, name: &str, x: f64, y: f64, w: f64, h: f64) -> ElementId {
    let id = ElementId::new(name).unwrap();
    let z = s.tab().doc.store().next_top_index().unwrap();
    let mut e = Element::new(id.clone(), ElementKind::Rectangle, z);
    e.transform = Transform::new(x, y, w, h);
    put(s, e);
    id
}

/// An arrow from `from` (free) to a point bound to `to`, solved so the document is consistent.
fn arrow_to(s: &mut Session, name: &str, from: (f64, f64), to: &ElementId) -> ElementId {
    let id = ElementId::new(name).unwrap();
    let z = s.tab().doc.store().next_top_index().unwrap();
    let tgt = s.store().get(to).unwrap().rect().center();
    let mut e = Element::linear(id.clone(), LinearKind::Arrow, z, &[Point::new(from.0, from.1), tgt]).unwrap();
    if let ElementKind::Arrow(d) = &mut e.kind {
        d.end_binding = Some(Binding { target: to.clone(), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] });
    }
    put(s, e);
    let to = to.clone();
    s.tab_mut()
        .doc
        .edit::<_, drafftink_edit::binding::BindingError>(|tx| {
            drafftink_edit::binding::update_bound_arrows(tx, std::slice::from_ref(&to))
        })
        .unwrap();
    id
}

fn label(s: &mut Session, container: &ElementId, content: &str) -> ElementId {
    let Session { tabs, services, ports, .. } = s;
    let doc = &mut tabs[0].doc;
    let (out, _) = doc
        .edit::<_, drafftink_edit::text_bind::TextBindError>(|tx| {
            create_label(tx, services.edit(), &mut *ports.ids, container, content, None)
        })
        .unwrap();
    out.label
}

fn end_of(s: &Session, arrow: &ElementId) -> Point {
    *world_points(s.element(arrow).unwrap()).unwrap().last().unwrap()
}

fn rect_of(s: &Session, id: &ElementId) -> drafftink_geom::kurbo::Rect {
    s.element(id).unwrap().rect()
}

fn ready(s: &mut Session) {
    s.tab_mut().doc.commit_untracked().unwrap();
}

fn exec(s: &mut Session, id: &str, args: Value) -> Value {
    s.execute(id, args).unwrap_or_else(|r| panic!("{id} refused: {}", r.message))
}

#[test]
fn moving_a_shape_moves_the_arrow_bound_to_it_inside_the_same_undo_step() {
    let mut s = session();
    let r = rect(&mut s, "box", 0.0, 0.0, 100.0, 100.0);
    let a = arrow_to(&mut s, "arr", (400.0, 50.0), &r);
    ready(&mut s);
    assert!(stale_arrows(s.store()).is_empty(), "the fixture starts consistent");
    let end_before = end_of(&s, &a);

    let out = exec(&mut s, "t.move", json!({"id": "box", "dx": 30.0, "dy": 200.0}));
    assert_eq!(out["changed"], true);
    assert!(stale_arrows(s.store()).is_empty(), "the arrow end follows the shape");
    let end_after = end_of(&s, &a);
    assert!(
        end_after.y > end_before.y + 100.0,
        "the arrow end came down with the shape: {end_before:?} -> {end_after:?}"
    );
    assert_eq!(s.tab().doc.undo_depth(), 1, "shape and arrow are one step");

    s.undo_steps(1).unwrap();
    assert_eq!(rect_of(&s, &r).x0, 0.0);
    assert_eq!(end_of(&s, &a), end_before, "undo puts the arrow back with the shape");
    assert!(stale_arrows(s.store()).is_empty());
}

#[test]
fn the_followups_are_part_of_the_command_and_roll_back_with_it() {
    let mut s = session();
    let r = rect(&mut s, "box", 0.0, 0.0, 100.0, 100.0);
    let a = arrow_to(&mut s, "arr", (400.0, 50.0), &r);
    ready(&mut s);
    let before = (s.fingerprint(), end_of(&s, &a));
    let e = s.execute("t.move_refuse", json!({"id": "box", "dx": 0.0, "dy": 300.0})).unwrap_err();
    assert_eq!(e.code, "nothing_to_do");
    assert_eq!((s.fingerprint(), end_of(&s, &a)), before, "the refused command left neither shape nor arrow moved");
}

#[test]
fn a_label_follows_its_container() {
    let mut s = session();
    let r = rect(&mut s, "box", 0.0, 0.0, 200.0, 100.0);
    let l = label(&mut s, &r, "hi");
    ready(&mut s);
    let c0 = s.element(&l).unwrap().rect().center();
    exec(&mut s, "t.move", json!({"id": "box", "dx": 50.0, "dy": 70.0}));
    let c1 = s.element(&l).unwrap().rect().center();
    assert!((c1.x - c0.x - 50.0).abs() < 1e-6 && (c1.y - c0.y - 70.0).abs() < 1e-6, "{c0:?} {c1:?}");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    // Resizing keeps the label centred in the new size.
    exec(&mut s, "t.resize", json!({"id": "box", "w": 400.0, "h": 200.0}));
    let b = rect_of(&s, &r);
    let c2 = s.element(&l).unwrap().rect().center();
    assert!((c2.x - b.center().x).abs() < 1e-6 && (c2.y - b.center().y).abs() < 1e-6, "{b:?} {c2:?}");
}

#[test]
fn a_container_grows_to_hold_a_longer_label_and_the_arrows_bound_to_it_follow() {
    let mut s = session();
    let r = rect(&mut s, "box", 0.0, 0.0, 120.0, 60.0);
    let l = label(&mut s, &r, "ab");
    let a = arrow_to(&mut s, "arr", (600.0, 30.0), &r);
    ready(&mut s);
    let w0 = rect_of(&s, &r).width();
    let end0 = end_of(&s, &a);
    // 60 characters at 10 units each (the fake is monospace at half the 20-unit font) cannot fit in 120 wide.
    exec(&mut s, "t.retext", json!({"id": l.as_str(), "text": "x".repeat(60)}));
    let grown = rect_of(&s, &r);
    assert!(grown.height() > 60.0 || grown.width() > w0, "the container grew: {grown:?}");
    assert!(stale_arrows(s.store()).is_empty(), "the arrow was re-solved against the grown container");
    let end1 = end_of(&s, &a);
    assert!(end1 != end0 || grown.size() == drafftink_geom::kurbo::Size::new(120.0, 60.0));
    assert_eq!(s.tab().doc.undo_depth(), 1, "typing, growing and re-routing are one step");
    s.undo_steps(1).unwrap();
    assert_eq!(rect_of(&s, &r).size(), drafftink_geom::kurbo::Size::new(120.0, 60.0));
    assert_eq!(end_of(&s, &a), end0);
    assert_eq!(label_of(s.store(), &r), Some(l));
}

#[test]
fn a_command_that_moved_nothing_runs_no_followups() {
    let mut s = session();
    let r = rect(&mut s, "box", 0.0, 0.0, 100.0, 100.0);
    let a = arrow_to(&mut s, "arr", (400.0, 50.0), &r);
    // Make the arrow stale behind the pass's back, as a remote edit or an import could.
    s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.update(&r, |e| e.transform.x += 40.0)).unwrap();
    ready(&mut s);
    assert!(!stale_arrows(s.store()).is_empty());
    let rev = s.tab().doc.revision();
    // Locking changes no geometry: the pass leaves the stale arrow alone, and writes nothing.
    exec(&mut s, "t.lock", json!({"id": "box", "lock": true}));
    assert!(!stale_arrows(s.store()).is_empty(), "only elements whose geometry changed are followed up");
    assert_eq!(s.tab().doc.revision(), rev + 1, "one write: the lock itself");
    let _ = a;
    // The next real move of the shape repairs the arrow.
    exec(&mut s, "t.lock", json!({"id": "box", "lock": false}));
    exec(&mut s, "t.move", json!({"id": "box", "dx": 1.0}));
    assert!(stale_arrows(s.store()).is_empty());
}

#[test]
fn the_pass_is_idempotent_and_settles_in_two_rounds_for_a_plain_move() {
    let mut s = session();
    let r = rect(&mut s, "box", 0.0, 0.0, 100.0, 100.0);
    let a = arrow_to(&mut s, "arr", (400.0, 50.0), &r);
    let _ = label(&mut s, &r, "ok");
    ready(&mut s);
    let revs0 = s.tab().doc.revision();
    exec(&mut s, "t.move", json!({"id": "box", "dx": 10.0}));
    let after_first = s.fingerprint();
    // Moving back and forth by zero changes nothing at all.
    let before = s.tab().doc.revision();
    exec(&mut s, "t.move", json!({"id": "box", "dx": 0.0}));
    assert_eq!(s.tab().doc.revision(), before, "a no-op command writes nothing, the pass included");
    assert_eq!(s.fingerprint(), after_first);
    assert!(s.tab().doc.revision() <= revs0 + 6, "a move costs a few writes, not a loop");
    assert!(stale_arrows(s.store()).is_empty());
    let _ = a;
}

/// An elbow arrow from the right of `a` to the left of `b`, solved so the document is consistent.
fn elbow_between(s: &mut Session, name: &str, a: &ElementId, b: &ElementId) -> ElementId {
    let id = ElementId::new(name).unwrap();
    let z = s.tab().doc.store().next_top_index().unwrap();
    let (ra, rb) = (s.store().get(a).unwrap().rect(), s.store().get(b).unwrap().rect());
    let pts = [Point::new(ra.x1 + 6.0, ra.center().y), Point::new(rb.x0 - 6.0, rb.center().y)];
    let mut e = Element::linear(id.clone(), LinearKind::Arrow, z, &pts).unwrap();
    if let ElementKind::Arrow(d) = &mut e.kind {
        d.path = drafftink_scene::PathType::Elbow;
        d.start_binding = Some(Binding { target: a.clone(), mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
        d.end_binding = Some(Binding { target: b.clone(), mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
    }
    put(s, e);
    let ids = [a.clone(), b.clone()];
    s.tab_mut()
        .doc
        .edit::<_, drafftink_edit::binding::BindingError>(|tx| drafftink_edit::binding::update_bound_arrows(tx, &ids))
        .unwrap();
    id
}

fn route_of(s: &Session, arrow: &ElementId) -> Vec<Point> {
    world_points(s.element(arrow).unwrap()).unwrap()
}

#[test]
fn an_elbow_arrow_goes_around_a_shape_dropped_in_its_way_and_back_when_it_leaves_in_one_undo_step_each() {
    let mut s = session();
    let a = rect(&mut s, "a", 0.0, 0.0, 100.0, 100.0);
    let b = rect(&mut s, "b", 500.0, 0.0, 100.0, 100.0);
    let wall = rect(&mut s, "wall", 250.0, 600.0, 100.0, 60.0);
    let arrow = elbow_between(&mut s, "arr", &a, &b);
    ready(&mut s);
    let straight = route_of(&s, &arrow);
    assert_eq!(straight.len(), 2, "{straight:?}");

    // The wall moves onto the line: the shape is not bound to the arrow, only in its way.
    exec(&mut s, "t.move", json!({"id": "wall", "dx": 0.0, "dy": -580.0}));
    let around = route_of(&s, &arrow);
    let w = rect_of(&s, &wall);
    assert!(around.len() > 2, "{around:?}");
    assert!(
        around.windows(2).all(|l| {
            let (x0, x1) = (l[0].x.min(l[1].x), l[0].x.max(l[1].x));
            let (y0, y1) = (l[0].y.min(l[1].y), l[0].y.max(l[1].y));
            !(x0 < w.x1 && x1 > w.x0 && y0 < w.y1 && y1 > w.y0)
        }),
        "{around:?} crosses {w:?}"
    );
    assert!(stale_arrows(s.store()).is_empty());
    assert_eq!(s.tab().doc.undo_depth(), 1, "the shape and the arrow it pushed aside are one step");

    // It leaves again: the detour goes with it.
    exec(&mut s, "t.move", json!({"id": "wall", "dx": 0.0, "dy": 580.0}));
    assert_eq!(route_of(&s, &arrow), straight);
    assert_eq!(s.tab().doc.undo_depth(), 2);

    s.undo_steps(1).unwrap();
    assert_eq!(route_of(&s, &arrow), around, "undo brings the shape and the detour back together");
    s.undo_steps(1).unwrap();
    assert_eq!(route_of(&s, &arrow), straight);
}

#[test]
fn deleting_the_shape_in_the_way_straightens_the_arrow() {
    let mut s = session();
    let a = rect(&mut s, "a", 0.0, 0.0, 100.0, 100.0);
    let b = rect(&mut s, "b", 500.0, 0.0, 100.0, 100.0);
    let _wall = rect(&mut s, "wall", 250.0, 20.0, 100.0, 60.0);
    let arrow = elbow_between(&mut s, "arr", &a, &b);
    ready(&mut s);
    assert!(route_of(&s, &arrow).len() > 2, "built with the shape in the way");
    exec(&mut s, "t.delete", json!({"id": "wall"}));
    assert_eq!(route_of(&s, &arrow).len(), 2);
    s.undo_steps(1).unwrap();
    assert!(route_of(&s, &arrow).len() > 2);
}

#[test]
fn deleting_a_container_leaves_its_arrow_and_label_without_an_error() {
    let mut s = session();
    let r = rect(&mut s, "box", 0.0, 0.0, 100.0, 100.0);
    let a = arrow_to(&mut s, "arr", (400.0, 50.0), &r);
    let l = label(&mut s, &r, "bye");
    ready(&mut s);
    exec(&mut s, "t.delete", json!({"id": "box"}));
    assert!(s.element(&r).is_none());
    assert!(s.element(&a).is_some() && s.element(&l).is_some(), "freeing them is the delete command's job");
}

#[test]
fn a_whole_batch_is_followed_up_once_and_is_one_step() {
    use crate::batch::{BatchOpts, BatchStep};
    use crate::ring::Origin;
    let mut s = session();
    let r = rect(&mut s, "box", 0.0, 0.0, 100.0, 100.0);
    let a = arrow_to(&mut s, "arr", (400.0, 50.0), &r);
    ready(&mut s);
    let steps = [
        BatchStep::new("t.move", json!({"id": "box", "dx": 10.0})),
        BatchStep::new("t.move", json!({"id": "box", "dy": 90.0})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.failed.is_none());
    assert!(stale_arrows(s.store()).is_empty());
    assert_eq!(s.tab().doc.undo_depth(), 1);
    let _ = a;
}

#[test]
fn text_content_edits_outside_a_label_do_not_need_a_container() {
    let mut s = session();
    let id = ElementId::new("free").unwrap();
    let z = s.tab().doc.store().next_top_index().unwrap();
    let mut e = Element::new(id.clone(), ElementKind::Text(TextData::new("a")), z);
    e.transform = Transform::new(5.0, 5.0, 10.0, 20.0);
    put(&mut s, e);
    ready(&mut s);
    exec(&mut s, "t.retext", json!({"id": "free", "text": "longer"}));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    let _ = End::End;
}
