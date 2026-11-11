//! Chrome trace-event export of the recent spans, frames and redraw requests.
//!
//! Open the file in `chrome://tracing` or <https://ui.perfetto.dev>. Spans sit on one track per
//! thread, frames on a "frames" track and redraw requests on a "redraws" track as instant
//! events. The data comes from bounded rings (see [`crate::TRACE_CAPACITY`]); `otherData.dropped`
//! says how many finished spans fell out of the ring.

use serde_json::{Value, json};

use crate::live::frame_records;
use crate::spans;
use crate::work::recent_redraws;

const PID: u32 = 1;
/// Track ids outside the thread-id range.
const FRAMES_TID: u32 = 1_000_000;
const REDRAWS_TID: u32 = 1_000_001;

fn us(ns: u64) -> f64 {
    ns as f64 / 1e3
}

fn meta(tid: u32, name: &str) -> Value {
    json!({"ph": "M", "name": "thread_name", "pid": PID, "tid": tid, "args": {"name": name}})
}

/// The trace as a JSON value (`{"traceEvents": [...], "displayTimeUnit": "ms", ...}`).
pub fn chrome_trace() -> Value {
    let data = spans::data();
    let mut events: Vec<Value> = Vec::with_capacity(data.events.len() + 16);
    events.push(json!({"ph": "M", "name": "process_name", "pid": PID, "tid": 0, "args": {"name": "drafftink"}}));
    for (tid, name) in &data.threads {
        events.push(meta(*tid, name));
    }
    events.push(meta(FRAMES_TID, "frames"));
    events.push(meta(REDRAWS_TID, "redraws"));
    for e in &data.events {
        events.push(json!({"ph": "X", "name": e.name, "cat": "span", "pid": PID, "tid": e.tid, "ts": us(e.start_ns), "dur": us(e.dur_ns)}));
    }
    for f in frame_records() {
        let reasons: Vec<&str> = f.reasons.iter().map(|r| r.name()).collect();
        events.push(json!({
            "ph": "X", "name": "frame", "cat": "frame", "pid": PID, "tid": FRAMES_TID,
            "ts": us(f.start_ns), "dur": us(f.total_ns), "args": {"reasons": reasons},
        }));
    }
    for (ns, reason) in recent_redraws() {
        events.push(json!({
            "ph": "i", "s": "t", "name": reason.name(), "cat": "redraw", "pid": PID, "tid": REDRAWS_TID, "ts": us(ns),
        }));
    }
    json!({"displayTimeUnit": "ms", "traceEvents": events, "otherData": {"dropped": data.dropped}})
}

/// The trace as a JSON string.
pub fn chrome_trace_json() -> String {
    chrome_trace().to_string()
}

/// Write the trace to `w`.
pub fn write_chrome_trace(mut w: impl std::io::Write) -> std::io::Result<()> {
    w.write_all(chrome_trace_json().as_bytes())
}

#[cfg(all(test, feature = "enabled"))]
mod tests {
    use super::*;
    use crate::test_lock;

    #[test]
    fn trace_has_spans_frames_redraws_and_thread_names() {
        let _l = test_lock();
        crate::reset();
        crate::set_enabled(true);
        crate::redraw(crate::RedrawReason::Command);
        crate::begin_frame();
        {
            let _outer = crate::span("tr_outer");
            let _inner = crate::span("tr_inner");
        }
        crate::end_frame();
        crate::set_enabled(false);

        let t = chrome_trace();
        let evs = t["traceEvents"].as_array().unwrap();
        let find = |name: &str| evs.iter().find(|e| e["name"] == name).unwrap_or_else(|| panic!("no event {name}"));
        let outer = find("tr_outer");
        let inner = find("tr_inner");
        assert_eq!(outer["ph"], "X");
        assert_eq!(outer["tid"], inner["tid"]);
        let (os, od) = (outer["ts"].as_f64().unwrap(), outer["dur"].as_f64().unwrap());
        let (is, id) = (inner["ts"].as_f64().unwrap(), inner["dur"].as_f64().unwrap());
        assert!(is >= os && is + id <= os + od + 1e-6);
        assert_eq!(find("frame")["args"]["reasons"][0], "command");
        let redraw = find("command");
        assert_eq!(redraw["ph"], "i");
        assert!(evs.iter().any(|e| e["ph"] == "M" && e["name"] == "thread_name" && e["args"]["name"] == "frames"));
        assert_eq!(t["otherData"]["dropped"], 0);
        assert_eq!(t["displayTimeUnit"], "ms");
    }

    #[test]
    fn writer_emits_valid_json() {
        let _l = test_lock();
        let mut buf = Vec::new();
        write_chrome_trace(&mut buf).unwrap();
        let v: Value = serde_json::from_slice(&buf).unwrap();
        assert!(v["traceEvents"].is_array());
    }
}
