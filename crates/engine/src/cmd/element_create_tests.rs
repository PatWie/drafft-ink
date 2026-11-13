//! Tests of `element.create`, `element.connect`, `element.relabel` and `element.set_name`. Pattern per command:
//! availability, exact refusal text, the fields of the created element, selection and tool effects, undo, and a
//! negative case that leaves the fingerprint unchanged. No UI, no DOM: a headless session and the fake ports.

use drafftink_edit::binding::stale_arrows;
use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    BindMode, Brush, Element, ElementId, ElementKind, Head, Kind, PathType, PressureMode, SceneRead, SceneTx,
    StoreError,
};
use serde_json::{Value, json};

use crate::batch::{BatchOpts, BatchStep};
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::state::Tool;
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

/// Refuses and checks that the fingerprint did not change.
fn refuse_clean(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = refuse(s, id, args);
    assert_eq!(s.fingerprint(), before, "{id} left a trace: {}", r.message);
    r
}

fn create(s: &mut Session, args: Value) -> Value {
    run(s, "element.create", args)
}

fn eid(s: &str) -> ElementId {
    ElementId::new(s).expect("a valid id")
}

fn el(s: &Session, id: &str) -> Element {
    s.element(&eid(id)).unwrap_or_else(|| panic!("no live element {id}")).clone()
}

fn new_id(v: &Value) -> String {
    v["id"].as_str().expect("the output has an id").to_owned()
}

/// Creates an element and returns it as stored.
fn made(s: &mut Session, args: Value) -> Element {
    let out = create(s, args);
    el(s, &new_id(&out))
}

fn rect_args(x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({"kind": "rectangle", "bounds": {"x": x, "y": y, "w": w, "h": h}})
}

fn live_order(s: &Session) -> Vec<String> {
    s.store().iter().map(|e| e.id.as_str().to_owned()).collect()
}

fn lock(s: &mut Session, id: &str) {
    s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.update(&eid(id), |e| e.locked = true)).expect("lock");
    s.tab_mut().doc.commit_untracked().unwrap();
}

fn pt(e: &Element, i: usize) -> Point {
    let w = drafftink_edit::linear::world_points(e).expect("a line");
    w[i]
}

/// Equal within 0.05: a fixed point of exactly 0.5 is stored as 0.5001, which moves a 100 wide box's centre by 0.01.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

fn selected(s: &Session) -> Vec<String> {
    s.selection().iter().map(|i| i.as_str().to_owned()).collect()
}

// ---------------------------------------------------------------------------------------------
// Availability
// ---------------------------------------------------------------------------------------------

#[test]
fn all_four_are_available_and_refuse_in_view_only_mode_with_the_shared_text() {
    let mut s = testing::headless_session();
    for id in ["element.create", "element.connect", "element.relabel", "element.set_name"] {
        assert!(s.availability(id).is_ok(), "{id}");
    }
    s.view_only = true;
    for id in ["element.create", "element.connect", "element.relabel", "element.set_name"] {
        let r = s.availability(id).unwrap_err();
        assert_eq!(r.code, "view_only", "{id}");
        assert!(r.message.contains("`view.set_view_only`"), "{}", r.message);
        let e = refuse(&mut s, id, json!({}));
        assert_eq!((e.code, e.message), (r.code, r.message), "execute and availability agree for {id}");
    }
}

// ---------------------------------------------------------------------------------------------
// element.create: boxes
// ---------------------------------------------------------------------------------------------

#[test]
fn a_rectangle_has_the_tool_defaults_a_seed_and_a_fresh_top_index() {
    let mut s = testing::headless_session();
    let out = create(&mut s, rect_args(10.0, 20.0, 100.0, 50.0));
    let e = el(&s, &new_id(&out));
    assert_eq!(e.kind, ElementKind::Rectangle);
    assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (10.0, 20.0, 100.0, 50.0));
    assert_eq!((e.transform.angle, e.transform.flip_x, e.transform.flip_y), (0.0, false, false));
    let mut expected = s.tool.style_defaults.clone();
    expected.seed = e.style.seed;
    assert_eq!(e.style, expected);
    assert_ne!(e.style.seed, 0, "the seed comes from the id source");
    assert_eq!((e.locked, e.deleted, e.frame_id, e.name), (false, false, None, None));
    assert_eq!(out["kind"], "rectangle");
    assert_eq!(out["bounds"], json!({"x": 10.0, "y": 20.0, "w": 100.0, "h": 50.0}));
    assert_eq!(out["created"], json!([e.id.as_str()]));
    assert_eq!(out["undo_label"], "Create element");
}

#[test]
fn diamond_ellipse_and_frame_are_boxes_of_their_kind() {
    let mut s = testing::headless_session();
    for (kind, expect) in [
        ("diamond", ElementKind::Diamond),
        ("ellipse", ElementKind::Ellipse),
        ("frame", ElementKind::Frame(drafftink_scene::FrameData { name: None, clip: true })),
    ] {
        let out = create(&mut s, json!({"kind": kind, "bounds": {"x": 5, "y": 6, "w": 70, "h": 40}}));
        let e = el(&s, &new_id(&out));
        assert_eq!(e.kind, expect, "{kind}");
        assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (5.0, 6.0, 70.0, 40.0));
    }
}

#[test]
fn negative_width_and_height_are_normalised() {
    let mut s = testing::headless_session();
    let out = create(&mut s, rect_args(110.0, 70.0, -100.0, -50.0));
    let e = el(&s, &new_id(&out));
    assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (10.0, 20.0, 100.0, 50.0));
}

#[test]
fn the_style_defaults_of_the_tool_are_used_and_the_style_argument_wins() {
    let mut s = testing::headless_session();
    s.tool.style_defaults.stroke_width = 4.0;
    s.tool.style_defaults.fill_color = Some(drafftink_scene::Color::rgb(1, 2, 3));
    let plain = made(&mut s, rect_args(0.0, 0.0, 10.0, 10.0));
    assert_eq!(plain.style.stroke_width, 4.0);
    assert_eq!(plain.style.fill_color, Some(drafftink_scene::Color::rgb(1, 2, 3)));
    let mut args = rect_args(0.0, 0.0, 10.0, 10.0);
    args["style"] = json!({"stroke_color": "#ff0000", "fill_color": null, "stroke_width": 8, "opacity": 0.5,
                           "corner_radius": 12, "sloppiness": "architect", "stroke_style": "dashed", "fill_pattern": "hachure"});
    let styled = made(&mut s, args);
    assert_eq!(styled.style.stroke_color, drafftink_scene::Color::rgb(255, 0, 0));
    assert_eq!(styled.style.fill_color, None);
    assert_eq!(styled.style.stroke_width, 8.0);
    assert_eq!(styled.style.opacity, 0.5);
    assert_eq!(styled.style.corner_radius, 12.0);
    assert_eq!(styled.style.sloppiness, drafftink_scene::Sloppiness::Architect);
    assert_eq!(styled.style.stroke_style, drafftink_scene::StrokeStyle::Dashed);
    assert_eq!(styled.style.fill_pattern, drafftink_scene::FillPattern::Hachure);
    assert_ne!(plain.style.seed, styled.style.seed, "each element gets its own seed");
}

#[test]
fn a_bad_style_is_refused_by_name() {
    let mut s = testing::headless_session();
    let mut args = rect_args(0.0, 0.0, 10.0, 10.0);
    args["style"] = json!({"stroke_width": 0});
    let r = refuse_clean(&mut s, "element.create", args.clone());
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("style.stroke_width"), "{}", r.message);
    args["style"] = json!({"stroke_colour": "#fff"});
    assert_eq!(refuse_clean(&mut s, "element.create", args.clone()).code, "bad_args");
    args["style"] = json!({"stroke_color": "blue"});
    let r = refuse_clean(&mut s, "element.create", args);
    assert!(r.message.contains("is not a colour"), "{}", r.message);
}

#[test]
fn style_properties_that_do_not_apply_are_ignored_with_a_warning() {
    let mut s = testing::headless_session();
    let mut args = rect_args(0.0, 0.0, 10.0, 10.0);
    args["style"] = json!({"font_size": 40, "stroke_width": 6});
    let out = create(&mut s, args);
    assert_eq!(el(&s, &new_id(&out)).style.stroke_width, 6.0);
    let w = out["warnings"].as_array().expect("warnings");
    assert_eq!(w.len(), 1, "{out}");
    assert!(w[0].as_str().unwrap_or_default().contains("font_size"), "{out}");
}

#[test]
fn too_small_names_the_kind_and_the_side() {
    let mut s = testing::headless_session();
    let r = refuse_clean(&mut s, "element.create", rect_args(0.0, 0.0, 0.5, 40.0));
    assert_eq!(r.code, "too_small");
    assert_eq!(r.message, "The rectangle is smaller than 1 unit in width. Give bounds with w and h of at least 1.");
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "ellipse", "bounds": {"x": 0, "y": 0, "w": 40, "h": 0}}));
    assert_eq!(r.message, "The ellipse is smaller than 1 unit in height. Give bounds with w and h of at least 1.");
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "frame", "bounds": {"x": 0, "y": 0, "w": 0.2, "h": 0.2}}));
    assert_eq!(r.message, "The frame is smaller than 1 unit in width and height. Give bounds with w and h of at least 1.");
    assert!(s.element(&eid("e1")).is_none());
    // Exactly 1 is allowed, and so is a negative size that normalises to at least 1.
    create(&mut s, rect_args(0.0, 0.0, 1.0, 1.0));
    create(&mut s, rect_args(0.0, 0.0, -1.0, -3.0));
}

