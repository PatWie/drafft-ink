//! `diagram_create`: the spec, the errors, and what ends up in the scene.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use drafftink_edit::layout::{
    Algorithm, Arrowheads, ClusterSpec, CreateOpts, DiagramError, DiagramSpec, Direction, EdgeSpec, LayoutOpts, NodeShape, NodeSpec,
    diagram_create,
};
use drafftink_edit::text_bind::MonospaceMeasure;
use drafftink_scene::{
    ElementKind, Head, Kind, MemoryStore, PathType, SceneRead, SceneStore, SceneTx, SeqIds, validate_scene,
};

fn spec(json: &str) -> DiagramSpec {
    serde_json::from_str(json).unwrap()
}

fn create(
    store: &mut MemoryStore,
    s: &DiagramSpec,
    opts: &CreateOpts,
) -> Result<drafftink_edit::layout::DiagramResult, DiagramError> {
    let mut ids = SeqIds::new();
    let mut tx = store.begin();
    let r = diagram_create(&mut tx, &MonospaceMeasure::default(), &mut ids, s, opts)?;
    tx.commit().unwrap();
    Ok(r)
}

fn small() -> DiagramSpec {
    spec(
        r#"{
        "nodes": [
            {"id": "start", "label": "Start", "shape": "ellipse"},
            {"id": "check", "label": "Is it valid?", "shape": "diamond"},
            {"id": "ok", "label": "Done"},
            {"id": "bad", "label": "Fix the input and try again"}
        ],
        "edges": [
            {"from": "start", "to": "check"},
            {"from": "check", "to": "ok", "label": "yes"},
            {"from": "check", "to": "bad", "label": "no"},
            {"from": "bad", "to": "check"}
        ],
        "layout": {"algorithm": "layered", "direction": "down"}
    }"#,
    )
}

#[test]
fn creates_nodes_labels_and_bound_arrows_in_one_transaction() {
    let mut store = MemoryStore::new();
    let r = create(&mut store, &small(), &CreateOpts::default()).unwrap();
    assert_eq!(r.nodes.len(), 4);
    assert_eq!(r.edges.len(), 4);
    // 4 nodes + 4 node labels + 4 arrows + 2 arrow labels.
    assert_eq!(store.len(), 14);
    assert_eq!(r.created.len(), 14);
    let report = validate_scene(&store);
    assert!(!report.has_errors(), "{report:?}");
    // Every arrow is bound at both ends to the nodes named in the spec.
    let pairs = [("start", "check"), ("check", "ok"), ("check", "bad"), ("bad", "check")];
    for (arrow, (a, b)) in r.edges.iter().zip(pairs) {
        let e = store.get(arrow).unwrap();
        let [s, t] = e.binding_targets();
        assert_eq!(s, Some(&r.nodes[a]));
        assert_eq!(t, Some(&r.nodes[b]));
        let d = e.linear_data().unwrap();
        assert_eq!(d.end_head, Head::Arrow);
        assert_eq!(d.start_head, Head::None);
    }
    // Labels are bound text, directly above their container.
    let check = store.get(&r.nodes["check"]).unwrap();
    let label = store.iter().find(|e| e.container_id() == Some(&check.id)).unwrap();
    assert_eq!(label.text_data().unwrap().content, "Is it valid?");
    // One undo step: a single revision.
    assert_eq!(store.revision(), 1);
}

#[test]
fn layered_down_puts_sources_above_targets_without_overlap() {
    let mut store = MemoryStore::new();
    let r = create(
        &mut store,
        &small(),
        &CreateOpts { origin: drafftink_geom::kurbo::Point::new(100.0, 200.0), frame: None },
    )
    .unwrap();
    let y = |n: &str| store.get(&r.nodes[n]).unwrap().transform.y;
    assert!(y("start") < y("check"));
    assert!(y("check") < y("ok"));
    assert_eq!(y("start"), 200.0);
    let rects: Vec<_> = r.nodes.values().map(|id| store.get(id).unwrap().rect()).collect();
    assert!(!drafftink_edit::layout::rects_overlap(&rects, 1e-6));
    assert_eq!(r.bounds[0], 100.0);
}

