//! `layout_apply`: re-layout of existing elements, arrows and labels follow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_edit::TargetError;
use drafftink_edit::layout::{
    Algorithm, ApplyError, ApplyOpts, CreateOpts, DiagramResult, DiagramSpec, Direction, LayoutAnchor, LayoutOpts,
    diagram_create, layout_apply,
};
use drafftink_edit::text_bind::MonospaceMeasure;
use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    Element, ElementId, ElementKind, Kind, MemoryStore, SceneRead, SceneStore, SceneTx, SeqIds, validate_scene,
};

fn build(json: &str, origin: Point) -> (MemoryStore, DiagramResult) {
    let spec: DiagramSpec = serde_json::from_str(json).unwrap();
    let mut store = MemoryStore::new();
    let mut ids = SeqIds::new();
    let mut tx = store.begin();
    let r = diagram_create(&mut tx, &MonospaceMeasure::default(), &mut ids, &spec, &CreateOpts { origin, frame: None })
        .unwrap();
    tx.commit().unwrap();
    (store, r)
}

const CHAIN: &str = r#"{
  "nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"},{"id":"c","label":"C"}],
  "edges":[{"from":"a","to":"b","label":"one"},{"from":"b","to":"c"}],
  "layout":{"algorithm":"grid","columns":3}
}"#;

fn apply(
    store: &mut MemoryStore,
    ids: &[ElementId],
    opts: &ApplyOpts,
) -> Result<drafftink_edit::layout::ApplyResult, ApplyError> {
    let mut tx = store.begin();
    let r = layout_apply(&mut tx, &MonospaceMeasure::default(), ids, opts)?;
    tx.commit().unwrap();
    Ok(r)
}

fn layered(direction: Direction) -> ApplyOpts {
    ApplyOpts {
        layout: LayoutOpts { algorithm: Algorithm::Layered, direction, ..LayoutOpts::default() },
        ..ApplyOpts::default()
    }
}

#[test]
fn layered_apply_moves_nodes_reroutes_arrows_and_keeps_labels() {
    let (mut s, r) = build(CHAIN, Point::new(100.0, 50.0));
    let ids: Vec<ElementId> = r.nodes.values().cloned().collect();
    let out = apply(&mut s, &ids, &layered(Direction::Down)).unwrap();
    assert_eq!(out.rerouted.len(), 2);
    let rect = |n: &str| s.get(&r.nodes[n]).unwrap().rect();
    // A chain now runs top to bottom.
    assert!(rect("a").y1 < rect("b").y0 && rect("b").y1 < rect("c").y0);
    // The top-left of the set did not move (anchor keep).
    assert_eq!(out.bounds.x0, 100.0);
    assert_eq!(out.bounds.y0, 50.0);
    // Arrows end on the new sides of the nodes, still bound.
    let a = s.get(&r.edges[0]).unwrap();
    let d = a.linear_data().unwrap();
    let start = a.transform.local_to_world(d.points[0]);
    let end = a.transform.local_to_world(*d.points.last().unwrap());
    assert!((start.y - (rect("a").y1 + 6.0)).abs() < 1e-6, "{start:?}");
    assert!((end.y - (rect("b").y0 - 6.0)).abs() < 1e-6, "{end:?}");
    assert_eq!(a.binding_targets(), [Some(&r.nodes["a"]), Some(&r.nodes["b"])]);
    // The node label moved with its node, the arrow label sits on the new path.
    let label_of = |id: &ElementId| s.iter().find(|e| e.container_id() == Some(id)).unwrap().rect();
    assert_eq!(label_of(&r.nodes["b"]).center(), rect("b").center());
    let mid = label_of(&r.edges[0]).center();
    assert!(
        (mid.x - (start.x + end.x) / 2.0).abs() < 1e-6 && (mid.y - (start.y + end.y) / 2.0).abs() < 1e-6,
        "{mid:?}"
    );
    let report = validate_scene(&s);
    assert!(!report.has_errors(), "{report:?}");
}