#[test]
fn a_line_needs_one_unit_in_either_direction() {
    let mut s = testing::headless_session();
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "line", "points": [[5, 5], [5.4, 5.4]]}));
    assert_eq!(r.code, "too_small");
    assert_eq!(r.message, "The line is smaller than 1 unit in width and height. Give points that are at least 1 unit apart.");
    // A horizontal line has no height and is fine, as in the old app.
    create(&mut s, json!({"kind": "line", "points": [[0, 5], [100, 5]]}));
}

#[test]
fn an_image_is_refused_with_the_exact_text() {
    let mut s = testing::headless_session();
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "image"}));
    assert_eq!(r.code, "wrong_kind");
    assert_eq!(r.message, "`element.create` cannot create images. Use `image.insert`.");
}

#[test]
fn fields_that_do_not_belong_to_the_kind_or_are_missing_are_bad_args() {
    let mut s = testing::headless_session();
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "rectangle", "points": [[0, 0], [1, 1]], "bounds": {"x":0,"y":0,"w":5,"h":5}}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`points`") && r.message.contains("does not apply to a rectangle"), "{}", r.message);
    assert!(r.message.contains("line, arrow and freehand"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "rectangle"}));
    assert!(r.message.contains("`bounds`: is required for a rectangle"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "arrow", "bounds": {"x":0,"y":0,"w":5,"h":5}}));
    assert!(r.message.contains("`bounds`"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "text", "text": "hi"}));
    assert!(r.message.contains("`at`"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "math"}));
    assert!(r.message.contains("`at`"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "rectangle", "capture": true, "bounds": {"x":0,"y":0,"w":5,"h":5}}));
    assert!(r.message.contains("`capture`"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "warp"}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn numbers_are_checked() {
    let mut s = testing::headless_session();
    let r = refuse_clean(&mut s, "element.create", rect_args(1e12, 0.0, 10.0, 10.0));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("bounds.x"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "line", "points": [[0, 0], [1, 1e300]]}));
    assert!(r.message.contains("points[1][1]"), "{}", r.message);
}

// ---------------------------------------------------------------------------------------------
// element.create: ids and names
// ---------------------------------------------------------------------------------------------

#[test]
fn a_chosen_id_is_used_and_an_id_in_use_is_refused_even_when_deleted() {
    let mut s = testing::headless_session();
    let mut a = rect_args(0.0, 0.0, 10.0, 10.0);
    a["id"] = json!("mine");
    let out = create(&mut s, a.clone());
    assert_eq!(out["id"], "mine");
    let r = refuse_clean(&mut s, "element.create", a.clone());
    assert_eq!(r.code, "id_in_use");
    assert_eq!(r.message, "The id `mine` is already used by another element. Omit `id` to get a fresh one.");
    s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.update(&eid("mine"), |e| e.deleted = true)).unwrap();
    assert_eq!(refuse_clean(&mut s, "element.create", a.clone()).code, "id_in_use", "a tombstone still owns its id");
    a["id"] = json!("not valid!");
    let r = refuse_clean(&mut s, "element.create", a);
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`id`"), "{}", r.message);
}

#[test]
fn a_name_is_stored_and_unique_and_checked() {
    let mut s = testing::headless_session();
    let mut a = rect_args(0.0, 0.0, 10.0, 10.0);
    a["name"] = json!("box");
    let first = new_id(&create(&mut s, a.clone()));
    assert_eq!(el(&s, &first).name.as_deref(), Some("box"));
    let r = refuse_clean(&mut s, "element.create", a.clone());
    assert_eq!(r.code, "name_taken");
    assert_eq!(
        r.message,
        format!("The name `box` is already used by element `{first}`. Names must be unique so that `@box` is unambiguous. Pick another name, or clear the other with `element.set_name`.")
    );
    for bad in ["", "@x", "$x", "a\nb", &"x".repeat(65)] {
        a["name"] = json!(bad);
        let r = refuse_clean(&mut s, "element.create", a.clone());
        assert_eq!(r.code, "bad_name", "{bad:?}");
        assert_eq!(
            r.message,
            "A name must be 1 to 64 characters, must not start with `@` or `$`, and must not contain control characters."
        );
    }
}

// ---------------------------------------------------------------------------------------------
// element.create: selection, tool, undo
// ---------------------------------------------------------------------------------------------

#[test]
fn the_new_element_is_selected_unless_told_otherwise() {
    let (mut s, ids) = testing::session_with_shapes().unwrap();
    assert_eq!(selected(&s), vec![ids[0].as_str().to_owned()]);
    let out = create(&mut s, rect_args(0.0, 0.0, 10.0, 10.0));
    assert_eq!(selected(&s), vec![new_id(&out)]);
    let mut keep = rect_args(0.0, 0.0, 10.0, 10.0);
    keep["select"] = json!(false);
    create(&mut s, keep);
    assert_eq!(selected(&s), vec![new_id(&out)], "select=false leaves the selection alone");
}

#[test]
fn from_tool_returns_to_select_unless_the_tool_is_locked_and_stays_one_undo_step() {
    let mut s = testing::headless_session();
    s.tool.active = Tool::Rectangle;
    let mut a = rect_args(0.0, 0.0, 10.0, 10.0);
    a["from_tool"] = json!(true);
    create(&mut s, a.clone());
    assert_eq!(s.tool.active, Tool::Select);
    assert_eq!(s.tab().doc.undo_depth(), 1);

    s.tool.active = Tool::Rectangle;
    s.tool.lock = true;
    create(&mut s, a.clone());
    assert_eq!(s.tool.active, Tool::Rectangle, "tool lock keeps the drawing tool");

    s.tool.lock = false;
    create(&mut s, rect_args(0.0, 0.0, 10.0, 10.0));
    assert_eq!(s.tool.active, Tool::Rectangle, "without from_tool the tool is not touched");
}

#[test]
fn a_refused_create_leaves_the_tool_alone() {
    let mut s = testing::headless_session();
    s.tool.active = Tool::Rectangle;
    let mut a = rect_args(0.0, 0.0, 0.1, 10.0);
    a["from_tool"] = json!(true);
    refuse_clean(&mut s, "element.create", a);
    assert_eq!(s.tool.active, Tool::Rectangle);
}

#[test]
fn undo_takes_the_element_and_the_selection_back_and_redo_restores_both() {
    let (mut s, ids) = testing::session_with_shapes().unwrap();
    let out = create(&mut s, rect_args(5.0, 5.0, 30.0, 30.0));
    let id = new_id(&out);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid(&id)).is_none());
    assert_eq!(selected(&s), vec![ids[0].as_str().to_owned()]);
    s.redo_steps(1).unwrap();
    assert_eq!(el(&s, &id).transform.w, 30.0);
    assert_eq!(selected(&s), vec![id]);
}

// ---------------------------------------------------------------------------------------------
// element.create: lines and arrows
// ---------------------------------------------------------------------------------------------

#[test]
fn a_line_keeps_its_points_has_no_heads_and_a_point_box() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "line", "points": [[10, 20], [110, 70], [60, 90]]}));
    let ElementKind::Line(d) = &e.kind else { panic!("a line") };
    assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (10.0, 20.0, 100.0, 70.0));
    assert_eq!(d.points, vec![Point::new(0.0, 0.0), Point::new(100.0, 50.0), Point::new(50.0, 70.0)]);
    assert_eq!((d.path, d.start_head, d.end_head, d.closed), (PathType::Sharp, Head::None, Head::None, false));
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
}

#[test]
fn an_arrow_has_the_old_defaults_no_start_head_and_an_arrow_end_head() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "arrow", "points": [[0, 0], [100, 0]]}));
    let d = e.linear_data().unwrap();
    assert_eq!((d.start_head, d.end_head, d.path), (Head::None, Head::Arrow, PathType::Sharp));
    assert!(d.head_size.is_none());
    // The heads can be chosen; null and "none" both mean no head.
    let e = made(&mut s, json!({"kind": "arrow", "points": [[0, 0], [100, 0]], "start_head": "circle", "end_head": null}));
    let d = e.linear_data().unwrap();
    assert_eq!((d.start_head, d.end_head), (Head::Circle, Head::None));
    let e = made(&mut s, json!({"kind": "line", "points": [[0, 0], [100, 0]], "end_head": "triangle", "start_head": "none"}));
    assert_eq!((e.linear_data().unwrap().start_head, e.linear_data().unwrap().end_head), (Head::None, Head::Triangle));
}

#[test]
fn arrow_types_closed_lines_and_the_point_count_rule() {
    let mut s = testing::headless_session();
    let round = made(&mut s, json!({"kind": "arrow", "points": [[0, 0], [50, 40], [100, 0]], "arrow_type": "round"}));
    let d = round.linear_data().unwrap();
    assert_eq!((d.path, d.points.len()), (PathType::Round, 3));
    let elbow = made(&mut s, json!({"kind": "arrow", "points": [[0, 0], [100, 80]], "arrow_type": "elbow"}));
    let d = elbow.linear_data().unwrap();
    assert_eq!(d.path, PathType::Elbow);
    assert!(d.points.len() >= 3, "an elbow between two free ends has a corner: {:?}", d.points);
    assert_eq!((pt(&elbow, 0), pt(&elbow, d.points.len() - 1)), (Point::new(0.0, 0.0), Point::new(100.0, 80.0)));
    let closed = made(&mut s, json!({"kind": "line", "points": [[0, 0], [100, 0], [50, 80]], "closed": true}));
    assert!(closed.linear_data().unwrap().closed);

    let r = refuse_clean(&mut s, "element.create", json!({"kind": "line", "points": [[0, 0]]}));
    assert_eq!(r.code, "too_few");
    assert_eq!(r.message, "`element.create` needs at least 2 points, and 1 were given. Select more, or pass `ids`.");
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "arrow", "points": []}));
    assert_eq!(r.code, "too_few");
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "line", "points": [[0, 0], [10, 10]], "closed": true}));
    assert_eq!(r.message, "`element.create` needs at least 3 points, and 2 were given. Select more, or pass `ids`.");
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "line", "points": [[0, 0], [10, 10], [0, 9]], "closed": true, "arrow_type": "elbow"}));
    assert_eq!(r.code, "bad_args");
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "arrow", "points": [[0, 0], [10, 10]], "closed": true}));
    assert!(r.message.contains("`closed`"), "{}", r.message);
}

