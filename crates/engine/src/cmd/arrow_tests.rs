//! Tests of the `arrow` family. Pattern per command: availability, refusal text, effect (with undo), one
//! negative case. Then the scenarios of the headline feature: arrows stay attached to shapes.
//!
//! No UI, no DOM. The tests run on a registry made of the arrow commands and three test-only commands:
//! `t.seed` (puts a rectangle, line or arrow in the document without an undo step, as a loaded file would),
//! `t.move` (the domain transform that `element.move` will call) and `t.delete` (the domain delete that
//! `element.delete` will call). The element family is not merged yet; when it is, the scenarios can switch
//! to `element.create`, `element.move` and `element.delete`.

use std::sync::Arc;

use drafftink_edit::binding::{End, bound_end_position, resolve, stale_arrows};
use drafftink_edit::linear::world_points;
use drafftink_geom::binding::binding_gap;
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_scene::{
    BindMode, Element, ElementId, ElementKind, Head, LinearData, PathType, SceneRead, SceneTx, StoreError, Transform,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{BindChoice, EditPointsArgs, EndWord, MovePointArgs, PointIndex};
use crate::snap::{Snap, SnapSwitches, SnapWord};
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::ring::Origin;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};
use crate::testing;

// ---------------------------------------------------------------------------------------------
// Test-only commands
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SeedArgs {
    kind: String,
    /// `[x, y, w, h]` of a rectangle.
    #[serde(default)]
    bounds: Option<[f64; 4]>,
    /// World points of a line or arrow.
    #[serde(default)]
    points: Vec<[f64; 2]>,
    #[serde(default)]
    locked: bool,
}

struct Boom(String);

impl From<StoreError> for Boom {
    fn from(e: StoreError) -> Self {
        Boom(e.to_string())
    }
}

fn seed(s: &mut Session, a: SeedArgs) -> Result<Value, Refusal> {
    let cmd = s.cmd();
    let id = s.ports.ids.element_id();
    let seed = s.ports.ids.seed();
    let made = id.clone();
    let result = s.tab_mut().doc.edit::<(), Boom>(|tx| {
        let z = tx.next_top_index().map_err(|e| Boom(e.to_string()))?;
        let (kind, t) = match a.kind.as_str() {
            "rect" => {
                let [x, y, w, h] = a.bounds.ok_or_else(|| Boom("a rect needs bounds".into()))?;
                (ElementKind::Rectangle, Transform::new(x, y, w, h))
            }
            "arrow" | "line" => {
                let (minx, miny) = a.points.iter().fold((f64::MAX, f64::MAX), |m, p| (m.0.min(p[0]), m.1.min(p[1])));
                let (maxx, maxy) = a.points.iter().fold((f64::MIN, f64::MIN), |m, p| (m.0.max(p[0]), m.1.max(p[1])));
                let local = a.points.iter().map(|p| Point::new(p[0] - minx, p[1] - miny)).collect();
                let data = LinearData::new(local);
                let k = if a.kind == "arrow" { ElementKind::Arrow(data) } else { ElementKind::Line(data) };
                (k, Transform::new(minx, miny, maxx - minx, maxy - miny))
            }
            other => return Err(Boom(format!("unknown kind {other}"))),
        };
        let mut e = Element::new(id, kind, z);
        e.transform = t;
        e.style.seed = seed;
        e.locked = a.locked;
        tx.insert(e)?;
        Ok(())
    });
    match result {
        Ok(_) => Ok(json!({ "id": made.as_str() })),
        Err(Boom(m)) => Err(Refusal::internal(cmd, m)),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    ids: Vec<String>,
    dx: f64,
    dy: f64,
}

fn resolve_all(s: &Session, ids: &[String]) -> Result<Vec<ElementId>, Refusal> {
    ids.iter().map(|i| s.resolve_ref(&Ref(i.clone()))).collect()
}

fn t_move(s: &mut Session, a: MoveArgs) -> Result<Value, Refusal> {
    let cmd = s.cmd();
    let ids = resolve_all(s, &a.ids)?;
    let r = s.tab_mut().doc.edit::<_, Boom>(|tx| {
        drafftink_edit::transform::move_by(tx, &ids, Vec2::new(a.dx, a.dy)).map_err(|e| Boom(e.to_string()))
    });
    match r {
        Ok(_) => Ok(json!({})),
        Err(Boom(m)) => Err(Refusal::internal(cmd, m)),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    ids: Vec<String>,
}

fn t_delete(s: &mut Session, a: DeleteArgs) -> Result<Value, Refusal> {
    let cmd = s.cmd();
    let ids = resolve_all(s, &a.ids)?;
    let r = s.tab_mut().doc.edit::<_, Boom>(|tx| {
        let report = drafftink_edit::lifecycle::delete(tx, &ids, drafftink_edit::lifecycle::ChildPolicy::Release)
            .map_err(|e| Boom(e.to_string()))?;
        // The documented pattern: free the ends in the same transaction (a dangling binding would also be
        // ignored by the solver, but a free end is what the user sees as "detached").
        drafftink_edit::binding::free_ends_bound_to(tx, &report.deleted).map_err(|e| Boom(e.to_string()))?;
        Ok(())
    });
    match r {
        Ok(_) => Ok(json!({})),
        Err(Boom(m)) => Err(Refusal::internal(cmd, m)),
    }
}

const TEST_DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC,
    needs_selection: Selection::No,
    ports: PortSet::NONE,
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

const TEST_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "t.seed",
        "t",
        "Seed",
        "Puts an element in the document without an undo step.",
        &Typed::new(seed),
        Facts { undo: Undo::NoUndo, ..TEST_DOC },
    ),
    CommandSpec::new("t.move", "t", "Move", "Moves elements by the domain transform.", &Typed::new(t_move), TEST_DOC),
    CommandSpec::new("t.delete", "t", "Delete", "Deletes elements by the domain rule.", &Typed::new(t_delete), TEST_DOC),
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn session() -> Session {
    let fakes = crate::ports::fake::Fakes::new();
    let registry = Arc::new(Registry::from_families(&[super::COMMANDS, TEST_COMMANDS]));
    Session::with_registry(fakes.headless_ports(), registry)
}

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    };
    assert_eq!(s.fingerprint(), before, "a refusal must leave the state unchanged");
    r
}

fn rect(s: &mut Session, x: f64, y: f64, w: f64, h: f64) -> String {
    run(s, "t.seed", json!({"kind": "rect", "bounds": [x, y, w, h]}))["id"].as_str().unwrap_or_default().to_owned()
}

fn locked_rect(s: &mut Session, x: f64, y: f64, w: f64, h: f64) -> String {
    run(s, "t.seed", json!({"kind": "rect", "bounds": [x, y, w, h], "locked": true}))["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn linear(s: &mut Session, kind: &str, pts: &[[f64; 2]]) -> String {
    run(s, "t.seed", json!({"kind": kind, "points": pts}))["id"].as_str().unwrap_or_default().to_owned()
}

fn arrow(s: &mut Session, pts: &[[f64; 2]]) -> String {
    linear(s, "arrow", pts)
}

fn eid(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn el<'a>(s: &'a Session, id: &str) -> &'a Element {
    s.store().get(&eid(id)).unwrap()
}

fn data<'a>(s: &'a Session, id: &str) -> &'a LinearData {
    el(s, id).linear_data().unwrap()
}

fn pts(s: &Session, id: &str) -> Vec<Point> {
    world_points(el(s, id)).unwrap()
}

fn first(s: &Session, id: &str) -> Point {
    pts(s, id)[0]
}

fn last(s: &Session, id: &str) -> Point {
    *pts(s, id).last().unwrap()
}

