//! `perf`: counters and timing from the `profile` crate (docs/commands.md §2.22).

use drafftink_profile::PerfStats;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::refusal::Refusal;
use crate::session::{PerfScope, Session};
use crate::spec::{CommandSpec, Facts, Typed};

/// What `perf.stats` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Since {
    /// Everything since the profiler started or was reset.
    #[default]
    Start,
    /// Only what happened after the previous `perf.stats` call of this session.
    LastCall,
}

/// Arguments of `perf.stats`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct StatsArgs {
    /// `start` (default) or `last_call`.
    pub since: Since,
    /// Add a Chrome-trace JSON of the retained spans under `trace`.
    pub trace: bool,
}

impl Default for StatsArgs {
    fn default() -> Self {
        Self { since: Since::Start, trace: false }
    }
}

/// The counters and histograms of the `profile` crate (`PerfStats`), and with `trace` the retained spans.
pub type StatsOut = Value;

fn stats(s: &mut Session, a: StatsArgs) -> Result<StatsOut, Refusal> {
    let now = match s.perf_scope {
        PerfScope::Process => drafftink_profile::snapshot(),
        PerfScope::Thread => drafftink_profile::snapshot_thread(),
    };
    let report = match (a.since, s.perf_mark.as_ref()) {
        (Since::LastCall, Some(prev)) => delta(&now, prev),
        _ => now.clone(),
    };
    if a.since == Since::LastCall || s.perf_mark.is_none() {
        s.perf_mark = Some(now);
    }
    let mut v = serde_json::to_value(&report).map_err(|e| Refusal::internal(s.cmd(), e.to_string()))?;
    if a.trace
        && let Value::Object(m) = &mut v
    {
        m.insert("trace".to_owned(), drafftink_profile::chrome_trace());
    }
    if let Value::Object(m) = &mut v {
        m.insert("changed".to_owned(), Value::Bool(false));
        m.insert("since".to_owned(), serde_json::to_value(a.since).unwrap_or(Value::Null));
    }
    Ok(v)
}

/// What happened between `prev` and `now`. Counters, redraws, frame counts and span counts are
/// differences; the frame-time window, gauges and maxima are the current readings.
pub(crate) fn delta(now: &PerfStats, prev: &PerfStats) -> PerfStats {
    let mut d = now.clone();
    for (k, v) in &mut d.counters {
        *v = v.saturating_sub(prev.counters.get(k).copied().unwrap_or(0));
    }
    d.redraws.total = now.redraws.total.saturating_sub(prev.redraws.total);
    for (k, v) in &mut d.redraws.by_reason {
        *v = v.saturating_sub(prev.redraws.by_reason.get(k).copied().unwrap_or(0));
    }
    d.redraws.recent.retain(|e| e.at_ms > prev.uptime_ms);
    d.frames.total = now.frames.total.saturating_sub(prev.frames.total);
    d.frames.over_16_7ms = now.frames.over_16_7ms.saturating_sub(prev.frames.over_16_7ms);
    d.frames.over_33_3ms = now.frames.over_33_3ms.saturating_sub(prev.frames.over_33_3ms);
    d.frames.unexplained = now.frames.unexplained.saturating_sub(prev.frames.unexplained);
    for (k, v) in &mut d.frames.by_reason {
        *v = v.saturating_sub(prev.frames.by_reason.get(k).copied().unwrap_or(0));
    }
    for sp in &mut d.spans {
        if let Some(p) = prev.spans.iter().find(|p| p.name == sp.name) {
            sp.count = sp.count.saturating_sub(p.count);
            sp.total_ms = (sp.total_ms - p.total_ms).max(0.0);
            sp.self_ms = (sp.self_ms - p.self_ms).max(0.0);
        }
    }
    d.spans.retain(|sp| sp.count > 0);
    d.spans_dropped = now.spans_dropped.saturating_sub(prev.spans_dropped);
    d
}

/// The perf family.
pub const COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "perf.stats",
    "perf",
    "Performance counters",
    "Returns the profiler's counters, per-phase histograms, redraw counts with their reasons and frame-time percentiles.",
    &Typed::new(stats),
    Facts::READ_ONLY,
)
.example(r#"{"since":"last_call"}"#)];

#[cfg(test)]
#[path = "perf_tests.rs"]
mod tests;
