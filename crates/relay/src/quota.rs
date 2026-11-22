//! The daily capacity of a host. A host with a daily limit (the free Cloudflare plan) fails every store call with
//! [`StoreError::DailyLimit`](crate::StoreError::DailyLimit) once the day's capacity is used up, until 00:00 UTC.

/// Milliseconds in a UTC day.
const DAY_MS: u32 = 86_400_000;

/// The time from `now_ms` (milliseconds since the Unix epoch) to the next 00:00 UTC, in `1..=86_400_000`. Exactly at
/// midnight it is a whole day, never 0: a client told to wait that long does not retry in a loop.
pub const fn ms_until_utc_midnight(now_ms: u64) -> u32 {
    // `now_ms % DAY_MS` is below 86 400 000, so it fits in `u32`.
    let into_day = (now_ms % DAY_MS as u64) as u32;
    DAY_MS - into_day
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3_600_000;
    const DAY: u64 = 24 * HOUR;

    #[test]
    fn at_midnight_a_whole_day_is_left() {
        assert_eq!(ms_until_utc_midnight(0), 86_400_000);
        assert_eq!(ms_until_utc_midnight(5 * DAY), 86_400_000);
    }

    #[test]
    fn one_millisecond_before_midnight_one_millisecond_is_left() {
        assert_eq!(ms_until_utc_midnight(DAY - 1), 1);
        assert_eq!(ms_until_utc_midnight(7 * DAY + DAY - 1), 1);
    }

    #[test]
    fn mid_day_counts_to_the_next_midnight() {
        assert_eq!(ms_until_utc_midnight(12 * HOUR), 12 * HOUR as u32);
        assert_eq!(ms_until_utc_midnight(19 * HOUR + 55 * 60_000), (4 * HOUR + 5 * 60_000) as u32);
    }

    #[test]
    fn the_day_boundary_starts_a_new_day() {
        assert_eq!(ms_until_utc_midnight(DAY + 1), 86_399_999);
        assert_eq!(ms_until_utc_midnight(u64::MAX) as u64, DAY - u64::MAX % DAY);
    }
}