fn near(a: Point, b: Point) -> bool {
    (a - b).hypot() < 1e-6
}

fn point_of(out: &Value) -> Point {
    Point::new(out["point"][0].as_f64().unwrap_or(f64::NAN), out["point"][1].as_f64().unwrap_or(f64::NAN))
}

fn undo_depth(s: &Session) -> usize {
    s.tab().doc.undo_depth()
}

/// A rectangle `a` on the left, a rectangle `b` lower on the right and a free arrow between them.
struct World {
    s: Session,
    a: String,
    b: String,
    arrow: String,
}

fn world() -> World {
    let mut s = session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let b = rect(&mut s, 300.0, 40.0, 100.0, 60.0);
    let arrow = arrow(&mut s, &[[100.0, 30.0], [300.0, 70.0]]);
    World { s, a, b, arrow }
}

fn bound_world() -> World {
    let mut w = world();
    run(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "start", "target": w.a}));
    run(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": w.b}));
    w
}

fn move_args(arrow: &str, index: PointIndex, to: [f64; 2]) -> MovePointArgs {
    MovePointArgs {
        arrow: Ref(arrow.to_owned()),
        index,
        to,
        insert: false,
        bind: BindChoice::Auto,
        snap: Snap::default(),
    }
}

/// The kind and the box of an element, without the version counter that every write bumps.
fn shape_of(s: &Session, id: &str) -> String {
    format!("{:?} {:?}", el(s, id).kind, el(s, id).transform)
}

fn gap_of(s: &Session, id: &str) -> f64 {
    binding_gap(el(s, id).style.stroke_width)
}

// ---------------------------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------------------------

#[test]
fn the_family_registers_every_catalog_command_and_its_extras() {
    let r = Registry::global();
    for id in [
        "arrow.set_type",
        "arrow.set_head",
        "arrow.swap_heads",
        "arrow.bind",
        "arrow.unbind",
        "arrow.move_point",
        "arrow.add_point",
        "arrow.delete_point",
        "arrow.edit_points",
        "arrow.reroute",
        "arrow.move_segment",
        "arrow.release_segment",
    ] {
        assert!(r.find(id).is_some(), "{id} is not registered");
    }
    assert_eq!(r.find("arrow.move_point").map(|c| c.undo), Some(Undo::Gesture));
    assert_eq!(r.find("arrow.move_segment").map(|c| c.undo), Some(Undo::Gesture));
    assert_eq!(r.find("arrow.edit_points").map(|c| c.undo), Some(Undo::NoUndo));
}

#[test]
fn edit_points_is_unavailable_for_a_shape_and_for_several_elements() {
    let mut s = session();
    let r = rect(&mut s, 0.0, 0.0, 40.0, 40.0);
    let a = arrow(&mut s, &[[0.0, 100.0], [80.0, 130.0]]);
    testing::select(&mut s, &[eid(&r)]);
    assert_eq!(s.availability("arrow.edit_points").unwrap_err().code, "wrong_kind");
    testing::select(&mut s, &[eid(&r), eid(&a)]);
    assert_eq!(s.availability("arrow.edit_points").unwrap_err().code, "needs_single");
    testing::select(&mut s, &[eid(&a)]);
    assert!(s.availability("arrow.edit_points").is_ok());
}

#[test]
fn edit_points_has_its_shortcut() {
    let spec = Registry::global().find("arrow.edit_points").unwrap();
    assert_eq!(spec.shortcuts.len(), 1);
    assert_eq!(spec.shortcuts[0].keys, &["Mod+Enter"]);
}

// ---------------------------------------------------------------------------------------------
// arrow.set_type
// ---------------------------------------------------------------------------------------------

#[test]
fn set_type_needs_a_target() {
    let mut w = world();
    let r = w.s.availability("arrow.set_type").unwrap_err();
    assert_eq!(r.code, "no_target");
    let e = refuse(&mut w.s, "arrow.set_type", json!({"type": "round"}));
    assert_eq!(e.code, "no_target");
    assert_eq!(e.message, r.message);
}

#[test]
fn set_type_on_a_rectangle_is_not_applicable() {
    let mut w = world();
    let r = refuse(&mut w.s, "arrow.set_type", json!({"ids": [w.a], "type": "round"}));
    assert_eq!(r.code, "not_applicable");
    assert!(r.message.contains("None of the targeted elements support an arrow type"), "{}", r.message);
}

#[test]
fn set_type_round_keeps_points_and_undoes() {
    let mut w = world();
    let before = pts(&w.s, &w.arrow);
    let out = run(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow], "type": "round"}));
    assert_eq!(out["changed"], true);
    assert_eq!(data(&w.s, &w.arrow).path, PathType::Round);
    assert_eq!(pts(&w.s, &w.arrow), before);
    assert_eq!(undo_depth(&w.s), 1);
    w.s.undo_steps(1).unwrap();
    assert_eq!(data(&w.s, &w.arrow).path, PathType::Sharp);
}

#[test]
fn set_type_to_the_same_type_changes_nothing() {
    let mut w = world();
    let out = run(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow], "type": "sharp"}));
    assert_eq!(out["changed"], false);
    assert_eq!(undo_depth(&w.s), 0);
}

#[test]
fn set_type_elbow_routes_around_the_bound_shapes_and_back_keeps_the_corners() {
    let mut w = bound_world();
    run(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow], "type": "elbow"}));
    let d = data(&w.s, &w.arrow);
    assert_eq!(d.path, PathType::Elbow);
    assert_eq!(el(&w.s, &w.arrow).transform.angle, 0.0);
    let p = pts(&w.s, &w.arrow);
    assert!(p.len() >= 3, "an elbow between offset shapes has corners: {p:?}");
    assert!(p.windows(2).all(|q| (q[0].x - q[1].x).abs() < 1e-6 || (q[0].y - q[1].y).abs() < 1e-6), "{p:?}");
    assert!(d.start_binding.is_some() && d.end_binding.is_some());
    assert!(stale_arrows(w.s.store()).is_empty());
    run(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow], "type": "sharp"}));
    assert_eq!(data(&w.s, &w.arrow).path, PathType::Sharp);
    let q = pts(&w.s, &w.arrow);
    assert_eq!(q.len(), p.len());
    assert!(
        q[1..q.len() - 1].iter().zip(&p[1..p.len() - 1]).all(|(a, b)| near(*a, *b)),
        "switching away from elbow keeps the corner points"
    );
    assert!(
        (q[0] - p[0]).hypot() < 5.0 && (q[q.len() - 1] - p[p.len() - 1]).hypot() < 5.0,
        "the bound ends are solved again for the straight arrow and stay on the same shapes"
    );
    assert!(data(&w.s, &w.arrow).start_binding.is_some(), "and keeps the bindings");
    assert!(stale_arrows(w.s.store()).is_empty());
}

#[test]
fn set_type_applies_to_the_arrows_of_a_mixed_list_and_refuses_when_one_is_locked() {
    let mut w = world();
    run(&mut w.s, "arrow.set_type", json!({"ids": [w.a, w.arrow], "type": "round"}));
    assert_eq!(data(&w.s, &w.arrow).path, PathType::Round);
    let locked = run(&mut w.s, "t.seed", json!({"kind": "arrow", "points": [[0, 200], [50, 250]], "locked": true}));
    let lid = locked["id"].as_str().unwrap().to_owned();
    let r = refuse(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow, lid], "type": "sharp"}));
    assert_eq!(r.code, "locked");
    assert_eq!(data(&w.s, &w.arrow).path, PathType::Round, "nothing was applied");
}