#[test]
fn directions_flip_the_flow() {
    for (dir, axis_vertical, forward) in [
        (Direction::Down, true, true),
        (Direction::Up, true, false),
        (Direction::Right, false, true),
        (Direction::Left, false, false),
    ] {
        let mut s =
            spec(r#"{"nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b"}]}"#);
        s.layout.direction = dir;
        let mut store = MemoryStore::new();
        let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
        let (a, b) = (store.get(&r.nodes["a"]).unwrap().transform, store.get(&r.nodes["b"]).unwrap().transform);
        let (pa, pb) = if axis_vertical { (a.y, b.y) } else { (a.x, b.x) };
        assert_eq!(pa < pb, forward, "{dir:?}");
    }
}

#[test]
fn direction_accepts_mermaid_spellings() {
    let l: LayoutOpts = serde_json::from_str(r#"{"direction":"LR"}"#).unwrap();
    assert_eq!(l.direction, Direction::Right);
    let l: LayoutOpts = serde_json::from_str(r#"{"direction":"TD","algorithm":"flowchart"}"#).unwrap();
    assert_eq!((l.direction, l.algorithm), (Direction::Down, Algorithm::Layered));
}

#[test]
fn unknown_symbolic_id_names_it_and_lists_the_known_ones() {
    let s = spec(r#"{"nodes":[{"id":"a"},{"id":"b"}],"edges":[{"from":"a","to":"b"},{"from":"a","to":"zzz"}]}"#);
    let mut store = MemoryStore::new();
    let err = create(&mut store, &s, &CreateOpts::default()).unwrap_err();
    assert_eq!(err, DiagramError::UnknownNode { index: 1, id: "zzz".into(), known: "a, b".into() });
    assert_eq!(err.to_string(), "Edge 1 refers to node `zzz`, which is not in `nodes`. Known nodes: a, b.");
    assert_eq!(store.len(), 0, "nothing is created on error");
}

#[test]
fn duplicate_ids_and_empty_specs_are_refused() {
    let mut store = MemoryStore::new();
    let dup = spec(r#"{"nodes":[{"id":"a"},{"id":"b"},{"id":"a"}]}"#);
    let err = create(&mut store, &dup, &CreateOpts::default()).unwrap_err();
    assert_eq!(err, DiagramError::DuplicateNode("a".into()));
    assert_eq!(err.to_string(), "Node id `a` appears twice. Node ids must be unique.");
    let none = spec(r#"{"nodes":[]}"#);
    assert_eq!(create(&mut store, &none, &CreateOpts::default()).unwrap_err(), DiagramError::NoNodes);
    let empty_id = spec(r#"{"nodes":[{"id":""}]}"#);
    assert!(matches!(create(&mut store, &empty_id, &CreateOpts::default()), Err(DiagramError::BadNodeId(_))));
}

#[test]
fn unknown_fields_are_rejected_by_the_schema() {
    assert!(serde_json::from_str::<DiagramSpec>(r#"{"nodes":[{"id":"a","colour":"red"}]}"#).is_err());
    assert!(serde_json::from_str::<DiagramSpec>(r#"{"nodes":[{"id":"a"}],"bogus":1}"#).is_err());
}

#[test]
fn limits_are_enforced() {
    let nodes: Vec<NodeSpec> = (0..2001).map(|i| NodeSpec::new(format!("n{i}"), "")).collect();
    let s = DiagramSpec { nodes, ..DiagramSpec::default() };
    let mut store = MemoryStore::new();
    let err = create(&mut store, &s, &CreateOpts::default()).unwrap_err();
    assert!(matches!(err, DiagramError::LimitExceeded { .. }), "{err:?}");
}

#[test]
fn self_loops_are_allowed_and_bound_at_both_ends() {
    let s = spec(
        r#"{"nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b"},{"from":"b","to":"b","label":"retry"}]}"#,
    );
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let loop_arrow = store.get(&r.edges[1]).unwrap();
    let [x, y] = loop_arrow.binding_targets();
    assert_eq!(x, Some(&r.nodes["b"]));
    assert_eq!(y, Some(&r.nodes["b"]));
    assert!(loop_arrow.linear_data().unwrap().points.len() >= 4);
    assert!(!validate_scene(&store).has_errors());
}

#[test]
fn tree_layout_needs_a_forest_and_names_the_node() {
    let mut s = spec(
        r#"{"nodes":[{"id":"r"},{"id":"a"},{"id":"b"},{"id":"c"}],"edges":[{"from":"r","to":"a"},{"from":"r","to":"b"},{"from":"a","to":"c"},{"from":"b","to":"c"}]}"#,
    );
    s.layout.algorithm = Algorithm::Tree;
    let mut store = MemoryStore::new();
    let err = create(&mut store, &s, &CreateOpts::default()).unwrap_err();
    assert_eq!(err, DiagramError::TreeNotForest("c".into()));
    // A cycle is not a forest either.
    let mut cyc = spec(r#"{"nodes":[{"id":"a"},{"id":"b"}],"edges":[{"from":"a","to":"b"},{"from":"b","to":"a"}]}"#);
    cyc.layout.algorithm = Algorithm::Tree;
    assert!(matches!(create(&mut store, &cyc, &CreateOpts::default()), Err(DiagramError::TreeNotForest(_))));
}

#[test]
fn tree_centres_parents_over_children() {
    let mut s = spec(
        r#"{"nodes":[{"id":"r","label":"root"},{"id":"a","label":"a"},{"id":"b","label":"b"}],"edges":[{"from":"r","to":"a"},{"from":"r","to":"b"}]}"#,
    );
    s.layout.algorithm = Algorithm::Tree;
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let c = |n: &str| store.get(&r.nodes[n]).unwrap().rect().center();
    assert!((c("r").x - (c("a").x + c("b").x) / 2.0).abs() < 1e-9);
    assert!(c("r").y < c("a").y);
    assert_eq!(c("a").y, c("b").y);
}

#[test]
fn grid_uses_columns() {
    let nodes: Vec<NodeSpec> = (0..7).map(|i| NodeSpec::new(format!("n{i}"), format!("N{i}"))).collect();
    let s = DiagramSpec {
        nodes,
        layout: LayoutOpts { algorithm: Algorithm::Grid, columns: Some(3), ..LayoutOpts::default() },
        ..DiagramSpec::default()
    };
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let ys: Vec<f64> = (0..7).map(|i| store.get(&r.nodes[&format!("n{i}")]).unwrap().transform.y).collect();
    assert_eq!(ys[0], ys[2]);
    assert!(ys[3] > ys[0]);
    assert_eq!(ys[3], ys[5]);
    assert!(ys[6] > ys[3]);
}

#[test]
fn none_layout_keeps_given_positions_and_asks_for_missing_ones() {
    let s = spec(
        r#"{"nodes":[{"id":"a","x":10,"y":20,"width":50,"height":40},{"id":"b","x":300,"y":20}],"layout":{"algorithm":"none"}}"#,
    );
    let mut store = MemoryStore::new();
    let r =
        create(&mut store, &s, &CreateOpts { origin: drafftink_geom::kurbo::Point::new(999.0, 999.0), frame: None })
            .unwrap();
    let a = store.get(&r.nodes["a"]).unwrap().transform;
    assert_eq!((a.x, a.y, a.w, a.h), (10.0, 20.0, 50.0, 40.0));
    let missing = spec(r#"{"nodes":[{"id":"a","x":1,"y":1},{"id":"b"}],"layout":{"algorithm":"none"}}"#);
    let err = create(&mut store, &missing, &CreateOpts::default()).unwrap_err();
    assert_eq!(err, DiagramError::MissingPosition("b".into()));
}

#[test]
fn explicit_sizes_are_respected_and_labels_make_nodes_big_enough() {
    let s = spec(
        r#"{"nodes":[
            {"id":"a","label":"x","width":200,"height":100},
            {"id":"b","label":"a very long label that needs several lines to fit into the shape","width":90,"height":20},
            {"id":"c","label":"s","size":[120,70]}]}"#,
    );
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let a = store.get(&r.nodes["a"]).unwrap().transform;
    assert_eq!((a.w, a.h), (200.0, 100.0));
    let c = store.get(&r.nodes["c"]).unwrap().transform;
    assert_eq!((c.w, c.h), (120.0, 70.0));
    let b = store.get(&r.nodes["b"]).unwrap();
    assert_eq!(b.transform.w, 90.0);
    assert!(b.transform.h > 20.0, "the node grew to hold its label: {}", b.transform.h);
    let label = store.iter().find(|e| e.container_id() == Some(&b.id)).unwrap();
    assert!(label.rect().y1 <= b.rect().y1);
    // Explicit width and height are not enlarged afterwards by label creation.
    assert!(!validate_scene(&store).has_errors());
}

#[test]
fn node_size_has_a_minimum_and_follows_the_label() {
    let s =
        spec(r#"{"nodes":[{"id":"a","label":"x"},{"id":"b","label":"a rather long label for a node"},{"id":"c"}]}"#);
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let t = |n: &str| store.get(&r.nodes[n]).unwrap().transform;
    assert_eq!((t("a").w, t("a").h), (80.0, 48.0));
    assert_eq!((t("c").w, t("c").h), (80.0, 48.0));
    assert!(t("b").w > 80.0);
}

#[test]
fn styles_names_shapes_and_arrow_options_are_applied() {
    let s = spec(
        r##"{"nodes":[
            {"id":"a","label":"A","name":"first","style":{"fill_color":"#ffcc00","stroke_width":4,"corner_radius":8}},
            {"id":"b","label":"B","shape":"text"}],
          "edges":[{"from":"a","to":"b","arrow_type":"elbow","arrowheads":"both","name":"link"},
                   {"from":"b","to":"a","arrowheads":"none"}]}"##,
    );
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let a = store.get(&r.nodes["a"]).unwrap();
    assert_eq!(a.name.as_deref(), Some("first"));
    assert_eq!(a.style.stroke_width, 4.0);
    assert_eq!(a.style.corner_radius, 8.0);
    assert!(a.style.fill_color.is_some());
    assert_eq!(store.get(&r.nodes["b"]).unwrap().tag(), Kind::Text);
    let e1 = store.get(&r.edges[0]).unwrap();
    let d = e1.linear_data().unwrap();
    assert_eq!(d.path, PathType::Elbow);
    assert_eq!((d.start_head, d.end_head), (Head::Arrow, Head::Arrow));
    assert_eq!(e1.name.as_deref(), Some("link"));
    let d2 = store.get(&r.edges[1]).unwrap().linear_data().unwrap().clone();
    assert_eq!((d2.start_head, d2.end_head), (Head::None, Head::None));
    assert!(!validate_scene(&store).has_errors());
}

#[test]
fn names_must_be_unique_and_valid() {
    let mut store = MemoryStore::new();
    let s = spec(r#"{"nodes":[{"id":"a","name":"x"},{"id":"b","name":"x"}]}"#);
    assert!(matches!(create(&mut store, &s, &CreateOpts::default()), Err(DiagramError::NameTaken { .. })));
    let s = spec(r#"{"nodes":[{"id":"a","name":"@x"}]}"#);
    assert!(matches!(create(&mut store, &s, &CreateOpts::default()), Err(DiagramError::BadName(_))));
    // A name already in the scene clashes too.
    let ok = spec(r#"{"nodes":[{"id":"a","name":"keep"}]}"#);
    create(&mut store, &ok, &CreateOpts::default()).unwrap();
    assert!(matches!(create(&mut store, &ok, &CreateOpts::default()), Err(DiagramError::NameTaken { .. })));
}

#[test]
fn groups_nest_and_arrows_between_members_join_them() {
    let s = spec(
        r#"{"nodes":[{"id":"a"},{"id":"b"},{"id":"c"},{"id":"d"}],
            "edges":[{"from":"a","to":"b"},{"from":"b","to":"c"},{"from":"c","to":"d"}],
            "groups":[{"id":"outer","members":["a","b","c"]},{"id":"inner","members":["a","b"]}]}"#,
    );
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let (outer, inner) = (&r.groups["outer"], &r.groups["inner"]);
    let a = store.get(&r.nodes["a"]).unwrap();
    assert_eq!(a.group_ids.as_slice(), &[inner.clone(), outer.clone()]);
    let c = store.get(&r.nodes["c"]).unwrap();
    assert_eq!(c.group_ids.as_slice(), std::slice::from_ref(outer));
    assert!(store.get(&r.nodes["d"]).unwrap().group_ids.is_empty());
    assert_eq!(store.get(&r.edges[0]).unwrap().group_ids.as_slice(), &[inner.clone(), outer.clone()]);
    assert_eq!(store.get(&r.edges[1]).unwrap().group_ids.as_slice(), std::slice::from_ref(outer));
    assert!(store.get(&r.edges[2]).unwrap().group_ids.is_empty());
    let report = validate_scene(&store);
    assert!(!report.has_errors(), "{report:?}");
}

// ---- clusters: titled boxes around nodes (Mermaid subgraphs) ----

/// The old app's `NESTED` test diagram as a spec: `Prep line` holds two nodes, `Real kitchen` and `Takeout kitchen` two
/// each, and the edges join the boxes and the node `swap`.
fn nested_clusters() -> DiagramSpec {
    spec(
        r#"{
        "nodes": [
            {"id": "a", "label": "mixer"}, {"id": "b", "label": "shaper"}, {"id": "swap", "label": "counter tickets"},
            {"id": "r1", "label": "oven"}, {"id": "r2", "label": "toppings"},
            {"id": "m1", "label": "prep bot"}, {"id": "m2", "label": "sim oven"}
        ],
        "edges": [
            {"from": "a", "to": "b"}, {"from": "stack", "to": "swap"},
            {"from": "swap", "to": "real"}, {"from": "swap", "to": "sim"},
            {"from": "r1", "to": "r2"}, {"from": "m1", "to": "m2"}
        ],
        "clusters": [
            {"id": "stack", "title": "Prep line", "members": ["a", "b"], "direction": "down"},
            {"id": "real", "title": "Real kitchen", "members": ["r1", "r2"]},
            {"id": "sim", "title": "Takeout kitchen", "members": ["m1", "m2"]}
        ]
    }"#,
    )
}

#[test]
fn children_of_a_cluster_sit_inside_the_box_below_the_title_band() {
    // The old test `nested_layout_places_cluster_children_inside_the_cluster_box`, with the stronger statement that
    // the whole child (not only its centre) is inside, and that the boxes do not overlap each other.
    let mut store = MemoryStore::new();
    let r = create(&mut store, &nested_clusters(), &CreateOpts::default()).unwrap();
    assert_eq!(r.clusters.len(), 3);
    let rect = |id: &drafftink_scene::ElementId| store.get(id).unwrap().rect();
    for (cluster, members) in [("stack", ["a", "b"]), ("real", ["r1", "r2"]), ("sim", ["m1", "m2"])] {
        let boxr = rect(&r.clusters[cluster]);
        for m in members {
            let child = rect(&r.nodes[m]);
            assert!(boxr.contains_rect(child), "{m} {child:?} escaped the box of {cluster} {boxr:?}");
            assert!(child.y0 >= boxr.y0 + 32.0, "{m} overlaps the title band of {cluster}");
        }
    }
    let boxes: Vec<_> = r.clusters.values().map(rect).collect();
    assert!(!drafftink_edit::layout::rects_overlap(&boxes, 0.5), "sibling boxes overlap: {boxes:?}");
    let swap = rect(&r.nodes["swap"]);
    assert!(boxes.iter().all(|b| b.intersect(swap).area() == 0.0), "a node outside lies on a box");
    let report = validate_scene(&store);
    assert!(!report.has_errors(), "{report:?}");
}

#[test]
fn a_cluster_is_a_filled_rectangle_with_its_title_text_and_lies_behind_its_members() {
    let mut store = MemoryStore::new();
    let r = create(&mut store, &nested_clusters(), &CreateOpts::default()).unwrap();
    let stack = store.get(&r.clusters["stack"]).unwrap();
    assert_eq!(stack.tag(), Kind::Rectangle);
    assert!(stack.style.fill_color.is_some());
    let titles: Vec<String> = r
        .created
        .iter()
        .filter_map(|id| match &store.get(id).unwrap().kind {
            ElementKind::Text(t) if store.get(id).unwrap().container_id().is_none() => Some(t.content.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(titles, ["Prep line", "Real kitchen", "Takeout kitchen"]);
    let title_id = r.created.iter().find(|id| matches!(&store.get(id).unwrap().kind, ElementKind::Text(t) if t.content == "Prep line")).unwrap();
    let title = store.get(title_id).unwrap().rect();
    let boxr = stack.rect();
    assert!(boxr.contains_rect(title), "the title is inside its box");
    assert!(title.y1 <= boxr.y0 + 32.0 + 0.5, "the title is in the band at the top");
    assert!((title.center().x - boxr.center().x).abs() < 0.5, "the title is centred");
    let z = |id: &drafftink_scene::ElementId| store.get(id).unwrap().index.clone();
    for m in ["a", "b"] {
        assert!(z(&r.clusters["stack"]) < z(&r.nodes[m]), "the box lies behind {m}");
    }
}

#[test]
fn clusters_nest_and_the_inner_box_is_inside_the_outer_one() {
    let s = spec(
        r#"{"nodes":[{"id":"a"},{"id":"b"},{"id":"c"},{"id":"d"}],
            "edges":[{"from":"a","to":"b"},{"from":"b","to":"c"},{"from":"c","to":"d"}],
            "clusters":[
                {"id":"outer","title":"Outer title that is rather long","members":["a"],"group":true},
                {"id":"inner","title":"Inner","parent":"outer","members":["b","c"],"group":true}]}"#,
    );
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let rect = |id: &drafftink_scene::ElementId| store.get(id).unwrap().rect();
    let (outer, inner) = (rect(&r.clusters["outer"]), rect(&r.clusters["inner"]));
    assert!(outer.contains_rect(inner), "{inner:?} is not inside {outer:?}");
    assert!(outer.contains_rect(rect(&r.nodes["a"])));
    assert!(inner.contains_rect(rect(&r.nodes["b"])) && inner.contains_rect(rect(&r.nodes["c"])));
    assert!(outer.intersect(rect(&r.nodes["d"])).area() == 0.0, "d is outside both boxes");
    // The outer box is wider than its long title needs.
    let title = r.created.iter().map(|id| store.get(id).unwrap()).find(|e| matches!(&e.kind, ElementKind::Text(t) if t.content.starts_with("Outer"))).unwrap();
    assert!(outer.width() >= title.rect().width() + 44.0 - 0.5);
    // z-order: outer box, inner box, then nodes.
    assert!(store.get(&r.clusters["outer"]).unwrap().index < store.get(&r.clusters["inner"]).unwrap().index);
    // Groups: the box, its title and its nodes share the cluster group; the groups nest innermost first.
    let (go, gi) = (&r.groups["outer"], &r.groups["inner"]);
    assert_eq!(store.get(&r.nodes["b"]).unwrap().group_ids.as_slice(), &[gi.clone(), go.clone()]);
    assert_eq!(store.get(&r.nodes["a"]).unwrap().group_ids.as_slice(), std::slice::from_ref(go));
    assert_eq!(store.get(&r.clusters["inner"]).unwrap().group_ids.as_slice(), &[gi.clone(), go.clone()]);
    assert_eq!(store.get(&r.clusters["outer"]).unwrap().group_ids.as_slice(), std::slice::from_ref(go));
    assert!(store.get(&r.nodes["d"]).unwrap().group_ids.is_empty());
    let report = validate_scene(&store);
    assert!(!report.has_errors(), "{report:?}");
}

#[test]
fn an_edge_may_end_at_a_cluster_and_is_bound_to_its_box() {
    let mut store = MemoryStore::new();
    let r = create(&mut store, &nested_clusters(), &CreateOpts::default()).unwrap();
    // `stack` -> `swap` starts at the box.
    let arrow = store.get(&r.edges[1]).unwrap();
    let [start, end] = arrow.binding_targets();
    assert_eq!(start, Some(&r.clusters["stack"]));
    assert_eq!(end, Some(&r.nodes["swap"]));
    // Every arrow is bound at both ends.
    for e in &r.edges {
        assert!(store.get(e).unwrap().binding_targets().iter().all(Option::is_some), "{e}");
    }
}

#[test]
fn flow_direction_of_a_cluster_applies_inside_it() {
    let s = spec(
        r#"{"nodes":[{"id":"a"},{"id":"b"},{"id":"c"},{"id":"d"}],
            "edges":[{"from":"a","to":"b"},{"from":"c","to":"d"}],
            "layout":{"direction":"down"},
            "clusters":[{"id":"h","title":"Across","members":["a","b"],"direction":"right"}]}"#,
    );
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let rect = |n: &str| store.get(&r.nodes[n]).unwrap().rect();
    assert!(rect("a").x1 <= rect("b").x0, "inside the cluster, b follows a to the right");
    assert!(rect("c").y1 <= rect("d").y0, "outside, d follows c downwards");
}

#[test]
fn an_empty_cluster_is_still_a_box_and_a_cluster_needs_an_automatic_layout() {
    let s = spec(r#"{"nodes":[{"id":"a"}],"clusters":[{"id":"e","title":"Nothing yet"}]}"#);
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    assert!(store.get(&r.clusters["e"]).unwrap().rect().width() >= 120.0);
    let fixed = spec(
        r#"{"nodes":[{"id":"a","x":0,"y":0}],"layout":{"algorithm":"none"},"clusters":[{"id":"c","members":["a"]}]}"#,
    );
    assert!(matches!(create(&mut MemoryStore::new(), &fixed, &CreateOpts::default()), Err(DiagramError::BadCluster { .. })));
}

#[test]
fn bad_clusters_are_refused_with_their_id() {
    let cases = [
        r#"{"nodes":[{"id":"a"}],"clusters":[{"id":"a"}]}"#,
        r#"{"nodes":[{"id":"a"}],"clusters":[{"id":"c"},{"id":"c"}]}"#,
        r#"{"nodes":[{"id":"a"}],"clusters":[{"id":"c","parent":"nope"}]}"#,
        r#"{"nodes":[{"id":"a"}],"clusters":[{"id":"c","parent":"c"}]}"#,
        r#"{"nodes":[{"id":"a"}],"clusters":[{"id":"c","parent":"d"},{"id":"d","parent":"c"}]}"#,
        r#"{"nodes":[{"id":"a"}],"clusters":[{"id":"c","members":["a"]},{"id":"d","members":["a"]}]}"#,
    ];
    for json in cases {
        let e = create(&mut MemoryStore::new(), &spec(json), &CreateOpts::default()).unwrap_err();
        assert!(matches!(e, DiagramError::BadCluster { .. } | DiagramError::DuplicateNode(_)), "{json}: {e}");
    }
    let unknown = spec(r#"{"nodes":[{"id":"a"}],"clusters":[{"id":"c","members":["zz"]}]}"#);
    assert!(matches!(
        create(&mut MemoryStore::new(), &unknown, &CreateOpts::default()),
        Err(DiagramError::UnknownGroupMember { .. })
    ));
}

#[test]
fn bad_groups_are_refused() {
    let mut store = MemoryStore::new();
    let overlap =
        spec(r#"{"nodes":[{"id":"a"},{"id":"b"},{"id":"c"}],"groups":[{"members":["a","b"]},{"members":["b","c"]}]}"#);
    assert!(matches!(
        create(&mut store, &overlap, &CreateOpts::default()),
        Err(DiagramError::GroupsOverlap { a: 0, b: 1 })
    ));
    let single = spec(r#"{"nodes":[{"id":"a"},{"id":"b"}],"groups":[{"members":["a"]}]}"#);
    assert!(matches!(create(&mut store, &single, &CreateOpts::default()), Err(DiagramError::BadGroup { .. })));
    let unknown = spec(r#"{"nodes":[{"id":"a"},{"id":"b"}],"groups":[{"members":["a","q"]}]}"#);
    assert!(matches!(
        create(&mut store, &unknown, &CreateOpts::default()),
        Err(DiagramError::UnknownGroupMember { .. })
    ));
}

#[test]
fn frame_wraps_everything_and_children_sit_below_it() {
    let mut store = MemoryStore::new();
    let r = create(
        &mut store,
        &small(),
        &CreateOpts { origin: drafftink_geom::kurbo::Point::ZERO, frame: Some("Flow".into()) },
    )
    .unwrap();
    let fid = r.frame.clone().unwrap();
    let f = store.get(&fid).unwrap();
    let ElementKind::Frame(d) = &f.kind else { panic!() };
    assert_eq!(d.name.as_deref(), Some("Flow"));
    for id in r.nodes.values().chain(&r.edges) {
        let e = store.get(id).unwrap();
        assert_eq!(e.frame_id.as_ref(), Some(&fid));
        assert!(f.rect().contains_rect(e.rect()));
    }
    let report = validate_scene(&store);
    assert!(!report.has_errors(), "{report:?}");
}

#[test]
fn elbow_arrows_are_orthogonal() {
    let mut s = small();
    for e in &mut s.edges {
        e.arrow_type = Some(PathType::Elbow);
    }
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    for id in &r.edges {
        let e = store.get(id).unwrap();
        let pts = &e.linear_data().unwrap().points;
        for w in pts.windows(2) {
            let (dx, dy) = ((w[1].x - w[0].x).abs(), (w[1].y - w[0].y).abs());
            assert!(dx < 1e-9 || dy < 1e-9, "segment {w:?} is not axis aligned in {pts:?}");
        }
    }
}

#[test]
fn long_edges_avoid_the_nodes_in_between() {
    // a -> b -> c -> d, plus a -> d which skips two layers.
    let s = spec(
        r#"{"nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"},{"id":"c","label":"C"},{"id":"d","label":"D"}],
            "edges":[{"from":"a","to":"b"},{"from":"b","to":"c"},{"from":"c","to":"d"},{"from":"a","to":"d"}]}"#,
    );
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let long = store.get(&r.edges[3]).unwrap();
    let world: Vec<drafftink_geom::kurbo::Point> =
        long.linear_data().unwrap().points.iter().map(|p| long.transform.local_to_world(*p)).collect();
    for id in [&r.nodes["b"], &r.nodes["c"]] {
        let rect = store.get(id).unwrap().rect();
        for w in world.windows(2) {
            assert!(!drafftink_geom::aabb::segment_intersects_rect(w[0], w[1], &rect), "long edge crosses {id}");
        }
    }
}

#[test]
fn parallel_and_opposite_edges_do_not_overlap_exactly() {
    let s = spec(
        r#"{"nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b"},{"from":"b","to":"a"},{"from":"a","to":"b"}]}"#,
    );
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let xs: Vec<f64> = r.edges.iter().map(|id| store.get(id).unwrap().transform.x).collect();
    assert!(xs[0] != xs[1] || xs[1] != xs[2], "{xs:?}");
    let mut seen = BTreeMap::new();
    for id in &r.edges {
        let e = store.get(id).unwrap();
        let key = format!("{:.3}", e.transform.x);
        *seen.entry(key).or_insert(0) += 1;
    }
    assert!(seen.values().all(|&n| n <= 2), "{seen:?}");
}

#[test]
fn shapes_cover_all_node_kinds() {
    let nodes = vec![
        NodeSpec { shape: NodeShape::Rectangle, ..NodeSpec::new("r", "R") },
        NodeSpec { shape: NodeShape::Ellipse, ..NodeSpec::new("e", "E") },
        NodeSpec { shape: NodeShape::Diamond, ..NodeSpec::new("d", "D") },
        NodeSpec { shape: NodeShape::Text, ..NodeSpec::new("t", "T") },
    ];
    let edges =
        vec![EdgeSpec { arrowheads: Some(Arrowheads::Both), ..EdgeSpec::new("r", "e") }, EdgeSpec::new("d", "t")];
    let s = DiagramSpec { nodes, edges, ..DiagramSpec::default() };
    let mut store = MemoryStore::new();
    let r = create(&mut store, &s, &CreateOpts::default()).unwrap();
    let kinds: Vec<Kind> = ["r", "e", "d", "t"].iter().map(|n| store.get(&r.nodes[*n]).unwrap().tag()).collect();
    assert_eq!(kinds, vec![Kind::Rectangle, Kind::Ellipse, Kind::Diamond, Kind::Text]);
    assert!(!validate_scene(&store).has_errors());
    let empty_text = DiagramSpec {
        nodes: vec![NodeSpec { label: None, shape: NodeShape::Text, ..NodeSpec::new("t", "") }],
        ..DiagramSpec::default()
    };
    assert!(matches!(create(&mut store, &empty_text, &CreateOpts::default()), Err(DiagramError::EmptyTextNode(_))));
}

#[test]
fn json_schema_is_available() {
    let schema = schemars::schema_for!(DiagramSpec);
    let text = serde_json::to_string(&schema).unwrap();
    assert!(text.contains("\"nodes\""));
    assert!(text.contains("\"layout\""));
}

mod hostile {
    use super::*;
    use drafftink_edit::layout::{Direction, EdgeSpec};
    use proptest::prelude::*;

    fn weird_f64() -> impl Strategy<Value = Option<f64>> {
        prop_oneof![
            40 => Just(None),
            20 => (1.0f64..300.0).prop_map(Some),
            1 => Just(Some(f64::NAN)),
            1 => Just(Some(f64::INFINITY)),
            1 => Just(Some(-5.0)),
            1 => Just(Some(0.0)),
            1 => Just(Some(1e300)),
            1 => Just(Some(1e-300)),
        ]
    }

    fn weird_gap() -> impl Strategy<Value = f64> {
        prop_oneof![30 => 0.0f64..100.0, 1 => Just(f64::NAN), 1 => Just(-1.0), 1 => Just(1e300), 1 => Just(f64::INFINITY)]
    }

    fn node() -> impl Strategy<Value = NodeSpec> {
        (
            "[a-d]{0,2}",
            prop::option::of(prop_oneof!["", "[ -~]{0,30}", "\\PC{0,40}", "(word ){0,60}"]),
            prop_oneof![
                Just(NodeShape::Rectangle),
                Just(NodeShape::Ellipse),
                Just(NodeShape::Diamond),
                Just(NodeShape::Text)
            ],
            weird_f64(),
            weird_f64(),
            prop_oneof![30 => Just(None), 1 => prop::option::of("[a-z@$]{0,4}")],
        )
            .prop_map(|(id, label, shape, width, height, name)| NodeSpec {
                id,
                label,
                shape,
                width,
                height,
                name,
                ..NodeSpec::new("x", "")
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        #[test]
        fn arbitrary_specs_never_panic_and_valid_results_validate(
            nodes in prop::collection::vec(node(), 1..7),
            edges in prop::collection::vec((0usize..300, 0usize..300), 0..9),
            algo in 0u8..5, dir in 0u8..4, ng in weird_gap(), lg in weird_gap(),
            cols in prop::option::of(0u32..9),
            ox in prop_oneof![20 => -50.0f64..50.0, 1 => Just(f64::NAN), 1 => Just(1e12)],
            frame in any::<bool>(),
        ) {
            let algorithm = [Algorithm::Layered, Algorithm::Tree, Algorithm::Grid, Algorithm::Force, Algorithm::None][usize::from(algo)];
            let direction = [Direction::Down, Direction::Up, Direction::Left, Direction::Right][usize::from(dir)];
            // Unique ids most of the time; edge ends beyond the node count are unknown ids.
            let count = nodes.len();
            let nodes: Vec<NodeSpec> = nodes.into_iter().enumerate().map(|(i, n)| NodeSpec { id: format!("n{i}"), ..n }).collect();
            let spec = DiagramSpec {
                nodes,
                edges: edges
                    .into_iter()
                    .map(|(a, b)| {
                        let end = |v: usize| if v >= 290 { "unknown".to_string() } else { format!("n{}", v % count.max(1)) };
                        EdgeSpec::new(end(a), end(b))
                    })
                    .collect(),
                groups: Vec::new(),
                clusters: Vec::new(),
                layout: LayoutOpts { algorithm, direction, node_gap: ng, layer_gap: lg, columns: cols },
            };
            let mut store = MemoryStore::new();
            let mut ids = SeqIds::new();
            let mut tx = store.begin();
            let opts = CreateOpts { origin: drafftink_geom::kurbo::Point::new(ox, 0.0), frame: frame.then(|| "F".to_string()) };
            let result = diagram_create(&mut tx, &MonospaceMeasure::default(), &mut ids, &spec, &opts);
            if result.is_ok() {
                tx.commit().unwrap();
                let report = validate_scene(&store);
                prop_assert!(!report.has_errors(), "{report:?}");
            }
            // Dropping `tx` on error leaves the store as it was.
            else {
                drop(tx);
                prop_assert_eq!(store.len_all(), 0);
            }
        }
    }
}

mod cluster_property {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// Any tree of clusters: every node is inside the box of each cluster around it, a box is inside the box
        /// around it, boxes with the same parent do not overlap, and a node outside a box does not lie on it.
        #[test]
        fn boxes_hold_exactly_their_members(
            parents in prop::collection::vec(prop::option::of(0usize..6), 1..6),
            homes in prop::collection::vec(prop::option::of(0usize..6), 1..12),
            edges in prop::collection::vec((0usize..30, 0usize..30), 0..14),
            titles in prop::collection::vec("[a-z ]{0,30}", 6),
            algo in 0u8..4, dir in 0u8..4,
        ) {
            let k = parents.len();
            // Parent of cluster c is a smaller index, so the clusters form a forest.
            let parent: Vec<Option<usize>> = parents.iter().enumerate().map(|(c, p)| p.filter(|_| c > 0).map(|p| p % c)).collect();
            let n = homes.len();
            let home: Vec<Option<usize>> = homes.iter().map(|h| h.map(|h| h % k)).collect();
            let algorithm = [Algorithm::Layered, Algorithm::Grid, Algorithm::Force, Algorithm::Layered][usize::from(algo)];
            let direction = [Direction::Down, Direction::Up, Direction::Left, Direction::Right][usize::from(dir)];
            let item = |v: usize| if v % 2 == 0 { format!("n{}", v / 2 % n) } else { format!("c{}", v / 2 % k) };
            let spec = DiagramSpec {
                nodes: (0..n).map(|i| NodeSpec::new(format!("n{i}"), format!("node {i}"))).collect(),
                edges: edges.into_iter().map(|(a, b)| EdgeSpec::new(item(a), item(b))).collect(),
                groups: Vec::new(),
                clusters: (0..k).map(|c| ClusterSpec {
                    id: format!("c{c}"),
                    title: titles[c].clone(),
                    parent: parent[c].map(|p| format!("c{p}")),
                    members: (0..n).filter(|&i| home[i] == Some(c)).map(|i| format!("n{i}")).collect(),
                    direction: None,
                    group: c % 2 == 0,
                }).collect(),
                layout: LayoutOpts { algorithm, direction, ..LayoutOpts::default() },
            };
            let mut store = MemoryStore::new();
            let r = create(&mut store, &spec, &CreateOpts::default()).unwrap();
            let rect = |id: &drafftink_scene::ElementId| store.get(id).unwrap().rect();
            let chain = |start: Option<usize>| std::iter::successors(start, |&c| parent[c]).collect::<Vec<_>>();
            for i in 0..n {
                let node = rect(&r.nodes[&format!("n{i}")]);
                let around = chain(home[i]);
                for c in 0..k {
                    let boxr = rect(&r.clusters[&format!("c{c}")]);
                    if around.contains(&c) {
                        prop_assert!(boxr.contains_rect(node), "n{i} escaped c{c}");
                    } else {
                        prop_assert!(boxr.intersect(node).area() < 0.5, "n{i} lies on c{c}");
                    }
                }
            }
            for c in 0..k {
                let boxr = rect(&r.clusters[&format!("c{c}")]);
                if let Some(p) = parent[c] {
                    prop_assert!(rect(&r.clusters[&format!("c{p}")]).contains_rect(boxr), "c{c} escaped c{p}");
                }
                for d in c + 1..k {
                    if parent[c] == parent[d] {
                        prop_assert!(boxr.intersect(rect(&r.clusters[&format!("c{d}")])).area() < 0.5, "c{c} and c{d} overlap");
                    }
                }
            }
            let report = validate_scene(&store);
            prop_assert!(!report.has_errors(), "{report:?}");
        }
    }
}

#[test]
fn hostile_runs_reach_the_success_path() {
    // The property test above would prove little if every random spec were refused.
    let spec = spec(r#"{"nodes":[{"id":"n0","label":"x"},{"id":"n1"}],"edges":[{"from":"n0","to":"n1"}]}"#);
    let mut store = MemoryStore::new();
    assert!(create(&mut store, &spec, &CreateOpts::default()).is_ok());
}
