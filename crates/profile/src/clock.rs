//! The one clock every module reads: nanoseconds since the first call in this process.
//!
//! `web_time::Instant` is `std::time::Instant` on native targets and `performance.now()` in the
//! browser, so spans and frames read the same clock on every target.

use std::sync::OnceLock;

// `web_time::Instant` is wasm-safe; clippy sees the std path it re-exports on native (clippy-below-l6).
#[allow(clippy::disallowed_types)]
use web_time::Instant;

#[allow(clippy::disallowed_types)]
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Nanoseconds since the process-wide epoch (the first call to this function).
// `web_time::Instant` is wasm-safe; clippy sees the std path it re-exports on native.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
pub fn now_ns() -> u64 {
    let epoch = EPOCH.get_or_init(Instant::now);
    u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Nanoseconds as milliseconds, rounded to whole microseconds (three decimals), so JSON
/// output is short and stable.
pub(crate) fn ns_to_ms(ns: u64) -> f64 {
    ((ns as f64) / 1e3).round() / 1e3
}
