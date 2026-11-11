//! `element.query`, `element.at` and the outline text helpers on a small diagram.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use drafftink_edit::TargetError;
use drafftink_edit::layout::{CreateOpts, DiagramResult, DiagramSpec, diagram_create};
use drafftink_edit::query::{
    AtOpts, Field, Order, Query, QueryCtx, QueryError, QueryOpts, describe_all, elements_at, resolve_ref, run_query,
};
use drafftink_edit::text_bind::MonospaceMeasure;
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Element, ElementId, ElementKind, Kind, MemoryStore, SceneRead, SceneStore, SceneTx, SeqIds};

fn q(json: &str) -> Query {
    serde_json::from_str(json).unwrap()
}

fn scene() -> (MemoryStore, DiagramResult) {
    let spec: DiagramSpec = serde_json::from_str(
        r##"{
        "nodes": [
            {"id": "start", "label": "Start", "shape": "ellipse", "name": "start", "style": {"fill_color": "#ff0000"}},
            {"id": "check", "label": "Is it valid?", "shape": "diamond"},
            {"id": "ok", "label": "Done", "name": "done"},
            {"id": "bad", "label": "Fix the input", "style": {"stroke_width": 4}}
        ],
        "edges": [
            {"from": "start", "to": "check"},
            {"from": "check", "to": "ok", "label": "yes"},
            {"from": "check", "to": "bad", "label": "no"},
            {"from": "bad", "to": "check"}
        ],
        "groups": [{"id": "g", "members": ["ok", "bad"]}]
    }"##,
    )
    .unwrap();
    let mut store = MemoryStore::new();
    let mut ids = SeqIds::new();
    let mut tx = store.begin();
    let r = diagram_create(&mut tx, &MonospaceMeasure::default(), &mut ids, &spec, &CreateOpts::default()).unwrap();
    tx.commit().unwrap();
    (store, r)
}

fn run(store: &MemoryStore, json: &str) -> Vec<ElementId> {
    run_query(store, &q(json), &QueryCtx::default(), &QueryOpts { limit: 1000, ..QueryOpts::default() }).unwrap().ids
}

#[test]
fn empty_query_returns_everything_except_bound_text() {
    let (s, r) = scene();
    let all = run(&s, "{}");
    // 4 nodes + 4 arrows; the 6 labels follow their containers.
    assert_eq!(all.len(), 8);
    assert!(all.iter().all(|id| s.get(id).unwrap().container_id().is_none()));
    assert!(all.contains(&r.nodes["start"]));
}