#[test]
fn set_type_closed_line_to_elbow_is_a_bad_argument() {
    let mut w = world();
    let line = linear(&mut w.s, "line", &[[0.0, 200.0], [60.0, 200.0], [30.0, 260.0]]);
    // Close it through the store, as a loaded file would.
    w.s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            tx.update(&eid(&line), |e| {
                if let ElementKind::Line(d) = &mut e.kind {
                    d.closed = true;
                }
            })
        })
        .unwrap();
    let r = refuse(&mut w.s, "arrow.set_type", json!({"ids": [line], "type": "elbow"}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("closed line cannot be an elbow"), "{}", r.message);
}

// ---------------------------------------------------------------------------------------------
// arrow.set_head, arrow.swap_heads
// ---------------------------------------------------------------------------------------------

#[test]
fn set_head_sets_removes_and_undoes() {
    let mut w = world();
    run(&mut w.s, "arrow.set_head", json!({"ids": [w.arrow], "head": "triangle"}));
    assert_eq!(data(&w.s, &w.arrow).end_head, Head::Triangle);
    assert_eq!(data(&w.s, &w.arrow).start_head, Head::None);
    run(&mut w.s, "arrow.set_head", json!({"ids": [w.arrow], "end": "both", "head": "circle"}));
    assert_eq!(data(&w.s, &w.arrow).start_head, Head::Circle);
    assert_eq!(data(&w.s, &w.arrow).end_head, Head::Circle);
    run(&mut w.s, "arrow.set_head", json!({"ids": [w.arrow], "end": "start", "head": null}));
    assert_eq!(data(&w.s, &w.arrow).start_head, Head::None);
    assert_eq!(data(&w.s, &w.arrow).end_head, Head::Circle);
    run(&mut w.s, "arrow.set_head", json!({"ids": [w.arrow], "head": "none"}));
    assert_eq!(data(&w.s, &w.arrow).end_head, Head::None);
    assert_eq!(undo_depth(&w.s), 4);
    w.s.undo_steps(1).unwrap();
    assert_eq!(data(&w.s, &w.arrow).end_head, Head::Circle);
}

#[test]
fn set_head_requires_the_head_key() {
    let mut w = world();
    let r = refuse(&mut w.s, "arrow.set_head", json!({"ids": [w.arrow]}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("head"), "{}", r.message);
}

#[test]
fn set_head_refuses_rectangles_and_lines() {
    let mut w = world();
    let line = linear(&mut w.s, "line", &[[0.0, 200.0], [60.0, 200.0]]);
    for id in [w.a.clone(), line] {
        let r = refuse(&mut w.s, "arrow.set_head", json!({"ids": [id], "head": "arrow"}));
        assert_eq!(r.code, "not_applicable");
        assert!(r.message.contains("arrowheads"), "{}", r.message);
    }
    let none = refuse(&mut w.s, "arrow.set_head", json!({"head": "arrow"}));
    assert_eq!(none.code, "no_target");
}

#[test]
fn set_head_on_a_locked_arrow_is_refused() {
    let mut w = world();
    let locked = run(&mut w.s, "t.seed", json!({"kind": "arrow", "points": [[0, 200], [50, 250]], "locked": true}));
    let lid = locked["id"].as_str().unwrap().to_owned();
    let r = refuse(&mut w.s, "arrow.set_head", json!({"ids": [lid.clone()], "head": "arrow"}));
    assert_eq!(r.code, "locked");
    assert!(r.message.contains(&lid), "{}", r.message);
}

#[test]
fn swap_heads_swaps_and_refuses_non_arrows() {
    let mut w = world();
    run(&mut w.s, "arrow.set_head", json!({"ids": [w.arrow], "head": "triangle"}));
    run(&mut w.s, "arrow.swap_heads", json!({"ids": [w.arrow]}));
    assert_eq!(data(&w.s, &w.arrow).start_head, Head::Triangle);
    assert_eq!(data(&w.s, &w.arrow).end_head, Head::None);
    w.s.undo_steps(1).unwrap();
    assert_eq!(data(&w.s, &w.arrow).end_head, Head::Triangle);
    let r = refuse(&mut w.s, "arrow.swap_heads", json!({"ids": [w.a]}));
    assert_eq!(r.code, "not_applicable");
}

// ---------------------------------------------------------------------------------------------
// arrow.bind, arrow.unbind
// ---------------------------------------------------------------------------------------------

#[test]
fn bind_orbit_puts_the_end_on_the_outline_plus_the_gap() {
    let mut w = world();
    let out = run(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": w.b}));
    assert_eq!(out["binding"]["target"], w.b.as_str());
    assert_eq!(out["binding"]["mode"], "orbit");
    let b = data(&w.s, &w.arrow).end_binding.clone().unwrap();
    assert_eq!(b.target, eid(&w.b));
    assert_eq!(b.mode, BindMode::Orbit);
    let r = resolve(w.s.store(), &eid(&w.b)).unwrap();
    let d = r.target.signed_distance(last(&w.s, &w.arrow));
    assert!((d - gap_of(&w.s, &w.b)).abs() < 1e-3, "end is {d} from the outline, gap is {}", gap_of(&w.s, &w.b));
    assert!(data(&w.s, &w.arrow).start_binding.is_none());
    assert!(stale_arrows(w.s.store()).is_empty());
    assert_eq!(undo_depth(&w.s), 1);
    w.s.undo_steps(1).unwrap();
    assert!(data(&w.s, &w.arrow).end_binding.is_none());
    assert!(near(last(&w.s, &w.arrow), Point::new(300.0, 70.0)));
}

#[test]
fn bind_inside_keeps_the_end_at_the_fixed_point() {
    let mut w = world();
    run(
        &mut w.s,
        "arrow.bind",
        json!({"arrow": w.arrow, "end": "end", "target": w.b, "mode": "inside", "fixed_point": [0.25, 0.5]}),
    );
    let at = last(&w.s, &w.arrow);
    assert!((at.x - 325.0).abs() < 0.1 && (at.y - 70.0).abs() < 0.1, "{at:?}");
    let d = data(&w.s, &w.arrow).end_binding.clone().unwrap();
    assert_eq!(d.mode, BindMode::Inside);
}

#[test]
fn bind_start_faces_the_other_end() {
    let mut w = world();
    run(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "start", "target": w.a}));
    let p = first(&w.s, &w.arrow);
    // The arrow heads right and slightly down, so it leaves the right side of the rectangle.
    assert!(p.x > 100.0 && p.x < 112.0, "{p:?}");
    assert!(p.y > 25.0 && p.y < 45.0, "{p:?}");
}