// ---------------------------------------------------------------------------------------------
// element.create: bindings
// ---------------------------------------------------------------------------------------------

fn two_boxes(s: &mut Session) -> (ElementId, ElementId) {
    let a = testing::add_rect(s, 0.0, 0.0, 100.0, 60.0).unwrap();
    let b = testing::add_rect(s, 300.0, 0.0, 100.0, 60.0).unwrap();
    (a, b)
}

#[test]
fn explicit_start_and_end_bind_in_orbit_mode_and_are_solved() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    let out = create(
        &mut s,
        json!({"kind": "arrow", "points": [[50, 30], [350, 30]], "start": {"target": a.as_str()}, "end": {"target": "e2"}}),
    );
    let e = el(&s, &new_id(&out));
    let d = e.linear_data().unwrap();
    let (sb, eb) = (d.start_binding.as_ref().unwrap(), d.end_binding.as_ref().unwrap());
    assert_eq!((&sb.target, sb.mode), (&a, BindMode::Orbit));
    assert_eq!((&eb.target, eb.mode), (&b, BindMode::Orbit));
    assert_eq!(out["start_target"], a.as_str());
    assert_eq!(out["end_target"], b.as_str());
    // The ends sit on the outlines plus the gap (5 + stroke / 2 = 6).
    assert!(close(pt(&e, 0).x, 106.0) && close(pt(&e, 0).y, 30.0), "{:?}", pt(&e, 0));
    assert!(close(pt(&e, 1).x, 294.0) && close(pt(&e, 1).y, 30.0), "{:?}", pt(&e, 1));
    assert!(stale_arrows(s.store()).is_empty());
}

#[test]
fn an_explicit_inside_binding_keeps_the_end_inside_at_the_fixed_point() {
    let mut s = testing::headless_session();
    let (a, _) = two_boxes(&mut s);
    let out = create(
        &mut s,
        json!({"kind": "arrow", "points": [[10, 10], [350, 30]], "start": {"target": "e1", "mode": "inside", "fixed_point": [0.25, 0.5]}}),
    );
    let e = el(&s, &new_id(&out));
    let sb = e.linear_data().unwrap().start_binding.clone().unwrap();
    assert_eq!((sb.target, sb.mode), (a, BindMode::Inside));
    assert!(close(sb.fixed_point[0], 0.25) && close(sb.fixed_point[1], 0.5), "{:?}", sb.fixed_point);
    assert_eq!(sb.fixed_point[1], 0.5001, "an exact 0.5 is stored as 0.5001 so the side does not flip on noise");
    assert!(close(pt(&e, 0).x, 25.0) && close(pt(&e, 0).y, 30.0), "{:?}", pt(&e, 0));
}

#[test]
fn a_target_that_cannot_be_bound_is_refused_with_the_catalog_text() {
    let mut s = testing::headless_session();
    let line = new_id(&create(&mut s, json!({"kind": "line", "points": [[0, 0], [100, 0]]})));
    let r = refuse_clean(
        &mut s,
        "element.create",
        json!({"kind": "arrow", "points": [[0, 50], [100, 50]], "start": {"target": line}}),
    );
    assert_eq!(r.code, "not_bindable");
    assert_eq!(
        r.message,
        "Line elements cannot be arrow targets. Bindable kinds: rectangle, diamond, ellipse, text, image, frame."
    );
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "arrow", "points": [[0, 0], [9, 9]], "end": {"target": "nope"}}));
    assert_eq!(r.code, "not_found");
}

#[test]
fn an_end_inside_a_shape_binds_inside_and_one_near_its_outline_binds_orbit() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    // Start inside A, end 4 units outside B's left edge.
    let out = create(&mut s, json!({"kind": "arrow", "points": [[50, 30], [296, 30]]}));
    let e = el(&s, &new_id(&out));
    let d = e.linear_data().unwrap();
    let (sb, eb) = (d.start_binding.as_ref().unwrap(), d.end_binding.as_ref().unwrap());
    assert_eq!((&sb.target, sb.mode), (&a, BindMode::Inside));
    assert_eq!((&eb.target, eb.mode), (&b, BindMode::Orbit));
    assert!(close(pt(&e, 0).x, 50.0) && close(pt(&e, 0).y, 30.0), "an inside end stays where it was drawn");
    assert!(stale_arrows(s.store()).is_empty());
}

#[test]
fn a_free_end_far_from_everything_stays_free() {
    let mut s = testing::headless_session();
    two_boxes(&mut s);
    let out = create(&mut s, json!({"kind": "arrow", "points": [[150, 200], [250, 200]]}));
    let e = el(&s, &new_id(&out));
    let d = e.linear_data().unwrap();
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
    assert!(out.get("start_target").is_none() && out.get("end_target").is_none());
}

#[test]
fn bind_off_binds_nothing_automatically_but_still_honours_named_targets() {
    let mut s = testing::headless_session();
    let (a, _) = two_boxes(&mut s);
    let out = create(&mut s, json!({"kind": "arrow", "points": [[50, 30], [350, 30]], "bind": "off"}));
    let d = el(&s, &new_id(&out)).linear_data().unwrap().clone();
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
    let out = create(
        &mut s,
        json!({"kind": "arrow", "points": [[50, 30], [350, 30]], "bind": "off", "start": {"target": a.as_str()}}),
    );
    let d = el(&s, &new_id(&out)).linear_data().unwrap().clone();
    assert!(d.start_binding.is_some() && d.end_binding.is_none());
}

#[test]
fn bind_inside_binds_a_margin_end_inside() {
    let mut s = testing::headless_session();
    let (a, _) = two_boxes(&mut s);
    let out = create(&mut s, json!({"kind": "arrow", "points": [[104, 30], [200, 150]], "bind": "inside"}));
    let sb = el(&s, &new_id(&out)).linear_data().unwrap().start_binding.clone().unwrap();
    assert_eq!((sb.target, sb.mode), (a, BindMode::Inside));
}

#[test]
fn the_setting_arrow_binding_off_turns_automatic_binding_off() {
    let mut s = testing::headless_session();
    two_boxes(&mut s);
    s.settings.set_raw("arrow.binding", json!(false));
    let out = create(&mut s, json!({"kind": "arrow", "points": [[50, 30], [350, 30]]}));
    let d = el(&s, &new_id(&out)).linear_data().unwrap().clone();
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
}

#[test]
fn bind_on_and_inside_bind_with_the_setting_off_and_off_never_binds() {
    let mut s = testing::headless_session();
    two_boxes(&mut s);
    s.settings.set_raw("arrow.binding", json!(false));
    for (policy, bound) in [("on", true), ("inside", true), ("auto", false), ("off", false)] {
        let out = create(&mut s, json!({"kind": "arrow", "points": [[50, 30], [350, 30]], "bind": policy}));
        let d = el(&s, &new_id(&out)).linear_data().unwrap().clone();
        assert_eq!((d.start_binding.is_some(), d.end_binding.is_some()), (bound, bound), "bind {policy} with the setting off");
    }
    s.settings.set_raw("arrow.binding", json!(true));
    for (policy, bound) in [("on", true), ("auto", true), ("off", false)] {
        let out = create(&mut s, json!({"kind": "arrow", "points": [[50, 30], [350, 30]], "bind": policy}));
        let d = el(&s, &new_id(&out)).linear_data().unwrap().clone();
        assert_eq!((d.start_binding.is_some(), d.end_binding.is_some()), (bound, bound), "bind {policy} with the setting on");
    }
}

#[test]
fn an_elbow_arrow_between_two_shapes_binds_and_routes_around_them() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    let out = create(
        &mut s,
        json!({"kind": "arrow", "points": [[50, 30], [350, 30]], "arrow_type": "elbow", "start": {"target": "e1"}, "end": {"target": "e2"}}),
    );
    let e = el(&s, &new_id(&out));
    let d = e.linear_data().unwrap();
    assert_eq!(d.path, PathType::Elbow);
    assert_eq!(d.start_binding.as_ref().map(|x| &x.target), Some(&a));
    assert_eq!(d.end_binding.as_ref().map(|x| &x.target), Some(&b));
    assert!(d.points.len() >= 2);
    assert!(stale_arrows(s.store()).is_empty());
    // Every segment of an elbow is horizontal or vertical.
    for w in drafftink_edit::linear::world_points(&e).unwrap().windows(2) {
        assert!(close(w[0].x, w[1].x) || close(w[0].y, w[1].y), "{w:?}");
    }
}