#[test]
fn kind_filter_takes_one_or_many() {
    let (s, r) = scene();
    assert_eq!(run(&s, r#"{"kind":"diamond"}"#), vec![r.nodes["check"].clone()]);
    assert_eq!(run(&s, r#"{"kind":["ellipse","diamond"]}"#).len(), 2);
    assert_eq!(run(&s, r#"{"kind":"arrow"}"#).len(), 4);
    // Asking for text includes the bound labels.
    assert_eq!(run(&s, r#"{"kind":"text"}"#).len(), 6);
}

#[test]
fn text_matches_labels_case_insensitively_and_by_regex() {
    let (s, r) = scene();
    assert_eq!(run(&s, r#"{"text":"VALID"}"#), vec![r.nodes["check"].clone()]);
    assert_eq!(run(&s, r#"{"text":{"contains":"fix"}}"#), vec![r.nodes["bad"].clone()]);
    let hits = run(&s, r#"{"text":{"regex":"^(yes|no)$"}}"#);
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|id| s.get(id).unwrap().tag() == Kind::Arrow));
    // Both conditions must hold.
    assert!(run(&s, r#"{"text":{"contains":"yes","regex":"^no$"}}"#).is_empty());
    let err = run_query(&s, &q(r#"{"text":{"regex":"("}}"#), &QueryCtx::default(), &QueryOpts::default()).unwrap_err();
    assert!(matches!(err, QueryError::BadRegex { .. }));
}

#[test]
fn name_is_a_glob() {
    let (s, r) = scene();
    assert_eq!(run(&s, r#"{"name":"start"}"#), vec![r.nodes["start"].clone()]);
    assert_eq!(run(&s, r#"{"name":"*"}"#).len(), 2);
    assert_eq!(run(&s, r#"{"name":"d?ne"}"#), vec![r.nodes["ok"].clone()]);
    assert!(run(&s, r#"{"name":"Start"}"#).is_empty(), "names are case-sensitive");
}

#[test]
fn style_filter_compares_exact_values() {
    let (s, r) = scene();
    assert_eq!(run(&s, r##"{"style":{"fill_color":"#ff0000"}}"##), vec![r.nodes["start"].clone()]);
    assert_eq!(run(&s, r#"{"style":{"stroke_width":4}}"#), vec![r.nodes["bad"].clone()]);
    // `transparent` means no fill; the red node and nothing else is excluded.
    let unfilled = run(&s, r#"{"kind":"rectangle","style":{"fill_color":"transparent"}}"#);
    assert_eq!(unfilled.len(), 2);
    assert!(run(&s, r#"{"style":{"font_size":20}}"#).is_empty(), "font props only match text");
    assert_eq!(run(&s, r#"{"kind":"text","style":{"font_size":20}}"#).len(), 6);
}

#[test]
fn region_modes() {
    let (s, r) = scene();
    let b = s.get(&r.nodes["start"]).unwrap().rect();
    let around = format!(
        r#"{{"region":{{"rect":{{"x":{},"y":{},"w":{},"h":{}}},"mode":"contain"}}}}"#,
        b.x0 - 1.0,
        b.y0 - 1.0,
        b.width() + 2.0,
        b.height() + 2.0
    );
    assert_eq!(run(&s, &around), vec![r.nodes["start"].clone()]);
    let inside = format!(
        r#"{{"region":{{"rect":{{"x":{},"y":{},"w":2,"h":2}},"mode":"contained_by"}}}}"#,
        b.x0 + 5.0,
        b.y0 + 5.0
    );
    assert_eq!(run(&s, &inside), vec![r.nodes["start"].clone()]);
    let far = run(&s, r#"{"region":{"rect":{"x":5000,"y":5000,"w":10,"h":10}}}"#);
    assert!(far.is_empty());
    let everything = run(&s, r#"{"region":{"rect":{"x":-1000,"y":-1000,"w":9000,"h":9000},"mode":"overlap"}}"#);
    assert_eq!(everything.len(), 8);
    // A negative width is normalised.
    assert!(!run(&s, r#"{"region":{"rect":{"x":9000,"y":9000,"w":-20000,"h":-20000}}}"#).is_empty());
}

#[test]
fn connected_to_finds_arrows_and_neighbours() {
    let (s, r) = scene();
    let check = &r.nodes["check"];
    let out = run(&s, &format!(r#"{{"connected_to":{{"ref":"{check}","direction":"out"}}}}"#));
    // Two arrows leave `check`, and the nodes they reach.
    assert!(out.contains(&r.nodes["ok"]) && out.contains(&r.nodes["bad"]));
    assert!(out.contains(&r.edges[1]) && out.contains(&r.edges[2]));
    assert!(!out.contains(&r.nodes["start"]) && !out.contains(check));
    let inn = run(&s, &format!(r#"{{"connected_to":{{"ref":"{check}","direction":"in"}}}}"#));
    assert!(inn.contains(&r.nodes["start"]) && inn.contains(&r.nodes["bad"]));
    assert!(!inn.contains(&r.nodes["ok"]));
    let any = run(&s, &format!(r#"{{"connected_to":{{"ref":"{check}"}},"kind":["rectangle","ellipse"]}}"#));
    assert_eq!(any.len(), 3);
    let err = run_query(&s, &q(r#"{"connected_to":{"ref":"nope"}}"#), &QueryCtx::default(), &QueryOpts::default())
        .unwrap_err();
    assert_eq!(err, QueryError::Target(TargetError::NotFound(ElementId::new("nope").unwrap())));
}

#[test]
fn group_frame_label_and_bound_text_filters() {
    let (s, r) = scene();
    let g = &r.groups["g"];
    let members = run(&s, &format!(r#"{{"group":"{g}"}}"#));
    assert_eq!(members.len(), 2);
    assert_eq!(run(&s, r#"{"has_label":true,"kind":"rectangle"}"#).len(), 2);
    assert_eq!(run(&s, r#"{"has_label":true,"kind":"arrow"}"#).len(), 2);
    assert_eq!(run(&s, r#"{"has_label":false}"#).len(), 2);
    let check = &r.nodes["check"];
    let label = run(&s, &format!(r#"{{"bound_text_of":"{check}"}}"#));
    assert_eq!(label.len(), 1);
    assert_eq!(s.get(&label[0]).unwrap().text_data().unwrap().content, "Is it valid?");
    assert!(run(&s, r#"{"frame":"nofr"}"#.replace("nofr", r.nodes["ok"].as_str()).as_str()).is_empty());
}

#[test]
fn locked_selected_and_ids() {
    let (mut s, r) = scene();
    let mut tx = s.begin();
    tx.update(&r.nodes["ok"], |e| e.locked = true).unwrap();
    tx.commit().unwrap();
    assert_eq!(run(&s, r#"{"locked":true}"#), vec![r.nodes["ok"].clone()]);
    let sel: BTreeSet<ElementId> = [r.nodes["start"].clone()].into();
    let ctx = QueryCtx { selected: Some(&sel) };
    let got = run_query(&s, &q(r#"{"selected":true}"#), &ctx, &QueryOpts::default()).unwrap();
    assert_eq!(got.ids, vec![r.nodes["start"].clone()]);
    let none = run_query(&s, &q(r#"{"selected":true}"#), &QueryCtx::default(), &QueryOpts::default()).unwrap();
    assert!(none.ids.is_empty());
    let ids = format!(r#"{{"ids":["{}","{}"]}}"#, r.nodes["start"], r.nodes["bad"]);
    assert_eq!(run(&s, &ids).len(), 2);
}

#[test]
fn ordering_and_paging() {
    let (s, r) = scene();
    let reading = run_query(
        &s,
        &q(r#"{"kind":["rectangle","ellipse","diamond"]}"#),
        &QueryCtx::default(),
        &QueryOpts { order: Order::Reading, ..QueryOpts::default() },
    )
    .unwrap();
    // Top to bottom: start, check, then the two rectangles.
    assert_eq!(reading.ids[0], r.nodes["start"]);
    assert_eq!(reading.ids[1], r.nodes["check"]);
    let page =
        run_query(&s, &q("{}"), &QueryCtx::default(), &QueryOpts { limit: 3, offset: 2, order: Order::Z }).unwrap();
    assert_eq!((page.total, page.ids.len(), page.truncated), (8, 3, true));
    let last =
        run_query(&s, &q("{}"), &QueryCtx::default(), &QueryOpts { limit: 3, offset: 6, order: Order::Z }).unwrap();
    assert_eq!((last.ids.len(), last.truncated), (2, false));
    for bad in [0, 1001] {
        let err = run_query(&s, &q("{}"), &QueryCtx::default(), &QueryOpts { limit: bad, ..QueryOpts::default() })
            .unwrap_err();
        assert_eq!(err, QueryError::BadLimit);
    }
}

#[test]
fn unknown_query_fields_are_rejected() {
    assert!(serde_json::from_str::<Query>(r#"{"colour":"red"}"#).is_err());
    assert!(
        serde_json::from_str::<Query>(r#"{"region":{"rect":{"x":0,"y":0,"w":1,"h":1},"mode":"sideways"}}"#).is_err()
    );
}

#[test]
fn describe_returns_the_requested_fields() {
    let (s, r) = scene();
    let v = describe_all(
        &s,
        &[r.nodes["check"].clone(), ElementId::new("zzz").unwrap()],
        &[Field::Id, Field::Kind, Field::Text, Field::Bounds, Field::BoundText, Field::Z],
    );
    assert_eq!(v[0]["kind"], "diamond");
    assert_eq!(v[0]["text"], "Is it valid?");
    assert!(v[0]["bounds"]["w"].as_f64().unwrap() > 80.0);
    assert!(v[0]["bound_text"].is_string());
    assert!(v[0]["z"].is_string());
    assert!(v[1].is_null());
    let arrow = describe_all(
        &s,
        &[r.edges[0].clone()],
        &[Field::Bindings, Field::Frame, Field::GroupIds, Field::Locked, Field::Angle, Field::Name, Field::Style],
    );
    assert_eq!(arrow[0]["bindings"]["start"], r.nodes["start"].as_str());
    assert_eq!(arrow[0]["bindings"]["end"], r.nodes["check"].as_str());
    assert!(arrow[0]["style"]["stroke_width"].is_number());
}

#[test]
fn resolve_ref_takes_ids_and_names() {
    let (s, r) = scene();
    assert_eq!(resolve_ref(&s, "@start").unwrap(), r.nodes["start"]);
    assert_eq!(resolve_ref(&s, r.nodes["bad"].as_str()).unwrap(), r.nodes["bad"]);
    assert_eq!(resolve_ref(&s, "@nobody").unwrap_err(), QueryError::UnknownName("nobody".into()));
    assert!(matches!(resolve_ref(&s, "missing"), Err(QueryError::Target(TargetError::NotFound(_)))));
    assert!(matches!(resolve_ref(&s, "not an id!"), Err(QueryError::BadRef(_))));
}

#[test]
fn element_at_hits_from_the_front() {
    let (s, r) = scene();
    let start = s.get(&r.nodes["start"]).unwrap().rect();
    // The ellipse has a fill, so its inside hits; its label hit counts as the ellipse.
    let hits = elements_at(&s, start.center(), &AtOpts::default());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, r.nodes["start"]);
    // Far away: nothing.
    assert!(elements_at(&s, Point::new(-5000.0, -5000.0), &AtOpts::default()).is_empty());
    // The inside of an unfilled rectangle is empty space, but its outline hits.
    let ok = s.get(&r.nodes["ok"]).unwrap().rect();
    let edge = Point::new(ok.x0, ok.center().y);
    assert_eq!(elements_at(&s, edge, &AtOpts { tolerance: 3.0, ..AtOpts::default() })[0].id, r.nodes["ok"]);
    let corner_inside = Point::new(ok.x0 + 8.0, ok.y0 + 8.0);
    assert!(elements_at(&s, corner_inside, &AtOpts { tolerance: 2.0, ..AtOpts::default() }).is_empty());
    // Group members hit as the group unless `deep`.
    let hit = &elements_at(&s, edge, &AtOpts { tolerance: 3.0, ..AtOpts::default() })[0];
    assert_eq!(hit.group.as_ref(), Some(&r.groups["g"]));
    let deep = &elements_at(&s, edge, &AtOpts { tolerance: 3.0, deep: true, ..AtOpts::default() })[0];
    assert_eq!(deep.group, None);
    // A point on an arrow hits the arrow.
    let arrow = s.get(&r.edges[0]).unwrap();
    let d = arrow.linear_data().unwrap();
    let mid = arrow.transform.local_to_world(d.points[0].midpoint(d.points[1]));
    assert_eq!(elements_at(&s, mid, &AtOpts::default())[0].id, r.edges[0]);
}

#[test]
fn element_at_all_orders_top_down_and_skips_locked() {
    let mut s = MemoryStore::new();
    let mut tx = s.begin();
    for (i, id) in ["back", "front"].into_iter().enumerate() {
        let z = tx.next_top_index().unwrap();
        let mut e = Element::new(ElementId::new(id).unwrap(), ElementKind::Rectangle, z).with_bounds(
            i as f64 * 10.0,
            0.0,
            100.0,
            100.0,
        );
        e.style.fill_color = Some(drafftink_scene::Color::rgb(1, 2, 3));
        tx.insert(e).unwrap();
    }
    tx.commit().unwrap();
    let p = Point::new(50.0, 50.0);
    let all = elements_at(&s, p, &AtOpts { all: true, ..AtOpts::default() });
    assert_eq!(all.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["front", "back"]);
    assert_eq!(elements_at(&s, p, &AtOpts::default()).len(), 1);
    let mut tx = s.begin();
    tx.update(&ElementId::new("front").unwrap(), |e| e.locked = true).unwrap();
    tx.commit().unwrap();
    assert_eq!(elements_at(&s, p, &AtOpts::default())[0].id.as_str(), "back");
    let with_locked = elements_at(&s, p, &AtOpts { include_locked: true, ..AtOpts::default() });
    assert_eq!(with_locked[0].id.as_str(), "front");
    assert!(elements_at(&s, Point::new(f64::NAN, 0.0), &AtOpts::default()).is_empty());
}
