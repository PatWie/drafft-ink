//! Tests of `diagram.create`, `diagram.import_mermaid` and `diagram.extend`. Text is measured by the fake services
//! (every character is half the font size wide), so sizes and positions are exact and need no fonts.

use drafftink_scene::{ElementId, ElementKind, Kind, SceneRead};
use serde_json::{Value, json};

use crate::batch::{BatchOpts, BatchStep};
use crate::ports::ClipPayload;
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

fn flow() -> Value {
    json!({
        "nodes": [
            {"id": "start", "label": "Start", "shape": "ellipse"},
            {"id": "check", "label": "Valid?", "shape": "diamond"},
            {"id": "done", "label": "Done"}
        ],
        "edges": [
            {"from": "start", "to": "check"},
            {"from": "check", "to": "done", "label": "yes"}
        ]
    })
}

fn live_of(s: &Session, kind: Kind) -> Vec<ElementId> {
    s.store().iter().filter(|e| e.tag() == kind).map(|e| e.id.clone()).collect()
}

// ---- diagram.create ----

#[test]
fn create_builds_shapes_labels_and_bound_arrows_in_one_undo_step() {
    let (mut s, _f) = testing::session();
    let out = run(&mut s, "diagram.create", flow());
    let nodes = out["nodes"].as_object().unwrap();
    assert_eq!(nodes.len(), 3);
    let (start, check, done) = (eid(&nodes["start"]), eid(&nodes["check"]), eid(&nodes["done"]));
    assert_eq!(s.element(&start).unwrap().tag(), Kind::Ellipse);
    assert_eq!(s.element(&check).unwrap().tag(), Kind::Diamond);
    assert_eq!(s.element(&done).unwrap().tag(), Kind::Rectangle);
    // 3 nodes, 3 node labels, 2 arrows, 1 arrow label.
    assert_eq!(s.store().iter().count(), 9);
    assert_eq!(out["created"].as_array().unwrap().len(), 9);
    let edges = out["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 2);
    let a0 = s.element(&eid(&edges[0])).unwrap();
    assert_eq!(a0.binding_targets(), [Some(&start), Some(&check)]);
    let a1 = s.element(&eid(&edges[1])).unwrap();
    assert_eq!(a1.binding_targets(), [Some(&check), Some(&done)]);
    assert!(drafftink_edit::text_bind::label_of(s.store(), &eid(&edges[1])).is_some());
    // The nodes have the minimum size and are laid out top to bottom.
    let (rs, rc, rd) = (s.element(&start).unwrap().rect(), s.element(&check).unwrap().rect(), s.element(&done).unwrap().rect());
    assert!(rs.width() >= 80.0 && rs.height() >= 48.0);
    assert!(rs.y1 < rc.y0 && rc.y1 < rd.y0, "layers grow downwards");
    // One undo step; the selection is the nodes and arrows.
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert_eq!(s.selection().len(), 5);
    s.undo_steps(1).unwrap();
    assert_eq!(s.store().iter().count(), 0);
    assert!(s.selection().is_empty());
}

#[test]
fn create_centres_the_diagram_on_the_viewport_by_default() {
    let (mut s, _f) = testing::session();
    run(&mut s, "view.set", json!({"scroll": [1000, 500]}));
    let out = run(&mut s, "diagram.create", flow());
    let b = &out["bounds"];
    let cx = b["x"].as_f64().unwrap() + b["w"].as_f64().unwrap() / 2.0;
    let cy = b["y"].as_f64().unwrap() + b["h"].as_f64().unwrap() / 2.0;
    assert!((cx - 1000.0).abs() <= 1.0 && (cy - 500.0).abs() <= 1.0, "centre ({cx}, {cy})");
    // Arrows still end on their shapes after the move.
    let e0 = eid(&out["edges"][0]);
    assert!(s.element(&e0).unwrap().binding_targets().iter().all(Option::is_some));
}

#[test]
fn create_at_a_point_and_below_content() {
    let (mut s, _f) = testing::session();
    let mut a = flow();
    a["at"] = json!([300, 200]);
    let out = run(&mut s, "diagram.create", a);
    assert_eq!((out["bounds"]["x"].as_f64().unwrap(), out["bounds"]["y"].as_f64().unwrap()), (300.0, 200.0));
    let bottom = out["bounds"]["y"].as_f64().unwrap() + out["bounds"]["h"].as_f64().unwrap();
    let mut b = flow();
    b["at"] = json!("below_content");
    let out2 = run(&mut s, "diagram.create", b);
    assert!(out2["bounds"]["y"].as_f64().unwrap() >= bottom, "{out2}");
}

#[test]
fn create_with_group_and_frame() {
    let (mut s, _f) = testing::session();
    let mut a = flow();
    a["group"] = json!(true);
    a["frame"] = json!("Login flow");
    let out = run(&mut s, "diagram.create", a);
    assert_eq!(out["groups"].as_array().unwrap().len(), 1);
    let frame = eid(&out["frame"]);
    assert!(matches!(&s.element(&frame).unwrap().kind, ElementKind::Frame(f) if f.name.as_deref() == Some("Login flow")));
    let start = eid(&out["nodes"]["start"]);
    assert_eq!(s.element(&start).unwrap().group_ids.len(), 1);
    assert_eq!(s.element(&start).unwrap().frame_id, Some(frame));
}

#[test]
fn create_with_the_none_layout_uses_the_given_positions() {
    let (mut s, _f) = testing::session();
    let out = run(
        &mut s,
        "diagram.create",
        json!({"nodes": [{"id": "a", "x": 10, "y": 20, "size": [100, 50]}, {"id": "b", "x": 400, "y": 20, "size": [100, 50]}],
               "edges": [{"from": "a", "to": "b"}], "layout": {"algorithm": "none"}}),
    );
    let a = s.element(&eid(&out["nodes"]["a"])).unwrap();
    assert_eq!((a.transform.x, a.transform.y), (10.0, 20.0));
    let r = refuse(&mut s, "diagram.create", json!({"nodes": [{"id": "a"}], "layout": {"algorithm": "none"}}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("Give x and y for every node"), "{}", r.message);
}

#[test]
fn symbolic_ids_work_in_a_batch_and_the_batch_is_one_undo_step() {
    let (mut s, _f) = testing::session();
    let steps = [
        BatchStep::new("diagram.create", flow()).named("d"),
        BatchStep::new("text.set", json!({"id": "$d.check", "text": "Is it valid?"})),
        BatchStep::new("element.noop_placeholder", json!({})),
    ];
    // The unknown command is caught by preflight and nothing runs.
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.failed.is_some());
    assert_eq!(s.store().iter().count(), 0);
    let out = s.execute_batch(Origin::Test, &steps[..2], &BatchOpts::default());
    assert!(out.failed.is_none(), "{:?}", out.failed);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    let check = live_of(&s, Kind::Diamond)[0].clone();
    let label = drafftink_edit::text_bind::label_of(s.store(), &check).unwrap();
    assert_eq!(s.element(&label).unwrap().text_data().unwrap().content, "Is it valid?");
    s.undo_steps(1).unwrap();
    assert_eq!(s.store().iter().count(), 0);
}

#[test]
fn create_refusals_have_the_catalog_texts() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "diagram.create", json!({"nodes": []}));
    assert_eq!((r.code, r.message.as_str()), ("bad_args", "Give at least one node."));
    let r = refuse(&mut s, "diagram.create", json!({"nodes": [{"id": "a"}, {"id": "a"}]}));
    assert_eq!((r.code, r.message.as_str()), ("duplicate_node", "Node id `a` appears twice. Node ids must be unique."));
    let r = refuse(&mut s, "diagram.create", json!({"nodes": [{"id": "a"}, {"id": "b"}], "edges": [{"from": "a", "to": "c"}]}));
    assert_eq!(r.code, "unknown_node");
    assert_eq!(r.message, "Edge 0 refers to node `c`, which is not in `nodes`. Known nodes: a, b.");
    let r = refuse(
        &mut s,
        "diagram.create",
        json!({"nodes": [{"id": "a"}, {"id": "b"}], "edges": [{"from": "a", "to": "b"}, {"from": "b", "to": "a"}], "layout": {"algorithm": "tree"}}),
    );
    assert_eq!(r.code, "tree_not_forest");
    assert!(r.message.contains("Use algorithm \"layered\", which handles cycles."), "{}", r.message);
    let r = refuse(&mut s, "diagram.create", json!({"nodes": [{"id": "a", "name": "x"}, {"id": "b", "name": "x"}]}));
    assert_eq!(r.code, "name_taken");
    let r = refuse(&mut s, "diagram.create", json!({"nodes": [{"id": "a", "name": "@bad"}]}));
    assert_eq!(r.code, "bad_name");
}

