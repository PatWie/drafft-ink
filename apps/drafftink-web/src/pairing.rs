//! The pure rules of pairing a page with the agent relay (docs/agents.md, section 5.2): which addresses are
//! allowed, how the pairing link in the page address is read and removed, the reconnect schedule, and the
//! frames. Nothing here touches the browser, so all of it is unit-tested natively.

use drafftink_bridge::rpc::{self, Incoming, UNAUTHORIZED};
use serde_json::{Value, json};

/// The relay's loopback-only rule, in words the user can act on.
pub const LOOPBACK_ONLY: &str = "The agent bridge only listens on this machine (loopback). \
    Use an address like ws://127.0.0.1:<port>, ws://localhost:<port> or ws://[::1]:<port>.";

/// The fragment key that carries the relay port.
pub const PORT_KEY: &str = "drafftink-agent-port";
/// The fragment key that carries the pairing token.
pub const TOKEN_KEY: &str = "drafftink-agent-token";

/// The close code a relay uses for a wrong or used token.
pub const CLOSE_POLICY: u16 = 1008;
/// The first reconnect delay.
const FIRST_DELAY_MS: u64 = 1_000;
/// The longest wait between two reconnect attempts.
const MAX_DELAY_MS: u64 = 30_000;
/// How long reconnecting goes on after the connection was lost.
pub const RECONNECT_WINDOW_MS: u64 = 10 * 60 * 1000;

/// Why a pairing request was refused before any socket opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    /// The address is not a loopback `ws://` address.
    #[error("{LOOPBACK_ONLY}")]
    NotLoopback,
    /// The address could not be read at all.
    #[error("The agent address is not valid: {0}. Use an address like ws://127.0.0.1:<port>")]
    BadUrl(&'static str),
    /// The token is not 64 hex characters.
    #[error("The token must be 64 hexadecimal characters, as printed by `drafftink-mcp --relay`.")]
    BadToken,
}

/// A relay address that passed [`check_agent_url`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentUrl {
    /// The address in a normal form: `ws://host[:port]/path`.
    pub url: String,
    /// The port (80 when the address names none).
    pub port: u16,
}

/// Checks a relay address. Only `ws://` to `127.0.0.1`, `localhost` or `[::1]` passes: a link must never be
/// able to point the page at another host.
pub fn check_agent_url(url: &str) -> Result<AgentUrl, PairingError> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://").ok_or(PairingError::BadUrl("it has no ws:// start"))?;
    if !scheme.eq_ignore_ascii_case("ws") {
        // wss:// to loopback is not served by the relay, and any other scheme is not a relay address.
        return Err(if scheme.eq_ignore_ascii_case("wss") || scheme.eq_ignore_ascii_case("http") {
            PairingError::NotLoopback
        } else {
            PairingError::BadUrl("it must start with ws://")
        });
    }
    if rest.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(PairingError::BadUrl("it contains spaces or control characters"));
    }
    if rest.contains('#') {
        return Err(PairingError::BadUrl("it must not contain a # part"));
    }
    let (authority, tail) = match rest.find(['/', '?']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err(PairingError::NotLoopback);
    }
    let (host, port) = split_authority(authority)?;
    let host = host.to_ascii_lowercase();
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
        return Err(PairingError::NotLoopback);
    }
    let path = if tail.is_empty() { "/" } else { tail };
    let path = if path.starts_with('/') { path.to_owned() } else { format!("/{path}") };
    let url = match port {
        Some(p) => format!("ws://{host}:{p}{path}"),
        None => format!("ws://{host}{path}"),
    };
    Ok(AgentUrl { url, port: port.unwrap_or(80) })
}

