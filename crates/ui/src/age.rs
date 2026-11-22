//! How long ago something happened, in words, and the date it happened on. Pure functions of milliseconds since the
//! Unix epoch: the clock is the session's (`Model::now_ms`), so a test that moves it sees the words change.

const SECOND: u64 = 1_000;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago", and from a week on the date ("12 Nov 2026"). A time
/// after `now` (the clocks of two peers differ) reads as "just now".
pub(crate) fn ago(now_ms: u64, then_ms: u64) -> String {
    let age = now_ms.saturating_sub(then_ms);
    match age {
        a if a < MINUTE => "just now".to_owned(),
        a if a < HOUR => format!("{} min ago", a / MINUTE),
        a if a < DAY => format!("{} h ago", a / HOUR),
        a if a < 2 * DAY => "yesterday".to_owned(),
        a if a < 7 * DAY => format!("{} days ago", a / DAY),
        _ => date(then_ms),
    }
}

/// "12 Nov 2026" in UTC.
pub(crate) fn date(ms: u64) -> String {
    let (year, month, day) = civil(ms / DAY);
    format!("{day} {} {year}", MONTHS.get(month.saturating_sub(1)).copied().unwrap_or("?"))
}

/// "2026-11-12 09:27 UTC": the exact time, for a tooltip.
pub(crate) fn exact(ms: u64) -> String {
    let (year, month, day) = civil(ms / DAY);
    let minutes = ms % DAY / MINUTE;
    format!("{year}-{month:02}-{day:02} {:02}:{:02} UTC", minutes / 60, minutes % 60)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// The civil date of a day count since 1970-01-01 (Howard Hinnant's `civil_from_days`, for days after the epoch).
fn civil(days: u64) -> (u64, usize, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, usize::try_from(month).unwrap_or(1), day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-22 09:27:36 UTC.
    const NOW: u64 = 1_795_339_656_000;

    #[test]
    fn the_exact_time_is_utc() {
        assert_eq!(exact(0), "1970-01-01 00:00 UTC");
        assert_eq!(exact(NOW), "2026-11-22 09:27 UTC");
        assert_eq!(exact(951_782_400_000), "2000-02-29 00:00 UTC", "a leap day");
        assert_eq!(date(NOW), "22 Nov 2026");
    }

    #[test]
    fn age_is_said_in_the_largest_fitting_unit() {
        assert_eq!(ago(NOW, NOW), "just now");
        assert_eq!(ago(NOW, NOW + 5 * MINUTE), "just now", "a clock that runs ahead");
        assert_eq!(ago(NOW, NOW - 59 * SECOND), "just now");
        assert_eq!(ago(NOW, NOW - MINUTE), "1 min ago");
        assert_eq!(ago(NOW, NOW - 59 * MINUTE), "59 min ago");
        assert_eq!(ago(NOW, NOW - HOUR), "1 h ago");
        assert_eq!(ago(NOW, NOW - 23 * HOUR), "23 h ago");
        assert_eq!(ago(NOW, NOW - DAY), "yesterday");
        assert_eq!(ago(NOW, NOW - 2 * DAY), "2 days ago");
        assert_eq!(ago(NOW, NOW - 6 * DAY), "6 days ago");
        assert_eq!(ago(NOW, NOW - 7 * DAY), "15 Nov 2026");
    }
}