#[test]
fn create_checks_limits_and_numbers() {
    let (mut s, _f) = testing::session();
    let many: Vec<Value> = (0..2001).map(|i| json!({"id": format!("n{i}")})).collect();
    let r = refuse(&mut s, "diagram.create", json!({"nodes": many}));
    assert_eq!(r.code, "limit_exceeded");
    assert!(r.message.contains("2001 nodes") && r.message.contains("2000"), "{}", r.message);
    let r = refuse(&mut s, "diagram.create", json!({"nodes": [{"id": "a", "width": 1e300}]}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut s, "diagram.create", json!({"nodes": [{"id": "a", "style": {"opacity": 4}}]}));
    assert!(r.message.contains("nodes[0].style.opacity"), "{}", r.message);
    let r = refuse(&mut s, "diagram.create", json!({"nodes": [{"id": "a"}], "layout": {"node_gap": -1}}));
    assert_eq!(r.code, "bad_args");
    s.view_only = true;
    assert_eq!(refuse(&mut s, "diagram.create", flow()).code, "view_only");
}

// ---- diagram.import_mermaid ----

#[test]
fn a_mermaid_flowchart_becomes_a_grouped_diagram_with_bound_arrows() {
    let (mut s, _f) = testing::session();
    let out = run(
        &mut s,
        "diagram.import_mermaid",
        json!({"source": {"text": "```mermaid\ngraph LR\n  A[Start] --> B{Ok?}\n  B -->|yes| C((Done))\n  B -.->|no| A\n```"}}),
    );
    let nodes = out["nodes"].as_object().unwrap();
    assert_eq!(nodes.len(), 3);
    assert_eq!(s.element(&eid(&nodes["B"])).unwrap().tag(), Kind::Diamond);
    assert_eq!(s.element(&eid(&nodes["C"])).unwrap().tag(), Kind::Ellipse);
    let (ra, rb, rc) = (
        s.element(&eid(&nodes["A"])).unwrap().rect(),
        s.element(&eid(&nodes["B"])).unwrap().rect(),
        s.element(&eid(&nodes["C"])).unwrap().rect(),
    );
    assert!(ra.x1 < rb.x0 && rb.x1 < rc.x0, "left to right");
    assert_eq!(out["edges"].as_array().unwrap().len(), 3);
    for e in out["edges"].as_array().unwrap() {
        assert!(s.element(&eid(e)).unwrap().binding_targets().iter().all(Option::is_some));
    }
    assert_eq!(out["groups"].as_array().unwrap().len(), 1, "the old import always made one group");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    // group=false leaves the elements loose.
    let out = run(&mut s, "diagram.import_mermaid", json!({"source": {"text": "graph TD; X-->Y"}, "group": false}));
    assert!(out["groups"].as_array().unwrap().is_empty());
}

#[test]
fn mermaid_subgraphs_are_nested_groups_and_the_loss_is_reported() {
    let (mut s, _f) = testing::session();
    let out = run(
        &mut s,
        "diagram.import_mermaid",
        json!({"source": {"text": "graph TD\nsubgraph one\nA-->B\nend\nB-->C[(db)]"}}),
    );
    assert_eq!(out["groups"].as_array().unwrap().len(), 2, "{out}");
    let w: Vec<&str> = out["warnings"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert!(w.iter().any(|x| x.contains("drawn as rectangles")), "{w:?}");
    assert!(w.iter().any(|x| x.contains("subgraph title")), "{w:?}");
}

#[test]
fn a_mermaid_sequence_diagram_gets_lifelines_and_messages() {
    let (mut s, _f) = testing::session();
    let out = run(
        &mut s,
        "diagram.import_mermaid",
        json!({"source": {"text": "sequenceDiagram\nparticipant A\nparticipant B as Bob\nA->>B: hello\nB-->>A: hi back\nNote over A,B: done"}, "at": [0, 0]}),
    );
    assert_eq!(out["nodes"].as_object().unwrap().len(), 3, "two participants and one note");
    let lines = live_of(&s, Kind::Line);
    assert_eq!(lines.len(), 2, "one lifeline per participant");
    let arrows = live_of(&s, Kind::Arrow);
    assert_eq!(arrows.len(), 2);
    let ys: Vec<f64> = arrows.iter().map(|a| s.element(a).unwrap().rect().y0).collect();
    assert!(ys[0] < ys[1], "messages go down in source order");
    let labels: Vec<String> = arrows
        .iter()
        .filter_map(|a| drafftink_edit::text_bind::label_of(s.store(), a))
        .map(|l| s.element(&l).unwrap().text_data().unwrap().content.clone())
        .collect();
    assert_eq!(labels, ["hello", "hi back"]);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(s.store().iter().count(), 0);
}

#[test]
fn mermaid_from_the_clipboard_and_its_refusals() {
    let (mut s, f) = testing::session();
    let r = refuse(&mut s, "diagram.import_mermaid", json!({"source": "clipboard"}));
    assert_eq!(r.code, "clipboard_empty");
    assert!(r.message.contains("source: {\"text\""), "{}", r.message);
    f.clipboard.set(ClipPayload::Text("graph TD; A-->B".to_owned()));
    let out = run(&mut s, "diagram.import_mermaid", json!({"source": "clipboard"}));
    assert_eq!(out["nodes"].as_object().unwrap().len(), 2);
    let mut headless = testing::headless_session();
    let r = refuse(&mut headless, "diagram.import_mermaid", json!({"source": "clipboard"}));
    assert_eq!(r.code, "port_unavailable");
    assert!(r.message.contains("source: {\"text\""), "{}", r.message);
    // Other diagram types and plain text.
    let r = refuse(&mut s, "diagram.import_mermaid", json!({"source": {"text": "classDiagram\nA <|-- B"}}));
    assert_eq!(
        (r.code, r.message.as_str()),
        ("mermaid_unsupported", "Mermaid diagram type `classDiagram` is not supported yet. Supported: flowchart/graph and sequenceDiagram.")
    );
    let r = refuse(&mut s, "diagram.import_mermaid", json!({"source": {"text": "just some words"}}));
    assert_eq!(r.code, "parse_failed");
    assert!(r.message.starts_with("The Mermaid text could not be read: ") && r.message.ends_with("Fix that line and try again."), "{}", r.message);
    assert!(r.message.contains("(line 1)"), "{}", r.message);
}

// ---- diagram.extend ----

#[test]
fn extend_adds_a_connected_node_next_to_the_selection_and_selects_it() {
    let (mut s, _f) = testing::session();
    let out = run(&mut s, "diagram.create", json!({"nodes": [{"id": "a", "label": "A"}], "at": [0, 0]}));
    let a = eid(&out["nodes"]["a"]);
    testing::select(&mut s, std::slice::from_ref(&a));
    let ra = s.element(&a).unwrap().rect();
    let out = run(&mut s, "diagram.extend", json!({"direction": "right", "label": "B"}));
    let (node, arrow) = (eid(&out["node"]), eid(&out["arrow"]));
    let rn = s.element(&node).unwrap().rect();
    assert!(rn.x0 >= ra.x1 + 80.0 - 1e-6 && (rn.center().y - ra.center().y).abs() < 1.0, "{rn:?} vs {ra:?}");
    assert_eq!(s.element(&arrow).unwrap().binding_targets(), [Some(&a), Some(&node)]);
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), vec![&node]);
    assert!(out["node_label"].is_string());
    assert_eq!(s.tab().doc.undo_depth(), 2);
    // The same direction again finds the next free spot.
    testing::select(&mut s, std::slice::from_ref(&a));
    let out2 = run(&mut s, "diagram.extend", json!({"direction": "right"}));
    let rn2 = s.element(&eid(&out2["node"])).unwrap().rect();
    assert!(rn.x1 <= rn2.x0 || rn2.x1 <= rn.x0 || rn.y1 <= rn2.y0 || rn2.y1 <= rn.y0, "the second node must not overlap the first");
}