#[test]
fn left_direction_flows_right_to_left() {
    let (mut s, r) = build(CHAIN, Point::ZERO);
    let ids: Vec<ElementId> = r.nodes.values().cloned().collect();
    apply(&mut s, &ids, &layered(Direction::Left)).unwrap();
    let x = |n: &str| s.get(&r.nodes[n]).unwrap().rect().x0;
    assert!(x("a") > x("b") && x("b") > x("c"));
}

#[test]
fn anchors() {
    let (mut s, r) = build(CHAIN, Point::new(200.0, 300.0));
    let ids: Vec<ElementId> = r.nodes.values().cloned().collect();
    let before = ids.iter().map(|i| s.get(i).unwrap().rect()).reduce(|a, b| a.union(b)).unwrap();
    let out = apply(&mut s, &ids, &ApplyOpts { anchor: LayoutAnchor::Center, ..layered(Direction::Down) }).unwrap();
    assert!(
        (out.bounds.center().x - before.center().x).abs() < 1e-9
            && (out.bounds.center().y - before.center().y).abs() < 1e-9
    );
    let out = apply(
        &mut s,
        &ids,
        &ApplyOpts { anchor: LayoutAnchor::At(Point::new(-10.0, -20.0)), ..layered(Direction::Right) },
    )
    .unwrap();
    assert_eq!((out.bounds.x0, out.bounds.y0), (-10.0, -20.0));
}

#[test]
fn reroute_false_keeps_arrows_bound_without_rerouting() {
    // reroute=false skips the full re-route, but bound arrows must still stay attached: the
    // binding solver re-aims their ends after the nodes move.
    let (mut s, r) = build(CHAIN, Point::ZERO);
    let ids: Vec<ElementId> = r.nodes.values().cloned().collect();
    let arrow = r.edges[0].clone();
    let bind = |s: &MemoryStore| match &s.get(&arrow).unwrap().kind {
        ElementKind::Arrow(a) => (a.start_binding.clone(), a.end_binding.clone()),
        _ => panic!("edge is not an arrow"),
    };
    let before = bind(&s);
    assert!(before.0.is_some() && before.1.is_some());
    let out = apply(&mut s, &ids, &ApplyOpts { reroute: false, ..layered(Direction::Down) }).unwrap();
    assert!(out.rerouted.is_empty());
    assert_eq!(bind(&s), before, "bindings are kept");
    // The ends are already solved: solving again changes nothing.
    let solved = s.get(&arrow).unwrap().clone();
    let mut tx = s.begin();
    drafftink_edit::binding::update_bound_arrows(&mut tx, &ids).unwrap();
    tx.commit().unwrap();
    assert_eq!(s.get(&arrow).unwrap().linear_data(), solved.linear_data());
}

#[test]
fn only_the_given_nodes_move_and_unselected_arrows_are_not_edges() {
    let (mut s, r) = build(CHAIN, Point::ZERO);
    let c_before = s.get(&r.nodes["c"]).unwrap().rect();
    let pair = [r.nodes["a"].clone(), r.nodes["b"].clone()];
    let out = apply(&mut s, &pair, &layered(Direction::Down)).unwrap();
    assert_eq!(out.rerouted, vec![r.edges[0].clone()]);
    assert_eq!(s.get(&r.nodes["c"]).unwrap().rect(), c_before);
    // Bound text and arrows given in ids do not count as nodes.
    let with_extras = [r.nodes["a"].clone(), r.nodes["b"].clone(), r.edges[0].clone()];
    assert!(apply(&mut s, &with_extras, &layered(Direction::Down)).is_ok());
}

