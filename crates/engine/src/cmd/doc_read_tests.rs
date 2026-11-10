//! Tests of `doc.outline` and `doc.inspect`: read-only, allowed in every state, deterministic.

use drafftink_scene::{ElementId, ElementKind, FrameData};
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

fn text(v: &Value) -> &str {
    v["text"].as_str().unwrap()
}

fn flow(s: &mut Session) -> Value {
    run(
        s,
        "diagram.create",
        json!({"nodes": [{"id": "a", "label": "Start", "name": "start"}, {"id": "b", "label": "End"}],
               "edges": [{"from": "a", "to": "b", "label": "go"}], "at": [0, 0]}),
    )
}

#[test]
fn both_commands_are_read_only_and_run_in_every_state() {
    let (mut s, _f) = testing::session();
    flow(&mut s);
    s.view_only = true;
    let rev = s.tab().doc.revision();
    let depth = s.tab().doc.undo_depth();
    for id in ["doc.outline", "doc.inspect"] {
        assert!(s.availability(id).is_ok(), "{id}");
        run(&mut s, id, json!({}));
    }
    s.text_edit = Some(crate::state::TextEdit { element: None, draft: true });
    run(&mut s, "doc.outline", json!({}));
    assert_eq!((s.tab().doc.revision(), s.tab().doc.undo_depth()), (rev, depth));
}