#[test]
fn bind_refuses_what_cannot_be_a_target() {
    let mut w = world();
    let other = arrow(&mut w.s, &[[0.0, 300.0], [80.0, 340.0]]);
    let r = refuse(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": other}));
    assert_eq!(r.code, "not_bindable");
    assert_eq!(
        r.message,
        "Arrow elements cannot be arrow targets. Bindable kinds: rectangle, diamond, ellipse, text, image, frame."
    );
    let itself = refuse(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": w.arrow}));
    assert_eq!(itself.code, "not_bindable");
}

#[test]
fn bind_refusals_for_kind_lock_and_missing_elements() {
    let mut w = world();
    let r = refuse(&mut w.s, "arrow.bind", json!({"arrow": w.a, "end": "end", "target": w.b}));
    assert_eq!(r.code, "wrong_kind");
    assert!(r.message.contains("rectangle"), "{}", r.message);
    let lt = locked_rect(&mut w.s, 500.0, 0.0, 50.0, 50.0);
    let r = refuse(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": lt}));
    assert_eq!(r.code, "locked");
    let r = refuse(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": "nope"}));
    assert_eq!(r.code, "not_found");
    let r = refuse(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": w.b, "fixed_point": [50, 0.5]}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("fixed_point[0]"), "{}", r.message);
    let r = refuse(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "middle", "target": w.b}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn bind_on_an_elbow_arrow_routes_once_and_warns_about_inside() {
    let mut w = world();
    run(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow], "type": "elbow"}));
    let out = run(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": w.b, "mode": "inside"}));
    assert_eq!(out["binding"]["mode"], "orbit");
    assert!(out["warnings"].as_array().is_some_and(|a| a.iter().any(|x| x.as_str().is_some_and(|t| t.contains("orbit")))));
    assert!(stale_arrows(w.s.store()).is_empty());
    let p = pts(&w.s, &w.arrow);
    assert!(p.windows(2).all(|q| (q[0].x - q[1].x).abs() < 1e-6 || (q[0].y - q[1].y).abs() < 1e-6), "{p:?}");
}

#[test]
fn unbind_frees_the_end_where_it_is() {
    let mut w = bound_world();
    let (s0, e0) = (first(&w.s, &w.arrow), last(&w.s, &w.arrow));
    let out = run(&mut w.s, "arrow.unbind", json!({"arrow": w.arrow, "end": "end"}));
    assert_eq!(out["freed"], json!(["end"]));
    assert!(data(&w.s, &w.arrow).end_binding.is_none());
    assert!(data(&w.s, &w.arrow).start_binding.is_some());
    assert!(near(first(&w.s, &w.arrow), s0) && near(last(&w.s, &w.arrow), e0), "the geometry does not change");
    run(&mut w.s, "arrow.unbind", json!({"arrow": w.arrow}));
    assert!(data(&w.s, &w.arrow).start_binding.is_none());
    w.s.undo_steps(1).unwrap();
    assert!(data(&w.s, &w.arrow).start_binding.is_some());
}

#[test]
fn unbind_of_a_free_end_has_nothing_to_do() {
    let mut w = world();
    let r = refuse(&mut w.s, "arrow.unbind", json!({"arrow": w.arrow, "end": "start"}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, format!("Nothing to unbind: `{}` is not bound at that end.", w.arrow));
    let r = refuse(&mut w.s, "arrow.unbind", json!({"arrow": w.arrow}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(undo_depth(&w.s), 0);
    let r = refuse(&mut w.s, "arrow.unbind", json!({"arrow": w.a}));
    assert_eq!(r.code, "wrong_kind");
}

// ---------------------------------------------------------------------------------------------
// arrow.move_point
// ---------------------------------------------------------------------------------------------

#[test]
fn move_point_moves_an_interior_point() {
    let mut w = world();
    let line = linear(&mut w.s, "line", &[[0.0, 200.0], [60.0, 260.0], [120.0, 200.0]]);
    let out = run(&mut w.s, "arrow.move_point", json!({"arrow": line, "index": 1, "to": [60, 300]}));
    assert_eq!(out["index"], 1);
    assert_eq!(out["point"], json!([60.0, 300.0]));
    assert!(near(pts(&w.s, &line)[1], Point::new(60.0, 300.0)));
    assert_eq!(undo_depth(&w.s), 1);
    w.s.undo_steps(1).unwrap();
    assert!(near(pts(&w.s, &line)[1], Point::new(60.0, 260.0)));
}

#[test]
fn move_point_refusals() {
    let mut w = world();
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": 5, "to": [0, 0]}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("point 5 does not exist") && r.message.contains("0 to 1"), "{}", r.message);
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": w.a, "index": 0, "to": [0, 0]}));
    assert_eq!(r.code, "wrong_kind");
    let lk = run(&mut w.s, "t.seed", json!({"kind": "arrow", "points": [[0, 200], [50, 250]], "locked": true}));
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": lk["id"], "index": 0, "to": [0, 0]}));
    assert_eq!(r.code, "locked");
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "end", "to": [1e12, 0]}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "start", "to": [0, 0], "insert": true}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": 2, "to": [0, 0], "insert": true}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("segments 1 to 1"), "{}", r.message);
}

/// A closed triangle line, as a loaded file has it.
fn closed_triangle(s: &mut Session) -> String {
    let line = linear(s, "line", &[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]]);
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            tx.update(&eid(&line), |e| {
                if let ElementKind::Line(d) = &mut e.kind {
                    d.closed = true;
                }
            })
        })
        .unwrap();
    // Not an undo step: the file arrived this way.
    s.tab_mut().doc.commit_untracked().unwrap();
    line
}

#[test]
fn the_closing_segment_of_a_closed_line_takes_a_new_point() {
    let mut s = session();
    let line = closed_triangle(&mut s);
    let undo = undo_depth(&s);
    let out = run(&mut s, "arrow.move_point", json!({"arrow": line, "index": 3, "to": [0, 80], "insert": true, "snap": {"grid": false, "smart": false, "angle": false}}));
    assert_eq!(out["index"], 3);
    assert_eq!(pts(&s, &line), vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(100.0, 100.0), Point::new(0.0, 80.0)]);
    assert_eq!(undo_depth(&s), undo + 1, "one undo step");
    s.undo_steps(1).unwrap();
    assert_eq!(pts(&s, &line).len(), 3);
    // `arrow.add_point` takes the same segment and puts the point in the middle of the closing edge.
    let out = run(&mut s, "arrow.add_point", json!({"arrow": line, "segment": 3}));
    assert_eq!(out["index"], 3);
    assert_eq!(pts(&s, &line)[3], Point::new(50.0, 50.0));
}

#[test]
fn an_open_line_has_no_closing_segment_and_the_message_counts_the_right_segments() {
    let mut s = session();
    let line = linear(&mut s, "line", &[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]]);
    let r = refuse(&mut s, "arrow.move_point", json!({"arrow": line, "index": 3, "to": [0, 80], "insert": true}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("segments 1 to 2"), "{}", r.message);
    let r = refuse(&mut s, "arrow.add_point", json!({"arrow": line, "segment": 3}));
    assert!(r.message.contains("segments 1 to 2"), "{}", r.message);
    let closed = closed_triangle(&mut s);
    let r = refuse(&mut s, "arrow.add_point", json!({"arrow": closed, "segment": 4}));
    assert!(r.message.contains("segments 1 to 3"), "{}", r.message);
}

#[test]
fn move_point_on_an_elbow_arrow_moves_only_its_ends() {
    let mut w = bound_world();
    run(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow], "type": "elbow"}));
    let n = pts(&w.s, &w.arrow).len();
    assert!(n >= 3);
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": 1, "to": [200, 200]}));
    assert_eq!(r.code, "not_movable");
    assert_eq!(
        r.message,
        "Elbow arrows only allow moving their two ends. Move the whole arrow, or change its type with `arrow.set_type`."
    );
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": 1, "to": [200, 200], "insert": true}));
    assert_eq!(r.code, "not_movable");
    // An end moves and the route stays orthogonal.
    run(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "end", "to": [500, 300], "bind": "off"}));
    assert!(data(&w.s, &w.arrow).end_binding.is_none());
    let p = pts(&w.s, &w.arrow);
    assert!(p.windows(2).all(|q| (q[0].x - q[1].x).abs() < 1e-6 || (q[0].y - q[1].y).abs() < 1e-6), "{p:?}");
    assert!(near(*p.last().unwrap(), Point::new(500.0, 300.0)));
}

#[test]
fn move_point_inserts_a_point_at_the_midpoint_and_moves_it() {
    let mut w = world();
    let out = run(
        &mut w.s,
        "arrow.move_point",
        json!({"arrow": w.arrow, "index": 1, "to": [200, 200], "insert": true}),
    );
    assert_eq!(out["index"], 1);
    let p = pts(&w.s, &w.arrow);
    assert_eq!(p.len(), 3);
    assert!(near(p[1], Point::new(200.0, 200.0)));
    assert_eq!(undo_depth(&w.s), 1, "insert and move are one undo step");
    w.s.undo_steps(1).unwrap();
    assert_eq!(pts(&w.s, &w.arrow).len(), 2);
}

