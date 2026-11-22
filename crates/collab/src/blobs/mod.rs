//! The second pipe under the client: plain HTTP requests for the sealed pictures of a room
//! (`docs/design/collab-e2e.md` section 9). A [`BlobTransport`] carries one request at a time and reports each answer
//! with the id of its request; it knows nothing about rooms, keys or retries. The sans-IO client decides what to ask
//! and when to ask again.
//!
//! Like [`Transport`](crate::Transport) it never blocks: `send` queues, `poll` returns what has arrived, and a wake
//! hook tells the owner when to poll. Implementations: [`NativeBlobTransport`](crate::NativeBlobTransport) (worker
//! threads and `ureq`), `WebBlobTransport` (browser `fetch`, wasm only) and [`loopback_blobs`] (in memory, for tests).

mod loopback;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use drafftink_protocol::v2::MembershipToken;

use crate::access::{AccessDenial, AccessGrant};

pub use loopback::{LoopbackBlobServer, LoopbackBlobs, loopback_blobs};
#[cfg(not(target_arch = "wasm32"))]
pub use native::{NativeBlobOptions, NativeBlobTransport};
#[cfg(target_arch = "wasm32")]
pub use web::WebBlobTransport;

/// The scheme word of the `Authorization` header: `Authorization: Drafft-Member <base64url token>`.
pub const AUTH_SCHEME: &str = "Drafft-Member";

/// The longest response body a transport accepts, in bytes: a sealed picture at its limit, and the list of a room's
/// pictures is far smaller.
pub const MAX_BLOB_BODY_BYTES: usize = drafftink_protocol::MAX_SEALED_IMAGE_BYTES;

/// The value of the `Authorization` header of a room's picture requests. It holds the membership token, so `Debug`
/// does not print it and there is no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct BlobAuth(String);

impl BlobAuth {
    /// The header value for `token`: `Drafft-Member <base64url token>`.
    pub fn member(token: &MembershipToken) -> Self {
        BlobAuth(format!("{AUTH_SCHEME} {}", URL_SAFE_NO_PAD.encode(token.expose_bytes())))
    }

    /// The value to put in the `Authorization` header. It is a secret: keep it out of logs.
    pub fn header_value(&self) -> &str {
        &self.0
    }

    /// The membership token this header carries, when it is one. For a server end that checks it.
    pub fn token(&self) -> Option<MembershipToken> {
        let text = self.0.strip_prefix(AUTH_SCHEME)?.trim();
        let bytes: [u8; drafftink_protocol::v2::MEMBERSHIP_TOKEN_BYTES] =
            URL_SAFE_NO_PAD.decode(text).ok()?.try_into().ok()?;
        Some(MembershipToken::from_bytes(bytes))
    }
}

impl fmt::Debug for BlobAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BlobAuth(<hidden>)")
    }
}

/// What a request asks for.
#[derive(Clone, PartialEq, Eq)]
pub enum BlobOp {
    /// `PUT` with this body: a sealed picture.
    Put(Vec<u8>),
    /// `GET` one picture.
    Get,
    /// `HEAD`: does the picture exist, and how long is it (`BlobReply::content_length`).
    Head,
    /// `GET` of the room's list of pictures (JSON).
    List,
}

impl BlobOp {
    /// The HTTP method.
    pub const fn method(&self) -> &'static str {
        match self {
            BlobOp::Put(_) => "PUT",
            BlobOp::Get | BlobOp::List => "GET",
            BlobOp::Head => "HEAD",
        }
    }
}

impl fmt::Debug for BlobOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobOp::Put(body) => write!(f, "Put({} bytes)", body.len()),
            BlobOp::Get => f.write_str("Get"),
            BlobOp::Head => f.write_str("Head"),
            BlobOp::List => f.write_str("List"),
        }
    }
}

/// One HTTP request for a picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRequest {
    /// The id the answer carries. Unique among the requests of one client.
    pub id: u64,
    /// The full address: `http(s)://host[:port]/r/<room>/f[/<name>]`.
    pub url: String,
    /// What to do.
    pub op: BlobOp,
    /// The `Authorization` header.
    pub auth: BlobAuth,
}

