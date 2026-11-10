//! The `PerfStats` JSON schema, shared by the native and web drivers.
//!
//! The schema is documented in `docs/performance.md`. Consumers must ignore unknown fields.
//! Adding a field keeps [`SCHEMA_VERSION`]; renaming, removing or changing the meaning of a
//! field bumps it. A test pins the key set so a change cannot be accidental.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::clock::ns_to_ms;
use crate::frame::FrameStats;
use crate::names::{Counter, Gauge, RedrawReason};

/// Version of the `PerfStats` and `ScenarioReport` JSON.
pub const SCHEMA_VERSION: u32 = 1;
/// Value of the `schema` field of a [`PerfStats`].
pub const STATS_SCHEMA: &str = "drafftink.perf";

/// Spans listed in `PerfStats.spans`, most self time first.
pub const MAX_SPAN_ROWS: usize = 64;

/// A snapshot of everything the profiler knows. Returned by `profile::snapshot()` and by the
/// read-only `perf.stats` command; serialized as JSON for the drivers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerfStats {
    /// Always [`STATS_SCHEMA`].
    pub schema: String,
    /// [`SCHEMA_VERSION`] of the producer.
    pub version: u32,
    /// The timing code is compiled in (the `enabled` feature).
    pub compiled: bool,
    /// Spans, phases and frames are being recorded now.
    pub enabled: bool,
    /// Milliseconds since the process clock started.
    pub uptime_ms: f64,
    /// Frame time, phases, histogram and per-reason frame counts.
    pub frames: FrameStats,
    /// Redraw requests.
    pub redraws: RedrawStats,
    /// Monotonic work counters by name; every [`Counter`] is present, ad hoc ones are added.
    pub counters: BTreeMap<String, u64>,
    /// Last-value readings by name; every [`Gauge`] is present.
    pub gauges: BTreeMap<String, u64>,
    /// Span aggregates, most self time first.
    pub spans: Vec<SpanStat>,
    /// Finished spans that fell out of the trace ring.
    pub spans_dropped: u64,
}

/// Redraw requests since the last reset.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RedrawStats {
    /// All requests.
    pub total: u64,
    /// Requests by reason; every [`RedrawReason`] is present.
    pub by_reason: BTreeMap<String, u64>,
    /// The last requests (at most 256), oldest first.
    pub recent: Vec<RedrawEvent>,
}

/// One redraw request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RedrawEvent {
    /// Milliseconds on the process clock.
    pub at_ms: f64,
    /// The reason's name.
    pub reason: String,
}

/// Aggregate of one span name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpanStat {
    /// The span's name.
    pub name: String,
    /// Times it ran.
    pub count: u64,
    /// Time of the outermost occurrences, in ms.
    pub total_ms: f64,
    /// Time less the time of child spans, in ms.
    pub self_ms: f64,
    /// Longest single run, in ms.
    pub max_ms: f64,
}