#[test]
fn a_new_arrow_follows_its_target_when_the_target_moves() {
    let mut s = testing::headless_session();
    let (_, _) = two_boxes(&mut s);
    let out = create(&mut s, json!({"kind": "arrow", "points": [[50, 30], [350, 30]], "start": {"target": "e1"}, "end": {"target": "e2"}}));
    let id = eid(&new_id(&out));
    // Move B down with the edit crate and let the binding module re-solve, as the transform commands do.
    s.tab_mut()
        .doc
        .edit::<(), drafftink_edit::binding::BindingError>(|tx| {
            tx.update(&eid("e2"), |e| e.transform.y += 100.0)?;
            drafftink_edit::binding::update_bound_arrows(tx, &[eid("e2")])?;
            Ok(())
        })
        .unwrap();
    assert!(stale_arrows(s.store()).is_empty());
    assert!(pt(s.element(&id).unwrap(), 1).y > 60.0);
}

// ---------------------------------------------------------------------------------------------
// element.create: freehand
// ---------------------------------------------------------------------------------------------

fn stroke() -> Vec<Value> {
    vec![json!([0, 0]), json!([10, 0.1]), json!([20, 0]), json!([30, 0.2]), json!([40, 0])]
}

#[test]
fn a_freehand_stroke_is_simplified_with_the_old_tolerance_and_keeps_its_pressures() {
    let mut s = testing::headless_session();
    let out = create(
        &mut s,
        json!({"kind": "freehand", "points": stroke(), "pressures": [0.1, 0.2, 0.3, 0.4, 0.5]}),
    );
    let e = el(&s, &new_id(&out));
    let ElementKind::Freehand(d) = &e.kind else { panic!("freehand") };
    assert_eq!(d.points, vec![Point::new(0.0, 0.0), Point::new(40.0, 0.0)]);
    assert_eq!(d.pressures, vec![0.1, 0.5]);
    assert_eq!(d.pressure_mode, PressureMode::Device, "pen data is used");
    assert_eq!((d.brush, d.highlighter, d.closed), (Brush::Normal, false, false));
    assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (0.0, 0.0, 40.0, 0.0));
    assert_eq!(e.style.stroke_width, 2.0);
}

#[test]
fn simplify_zero_keeps_every_point_and_no_pressures_means_uniform() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "freehand", "points": stroke(), "simplify": 0}));
    let ElementKind::Freehand(d) = &e.kind else { panic!("freehand") };
    assert_eq!((d.points.len(), d.pressures.len(), d.pressure_mode), (5, 0, PressureMode::Uniform));
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "freehand", "points": stroke(), "simplify": -1}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn the_style_decides_the_pressure_mode() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "freehand", "points": stroke(), "pressures": [1, 1, 1, 1, 1], "style": {"pressure_mode": "simulated", "freehand_style": "calligraphy"}}));
    let ElementKind::Freehand(d) = &e.kind else { panic!("freehand") };
    assert_eq!((d.pressure_mode, d.brush), (PressureMode::Simulated, Brush::Calligraphy));
}

#[test]
fn a_single_point_is_a_dot_and_no_points_are_too_few() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "freehand", "points": [[5, 6]]}));
    let ElementKind::Freehand(d) = &e.kind else { panic!("freehand") };
    assert_eq!(d.points, vec![Point::new(0.0, 0.0)]);
    assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (5.0, 6.0, 0.0, 0.0));
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "freehand", "points": []}));
    assert_eq!(r.message, "`element.create` needs at least 1 points, and 0 were given. Select more, or pass `ids`.");
}

#[test]
fn pressures_must_match_the_points_and_lie_between_zero_and_one() {
    let mut s = testing::headless_session();
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "freehand", "points": stroke(), "pressures": [0.5]}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`pressures`") && r.message.contains("5 points and 1 pressures"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "freehand", "points": stroke(), "pressures": [0.1, 0.1, 2, 0.1, 0.1]}));
    assert!(r.message.contains("pressures[2]"), "{}", r.message);
}

#[test]
fn a_highlighter_is_wide_and_half_transparent_unless_the_style_says_otherwise() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "freehand", "points": stroke(), "highlighter": true}));
    let ElementKind::Freehand(d) = &e.kind else { panic!("freehand") };
    assert!(d.highlighter);
    assert_eq!(e.style.stroke_width, 12.0);
    assert_eq!(e.style.stroke_color, drafftink_scene::Color::rgba(0x63, 0x66, 0xf1, 128));
    // A wider default stays; a thin one is raised.
    s.tool.style_defaults.stroke_width = 20.0;
    let e = made(&mut s, json!({"kind": "freehand", "points": stroke(), "highlighter": true}));
    assert_eq!(e.style.stroke_width, 20.0, "the width is max(default, 12)");
    // The style wins.
    let e = made(&mut s, json!({"kind": "freehand", "points": stroke(), "highlighter": true, "style": {"stroke_width": 3, "stroke_color": "#ffff00"}}));
    assert_eq!(e.style.stroke_width, 3.0);
    assert_eq!(e.style.stroke_color, drafftink_scene::Color::rgb(255, 255, 0));
}

// ---------------------------------------------------------------------------------------------
// element.create: text and math
// ---------------------------------------------------------------------------------------------

#[test]
fn a_free_text_is_measured_and_uses_the_text_defaults() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "text", "text": "hello", "at": [30, 40]}));
    let ElementKind::Text(t) = &e.kind else { panic!("text") };
    assert_eq!(t.content, "hello");
    assert_eq!(t.font, drafftink_scene::TextStyle::default());
    assert!(t.auto_resize && t.container_id.is_none() && t.runs.is_empty());
    // Monospace: 5 characters of half the 20 pt size, one line of 20 * 1.25.
    assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (30.0, 40.0, 50.0, 25.0));
}

#[test]
fn the_text_font_comes_from_the_style_argument() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "text", "text": "ab", "at": [0, 0],
                   "style": {"font_family": "vanilla_extract", "font_size": 40, "font_weight": "heavy", "text_align": "center", "line_height": 1.5, "stroke_color": "#112233"}}));
    let ElementKind::Text(t) = &e.kind else { panic!("text") };
    assert_eq!(
        (t.font.family, t.font.size, t.font.weight, t.font.align, t.font.line_height),
        (drafftink_scene::FontFamily::VanillaExtract, 40.0, drafftink_scene::FontWeight::Heavy, drafftink_scene::TextAlign::Center, 1.5)
    );
    assert_eq!(e.style.stroke_color, drafftink_scene::Color::rgb(0x11, 0x22, 0x33));
    assert_eq!((e.transform.w, e.transform.h), (40.0, 60.0));
}

#[test]
fn a_fixed_width_wraps_the_text_and_turns_auto_resize_off() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "text", "text": "hello", "at": [0, 0], "width": 40}));
    let ElementKind::Text(t) = &e.kind else { panic!("text") };
    assert!(!t.auto_resize);
    // "hello" is 50 wide and does not fit in 40: it breaks after four characters.
    assert_eq!((e.transform.w, e.transform.h), (40.0, 50.0));
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "text", "text": "x", "at": [0, 0], "width": 0}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn empty_text_is_refused_with_the_exact_text() {
    let mut s = testing::headless_session();
    for text in ["", "   \n "] {
        let r = refuse_clean(&mut s, "element.create", json!({"kind": "text", "text": text, "at": [0, 0]}));
        assert_eq!(r.code, "empty_text");
        assert_eq!(r.message, "A text element cannot be empty. Give `text`, or start typing with `text.edit_begin`.");
    }
}

#[test]
fn text_as_the_label_of_a_container_is_bound_centred_and_grows_the_container() {
    let mut s = testing::headless_session();
    let rect = new_id(&create(&mut s, rect_args(0.0, 0.0, 100.0, 60.0)));
    let out = create(&mut s, json!({"kind": "text", "text": "hello world", "container": rect}));
    let label = el(&s, &new_id(&out));
    let ElementKind::Text(t) = &label.kind else { panic!("text") };
    assert_eq!(t.container_id.as_ref().map(ElementId::as_str), Some(rect.as_str()));
    assert_eq!((t.font.align, t.font.vertical_align), (drafftink_scene::TextAlign::Center, drafftink_scene::VerticalAlign::Middle));
    assert_eq!(out["container"], rect.as_str());
    // The label sits inside the container and directly above it in z-order.
    let c = el(&s, &rect);
    assert!(c.rect().contains_rect(label.rect()), "{:?} in {:?}", label.rect(), c.rect());
    let order = live_order(&s);
    let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
    assert_eq!(pos(label.id.as_str()), pos(&rect) + 1);
    // The label is the new element, but the selection is the container.
    assert_eq!(selected(&s), vec![rect.clone()]);
    assert_eq!(s.tab().doc.undo_depth(), 2, "two commands, two steps");
}

#[test]
fn a_long_label_makes_the_container_grow_and_undo_restores_it() {
    let mut s = testing::headless_session();
    let rect = new_id(&create(&mut s, rect_args(0.0, 0.0, 60.0, 40.0)));
    let before = el(&s, &rect);
    let long = "this label is far too long for a box of this size and must wrap onto several lines";
    create(&mut s, json!({"kind": "text", "text": long, "container": rect}));
    let after = el(&s, &rect);
    assert!(after.transform.h > before.transform.h, "{:?} -> {:?}", before.transform, after.transform);
    s.undo_steps(1).unwrap();
    assert_eq!(el(&s, &rect).transform, before.transform);
}

#[test]
fn a_label_can_have_a_chosen_id_a_name_and_its_own_colour() {
    let mut s = testing::headless_session();
    let rect = new_id(&create(&mut s, rect_args(0.0, 0.0, 200.0, 100.0)));
    let out = create(
        &mut s,
        json!({"kind": "text", "text": "hi", "container": rect, "id": "lab", "name": "caption", "style": {"stroke_color": "#ff0000"}}),
    );
    assert_eq!(out["id"], "lab");
    let l = el(&s, "lab");
    assert_eq!(l.name.as_deref(), Some("caption"));
    assert_eq!(l.style.stroke_color, drafftink_scene::Color::rgb(255, 0, 0));
}