#[test]
fn extend_refusals() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "diagram.extend", json!({"direction": "down"}));
    assert_eq!(r.code, "needs_single");
    let t = testing::add(&mut s, ElementKind::Text(drafftink_scene::TextData::new("t")), 0.0, 0.0, 10.0, 20.0).unwrap();
    let r = refuse(&mut s, "diagram.extend", json!({"from": t.as_str(), "direction": "down"}));
    assert_eq!(r.code, "wrong_kind");
    assert!(r.message.contains("rectangle, diamond and ellipse"), "{}", r.message);
    assert_eq!(refuse(&mut s, "diagram.extend", json!({"from": "nope", "direction": "down"})).code, "not_found");
    let rc = testing::add_rect(&mut s, 0.0, 100.0, 50.0, 50.0).unwrap();
    assert_eq!(refuse(&mut s, "diagram.extend", json!({"from": rc.as_str(), "direction": "down", "gap": -5})).code, "bad_args");
}

#[test]
fn extend_has_the_flowchart_shortcuts() {
    let s = testing::session().0;
    let spec = s.spec("diagram.extend").unwrap();
    let keys: Vec<&str> = spec.shortcuts.iter().flat_map(|b| b.keys.iter().copied()).collect();
    assert_eq!(keys, ["Mod+Left", "Mod+Right", "Mod+Up", "Mod+Down"]);
}

