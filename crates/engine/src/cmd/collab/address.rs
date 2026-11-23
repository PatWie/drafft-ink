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
    /// The id of a private room without its key. A plain room is never named like that, and joining it as one
    /// would only fail somewhere else.
    #[error("This is a private room. Paste the whole invite link (it contains the key after the comma).")]
    BareId,
}

/// True for text that looks like the id of an encrypted room: 22 characters of base64url (16 bytes).
fn looks_like_room_id(text: &str) -> bool {
    text.len() == ENCRYPTED_ID_CHARS && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// The length of an encrypted room's id as text.
const ENCRYPTED_ID_CHARS: usize = 22;

impl RoomSpec {
    /// Reads `text`: a link (`https://host/#room=<id>,<key>`, `#room=<id>,<key>`) or `<id>,<key>` is an encrypted
    /// room, anything else must be a plain room name.
    pub(super) fn parse(text: &str) -> Result<Self, RoomSpecError> {
        let text = text.trim();
        if !(text.contains(',') || text.contains("room=") || text.contains('#')) {
            if looks_like_room_id(text) {
                return Err(RoomSpecError::BareId);
            }
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

/// Where an invite link opens when nothing better is known: the demo page. It does not host a relay of its own, so a
/// link to it carries the server.
const FALLBACK_LINK_ORIGIN: &str = "https://drafft.ink";

/// The invite link of an encrypted room: `<origin>/#room=<id>,<key>`, with `&server=` only when the page at `<origin>`
/// would not find the room's server by itself.
///
/// * On the web (`page` is the server that delivered the app), the link opens that page. When the room's server is the
///   page's own, the link has no `&server=`: the page already means its own server. A room on another server names it.
/// * On native, a secure server that is not this machine is taken to serve the app next to its relay (the Worker and
///   `drafftink-headless serve` do), so the link opens that server's own page with no `&server=`. A plain `ws://` or
///   loopback server does not, so its link opens the demo page and names the server.
///
/// The Cloudflare Access cookie and the Worker's allowed origins both belong to the relay's own origin, which is why a
/// link must open the relay's page and not another one.
pub(super) fn invite_link(link: &drafftink_seal::RoomLink, page: Option<&ServerUrl>, server: Option<&ServerUrl>) -> String {
    let origin = match (page, server) {
        (Some(page), _) => page.http_base(),
        (None, Some(server)) if server.is_secure() && !server.is_loopback() => server.http_base(),
        (None, _) => FALLBACK_LINK_ORIGIN.to_owned(),
    };
    let mut text = link.url(&origin).to_string();
    if let Some(server) = server.filter(|s| s.http_base() != origin) {
        text.push_str("&server=");
        text.push_str(&encode_server(server));
    }
    text
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

#[cfg(test)]
mod tests {
    use drafftink_seal::{RoomKey, RoomLink};

    use super::*;

    fn link() -> RoomLink {
        RoomLink::new(RoomKey::from_bytes([7; 32]))
    }

    fn url(text: &str) -> ServerUrl {
        ServerUrl::parse(text, UrlPolicy { allow_insecure: true }).unwrap()
    }

    fn fragment() -> String {
        link().fragment().to_string()
    }

    #[test]
    fn a_link_from_the_page_of_the_relay_has_no_server() {
        let page = url("wss://canvas.patwie.com/ws");
        let made = invite_link(&link(), Some(&page), Some(&url("wss://canvas.patwie.com/ws")));
        assert_eq!(made, format!("https://canvas.patwie.com/{}", fragment()));
    }

    #[test]
    fn a_page_without_a_relay_names_the_server_it_uses() {
        let page = url("wss://drafft.ink/ws");
        let demo = url("wss://canvas.patwie.com/ws");
        let made = invite_link(&link(), Some(&page), Some(&demo));
        assert_eq!(made, format!("https://drafft.ink/{}&server=wss://canvas.patwie.com/ws", fragment()));
        let same = invite_link(&link(), Some(&page), Some(&url("wss://drafft.ink/ws")));
        assert_eq!(same, format!("https://drafft.ink/{}", fragment()), "the demo's own page needs no server either");
    }

    #[test]
    fn a_native_app_opens_the_page_of_a_secure_relay_and_the_demo_for_a_bare_server() {
        let relay = invite_link(&link(), None, Some(&url("wss://canvas.patwie.com/ws")));
        assert_eq!(relay, format!("https://canvas.patwie.com/{}", fragment()));
        let port = invite_link(&link(), None, Some(&url("wss://relay.example.com:8443/ws")));
        assert_eq!(port, format!("https://relay.example.com:8443/{}", fragment()));
        let bare = invite_link(&link(), None, Some(&url("ws://127.0.0.1:3030/ws")));
        assert_eq!(bare, format!("https://drafft.ink/{}&server=ws://127.0.0.1:3030/ws", fragment()));
        let lan = invite_link(&link(), None, Some(&url("ws://192.168.1.5:3030/ws")));
        assert!(lan.starts_with("https://drafft.ink/#room=") && lan.ends_with("&server=ws://192.168.1.5:3030/ws"), "{lan}");
        let loopback_tls = invite_link(&link(), None, Some(&url("wss://localhost:3030/ws")));
        assert!(loopback_tls.starts_with("https://drafft.ink/"), "{loopback_tls}");
    }

    #[test]
    fn a_bare_room_id_is_a_private_room_and_never_a_plain_one() {
        let id = link().id().to_string();
        assert_eq!(id.len(), ENCRYPTED_ID_CHARS);
        let said = RoomSpec::parse(&id).err().map(|e| e.to_string());
        assert_eq!(
            said.as_deref(),
            Some("This is a private room. Paste the whole invite link (it contains the key after the comma).")
        );
        assert!(matches!(RoomSpec::parse("Test"), Ok(RoomSpec::Plain(_))), "a short name is a plain room");
        let full = format!("https://canvas.patwie.com/{}", fragment());
        assert!(matches!(RoomSpec::parse(&full), Ok(RoomSpec::Sealed(_))), "a whole link");
        assert!(matches!(RoomSpec::parse(&format!("{id},{}", &fragment()[fragment().find(',').unwrap() + 1..])), Ok(RoomSpec::Sealed(_))), "id,key");
    }
}