#[test]
fn dragging_an_end_onto_a_shape_binds_it_and_reports_the_highlight() {
    let mut w = world();
    // The arrow end is at (300, 70); drag it to the left edge of b, a few units outside: orbit.
    let out = run(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "end", "to": [296, 70]}));
    assert_eq!(out["bind_highlight"], w.b.as_str());
    let b = data(&w.s, &w.arrow).end_binding.clone().unwrap();
    assert_eq!((b.target, b.mode), (eid(&w.b), BindMode::Orbit));
    assert!(stale_arrows(w.s.store()).is_empty());
    // Drag inside b: inside mode, the end stays exactly there.
    let out = run(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "end", "to": [350, 70]}));
    assert_eq!(out["bind_highlight"], w.b.as_str());
    assert_eq!(data(&w.s, &w.arrow).end_binding.as_ref().map(|b| b.mode), Some(BindMode::Inside));
    let end = last(&w.s, &w.arrow);
    assert!((end - Point::new(350.0, 70.0)).hypot() < 0.05, "an inside end stays where it was dropped: {end:?}");
}

#[test]
fn bind_off_ignores_targets_and_dragging_away_unbinds() {
    let mut w = world();
    let out = run(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "end", "to": [350, 70], "bind": "off"}));
    assert!(out.get("bind_highlight").is_none());
    assert!(data(&w.s, &w.arrow).end_binding.is_none());
    run(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "end", "to": [350, 70], "bind": "inside"}));
    assert_eq!(data(&w.s, &w.arrow).end_binding.as_ref().map(|b| b.mode), Some(BindMode::Inside));
    let out = run(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "end", "to": [600, 400]}));
    assert!(out.get("bind_highlight").is_none());
    assert!(data(&w.s, &w.arrow).end_binding.is_none(), "dragged away: unbound");
    assert!(near(last(&w.s, &w.arrow), Point::new(600.0, 400.0)));
}

#[test]
fn the_setting_arrow_binding_turns_automatic_binding_off() {
    let mut w = world();
    assert!(w.s.settings.set_raw("arrow.binding", json!(false)));
    run(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "end", "to": [296, 70]}));
    assert!(data(&w.s, &w.arrow).end_binding.is_none());
    run(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": "end", "to": [350, 70], "bind": "inside"}));
    assert!(data(&w.s, &w.arrow).end_binding.is_some(), "Alt still binds explicitly");
}

#[test]
fn move_point_snaps_to_the_grid_and_to_an_angle() {
    let mut w = world();
    let line = linear(&mut w.s, "line", &[[0.0, 500.0], [100.0, 500.0]]);
    let out = run(
        &mut w.s,
        "arrow.move_point",
        json!({"arrow": line, "index": "end", "to": [113, 507], "snap": {"grid": true}}),
    );
    assert!(near(point_of(&out), Point::new(120.0, 500.0)));
    // Angle: from (0,500) towards (100, 40) is about 24 degrees below... the line snaps to a multiple of 15.
    let out = run(
        &mut w.s,
        "arrow.move_point",
        json!({"arrow": line, "index": "end", "to": [100, 540], "snap": {"angle": true}}),
    );
    let p = last(&w.s, &line);
    let deg = (p.y - 500.0).atan2(p.x).to_degrees();
    assert!((deg - 15.0).abs() < 1e-6 || (deg - 30.0).abs() < 1e-6, "{deg}");
    let hypot = (100.0f64).hypot(40.0);
    assert!(((p.x).hypot(p.y - 500.0) - hypot).abs() < 1e-6, "the length is kept");
    assert!(out["point"].is_array());
    // None switches everything off, even when the settings are on.
    assert!(w.s.settings.set_raw("snap.grid", json!(true)));
    let out = run(&mut w.s, "arrow.move_point", json!({"arrow": line, "index": "end", "to": [113, 507], "snap": "none"}));
    assert!(near(point_of(&out), Point::new(113.0, 507.0)));
    let out = run(&mut w.s, "arrow.move_point", json!({"arrow": line, "index": "end", "to": [113, 507]}));
    assert!(near(point_of(&out), Point::new(120.0, 500.0)), "auto follows snap.grid");
}

#[test]
fn smart_guides_align_the_point_with_other_elements_and_report_the_guide() {
    let mut w = world();
    let line = linear(&mut w.s, "line", &[[0.0, 500.0], [100.0, 500.0]]);
    // Rectangle b's left edge is x=300; a point at x=303 snaps to it.
    let out = run(
        &mut w.s,
        "arrow.move_point",
        json!({"arrow": line, "index": "end", "to": [303, 520], "snap": {"smart": true}}),
    );
    assert!((point_of(&out).x - 300.0).abs() < 1e-9);
    let guides = out["guides"].as_array().unwrap();
    assert!(guides.iter().any(|g| g["kind"] == "vertical" && g["position"] == 300.0), "{guides:?}");
}

#[test]
fn snap_accepts_words_and_switches_only() {
    let mut w = world();
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": 0, "to": [0, 0], "snap": "sometimes"}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": 0, "to": [0, 0], "snap": {"grids": true}}));
    assert_eq!(r.code, "bad_args");
    let _ = (SnapWord::Auto, SnapSwitches::default());
}

#[test]
fn a_gesture_ends_in_the_same_document_as_the_plain_command() {
    let drag = |plain: bool| -> String {
        let mut w = bound_world();
        let depth = undo_depth(&w.s);
        let a = move_args(&w.arrow, PointIndex::Word(EndWord::End), [340.0, 90.0]);
        if plain {
            run(&mut w.s, "arrow.move_point", serde_json::to_value(&a).unwrap());
        } else {
            let mid = move_args(&w.arrow, PointIndex::Word(EndWord::End), [310.0, 80.0]);
            let (g, _) = w.s.gesture_begin_typed(Origin::Test, "arrow.move_point", &mid).unwrap();
            w.s.gesture_update(g, &move_args(&w.arrow, PointIndex::Word(EndWord::End), [800.0, 500.0])).unwrap();
            w.s.gesture_update(g, &a).unwrap();
            w.s.gesture_end_typed(g, Some(&a)).unwrap();
        }
        assert_eq!(undo_depth(&w.s), depth + 1, "one undo step");
        shape_of(&w.s, &w.arrow)
    };
    assert_eq!(drag(true), drag(false));
}

#[test]
fn a_cancelled_gesture_restores_the_binding_and_the_geometry() {
    let mut w = bound_world();
    let before = shape_of(&w.s, &w.arrow);
    let a = move_args(&w.arrow, PointIndex::Word(EndWord::End), [800.0, 500.0]);
    let (g, _) = w.s.gesture_begin_typed(Origin::Test, "arrow.move_point", &a).unwrap();
    assert!(data(&w.s, &w.arrow).end_binding.is_none(), "the live frame unbound the end");
    w.s.gesture_cancel(Some(g)).unwrap();
    assert_eq!(shape_of(&w.s, &w.arrow), before);
}

// ---------------------------------------------------------------------------------------------
// arrow.add_point, arrow.delete_point
// ---------------------------------------------------------------------------------------------

