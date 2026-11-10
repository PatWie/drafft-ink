//! The server address, checked once. A [`ServerUrl`] can only be built by [`ServerUrl::parse`], so a transport that
//! takes one never sees a `http://`, `file://` or credential-carrying address.

use std::fmt;

/// What the address policy allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UrlPolicy {
    /// Allow plain `ws://` to a host that is not this machine. Off by default: frames are not encrypted by the
    /// protocol, so anyone on the path could read and change the drawing (docs/protocol.md section 8).
    pub allow_insecure: bool,
}

/// Why an address was refused. Every message says what to do.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    /// Empty text.
    #[error("The server address is empty. Use an address like wss://draw.example.com")]
    Empty,
    /// The scheme is not `ws` or `wss`.
    #[error("The server address must start with wss:// (or ws:// for a server on this machine)")]
    Scheme,
    /// Plain `ws://` to another machine.
    #[error(
        "ws:// sends the drawing unencrypted. Use wss:// for {0}, or allow insecure connections if you trust the \
         network"
    )]
    Insecure(String),
    /// A `user:password@` part.
    #[error("The server address must not contain a user name or password. Give the access token separately")]
    Credentials,
    /// A `#fragment`.
    #[error("The server address must not contain a # part")]
    Fragment,
    /// No host name.
    #[error("The server address has no host name")]
    NoHost,
    /// Bad host characters or an unreadable port.
    #[error("The server address is not valid: {0}")]
    Invalid(&'static str),
}

/// A checked `ws://` or `wss://` address of the collaboration endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct ServerUrl {
    secure: bool,
    host: String,
    port: u16,
    path: String,
    loopback: bool,
}

/// The longest address accepted, in bytes.
const MAX_URL_BYTES: usize = 2048;

impl ServerUrl {
    /// Checks `text`. An address with no path, or only `/`, gets the endpoint path `/ws`.
    pub fn parse(text: &str, policy: UrlPolicy) -> Result<Self, UrlError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(UrlError::Empty);
        }
        if text.len() > MAX_URL_BYTES {
            return Err(UrlError::Invalid("it is too long"));
        }
        if text.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(UrlError::Invalid("it contains spaces or control characters"));
        }
        let (secure, rest) = if let Some(r) = strip_scheme(text, "wss://") {
            (true, r)
        } else if let Some(r) = strip_scheme(text, "ws://") {
            (false, r)
        } else {
            return Err(UrlError::Scheme);
        };
        if rest.contains('#') {
            return Err(UrlError::Fragment);
        }
        let (authority, tail) = match rest.find(['/', '?']) {
            Some(i) => rest.split_at(i),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(UrlError::Credentials);
        }
        let (host, port) = split_host_port(authority, secure)?;
        let path = normalize_path(tail);
        let loopback = is_loopback(&host);
        if !secure && !loopback && !policy.allow_insecure {
            return Err(UrlError::Insecure(host));
        }
        Ok(ServerUrl { secure, host, port, path, loopback })
    }

    /// True for `wss://`.
    pub fn is_secure(&self) -> bool {
        self.secure
    }

    /// The host name or IP address, without brackets.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port, defaulted from the scheme.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The request path and query, starting with `/`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// True when the host is this machine (`localhost`, `127.x.x.x`, `::1`).
    pub fn is_loopback(&self) -> bool {
        self.loopback
    }
}

impl fmt::Display for ServerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = if self.secure { "wss" } else { "ws" };
        let default = if self.secure { 443 } else { 80 };
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == default {
            write!(f, "{scheme}://{host}{}", self.path)
        } else {
            write!(f, "{scheme}://{host}:{}{}", self.port, self.path)
        }
    }
}

impl fmt::Debug for ServerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServerUrl({self})")
    }
}

fn strip_scheme<'a>(text: &'a str, scheme: &str) -> Option<&'a str> {
    let head = text.get(..scheme.len())?;
    head.eq_ignore_ascii_case(scheme).then(|| &text[scheme.len()..])
}

