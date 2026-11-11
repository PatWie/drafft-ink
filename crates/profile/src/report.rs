//! Scenario reports and the budgets file (`perf/budgets.json`).
//!
//! A driver (native or web) runs one scenario, takes a [`PerfStats`] snapshot, adds what only
//! it can measure (process CPU, the GPU adapter, the commit) and writes a [`ScenarioReport`].
//! `cargo xtask perf` loads the budgets and calls [`Budgets::check`] on each report.

use std::collections::BTreeMap;
use std::fmt;

use thiserror::Error;

use serde::{Deserialize, Serialize};

use crate::stats::{DocumentError, PerfStats, SCHEMA_VERSION, check_header};

/// Value of the `schema` field of a [`ScenarioReport`].
pub const REPORT_SCHEMA: &str = "drafftink.perf.scenario";

/// The format version of the budgets file that [`Budgets::from_json`] reads.
const BUDGETS_VERSION: u32 = 1;

/// One scenario run: the in-app stats plus what the driver measured outside the app.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenarioReport {
    /// Always [`REPORT_SCHEMA`].
    pub schema: String,
    /// [`SCHEMA_VERSION`] of the producer.
    pub version: u32,
    /// Scenario name: `idle`, `pan`, `zoom`, `drag` or `hover`.
    pub scenario: String,
    /// Elements in the generated scene (1000, 10000, 100000), if the scenario has a scene.
    pub scene_size: Option<u64>,
    /// Free-form facts: `platform` (`native` or `web`), `adapter`, `commit`, `build`, `browser`.
    pub meta: BTreeMap<String, String>,
    /// Numbers measured by the driver, for example `cpu_pct` (percent of one core).
    pub external: BTreeMap<String, f64>,
    /// The app's own snapshot, taken after the scenario.
    pub stats: PerfStats,
}

impl ScenarioReport {
    /// A report with no meta or external numbers.
    pub fn new(scenario: &str, scene_size: Option<u64>, stats: PerfStats) -> Self {
        Self {
            schema: REPORT_SCHEMA.to_string(),
            version: SCHEMA_VERSION,
            scenario: scenario.to_string(),
            scene_size,
            meta: BTreeMap::new(),
            external: BTreeMap::new(),
            stats,
        }
    }

    /// A report of the work a [`crate::WorkScope`] saw, for a scenario that a test drove on the current thread.
    /// `meta.source` says so, and `meta.build` is `debug` or `release`.
    pub fn from_work(scenario: &str, scene_size: Option<u64>, work: &crate::WorkScope) -> Self {
        let mut report = Self::new(scenario, scene_size, work.stats());
        report.meta.insert("platform".into(), "native".into());
        report.meta.insert("source".into(), "work-counter test".into());
        report.meta.insert("build".into(), if cfg!(debug_assertions) { "debug" } else { "release" }.into());
        report
    }

    /// The key of this report in the budgets file: `pan@10k`, or `idle` without a scene size.
    pub fn key(&self) -> String {
        scenario_key(&self.scenario, self.scene_size)
    }

    /// A number by metric path: `external.<name>` or any path of [`PerfStats::metric`].
    pub fn metric(&self, path: &str) -> Option<f64> {
        match path.strip_prefix("external.") {
            Some(name) => self.external.get(name).copied(),
            None => self.stats.metric(path),
        }
    }

    /// Parse JSON written by [`serde_json::to_string`] of a report.
    pub fn from_json(json: &str) -> Result<Self, DocumentError> {
        const WHAT: &str = "ScenarioReport";
        let r: Self = serde_json::from_str(json).map_err(|source| DocumentError::Json { what: WHAT, source })?;
        check_header(WHAT, &r.schema, REPORT_SCHEMA, r.version)?;
        Ok(r)
    }