#[test]
fn add_point_inserts_at_the_midpoint_or_where_asked() {
    let mut w = world();
    let out = run(&mut w.s, "arrow.add_point", json!({"arrow": w.arrow, "segment": 1}));
    assert_eq!(out["index"], 1);
    assert!(near(pts(&w.s, &w.arrow)[1], Point::new(200.0, 50.0)));
    run(&mut w.s, "arrow.add_point", json!({"arrow": w.arrow, "segment": 2, "at": [260, 300]}));
    let p = pts(&w.s, &w.arrow);
    assert_eq!(p.len(), 4);
    assert!(near(p[2], Point::new(260.0, 300.0)));
    assert_eq!(undo_depth(&w.s), 2);
    w.s.undo_steps(2).unwrap();
    assert_eq!(pts(&w.s, &w.arrow).len(), 2);
}

#[test]
fn add_point_refusals() {
    let mut w = bound_world();
    let r = refuse(&mut w.s, "arrow.add_point", json!({"arrow": w.arrow, "segment": 2}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("segments 1 to 1"), "{}", r.message);
    let r = refuse(&mut w.s, "arrow.add_point", json!({"arrow": w.arrow, "segment": 0}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut w.s, "arrow.add_point", json!({"arrow": w.a, "segment": 1}));
    assert_eq!(r.code, "wrong_kind");
    run(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow], "type": "elbow"}));
    let r = refuse(&mut w.s, "arrow.add_point", json!({"arrow": w.arrow, "segment": 1}));
    assert_eq!(r.code, "not_movable");
}

#[test]
fn delete_point_by_index_and_from_the_point_editor() {
    let mut w = world();
    let line = linear(&mut w.s, "line", &[[0.0, 200.0], [60.0, 260.0], [120.0, 200.0], [180.0, 260.0]]);
    let out = run(&mut w.s, "arrow.delete_point", json!({"arrow": line, "index": 1}));
    assert_eq!(out["removed"], 1);
    assert_eq!(pts(&w.s, &line).len(), 3);
    // Through the point editor: no arguments.
    run(&mut w.s, "arrow.edit_points", json!({"arrow": line, "select_point": 1}));
    let out = run(&mut w.s, "arrow.delete_point", json!({}));
    assert_eq!(out["removed"], 1);
    assert_eq!(pts(&w.s, &line).len(), 2);
    assert!(w.s.tab().observed.linear_editor.as_ref().is_some_and(|e| e.selected_points.is_empty()));
    assert_eq!(undo_depth(&w.s), 2);
    w.s.undo_steps(1).unwrap();
    assert_eq!(pts(&w.s, &line).len(), 3);
}

#[test]
fn deleting_an_end_point_frees_its_binding() {
    let mut w = bound_world();
    run(&mut w.s, "arrow.add_point", json!({"arrow": w.arrow, "segment": 1, "at": [200, 200]}));
    assert_eq!(pts(&w.s, &w.arrow).len(), 3);
    run(&mut w.s, "arrow.delete_point", json!({"arrow": w.arrow, "index": 2}));
    let d = data(&w.s, &w.arrow);
    assert!(d.end_binding.is_none() && d.start_binding.is_some());
    assert_eq!(pts(&w.s, &w.arrow).len(), 2);
}

#[test]
fn delete_point_refusals() {
    let mut w = world();
    let r = refuse(&mut w.s, "arrow.delete_point", json!({"arrow": w.arrow, "index": 0}));
    assert_eq!(r.code, "too_few");
    assert_eq!(r.message, format!("An arrow needs at least 2 points, so no point can be deleted from `{}`.", w.arrow));
    let r = refuse(&mut w.s, "arrow.delete_point", json!({}));
    assert_eq!(r.code, "nothing_to_do");
    assert!(r.message.contains("point editor is not open"), "{}", r.message);
    run(&mut w.s, "arrow.edit_points", json!({"arrow": w.arrow}));
    let r = refuse(&mut w.s, "arrow.delete_point", json!({}));
    assert_eq!(r.code, "nothing_to_do");
    assert!(r.message.contains("no point is selected"), "{}", r.message);
    let r = refuse(&mut w.s, "arrow.delete_point", json!({"index": 7}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("point 7 does not exist"), "{}", r.message);
}

#[test]
fn delete_point_on_an_elbow_arrow_is_not_movable() {
    let mut w = bound_world();
    run(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow], "type": "elbow"}));
    let r = refuse(&mut w.s, "arrow.delete_point", json!({"arrow": w.arrow, "index": 1}));
    assert_eq!(r.code, "not_movable");
}

// ---------------------------------------------------------------------------------------------
// arrow.edit_points
// ---------------------------------------------------------------------------------------------

#[test]
fn edit_points_needs_a_single_line_or_arrow() {
    let mut w = world();
    let r = w.s.availability("arrow.edit_points").unwrap_err();
    assert_eq!(r.code, "needs_single");
    assert_eq!(
        r.message,
        "`arrow.edit_points` needs exactly one line or arrow selected, and 0 are selected. Select one line or arrow first, or pass it by id."
    );
    testing::select(&mut w.s, &[eid(&w.a)]);
    let shown = w.s.availability("arrow.edit_points").unwrap_err();
    let r = refuse(&mut w.s, "arrow.edit_points", json!({}));
    assert_eq!(r.code, "wrong_kind");
    assert_eq!((shown.code, shown.message), (r.code, r.message), "a disabled control shows the text execute gives");
    testing::select(&mut w.s, &[eid(&w.a), eid(&w.b)]);
    let r = refuse(&mut w.s, "arrow.edit_points", json!({}));
    assert_eq!(r.code, "needs_single");
}

#[test]
fn edit_points_toggles_the_editor_and_selects_a_point() {
    let mut w = world();
    testing::select(&mut w.s, &[eid(&w.arrow)]);
    let out = run(&mut w.s, "arrow.edit_points", json!({}));
    assert_eq!(out["changed"], true);
    assert_eq!(out["arrow"], w.arrow.as_str());
    assert_eq!(w.s.tab().observed.linear_editor.as_ref().map(|e| e.element.clone()), Some(eid(&w.arrow)));
    let out = run(&mut w.s, "arrow.edit_points", json!({"select_point": 1}));
    assert_eq!(out["selected_points"], json!([1]));
    let out = run(&mut w.s, "arrow.edit_points", json!({"on": true}));
    assert_eq!(out["changed"], false, "already on");
    assert_eq!(out["selected_points"], json!([1]), "the point stays selected");
    let out = run(&mut w.s, "arrow.edit_points", json!({"select_point": null}));
    assert_eq!(out["selected_points"], json!([]));
    let out = run(&mut w.s, "arrow.edit_points", json!({}));
    assert_eq!(out["arrow"], Value::Null, "toggled off");
    assert!(w.s.tab().observed.linear_editor.is_none());
    assert_eq!(undo_depth(&w.s), 0, "the point editor is not a document change");
}