#[test]
fn the_outline_text_is_one_line_per_node_and_edge() {
    let (mut s, _f) = testing::session();
    let out = flow(&mut s);
    let (a, b, e) = (out["nodes"]["a"].as_str().unwrap(), out["nodes"]["b"].as_str().unwrap(), out["edges"][0].as_str().unwrap());
    let o = run(&mut s, "doc.outline", json!({"format": "text"}));
    assert!(o["outline"].is_null());
    let t = text(&o);
    assert!(t.starts_with("nodes (2):\n"), "{t}");
    assert!(t.contains(&format!("{a} @start \"Start\" rectangle (0,0,")), "{t}");
    assert!(t.contains(&format!("{a} -> {b} \"go\" [{e}]")), "{t}");
    let j = run(&mut s, "doc.outline", json!({"format": "json"}));
    assert!(j["text"].is_null());
    assert_eq!(j["outline"]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(j["outline"]["edges"][0]["label"], "go");
    let both = run(&mut s, "doc.outline", json!({}));
    assert!(both["text"].is_string() && both["outline"].is_object());
}

#[test]
fn the_outline_does_not_depend_on_creation_order() {
    let build = |order: [(&str, f64, f64); 3]| {
        let (mut s, _f) = testing::session();
        for (label, x, y) in order {
            let id = testing::add_rect(&mut s, x, y, 100.0, 50.0).unwrap();
            run(&mut s, "text.set", json!({"id": id.as_str(), "text": label}));
        }
        let t = text(&run(&mut s, "doc.outline", json!({"format": "text"}))).to_owned();
        // Ids differ between the two runs; compare the lines without them.
        t.lines().map(|l| l.split_once(' ').map_or(l, |x| x.1).to_owned()).collect::<Vec<_>>()
    };
    let a = build([("one", 0.0, 0.0), ("two", 200.0, 0.0), ("three", 0.0, 200.0)]);
    let b = build([("three", 0.0, 200.0), ("one", 0.0, 0.0), ("two", 200.0, 0.0)]);
    assert_eq!(a, b);
    assert!(a[1].contains("\"one\"") && a[2].contains("\"two\"") && a[3].contains("\"three\""), "{a:?}");
}

#[test]
fn an_empty_document_and_truncation_say_so() {
    let (mut s, _f) = testing::session();
    assert_eq!(text(&run(&mut s, "doc.outline", json!({"format": "text"}))), "empty document: no elements.\n");
    for i in 0..5 {
        testing::add_rect(&mut s, f64::from(i) * 100.0, 0.0, 50.0, 50.0).unwrap();
    }
    let o = run(&mut s, "doc.outline", json!({"max_nodes": 2}));
    assert!(text(&o).contains("showing 2 of 5 nodes"), "{}", text(&o));
    assert_eq!(o["outline"]["nodes"].as_array().unwrap().len(), 2);
    assert!(o["outline"]["truncated"]["hint"].is_string());
}

#[test]
fn scopes_selection_visible_and_frame() {
    let (mut s, _f) = testing::session();
    let near = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let _far = testing::add_rect(&mut s, 100_000.0, 0.0, 50.0, 50.0).unwrap();
    let frame = testing::add(&mut s, ElementKind::Frame(FrameData { name: Some("F".into()), clip: true }), 500.0, 500.0, 300.0, 300.0).unwrap();
    let inside = testing::add_rect(&mut s, 520.0, 520.0, 50.0, 50.0).unwrap();
    s.tab_mut()
        .doc
        .edit::<(), drafftink_scene::StoreError>(|tx| {
            use drafftink_scene::SceneTx;
            tx.update(&inside, |e| e.frame_id = Some(frame.clone()))
        })
        .unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    let ids = |v: &Value| -> Vec<String> { v["outline"]["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap().to_owned()).collect() };
    let r = refuse(&mut s, "doc.outline", json!({"scope": "selection"}));
    assert_eq!(r.code, "nothing_to_do");
    assert!(r.message.contains("scope \"all\""), "{}", r.message);
    testing::select(&mut s, std::slice::from_ref(&near));
    assert_eq!(ids(&run(&mut s, "doc.outline", json!({"scope": "selection", "format": "json"}))), [near.as_str()]);
    let visible = ids(&run(&mut s, "doc.outline", json!({"scope": "visible", "format": "json"})));
    assert!(visible.contains(&near.as_str().to_owned()) && !visible.contains(&"e2".to_owned()), "{visible:?}");
    let framed = run(&mut s, "doc.outline", json!({"scope": "frame", "frame": frame.as_str(), "format": "json"}));
    assert_eq!(ids(&framed), [inside.as_str()]);
    assert_eq!(refuse(&mut s, "doc.outline", json!({"scope": "frame"})).code, "bad_args");
    assert_eq!(refuse(&mut s, "doc.outline", json!({"scope": "all", "frame": frame.as_str()})).code, "bad_args");
    assert_eq!(refuse(&mut s, "doc.outline", json!({"scope": "frame", "frame": near.as_str()})).code, "wrong_kind");
    assert_eq!(refuse(&mut s, "doc.outline", json!({"max_nodes": 0})).code, "bad_args");
}

#[test]
fn inspect_summarises_the_document() {
    let (mut s, _f) = testing::session();
    flow(&mut s);
    let r = testing::add_rect(&mut s, 400.0, 400.0, 50.0, 50.0).unwrap();
    s.tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| {
        use drafftink_scene::SceneTx;
        tx.update(&r, |e| e.locked = true)
    }).unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    let v = run(&mut s, "doc.inspect", json!({}));
    assert_eq!(v["kinds"]["rectangle"], 3);
    assert_eq!(v["kinds"]["arrow"], 1);
    assert_eq!(v["locked"], 1);
    assert_eq!(v["frames"], 0);
    assert_eq!(v["tab"], "t1");
    assert_eq!(v["dirty"], true);
    assert!(v["bounds"]["w"].as_f64().unwrap() >= 450.0 - 1.0);
    let r = refuse(&mut s, "doc.inspect", json!({"ids": ["x"]}));
    assert!(r.message.contains("detail \"full\""), "{}", r.message);
}

#[test]
fn inspect_full_pages_and_lists_deleted_elements_on_request() {
    let (mut s, _f) = testing::session();
    let ids: Vec<ElementId> = (0..5).map(|i| testing::add_rect(&mut s, f64::from(i) * 100.0, 0.0, 50.0, 50.0).unwrap()).collect();
    run(&mut s, "text.set", json!({"id": ids[0].as_str(), "text": "hello"}));
    let v = run(&mut s, "doc.inspect", json!({"detail": "full", "limit": 3}));
    assert_eq!((v["total"].clone(), v["truncated"].clone()), (json!(6), json!(true)), "five rectangles and a label");
    let first = &v["items"][0];
    for k in ["id", "kind", "name", "text", "bounds", "style", "angle", "group_ids", "frame", "bindings", "bound_text", "locked", "z"] {
        assert!(first.get(k).is_some(), "missing {k}: {first}");
    }
    assert!(first["bound_text"].as_str().is_some() || first["text"] == "hello");
    let rest = run(&mut s, "doc.inspect", json!({"detail": "full", "limit": 3, "offset": 3}));
    assert_eq!((rest["items"].as_array().unwrap().len(), rest["truncated"].clone()), (3, json!(false)));
    let one = run(&mut s, "doc.inspect", json!({"detail": "full", "ids": [ids[1].as_str()]}));
    assert_eq!(one["items"][0]["id"], ids[1].as_str());
    assert_eq!(refuse(&mut s, "doc.inspect", json!({"detail": "full", "ids": ["nope"]})).code, "not_found");
    assert_eq!(refuse(&mut s, "doc.inspect", json!({"detail": "full", "limit": 0})).code, "bad_args");
    // A deleted element is only listed on request, marked as deleted.
    s.tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| {
        use drafftink_scene::SceneTx;
        tx.update(&ids[4], |e| e.deleted = true)
    }).unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    assert_eq!(run(&mut s, "doc.inspect", json!({"detail": "full"}))["total"], 5);
    let all = run(&mut s, "doc.inspect", json!({"detail": "full", "include_deleted": true}));
    assert_eq!(all["total"], 6);
    assert!(all["items"].as_array().unwrap().iter().any(|i| i["deleted"] == true));
}
