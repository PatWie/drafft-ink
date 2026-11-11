//! `doc.outline`: structure, order, scope, truncation, and the text form.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use drafftink_edit::layout::{CreateOpts, DiagramResult, DiagramSpec, diagram_create};
use drafftink_edit::query::{Outline, OutlineOpts, OutlineScope, outline};
use drafftink_edit::text_bind::MonospaceMeasure;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_scene::{
    Element, ElementId, ElementKind, Kind, LinearKind, MemoryStore, SceneRead, SceneStore, SceneTx, SeqIds, TextData,
    ZIndex,
};

fn build(spec_json: &str, opts: &CreateOpts) -> (MemoryStore, DiagramResult) {
    let spec: DiagramSpec = serde_json::from_str(spec_json).unwrap();
    let mut store = MemoryStore::new();
    let mut ids = SeqIds::new();
    let mut tx = store.begin();
    let r = diagram_create(&mut tx, &MonospaceMeasure::default(), &mut ids, &spec, opts).unwrap();
    tx.commit().unwrap();
    (store, r)
}

const SMALL: &str = r#"{
    "nodes": [
        {"id": "start", "label": "Start", "shape": "ellipse", "name": "start"},
        {"id": "check", "label": "Is it valid?", "shape": "diamond"},
        {"id": "ok", "label": "Done"},
        {"id": "bad", "label": "Fix the input\nand retry"}
    ],
    "edges": [
        {"from": "start", "to": "check"},
        {"from": "check", "to": "ok", "label": "yes"},
        {"from": "check", "to": "bad", "label": "no", "arrow_type": "elbow"},
        {"from": "bad", "to": "check", "arrowheads": "both"}
    ],
    "groups": [{"members": ["ok", "bad"]}]
}"#;

fn add_loose(store: &mut MemoryStore) {
    let mut tx = store.begin();
    let z = tx.next_top_index().unwrap();
    let mut t =
        Element::new(ElementId::new("note").unwrap(), ElementKind::Text(TextData::new("remember: \"quotes\" work")), z)
            .with_bounds(400.0, 10.0, 200.0, 25.0);
    t.locked = true;
    tx.insert(t).unwrap();
    let z = tx.next_top_index().unwrap();
    let line = Element::linear(
        ElementId::new("rule").unwrap(),
        LinearKind::Line,
        z,
        &[Point::new(0.0, 500.0), Point::new(300.0, 500.0)],
    )
    .unwrap();
    tx.insert(line).unwrap();
    tx.commit().unwrap();
}

#[test]
fn snapshot_of_a_small_diagram() {
    let (mut store, _) = build(SMALL, &CreateOpts::default());
    add_loose(&mut store);
    let o = outline(&store, &OutlineOpts::default());
    insta::assert_snapshot!("small_text", o.to_text());
    insta::assert_snapshot!("small_json", serde_json::to_string_pretty(&o).unwrap());
}

#[test]
fn snapshot_with_a_frame() {
    let (store, _) = build(
        r#"{"nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b","label":"go"}]}"#,
        &CreateOpts { origin: Point::new(10.0, 20.0), frame: Some("Flow".into()) },
    );
    insta::assert_snapshot!("frame_text", outline(&store, &OutlineOpts::default()).to_text());
}

#[test]
fn nodes_edges_and_labels_are_structured() {
    let (store, r) = build(SMALL, &CreateOpts::default());
    let o = outline(&store, &OutlineOpts::default());
    assert_eq!(o.nodes.len(), 4);
    assert_eq!(o.edges.len(), 4);
    assert!(o.texts.is_empty() && o.frames.is_empty());
    let check = o.nodes.iter().find(|n| n.id == r.nodes["check"]).unwrap();
    assert_eq!(check.label.as_deref(), Some("Is it valid?"));
    assert_eq!(check.kind, Kind::Diamond);
    let named = o.nodes.iter().find(|n| n.name.as_deref() == Some("start")).unwrap();
    assert_eq!(named.id, r.nodes["start"]);
    let yes = o.edges.iter().find(|e| e.label.as_deref() == Some("yes")).unwrap();
    assert_eq!((&yes.from, &yes.to), (&Some(r.nodes["check"].clone()), &Some(r.nodes["ok"].clone())));
    // Bound labels are not nodes of their own.
    assert!(o.nodes.iter().all(|n| n.kind != Kind::Text));
    // One group with both members, in outline order.
    assert_eq!(o.groups.len(), 1);
    assert_eq!(o.groups[0].members.len(), 2);
}

