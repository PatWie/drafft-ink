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
/// Length of a generated room name.
const GENERATED_ROOM_CHARS: usize = 10;
/// The characters of a generated room name.
const ROOM_ALPHABET: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

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

    /// A 10-character name made of `bits`.
    pub(super) fn generate(mut bits: u128) -> Result<Self, RoomNameError> {
        let base = ROOM_ALPHABET.len() as u128;
        let text: String = (0..GENERATED_ROOM_CHARS)
            .map(|_| {
                let digit = usize::try_from(bits % base).unwrap_or(0);
                bits /= base;
                char::from(ROOM_ALPHABET.get(digit).copied().unwrap_or(b'a'))
            })
            .collect();
        Self::parse(&text)
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

/// `#room=<id>&server=<url>`: what a person sends to invite another.
pub(super) fn room_link(room: &RoomName, server: &ServerUrl) -> String {
    let mut out = format!("#room={room}&server=");
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