/// The same flow with the commands of this task only: relabel with `text.set`, no connection.
#[test]
fn scenario_an_agent_builds_relabels_reads_and_undoes() {
    let (mut s, _f) = testing::session();
    let out = run(
        &mut s,
        "diagram.create",
        json!({
            "nodes": [
                {"id": "start", "label": "Start", "shape": "ellipse"},
                {"id": "read", "label": "Read input"},
                {"id": "valid", "label": "Valid?", "shape": "diamond"},
                {"id": "fix", "label": "Fix input"},
                {"id": "save", "label": "Save"},
                {"id": "done", "label": "Done", "shape": "ellipse"}
            ],
            "edges": [
                {"from": "start", "to": "read"},
                {"from": "read", "to": "valid"},
                {"from": "valid", "to": "save", "label": "yes"},
                {"from": "valid", "to": "fix", "label": "no"},
                {"from": "save", "to": "done"}
            ],
            "at": [0, 0]
        }),
    );
    assert_eq!(s.tab().doc.undo_depth(), 1);
    let outline = |s: &mut Session| run(s, "doc.outline", json!({"format": "text"}))["text"].as_str().unwrap().to_owned();
    let built = outline(&mut s);
    assert!(built.starts_with("nodes (6):\n") && built.contains("edges (5):\n"), "{built}");
    let (valid, save) = (out["nodes"]["valid"].as_str().unwrap(), out["nodes"]["save"].as_str().unwrap());
    assert!(built.contains(&format!("{valid} -> {save} \"yes\"")), "{built}");
    assert_eq!(outline(&mut s), built, "reading twice gives the same text");

    run(&mut s, "text.set", json!({"id": valid, "text": "Input valid?"}));
    let relabeled = outline(&mut s);
    assert!(relabeled.contains("\"Input valid?\" diamond") && !relabeled.contains("\"Valid?\""), "{relabeled}");
    assert_eq!(s.tab().doc.undo_depth(), 2);

    s.undo_steps(1).unwrap();
    assert_eq!(outline(&mut s), built, "one undo takes back the relabel only");
    s.undo_steps(1).unwrap();
    assert_eq!(outline(&mut s), "empty document: no elements.\n");
}