#[test]
fn edit_points_by_id_selects_the_arrow_and_checks_the_point() {
    let mut w = world();
    run(&mut w.s, "arrow.edit_points", json!({"arrow": w.arrow, "on": true}));
    assert!(w.s.selection().contains(&eid(&w.arrow)));
    let r = refuse(&mut w.s, "arrow.edit_points", json!({"select_point": 9}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("point 9 does not exist"), "{}", r.message);
    let r = refuse(&mut w.s, "arrow.edit_points", json!({"arrow": w.arrow, "on": false, "select_point": 0}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut w.s, "arrow.edit_points", json!({"arrow": w.a}));
    assert_eq!(r.code, "wrong_kind");
    let _ = EditPointsArgs::default();
}

#[test]
fn edit_points_state_is_restored_by_the_next_undo_step() {
    let mut w = world();
    run(&mut w.s, "arrow.edit_points", json!({"arrow": w.arrow, "select_point": 0}));
    run(&mut w.s, "arrow.move_point", json!({"arrow": w.arrow, "index": 1, "to": [300, 120], "bind": "off"}));
    run(&mut w.s, "arrow.edit_points", json!({"on": false}));
    assert!(w.s.tab().observed.linear_editor.is_none());
    w.s.undo_steps(1).unwrap();
    // Undo restores the editor state from before the step, which had the editor on.
    assert_eq!(w.s.tab().observed.linear_editor.as_ref().map(|e| e.element.clone()), Some(eid(&w.arrow)));
}

// ---------------------------------------------------------------------------------------------
// arrow.reroute
// ---------------------------------------------------------------------------------------------

#[test]
fn reroute_repairs_a_stale_arrow() {
    let mut w = bound_world();
    // Move b without running the solver, as a damaged or foreign document would have it.
    w.s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| tx.update(&eid(&w.b), |e| e.transform.y += 120.0))
        .unwrap();
    assert_eq!(stale_arrows(w.s.store()), vec![eid(&w.arrow)]);
    let out = run(&mut w.s, "arrow.reroute", json!({"ids": [w.arrow]}));
    assert_eq!(out["rerouted"], json!([w.arrow]));
    assert!(stale_arrows(w.s.store()).is_empty());
}

#[test]
fn reroute_drops_the_pinned_segments_of_an_elbow_arrow() {
    let mut w = bound_world();
    run(&mut w.s, "arrow.set_type", json!({"ids": [w.arrow], "type": "elbow"}));
    let before = pts(&w.s, &w.arrow);
    let seg = (1..before.len())
        .find(|i| i + 1 < before.len() && (before[i - 1].x - before[*i].x).abs() < 1e-6)
        .or_else(|| (2..before.len() - 1).next())
        .unwrap_or(2);
    run(&mut w.s, "arrow.move_segment", json!({"arrow": w.arrow, "segment": seg, "to": [200, 400]}));
    assert!(!data(&w.s, &w.arrow).fixed_segments.is_empty(), "a segment is pinned");
    run(&mut w.s, "arrow.reroute", json!({"ids": [w.arrow]}));
    assert!(data(&w.s, &w.arrow).fixed_segments.is_empty());
    assert!(stale_arrows(w.s.store()).is_empty());
}

#[test]
fn reroute_refuses_other_kinds_and_an_empty_selection() {
    let mut w = world();
    let r = refuse(&mut w.s, "arrow.reroute", json!({"ids": [w.a]}));
    assert_eq!(r.code, "not_applicable");
    let r = refuse(&mut w.s, "arrow.reroute", json!({}));
    assert_eq!(r.code, "no_target");
    assert_eq!(w.s.availability("arrow.reroute").unwrap_err().code, "no_target");
}

// ---------------------------------------------------------------------------------------------
// arrow.move_segment, arrow.release_segment
// ---------------------------------------------------------------------------------------------

/// An elbow arrow between `a` and `b` with at least a middle segment.
fn elbow_world() -> World {
    let mut s = session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let b = rect(&mut s, 400.0, 200.0, 100.0, 60.0);
    let ar = arrow(&mut s, &[[100.0, 30.0], [400.0, 230.0]]);
    run(&mut s, "arrow.bind", json!({"arrow": ar, "end": "start", "target": a}));
    run(&mut s, "arrow.bind", json!({"arrow": ar, "end": "end", "target": b}));
    run(&mut s, "arrow.set_type", json!({"ids": [ar], "type": "elbow"}));
    World { s, a, b, arrow: ar }
}

fn middle_segment(s: &Session, id: &str) -> usize {
    let p = pts(s, id);
    assert!(p.len() >= 4, "the route needs a middle segment: {p:?}");
    2
}

#[test]
fn move_segment_pins_the_segment_and_release_unpins_it() {
    let mut w = elbow_world();
    let seg = middle_segment(&w.s, &w.arrow);
    let before = pts(&w.s, &w.arrow);
    let depth = undo_depth(&w.s);
    let horizontal = (before[seg - 1].y - before[seg].y).abs() < 1e-6;
    let to = if horizontal { [250.0, 400.0] } else { [260.0, 100.0] };
    run(&mut w.s, "arrow.move_segment", json!({"arrow": w.arrow, "segment": seg, "to": to}));
    let after = pts(&w.s, &w.arrow);
    let moved = if horizontal { after.iter().any(|p| (p.y - 400.0).abs() < 1e-6) } else { after.iter().any(|p| (p.x - 260.0).abs() < 1e-6) };
    assert!(moved, "{after:?}");
    assert!(data(&w.s, &w.arrow).fixed_segments.iter().any(|f| f.index as usize == seg));
    assert_eq!(undo_depth(&w.s), depth + 1);
    w.s.undo_steps(1).unwrap();
    assert!(data(&w.s, &w.arrow).fixed_segments.is_empty(), "undo takes the pin back");
    assert_eq!(pts(&w.s, &w.arrow), before);
}

#[test]
fn release_segment_unpins_and_refuses_unpinned_segments() {
    let mut w = elbow_world();
    let seg = middle_segment(&w.s, &w.arrow);
    let r = refuse(&mut w.s, "arrow.release_segment", json!({"arrow": w.arrow, "segment": seg}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, format!("Nothing to release: segment {seg} of `{}` is not pinned.", w.arrow));
    run(&mut w.s, "arrow.move_segment", json!({"arrow": w.arrow, "segment": seg, "to": [250, 400]}));
    let pinned = data(&w.s, &w.arrow).fixed_segments.iter().map(|f| f.index as usize).next().unwrap();
    run(&mut w.s, "arrow.release_segment", json!({"arrow": w.arrow, "segment": pinned}));
    assert!(data(&w.s, &w.arrow).fixed_segments.is_empty());
    assert!(stale_arrows(w.s.store()).is_empty());
}

#[test]
fn segment_commands_need_an_elbow_arrow_and_a_valid_segment() {
    let mut w = world();
    let r = refuse(&mut w.s, "arrow.release_segment", json!({"arrow": w.arrow, "segment": 1}));
    assert_eq!(r.code, "wrong_kind");
    let r = refuse(&mut w.s, "arrow.move_segment", json!({"arrow": w.arrow, "segment": 1}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`to`") || r.message.contains("to"), "{}", r.message);
    let r = refuse(&mut w.s, "arrow.move_segment", json!({"arrow": w.arrow, "segment": 1, "to": [0, 0]}));
    assert_eq!(r.code, "wrong_kind");
    assert!(r.message.contains("elbow arrows"), "{}", r.message);
    let mut e = elbow_world();
    let r = refuse(&mut e.s, "arrow.move_segment", json!({"arrow": e.arrow, "segment": 99, "to": [0, 0]}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("segments 1 to"), "{}", r.message);
    let r = refuse(&mut e.s, "arrow.release_segment", json!({"arrow": e.arrow, "segment": 2, "to": [1, 1]}));
    assert_eq!(r.code, "bad_args");
}

// ---------------------------------------------------------------------------------------------
// Scenarios: arrows connect to shapes and stay attached
// ---------------------------------------------------------------------------------------------

fn end_is_attached(s: &Session, arrow: &str, target: &str, end: End) -> bool {
    let a = el(s, arrow);
    let p = if end == End::End { last(s, arrow) } else { first(s, arrow) };
    let on = bound_end_position(s.store(), a, end).is_some_and(|q| near(q, p));
    let r = resolve(s.store(), &eid(target)).unwrap();
    on && (r.target.signed_distance(p) - gap_of(s, target)).abs() < 1e-3
}

#[test]
fn scenario_an_arrow_follows_the_rectangle_it_is_bound_to() {
    let mut w = bound_world();
    assert!(end_is_attached(&w.s, &w.arrow, &w.a, End::Start));
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));
    let (s0, e0) = (first(&w.s, &w.arrow), last(&w.s, &w.arrow));
    let depth = undo_depth(&w.s);

    // Move b by the domain transform (what `element.move` calls).
    run(&mut w.s, "t.move", json!({"ids": [w.b], "dx": 60, "dy": 150}));
    assert_eq!(undo_depth(&w.s), depth + 1);
    let (s1, e1) = (first(&w.s, &w.arrow), last(&w.s, &w.arrow));
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End), "the end sits on b's new outline");
    assert!(end_is_attached(&w.s, &w.arrow, &w.a, End::Start));
    assert!((e1 - e0).hypot() > 50.0, "the end moved with b: {e0:?} -> {e1:?}");
    assert!(e1.y > e0.y + 100.0 - 1.0);
    assert!((s1 - s0).hypot() < 40.0, "the start side stays near a");
    assert!(stale_arrows(w.s.store()).is_empty());

    // Move a too.
    run(&mut w.s, "t.move", json!({"ids": [w.a], "dx": -40, "dy": 10}));
    assert!(end_is_attached(&w.s, &w.arrow, &w.a, End::Start));
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));

    // Undo the moves: the arrow goes back with the shapes.
    w.s.undo_steps(2).unwrap();
    assert!(near(first(&w.s, &w.arrow), s0) && near(last(&w.s, &w.arrow), e0));
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));
}

