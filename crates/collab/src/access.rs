//! Access to a private server: a Cloudflare Access service token (`docs/design/collab-e2e.md` section 19.6).
//!
//! A server behind Cloudflare Access wants two headers on every request, the WebSocket handshake included:
//! `CF-Access-Client-Id` and `CF-Access-Client-Secret`. This module holds the secret and decides where it may go:
//!
//! * [`AccessCredentials`]: the two values. The secret is kept in a buffer that is wiped when it is dropped, there is
//!   no `Display`, and `Debug` prints a placeholder.
//! * [`AccessOrigin`]: scheme, host and port of a server. Credentials belong to one origin.
//! * [`AccessGrant`]: credentials bound to their origin. [`AccessGrant::headers_for`] is the only way to read them
//!   out, and it answers only for the stored origin over `https` or `wss`, or to this machine.
//! * [`AccessDenial`]: why a server turned a request away, as a transport reports it.
//!
//! The transports never follow a redirect for a request that carries the headers. Access answers an unknown or expired
//! credential with a redirect to its own login page, so a redirect is reported as a denial instead.

use std::fmt;

use zeroize::Zeroizing;

use crate::url::{ServerUrl, UrlError, UrlPolicy, is_loopback_host};

/// The header that carries the client id of a service token.
pub const CLIENT_ID_HEADER: &str = "CF-Access-Client-Id";
/// The header that carries the client secret of a service token.
pub const CLIENT_SECRET_HEADER: &str = "CF-Access-Client-Secret";
/// The longest client id or client secret accepted, in bytes. Cloudflare's are about 40 and 64 characters long.
pub const MAX_ACCESS_VALUE_BYTES: usize = 512;

/// Which of the two values a check is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessField {
    /// The client id.
    ClientId,
    /// The client secret.
    ClientSecret,
}

impl AccessField {
    /// The words a person uses for it.
    pub const fn name(self) -> &'static str {
        match self {
            AccessField::ClientId => "client id",
            AccessField::ClientSecret => "client secret",
        }
    }
}

/// Why a value cannot be used as part of a service token. The text never holds the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccessError {
    /// Nothing was given.
    #[error("The access token's {} is empty.", .0.name())]
    Empty(AccessField),
    /// Longer than [`MAX_ACCESS_VALUE_BYTES`].
    #[error("The access token's {} is longer than {MAX_ACCESS_VALUE_BYTES} bytes.", .0.name())]
    TooLong(AccessField),
    /// Not printable ASCII, so it cannot be sent in a header.
    #[error("The access token's {} has spaces, line breaks or characters that a header cannot carry.", .0.name())]
    NotPlain(AccessField),
}

fn check(field: AccessField, value: &str) -> Result<(), AccessError> {
    if value.is_empty() {
        return Err(AccessError::Empty(field));
    }
    if value.len() > MAX_ACCESS_VALUE_BYTES {
        return Err(AccessError::TooLong(field));
    }
    if !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(AccessError::NotPlain(field));
    }
    Ok(())
}

/// A Cloudflare Access service token: client id and client secret.
///
/// `Debug` prints `AccessCredentials(<hidden>)` and there is no `Display`, so a logged struct cannot leak it. Both
/// values are wiped from memory when the value is dropped (best effort: a copy made before it was wrapped is not).
#[derive(Clone)]
pub struct AccessCredentials {
    client_id: Zeroizing<String>,
    client_secret: Zeroizing<String>,
}

impl AccessCredentials {
    /// Wraps a service token. Each value must be 1 to [`MAX_ACCESS_VALUE_BYTES`] printable ASCII characters.
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Result<Self, AccessError> {
        let client_id = Zeroizing::new(client_id.into());
        let client_secret = Zeroizing::new(client_secret.into());
        check(AccessField::ClientId, &client_id)?;
        check(AccessField::ClientSecret, &client_secret)?;
        Ok(AccessCredentials { client_id, client_secret })
    }

    /// The client id. It names the token and is not secret by itself, but it is kept out of logs too.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// The client secret. Hand it only to a transport, for the header, and never to a log or a text.
    pub fn expose_secret(&self) -> &str {
        &self.client_secret
    }
}

impl PartialEq for AccessCredentials {
    fn eq(&self, other: &Self) -> bool {
        // Both values live on this machine and the comparison answers nobody, so a plain comparison is enough.
        self.client_id == other.client_id && self.client_secret == other.client_secret
    }
}

impl Eq for AccessCredentials {}

impl fmt::Debug for AccessCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessCredentials(<hidden>)")
    }
}

/// The server a token belongs to: scheme, host and port. No path, no query.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccessOrigin {
    secure: bool,
    host: String,
    port: u16,
}

