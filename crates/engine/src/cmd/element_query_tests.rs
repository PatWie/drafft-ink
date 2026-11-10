//! Tests of `element.query` and `element.at`. Pattern per command: availability, exact refusal text, the
//! result, a negative case that leaves the fingerprint alone. No UI: a headless session and the fake ports.

use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    BindMode, Binding as ArrowBinding, Color, Element, ElementId, ElementKind, FrameData, GroupId, LinearData, MathData,
    SceneTx, StoreError, TextData,
};
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

fn refuse_clean(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    };
    assert_eq!(s.fingerprint(), before, "{id} refused with `{}` but changed state", r.code);
    r
}

fn update(s: &mut Session, id: &ElementId, f: impl FnOnce(&mut Element)) {
    s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.update(id, f)).unwrap();
    s.tab_mut().doc.forget_journal();
}

fn filled(s: &mut Session, kind: ElementKind, x: f64, y: f64, w: f64, h: f64) -> ElementId {
    let id = testing::add(s, kind, x, y, w, h).unwrap();
    update(s, &id, |e| e.style.fill_color = Some(Color::rgb(200, 200, 255)));
    id
}

struct World {
    s: Session,
    login: ElementId,
    label: ElementId,
    db: ElementId,
    node: ElementId,
    text: ElementId,
    math: ElementId,
    ga: ElementId,
    gb: ElementId,
    locked: ElementId,
    frame: ElementId,
    kid: ElementId,
    arrow: ElementId,
}

