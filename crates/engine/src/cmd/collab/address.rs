//! Server addresses, room names and the shareable room link.
//!
//! Pure functions and small newtypes: nothing here touches a session. The commands map the typed errors to
//! refusals.

use std::fmt::{self, Write as _};

use drafftink_collab::{ServerUrl, UrlError, UrlPolicy};
use drafftink_protocol::RoomId;

use crate::ports::Net;

/// Longest room name the catalog allows.
const MAX_ROOM_CHARS: usize = 64;

/// Why a server address could not be used.
#[derive(Debug, thiserror::Error)]
pub(super) enum AddressError {
    /// The text is not an acceptable `ws://` or `wss://` address.
    #[error("{0}")]
    Url(#[from] UrlError),
    /// A path such as `/ws` was given where no web page provides a server.
    #[error("A path such as `/ws` needs the web app, where it means the server of this page")]
    NoOrigin,
}

/// Checks a server address. A text that starts with `/` means the server of the page that delivered the app
/// and is resolved by the shell through [`Net::same_origin_url`].
pub(super) fn resolve_server(net: Option<&dyn Net>, text: &str) -> Result<ServerUrl, AddressError> {
    let text = text.trim();
    let policy = UrlPolicy { allow_insecure: net.is_some_and(Net::allow_insecure) };
    if text.starts_with('/') {
        let full = net.and_then(|n| n.same_origin_url(text)).ok_or(AddressError::NoOrigin)?;
        return Ok(ServerUrl::parse(&full, policy)?);
    }
    Ok(ServerUrl::parse(text, policy)?)
}

/// A room name that passed the catalog's rule: 1 to 64 characters, letters, digits, `-` and `_`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RoomName(RoomId);

/// Why a room name was refused.
#[derive(Debug, thiserror::Error)]
#[error("must be 1 to {MAX_ROOM_CHARS} characters: letters, digits, `-` and `_`")]
pub(super) struct RoomNameError;

impl RoomName {
    /// Checks `text`.
    pub(super) fn parse(text: &str) -> Result<Self, RoomNameError> {
        let ok = !text.is_empty()
            && text.chars().count() <= MAX_ROOM_CHARS
            && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        if !ok {
            return Err(RoomNameError);
        }
        RoomId::new(text).map(Self).map_err(|_| RoomNameError)
    }

    /// The protocol id.
    pub(super) fn id(&self) -> &RoomId {
        &self.0
    }
}

impl fmt::Display for RoomName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The room argument of a command: the name of a plaintext room, or an encrypted room's link.
pub(super) enum RoomSpec {
    /// A plaintext room, by name.
    Plain(RoomName),
    /// An encrypted room: the key opens it, and its id is a hash of the key.
    Sealed(drafftink_seal::RoomLink),
}

/// Why a room argument is not a room.
#[derive(Debug, thiserror::Error)]
pub(super) enum RoomSpecError {
    /// A plain name that breaks the catalog's rule.
    #[error("{0}")]
    Name(#[from] RoomNameError),
    /// A link that is damaged or has no key. The text says how to proceed and repeats nothing of the input.
    #[error("{0}")]
    Link(#[from] drafftink_seal::LinkError),
}

impl RoomSpec {
    /// Reads `text`: a link (`https://host/#room=<id>,<key>`, `#room=<id>,<key>`) or `<id>,<key>` is an encrypted
    /// room, anything else must be a plain room name.
    pub(super) fn parse(text: &str) -> Result<Self, RoomSpecError> {
        let text = text.trim();
        if !(text.contains(',') || text.contains("room=") || text.contains('#')) {
            return Ok(RoomSpec::Plain(RoomName::parse(text)?));
        }
        let link = if text.contains("room=") { drafftink_seal::RoomLink::parse(text) } else { drafftink_seal::RoomLink::parse(&format!("room={text}")) };
        Ok(RoomSpec::Sealed(link?))
    }
}

/// `#room=<id>&server=<url>`: what a person sends to invite another.
pub(super) fn room_link(room: &RoomName, server: &ServerUrl) -> String {
    format!("#room={room}&server={}", encode_server(server))
}

/// The server address as it goes into a link: percent-encoded except for the characters a link keeps.
pub(super) fn encode_server(server: &ServerUrl) -> String {
    let mut out = String::new();
    for b in server.to_string().bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b':' | b'/') {
            out.push(char::from(b));
        } else {
            // Writing to a String cannot fail.
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The room and server named by a page address or command line: `?room=a&server=b`, `#room=a&server=b`, or
/// the whole address. The shell calls this at start-up and passes the result to `collab.connect`.
///
/// A server given as `host:port` or `host` (the old app's form) becomes `ws://host:port/ws`; a text with a
/// scheme or a leading `/` is kept as it is for `collab.connect` to check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomLink {
    /// The room to join.
    pub room: Option<String>,
    /// The server address, ready for `collab.connect`.
    pub server: Option<String>,
}

impl RoomLink {
    /// Reads the `room` and `server` parameters from the query (`?`) and the fragment (`#`) of `text`. The
    /// query wins over the fragment, as in the old app.
    pub fn parse(text: &str) -> Self {
        let (before_hash, fragment) = text.split_once('#').unwrap_or((text, ""));
        let query = before_hash.split_once('?').map_or("", |(_, q)| q);
        let mut link = Self::default();
        for part in [query, fragment] {
            for (key, value) in part.split('&').filter_map(|pair| pair.split_once('=')) {
                let value = decode(value);
                if value.is_empty() {
                    continue;
                }
                match key {
                    "room" if link.room.is_none() => link.room = Some(value),
                    "server" if link.server.is_none() => link.server = Some(with_scheme(&value)),
                    _ => {}
                }
            }
        }
        link
    }
}

fn with_scheme(server: &str) -> String {
    let s = server.trim();
    if s.starts_with('/') || s.contains("://") { s.to_owned() } else { format!("ws://{}", s.trim_end_matches('/')) }
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let escaped = (b == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match escaped {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
