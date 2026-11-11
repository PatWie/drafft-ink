//! Tests of the `gesture` commands over the real registry. The machinery (frames, one undo step,
//! cancel, watchdog, typed path) is tested with a synthetic gesture command in `executor_tests.rs`,
//! because no real command has `undo = Gesture` until the `element` family lands.

use serde_json::json;

use crate::batch::{BatchOpts, BatchStep};
use crate::ring::Origin;
use crate::testing;

#[test]
fn begin_names_the_commands_that_can_run_as_a_gesture() {
    let mut s = testing::headless_session();
    let r = s.execute("gesture.begin", json!({"command": "view.pan", "args": {"dx": 1, "dy": 1}})).unwrap_err();
    assert_eq!(r.code, "not_gesturable");
    assert!(r.message.starts_with("`view.pan` cannot run as a gesture. Gestures work with:"), "{}", r.message);
    assert_eq!(r.command, "gesture.begin");
    let r = s.execute("gesture.begin", json!({"command": "view.pann"})).unwrap_err();
    assert_eq!(r.code, "unknown_command");
    assert!(r.message.contains("`view.pan`"), "{}", r.message);
    assert_eq!(r.command, "gesture.begin");
    assert!(!s.gesture_running());
}

#[test]
fn update_end_and_cancel_without_a_gesture() {
    let mut s = testing::headless_session();
    let r = s.execute("gesture.update", json!({"gesture": "g1", "args": {}})).unwrap_err();
    assert_eq!(r.code, "no_gesture");
    assert_eq!(r.message, "There is no running gesture `g1`. Start one with `gesture.begin`.");
    assert_eq!(s.execute("gesture.end", json!({})).unwrap_err().code, "no_gesture");
    assert_eq!(s.execute("gesture.update", json!({"gesture": "nonsense"})).unwrap_err().code, "no_gesture");
    // A stray cancel (window blur) is normal.
    let out = s.execute("gesture.cancel", json!({})).unwrap();
    assert_eq!((out["changed"].as_bool(), out["cancelled"].as_bool()), (Some(false), Some(false)));
    s.execute("gesture.cancel", json!({"gesture": "g7"})).unwrap();
}

#[test]
fn gesture_commands_cannot_be_used_in_batches() {
    let mut s = testing::headless_session();
    let out = s.execute_batch(Origin::Test, &[BatchStep::new("gesture.cancel", json!({}))], &BatchOpts::default());
    let f = out.failed.unwrap();
    assert_eq!(f.refusal.code, "not_batchable");
    assert!(f.refusal.message.contains("Run it on its own"), "{}", f.refusal.message);
}

#[test]
fn gesture_commands_are_not_for_agents_or_palettes() {
    let s = testing::headless_session();
    for id in ["gesture.begin", "gesture.update", "gesture.end", "gesture.cancel"] {
        let spec = s.spec(id).unwrap();
        assert!(!spec.agent && !spec.palette, "{id}");
        assert!(spec.is_own_transaction(), "{id}");
    }
}

#[test]
fn refusals_of_gesture_commands_are_recorded_in_the_ring() {
    let mut s = testing::headless_session();
    let _ = s.execute("gesture.end", json!({}));
    let last = s.ring().iter().last().unwrap();
    assert_eq!((last.id.as_str(), last.outcome.code()), ("gesture.end", Some("no_gesture")));
}

/// The sequence behind the report "a drag is in progress" after the opacity slider: an open gesture refuses
/// every other command, and the commands report it as their availability, which greys the whole UI. Ending
/// the gesture lifts both, and the whole drag is one undo step.
#[test]
fn an_open_style_gesture_refuses_other_commands_until_it_ends() {
    let mut s = testing::headless_session();
    let id = testing::add_rect(&mut s, 0.0, 0.0, 100.0, 60.0).unwrap();
    testing::select(&mut s, std::slice::from_ref(&id));
    let depth = s.tab().doc.undo_depth();

    let begun = s.execute("gesture.begin", json!({"command": "style.set", "args": {"props": {"opacity": 0.9}}})).unwrap();
    let gesture = begun["gesture"].as_str().unwrap().to_owned();
    s.execute("gesture.update", json!({"gesture": gesture, "args": {"props": {"opacity": 0.5}}})).unwrap();

    let refused = s.execute("selection.select_at", json!({"point": [500, 500]})).unwrap_err();
    assert_eq!(refused.code, "gesture_active");
    assert_eq!(
        refused.message,
        "A drag is in progress. Finish it with `gesture.end` or cancel it with `gesture.cancel` before running `selection.select_at`."
    );
    let availability = s.registry().find("tool.set").map(|spec| s.availability(spec.id));
    assert!(availability.as_ref().is_some_and(|a| a.as_ref().is_err_and(|r| r.code == "gesture_active")), "{availability:?}");

    s.execute("gesture.end", json!({"gesture": gesture, "args": {"props": {"opacity": 0.5}}})).unwrap();
    assert!(!s.gesture_running());
    s.execute("selection.select_at", json!({"point": [500, 500]})).unwrap();
    assert_eq!(s.tab().doc.undo_depth(), depth + 1, "a whole drag is one undo step");
}