#[test]
fn nodes_come_in_reading_order() {
    let (store, r) = build(SMALL, &CreateOpts::default());
    let o = outline(&store, &OutlineOpts::default());
    let order: Vec<&ElementId> = o.nodes.iter().map(|n| &n.id).collect();
    assert_eq!(order[0], &r.nodes["start"]);
    assert_eq!(order[1], &r.nodes["check"]);
    let ys: Vec<i64> = o.nodes.iter().map(|n| n.bounds[1] + n.bounds[3] / 2).collect();
    assert!(ys.windows(2).all(|w| w[0] <= w[1]), "{ys:?}");
}

#[test]
fn z_order_does_not_change_the_outline() {
    let (mut store, r) = build(SMALL, &CreateOpts::default());
    let before = outline(&store, &OutlineOpts::default());
    // Bring the first node (and nothing else) to the very front.
    let top = store.top_z().unwrap();
    let key = ZIndex::between(Some(&top), None).unwrap();
    let mut tx = store.begin();
    tx.update(&r.nodes["start"], |e| e.index = key.clone()).unwrap();
    tx.commit().unwrap();
    assert_eq!(outline(&store, &OutlineOpts::default()), before);
}

#[test]
fn loose_text_and_lines_are_listed_separately() {
    let (mut store, _) = build(SMALL, &CreateOpts::default());
    add_loose(&mut store);
    let o = outline(&store, &OutlineOpts::default());
    assert_eq!(o.texts.len(), 1);
    assert!(o.texts[0].locked);
    assert!(o.nodes.iter().any(|n| n.kind == Kind::Line));
    let text = o.to_text();
    assert!(text.contains(r#"note "remember: \"quotes\" work" text (400,10,200x25) locked"#), "{text}");
}

#[test]
fn text_form_matches_the_documented_shape() {
    let (store, r) = build(
        r#"{"nodes":[{"id":"a","label":"Start","shape":"rectangle"},{"id":"b","label":"End"}],"edges":[{"from":"a","to":"b","label":"yes"}]}"#,
        &CreateOpts::default(),
    );
    let t = outline(&store, &OutlineOpts::default()).to_text();
    let a = &r.nodes["a"];
    let b = &r.nodes["b"];
    assert!(t.contains(&format!("{a} \"Start\" rectangle (0,0,80x48)")), "{t}");
    assert!(t.contains(&format!("{a} -> {b} \"yes\" [{}]", r.edges[0])), "{t}");
    assert!(t.starts_with("nodes (2):\n"));
}

#[test]
fn loose_arrow_ends_show_as_question_marks() {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    let z = tx.next_top_index().unwrap();
    let mut a = Element::linear(
        ElementId::new("arr").unwrap(),
        LinearKind::Arrow,
        z,
        &[Point::new(0.0, 0.0), Point::new(50.0, 0.0)],
    )
    .unwrap();
    if let ElementKind::Arrow(d) = &mut a.kind {
        d.end_head = drafftink_scene::Head::Arrow;
    }
    tx.insert(a).unwrap();
    tx.commit().unwrap();
    let t = outline(&store, &OutlineOpts::default()).to_text();
    assert!(t.contains("? -> ? [arr]"), "{t}");
}

#[test]
fn empty_document() {
    let store = MemoryStore::new();
    let o = outline(&store, &OutlineOpts::default());
    assert!(o.is_empty());
    assert_eq!(o.to_text(), "empty document: no elements.\n");
}

#[test]
fn scope_selection_frame_and_region() {
    let (store, r) = build(SMALL, &CreateOpts { origin: Point::ZERO, frame: Some("F".into()) });
    let sel: BTreeSet<ElementId> = [r.nodes["start"].clone(), r.nodes["check"].clone()].into();
    let o = outline(
        &store,
        &OutlineOpts { scope: OutlineScope { only: Some(sel), ..OutlineScope::default() }, ..OutlineOpts::default() },
    );
    assert_eq!(o.nodes.len(), 2);
    // The arrow between the two selected nodes is shown, the others are not.
    assert_eq!(o.edges.len(), 1);
    assert_eq!(o.edges[0].id, r.edges[0]);

    let fid = r.frame.clone().unwrap();
    let o = outline(
        &store,
        &OutlineOpts {
            scope: OutlineScope { frame: Some(fid.clone()), ..OutlineScope::default() },
            ..OutlineOpts::default()
        },
    );
    assert_eq!(o.nodes.len(), 4);
    assert_eq!(o.frames.len(), 1);
    assert_eq!(o.frames[0].children.len(), 8);

    let first = store.get(&r.nodes["start"]).unwrap().rect();
    let o = outline(
        &store,
        &OutlineOpts {
            scope: OutlineScope { region: Some(first.inflate(1.0, 1.0)), ..OutlineScope::default() },
            ..OutlineOpts::default()
        },
    );
    assert!(o.nodes.iter().any(|n| n.id == r.nodes["start"]));
    assert!(o.nodes.len() < 4);
    let nothing = outline(
        &store,
        &OutlineOpts {
            scope: OutlineScope { region: Some(Rect::new(9e3, 9e3, 9.1e3, 9.1e3)), ..OutlineScope::default() },
            ..OutlineOpts::default()
        },
    );
    assert!(nothing.nodes.is_empty());
}

#[test]
fn truncation_says_so() {
    let nodes: String = (0..30).map(|i| format!(r#"{{"id":"n{i}","label":"N{i}"}}"#)).collect::<Vec<_>>().join(",");
    let edges: String =
        (1..30).map(|i| format!(r#"{{"from":"n{}","to":"n{i}"}}"#, i - 1)).collect::<Vec<_>>().join(",");
    let (store, _) = build(&format!(r#"{{"nodes":[{nodes}],"edges":[{edges}]}}"#), &CreateOpts::default());
    let o = outline(&store, &OutlineOpts { max_nodes: 10, ..OutlineOpts::default() });
    assert_eq!(o.nodes.len(), 10);
    assert_eq!(o.edges.len(), 10);
    let t = o.truncated.clone().unwrap();
    assert_eq!((t.nodes_total, t.nodes_shown, t.edges_total, t.edges_shown), (30, 10, 29, 10));
    let text = o.to_text();
    assert!(text.contains("... showing 10 of 30 nodes and 10 of 29 edges."), "{text}");
    assert!(text.contains("max_nodes=10"));
    let full = outline(&store, &OutlineOpts::default());
    assert!(full.truncated.is_none());
}

#[test]
fn long_labels_are_cut_in_the_text_form_only() {
    let long = "x".repeat(200);
    let (store, _) = build(&format!(r#"{{"nodes":[{{"id":"a","label":"{long}"}}]}}"#), &CreateOpts::default());
    let o = outline(&store, &OutlineOpts::default());
    assert_eq!(o.nodes[0].label.as_deref(), Some(long.as_str()));
    let text = o.to_text();
    assert!(text.contains(&format!("{}…", "x".repeat(80))), "{text}");
}

#[test]
fn deleted_elements_are_not_in_the_outline() {
    let (mut store, r) = build(SMALL, &CreateOpts::default());
    let mut tx = store.begin();
    tx.update(&r.nodes["ok"], |e| e.deleted = true).unwrap();
    tx.commit().unwrap();
    let o: Outline = outline(&store, &OutlineOpts::default());
    assert!(o.nodes.iter().all(|n| n.id != r.nodes["ok"]));
}
