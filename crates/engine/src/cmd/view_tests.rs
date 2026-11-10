//! Tests of the `view` family. Pattern per command: availability, refusal text, effect, one negative case.
//! No UI, no DOM: a headless session and the fake ports.

use drafftink_scene::ElementKind;
use serde_json::{Value, json};

use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::state::{BASE_ZOOM, ZOOM_MAX, ZOOM_MIN};
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

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn session() -> Session {
    let mut s = testing::headless_session();
    run(&mut s, "view.set_viewport", json!({"width": 800, "height": 600}));
    s
}

fn scroll(s: &Session) -> (f64, f64) {
    (s.view().scroll.x, s.view().scroll.y)
}

// ---- always available, none create undo steps or document changes ----

#[test]
fn camera_commands_are_always_available_and_never_touch_history() {
    let mut s = session();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    s.view_only = true;
    for id in ["view.set", "view.pan", "view.zoom_at", "view.zoom_in", "view.zoom_out", "view.zoom_reset", "view.zoom_to_fit", "view.center", "view.scroll_page", "view.set_viewport", "view.set_view_only", "view.get", "view.screenshot"] {
        assert!(s.availability(id).is_ok(), "{id} must be available in view-only mode");
    }
    s.view_only = false;
    let rev = s.tab().doc.revision();
    run(&mut s, "view.pan", json!({"dx": 10, "dy": 10}));
    run(&mut s, "view.zoom_in", json!({}));
    run(&mut s, "view.center", json!({"on": "content"}));
    assert_eq!(s.tab().doc.undo_depth(), 0);
    assert_eq!(s.tab().doc.revision(), rev);
}

// ---- view.set ----

#[test]
fn set_moves_the_camera_and_clamps_zoom() {
    let mut s = session();
    let out = run(&mut s, "view.set", json!({"scroll": [200, -100], "zoom": 2}));
    assert_eq!(out["changed"], true);
    assert_eq!(out["scroll"], json!([200.0, -100.0]));
    assert_eq!(scroll(&s), (200.0, -100.0));
    assert_eq!(s.view().zoom, 2.0);
    run(&mut s, "view.set", json!({"zoom": 1000}));
    assert_eq!(s.view().zoom, ZOOM_MAX);
    run(&mut s, "view.set", json!({"zoom": 0.0001}));
    assert_eq!(s.view().zoom, ZOOM_MIN);
}

