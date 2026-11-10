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