#[test]
fn label_errors_are_wrong_kind_locked_already_bound_and_not_found() {
    let mut s = testing::headless_session();
    let rect = new_id(&create(&mut s, rect_args(0.0, 0.0, 200.0, 100.0)));
    let line = new_id(&create(&mut s, json!({"kind": "line", "points": [[0, 200], [100, 200]]})));
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "text", "text": "x", "container": line}));
    assert_eq!(r.code, "wrong_kind");
    assert!(r.message.contains("cannot hold a label"), "{}", r.message);
    create(&mut s, json!({"kind": "text", "text": "first", "container": rect}));
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "text", "text": "second", "container": rect}));
    assert_eq!(r.code, "already_bound");
    assert!(r.message.contains(&format!("`{rect}` already has a label")) && r.message.contains("`text.unbind`"), "{}", r.message);
    let free = new_id(&create(&mut s, rect_args(0.0, 300.0, 100.0, 100.0)));
    lock(&mut s, &free);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "text", "text": "x", "container": free}));
    assert_eq!(r.code, "locked");
    assert!(r.message.contains(&format!("`{free}`")), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "text", "text": "x", "container": "nope"}));
    assert_eq!(r.code, "not_found");
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "text", "text": "x", "container": rect, "at": [0, 0]}));
    assert!(r.message.contains("`at`"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "text", "text": "  ", "container": rect}));
    assert_eq!(r.code, "empty_text");
}

#[test]
fn an_arrow_can_be_labelled_at_creation() {
    let mut s = testing::headless_session();
    let arrow = new_id(&create(&mut s, json!({"kind": "arrow", "points": [[0, 0], [300, 0]]})));
    let out = create(&mut s, json!({"kind": "text", "text": "go", "container": arrow}));
    let l = el(&s, &new_id(&out));
    let ElementKind::Text(t) = &l.kind else { panic!("text") };
    assert_eq!((t.container_id.as_ref().map(ElementId::as_str), t.label_position), (Some(arrow.as_str()), Some(0.5)));
}

#[test]
fn invalid_latex_is_refused_with_the_catalog_text_when_the_session_has_a_formula_engine() {
    let (mut s, _f) = testing::session_typeset();
    let before = s.fingerprint();
    let r = refuse(&mut s, "element.create", json!({"kind": "math", "at": [0, 0], "latex": "\\nosuchcommand{x}"}));
    assert_eq!(r.code, "invalid_latex");
    assert!(r.message.starts_with("The LaTeX could not be parsed: "), "{}", r.message);
    assert!(
        r.message.ends_with(" Fix the expression. Examples: `x^2`, `\\frac{a}{b}`, `\\sqrt{x}`, `\\sum_{i=1}^n i`."),
        "{}",
        r.message
    );
    assert_eq!(s.fingerprint(), before);
    // A real formula gets the size of its layout, and the placeholder is valid.
    let out = create(&mut s, json!({"kind": "math", "at": [10, 20], "latex": "\\frac{a}{b} + \\sqrt{x}"}));
    assert!(out["bounds"]["w"].as_f64().unwrap() > 20.0 && out["bounds"]["h"].as_f64().unwrap() > 10.0, "{out}");
    let out = create(&mut s, json!({"kind": "math", "at": [0, 0]}));
    assert_eq!(el(&s, &new_id(&out)).tag(), Kind::Math);
}

#[test]
fn math_has_the_old_placeholder_and_a_measured_box() {
    let mut s = testing::headless_session();
    let e = made(&mut s, json!({"kind": "math", "at": [10, 20]}));
    let ElementKind::Math(m) = &e.kind else { panic!("math") };
    assert_eq!((m.latex.as_str(), m.font_size), ("x^2 + y^2 = r^2", 20.0));
    assert_eq!((e.transform.x, e.transform.y), (10.0, 20.0));
    assert_eq!((e.transform.w, e.transform.h), (150.0, 26.0));
    let e = made(&mut s, json!({"kind": "math", "at": [0, 0], "latex": "\\frac{a}{b}", "style": {"font_size": 40}}));
    let ElementKind::Math(m) = &e.kind else { panic!("math") };
    assert_eq!((m.latex.as_str(), m.font_size), ("\\frac{a}{b}", 40.0));
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "math", "at": [0, 0], "latex": "  "}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`latex`"), "{}", r.message);
    let r = refuse_clean(&mut s, "element.create", json!({"kind": "rectangle", "latex": "x", "bounds": {"x":0,"y":0,"w":5,"h":5}}));
    assert!(r.message.contains("`latex`"), "{}", r.message);
}

// ---------------------------------------------------------------------------------------------
// element.create: frames
// ---------------------------------------------------------------------------------------------

fn frame(s: &mut Session, x: f64, y: f64, w: f64, h: f64) -> String {
    new_id(&create(s, json!({"kind": "frame", "bounds": {"x": x, "y": y, "w": w, "h": h}})))
}

#[test]
fn a_new_element_joins_the_frame_under_its_centre_and_sits_below_it() {
    let mut s = testing::headless_session();
    let f = frame(&mut s, 0.0, 0.0, 400.0, 300.0);
    let out = create(&mut s, rect_args(50.0, 50.0, 100.0, 100.0));
    let id = new_id(&out);
    assert_eq!(el(&s, &id).frame_id.as_ref().map(ElementId::as_str), Some(f.as_str()));
    assert_eq!(out["frame"], f.as_str());
    let order = live_order(&s);
    let pos = |x: &str| order.iter().position(|o| o == x).unwrap();
    assert_eq!(pos(&id) + 1, pos(&f), "the child sits directly below its frame");
    // A shape whose centre is outside the frame does not join.
    let out = create(&mut s, rect_args(390.0, 50.0, 100.0, 100.0));
    assert!(el(&s, &new_id(&out)).frame_id.is_none());
    assert!(out.get("frame").is_none());
}

#[test]
fn frame_null_and_frame_ref_are_honoured() {
    let mut s = testing::headless_session();
    let f = frame(&mut s, 0.0, 0.0, 400.0, 300.0);
    let mut a = rect_args(50.0, 50.0, 100.0, 100.0);
    a["frame"] = Value::Null;
    assert!(made(&mut s, a.clone()).frame_id.is_none());
    a["frame"] = json!(f);
    a["bounds"]["x"] = json!(900);
    let id = new_id(&create(&mut s, a.clone()));
    assert_eq!(el(&s, &id).frame_id.as_ref().map(ElementId::as_str), Some(f.as_str()), "an explicit frame is used wherever the shape is");
    // A name works too.
    run(&mut s, "element.set_name", json!({"id": f, "name": "page"}));
    a["frame"] = json!("@page");
    let id = new_id(&create(&mut s, a));
    assert_eq!(el(&s, &id).frame_id.as_ref().map(ElementId::as_str), Some(f.as_str()));
}

#[test]
fn a_frame_argument_that_is_not_a_usable_frame_is_refused() {
    let mut s = testing::headless_session();
    let r = new_id(&create(&mut s, rect_args(0.0, 0.0, 50.0, 50.0)));
    let mut a = rect_args(10.0, 10.0, 10.0, 10.0);
    a["frame"] = json!(r);
    let e = refuse_clean(&mut s, "element.create", a.clone());
    assert_eq!(e.code, "wrong_kind");
    assert!(e.message.contains("not a frame"), "{}", e.message);
    let f = frame(&mut s, 200.0, 0.0, 100.0, 100.0);
    lock(&mut s, &f);
    a["frame"] = json!(f);
    let e = refuse_clean(&mut s, "element.create", a);
    assert_eq!(e.code, "locked");
    assert!(e.message.contains(&format!("`{f}`")), "{}", e.message);
    let e = refuse_clean(&mut s, "element.create", json!({"kind": "frame", "bounds": {"x":0,"y":0,"w":9,"h":9}, "frame": f}));
    assert_eq!(e.code, "bad_args");
    assert!(e.message.contains("frame"), "{}", e.message);
}

#[test]
fn a_locked_frame_is_skipped_by_auto() {
    let mut s = testing::headless_session();
    let f = frame(&mut s, 0.0, 0.0, 400.0, 300.0);
    lock(&mut s, &f);
    let out = create(&mut s, rect_args(50.0, 50.0, 100.0, 100.0));
    assert!(el(&s, &new_id(&out)).frame_id.is_none());
}

#[test]
fn a_new_frame_takes_in_the_unlocked_elements_that_lie_completely_inside() {
    let mut s = testing::headless_session();
    let quiet = |s: &mut Session, x: f64, y: f64, w: f64, h: f64| {
        let mut a = rect_args(x, y, w, h);
        a["select"] = json!(false);
        new_id(&create(s, a))
    };
    let inside = quiet(&mut s, 20.0, 20.0, 50.0, 50.0);
    let partly = quiet(&mut s, 350.0, 20.0, 100.0, 50.0);
    let locked = quiet(&mut s, 100.0, 100.0, 50.0, 50.0);
    lock(&mut s, &locked);
    let outside = quiet(&mut s, 600.0, 0.0, 50.0, 50.0);
    let depth = s.tab().doc.undo_depth();
    let out = create(&mut s, json!({"kind": "frame", "bounds": {"x": 0, "y": 0, "w": 400, "h": 300}}));
    let f = new_id(&out);
    assert_eq!(out["captured"], json!([inside]));
    assert_eq!(el(&s, &inside).frame_id.as_ref().map(ElementId::as_str), Some(f.as_str()));
    for other in [&partly, &locked, &outside] {
        assert!(el(&s, other).frame_id.is_none(), "{other}");
    }
    let order = live_order(&s);
    let pos = |x: &str| order.iter().position(|o| o == x).unwrap();
    assert!(pos(&inside) < pos(&f), "children sit below the frame");
    // One undo step undoes the frame and its capture.
    assert_eq!(s.tab().doc.undo_depth(), depth + 1);
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid(&f)).is_none());
    assert!(el(&s, &inside).frame_id.is_none());
}

