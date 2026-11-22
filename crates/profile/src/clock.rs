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
    #[cfg(test)]
    if let Some(ns) = fake::read() {
        return ns;
    }
    let epoch = EPOCH.get_or_init(Instant::now);
    u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Nanoseconds as milliseconds, rounded to whole microseconds (three decimals), so JSON
/// output is short and stable.
pub(crate) fn ns_to_ms(ns: u64) -> f64 {
    ((ns as f64) / 1e3).round() / 1e3
}

/// A clock a test moves by hand, so a timing assertion is exact and cannot be disturbed by a busy machine.
#[cfg(test)]
pub(crate) mod fake {
    use std::cell::Cell;

    thread_local! {
        /// The fake time of this thread, or `None` to read the real clock. Per thread, so tests that run in
        /// parallel threads do not see each other's clock.
        static NOW_NS: Cell<Option<u64>> = const { Cell::new(None) };
    }

    pub(super) fn read() -> Option<u64> {
        NOW_NS.with(Cell::get)
    }

    /// Holds the calling thread on a fake clock at 0 ns until dropped (also when the test panics).
    #[must_use = "the fake clock lasts until this is dropped"]
    pub(crate) struct FakeClock;

    impl FakeClock {
        /// Switch this thread to a fake clock that starts at 0 ns and moves only with [`FakeClock::advance`].
        pub(crate) fn install() -> Self {
            NOW_NS.with(|t| t.set(Some(0)));
            Self
        }

        /// Move the clock forward by `ns`.
        pub(crate) fn advance(&self, ns: u64) {
            NOW_NS.with(|t| t.set(t.get().map(|now| now + ns)));
        }
    }

    impl Drop for FakeClock {
        fn drop(&mut self) {
            NOW_NS.with(|t| t.set(None));
        }
    }
}