impl AccessOrigin {
    /// The origin of a server address: `wss://draw.example.com/ws` is `https://draw.example.com`.
    pub fn of(url: &ServerUrl) -> Self {
        AccessOrigin { secure: url.is_secure(), host: url.host().to_owned(), port: url.port() }
    }

    /// Reads `https://host[:port]` or `http://host[:port]`. A path, a query, a fragment or a `user:password@` part is
    /// refused: an origin is only the server.
    pub fn parse(text: &str) -> Result<Self, UrlError> {
        let text = text.trim();
        let (scheme, rest) = if let Some(rest) = strip_prefix_ignore_case(text, "https://") {
            ("wss://", rest)
        } else if let Some(rest) = strip_prefix_ignore_case(text, "http://") {
            ("ws://", rest)
        } else {
            return Err(UrlError::Scheme);
        };
        if rest.trim_end_matches('/').contains(['/', '?']) {
            return Err(UrlError::Invalid("an origin is only the server, with no path"));
        }
        let url = ServerUrl::parse(&format!("{scheme}{rest}"), UrlPolicy { allow_insecure: true })?;
        Ok(Self::of(&url))
    }

    /// The origin of the address of an HTTP request, such as `https://host:8443/r/abc/f/name?x=1`. `None` when the
    /// text is not an `http://` or `https://` address of a server.
    pub fn of_http_url(url: &str) -> Option<Self> {
        let (scheme, rest) = url.split_once("://")?;
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        Self::parse(&format!("{scheme}://{}", &rest[..end])).ok()
    }

    /// True for `https` and `wss`.
    pub fn is_secure(&self) -> bool {
        self.secure
    }

    /// The host name or IP address, without brackets, in lower case.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// True when the host is this machine.
    pub fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

impl fmt::Display for AccessOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (scheme, default) = if self.secure { ("https", 443) } else { ("http", 80) };
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == default { write!(f, "{scheme}://{host}") } else { write!(f, "{scheme}://{host}:{}", self.port) }
    }
}

impl fmt::Debug for AccessOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccessOrigin({self})")
    }
}

/// The two header values of one request. `Debug` prints a placeholder.
#[derive(Clone, Copy)]
pub struct AccessHeaders<'a> {
    client_id: &'a str,
    client_secret: &'a str,
}

impl<'a> AccessHeaders<'a> {
    /// The header names and values, in the order they are sent.
    pub fn pairs(&self) -> [(&'static str, &'a str); 2] {
        [(CLIENT_ID_HEADER, self.client_id), (CLIENT_SECRET_HEADER, self.client_secret)]
    }
}

impl fmt::Debug for AccessHeaders<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessHeaders(<hidden>)")
    }
}

/// A service token and the one origin it may be sent to.
#[derive(Debug, Clone)]
pub struct AccessGrant {
    origin: AccessOrigin,
    credentials: AccessCredentials,
}

impl AccessGrant {
    /// Binds `credentials` to `origin`.
    pub fn new(origin: AccessOrigin, credentials: AccessCredentials) -> Self {
        AccessGrant { origin, credentials }
    }

    /// The origin the token belongs to.
    pub fn origin(&self) -> &AccessOrigin {
        &self.origin
    }

    /// The headers for a request to `target`, or `None` when the token must not go there.
    ///
    /// The token goes only to the origin it was stored for (same scheme, host and port), and only over `https` or
    /// `wss`, except to this machine. Plain `ws://` to another machine stays closed even when the person allowed
    /// insecure drawing connections: that setting protects a drawing, this protects a password.
    pub fn headers_for(&self, target: &AccessOrigin) -> Option<AccessHeaders<'_>> {
        (*target == self.origin && (target.is_secure() || target.is_loopback())).then(|| AccessHeaders {
            client_id: self.credentials.client_id(),
            client_secret: self.credentials.expose_secret(),
        })
    }
}

/// Why a server turned a request away at its door, as the transport saw it. The transport decides from the status and
/// the Worker's JSON body (`{"error":"access_required"|"access_invalid"}`); the client turns it into words with
/// [`explain_access`](crate::explain_access).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessDenial {
    /// The server is private and no token was sent.
    Required,
    /// A token was sent and the server did not accept it: expired, revoked, or not for this server.
    Rejected,
    /// A browser's sign-in has ended. Reloading the page signs in again.
    SignInExpired,
}

/// The statuses of an answer that sends the client somewhere else.
const REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];