#[test]
fn capture_false_takes_nothing_and_a_group_is_taken_only_whole() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 20.0, 20.0, 50.0, 50.0).unwrap();
    let out = create(&mut s, json!({"kind": "frame", "bounds": {"x": 0, "y": 0, "w": 400, "h": 300}, "capture": false}));
    assert!(out.get("captured").is_none());
    assert!(el(&s, a.as_str()).frame_id.is_none());
    // A group with one member outside is not taken.
    let b = testing::add_rect(&mut s, 520.0, 20.0, 50.0, 50.0).unwrap();
    let c = testing::add_rect(&mut s, 600.0, 520.0, 50.0, 50.0).unwrap();
    run_group(&mut s, &[&b, &c]);
    let out = create(&mut s, json!({"kind": "frame", "bounds": {"x": 500, "y": 0, "w": 100, "h": 100}}));
    assert!(out.get("captured").is_none(), "{out}");
}

fn run_group(s: &mut Session, ids: &[&ElementId]) {
    let g = drafftink_scene::GroupId::new("g9").unwrap();
    for id in ids {
        let gg = g.clone();
        s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.update(id, |e| e.group_ids.push(gg))).unwrap();
    }
    s.tab_mut().doc.commit_untracked().unwrap();
}

#[test]
fn an_arrow_inside_a_frame_joins_only_when_an_end_touches_a_member() {
    let mut s = testing::headless_session();
    let f = frame(&mut s, 0.0, 0.0, 400.0, 300.0);
    let r = new_id(&create(&mut s, rect_args(20.0, 20.0, 100.0, 60.0)));
    // An arrow whose start is on the member joins.
    let joined = create(&mut s, json!({"kind": "arrow", "points": [[60, 50], [300, 200]]}));
    assert_eq!(joined["frame"], f.as_str(), "{joined}");
    // A free arrow in empty space stays out silently under auto.
    let free = create(&mut s, json!({"kind": "arrow", "points": [[350, 250], [390, 290]]}));
    assert!(free.get("frame").is_none());
    assert!(free["warnings"].as_array().unwrap().is_empty());
    // With an explicit frame the refusal is a warning.
    let explicit = create(&mut s, json!({"kind": "arrow", "points": [[350, 250], [390, 290]], "frame": f}));
    assert!(explicit["warnings"][0].as_str().unwrap().contains("not added to frame"), "{explicit}");
    let _ = r;
}

// ---------------------------------------------------------------------------------------------
// element.create: after
// ---------------------------------------------------------------------------------------------

#[test]
fn after_puts_the_element_directly_above_the_named_one() {
    let (mut s, ids) = testing::session_with_shapes().unwrap();
    let mut a = rect_args(0.0, 0.0, 10.0, 10.0);
    a["after"] = json!(ids[0].as_str());
    let id = new_id(&create(&mut s, a));
    let order = live_order(&s);
    assert_eq!(order, vec![ids[0].as_str().to_owned(), id, ids[1].as_str().to_owned(), ids[2].as_str().to_owned()]);
    let mut a = rect_args(0.0, 0.0, 10.0, 10.0);
    a["after"] = json!("nope");
    assert_eq!(refuse_clean(&mut s, "element.create", a).code, "not_found");
}

#[test]
fn after_does_not_split_a_container_from_its_label() {
    let mut s = testing::headless_session();
    let r = new_id(&create(&mut s, rect_args(0.0, 0.0, 100.0, 60.0)));
    let top = new_id(&create(&mut s, rect_args(300.0, 0.0, 100.0, 60.0)));
    let label = testing::add_text_label(&mut s, &eid(&r), "hi").unwrap();
    let mut a = rect_args(0.0, 200.0, 10.0, 10.0);
    a["after"] = json!(r);
    let id = new_id(&create(&mut s, a));
    assert_eq!(live_order(&s), vec![r, label.as_str().to_owned(), id, top]);
}

#[test]
fn after_stays_in_the_frame_block_and_is_ignored_with_a_warning_across_frames() {
    let mut s = testing::headless_session();
    let f = frame(&mut s, 0.0, 0.0, 400.0, 300.0);
    let outside = new_id(&create(&mut s, rect_args(600.0, 0.0, 50.0, 50.0)));
    let mut a = rect_args(50.0, 50.0, 20.0, 20.0);
    a["after"] = json!(outside);
    let out = create(&mut s, a);
    assert!(out["warnings"][0].as_str().unwrap().contains("`after` was ignored"), "{out}");
    let order = live_order(&s);
    let pos = |x: &str| order.iter().position(|o| o == x).unwrap();
    assert_eq!(pos(&new_id(&out)) + 1, pos(&f));
}

// ---------------------------------------------------------------------------------------------
// batches
// ---------------------------------------------------------------------------------------------

#[test]
fn a_batch_builds_a_diagram_with_names_refs_and_one_undo_step() {
    let mut s = testing::headless_session();
    let steps = [
        BatchStep::new("element.create", json!({"kind": "rectangle", "bounds": {"x": 0, "y": 0, "w": 100, "h": 60}, "name": "a"})).named("a"),
        BatchStep::new("element.create", json!({"kind": "ellipse", "bounds": {"x": 300, "y": 0, "w": 100, "h": 60}})).named("b"),
        BatchStep::new("element.set_name", json!({"id": "$b", "name": "second"})),
        BatchStep::new("element.connect", json!({"from": "$a", "to": "@second", "label": "go", "name": "link"})).named("link"),
        BatchStep::new("element.relabel", json!({"id": "@a", "text": "Start"})),
        BatchStep::new("element.relabel", json!({"id": "$link", "text": "next"})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.ok(), "{:?}", out.failed);
    assert_eq!(out.completed, 6);
    assert_eq!(s.tab().doc.undo_depth(), 1, "a batch is one undo step");
    let link = s.resolve_ref(&crate::session::Ref("@link".to_owned())).unwrap();
    let d = s.element(&link).unwrap().linear_data().unwrap().clone();
    assert_eq!(d.start_binding.as_ref().map(|b| b.target.as_str()), Some("e1"));
    let b = s.resolve_ref(&crate::session::Ref("@second".to_owned())).unwrap();
    assert_eq!(d.end_binding.as_ref().map(|x| &x.target), Some(&b));
    let labels: Vec<String> = s
        .store()
        .iter()
        .filter_map(|e| e.text_data().map(|t| t.content.clone()))
        .collect();
    assert_eq!(labels.len(), 2);
    assert!(labels.contains(&"Start".to_owned()) && labels.contains(&"next".to_owned()), "{labels:?}");
    assert!(stale_arrows(s.store()).is_empty());
    s.undo_steps(1).unwrap();
    assert_eq!(s.store().iter().count(), 0, "one undo removes the whole diagram");
}

#[test]
fn a_failing_step_rolls_the_whole_batch_back() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    let steps = [
        BatchStep::new("element.create", rect_args(0.0, 0.0, 100.0, 60.0)).named("a"),
        BatchStep::new("element.create", rect_args(0.0, 0.0, 0.5, 60.0)),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    let failed = out.failed.expect("the second step fails");
    assert_eq!((failed.index, failed.refusal.code), (1, "too_small"));
    assert!(out.rolled_back);
    assert_eq!(s.fingerprint(), before);
}

// ---------------------------------------------------------------------------------------------
// element.connect
// ---------------------------------------------------------------------------------------------

#[test]
fn connect_binds_both_ends_at_the_facing_sides_and_selects_the_arrow() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    let out = run(&mut s, "element.connect", json!({"from": a.as_str(), "to": b.as_str()}));
    let id = new_id(&out);
    assert_eq!(out["created"], json!([id]));
    let e = el(&s, &id);
    let d = e.linear_data().unwrap();
    assert!(matches!(e.kind, ElementKind::Arrow(_)));
    assert_eq!((d.start_head, d.end_head, d.path), (Head::None, Head::Arrow, PathType::Sharp));
    let (sb, eb) = (d.start_binding.as_ref().unwrap(), d.end_binding.as_ref().unwrap());
    assert_eq!((&sb.target, sb.mode, &eb.target, eb.mode), (&a, BindMode::Orbit, &b, BindMode::Orbit));
    // A's right side faces B: the start is on A's right outline plus the gap, the end on B's left.
    assert!(close(pt(&e, 0).x, 106.0) && close(pt(&e, 0).y, 30.0), "{:?}", pt(&e, 0));
    assert!(close(pt(&e, 1).x, 294.0) && close(pt(&e, 1).y, 30.0), "{:?}", pt(&e, 1));
    assert!(stale_arrows(s.store()).is_empty());
    assert_eq!(selected(&s), vec![id.clone()]);
    // The arrow is above both.
    let order = live_order(&s);
    assert_eq!(order.last().map(String::as_str), Some(id.as_str()));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid(&id)).is_none());
}