    /// Serialize as indented JSON.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Writes the report to `<dir>/<key>.json` when the environment variable [`PERF_OUT_ENV`] names a directory,
    /// and does nothing otherwise. Work-counter tests call it with the numbers they measured, so that
    /// `cargo xtask perf` can check them against `perf/budgets.json`. Returns the path written.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_if_requested(&self) -> std::io::Result<Option<std::path::PathBuf>> {
        let Some(dir) = std::env::var_os(PERF_OUT_ENV) else { return Ok(None) };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", self.key()));
        std::fs::write(&path, self.to_json_pretty())?;
        Ok(Some(path))
    }
}

/// The environment variable that names the directory [`ScenarioReport::save_if_requested`] writes to.
pub const PERF_OUT_ENV: &str = "DRAFFTINK_PERF_OUT";

/// `name@size` with the size as `1k`, `10k`, `100k`, `1m` when it is round, else the number.
pub fn scenario_key(scenario: &str, scene_size: Option<u64>) -> String {
    match scene_size {
        None => scenario.to_string(),
        Some(n) if n >= 1_000_000 && n % 1_000_000 == 0 => format!("{scenario}@{}m", n / 1_000_000),
        Some(n) if n >= 1_000 && n % 1_000 == 0 => format!("{scenario}@{}k", n / 1_000),
        Some(n) => format!("{scenario}@{n}"),
    }
}

/// Does this metric count work instead of measuring time? Deterministic metrics are counters,
/// gauges, redraw counts, span run counts and `frame.unexplained`. They give the same value on
/// every machine, so they gate everywhere. All others (`frame.*`, `phase.*`, `interval.*`,
/// `fps`, `frames`, span times, `external.*`) depend on the hardware and gate only on the
/// reference runner.
pub fn is_deterministic(metric: &str) -> bool {
    metric.starts_with("counter.")
        || metric.starts_with("gauge.")
        || metric.starts_with("redraws.")
        || metric == "frame.unexplained"
        || (metric.starts_with("span.") && metric.ends_with(".count"))
}

/// A limit on one metric. At least one of `max` and `min` is set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    /// The metric must be at most this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// The metric must be at least this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Why the limit is what it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The contents of `perf/budgets.json`: scenario key, then metric path, then limit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Budgets {
    /// Format version; currently 1 (`BUDGETS_VERSION`).
    pub version: u32,
    /// `scenarios["pan@10k"]["frame.p99_ms"] = {"max": 16.7}`.
    pub scenarios: BTreeMap<String, BTreeMap<String, Limit>>,
}

/// A budget file that cannot be used.
#[derive(Debug, Error)]
pub enum BudgetError {
    /// The text is not JSON, or does not have the fields of a budgets file.
    #[error("budgets: invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The file has a format version this reader does not know.
    #[error("budgets: version {found} is not supported (expected {BUDGETS_VERSION})")]
    Version {
        /// The `version` in the file.
        found: u32,
    },
    /// A limit sets neither `max` nor `min`.
    #[error("budgets: {scenario} {metric}: set \"max\" or \"min\"")]
    NoBound {
        /// The scenario key.
        scenario: String,
        /// The metric path.
        metric: String,
    },
    /// A limit is NaN or infinite.
    #[error("budgets: {scenario} {metric}: limits must be finite numbers")]
    NotFinite {
        /// The scenario key.
        scenario: String,
        /// The metric path.
        metric: String,
    },
    /// A limit has a `min` above its `max`, so no value can pass.
    #[error("budgets: {scenario} {metric}: min {min} is above max {max}")]
    EmptyRange {
        /// The scenario key.
        scenario: String,
        /// The metric path.
        metric: String,
        /// The `min` of the limit.
        min: f64,
        /// The `max` of the limit.
        max: f64,
    },
}

/// A metric that broke its limit, or that the report does not contain.
#[derive(Clone, Debug, PartialEq)]
pub struct Violation {
    /// The scenario key.
    pub scenario: String,
    /// The metric path.
    pub metric: String,
    /// What was measured; `None` when the report has no such metric.
    pub actual: Option<f64>,
    /// The limit that failed.
    pub limit: Limit,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let want = match (self.limit.min, self.limit.max) {
            (Some(lo), Some(hi)) => format!("between {lo} and {hi}"),
            (Some(lo), None) => format!("at least {lo}"),
            (None, Some(hi)) => format!("at most {hi}"),
            (None, None) => "unconstrained".to_string(),
        };
        match self.actual {
            Some(a) => write!(f, "{} {}: {a} but must be {want}", self.scenario, self.metric),
            None => write!(f, "{} {}: the report has no such metric (must be {want})", self.scenario, self.metric),
        }
    }
}