impl AccessDenial {
    /// Decides whether an HTTP answer is the server's gate. `sent` says whether the request carried a token.
    ///
    /// * The Worker's own JSON, `{"error":"access_invalid"}` or `{"error":"access_required"}`, on a `401` or `403`.
    ///   `access_required` after a token was sent means the token was not taken.
    /// * Any `401` without a JSON `error` of another kind, and any redirect: Access answers an unknown or expired
    ///   credential with a redirect to its login, and a bare `401` when the request says it is a script.
    ///
    /// Everything else is no denial. A `401` or `403` whose JSON names another error is the relay's own (a membership
    /// token that does not open the room, `{"error":"unauthorized"}`), and a `403` without JSON is some other refusal.
    pub fn of_reply(status: u16, body: &[u8], sent: bool) -> Option<Self> {
        if REDIRECTS.contains(&status) {
            return Some(Self::when_sent(sent));
        }
        let error = matches!(status, 401 | 403).then(|| json_error(body)).flatten();
        match (status, error) {
            (401 | 403, Some(JsonError::Invalid)) => Some(AccessDenial::Rejected),
            (401 | 403, Some(JsonError::Required)) | (401, None) => Some(Self::when_sent(sent)),
            _ => None,
        }
    }

    fn when_sent(sent: bool) -> Self {
        if sent { AccessDenial::Rejected } else { AccessDenial::Required }
    }

    /// The same denial seen from a browser, which never sends the token: the person signs in again.
    pub fn in_browser(self) -> Self {
        AccessDenial::SignInExpired
    }
}

/// What the `error` field of a JSON answer says.
enum JsonError {
    /// `access_required`: the Worker saw no assertion from Access.
    Required,
    /// `access_invalid`: the Worker saw one and did not accept it.
    Invalid,
    /// Any other word: the relay's own refusal.
    Other,
}