fn world() -> World {
    let mut s = testing::headless_session();
    let login = filled(&mut s, ElementKind::Rectangle, 0.0, 0.0, 200.0, 100.0);
    let label = testing::add(&mut s, ElementKind::Text(TextData::new("Sign In")), 10.0, 10.0, 80.0, 20.0).unwrap();
    let db = filled(&mut s, ElementKind::Rectangle, 300.0, 0.0, 100.0, 100.0);
    let node = filled(&mut s, ElementKind::Ellipse, 0.0, 200.0, 100.0, 100.0);
    let text = testing::add(&mut s, ElementKind::Text(TextData::new("Hello World")), 500.0, 0.0, 100.0, 30.0).unwrap();
    let math = testing::add(&mut s, ElementKind::Math(MathData::new("x^2 + y^2")), 500.0, 100.0, 60.0, 30.0).unwrap();
    let ga = filled(&mut s, ElementKind::Rectangle, 0.0, 400.0, 50.0, 50.0);
    let gb = filled(&mut s, ElementKind::Rectangle, 100.0, 400.0, 50.0, 50.0);
    let locked = filled(&mut s, ElementKind::Rectangle, 300.0, 400.0, 50.0, 50.0);
    let frame = testing::add(
        &mut s,
        ElementKind::Frame(FrameData { name: Some("Login frame".into()), clip: true }),
        0.0,
        600.0,
        300.0,
        300.0,
    )
    .unwrap();
    let kid = filled(&mut s, ElementKind::Rectangle, 20.0, 620.0, 50.0, 50.0);
    let arrow = testing::add(
        &mut s,
        ElementKind::Arrow(LinearData::new(vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)])),
        200.0,
        50.0,
        100.0,
        0.0,
    )
    .unwrap();
    let (l, d, f) = (login.clone(), db.clone(), frame.clone());
    update(&mut s, &login, |e| e.name = Some("login_box".into()));
    update(&mut s, &db, |e| {
        e.name = Some("db_box".into());
        e.style.stroke_width = 4.0;
    });
    update(&mut s, &label, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.container_id = Some(l.clone());
        }
    });
    for g in [&ga, &gb] {
        update(&mut s, g, |e| e.group_ids.push(GroupId::new("G").unwrap()));
    }
    update(&mut s, &locked, |e| e.locked = true);
    update(&mut s, &kid, |e| e.frame_id = Some(f));
    update(&mut s, &arrow, |e| {
        if let ElementKind::Arrow(a) = &mut e.kind {
            a.start_binding = Some(ArrowBinding { target: l, mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
            a.end_binding = Some(ArrowBinding { target: d, mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
        }
    });
    s.take_dirty();
    World { s, login, label, db, node, text, math, ga, gb, locked, frame, kid, arrow }
}

fn item_ids(v: &Value) -> Vec<String> {
    v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_owned()).collect()
}

fn ids(list: &[&ElementId]) -> Vec<String> {
    list.iter().map(|i| i.as_str().to_owned()).collect()
}

fn find(s: &mut Session, filter: Value) -> Vec<String> {
    item_ids(&run(s, "element.query", json!({"filter": filter})))
}

// ---- element.query ----

#[test]
fn query_is_always_available_and_changes_nothing() {
    let mut w = world();
    w.s.view_only = true;
    w.s.text_edit = Some(crate::state::TextEdit { element: None, draft: true });
    assert!(w.s.availability("element.query").is_ok());
    assert!(w.s.availability("element.at").is_ok());
    let before = w.s.fingerprint();
    run(&mut w.s, "element.query", json!({}));
    run(&mut w.s, "element.at", json!({"point": [10, 10]}));
    assert_eq!(w.s.fingerprint(), before);
    assert_eq!(w.s.take_dirty(), None);
    let spec = w.s.spec("element.query").unwrap();
    assert!(spec.reads_only && spec.agent && !spec.palette);
}

#[test]
fn query_without_a_filter_lists_every_element_back_to_front_with_the_default_fields() {
    let mut w = world();
    let v = run(&mut w.s, "element.query", json!({}));
    assert_eq!(v["total"], 11, "the bound label follows its container and is left out");
    assert_eq!(v["truncated"], false);
    assert_eq!(v["changed"], false);
    let first = &v["items"][0];
    let mut keys: Vec<&str> = first.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["bounds", "id", "kind", "name", "text"]);
    assert_eq!(first["id"], w.login.as_str());
    assert_eq!(first["kind"], "rectangle");
    assert_eq!(first["name"], "login_box");
    assert_eq!(first["text"], "Sign In", "a container reports its label");
    assert_eq!(first["bounds"], json!({"x": 0.0, "y": 0.0, "w": 200.0, "h": 100.0}));
    assert_eq!(item_ids(&v).last().map(String::as_str), Some(w.arrow.as_str()));
}

#[test]
fn query_filters_by_kind_text_name_and_style() {
    let mut w = world();
    assert_eq!(find(&mut w.s, json!({"kind": "ellipse"})), ids(&[&w.node]));
    assert_eq!(find(&mut w.s, json!({"kind": ["ellipse", "math"]})), ids(&[&w.node, &w.math]));
    assert_eq!(find(&mut w.s, json!({"text": "sign in"})), ids(&[&w.login]), "case-insensitive, matches the label");
    assert_eq!(find(&mut w.s, json!({"text": {"contains": "login"}})), ids(&[&w.frame]), "frame names count");
    assert_eq!(find(&mut w.s, json!({"text": {"regex": "^x\\^2"}})), ids(&[&w.math]), "LaTeX counts");
    assert_eq!(find(&mut w.s, json!({"kind": "text"})), ids(&[&w.label, &w.text]), "kind=text includes bound text");
    assert_eq!(find(&mut w.s, json!({"name": "*_box"})), ids(&[&w.login, &w.db]));
    assert_eq!(find(&mut w.s, json!({"name": "db_???"})), ids(&[&w.db]));
    assert_eq!(find(&mut w.s, json!({"style": {"stroke_width": 4}})), ids(&[&w.db]));
}

#[test]
fn query_filters_by_region_connection_group_frame_lock_and_selection() {
    let mut w = world();
    let rect = json!({"x": -10, "y": -10, "w": 230, "h": 120});
    assert_eq!(find(&mut w.s, json!({"region": {"rect": rect}})), ids(&[&w.login, &w.arrow]), "the arrow starts at x=200");
    assert_eq!(
        find(&mut w.s, json!({"region": {"rect": {"x": 150, "y": 40, "w": 200, "h": 20}}})),
        ids(&[&w.login, &w.db, &w.arrow]),
        "overlap is the default"
    );
    assert_eq!(
        find(&mut w.s, json!({"region": {"rect": {"x": 150, "y": 40, "w": 200, "h": 20}, "mode": "contain"}})),
        ids(&[&w.arrow]),
        "only the arrow lies completely inside"
    );
    assert_eq!(find(&mut w.s, json!({"connected_to": {"ref": "@login_box"}})), ids(&[&w.db, &w.arrow]));
    assert_eq!(find(&mut w.s, json!({"connected_to": {"ref": "@db_box", "direction": "out"}})), Vec::<String>::new());
    assert_eq!(find(&mut w.s, json!({"connected_to": {"ref": "@login_box", "direction": "out"}})), ids(&[&w.db, &w.arrow]));
    assert_eq!(find(&mut w.s, json!({"group": "G"})), ids(&[&w.ga, &w.gb]));
    assert_eq!(find(&mut w.s, json!({"frame": w.frame.as_str()})), ids(&[&w.kid]));
    assert_eq!(find(&mut w.s, json!({"locked": true})), ids(&[&w.locked]), "locked elements can be found");
    assert_eq!(find(&mut w.s, json!({"has_label": true})), ids(&[&w.login]));
    assert_eq!(find(&mut w.s, json!({"bound_text_of": "@login_box"})), ids(&[&w.label]));
    assert_eq!(find(&mut w.s, json!({"selected": true})), Vec::<String>::new());
    run(&mut w.s, "selection.select", json!({"ids": [w.node.as_str(), w.db.as_str()]}));
    assert_eq!(find(&mut w.s, json!({"selected": true})), ids(&[&w.db, &w.node]));
    assert_eq!(find(&mut w.s, json!({"ids": ["@db_box", w.node.as_str()], "selected": true})), ids(&[&w.db, &w.node]));
}

#[test]
fn query_combines_fields_with_and() {
    let mut w = world();
    assert_eq!(find(&mut w.s, json!({"kind": "rectangle", "name": "*_box", "style": {"stroke_width": 4}})), ids(&[&w.db]));
    assert!(find(&mut w.s, json!({"kind": "ellipse", "name": "*_box"})).is_empty());
}

#[test]
fn query_pages_with_limit_and_offset_and_says_when_more_exist() {
    let mut w = world();
    let p1 = run(&mut w.s, "element.query", json!({"limit": 4, "fields": ["id"]}));
    assert_eq!((p1["total"].as_u64(), p1["truncated"].as_bool()), (Some(11), Some(true)));
    assert_eq!(p1["items"].as_array().unwrap().len(), 4);
    assert_eq!(p1["items"][0], json!({"id": w.login.as_str()}));
    let p3 = run(&mut w.s, "element.query", json!({"limit": 4, "offset": 8, "fields": ["id"]}));
    assert_eq!(p3["items"].as_array().unwrap().len(), 3);
    assert_eq!(p3["truncated"], false);
    let beyond = run(&mut w.s, "element.query", json!({"offset": 500}));
    assert_eq!((beyond["total"].as_u64(), beyond["items"].clone(), beyond["truncated"].clone()), (Some(11), json!([]), json!(false)));
}

#[test]
fn query_orders_by_z_reading_or_position() {
    let mut w = world();
    let z = item_ids(&run(&mut w.s, "element.query", json!({"filter": {"kind": ["rectangle"]}, "fields": ["id"]})));
    assert_eq!(z[0], w.login.as_str());
    let reading = item_ids(&run(&mut w.s, "element.query", json!({"filter": {"kind": "rectangle"}, "order": "reading"})));
    let position = item_ids(&run(&mut w.s, "element.query", json!({"filter": {"kind": "rectangle"}, "order": "position"})));
    // Reading: rows from the top, left to right. Position: columns from the left, top to bottom.
    assert_eq!(reading, ids(&[&w.login, &w.db, &w.ga, &w.gb, &w.locked, &w.kid]));
    assert_eq!(position, ids(&[&w.login, &w.ga, &w.kid, &w.gb, &w.db, &w.locked]));
}

#[test]
fn query_reports_the_selected_fields() {
    let mut w = world();
    let v = run(&mut w.s, "element.query", json!({
        "filter": {"ids": [w.arrow.as_str()]},
        "fields": ["bindings", "z", "angle", "locked", "style", "group_ids", "frame", "bound_text"]
    }));
    let i = &v["items"][0];
    assert_eq!(i["bindings"], json!({"start": w.login.as_str(), "end": w.db.as_str()}));
    assert!(i["z"].is_string() && i["style"].is_object());
    assert_eq!((i["angle"].clone(), i["locked"].clone(), i["group_ids"].clone(), i["frame"].clone()), (json!(0.0), json!(false), json!([]), Value::Null));
    assert!(i.get("id").is_none(), "only the asked-for fields");
    let v = run(&mut w.s, "element.query", json!({"filter": {"ids": ["@login_box"]}, "fields": ["bound_text", "frame"]}));
    assert_eq!(v["items"][0]["bound_text"], w.label.as_str());
    let v = run(&mut w.s, "element.query", json!({"filter": {"ids": [w.kid.as_str()]}, "fields": ["frame"]}));
    assert_eq!(v["items"][0]["frame"], w.frame.as_str());
}

#[test]
fn query_refuses_a_malformed_query_with_bad_args() {
    let mut w = world();
    let r = refuse_clean(&mut w.s, "element.query", json!({"limit": 0}));
    assert_eq!(r.code, "bad_args");
    assert_eq!(
        r.message,
        "Invalid arguments for `element.query`: `limit`: must be a whole number from 1 to 1000. See `engine.describe` for the schema."
    );
    assert_eq!(refuse_clean(&mut w.s, "element.query", json!({"limit": 1001})).code, "bad_args");
    assert_eq!(refuse_clean(&mut w.s, "element.query", json!({"limit": -1})).code, "bad_args");
    assert_eq!(refuse_clean(&mut w.s, "element.query", json!({"offset": -1})).code, "bad_args");
    let r = refuse_clean(&mut w.s, "element.query", json!({"fields": []}));
    assert!(r.message.contains("`fields`: must list 1 to 32 fields"), "{}", r.message);
    let r = refuse_clean(&mut w.s, "element.query", json!({"fields": ["colour"]}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("fields"), "{}", r.message);
    let r = refuse_clean(&mut w.s, "element.query", json!({"filter": {"colour": "red"}}));
    assert!(r.message.contains("unknown key `colour`") || r.message.contains("colour"), "{}", r.message);
    let r = refuse_clean(&mut w.s, "element.query", json!({"filter": {"kind": "blob"}}));
    assert_eq!(r.code, "bad_args");
    let r = refuse_clean(&mut w.s, "element.query", json!({"filter": {"text": {"regex": "(("}}}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`filter`: the text regex `((` is not valid"), "{}", r.message);
    let r = refuse_clean(&mut w.s, "element.query", json!({"filter": {"ids": ["../etc"]}}));
    assert!(r.message.contains("is not an element id or an @name"), "{}", r.message);
    let r = refuse_clean(&mut w.s, "element.query", json!({"filter": {"region": {"rect": {"x": 1.7e308, "y": 0, "w": 1.7e308, "h": 1}}}}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`filter`: the region must have finite numbers"), "{}", r.message);
    let r = refuse_clean(&mut w.s, "element.query", json!({"order": "random"}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn query_refuses_unknown_references_with_not_found() {
    let mut w = world();
    let r = refuse_clean(&mut w.s, "element.query", json!({"filter": {"ids": ["nope"]}}));
    assert_eq!(r.code, "not_found");
    assert_eq!(r.message, "No element matches `nope`. Look up ids with `element.query` or `doc.outline`.");
    let r = refuse_clean(&mut w.s, "element.query", json!({"filter": {"connected_to": {"ref": "@ghost"}}}));
    assert_eq!(r.message, "No element matches `@ghost`. Look up ids with `element.query` or `doc.outline`.");
    assert_eq!(refuse_clean(&mut w.s, "element.query", json!({"filter": {"frame": "gone"}})).code, "not_found");
    assert_eq!(refuse_clean(&mut w.s, "element.query", json!({"filter": {"bound_text_of": "gone"}})).code, "not_found");
}

#[test]
fn a_long_hostile_pattern_is_cut_in_the_message() {
    let mut w = world();
    let pattern = format!("({}", "a".repeat(3000));
    let r = refuse_clean(&mut w.s, "element.query", json!({"filter": {"text": {"regex": pattern}}}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.chars().count() < 600, "{}", r.message.chars().count());
}

// ---- element.at ----

fn at(s: &mut Session, args: Value) -> Value {
    run(s, "element.at", args)
}

fn hit_ids(v: &Value) -> Vec<String> {
    v["hits"].as_array().unwrap().iter().map(|h| h["id"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn at_returns_the_topmost_hit_with_its_description() {
    let mut w = world();
    let v = at(&mut w.s, json!({"point": [150, 60]}));
    assert_eq!(v["count"], 1);
    assert_eq!(hit_ids(&v), ids(&[&w.login]));
    assert_eq!(v["hits"][0]["kind"], "rectangle");
    assert_eq!(v["hits"][0]["name"], "login_box");
    assert!(v["hits"][0].get("group").is_none());
    let v = at(&mut w.s, json!({"point": [1000, 1000]}));
    assert_eq!((v["count"].clone(), v["hits"].clone()), (json!(0), json!([])));
}

#[test]
fn at_lists_every_hit_top_to_bottom_with_all() {
    let mut w = world();
    let top = filled(&mut w.s, ElementKind::Rectangle, 0.0, 0.0, 200.0, 100.0);
    let v = at(&mut w.s, json!({"point": [150, 60], "all": true}));
    assert_eq!(hit_ids(&v), ids(&[&top, &w.login]));
    let v = at(&mut w.s, json!({"point": [150, 60]}));
    assert_eq!(hit_ids(&v), ids(&[&top]));
}

#[test]
fn at_reports_a_group_as_one_unit_unless_deep() {
    let mut w = world();
    let v = at(&mut w.s, json!({"point": [25, 425]}));
    assert_eq!(v["hits"][0]["id"], w.ga.as_str());
    assert_eq!(v["hits"][0]["group"], "G");
    assert_eq!(v["hits"][0]["members"], json!(ids(&[&w.ga, &w.gb])));
    let v = at(&mut w.s, json!({"point": [25, 425], "deep": true}));
    assert!(v["hits"][0].get("group").is_none() && v["hits"][0].get("members").is_none());
}

#[test]
fn at_skips_locked_elements_hits_labels_as_their_container_and_frames_on_the_outline_only() {
    let mut w = world();
    let p = json!([325, 425]);
    assert_eq!(at(&mut w.s, json!({"point": p}))["count"], 0);
    assert_eq!(hit_ids(&at(&mut w.s, json!({"point": p, "include_locked": true}))), ids(&[&w.locked]));
    assert_eq!(hit_ids(&at(&mut w.s, json!({"point": [40, 20]}))), ids(&[&w.login]), "on the label: its container");
    assert_eq!(at(&mut w.s, json!({"point": [200, 800]}))["count"], 0, "inside a frame");
    assert_eq!(hit_ids(&at(&mut w.s, json!({"point": [150, 600]}))), ids(&[&w.frame]), "on the frame outline");
    assert_eq!(hit_ids(&at(&mut w.s, json!({"point": [45, 645]}))), ids(&[&w.kid]), "a child of a frame is hit as itself");
}

#[test]
fn at_converts_the_tolerance_from_screen_pixels_with_the_zoom() {
    let mut w = world();
    // 10 world units right of the db box: 16.8 px at the base zoom.
    let p = json!([410, 50]);
    assert_eq!(at(&mut w.s, json!({"point": p}))["count"], 0);
    assert_eq!(at(&mut w.s, json!({"point": p, "tolerance": 20}))["count"], 1);
    w.s.tab_mut().view.zoom = 0.5;
    assert_eq!(at(&mut w.s, json!({"point": p}))["count"], 1, "6 px are 12 world units at zoom 0.5");
    assert_eq!(at(&mut w.s, json!({"point": p, "tolerance": 0}))["count"], 0);
}

#[test]
fn at_refuses_bad_arguments() {
    let mut w = world();
    for (args, key) in [
        (json!({"point": [1e308, 0]}), "point[0]"),
        (json!({"point": [0, -1e308]}), "point[1]"),
        (json!({"point": [0, 0], "tolerance": -1}), "tolerance"),
        (json!({"point": [0, 0], "tolerance": 5000}), "tolerance"),
    ] {
        let r = refuse_clean(&mut w.s, "element.at", args);
        assert_eq!(r.code, "bad_args");
        assert!(r.message.contains(&format!("`{key}`")), "{}", r.message);
    }
    for args in [json!({}), json!({"point": [1]}), json!({"point": "0,0"}), json!({"point": [0, 0], "extra": 1})] {
        assert_eq!(refuse_clean(&mut w.s, "element.at", args).code, "bad_args");
    }
}