impl Budgets {
    /// Parse and validate a budgets file.
    pub fn from_json(json: &str) -> Result<Self, BudgetError> {
        let b: Self = serde_json::from_str(json)?;
        if b.version != BUDGETS_VERSION {
            return Err(BudgetError::Version { found: b.version });
        }
        for (scenario, metrics) in &b.scenarios {
            for (metric, limit) in metrics {
                let key = || (scenario.clone(), metric.clone());
                if limit.max.is_none() && limit.min.is_none() {
                    let (scenario, metric) = key();
                    return Err(BudgetError::NoBound { scenario, metric });
                }
                if [limit.max, limit.min].into_iter().flatten().any(|v| !v.is_finite()) {
                    let (scenario, metric) = key();
                    return Err(BudgetError::NotFinite { scenario, metric });
                }
                if let (Some(min), Some(max)) = (limit.min, limit.max)
                    && min > max
                {
                    let (scenario, metric) = key();
                    return Err(BudgetError::EmptyRange { scenario, metric, min, max });
                }
            }
        }
        Ok(b)
    }

    /// Does the file budget this scenario key?
    pub fn has(&self, key: &str) -> bool {
        self.scenarios.contains_key(key)
    }

    /// Check one report against the limits for its scenario key. A scenario with no entry has
    /// no violations (the gate lists unbudgeted scenarios separately). A limit on a metric the
    /// report lacks, or whose value is not a number, is a violation.
    pub fn check(&self, report: &ScenarioReport) -> Vec<Violation> {
        self.check_filtered(report, true)
    }