/// The kind of the `error` field, when the body is JSON that has one.
fn json_error(body: &[u8]) -> Option<JsonError> {
    let value: serde_json::Value = serde_json::from_slice(body.get(..body.len().min(4096))?).ok()?;
    Some(match value.get("error")?.as_str()? {
        "access_required" => JsonError::Required,
        "access_invalid" => JsonError::Invalid,
        _ => JsonError::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> ServerUrl {
        ServerUrl::parse(text, UrlPolicy { allow_insecure: true }).unwrap()
    }

    fn origin(text: &str) -> AccessOrigin {
        AccessOrigin::of(&url(text))
    }

    fn grant(text: &str) -> AccessGrant {
        AccessGrant::new(origin(text), AccessCredentials::new("id-1.access", "s3cret-VALUE").unwrap())
    }

    #[test]
    fn values_are_checked_and_the_errors_never_repeat_them() {
        assert_eq!(AccessCredentials::new("", "x").unwrap_err(), AccessError::Empty(AccessField::ClientId));
        assert_eq!(AccessCredentials::new("x", "").unwrap_err(), AccessError::Empty(AccessField::ClientSecret));
        let long = "a".repeat(MAX_ACCESS_VALUE_BYTES + 1);
        assert_eq!(
            AccessCredentials::new("x", long.clone()).unwrap_err(),
            AccessError::TooLong(AccessField::ClientSecret)
        );
        assert!(AccessCredentials::new("x".repeat(MAX_ACCESS_VALUE_BYTES), "y").is_ok());
        for bad in ["a b", "a\nb", "tab\t", "héllo", "a\u{7f}"] {
            let e = AccessCredentials::new("id", bad).unwrap_err();
            assert_eq!(e, AccessError::NotPlain(AccessField::ClientSecret), "{bad:?}");
            assert!(!e.to_string().contains(bad), "{e}");
        }
        assert!(!AccessError::TooLong(AccessField::ClientSecret).to_string().contains(&long));
    }

    #[test]
    fn debug_prints_a_placeholder_and_there_is_no_display() {
        let c = AccessCredentials::new("id-1.access", "s3cret-VALUE").unwrap();
        let g = grant("wss://canvas.example.com/ws");
        let shown = format!("{c:?} {g:?} {:?}", g.headers_for(g.origin()).unwrap());
        assert!(shown.contains("AccessCredentials(<hidden>)"), "{shown}");
        assert!(!shown.contains("s3cret") && !shown.contains("id-1"), "{shown}");
    }

    #[test]
    fn an_origin_is_scheme_host_and_port() {
        assert_eq!(origin("wss://Canvas.Example.com/ws").to_string(), "https://canvas.example.com");
        assert_eq!(origin("wss://canvas.example.com:8443/r/x/ws").to_string(), "https://canvas.example.com:8443");
        assert_eq!(origin("ws://127.0.0.1:3030/ws").to_string(), "http://127.0.0.1:3030");
        assert_eq!(origin("ws://[::1]:3030").to_string(), "http://[::1]:3030");
        assert_eq!(
            AccessOrigin::of_http_url("https://Canvas.Example.com/r/abc/f/n?x=1").unwrap(),
            origin("wss://canvas.example.com/ws")
        );
        assert_eq!(AccessOrigin::of_http_url("http://[::1]:3030/r/a/f").unwrap(), origin("ws://[::1]:3030"));
        for bad in ["", "ws://host/x", "https://", "https://u:p@host/x", "file:///etc", "nonsense"] {
            assert!(AccessOrigin::of_http_url(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn an_origin_is_read_back_from_its_text() {
        for text in [
            "https://canvas.example.com",
            "https://canvas.example.com:8443",
            "http://127.0.0.1:3030",
            "http://[::1]:3030",
        ] {
            assert_eq!(AccessOrigin::parse(text).unwrap().to_string(), text);
        }
        for bad in [
            "",
            "canvas.example.com",
            "ftp://x",
            "wss://canvas.example.com",
            "https://u:p@host",
            "https://host/some/path",
            "https://host?x=1",
            "https://host#x",
        ] {
            assert!(AccessOrigin::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_token_goes_to_its_own_origin_over_https() {
        let g = grant("wss://canvas.example.com/ws");
        let pairs = g.headers_for(&origin("wss://canvas.example.com/r/abc/ws")).unwrap().pairs();
        assert_eq!(pairs, [(CLIENT_ID_HEADER, "id-1.access"), (CLIENT_SECRET_HEADER, "s3cret-VALUE")]);
        assert!(g.headers_for(&AccessOrigin::parse("https://canvas.example.com").unwrap()).is_some());
    }

    #[test]
    fn the_token_goes_nowhere_else() {
        let g = grant("wss://canvas.example.com/ws");
        for other in [
            "wss://other.example.com/ws",
            "wss://canvas.example.com:8443/ws",
            "wss://evil.canvas.example.com/ws",
            "wss://canvas.example.com.evil.net/ws",
            "ws://canvas.example.com/ws",
        ] {
            assert!(g.headers_for(&origin(other)).is_none(), "{other}");
        }
    }

    #[test]
    fn plain_text_is_for_this_machine_only_even_when_insecure_connections_are_allowed() {
        let local = grant("ws://127.0.0.1:3030/ws");
        assert!(local.headers_for(&origin("ws://127.0.0.1:3030/ws")).is_some());
        assert!(local.headers_for(&origin("ws://localhost:3030/ws")).is_none(), "another name is another origin");
        let lan = grant("ws://192.168.1.20:3030/ws");
        assert!(lan.headers_for(&origin("ws://192.168.1.20:3030/ws")).is_none());
        let named = grant("ws://canvas.example.com/ws");
        assert!(named.headers_for(&origin("ws://canvas.example.com/ws")).is_none());
    }

    #[test]
    fn a_gate_is_recognised_by_status_and_by_the_workers_json() {
        let json = |e: &str| format!(r#"{{"error":"{e}","message":"m"}}"#).into_bytes();
        for status in [401, 301, 302, 303, 307, 308] {
            assert_eq!(AccessDenial::of_reply(status, b"", false), Some(AccessDenial::Required), "{status}");
            assert_eq!(AccessDenial::of_reply(status, b"", true), Some(AccessDenial::Rejected), "{status}");
        }
        assert_eq!(AccessDenial::of_reply(401, &json("access_required"), false), Some(AccessDenial::Required));
        assert_eq!(AccessDenial::of_reply(401, &json("access_required"), true), Some(AccessDenial::Rejected));
        assert_eq!(AccessDenial::of_reply(401, &json("access_invalid"), false), Some(AccessDenial::Rejected));
        assert_eq!(AccessDenial::of_reply(401, &json("access_invalid"), true), Some(AccessDenial::Rejected));
        assert_eq!(AccessDenial::of_reply(403, &json("access_invalid"), true), Some(AccessDenial::Rejected));
        // A 403 without the Worker's JSON is some other refusal (a relay says 403 for its own reasons), and a 401 with
        // another JSON error is the relay's own: a membership token that does not open the room.
        assert_eq!(AccessDenial::of_reply(403, b"", true), None);
        assert_eq!(AccessDenial::of_reply(403, &json("other"), true), None);
        assert_eq!(AccessDenial::of_reply(401, &json("unauthorized"), true), None);
        assert_eq!(AccessDenial::of_reply(401, &json("unauthorized"), false), None);
        for status in [200, 201, 204, 304, 400, 404, 429, 500, 503] {
            assert_eq!(AccessDenial::of_reply(status, b"", true), None, "{status}");
        }
        assert_eq!(AccessDenial::of_reply(200, &json("access_invalid"), true), None);
        assert_eq!(AccessDenial::of_reply(401, b"not json", false), Some(AccessDenial::Required));
    }
}