#[test]
fn connect_faces_the_sides_whatever_the_direction() {
    let mut s = testing::headless_session();
    let top = testing::add_rect(&mut s, 0.0, 0.0, 100.0, 60.0).unwrap();
    let low = testing::add_rect(&mut s, 0.0, 300.0, 100.0, 60.0).unwrap();
    let out = run(&mut s, "element.connect", json!({"from": low.as_str(), "to": top.as_str()}));
    let e = el(&s, &new_id(&out));
    assert!(close(pt(&e, 0).x, 50.0) && close(pt(&e, 0).y, 294.0), "{:?}", pt(&e, 0));
    assert!(close(pt(&e, 1).x, 50.0) && close(pt(&e, 1).y, 66.0), "{:?}", pt(&e, 1));
}

#[test]
fn connect_options_kind_type_mode_heads_style_name_and_label() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    let out = run(
        &mut s,
        "element.connect",
        json!({"from": a.as_str(), "to": "e2", "kind": "line", "arrow_type": "elbow", "start_head": "circle", "end_head": "triangle",
               "style": {"stroke_color": "#00ff00", "stroke_width": 4}, "name": "wire"}),
    );
    let e = el(&s, &new_id(&out));
    assert!(matches!(e.kind, ElementKind::Line(_)));
    let d = e.linear_data().unwrap();
    assert_eq!((d.path, d.start_head, d.end_head), (PathType::Elbow, Head::Circle, Head::Triangle));
    assert_eq!(e.style.stroke_color, drafftink_scene::Color::rgb(0, 255, 0));
    assert_eq!(e.style.stroke_width, 4.0);
    assert_eq!(e.name.as_deref(), Some("wire"));
    assert_eq!(d.start_binding.as_ref().map(|x| &x.target), Some(&a));
    assert_eq!(d.end_binding.as_ref().map(|x| &x.target), Some(&b));
    assert!(out.get("label").is_none());
    assert!(stale_arrows(s.store()).is_empty());

    let out = run(&mut s, "element.connect", json!({"from": a.as_str(), "to": "e2", "label": "flows to"}));
    let arrow = new_id(&out);
    let label = el(&s, out["label"].as_str().expect("the label id"));
    assert_eq!(label.text_data().unwrap().content, "flows to");
    assert_eq!(label.text_data().unwrap().container_id.as_ref().map(ElementId::as_str), Some(arrow.as_str()));
    assert_eq!(label.text_data().unwrap().label_position, Some(0.5));
    assert!(stale_arrows(s.store()).is_empty());
    let r = refuse_clean(&mut s, "element.connect", json!({"from": a.as_str(), "to": "e2", "kind": "line", "label": "no"}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`label`") && r.message.contains("only an arrow"), "{}", r.message);

    let out = run(&mut s, "element.connect", json!({"from": a.as_str(), "to": b.as_str(), "bind_mode": "inside", "label": "  "}));
    assert!(out.get("label").is_none(), "a blank label means none");
    let d = el(&s, &new_id(&out)).linear_data().unwrap().clone();
    assert_eq!(d.start_binding.as_ref().map(|x| x.mode), Some(BindMode::Inside));
}

#[test]
fn connect_refuses_with_the_catalog_texts() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    let line = new_id(&create(&mut s, json!({"kind": "line", "points": [[0, 200], [100, 200]]})));
    let r = refuse_clean(&mut s, "element.connect", json!({"from": a.as_str(), "to": a.as_str()}));
    assert_eq!(r.code, "same_element");
    assert_eq!(r.message, "Cannot connect an element to itself with `element.connect`. Pass two different elements.");
    run(&mut s, "element.set_name", json!({"id": a.as_str(), "name": "first"}));
    let r = refuse_clean(&mut s, "element.connect", json!({"from": "@first", "to": a.as_str()}));
    assert_eq!(r.code, "same_element", "a name and an id of one element are the same element");
    let r = refuse_clean(&mut s, "element.connect", json!({"from": a.as_str(), "to": line}));
    assert_eq!(r.code, "not_bindable");
    assert_eq!(
        r.message,
        "Line elements cannot be arrow targets. Bindable kinds: rectangle, diamond, ellipse, text, image, frame."
    );
    let r = refuse_clean(&mut s, "element.connect", json!({"from": a.as_str(), "to": "ghost"}));
    assert_eq!(r.code, "not_found");
    lock(&mut s, b.as_str());
    let r = refuse_clean(&mut s, "element.connect", json!({"from": a.as_str(), "to": b.as_str()}));
    assert_eq!(r.code, "locked");
    assert!(r.message.contains(&format!("`{b}`")), "{}", r.message);
    let r = refuse_clean(&mut s, "element.connect", json!({"from": a.as_str(), "to": "e2", "kind": "curve"}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn connect_refuses_a_label_text_and_a_taken_name_and_leaves_no_trace() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    let label = testing::add_text_label(&mut s, &a, "hi").unwrap();
    let r = refuse_clean(&mut s, "element.connect", json!({"from": b.as_str(), "to": label.as_str()}));
    assert_eq!(r.code, "not_bindable");
    assert_eq!(r.message, "Text that labels another element cannot be an arrow target. Bind to the labelled element instead.");
    run(&mut s, "element.set_name", json!({"id": a.as_str(), "name": "dup"}));
    let r = refuse_clean(&mut s, "element.connect", json!({"from": a.as_str(), "to": b.as_str(), "name": "dup"}));
    assert_eq!(r.code, "name_taken");
}

#[test]
fn connect_puts_the_arrow_into_the_frame_both_shapes_are_in() {
    let mut s = testing::headless_session();
    let f = frame(&mut s, -50.0, -50.0, 600.0, 200.0);
    let a = new_id(&create(&mut s, rect_args(0.0, 0.0, 100.0, 60.0)));
    let b = new_id(&create(&mut s, rect_args(300.0, 0.0, 100.0, 60.0)));
    let out = run(&mut s, "element.connect", json!({"from": a, "to": b}));
    assert_eq!(el(&s, &new_id(&out)).frame_id.as_ref().map(ElementId::as_str), Some(f.as_str()));
}

#[test]
fn testing_helpers_build_an_arrow_and_a_label_without_commands() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    let arrow = testing::create_arrow_between(&mut s, &a, &b).unwrap();
    let d = s.element(&arrow).unwrap().linear_data().unwrap().clone();
    assert_eq!(d.start_binding.as_ref().map(|x| &x.target), Some(&a));
    assert_eq!(d.end_binding.as_ref().map(|x| &x.target), Some(&b));
    assert!(stale_arrows(s.store()).is_empty());
    let l = testing::add_text_label(&mut s, &arrow, "x").unwrap();
    assert_eq!(s.element(&l).unwrap().container_id(), Some(&arrow));
    assert_eq!(s.tab().doc.undo_depth(), 0, "fixtures are not undo steps");
}

// ---------------------------------------------------------------------------------------------
// element.relabel
// ---------------------------------------------------------------------------------------------

#[test]
fn relabel_creates_updates_and_removes_a_label() {
    let mut s = testing::headless_session();
    let r = new_id(&create(&mut s, rect_args(0.0, 0.0, 200.0, 100.0)));
    let out = run(&mut s, "element.relabel", json!({"id": r, "text": "one"}));
    assert_eq!(out["action"], "created");
    let label = out["label"].as_str().expect("label id").to_owned();
    assert_eq!(el(&s, &label).text_data().unwrap().content, "one");
    assert_eq!(el(&s, &label).container_id().map(ElementId::as_str), Some(r.as_str()));

    let out = run(&mut s, "element.relabel", json!({"id": r, "text": "two"}));
    assert_eq!((out["action"].as_str(), out["label"].as_str()), (Some("updated"), Some(label.as_str())));
    assert_eq!(el(&s, &label).text_data().unwrap().content, "two");

    let out = run(&mut s, "element.relabel", json!({"id": r, "text": ""}));
    assert_eq!(out["action"], "removed");
    assert!(s.element(&eid(&label)).is_none());
    assert_eq!(out["deleted"], json!([label]));
}

#[test]
fn relabel_creates_the_label_with_the_text_defaults_of_the_tool() {
    let mut s = testing::headless_session();
    run(&mut s, "style.set", json!({"props": {"font_size": 36}, "scope": "defaults"}));
    let (a, _b) = two_boxes(&mut s);
    let out = run(&mut s, "element.relabel", json!({"id": a.as_str(), "text": "hello"}));
    assert_eq!(out["action"], "created");
    let label = el(&s, out["label"].as_str().unwrap());
    assert_eq!(label.text_data().unwrap().font.size, 36.0);
}

#[test]
fn relabel_with_empty_text_on_a_bound_label_removes_the_label_but_refuses_on_free_text() {
    let mut s = testing::headless_session();
    let r = new_id(&create(&mut s, rect_args(0.0, 0.0, 200.0, 100.0)));
    let label = run(&mut s, "element.relabel", json!({"id": r, "text": "one"}))["label"].as_str().expect("label id").to_owned();
    // The label given as `id` counts as its container: the same as `text.set` with empty text.
    let out = run(&mut s, "element.relabel", json!({"id": label, "text": ""}));
    assert_eq!((out["action"].as_str(), out["deleted"].clone()), (Some("removed"), json!([label])));
    assert!(s.element(&eid(&label)).is_none());
    assert!(s.element(&eid(&r)).is_some(), "the container stays");

    let free = new_id(&create(&mut s, json!({"kind": "text", "text": "free", "at": [0, 300]})));
    let before = s.fingerprint();
    let e = s.execute("element.relabel", json!({"id": free, "text": ""})).unwrap_err();
    assert_eq!(e.code, "empty_text");
    assert_eq!(e.message, "A text element cannot be empty. Delete it with `element.delete`.");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn relabel_on_a_text_element_replaces_its_content_and_re_measures() {
    let mut s = testing::headless_session();
    let t = new_id(&create(&mut s, json!({"kind": "text", "text": "hello", "at": [0, 0]})));
    let out = run(&mut s, "element.relabel", json!({"id": t, "text": "hello world!!"}));
    assert_eq!(out["action"], "updated");
    let e = el(&s, &t);
    assert_eq!(e.text_data().unwrap().content, "hello world!!");
    assert_eq!((e.transform.w, e.transform.h), (130.0, 25.0));
}

#[test]
fn relabel_grows_the_container_and_keeps_bound_arrows_solved() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    let arrow = testing::create_arrow_between(&mut s, &a, &b).unwrap();
    let before = el(&s, a.as_str()).transform;
    run(&mut s, "element.relabel", json!({"id": a.as_str(), "text": "a label that is far too long for this box so it has to wrap a lot"}));
    assert!(el(&s, a.as_str()).transform.h > before.h);
    assert!(stale_arrows(s.store()).is_empty());
    let _ = arrow;
}

#[test]
fn relabel_an_arrow() {
    let mut s = testing::headless_session();
    let (a, b) = two_boxes(&mut s);
    let arrow = testing::create_arrow_between(&mut s, &a, &b).unwrap();
    let out = run(&mut s, "element.relabel", json!({"id": arrow.as_str(), "text": "go"}));
    assert_eq!(out["action"], "created");
    assert_eq!(el(&s, out["label"].as_str().unwrap()).text_data().unwrap().label_position, Some(0.5));
}

#[test]
fn relabel_with_create_false_and_no_label_does_nothing() {
    let mut s = testing::headless_session();
    let r = new_id(&create(&mut s, rect_args(0.0, 0.0, 200.0, 100.0)));
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "element.relabel", json!({"id": r, "text": "x", "create": false}));
    assert_eq!(out["action"], "none");
    assert_eq!(out["changed"], false);
    let out = run(&mut s, "element.relabel", json!({"id": r, "text": ""}));
    assert_eq!(out["action"], "none");
    assert_eq!(s.tab().doc.undo_depth(), depth, "a no-op is no undo step");
}

