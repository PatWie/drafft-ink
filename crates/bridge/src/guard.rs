//! The checks every transport applies before it trusts a peer: the token, the `Host` header and the
//! `Origin` header. They are pure functions of strings, so they are tested without a socket.
//!
//! Why `Host` and `Origin`: the bridge speaks WebSocket, and a web page can open a WebSocket to
//! `127.0.0.1`. The token already stops it, but defence in depth costs nothing here. `Origin` tells us
//! which page is calling (a browser always sends it), and `Host` blocks DNS rebinding, where a hostile
//! name resolves to 127.0.0.1 and the browser treats our port as the page's own origin.

use subtle::ConstantTimeEq;

/// The shortest token the bridge accepts when the caller supplies its own.
pub const MIN_TOKEN_LEN: usize = 32;

/// Compares two tokens without stopping at the first difference. The length is not secret (tokens have a
/// fixed length).
pub fn tokens_match(given: &str, expected: &str) -> bool {
    given.len() == expected.len() && bool::from(given.as_bytes().ct_eq(expected.as_bytes()))
}

/// Why a caller-supplied token is refused, in plain words.
pub fn token_problem(token: &str) -> Option<String> {
    if token.len() < MIN_TOKEN_LEN {
        return Some(format!("a token needs at least {MIN_TOKEN_LEN} characters, and this one has {}", token.len()));
    }
    if !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Some("a token may only contain visible ASCII characters, without spaces".to_owned());
    }
    None
}

/// Why a connection was refused before the WebSocket started.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// The `Host` header is missing or names something other than this loopback port.
    #[error("the Host header `{0}` is not 127.0.0.1:{1} or localhost:{1}")]
    Host(String, u16),
    /// The `Origin` header names a page that may not use the bridge.
    #[error("the Origin `{0}` may not use the bridge")]
    Origin(String),
    /// The `Authorization` header is missing, is not `Bearer <token>`, or carries a wrong token.
    #[error("the Authorization header must be `Bearer <token>` with this launch's token")]
    Bearer,
}

/// Accepts `127.0.0.1:<port>` and `localhost:<port>` (any letter case) and nothing else. A missing header,
/// a missing port, another port, an IPv6 literal or any other name is refused.
pub fn check_host(host: Option<&str>, port: u16) -> Result<(), Refused> {
    let shown = host.unwrap_or("").to_owned();
    let Some(h) = host else { return Err(Refused::Host(shown, port)) };
    let h = h.trim().to_ascii_lowercase();
    if h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}") {
        Ok(())
    } else {
        Err(Refused::Host(shown, port))
    }
}

/// Accepts `Authorization: Bearer <token>` with the launch's token and nothing else. The scheme name is not
/// case sensitive (RFC 9110); the token is compared in constant time.
pub fn check_bearer(authorization: Option<&str>, token: &str) -> Result<(), Refused> {
    let value = authorization.ok_or(Refused::Bearer)?.trim();
    let (scheme, given) = value.split_once(' ').ok_or(Refused::Bearer)?;
    if scheme.eq_ignore_ascii_case("bearer") && tokens_match(given.trim(), token) {
        Ok(())
    } else {
        Err(Refused::Bearer)
    }
}

fn is_loopback_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")) else {
        return false;
    };
    let (host, port) = match rest.rfind(':') {
        Some(i) if !rest.ends_with(']') => (rest.get(..i).unwrap_or(""), rest.get(i + 1..)),
        _ => (rest, None),
    };
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    port_ok && matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// Accepts a request with no `Origin` (a program, not a page), an `Origin` equal to one of `allowed`
/// (the app's own origin), or a loopback origin (`http(s)://localhost`, `127.0.0.1` or `[::1]`, any port).
/// Everything else is refused, including `null`, which sandboxed frames and `file:` pages send.
pub fn check_origin(origin: Option<&str>, allowed: &[String]) -> Result<(), Refused> {
    let Some(o) = origin else { return Ok(()) };
    let o = o.trim().to_ascii_lowercase();
    if allowed.iter().any(|a| a.trim().trim_end_matches('/').eq_ignore_ascii_case(&o)) || is_loopback_origin(&o) {
        Ok(())
    } else {
        Err(Refused::Origin(o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_must_be_this_loopback_port() {
        assert!(check_host(Some("127.0.0.1:4000"), 4000).is_ok());
        assert!(check_host(Some("localhost:4000"), 4000).is_ok());
        assert!(check_host(Some("LocalHost:4000"), 4000).is_ok());
        for bad in [
            None,
            Some(""),
            Some("127.0.0.1"),
            Some("127.0.0.1:4001"),
            Some("evil.com:4000"),
            Some("127.0.0.1.evil.com:4000"),
            Some("[::1]:4000"),
            Some("0.0.0.0:4000"),
            Some("localhost.evil.com:4000"),
            Some("127.0.0.1:4000, evil.com"),
        ] {
            assert!(check_host(bad, 4000).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn origin_rules() {
        let own = vec!["https://drafft.ink".to_owned()];
        for ok in [
            None,
            Some("https://drafft.ink"),
            Some("https://DRAFFT.ink"),
            Some("http://localhost:8080"),
            Some("http://127.0.0.1:3030"),
            Some("https://localhost"),
            Some("http://[::1]:5173"),
        ] {
            assert!(check_origin(ok, &own).is_ok(), "{ok:?}");
        }
        for bad in [
            "null",
            "https://evil.com",
            "https://drafft.ink.evil.com",
            "http://localhost.evil.com",
            "http://127.0.0.1@evil.com",
            "http://127.0.0.1:80/evil",
            "file://",
            "http://localhost:abc",
            "ftp://localhost",
            "http://",
            "",
            "https://drafft.ink:8443",
            "http://evil.com:80#localhost",
        ] {
            assert!(check_origin(Some(bad), &own).is_err(), "{bad:?}");
        }
        assert!(check_origin(Some("https://drafft.ink"), &[]).is_err());
    }

    #[test]
    fn bearer_must_carry_the_token() {
        let t = "a".repeat(64);
        for ok in [format!("Bearer {t}"), format!("bearer {t}"), format!("BEARER  {t} ")] {
            assert!(check_bearer(Some(&ok), &t).is_ok(), "{ok:?}");
        }
        for bad in [
            None,
            Some(""),
            Some("Bearer"),
            Some("Bearer "),
            Some(&t[..]),
            Some("Basic YTph"),
            Some("Bearer wrong"),
            Some("Token aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ] {
            assert!(check_bearer(bad, &t).is_err(), "{bad:?}");
        }
        assert!(check_bearer(Some(&format!("Bearer {t}x")), &t).is_err());
    }

    #[test]
    fn token_comparison() {
        let t = "a".repeat(64);
        assert!(tokens_match(&t, &t));
        assert!(!tokens_match(&t, &"a".repeat(63)));
        assert!(!tokens_match(&format!("{}b", "a".repeat(63)), &t));
        assert!(!tokens_match("", &t));
        assert!(token_problem("short").is_some());
        assert!(token_problem(&"a b".repeat(20)).is_some());
        assert!(token_problem(&t).is_none());
    }
}