impl PerfStats {
    /// Serialize as compact JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Serialize as indented JSON.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Parse JSON produced by [`Self::to_json`]. Unknown fields are ignored; a producer with a
    /// newer major `version` is rejected so a gate never reads a layout it does not know.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let stats: Self =
            serde_json::from_str(json).map_err(|e| format!("not a PerfStats document: {e}"))?;
        if stats.schema != STATS_SCHEMA {
            return Err(format!(
                "schema is \"{}\", expected \"{STATS_SCHEMA}\"",
                stats.schema
            ));
        }
        if stats.version > SCHEMA_VERSION {
            return Err(format!(
                "PerfStats version {} is newer than this reader ({SCHEMA_VERSION})",
                stats.version
            ));
        }
        Ok(stats)
    }

    /// An empty snapshot, as a process that recorded nothing would report.
    pub fn empty() -> Self {
        Self::assemble(
            FrameStats::default(),
            [0; Counter::COUNT],
            [0; Gauge::COUNT],
            [0; RedrawReason::COUNT],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn assemble(
        frames: FrameStats,
        counters: [u64; Counter::COUNT],
        gauges: [u64; Gauge::COUNT],
        redraws: [u64; RedrawReason::COUNT],
        named: Vec<(&'static str, u64)>,
        recent: Vec<(u64, RedrawReason)>,
        spans: Vec<(&'static str, crate::spans::Agg)>,
        spans_dropped: u64,
    ) -> Self {
        let mut counter_map: BTreeMap<String, u64> = Counter::ALL
            .iter()
            .map(|c| {
                (
                    c.name().to_string(),
                    counters.get(c.index()).copied().unwrap_or(0),
                )
            })
            .collect();
        for (name, n) in named {
            *counter_map.entry(name.to_string()).or_insert(0) += n;
        }
        let gauge_map = Gauge::ALL
            .iter()
            .map(|g| {
                (
                    g.name().to_string(),
                    gauges.get(g.index()).copied().unwrap_or(0),
                )
            })
            .collect();
        let by_reason: BTreeMap<String, u64> = RedrawReason::ALL
            .iter()
            .map(|r| {
                (
                    r.name().to_string(),
                    redraws.get(r.index()).copied().unwrap_or(0),
                )
            })
            .collect();
        let mut rows: Vec<SpanStat> = spans
            .into_iter()
            .map(|(name, a)| SpanStat {
                name: name.to_string(),
                count: a.count,
                total_ms: ns_to_ms(a.total_ns),
                self_ms: ns_to_ms(a.self_ns),
                max_ms: ns_to_ms(a.max_ns),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.self_ms
                .total_cmp(&a.self_ms)
                .then_with(|| a.name.cmp(&b.name))
        });
        rows.truncate(MAX_SPAN_ROWS);
        Self {
            schema: STATS_SCHEMA.to_string(),
            version: SCHEMA_VERSION,
            compiled: crate::COMPILED,
            enabled: crate::enabled(),
            uptime_ms: ns_to_ms(crate::clock::now_ns()),
            frames,
            redraws: RedrawStats {
                total: by_reason.values().sum(),
                by_reason,
                recent: recent
                    .into_iter()
                    .map(|(ns, r)| RedrawEvent {
                        at_ms: ns_to_ms(ns),
                        reason: r.name().to_string(),
                    })
                    .collect(),
            },
            counters: counter_map,
            gauges: gauge_map,
            spans: rows,
            spans_dropped,
        }
    }

    /// Look up one number by its metric path, the vocabulary of `perf/budgets.json`:
    ///
    /// - `frames`, `fps`
    /// - `frame.{mean,p50,p95,p99,max}_ms`, `frame.over_16_7ms`, `frame.over_33_3ms`,
    ///   `frame.unexplained`
    /// - `interval.{mean,p50,p95,p99,max}_ms`
    /// - `phase.<phase>.{mean,p50,p95,p99,max}_ms` (phase names plus `other`)
    /// - `redraws.total`, `redraws.<reason>`
    /// - `counter.<name>`, `gauge.<name>`
    /// - `span.<name>.{count,total_ms,self_ms,max_ms}`
    ///
    /// Returns `None` for a path that does not exist, so a budget on a misspelled or
    /// unreported metric fails instead of passing.
    pub fn metric(&self, path: &str) -> Option<f64> {
        fn summary(s: &crate::frame::Summary, stat: &str) -> Option<f64> {
            Some(match stat {
                "mean_ms" => s.mean_ms,
                "p50_ms" => s.p50_ms,
                "p95_ms" => s.p95_ms,
                "p99_ms" => s.p99_ms,
                "max_ms" => s.max_ms,
                _ => return None,
            })
        }
        let f = &self.frames;
        match path {
            "frames" => return Some(f.total as f64),
            "fps" => return Some(f.fps),
            "frame.over_16_7ms" => return Some(f.over_16_7ms as f64),
            "frame.over_33_3ms" => return Some(f.over_33_3ms as f64),
            "frame.unexplained" => return Some(f.unexplained as f64),
            "redraws.total" => return Some(self.redraws.total as f64),
            _ => {}
        }
        if let Some(stat) = path.strip_prefix("frame.") {
            return summary(&f.frame_ms, stat);
        }
        if let Some(stat) = path.strip_prefix("interval.") {
            return summary(&f.interval_ms, stat);
        }
        if let Some(rest) = path.strip_prefix("phase.") {
            let (name, stat) = rest.split_once('.')?;
            return summary(f.phase_ms.get(name)?, stat);
        }
        if let Some(name) = path.strip_prefix("redraws.") {
            return self.redraws.by_reason.get(name).map(|n| *n as f64);
        }
        if let Some(name) = path.strip_prefix("counter.") {
            return self.counters.get(name).map(|n| *n as f64);
        }
        if let Some(name) = path.strip_prefix("gauge.") {
            return self.gauges.get(name).map(|n| *n as f64);
        }
        if let Some(rest) = path.strip_prefix("span.") {
            let (name, stat) = rest.rsplit_once('.')?;
            let s = self.spans.iter().find(|s| s.name == name)?;
            return match stat {
                "count" => Some(s.count as f64),
                "total_ms" => Some(s.total_ms),
                "self_ms" => Some(s.self_ms),
                "max_ms" => Some(s.max_ms),
                _ => None,
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_set(v: &serde_json::Value) -> Vec<String> {
        let mut k: Vec<String> = v
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        k.sort();
        k
    }

    #[test]
    fn schema_v1_key_set_is_pinned() {
        // Changing any list below changes the contract with the drivers and the gate:
        // follow the rules in docs/performance.md ("Schema versioning") and update the doc.
        let v = serde_json::to_value(PerfStats::empty()).unwrap();
        assert_eq!(
            key_set(&v),
            [
                "compiled",
                "counters",
                "enabled",
                "frames",
                "gauges",
                "redraws",
                "schema",
                "spans",
                "spans_dropped",
                "uptime_ms",
                "version"
            ]
        );
        assert_eq!(
            key_set(&v["frames"]),
            [
                "by_reason",
                "fps",
                "frame_ms",
                "histogram",
                "interval_ms",
                "over_16_7ms",
                "over_33_3ms",
                "phase_ms",
                "total",
                "unexplained",
                "window"
            ]
        );
        assert_eq!(
            key_set(&v["frames"]["frame_ms"]),
            ["count", "max_ms", "mean_ms", "p50_ms", "p95_ms", "p99_ms"]
        );
        assert_eq!(key_set(&v["frames"]["histogram"]), ["counts", "upper_ms"]);
        assert_eq!(key_set(&v["redraws"]), ["by_reason", "recent", "total"]);
        assert_eq!(v["schema"], "drafftink.perf");
        assert_eq!(v["version"], 1);
    }

    #[test]
    fn empty_lists_every_counter_gauge_reason_and_phase() {
        let s = PerfStats::empty();
        for c in Counter::ALL {
            assert_eq!(s.counters.get(c.name()), Some(&0), "{}", c.name());
        }
        for g in Gauge::ALL {
            assert_eq!(s.gauges.get(g.name()), Some(&0));
        }
        for r in RedrawReason::ALL {
            assert_eq!(s.redraws.by_reason.get(r.name()), Some(&0));
        }
        assert_eq!(s.metric("redraws.total"), Some(0.0));
    }

    #[test]
    fn json_round_trips() {
        let s = PerfStats::empty();
        assert_eq!(PerfStats::from_json(&s.to_json()).unwrap(), s);
        assert_eq!(PerfStats::from_json(&s.to_json_pretty()).unwrap(), s);
    }

    #[test]
    fn reader_ignores_unknown_fields_but_rejects_newer_versions() {
        let mut v = serde_json::to_value(PerfStats::empty()).unwrap();
        v["added_later"] = serde_json::json!({"x": 1});
        assert!(PerfStats::from_json(&v.to_string()).is_ok());
        v["version"] = serde_json::json!(SCHEMA_VERSION + 1);
        assert!(
            PerfStats::from_json(&v.to_string())
                .unwrap_err()
                .contains("newer")
        );
        v["version"] = serde_json::json!(SCHEMA_VERSION);
        v["schema"] = serde_json::json!("something.else");
        assert!(
            PerfStats::from_json(&v.to_string())
                .unwrap_err()
                .contains("schema")
        );
        assert!(PerfStats::from_json("[]").is_err());
    }

    #[test]
    fn metric_paths() {
        let mut s = PerfStats::empty();
        s.frames.frame_ms.p99_ms = 12.5;
        s.frames.phase_ms.insert(
            "encode".into(),
            crate::frame::Summary {
                p95_ms: 4.0,
                ..Default::default()
            },
        );
        s.counters.insert("elements_encoded".into(), 42);
        s.gauges.insert("elements_total".into(), 1000);
        s.redraws.by_reason.insert("hover_change".into(), 3);
        s.spans.push(SpanStat {
            name: "hit_test".into(),
            count: 9,
            total_ms: 1.5,
            self_ms: 1.0,
            max_ms: 0.5,
        });
        assert_eq!(s.metric("frame.p99_ms"), Some(12.5));
        assert_eq!(s.metric("phase.encode.p95_ms"), Some(4.0));
        assert_eq!(s.metric("counter.elements_encoded"), Some(42.0));
        assert_eq!(s.metric("gauge.elements_total"), Some(1000.0));
        assert_eq!(s.metric("redraws.hover_change"), Some(3.0));
        assert_eq!(s.metric("span.hit_test.count"), Some(9.0));
        assert_eq!(s.metric("span.hit_test.self_ms"), Some(1.0));
        for bad in [
            "",
            "frame",
            "frame.p42_ms",
            "phase.nope.p50_ms",
            "phase.encode",
            "counter.nope",
            "span.nope.count",
            "span.hit_test.nope",
            "x",
        ] {
            assert_eq!(s.metric(bad), None, "{bad}");
        }
    }
}