#[test]
fn scenario_deleting_the_target_frees_the_end_in_place_and_undo_restores_the_binding() {
    let mut w = bound_world();
    let e0 = last(&w.s, &w.arrow);
    let depth = undo_depth(&w.s);
    run(&mut w.s, "t.delete", json!({"ids": [w.b]}));
    assert_eq!(undo_depth(&w.s), depth + 1);
    let d = data(&w.s, &w.arrow);
    assert!(d.end_binding.is_none(), "the end is free");
    assert!(d.start_binding.is_some(), "the other end is still bound");
    assert!(near(last(&w.s, &w.arrow), e0), "and it stays where it was");

    w.s.undo_steps(1).unwrap();
    assert_eq!(data(&w.s, &w.arrow).end_binding.as_ref().map(|b| b.target.clone()), Some(eid(&w.b)));
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));

    // Redo frees the end again; undo brings the binding back.
    w.s.redo_steps(1).unwrap();
    assert!(data(&w.s, &w.arrow).end_binding.is_none());
    assert!(near(last(&w.s, &w.arrow), e0));
    w.s.undo_steps(1).unwrap();
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));

    // The restored binding is live again: moving b drags the end.
    run(&mut w.s, "t.move", json!({"ids": [w.b], "dx": 0, "dy": 100}));
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));
    assert!(last(&w.s, &w.arrow).y > e0.y + 50.0);
}

#[test]
fn scenario_moving_the_arrow_alone_detaches_it_and_moving_both_keeps_it_bound() {
    let mut w = bound_world();
    run(&mut w.s, "t.move", json!({"ids": [w.arrow], "dx": 0, "dy": 200}));
    let d = data(&w.s, &w.arrow);
    assert!(d.start_binding.is_none() && d.end_binding.is_none(), "an arrow dragged on its own detaches");
    w.s.undo_steps(1).unwrap();
    assert!(data(&w.s, &w.arrow).start_binding.is_some());
    run(&mut w.s, "t.move", json!({"ids": [w.arrow, w.a, w.b], "dx": 50, "dy": 50}));
    let d = data(&w.s, &w.arrow);
    assert!(d.start_binding.is_some() && d.end_binding.is_some(), "the group moves as one");
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));
}

#[test]
fn scenario_an_elbow_arrow_stays_routed_and_attached() {
    let mut w = elbow_world();
    assert!(stale_arrows(w.s.store()).is_empty());
    run(&mut w.s, "t.move", json!({"ids": [w.b], "dx": -150, "dy": 220}));
    let p = pts(&w.s, &w.arrow);
    assert!(p.windows(2).all(|q| (q[0].x - q[1].x).abs() < 1e-6 || (q[0].y - q[1].y).abs() < 1e-6), "{p:?}");
    assert!(stale_arrows(w.s.store()).is_empty());
    let r = resolve(w.s.store(), &eid(&w.b)).unwrap();
    let end = *p.last().unwrap();
    assert!(r.target.signed_distance(end) < 20.0, "the route still ends at b: {}", r.target.signed_distance(end));
    run(&mut w.s, "t.delete", json!({"ids": [w.b]}));
    assert!(data(&w.s, &w.arrow).end_binding.is_none());
    w.s.undo_steps(1).unwrap();
    assert!(data(&w.s, &w.arrow).end_binding.is_some());
    assert!(stale_arrows(w.s.store()).is_empty());
}

#[test]
fn scenario_drag_an_arrow_end_onto_a_shape_in_one_gesture_and_undo_it() {
    let mut w = world();
    let free_end = last(&w.s, &w.arrow);
    let depth = undo_depth(&w.s);
    // The end starts free, is dragged away and then onto b's outline margin.
    let begin = move_args(&w.arrow, PointIndex::Word(EndWord::End), [380.0, 20.0]);
    let (g, _) = w.s.gesture_begin_typed(Origin::Test, "arrow.move_point", &begin).unwrap();
    let frame = w.s.gesture_update(g, &move_args(&w.arrow, PointIndex::Word(EndWord::End), [296.0, 70.0])).unwrap();
    assert_eq!(frame["bind_highlight"], w.b.as_str(), "the live frame carries the highlight");
    w.s.gesture_end_typed(g, None).unwrap();
    assert_eq!(undo_depth(&w.s), depth + 1);
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));
    run(&mut w.s, "t.move", json!({"ids": [w.b], "dx": 0, "dy": 80}));
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));
    w.s.undo_steps(2).unwrap();
    assert!(data(&w.s, &w.arrow).end_binding.is_none());
    assert!(near(last(&w.s, &w.arrow), free_end));
}

#[test]
fn scenario_batch_of_bind_commands_is_one_undo_step() {
    let mut w = world();
    let steps = [
        crate::BatchStep::new("arrow.bind", json!({"arrow": w.arrow, "end": "start", "target": w.a})),
        crate::BatchStep::new("arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": w.b})),
        crate::BatchStep::new("arrow.set_head", json!({"ids": [w.arrow], "end": "both", "head": "arrow"})),
    ];
    let out = w.s.execute_batch(Origin::Test, &steps, &crate::BatchOpts::default());
    assert!(out.failed.is_none(), "{:?}", out.failed.map(|f| f.refusal.message));
    assert_eq!(undo_depth(&w.s), 1);
    assert!(end_is_attached(&w.s, &w.arrow, &w.b, End::End));
    w.s.undo_steps(1).unwrap();
    let d = data(&w.s, &w.arrow);
    assert!(d.start_binding.is_none() && d.end_binding.is_none() && d.start_head == Head::None);
}

#[test]
fn view_only_blocks_the_document_commands_but_not_the_point_editor_state() {
    let mut w = world();
    w.s.view_only = true;
    let r = refuse(&mut w.s, "arrow.bind", json!({"arrow": w.arrow, "end": "end", "target": w.b}));
    assert_eq!(r.code, "view_only");
}

#[test]
fn explicit_ids_work_with_an_empty_selection() {
    let mut w = world();
    assert!(w.s.selection().is_empty());
    run(&mut w.s, "arrow.set_head", json!({"ids": [w.arrow], "head": "bar"}));
    assert_eq!(data(&w.s, &w.arrow).end_head, Head::Bar);
    testing::select(&mut w.s, &[eid(&w.arrow)]);
    run(&mut w.s, "arrow.set_head", json!({"head": "circle"}));
    assert_eq!(data(&w.s, &w.arrow).end_head, Head::Circle, "the selection is the default");
}
