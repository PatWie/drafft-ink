//! Tests of `perf.stats`.

use serde_json::{Value, json};

use crate::ring::Origin;
use crate::testing;

#[test]
fn stats_returns_the_profile_snapshot_with_its_schema_and_counters() {
    let mut s = testing::headless_session();
    let v = s.execute_from(Origin::Test, "perf.stats", json!({})).unwrap();
    assert_eq!(v["schema"], "drafftink.perf");
    assert_eq!(v["changed"], false);
    assert_eq!(v["since"], "start");
    assert!(v["counters"]["commands"].is_u64());
    assert!(v["redraws"]["by_reason"].is_object());
    assert!(v.get("trace").is_none(), "no trace unless asked");
}

#[test]
fn trace_is_included_on_request() {
    let mut s = testing::headless_session();
    let v = s.execute("perf.stats", json!({"trace": true})).unwrap();
    assert!(v["trace"]["traceEvents"].is_array());
}

#[test]
fn last_call_reports_only_what_happened_since_the_previous_call() {
    // Other tests of this process count commands too: this session reads its own thread only, so the
    // numbers below are exact however many tests run in parallel.
    let mut s = testing::headless_session();
    s.count_this_thread_only();
    let count = |v: &Value| v["counters"]["commands"].as_u64().unwrap();
    s.execute("perf.stats", json!({})).unwrap();
    for _ in 0..5 {
        s.execute("view.get", json!({})).unwrap();
    }
    let delta = s.execute("perf.stats", json!({"since": "last_call"})).unwrap();
    // The five view.get calls and the first perf.stats call.
    assert_eq!(count(&delta), 6);
    assert_eq!(delta["since"], "last_call");
    let all = s.execute("perf.stats", json!({"since": "start"})).unwrap();
    assert_eq!(count(&all), 8, "start: first perf.stats, 5 view.get, the last_call perf.stats, itself");
    // A second delta right away sees the one call that happened in between: the `since=start` call.
    let again = s.execute("perf.stats", json!({"since": "last_call"})).unwrap();
    assert_eq!(count(&again), 2, "the since=start call and the one before this");
}

#[test]
fn one_sessions_commands_do_not_show_in_another_threads_report() {
    let mut s = testing::headless_session();
    s.count_this_thread_only();
    let count = |v: &Value| v["counters"]["commands"].as_u64().unwrap();
    s.execute("perf.stats", json!({})).unwrap();
    std::thread::spawn(|| {
        let mut other = testing::headless_session();
        for _ in 0..10 {
            other.execute("view.get", json!({})).unwrap();
        }
    })
    .join()
    .unwrap();
    let delta = s.execute("perf.stats", json!({"since": "last_call"})).unwrap();
    assert_eq!(count(&delta), 1, "only the first perf.stats call of this thread");
}

#[test]
fn bad_arguments_are_refused() {
    let mut s = testing::headless_session();
    for args in [json!({"since": "yesterday"}), json!({"trace": 1}), json!({"verbose": true})] {
        assert_eq!(s.execute("perf.stats", args).unwrap_err().code, "bad_args");
    }
}

#[test]
fn delta_subtracts_counters_and_drops_idle_spans() {
    use drafftink_profile::PerfStats;
    let mut prev = PerfStats::empty();
    let mut now = PerfStats::empty();
    prev.counters.insert("commands".into(), 10);
    now.counters.insert("commands".into(), 17);
    now.redraws.total = 4;
    prev.redraws.total = 1;
    let d = crate::cmd::perf::delta(&now, &prev);
    assert_eq!(d.counters["commands"], 7);
    assert_eq!(d.redraws.total, 3);
    // Saturating: a reset between calls cannot make a counter negative.
    let d = crate::cmd::perf::delta(&prev, &now);
    assert_eq!(d.counters["commands"], 0);
}