fn split_host_port(authority: &str, secure: bool) -> Result<(String, u16), UrlError> {
    let default = if secure { 443 } else { 80 };
    if authority.is_empty() {
        return Err(UrlError::NoHost);
    }
    let (host, port_text) = if let Some(inner) = authority.strip_prefix('[') {
        let end = inner.find(']').ok_or(UrlError::Invalid("the [IPv6] address is not closed"))?;
        let host = &inner[..end];
        let after = &inner[end + 1..];
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':').ok_or(UrlError::Invalid("unexpected text after the address"))?),
        };
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(UrlError::Invalid("the [IPv6] address cannot be read"));
        }
        (host.to_ascii_lowercase(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_ascii_lowercase(), Some(p)),
            None => (authority.to_ascii_lowercase(), None),
        }
    };
    if host.is_empty() {
        return Err(UrlError::NoHost);
    }
    let is_v6 = host.contains(':');
    if !is_v6 && !host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')) {
        return Err(UrlError::Invalid("the host name has characters that are not allowed"));
    }
    let port = match port_text {
        None => default,
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n != 0 => n,
            _ => return Err(UrlError::Invalid("the port must be a number from 1 to 65535")),
        },
    };
    Ok((host, port))
}

fn normalize_path(tail: &str) -> String {
    match tail {
        "" | "/" => "/ws".to_owned(),
        t if t.starts_with('?') => format!("/ws{t}"),
        t if t.starts_with("/?") => format!("/ws{}", &t[1..]),
        t => t.to_owned(),
    }
}

fn is_loopback(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        return v4.is_loopback();
    }
    if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        return v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> ServerUrl {
        ServerUrl::parse(s, UrlPolicy::default()).unwrap()
    }
    fn err(s: &str) -> UrlError {
        ServerUrl::parse(s, UrlPolicy::default()).unwrap_err()
    }

    #[test]
    fn wss_is_the_default_and_gets_the_endpoint_path() {
        let u = ok("wss://Draw.Example.com");
        assert_eq!((u.host(), u.port(), u.path(), u.is_secure()), ("draw.example.com", 443, "/ws", true));
        assert_eq!(u.to_string(), "wss://draw.example.com/ws");
        assert_eq!(ok("wss://draw.example.com:8443/sync").to_string(), "wss://draw.example.com:8443/sync");
        assert_eq!(ok("wss://h/?x=1").path(), "/ws?x=1");
    }

    #[test]
    fn plain_ws_is_only_for_this_machine() {
        for s in ["ws://localhost:3030/ws", "ws://127.0.0.1:1", "ws://127.9.9.9", "ws://[::1]:3030", "ws://a.localhost"]
        {
            assert!(ok(s).is_loopback(), "{s}");
        }
        for s in ["ws://example.com", "ws://192.168.1.5:3030", "ws://10.0.0.1", "ws://localhost.evil.com", "ws://[::2]"]
        {
            assert!(matches!(err(s), UrlError::Insecure(_)), "{s}");
        }
        let allowed = UrlPolicy { allow_insecure: true };
        assert!(ServerUrl::parse("ws://192.168.1.5:3030", allowed).is_ok());
    }

    #[test]
    fn other_schemes_credentials_and_junk_are_refused() {
        assert_eq!(err(""), UrlError::Empty);
        for s in [
            "http://localhost",
            "https://x.com",
            "file:///etc/passwd",
            "ftp://x",
            "localhost:3030",
            "javascript:alert(1)",
        ] {
            assert_eq!(err(s), UrlError::Scheme, "{s}");
        }
        assert_eq!(err("wss://user:pw@host/ws"), UrlError::Credentials);
        assert_eq!(err("wss://host/ws#frag"), UrlError::Fragment);
        assert_eq!(err("wss:///ws"), UrlError::NoHost);
        for s in [
            "wss://host:0",
            "wss://host:99999",
            "wss://host:abc",
            "wss://ho st",
            "wss://ho$t",
            "wss://[::1",
            "wss://[nope]",
        ] {
            assert!(matches!(err(s), UrlError::Invalid(_) | UrlError::Scheme), "{s}");
        }
        assert!(matches!(err(&format!("wss://{}", "a".repeat(3000))), UrlError::Invalid(_)));
    }

    #[test]
    fn display_round_trips() {
        for s in ["wss://a.b/ws", "ws://127.0.0.1:3030/ws", "ws://[::1]:3030/ws", "wss://a.b:444/x?y=1"] {
            assert_eq!(ok(&ok(s).to_string()), ok(s));
        }
    }
}