#[test]
fn set_reports_no_change_for_the_same_camera_and_refuses_huge_numbers() {
    let mut s = session();
    run(&mut s, "view.set", json!({"scroll": [5, 5]}));
    let out = run(&mut s, "view.set", json!({"scroll": [5, 5]}));
    assert_eq!(out["changed"], false);
    assert!(s.take_dirty().is_some(), "the first set marked a camera redraw");
    run(&mut s, "view.set", json!({"scroll": [5, 5]}));
    assert_eq!(s.take_dirty(), None, "a set that changes nothing requests no redraw");

    let r = refuse(&mut s, "view.set", json!({"scroll": [1e300, 0]}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("scroll[0]"), "{}", r.message);
    assert_eq!(scroll(&s), (5.0, 5.0));
    let r = refuse(&mut s, "view.set", json!({"scrol": [1, 1]}));
    assert!(r.message.contains("did you mean `scroll`"), "{}", r.message);
}

// ---- view.pan ----

#[test]
fn pan_moves_the_content_with_the_pointer() {
    let mut s = session();
    run(&mut s, "view.set", json!({"scroll": [0, 0], "zoom": 2}));
    run(&mut s, "view.pan", json!({"dx": 100, "dy": -40}));
    // The content moved 100 px right, so the world point at the centre is 50 units to the left.
    assert!(close(s.view().scroll.x, -50.0) && close(s.view().scroll.y, 20.0), "{:?}", scroll(&s));
}

#[test]
fn pan_refuses_numbers_that_are_not_screen_distances() {
    let mut s = session();
    let r = refuse(&mut s, "view.pan", json!({"dx": 1e300, "dy": 0}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut s, "view.pan", json!({"dx": 1}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("dy"), "{}", r.message);
}

// ---- view.zoom_at, zoom_in, zoom_out ----

#[test]
fn zoom_at_keeps_the_point_under_the_pointer_fixed() {
    let mut s = session();
    run(&mut s, "view.set", json!({"scroll": [30, 40], "zoom": 1}));
    let at = [650.0, 120.0];
    let world_before = s.view().screen_to_world(drafftink_geom::kurbo::Point::new(at[0], at[1]));
    run(&mut s, "view.zoom_at", json!({"factor": 1.5, "at": at}));
    assert!(close(s.view().zoom, 1.5));
    let world_after = s.view().screen_to_world(drafftink_geom::kurbo::Point::new(at[0], at[1]));
    assert!(close(world_before.x, world_after.x) && close(world_before.y, world_after.y));

    run(&mut s, "view.zoom_at", json!({"zoom": 4}));
    assert!(close(s.view().zoom, 4.0));
}

#[test]
fn zoom_at_wants_exactly_one_of_factor_and_zoom() {
    let mut s = session();
    let r = refuse(&mut s, "view.zoom_at", json!({}));
    assert_eq!(r.message, "Invalid arguments for `view.zoom_at`: Give exactly one of `factor` or `zoom`. See `engine.describe` for the schema.");
    let r = refuse(&mut s, "view.zoom_at", json!({"factor": 2, "zoom": 2}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut s, "view.zoom_at", json!({"factor": -2}));
    assert!(r.message.contains("`factor`"), "{}", r.message);
    let r = refuse(&mut s, "view.zoom_at", json!({"factor": 2, "at": "elsewhere"}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn zoom_in_and_out_step_by_a_quarter_and_clamp() {
    let mut s = session();
    run(&mut s, "view.set", json!({"zoom": 1}));
    run(&mut s, "view.zoom_in", json!({}));
    assert!(close(s.view().zoom, 1.25));
    run(&mut s, "view.zoom_out", json!({}));
    assert!(close(s.view().zoom, 1.0));
    run(&mut s, "view.set", json!({"zoom": ZOOM_MAX}));
    let out = run(&mut s, "view.zoom_in", json!({}));
    assert_eq!(out["changed"], false, "already at the largest zoom");
    let r = refuse(&mut s, "view.zoom_in", json!({"at": [1, "x"]}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn zoom_reset_goes_to_one_hundred_percent() {
    let mut s = session();
    testing::add_rect(&mut s, 1000.0, 1000.0, 100.0, 100.0).unwrap();
    run(&mut s, "view.set", json!({"scroll": [7, 7], "zoom": 5}));
    let out = run(&mut s, "view.zoom_reset", json!({}));
    assert_eq!(out["zoom"], BASE_ZOOM);
    assert!(close(out["percent"].as_f64().unwrap(), 100.0));
    assert_eq!(scroll(&s), (7.0, 7.0), "the centre stays");
    run(&mut s, "view.zoom_reset", json!({"recenter": true}));
    assert_eq!(scroll(&s), (1050.0, 1050.0));
}

#[test]
fn zoom_reset_with_recenter_on_an_empty_document_warns() {
    let mut s = session();
    run(&mut s, "view.set", json!({"zoom": 3}));
    let out = run(&mut s, "view.zoom_reset", json!({"recenter": true}));
    assert_eq!(out["warnings"].as_array().unwrap().len(), 1);
    assert!(refuse_text(&mut s, "view.zoom_reset", json!({"recenter": "yes"})).contains("recenter"));
}

fn refuse_text(s: &mut Session, id: &str, args: Value) -> String {
    refuse(s, id, args).message
}

// ---- view.zoom_to_fit ----

#[test]
fn fit_all_centres_the_content_and_uses_the_padding() {
    let mut s = session();
    testing::add_rect(&mut s, 0.0, 0.0, 100.0, 50.0).unwrap();
    testing::add_rect(&mut s, 300.0, 150.0, 100.0, 50.0).unwrap();
    // Content: x 0..400, y 0..200. Available: 800-100 by 600-100 => zoom min(700/400, 500/200) = 1.75.
    let out = run(&mut s, "view.zoom_to_fit", json!({"scope": "all"}));
    assert!(close(out["zoom"].as_f64().unwrap(), 1.75), "{out}");
    assert_eq!(scroll(&s), (200.0, 100.0));
    let out = run(&mut s, "view.zoom_to_fit", json!({"scope": "all", "padding": 0}));
    assert!(close(out["zoom"].as_f64().unwrap(), 2.0), "{out}");
}

#[test]
fn fit_does_not_blow_up_a_single_small_element() {
    let mut s = session();
    testing::add_rect(&mut s, 10.0, 10.0, 4.0, 4.0).unwrap();
    run(&mut s, "view.zoom_to_fit", json!({}));
    assert!(close(s.view().zoom, 3.36), "the default max_zoom caps it: {}", s.view().zoom);
    run(&mut s, "view.zoom_to_fit", json!({"max_zoom": 8}));
    assert!(s.view().zoom > 3.36);
}

#[test]
fn fit_auto_prefers_the_selection_and_scope_ids_resolves_refs() {
    let mut s = session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    let b = testing::add_rect(&mut s, 1000.0, 0.0, 10.0, 10.0).unwrap();
    testing::select(&mut s, std::slice::from_ref(&b));
    run(&mut s, "view.zoom_to_fit", json!({}));
    assert_eq!(scroll(&s), (1005.0, 5.0));
    run(&mut s, "view.zoom_to_fit", json!({"scope": "ids", "ids": [a.as_str()]}));
    assert_eq!(scroll(&s), (5.0, 5.0));
    run(&mut s, "view.zoom_to_fit", json!({"scope": "rect", "rect": {"x": 0, "y": 0, "w": 100, "h": 100}}));
    assert_eq!(scroll(&s), (50.0, 50.0));
}

#[test]
fn fit_refusals_say_what_to_do() {
    let mut s = session();
    let r = refuse(&mut s, "view.zoom_to_fit", json!({}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "Nothing to fit: the document is empty. Draw something, or use `view.center`.");
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    let r = refuse(&mut s, "view.zoom_to_fit", json!({"scope": "selection"}));
    assert!(r.message.contains("nothing is selected"), "{}", r.message);
    let r = refuse(&mut s, "view.zoom_to_fit", json!({"scope": "ids", "ids": ["nope"]}));
    assert_eq!(r.code, "not_found");
    let r = refuse(&mut s, "view.zoom_to_fit", json!({"scope": "ids"}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut s, "view.zoom_to_fit", json!({"scope": "rect", "rect": {"x": 0, "y": 0, "w": -5, "h": 5}}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut s, "view.zoom_to_fit", json!({"padding": -1}));
    assert!(r.message.contains("padding"), "{}", r.message);
}

#[test]
fn fit_ignores_deleted_elements_and_uses_the_rotated_box() {
    let mut s = session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 100.0, 0.0).unwrap();
    s.tab_mut()
        .doc
        .edit::<(), drafftink_scene::StoreError>(|tx| {
            use drafftink_scene::SceneTx;
            tx.update(&a, |e| e.transform.angle = std::f64::consts::FRAC_PI_2)
        })
        .unwrap();
    let gone = testing::add_rect(&mut s, 5000.0, 5000.0, 10.0, 10.0).unwrap();
    s.tab_mut()
        .doc
        .edit::<(), drafftink_scene::StoreError>(|tx| {
            use drafftink_scene::SceneTx;
            tx.update(&gone, |e| e.deleted = true)
        })
        .unwrap();
    run(&mut s, "view.center", json!({"on": "content"}));
    // A 100x0 box turned a quarter turn about its centre (50, 0) spans x 50..50, y -50..50.
    assert!(close(s.view().scroll.x, 50.0) && close(s.view().scroll.y, 0.0), "{:?}", scroll(&s));
}

// ---- view.center ----

#[test]
fn center_moves_the_camera_and_keeps_the_zoom() {
    let mut s = session();
    run(&mut s, "view.set", json!({"scroll": [9, 9], "zoom": 2}));
    run(&mut s, "view.center", json!({}));
    assert_eq!(scroll(&s), (0.0, 0.0));
    run(&mut s, "view.center", json!({"on": "point", "point": [10, 20]}));
    assert_eq!(scroll(&s), (10.0, 20.0));
    assert_eq!(s.view().zoom, 2.0);
    let a = testing::add_rect(&mut s, 100.0, 100.0, 50.0, 50.0).unwrap();
    testing::select(&mut s, &[a]);
    run(&mut s, "view.center", json!({"on": "selection"}));
    assert_eq!(scroll(&s), (125.0, 125.0));
}

#[test]
fn center_refuses_an_empty_target() {
    let mut s = session();
    let r = refuse(&mut s, "view.center", json!({"on": "content"}));
    assert_eq!(r.code, "nothing_to_do");
    assert!(r.message.contains("the document is empty"), "{}", r.message);
    let r = refuse(&mut s, "view.center", json!({"on": "selection"}));
    assert!(r.message.contains("nothing is selected"), "{}", r.message);
    let r = refuse(&mut s, "view.center", json!({"on": "point"}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut s, "view.center", json!({"on": "moon"}));
    assert_eq!(r.code, "bad_args");
}

// ---- view.scroll_page ----

#[test]
fn scroll_page_moves_by_a_fraction_of_the_viewport() {
    let mut s = session();
    run(&mut s, "view.set", json!({"zoom": 2}));
    run(&mut s, "view.scroll_page", json!({"direction": "down"}));
    // 0.8 of 600 px at zoom 2 is 240 world units.
    assert!(close(s.view().scroll.y, 240.0), "{:?}", scroll(&s));
    run(&mut s, "view.scroll_page", json!({"direction": "right", "fraction": 0.5}));
    assert!(close(s.view().scroll.x, 200.0));
    run(&mut s, "view.scroll_page", json!({"direction": "up"}));
    run(&mut s, "view.scroll_page", json!({"direction": "left", "fraction": 0.5}));
    assert_eq!(scroll(&s).0, 0.0);
}

#[test]
fn scroll_page_refuses_a_bad_direction_or_fraction() {
    let mut s = session();
    assert_eq!(refuse(&mut s, "view.scroll_page", json!({"direction": "sideways"})).code, "bad_args");
    assert_eq!(refuse(&mut s, "view.scroll_page", json!({})).code, "bad_args");
    let r = refuse(&mut s, "view.scroll_page", json!({"direction": "up", "fraction": 0}));
    assert!(r.message.contains("fraction"), "{}", r.message);
}

// ---- view.set_viewport ----

#[test]
fn set_viewport_applies_to_every_tab() {
    let mut s = testing::headless_session();
    let id = s.alloc_tab_id();
    s.tabs.push(crate::state::Tab::new(id));
    let out = run(&mut s, "view.set_viewport", json!({"width": 1000, "height": 500, "scale_factor": 2}));
    assert_eq!(out["changed"], true);
    assert!(s.tabs.iter().all(|t| t.view.viewport.w == 1000.0 && t.view.viewport.scale_factor == 2.0));
    let out = run(&mut s, "view.set_viewport", json!({"width": 1000, "height": 500, "scale_factor": 2}));
    assert_eq!(out["changed"], false);
}

#[test]
fn set_viewport_needs_a_positive_size() {
    let mut s = session();
    for args in [json!({"width": 0, "height": 10}), json!({"width": 10, "height": -1}), json!({"width": 1e9, "height": 10}), json!({"width": 10})] {
        let r = refuse(&mut s, "view.set_viewport", args);
        assert_eq!(r.code, "bad_args");
    }
    assert_eq!(s.view().viewport.w, 800.0);
}

// ---- view.set_view_only ----

#[test]
fn view_only_toggles_and_gates_document_commands() {
    let mut s = session();
    assert_eq!(run(&mut s, "view.set_view_only", json!({}))["view_only"], true);
    assert!(s.view_only);
    assert_eq!(run(&mut s, "view.set_view_only", json!({}))["view_only"], false);
    let out = run(&mut s, "view.set_view_only", json!({"on": false}));
    assert_eq!(out["changed"], false);
    run(&mut s, "view.set_view_only", json!({"on": true}));
}

#[test]
fn view_only_rejects_a_non_boolean() {
    let mut s = session();
    let r = refuse(&mut s, "view.set_view_only", json!({"on": "yes"}));
    assert_eq!(r.code, "bad_args");
    assert!(!s.view_only);
}

// ---- view.get ----

#[test]
fn get_reports_the_camera_the_visible_rectangle_and_modes() {
    let mut s = session();
    run(&mut s, "view.set", json!({"scroll": [100, 50], "zoom": 2}));
    s.tool.lock = true;
    let v = run(&mut s, "view.get", json!({}));
    assert_eq!(v["changed"], false);
    assert_eq!(v["scroll"], json!([100.0, 50.0]));
    assert_eq!(v["viewport"], json!({"w": 800.0, "h": 600.0, "scale_factor": 1.0}));
    assert_eq!(v["visible"], json!({"x": -100.0, "y": -100.0, "w": 400.0, "h": 300.0}));
    assert_eq!(v["tool"], "select");
    assert_eq!(v["tool_lock"], true);
    assert_eq!(v["view_only"], false);
    assert!(close(v["percent"].as_f64().unwrap(), 2.0 / BASE_ZOOM * 100.0));
    assert_eq!(refuse(&mut s, "view.get", json!({"x": 1})).code, "bad_args");
}

// ---- view.screenshot ----

#[test]
fn screenshot_needs_the_renderer_and_says_so() {
    let mut s = testing::headless_session();
    let r = refuse(&mut s, "view.screenshot", json!({}));
    assert_eq!(r.code, "port_unavailable");
    assert_eq!(
        r.message,
        "`view.screenshot` needs the renderer, which this session does not provide (headless without a GPU). Read the structure with `doc.outline` instead."
    );
}

#[test]
fn screenshot_renders_a_region_through_the_port_and_changes_nothing() {
    let (mut s, fakes) = testing::session();
    run(&mut s, "view.set_viewport", json!({"width": 400, "height": 300}));
    testing::add(&mut s, ElementKind::Rectangle, 0.0, 0.0, 200.0, 100.0).unwrap();
    let before = s.fingerprint();
    let out = run(&mut s, "view.screenshot", json!({"region": "all", "scale": 2}));
    assert_eq!((out["width"].as_u64(), out["height"].as_u64()), (Some(400), Some(200)));
    assert_eq!(out["bytes"]["mime"], "image/png");
    assert!(out["bytes"]["base64"].as_str().unwrap().len() > 8);
    assert_eq!(fakes.renderer.calls(), 1);
    assert_eq!(s.fingerprint(), before);
    assert_eq!(s.tab().doc.undo_depth(), 0);

    let out = run(&mut s, "view.screenshot", json!({"max_side_px": 100, "region": "viewport"}));
    assert_eq!((out["width"].as_u64(), out["height"].as_u64()), (Some(100), Some(75)));
    let out = run(&mut s, "view.screenshot", json!({"region": {"x": 0, "y": 0, "w": 50, "h": 50}}));
    assert_eq!(out["width"], 50, "50 world units at scale 1");
}

#[test]
fn screenshot_refusals() {
    let (mut s, _) = testing::session();
    let r = refuse(&mut s, "view.screenshot", json!({"region": "all"}));
    assert_eq!(r.code, "nothing_to_do");
    let r = refuse(&mut s, "view.screenshot", json!({"region": "selection"}));
    assert_eq!(r.code, "nothing_to_do");
    let r = refuse(&mut s, "view.screenshot", json!({"max_side_px": 5}));
    assert!(r.message.contains("max_side_px"), "{}", r.message);
    let r = refuse(&mut s, "view.screenshot", json!({"region": {"x": 0, "y": 0, "w": 0, "h": 5}}));
    assert_eq!(r.code, "nothing_to_do");
    let r = refuse(&mut s, "view.screenshot", json!({"region": {"ids": ["ghost"]}}));
    assert_eq!(r.code, "not_found");
    let r = refuse(&mut s, "view.screenshot", json!({"scale": 100}));
    assert!(r.message.contains("scale"), "{}", r.message);
}

// ---- the ring and the dirty reason ----

#[test]
fn high_rate_view_commands_are_ring_quiet_and_set_the_camera_dirty_reason() {
    let mut s = session();
    let n = s.ring().len();
    run(&mut s, "view.pan", json!({"dx": 1, "dy": 1}));
    run(&mut s, "view.zoom_at", json!({"factor": 1.1}));
    run(&mut s, "view.set", json!({"zoom": 1}));
    assert_eq!(s.ring().len(), n, "quiet commands leave no trace on success");
    assert_eq!(s.take_dirty(), Some(crate::spec::Redraw::Camera));
    let _ = refuse(&mut s, "view.pan", json!({"dx": "x", "dy": 0}));
    assert_eq!(s.ring().len(), n + 1, "a refusal is recorded");
    run(&mut s, "view.zoom_in", json!({}));
    assert_eq!(s.ring().len(), n + 2, "view.zoom_in is an ordinary command");
    assert_eq!(s.ring().iter().last().unwrap().id, "view.zoom_in");
}

#[test]
fn view_commands_work_in_a_batch_and_roll_back_with_it() {
    use crate::batch::{BatchOpts, BatchStep};
    let mut s = session();
    let before = s.fingerprint();
    let steps = [
        BatchStep::new("view.set", json!({"zoom": 3})),
        BatchStep::new("view.pan", json!({"dx": 50, "dy": 0})),
        BatchStep::new("view.center", json!({"on": "content"})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.rolled_back && !out.ok(), "the last step has nothing to centre on");
    assert_eq!(out.failed.unwrap().refusal.code, "nothing_to_do");
    assert_eq!(s.fingerprint(), before, "the camera is part of what a batch rolls back");
}
