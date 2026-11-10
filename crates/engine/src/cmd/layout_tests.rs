//! Tests of `layout.apply`: shapes laid out by an algorithm, bound arrows kept and re-routed, labels kept with their
//! shapes, one undo step.

use drafftink_scene::{ElementId, Kind, SceneRead};
use serde_json::{Value, json};

use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

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
    assert_eq!(s.fingerprint(), before, "{id} refused but changed the session");
    r
}

fn eid(v: &Value) -> ElementId {
    ElementId::new(v.as_str().unwrap()).unwrap()
}

/// Four labelled nodes in a diamond-shaped graph, then piled up on top of each other.
fn piled_up() -> (Session, Vec<ElementId>, Vec<ElementId>) {
    let (mut s, _f) = testing::session();
    let out = run(
        &mut s,
        "diagram.create",
        json!({"nodes": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}, {"id": "c", "label": "C"}, {"id": "d", "label": "D"}],
               "edges": [{"from": "a", "to": "b"}, {"from": "a", "to": "c"}, {"from": "b", "to": "d"}, {"from": "c", "to": "d"}],
               "at": [0, 0]}),
    );
    let nodes: Vec<ElementId> = ["a", "b", "c", "d"].iter().map(|k| eid(&out["nodes"][k])).collect();
    let edges: Vec<ElementId> = out["edges"].as_array().unwrap().iter().map(eid).collect();
    for (i, n) in nodes.iter().enumerate() {
        let n = n.clone();
        s.tab_mut()
            .doc
            .edit::<(), drafftink_scene::StoreError>(|tx| {
                use drafftink_scene::SceneTx;
                tx.update(&n, |e| {
                    e.transform.x = 10.0 * i as f64;
                    e.transform.y = 5.0 * i as f64;
                })
            })
            .unwrap();
    }
    s.tab_mut().doc.commit_untracked().unwrap();
    s.tab_mut().observed.selection.clear();
    (s, nodes, edges)
}

fn overlaps(s: &Session, ids: &[ElementId]) -> bool {
    let rs: Vec<_> = ids.iter().map(|i| s.element(i).unwrap().rect()).collect();
    drafftink_edit::layout::rects_overlap(&rs, 0.5)
}

#[test]
fn layered_layout_untangles_the_nodes_keeps_arrows_bound_and_is_one_undo_step() {
    let (mut s, nodes, edges) = piled_up();
    testing::select(&mut s, &nodes);
    let depth = s.tab().doc.undo_depth();
    let before = s.element(&nodes[0]).unwrap().rect();
    let out = run(&mut s, "layout.apply", json!({"algorithm": "layered", "direction": "right"}));
    assert!(!overlaps(&s, &nodes));
    assert_eq!(s.tab().doc.undo_depth(), depth + 1);
    // The top-left of the set stays where it was (anchor keep); layers go left to right.
    let (ra, rb, rd) = (
        s.element(&nodes[0]).unwrap().rect(),
        s.element(&nodes[1]).unwrap().rect(),
        s.element(&nodes[3]).unwrap().rect(),
    );
    assert!(ra.x1 < rb.x0 && rb.x1 < rd.x0);
    for e in &edges {
        let t = s.element(e).unwrap().binding_targets();
        assert!(t.iter().all(Option::is_some), "arrows stay bound");
    }
    assert!(out["rerouted"].as_array().unwrap().len() == 4);
    // Labels stay centred in their shapes.
    for n in &nodes {
        let l = drafftink_edit::text_bind::label_of(s.store(), n).unwrap();
        let (c, lc) = (s.element(n).unwrap().rect().center(), s.element(&l).unwrap().rect().center());
        assert!((c - lc).hypot() < 1.0);
    }
    s.undo_steps(1).unwrap();
    assert_eq!(s.element(&nodes[0]).unwrap().rect(), before);
}

