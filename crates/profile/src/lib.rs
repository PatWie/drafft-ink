//! Performance tooling for Drafft.ink (layer L0, no workspace dependencies, builds for wasm32).
//!
//! | You want | Use |
//! |---|---|
//! | Time a region, nested | [`span`], [`scope`] |
//! | Time a frame phase | [`phase`], [`in_phase`] with [`Phase`] |
//! | Mark a frame | [`begin_frame`], [`end_frame`] |
//! | Say why a redraw happens | [`redraw`] with a [`RedrawReason`] |
//! | Count work | [`count`], [`inc`], [`set_gauge`] with [`Counter`], [`Gauge`] |
//! | Assert on work in a test | [`WorkScope`] (no clock, per thread), [`snapshot_thread`] |
//! | Export | [`snapshot`] -> [`PerfStats`] JSON, [`chrome_trace_json`] |
//! | Gate | [`Budgets`], [`ScenarioReport`] |
//!
//! Spans, phases and frames record only while [`set_enabled`]`(true)` and only when the
//! `enabled` cargo feature (on by default) is compiled in. Counters and redraw reasons are
//! always on. `docs/performance.md` is the full guide.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![warn(missing_docs)]

mod clock;
mod frame;
mod live;
mod names;
mod report;
mod spans;
mod stats;
mod trace;
mod work;

pub use clock::now_ns;
pub use frame::{
    BUDGET_30FPS_NS, BUDGET_60FPS_NS, DEFAULT_WINDOW, FrameRecord, FrameRecorder, FrameStats, HISTOGRAM_BOUNDS_MS,
    Histogram, Summary,
};
pub use live::{PhaseGuard, begin_frame, end_frame, in_phase, phase, reset, set_frame_window, snapshot};
pub use names::{Counter, Gauge, Phase, RedrawReason, RedrawReasons};
pub use report::{
    BudgetError, Budgets, Limit, PERF_OUT_ENV, REPORT_SCHEMA, ScenarioReport, Violation, is_deterministic, scenario_key,
};
pub use spans::{COMPILED, OTHER, Span, enabled, flush_thread, scope, set_enabled, span};
pub use stats::{
    DocumentError, MAX_SPAN_ROWS, PerfStats, RedrawEvent, RedrawStats, SCHEMA_VERSION, STATS_SCHEMA, SpanStat,
};
pub use trace::{chrome_trace, chrome_trace_json, write_chrome_trace};
pub use work::{WorkScope, count, count_named, inc, redraw, set_gauge, snapshot_thread};

/// Turn recording on when the environment variable `name` is `1`, `true`, `on` or `yes`.
/// Returns whether it is on. Not available in the browser, which has no environment; the web
/// entry point calls [`set_enabled`] from `window.drafftink.perf`.
#[cfg(not(target_arch = "wasm32"))]
pub fn enable_from_env(name: &str) -> bool {
    let on = std::env::var(name).is_ok_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"));
    if on {
        set_enabled(true);
    }
    on
}

/// Serializes tests that touch the process-wide recorder (the run-time flag, the rings).
/// Tests that only use [`WorkScope`] or a [`FrameRecorder`] value need no lock.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
