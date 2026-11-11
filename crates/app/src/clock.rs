//! The clock port for both targets.

use drafftink_engine::ports::Clock;
use web_time::{Instant, SystemTime, UNIX_EPOCH};

/// Wall time from the system clock and monotonic time since the app started. Works in the browser
/// (`web_time` maps both to `Date.now` and `performance.now`).
#[derive(Debug, Clone, Copy)]
pub struct AppClock {
    start: Instant,
}

impl AppClock {
    /// A clock whose monotonic time starts at zero now.
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }
}

impl Default for AppClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for AppClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }

    fn mono_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}