    /// Like [`Self::check`]; with `timing` false only the [`is_deterministic`] metrics are
    /// checked, for runners whose hardware is not the reference.
    pub fn check_filtered(&self, report: &ScenarioReport, timing: bool) -> Vec<Violation> {
        let key = report.key();
        let Some(metrics) = self.scenarios.get(&key) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (metric, limit) in metrics {
            if !timing && !is_deterministic(metric) {
                continue;
            }
            let actual = report.metric(metric).filter(|v| v.is_finite());
            let ok = actual.is_some_and(|a| limit.max.is_none_or(|hi| a <= hi) && limit.min.is_none_or(|lo| a >= lo));
            if !ok {
                out.push(Violation { scenario: key.clone(), metric: metric.clone(), actual, limit: limit.clone() });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> ScenarioReport {
        let mut stats = PerfStats::empty();
        stats.frames.frame_ms.p99_ms = 20.0;
        stats.counters.insert("elements_encoded".into(), 500);
        let mut r = ScenarioReport::new("pan", Some(10_000), stats);
        r.external.insert("cpu_pct".into(), 55.0);
        r
    }

    fn budgets(body: &str) -> Budgets {
        Budgets::from_json(&format!(r#"{{"version":1,"scenarios":{body}}}"#)).unwrap()
    }

    #[test]
    fn keys() {
        assert_eq!(scenario_key("idle", None), "idle");
        assert_eq!(scenario_key("pan", Some(1_000)), "pan@1k");
        assert_eq!(scenario_key("pan", Some(100_000)), "pan@100k");
        assert_eq!(scenario_key("pan", Some(2_000_000)), "pan@2m");
        assert_eq!(scenario_key("pan", Some(1_500)), "pan@1500");
        assert_eq!(scenario_key("pan", Some(7)), "pan@7");
        assert_eq!(report().key(), "pan@10k");
    }

    #[test]
    fn passing_and_failing_limits() {
        let b = budgets(
            r#"{"pan@10k":{"frame.p99_ms":{"max":16.7,"note":"60 fps"},"counter.elements_encoded":{"max":500,"min":500},
                "external.cpu_pct":{"max":100}}}"#,
        );
        let v = b.check(&report());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].metric, "frame.p99_ms");
        assert_eq!(v[0].actual, Some(20.0));
        assert_eq!(v[0].to_string(), "pan@10k frame.p99_ms: 20 but must be at most 16.7");
    }

    #[test]
    fn min_limits() {
        let b = budgets(r#"{"pan@10k":{"external.cpu_pct":{"min":60}}}"#);
        assert_eq!(b.check(&report())[0].to_string(), "pan@10k external.cpu_pct: 55 but must be at least 60");
    }

    #[test]
    fn a_limit_on_a_missing_metric_fails() {
        let b = budgets(r#"{"pan@10k":{"external.gpu_ms":{"max":5},"phase.nope.p50_ms":{"max":5}}}"#);
        let v = b.check(&report());
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|x| x.actual.is_none()));
        assert!(v[0].to_string().contains("no such metric"));
    }

    #[test]
    fn nan_measurements_fail() {
        let mut r = report();
        r.external.insert("cpu_pct".into(), f64::NAN);
        let b = budgets(r#"{"pan@10k":{"external.cpu_pct":{"max":100}}}"#);
        assert_eq!(b.check(&r).len(), 1);
    }

    #[test]
    fn unbudgeted_scenarios_have_no_violations() {
        let b = budgets(r#"{"zoom@10k":{"frame.p99_ms":{"max":1}}}"#);
        assert!(b.check(&report()).is_empty());
        assert!(b.has("zoom@10k") && !b.has("pan@10k"));
    }

    #[test]
    fn invalid_budget_files_are_rejected() {
        let bad = [
            ("not json", "invalid JSON"),
            (r#"{"version":2,"scenarios":{}}"#, "not supported"),
            (r#"{"version":1,"scenarios":{"a":{"m":{}}}}"#, "set \"max\" or \"min\""),
            (r#"{"version":1,"scenarios":{"a":{"m":{"min":5,"max":1}}}}"#, "above max"),
            (r#"{"version":1,"scenarios":{"a":{"m":{"max":"x"}}}}"#, "invalid JSON"),
        ];
        for (json, why) in bad {
            let e = Budgets::from_json(json).unwrap_err();
            assert!(e.to_string().contains(why), "{json}: {e}");
        }
    }

    #[test]
    fn report_round_trips_and_rejects_foreign_documents() {
        let r = report();
        assert_eq!(ScenarioReport::from_json(&r.to_json_pretty()).unwrap(), r);
        assert!(ScenarioReport::from_json(&PerfStats::empty().to_json()).is_err());
        assert!(ScenarioReport::from_json("{}").is_err());
    }

    #[test]
    fn deterministic_metrics_are_classified() {
        for m in [
            "counter.elements_encoded",
            "gauge.elements_total",
            "redraws.total",
            "redraws.timer",
            "frame.unexplained",
            "span.encode.count",
        ] {
            assert!(is_deterministic(m), "{m}");
        }
        for m in [
            "frame.p99_ms",
            "phase.encode.p50_ms",
            "interval.max_ms",
            "fps",
            "frames",
            "span.encode.self_ms",
            "external.cpu_pct",
        ] {
            assert!(!is_deterministic(m), "{m}");
        }
    }

    #[test]
    fn timing_limits_can_be_skipped_off_the_reference_runner() {
        let b = budgets(r#"{"pan@10k":{"frame.p99_ms":{"max":16.7},"counter.elements_encoded":{"max":10}}}"#);
        assert_eq!(b.check(&report()).len(), 2);
        let v = b.check_filtered(&report(), false);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].metric, "counter.elements_encoded");
    }

    #[test]
    fn budgets_round_trip() {
        let b = budgets(r#"{"idle":{"redraws.total":{"max":1}}}"#);
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(Budgets::from_json(&json).unwrap(), b);
    }
}