fn split_authority(authority: &str) -> Result<(&str, Option<u16>), PairingError> {
    let bad_port = PairingError::BadUrl("the port is not a number from 1 to 65535");
    let (host, port) = if let Some(end) = authority.find(']') {
        let (host, after) = authority.split_at(end + 1);
        (host, after.strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    let port = match port {
        None => None,
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n != 0 => Some(n),
            _ => return Err(bad_port),
        },
    };
    Ok((host, port))
}

/// True for 64 hexadecimal characters.
pub fn is_token(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The address the relay listens on for a port: the page builds it itself, so a link cannot name a host.
pub fn relay_url(port: u16) -> String {
    format!("ws://127.0.0.1:{port}/")
}

/// A pairing found in the page address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashPairing {
    /// The relay port.
    pub port: u16,
    /// The one-time pairing token.
    pub token: String,
}

/// Reads the pairing out of a fragment (`location.hash`, with or without `#`) and returns the fragment
/// without the two keys, without a leading `#`. Both keys are removed even when the pairing is unusable, so
/// a token never stays in the address bar or the history.
pub fn take_pairing(hash: &str) -> (Option<HashPairing>, String) {
    let hash = hash.strip_prefix('#').unwrap_or(hash);
    let (mut port, mut token) = (None, None);
    let mut kept = Vec::new();
    for part in hash.split('&').filter(|p| !p.is_empty()) {
        match part.split_once('=') {
            Some((PORT_KEY, v)) => port = v.parse::<u16>().ok().filter(|p| *p != 0),
            Some((TOKEN_KEY, v)) => token = Some(v.to_owned()),
            _ => kept.push(part),
        }
    }
    let pairing = match (port, token) {
        (Some(port), Some(token)) if is_token(&token) => Some(HashPairing { port, token }),
        _ => None,
    };
    (pairing, kept.join("&"))
}

/// The reconnect schedule: 1 s, 2 s, 4 s, ... up to 30 s between attempts, for 10 minutes after the first
/// failure. The caller passes the time; this holds no clock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backoff {
    attempts: u32,
    first_failure_ms: Option<u64>,
}

impl Backoff {
    /// A schedule that has not failed yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// A connection worked: start over.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The wait before the next attempt after a failure at `now_ms`, or `None` when the window is over.
    pub fn next_delay_ms(&mut self, now_ms: u64) -> Option<u64> {
        let first = *self.first_failure_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(first) >= RECONNECT_WINDOW_MS {
            return None;
        }
        let delay = FIRST_DELAY_MS.saturating_mul(1u64 << self.attempts.min(5)).min(MAX_DELAY_MS);
        self.attempts = self.attempts.saturating_add(1);
        Some(delay)
    }

    /// Failed attempts since the last success.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

/// The first frame: `hello` with the token and what this page is.
pub fn hello_frame(token: &str, version: &str, registry_count: usize, registry_digest: &str) -> String {
    rpc::request_text(
        &json!(1),
        "hello",
        &json!({
            "token": token,
            "page": { "version": version, "registry": { "count": registry_count, "digest": registry_digest } },
        }),
    )
}

/// What the relay answered to `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelloAnswer {
    /// Paired. The token to use on a reconnect.
    Welcome {
        /// The new resume token.
        resume_token: String,
    },
    /// The relay refused the token. Pairing again needs a new link.
    Unauthorized,
    /// Another error, with the relay's words.
    Failed(String),
}

/// Reads the relay's answer to `hello`.
pub fn parse_hello_answer(frame: &str) -> HelloAnswer {
    match rpc::parse(frame) {
        Ok(Incoming::Response { outcome: Ok(result), .. }) => {
            match result.get("resume_token").and_then(Value::as_str) {
                Some(t) if is_token(t) => HelloAnswer::Welcome { resume_token: t.to_owned() },
                _ => HelloAnswer::Failed("the relay did not send a resume token".to_owned()),
            }
        }
        Ok(Incoming::Response { outcome: Err(e), .. }) if e.code == UNAUTHORIZED => HelloAnswer::Unauthorized,
        Ok(Incoming::Response { outcome: Err(e), .. }) => HelloAnswer::Failed(e.message),
        _ => HelloAnswer::Failed("the relay did not answer hello".to_owned()),
    }
}