// ---- the agent scenario ----

/// An agent builds a flowchart in one call, relabels a node, connects two nodes, reads the outline, undoes once
/// and reads the outline again. Each of its edits is one undo step, and the outline is the same text every time
/// the document is the same.
#[test]
fn scenario_an_agent_builds_edits_and_reads_back_a_flowchart() {
    let (mut s, _f) = testing::session();
    if s.spec("element.connect").is_none() || s.spec("element.relabel").is_none() {
        // `element.relabel` and `element.connect` belong to the element family (branch f-element-selection). The
        // text-only variant below covers the same flow with `text.set` until that family is registered.
        eprintln!("skipped: element.relabel / element.connect are not registered yet");
        return;
    }
    let out = run(
        &mut s,
        "diagram.create",
        json!({
            "nodes": [
                {"id": "start", "label": "Start", "shape": "ellipse", "name": "start"},
                {"id": "read", "label": "Read input"},
                {"id": "valid", "label": "Valid?", "shape": "diamond"},
                {"id": "fix", "label": "Fix input"},
                {"id": "save", "label": "Save"},
                {"id": "done", "label": "Done", "shape": "ellipse"}
            ],
            "edges": [
                {"from": "start", "to": "read"},
                {"from": "read", "to": "valid"},
                {"from": "valid", "to": "save", "label": "yes"},
                {"from": "valid", "to": "fix", "label": "no"},
                {"from": "save", "to": "done"}
            ],
            "at": [0, 0]
        }),
    );
    assert_eq!(s.tab().doc.undo_depth(), 1, "the whole flowchart is one undo step");
    let n = |k: &str| out["nodes"][k].as_str().unwrap().to_owned();
    let (valid, fix, read) = (n("valid"), n("fix"), n("read"));

    // Relabel a node by the id the first call returned.
    run(&mut s, "element.relabel", json!({"id": valid, "text": "Input valid?"}));
    // Connect two nodes that the diagram did not connect.
    let outline_before = run(&mut s, "doc.outline", json!({"format": "text"}))["text"].as_str().unwrap().to_owned();
    assert!(outline_before.starts_with("nodes (6):\n"), "{outline_before}");
    assert!(outline_before.contains("\"Input valid?\" diamond"), "{outline_before}");
    assert!(outline_before.contains("edges (5):\n"), "{outline_before}");
    let c = run(&mut s, "element.connect", json!({"from": fix, "to": read, "label": "retry"}));
    assert_eq!(c["created"].as_array().unwrap().len(), 2, "an arrow and its label: {c}");
    let outline_after = run(&mut s, "doc.outline", json!({"format": "text"}));
    let t = outline_after["text"].as_str().unwrap();
    assert!(t.contains("edges (6):\n") && t.contains(&format!("{fix} -> {read} \"retry\"")), "{t}");
    assert_eq!(s.tab().doc.undo_depth(), 3);
    // The outline is the same text when read again.
    assert_eq!(run(&mut s, "doc.outline", json!({"format": "text"})), outline_after);

    // One undo takes back the connection only.
    s.undo_steps(1).unwrap();
    let back = run(&mut s, "doc.outline", json!({"format": "text"}));
    assert_eq!(back["text"].as_str().unwrap(), outline_before);
    assert!(back["outline"].is_null());
    // The following undo steps take back the relabel, then the diagram.
    s.undo_steps(1).unwrap();
    let t = run(&mut s, "doc.outline", json!({"format": "text"}));
    assert!(t["text"].as_str().unwrap().contains("\"Valid?\" diamond"));
    s.undo_steps(1).unwrap();
    assert_eq!(run(&mut s, "doc.outline", json!({"format": "text"}))["text"], "empty document: no elements.\n");
}