#[test]
fn anchor_center_and_top_left_with_at() {
    let (mut s, nodes, _e) = piled_up();
    testing::select(&mut s, &nodes);
    let union = |s: &Session| nodes.iter().map(|n| s.element(n).unwrap().rect()).reduce(|a, b| a.union(b)).unwrap();
    let old = union(&s);
    run(&mut s, "layout.apply", json!({"algorithm": "grid", "anchor": "center"}));
    let c = union(&s).center();
    assert!((c - old.center()).hypot() < 1.0, "{c:?} vs {:?}", old.center());
    run(&mut s, "layout.apply", json!({"algorithm": "grid", "anchor": "top_left", "at": [500, 700]}));
    let u = union(&s);
    assert!((u.x0 - 500.0).abs() < 1.0 && (u.y0 - 700.0).abs() < 1.0, "{u:?}");
    let r = refuse(&mut s, "layout.apply", json!({"algorithm": "grid", "at": [1, 2]}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("anchor"), "{}", r.message);
}

#[test]
fn the_grid_layout_takes_columns_and_a_filter_selects_the_shapes() {
    let (mut s, nodes, _e) = piled_up();
    run(&mut s, "layout.apply", json!({"filter": {"kind": "rectangle"}, "algorithm": "grid", "columns": 2}));
    let r: Vec<_> = nodes.iter().map(|n| s.element(n).unwrap().rect()).collect();
    assert!((r[0].y0 - r[1].y0).abs() < 1.0 && (r[2].y0 - r[3].y0).abs() < 1.0 && r[2].y0 > r[0].y1);
    let e = refuse(&mut s, "layout.apply", json!({"filter": {"kind": "diamond"}}));
    assert_eq!(e.code, "no_target");
    let e = refuse(&mut s, "layout.apply", json!({"filter": {"kind": "rectangle"}, "ids": [nodes[0].as_str()]}));
    assert_eq!(e.code, "bad_args");
}

#[test]
fn layout_refusals() {
    let (mut s, nodes, _e) = piled_up();
    let e = refuse(&mut s, "layout.apply", json!({}));
    assert_eq!(e.code, "no_target");
    testing::select(&mut s, &nodes[..1]);
    let e = s.availability("layout.apply").unwrap_err();
    assert_eq!(e.code, "too_few");
    assert_eq!(e.message, "`layout.apply` needs at least 2 elements, and 1 were given. Select more, or pass `ids`.");
    // A cycle breaks the tree layout.
    let ids: Vec<&str> = nodes.iter().map(ElementId::as_str).collect();
    let e = refuse(&mut s, "layout.apply", json!({"ids": ids, "algorithm": "tree"}));
    assert_eq!(e.code, "tree_not_forest");
    assert!(e.message.contains("Use algorithm \"layered\", which handles cycles."), "{}", e.message);
    // The diamond-shaped graph has a node with two incoming edges, so it is not a forest either.
    assert_eq!(refuse(&mut s, "layout.apply", json!({"ids": ids, "columns": 0})).code, "bad_args");
    assert_eq!(refuse(&mut s, "layout.apply", json!({"ids": ["nope", "nada"]})).code, "not_found");
    s.tab_mut()
        .doc
        .edit::<(), drafftink_scene::StoreError>(|tx| {
            use drafftink_scene::SceneTx;
            tx.update(&nodes[2], |e| e.locked = true)
        })
        .unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    let e = refuse(&mut s, "layout.apply", json!({"ids": ids}));
    assert_eq!(e.code, "locked");
    assert!(e.message.contains("`element.unlock_all`"), "{}", e.message);
}

#[test]
fn an_arrow_between_non_selected_shapes_is_left_alone() {
    let (mut s, nodes, edges) = piled_up();
    let a_before = s.element(&edges[0]).unwrap().clone();
    let ids: Vec<&str> = vec![nodes[2].as_str(), nodes[3].as_str()];
    run(&mut s, "layout.apply", json!({"ids": ids, "algorithm": "grid"}));
    // The arrow a->b has b outside the set; only c and d moved. It is still bound to both ends.
    let a_after = s.element(&edges[0]).unwrap();
    assert_eq!(a_after.binding_targets(), a_before.binding_targets());
    assert_eq!(s.store().iter().filter(|e| e.tag() == Kind::Arrow).count(), 4);
}