#[test]
fn relabel_refusals() {
    let mut s = testing::headless_session();
    let line = new_id(&create(&mut s, json!({"kind": "line", "points": [[0, 0], [100, 0]]})));
    let r = refuse_clean(&mut s, "element.relabel", json!({"id": line, "text": "x"}));
    assert_eq!(r.code, "wrong_kind");
    assert_eq!(
        r.message,
        "`element.relabel` does not work on line elements. It works on text, rectangle, diamond, ellipse and arrow."
    );
    let rect = new_id(&create(&mut s, rect_args(0.0, 100.0, 100.0, 100.0)));
    let math = new_id(&create(&mut s, json!({"kind": "math", "at": [0, 400]})));
    assert_eq!(refuse_clean(&mut s, "element.relabel", json!({"id": math, "text": "x"})).code, "wrong_kind");
    assert_eq!(refuse_clean(&mut s, "element.relabel", json!({"id": "nope", "text": "x"})).code, "not_found");
    lock(&mut s, &rect);
    let r = refuse_clean(&mut s, "element.relabel", json!({"id": rect, "text": "x"}));
    assert_eq!(r.code, "locked");
    assert!(r.message.contains(&format!("`{rect}`")), "{}", r.message);
    let text = new_id(&create(&mut s, json!({"kind": "text", "text": "hi", "at": [0, 600]})));
    let r = refuse_clean(&mut s, "element.relabel", json!({"id": text, "text": "  "}));
    assert_eq!(r.code, "empty_text");
    assert_eq!(r.message, "A text element cannot be empty. Delete it with `element.delete`.");
}

#[test]
fn relabel_undo_restores_the_label_and_the_selection_survives_a_removal() {
    let mut s = testing::headless_session();
    let r = new_id(&create(&mut s, rect_args(0.0, 0.0, 200.0, 100.0)));
    run(&mut s, "element.relabel", json!({"id": r, "text": "one"}));
    assert_eq!(selected(&s), vec![r.clone()]);
    run(&mut s, "element.relabel", json!({"id": r, "text": ""}));
    assert_eq!(selected(&s), vec![r.clone()]);
    s.undo_steps(1).unwrap();
    assert_eq!(testing_label_count(&s), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(testing_label_count(&s), 0);
}

fn testing_label_count(s: &Session) -> usize {
    s.store().iter().filter(|e| e.tag() == Kind::Text).count()
}

// ---------------------------------------------------------------------------------------------
// element.set_name
// ---------------------------------------------------------------------------------------------

#[test]
fn set_name_sets_changes_and_clears() {
    let mut s = testing::headless_session();
    let r = new_id(&create(&mut s, rect_args(0.0, 0.0, 50.0, 50.0)));
    let out = run(&mut s, "element.set_name", json!({"id": r, "name": "box"}));
    assert_eq!((out["id"].as_str(), out["name"].as_str(), out["undo_label"].as_str()), (Some(r.as_str()), Some("box"), Some("Name element")));
    assert_eq!(el(&s, &r).name.as_deref(), Some("box"));
    // The element is now addressable by @name.
    run(&mut s, "element.set_name", json!({"id": "@box", "name": "crate"}));
    assert_eq!(el(&s, &r).name.as_deref(), Some("crate"));
    let out = run(&mut s, "element.set_name", json!({"id": r, "name": null}));
    assert!(out.get("name").is_none());
    assert_eq!(el(&s, &r).name, None);
    run(&mut s, "element.set_name", json!({"id": r, "name": "again"}));
    run(&mut s, "element.set_name", json!({"id": r}));
    assert_eq!(el(&s, &r).name, None, "an absent name clears");
}

#[test]
fn setting_the_same_name_again_is_a_no_op_without_an_undo_step() {
    let mut s = testing::headless_session();
    let r = new_id(&create(&mut s, rect_args(0.0, 0.0, 50.0, 50.0)));
    run(&mut s, "element.set_name", json!({"id": r, "name": "box"}));
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "element.set_name", json!({"id": r, "name": "box"}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), depth);
    let out = run(&mut s, "element.set_name", json!({"id": r, "name": null}));
    assert_eq!(out["changed"], true);
    let out = run(&mut s, "element.set_name", json!({"id": r, "name": null}));
    assert_eq!(out["changed"], false);
}

#[test]
fn set_name_refusals_have_the_catalog_texts() {
    let mut s = testing::headless_session();
    let a = new_id(&create(&mut s, rect_args(0.0, 0.0, 50.0, 50.0)));
    let b = new_id(&create(&mut s, rect_args(100.0, 0.0, 50.0, 50.0)));
    run(&mut s, "element.set_name", json!({"id": a, "name": "box"}));
    let r = refuse_clean(&mut s, "element.set_name", json!({"id": b, "name": "box"}));
    assert_eq!(r.code, "name_taken");
    assert_eq!(
        r.message,
        format!("The name `box` is already used by element `{a}`. Names must be unique so that `@box` is unambiguous. Pick another name, or clear the other with `element.set_name`.")
    );
    for bad in ["", "@x", "$x", "tab\there", &"y".repeat(65)] {
        let r = refuse_clean(&mut s, "element.set_name", json!({"id": b, "name": bad}));
        assert_eq!(r.code, "bad_name", "{bad:?}");
        assert_eq!(
            r.message,
            "A name must be 1 to 64 characters, must not start with `@` or `$`, and must not contain control characters."
        );
    }
    assert!(s.element(&eid(&b)).is_some_and(|e| e.name.is_none()));
    let r = refuse_clean(&mut s, "element.set_name", json!({"id": "@ghost", "name": "x"}));
    assert_eq!(r.code, "not_found");
    assert!(r.message.contains("@ghost"), "{}", r.message);
}

#[test]
fn a_locked_element_can_still_be_named_and_a_name_can_be_64_characters() {
    let mut s = testing::headless_session();
    let a = new_id(&create(&mut s, rect_args(0.0, 0.0, 50.0, 50.0)));
    lock(&mut s, &a);
    let name = "n".repeat(64);
    run(&mut s, "element.set_name", json!({"id": a, "name": name}));
    assert_eq!(el(&s, &a).name.as_deref(), Some(name.as_str()));
}

#[test]
fn set_name_undo_restores_the_old_name() {
    let mut s = testing::headless_session();
    let a = new_id(&create(&mut s, rect_args(0.0, 0.0, 50.0, 50.0)));
    run(&mut s, "element.set_name", json!({"id": a, "name": "one"}));
    run(&mut s, "element.set_name", json!({"id": a, "name": "two"}));
    s.undo_steps(1).unwrap();
    assert_eq!(el(&s, &a).name.as_deref(), Some("one"));
    s.undo_steps(1).unwrap();
    assert_eq!(el(&s, &a).name, None);
}

// ---------------------------------------------------------------------------------------------
// Determinism and state
// ---------------------------------------------------------------------------------------------

#[test]
fn ids_and_seeds_come_from_the_id_source_so_two_sessions_agree() {
    let make = || {
        let mut s = testing::headless_session();
        let a = new_id(&create(&mut s, rect_args(0.0, 0.0, 50.0, 50.0)));
        let b = new_id(&create(&mut s, json!({"kind": "freehand", "points": stroke()})));
        let c = new_id(&create(&mut s, json!({"kind": "text", "text": "x", "at": [0, 0]})));
        [a, b, c].map(|i| (i.clone(), el(&s, &i).style.seed))
    };
    assert_eq!(make(), make());
}
