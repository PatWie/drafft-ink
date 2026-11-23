//! Reading the answer of `GET /api/status`, which the web transport asks after a socket that never opened.
//!
//! A browser reports a failed `WebSocket` upgrade only as a close with code 1006: no status, no body. The server's own
//! status address tells the reasons apart (`docs/design/collab-e2e.md` section 11.6). The reading is a pure function so
//! that it is tested without a browser.

use crate::transport::Unavailable;

/// What the answer to `GET /api/status` says about a server whose socket never opened. `None`: it says nothing that
/// changes the plain "cannot reach the server" (a healthy server that refused this one connection, or any answer this
/// function does not know). A sign-in that ended is read by [`crate::AccessDenial::of_reply`], not here.
pub(crate) fn unavailable_of(status: u16, body: &[u8]) -> Option<Unavailable> {
    match status {
        429 | 503 => daily_limit(body),
        // A web host without a relay has no such route: GitHub Pages answers 404, another static host 404 or 405.
        404 | 405 | 501 => Some(Unavailable::NoRelay),
        _ => None,
    }
}

fn daily_limit(body: &[u8]) -> Option<Unavailable> {
    let value: serde_json::Value = serde_json::from_slice(body.get(..body.len().min(4096))?).ok()?;
    (value.get("error")?.as_str()? == "daily_limit").then(|| Unavailable::DailyLimit {
        retry_after_ms: value
            .get("retry_after_s")
            .and_then(serde_json::Value::as_u64)
            .and_then(|s| u32::try_from(s.saturating_mul(1000)).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_without_the_route_has_no_relay() {
        for status in [404, 405, 501] {
            assert_eq!(unavailable_of(status, b"<html>Not found</html>"), Some(Unavailable::NoRelay), "{status}");
        }
    }

    #[test]
    fn the_daily_limit_comes_with_the_time_to_wait() {
        let body = br#"{"error":"daily_limit","retry_after_s":3600}"#;
        assert_eq!(unavailable_of(429, body), Some(Unavailable::DailyLimit { retry_after_ms: Some(3_600_000) }));
        assert_eq!(
            unavailable_of(503, br#"{"error":"daily_limit"}"#),
            Some(Unavailable::DailyLimit { retry_after_ms: None })
        );
    }

    #[test]
    fn a_healthy_or_unknown_answer_changes_nothing() {
        assert_eq!(unavailable_of(200, br#"{"status":"ok"}"#), None);
        assert_eq!(unavailable_of(200, b"<!doctype html>"), None);
        assert_eq!(unavailable_of(503, b"<html>maintenance</html>"), None);
        assert_eq!(unavailable_of(500, b""), None);
        assert_eq!(unavailable_of(429, br#"{"error":"other"}"#), None);
    }
}