#[test]
fn refusals() {
    let (mut s, r) = build(CHAIN, Point::ZERO);
    let a = r.nodes["a"].clone();
    let b = r.nodes["b"].clone();
    let err = apply(&mut s, std::slice::from_ref(&a), &layered(Direction::Down)).unwrap_err();
    assert_eq!(err, ApplyError::TooFew(1));
    let missing = ElementId::new("zzz").unwrap();
    assert_eq!(
        apply(&mut s, &[a.clone(), missing.clone()], &layered(Direction::Down)).unwrap_err(),
        ApplyError::Target(TargetError::NotFound(missing))
    );
    let mut tx = s.begin();
    tx.update(&b, |e| e.locked = true).unwrap();
    tx.commit().unwrap();
    let err = apply(&mut s, &[a.clone(), b.clone()], &layered(Direction::Down)).unwrap_err();
    assert_eq!(err, ApplyError::Target(TargetError::Locked(vec![b.clone()])));
    // Tree layout on a diamond-shaped graph: the node with two parents is named.
    let (mut s2, r2) = build(
        r#"{"nodes":[{"id":"r"},{"id":"x"},{"id":"y"},{"id":"z"}],"edges":[{"from":"r","to":"x"},{"from":"r","to":"y"},{"from":"x","to":"z"},{"from":"y","to":"z"}]}"#,
        Point::ZERO,
    );
    let ids: Vec<ElementId> = r2.nodes.values().cloned().collect();
    let tree = ApplyOpts {
        layout: LayoutOpts { algorithm: Algorithm::Tree, ..LayoutOpts::default() },
        ..ApplyOpts::default()
    };
    let before: Vec<_> = ids.iter().map(|i| s2.get(i).unwrap().rect()).collect();
    let err = apply(&mut s2, &ids, &tree).unwrap_err();
    assert_eq!(err, ApplyError::TreeNotForest(r2.nodes["z"].clone()));
    let after: Vec<_> = ids.iter().map(|i| s2.get(i).unwrap().rect()).collect();
    assert_eq!(before, after, "a refusal changes nothing");
}

#[test]
fn works_on_plain_shapes_without_arrows() {
    let mut s = MemoryStore::new();
    let mut tx = s.begin();
    let mut ids = Vec::new();
    for i in 0..6 {
        let z = tx.next_top_index().unwrap();
        let id = ElementId::new(&format!("s{i}")).unwrap();
        tx.insert(Element::new(id.clone(), ElementKind::Rectangle, z).with_bounds(0.0, 0.0, 60.0, 40.0)).unwrap();
        ids.push(id);
    }
    tx.commit().unwrap();
    let opts = ApplyOpts {
        layout: LayoutOpts { algorithm: Algorithm::Grid, columns: Some(3), ..LayoutOpts::default() },
        ..ApplyOpts::default()
    };
    let out = apply(&mut s, &ids, &opts).unwrap();
    assert_eq!(out.moved.len(), 5, "the first node already sits at the origin");
    let rects: Vec<_> = ids.iter().map(|i| s.get(i).unwrap().rect()).collect();
    assert!(!drafftink_edit::layout::rects_overlap(&rects, 1e-6));
    assert_eq!(rects[0].y0, rects[2].y0);
    assert!(rects[3].y0 > rects[0].y0);
}

#[test]
fn rotated_shapes_are_laid_out_by_their_rotated_bounds() {
    let mut s = MemoryStore::new();
    let mut tx = s.begin();
    for (i, angle) in [0.0, std::f64::consts::FRAC_PI_4].into_iter().enumerate() {
        let z = tx.next_top_index().unwrap();
        let mut e = Element::new(ElementId::new(&format!("s{i}")).unwrap(), ElementKind::Rectangle, z)
            .with_bounds(0.0, 0.0, 100.0, 20.0);
        e.transform.angle = angle;
        tx.insert(e).unwrap();
    }
    tx.commit().unwrap();
    let ids = [ElementId::new("s0").unwrap(), ElementId::new("s1").unwrap()];
    let opts = ApplyOpts {
        layout: LayoutOpts { algorithm: Algorithm::Grid, columns: Some(2), node_gap: 10.0, ..LayoutOpts::default() },
        ..ApplyOpts::default()
    };
    apply(&mut s, &ids, &opts).unwrap();
    let boxes: Vec<_> = ids
        .iter()
        .map(|i| {
            let t = s.get(i).unwrap().transform;
            t.obox().aabb()
        })
        .collect();
    assert!(boxes[1].x0 - boxes[0].x1 >= 10.0 - 1e-6, "{boxes:?}");
    let _ = Kind::Rectangle;
}