/// What a page remembers for the tab (`sessionStorage`): the port and the token to resume with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    /// The relay port.
    pub port: u16,
    /// The resume token.
    pub resume_token: String,
}

impl Remembered {
    /// The text to store.
    pub fn to_text(&self) -> String {
        json!({ "port": self.port, "resume_token": self.resume_token }).to_string()
    }

    /// Reads stored text. Anything that is not exactly a port and a token is ignored.
    pub fn from_text(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        let port = u16::try_from(v.get("port")?.as_u64()?).ok().filter(|p| *p != 0)?;
        let resume_token = v.get("resume_token")?.as_str()?.to_owned();
        is_token(&resume_token).then_some(Self { port, resume_token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn loopback_ws_addresses_pass() {
        let ok = |u: &str| check_agent_url(u).unwrap().url;
        assert_eq!(ok("ws://127.0.0.1:4555/"), "ws://127.0.0.1:4555/");
        assert_eq!(ok("ws://127.0.0.1:4555"), "ws://127.0.0.1:4555/");
        assert_eq!(ok("WS://LOCALHOST:80/x"), "ws://localhost:80/x");
        assert_eq!(ok("ws://[::1]:9000/"), "ws://[::1]:9000/");
        assert_eq!(ok("  ws://localhost/ "), "ws://localhost/");
    }

    #[test]
    fn everything_else_is_refused_with_the_loopback_message() {
        for url in [
            "ws://example.com:4555/",
            "ws://127.0.0.1.evil.com:4555/",
            "ws://localhost.evil.com/",
            "ws://127.0.0.1@evil.com/",
            "ws://evil.com@127.0.0.1/",
            "ws://192.168.1.4:4555/",
            "ws://0.0.0.0:4555/",
            "ws://127.0.0.2:4555/",
            "wss://127.0.0.1:4555/",
            "http://127.0.0.1:4555/",
        ] {
            let e = check_agent_url(url).unwrap_err();
            assert_eq!(e, PairingError::NotLoopback, "{url}");
            assert!(e.to_string().contains("only listens on this machine"), "{url}");
        }
    }

    #[test]
    fn malformed_addresses_are_refused_in_plain_words() {
        for url in [
            "",
            "127.0.0.1:4555",
            "ftp://127.0.0.1/",
            "ws://127.0.0.1:0/",
            "ws://127.0.0.1:99999/",
            "ws://127.0.0.1:x/",
            "ws://127.0.0.1/ a",
            "ws://127.0.0.1/#frag",
        ] {
            assert!(matches!(check_agent_url(url), Err(PairingError::BadUrl(_))), "{url}");
        }
    }

    #[test]
    fn tokens_are_64_hex() {
        assert!(is_token(TOKEN));
        assert!(!is_token(&TOKEN[1..]));
        assert!(!is_token(&format!("{TOKEN}0")));
        assert!(!is_token(&TOKEN.replace('a', "g")));
        assert!(!is_token(""));
    }

    #[test]
    fn the_relay_address_is_built_from_the_port_only() {
        assert_eq!(relay_url(4555), "ws://127.0.0.1:4555/");
        assert_eq!(check_agent_url(&relay_url(1)).unwrap(), AgentUrl { url: relay_url(1), port: 1 });
        assert_eq!(check_agent_url("ws://localhost/").unwrap().port, 80);
    }

    #[test]
    fn the_pairing_is_read_and_removed_from_the_fragment() {
        let hash = format!("#room=abc&drafftink-agent-port=4555&drafftink-agent-token={TOKEN}&x=1");
        let (pairing, rest) = take_pairing(&hash);
        assert_eq!(pairing, Some(HashPairing { port: 4555, token: TOKEN.to_owned() }));
        assert_eq!(rest, "room=abc&x=1");
        let only = format!("drafftink-agent-token={TOKEN}&drafftink-agent-port=7");
        assert_eq!(take_pairing(&only), (Some(HashPairing { port: 7, token: TOKEN.to_owned() }), String::new()));
    }

    #[test]
    fn a_half_or_bad_pairing_is_removed_but_not_used() {
        let (p, rest) = take_pairing("#drafftink-agent-port=4555&a=b");
        assert_eq!((p, rest.as_str()), (None, "a=b"));
        let (p, rest) = take_pairing(&format!("#drafftink-agent-token={TOKEN}"));
        assert_eq!((p, rest.as_str()), (None, ""));
        let (p, rest) = take_pairing("#drafftink-agent-port=0&drafftink-agent-token=short");
        assert_eq!((p, rest.as_str()), (None, ""));
        let (p, rest) = take_pairing("#room=1");
        assert_eq!((p, rest.as_str()), (None, "room=1"));
        assert_eq!(take_pairing(""), (None, String::new()));
    }

    #[test]
    fn backoff_doubles_to_thirty_seconds() {
        let mut b = Backoff::new();
        let delays: Vec<u64> = (0..9).filter_map(|i| b.next_delay_ms(i * 1000)).collect();
        assert_eq!(delays, [1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000, 30000]);
        assert_eq!(b.attempts(), 9);
    }

    #[test]
    fn backoff_gives_up_after_ten_minutes_and_resets_on_success() {
        let mut b = Backoff::new();
        assert_eq!(b.next_delay_ms(1_000), Some(1000));
        assert!(b.next_delay_ms(1_000 + RECONNECT_WINDOW_MS - 1).is_some());
        assert_eq!(b.next_delay_ms(1_000 + RECONNECT_WINDOW_MS), None);
        b.reset();
        assert_eq!(b.next_delay_ms(9_999_999), Some(1000));
    }

    #[test]
    fn the_hello_frame_has_the_token_and_the_page() {
        let v: Value = serde_json::from_str(&hello_frame(TOKEN, "0.2.0", 12, "abcd")).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "hello");
        assert_eq!(v["params"]["token"], TOKEN);
        assert_eq!(v["params"]["page"]["version"], "0.2.0");
        assert_eq!(v["params"]["page"]["registry"], json!({ "count": 12, "digest": "abcd" }));
    }

    #[test]
    fn the_hello_answer_is_read() {
        let ok = rpc::response_ok(&json!(1), &json!({ "protocol": 1, "role": "relay", "resume_token": TOKEN }));
        assert_eq!(parse_hello_answer(&ok), HelloAnswer::Welcome { resume_token: TOKEN.to_owned() });
        let no_token = rpc::response_ok(&json!(1), &json!({ "protocol": 1 }));
        assert!(matches!(parse_hello_answer(&no_token), HelloAnswer::Failed(_)));
        let wrong = rpc::response_err(Some(&json!(1)), &rpc::RpcError::new(UNAUTHORIZED, "bad token"));
        assert_eq!(parse_hello_answer(&wrong), HelloAnswer::Unauthorized);
        let other = rpc::response_err(Some(&json!(1)), &rpc::RpcError::new(rpc::BUSY, "busy"));
        assert_eq!(parse_hello_answer(&other), HelloAnswer::Failed("busy".to_owned()));
        assert!(matches!(parse_hello_answer("not json"), HelloAnswer::Failed(_)));
    }

    #[test]
    fn remembered_pairing_round_trips_and_rejects_junk() {
        let r = Remembered { port: 4555, resume_token: TOKEN.to_owned() };
        assert_eq!(Remembered::from_text(&r.to_text()), Some(r));
        for junk in [
            "",
            "{}",
            "[]",
            r#"{"port":0,"resume_token":"x"}"#,
            r#"{"port":70000,"resume_token":"x"}"#,
            &format!(r#"{{"port":1,"resume_token":"{}"}}"#, &TOKEN[2..]),
        ] {
            assert_eq!(Remembered::from_text(junk), None, "{junk}");
        }
    }
}