/// What came back from a server.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct BlobReply {
    /// The HTTP status.
    pub status: u16,
    /// The body. Empty for `HEAD`.
    pub body: Vec<u8>,
    /// The `Content-Length` header, when there is one. For `HEAD` this is the size of the picture.
    pub content_length: Option<u64>,
    /// The `Retry-After` header in milliseconds, when it is a number of seconds.
    pub retry_after_ms: Option<u64>,
    /// Set when the answer is the server's gate and not the relay's: a `401` or a redirect to the Access login. The
    /// transport decides, because only it knows whether it sent a service token or is a browser.
    pub denial: Option<AccessDenial>,
}

impl fmt::Debug for BlobReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlobReply")
            .field("status", &self.status)
            .field("body", &format_args!("{} bytes", self.body.len()))
            .field("content_length", &self.content_length)
            .field("retry_after_ms", &self.retry_after_ms)
            .field("denial", &self.denial)
            .finish()
    }
}

/// The answer to one [`BlobRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobResponse {
    /// The id of the request.
    pub id: u64,
    /// The reply, or a short text for a failure of the network (no connection, timeout, a body over the limit).
    pub result: Result<BlobReply, String>,
}

/// A request that a transport could not even start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlobTransportError {
    /// The address is not one this transport may use (for example plain `http://` to another machine).
    #[error("{0}")]
    Refused(String),
    /// The transport cannot work (no worker thread, no browser window).
    #[error("{0}")]
    Failed(String),
}

/// A pipe for picture requests. See the module docs.
pub trait BlobTransport {
    /// Gives the transport the service token of a private server (`None`: no token). It goes into the headers of each
    /// request, under the rules of [`AccessGrant::headers_for`], and a redirect is never followed while it does. A
    /// transport that cannot send headers the page does not control (the browser's `fetch`, which carries the
    /// sign-in cookie) ignores it, which is why this has a default.
    fn set_access(&mut self, _access: Option<AccessGrant>) {}

    /// Queues one request. The answer arrives through [`poll`](Self::poll), with the id of the request.
    fn send(&mut self, request: BlobRequest) -> Result<(), BlobTransportError>;
    /// The next answer, without blocking.
    fn poll(&mut self) -> Option<BlobResponse>;
}

/// Parses a `Retry-After` header: a number of seconds, as milliseconds. A date is not read (the relay sends seconds).
pub(crate) fn retry_after_ms(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok().map(|s| s.saturating_mul(1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_carries_the_token_and_debug_hides_it() {
        let token = MembershipToken::from_bytes([9; 32]);
        let auth = BlobAuth::member(&token);
        assert!(auth.header_value().starts_with("Drafft-Member "));
        assert_eq!(auth.token(), Some(token));
        let shown = format!(
            "{auth:?} {:?}",
            BlobRequest { id: 1, url: "u".into(), op: BlobOp::Put(vec![7; 9]), auth: auth.clone() }
        );
        assert!(!shown.contains("Drafft-Member") && !shown.contains(&URL_SAFE_NO_PAD.encode([9u8; 32])));
        assert!(shown.contains("Put(9 bytes)"));
    }

    #[test]
    fn a_foreign_header_has_no_token() {
        assert_eq!(BlobAuth("Bearer abc".to_owned()).token(), None);
        assert_eq!(BlobAuth("Drafft-Member !!".to_owned()).token(), None);
        assert_eq!(BlobAuth(format!("Drafft-Member {}", URL_SAFE_NO_PAD.encode([1u8; 5]))).token(), None);
    }

    #[test]
    fn retry_after_reads_seconds_only() {
        assert_eq!(retry_after_ms("3"), Some(3000));
        assert_eq!(retry_after_ms(" 120 "), Some(120_000));
        assert_eq!(retry_after_ms("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(retry_after_ms("-1"), None);
    }
}
