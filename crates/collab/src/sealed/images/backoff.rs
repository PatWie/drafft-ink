//! When to ask again after a request for a picture failed.

/// The first wait: 1 second. Each further failure doubles it.
pub const IMAGE_RETRY_BASE_MS: u64 = 1_000;
/// The longest wait between two tries: 2 minutes.
pub const IMAGE_RETRY_MAX_MS: u64 = 120_000;

/// The wait after the `attempt`-th failure (0 for the first): 1, 2, 4, 8 ... seconds, at most 2 minutes. No jitter: a
/// client asks for one picture at most every wait, and the relay's own limit (429) spreads the rest.
pub(crate) fn image_backoff_ms(attempt: u32) -> u64 {
    IMAGE_RETRY_BASE_MS.saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX)).min(IMAGE_RETRY_MAX_MS)
}

/// The wait after a failure: what the relay asked for in `Retry-After` (at least the base wait, at most the cap), or
/// the backoff of this attempt.
pub(crate) fn retry_delay_ms(attempt: u32, retry_after_ms: Option<u64>) -> u64 {
    retry_after_ms.map_or_else(|| image_backoff_ms(attempt), |ms| ms.clamp(IMAGE_RETRY_BASE_MS, IMAGE_RETRY_MAX_MS))
}

/// When a failed request is tried again, and how many tries failed so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Retry {
    /// Failures so far.
    pub(super) attempt: u32,
    /// Not before this time.
    pub(super) not_before: u64,
}

impl Retry {
    /// A first try, due at once.
    pub(super) const fn at(not_before: u64) -> Self {
        Retry { attempt: 0, not_before }
    }

    /// The retry after one more failure at `now`.
    pub(super) fn failed(attempt: u32, now: u64, retry_after_ms: Option<u64>) -> Self {
        Retry {
            attempt: attempt.saturating_add(1),
            not_before: now.saturating_add(retry_delay_ms(attempt, retry_after_ms)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_doubles_from_one_second_to_two_minutes() {
        let waits: Vec<u64> = (0..10).map(image_backoff_ms).collect();
        assert_eq!(waits, [1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 120_000, 120_000, 120_000]);
        assert_eq!(image_backoff_ms(63), IMAGE_RETRY_MAX_MS);
        assert_eq!(image_backoff_ms(64), IMAGE_RETRY_MAX_MS);
        assert_eq!(image_backoff_ms(u32::MAX), IMAGE_RETRY_MAX_MS);
    }

    #[test]
    fn retry_after_wins_within_its_bounds() {
        assert_eq!(retry_delay_ms(0, None), 1_000);
        assert_eq!(retry_delay_ms(3, None), 8_000);
        assert_eq!(retry_delay_ms(0, Some(7_000)), 7_000);
        assert_eq!(retry_delay_ms(5, Some(7_000)), 7_000);
        assert_eq!(retry_delay_ms(0, Some(0)), 1_000);
        assert_eq!(retry_delay_ms(0, Some(u64::MAX)), IMAGE_RETRY_MAX_MS);
    }

    #[test]
    fn a_failure_counts_and_schedules() {
        assert_eq!(Retry::failed(0, 500, None), Retry { attempt: 1, not_before: 1_500 });
        assert_eq!(Retry::failed(2, 10_000, None), Retry { attempt: 3, not_before: 14_000 });
        assert_eq!(Retry::failed(u32::MAX, u64::MAX, None).attempt, u32::MAX);
    }
}
