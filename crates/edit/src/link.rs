//! Hyperlinks and element links: the rules behind `element.set_link` and `element.open_link`.
//!
//! An element has one optional `link` string. It holds either
//! * a **web link**: an `http://` or `https://` address, or a `mailto:` address, stored with a lower-case
//!   scheme (a bare `example.com/page` gets `https://`), or
//! * an **element link**: `#<id>`, the id of another element of the same document. The input forms `#<id>` and
//!   `@<name>` are accepted; `@<name>` is stored as the id, so that renaming the target does not break the link.
//!
//! Every other scheme (`javascript:`, `data:`, `file:`, `vbscript:` ...) is refused, when the link is set and
//! again when it is opened, because a document from outside (an imported `.excalidraw` file) can carry any
//! text in `link`. This module only checks the text and writes the field. Resolving a `@name` or a missing
//! target needs the session, so the command does that.
//!
//! The same module owns where the **link icon** of an element sits ([`icon_rect`]) and whether a point hits it
//! ([`hits_icon`]), so the renderer and the pointer path use one rule (Excalidraw `getLinkHandleFromCoords`).

use drafftink_scene::{Element, ElementId, SceneRead, SceneTx, StoreError, limits::MAX_LINK_BYTES};
use kurbo::{Point, Rect, Vec2};

/// The character that starts an element link.
pub const ELEMENT_PREFIX: char = '#';

/// Why a link was refused. The text says how to proceed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    /// A scheme other than http, https or mailto.
    #[error(
        "Links can only use http://, https:// or mailto:, not `{scheme}:`. Use a web address such as https://example.com, or link to an element with `#id` or `@name`."
    )]
    BadScheme {
        /// The refused scheme, lower case.
        scheme: String,
    },
    /// The text has spaces or line breaks.
    #[error("A link cannot contain spaces or line breaks. Write a space as %20, or remove it.")]
    Whitespace,
    /// The text has control characters, quotes, angle brackets or a backslash.
    #[error("A link cannot contain control characters, quotes, angle brackets or backslashes. Remove them.")]
    BadCharacter,
    /// The text is longer than the document allows.
    #[error("The link is {len} bytes long and the most allowed is {max}. Use a shorter address.")]
    TooLong {
        /// Length of the text.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// The text is not a web address.
    #[error(
        "`{shown}` is not a web address. Write it like https://example.com/page, link to an element with `#id` or `@name`, or pass null to remove the link."
    )]
    NotAnAddress {
        /// The start of the text.
        shown: String,
    },
    /// The address has a user name or password in front of the host.
    #[error("A link cannot have a user name or password in front of the host (the part before `@`). Remove that part.")]
    UserInfo,
    /// `#` or `@` without anything after it.
    #[error("An element link needs an id or a name after the `#` or `@`, such as `#e1` or `@box`.")]
    EmptyElementRef,
    /// The element does not exist or was deleted.
    #[error("element {0} does not exist")]
    NotFound(ElementId),
    /// The element is locked.
    #[error("element {0} is locked")]
    Locked(ElementId),
    /// The store refused the change.
    #[error("{0}")]
    Store(String),
}

impl From<StoreError> for LinkError {
    fn from(e: StoreError) -> Self {
        LinkError::Store(e.to_string())
    }
}

/// A link as the user typed it, checked but not yet resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// Empty text: remove the link.
    Clear,
    /// A web link in its stored form.
    Url(String),
    /// An element link: an id, or `@name`. The session resolves it.
    Element(String),
}

/// A stored link, classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stored {
    /// A web link, normalised, safe to hand to the browser.
    Url(String),
    /// The id of the target element (not yet checked against the document).
    Element(String),
}

/// The stored form of an element link.
pub fn element_link(target: &ElementId) -> String {
    format!("{ELEMENT_PREFIX}{}", target.as_str())
}

/// Checks the text a user typed. Spaces around it are ignored.
pub fn parse_input(text: &str) -> Result<Input, LinkError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Input::Clear);
    }
    check_shape(text)?;
    if let Some(rest) = text.strip_prefix(ELEMENT_PREFIX) {
        return element_ref(rest).map(Input::Element);
    }
    if text.starts_with('@') {
        return element_ref(text).map(Input::Element);
    }
    normalize_url(text).map(Input::Url)
}

/// An id or a `@name` after the prefix; the name keeps its `@`.
fn element_ref(rest: &str) -> Result<String, LinkError> {
    if rest.is_empty() || rest == "@" { Err(LinkError::EmptyElementRef) } else { Ok(rest.to_owned()) }
}

/// Classifies the text stored in an element's `link`. A web link is checked again: imported documents can hold
/// anything.
pub fn classify_stored(stored: &str) -> Result<Stored, LinkError> {
    check_shape(stored)?;
    match stored.strip_prefix(ELEMENT_PREFIX) {
        Some(id) if !id.is_empty() => Ok(Stored::Element(id.to_owned())),
        Some(_) => Err(LinkError::EmptyElementRef),
        None => normalize_url(stored).map(Stored::Url),
    }
}

fn check_shape(text: &str) -> Result<(), LinkError> {
    if text.len() > MAX_LINK_BYTES {
        return Err(LinkError::TooLong { len: text.len(), max: MAX_LINK_BYTES });
    }
    if text.chars().any(char::is_whitespace) {
        return Err(LinkError::Whitespace);
    }
    if text.chars().any(|c| c.is_control() || matches!(c, '"' | '<' | '>' | '\\')) {
        return Err(LinkError::BadCharacter);
    }
    Ok(())
}

fn shown(text: &str) -> String {
    let mut s: String = text.chars().take(60).collect();
    if s.len() < text.len() {
        s.push_str("...");
    }
    s
}

/// The scheme of `text` and the rest after its colon, when `text` starts with one. `localhost:3000/x` and
/// `example.com:8080` are host and port, not a scheme.
fn split_scheme(text: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = text.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid {
        return None;
    }
    let port_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let port = rest.get(..port_len).unwrap_or("");
    let is_port = !port.is_empty() && port.chars().all(|c| c.is_ascii_digit());
    (!is_port).then_some((scheme, rest))
}

/// Checks a web address and returns its stored form.
fn normalize_url(text: &str) -> Result<String, LinkError> {
    let not_an_address = || LinkError::NotAnAddress { shown: shown(text) };
    let Some((scheme, rest)) = split_scheme(text) else {
        // No scheme: `example.com/page` means https. A single word is more likely a typo than a host.
        check_authority(text, true).map_err(|e| e.unwrap_or_else(not_an_address))?;
        return Ok(format!("https://{text}"));
    };
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        "http" | "https" => {
            let after = rest.strip_prefix("//").ok_or_else(not_an_address)?;
            check_authority(after, false).map_err(|e| e.unwrap_or_else(not_an_address))?;
            Ok(format!("{scheme}://{after}"))
        }
        "mailto" => {
            let address = rest.split('?').next().unwrap_or("");
            let ok = address.split_once('@').is_some_and(|(user, host)| !user.is_empty() && !host.is_empty());
            if ok { Ok(format!("mailto:{rest}")) } else { Err(not_an_address()) }
        }
        _ => Err(LinkError::BadScheme { scheme }),
    }
}

/// Splits `host[:port]`, with an IPv6 address in brackets. `None` when the brackets or the colon are wrong.
fn split_host_port(authority: &str) -> Option<(&str, Option<&str>)> {
    if authority.starts_with('[') {
        let close = authority.find(']')?;
        let host = authority.get(..=close)?;
        let tail = authority.get(close + 1..)?;
        return match tail {
            "" => Some((host, None)),
            _ => tail.strip_prefix(':').map(|p| (host, Some(p))),
        };
    }
    Some(match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    })
}

fn is_port(port: &str) -> bool {
    !port.is_empty() && port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit()) && port.parse::<u32>().is_ok_and(|n| n <= 65_535)
}

fn is_hostname(host: &str) -> bool {
    host.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '%'))
    })
}

fn is_ipv6(host: &str) -> bool {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .is_some_and(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.')))
}

/// Checks the part before the first `/`, `?` or `#`. `Err(None)` means "not an address" and the caller words it.
/// With `need_dot` a single word such as `hello` is refused (only `localhost` and IPv6 literals pass).
fn check_authority(after_scheme: &str, need_dot: bool) -> Result<(), Option<LinkError>> {
    let end = after_scheme.find(['/', '?', '#']).unwrap_or(after_scheme.len());
    let authority = after_scheme.get(..end).unwrap_or("");
    if authority.contains('@') {
        return Err(Some(LinkError::UserInfo));
    }
    let (host, port) = split_host_port(authority).ok_or(None)?;
    if port.is_some_and(|p| !is_port(p)) {
        return Err(None);
    }
    let name_ok = is_hostname(host) && (!need_dot || host.contains('.') || host.eq_ignore_ascii_case("localhost"));
    if name_ok || is_ipv6(host) { Ok(()) } else { Err(None) }
}

/// Side of the link icon in screen pixels (Excalidraw `DEFAULT_LINK_SIZE`).
pub const ICON_SIZE_PX: f64 = 12.0;
/// How far outside the icon a click still hits it, in screen pixels.
pub const ICON_REACH_PX: f64 = 4.0;

/// The link icon of an element in world units: a square beside the top-right corner of its box, where the north-east
/// resize handle sits when the element is selected, rotated with the element about its centre. The icon keeps
/// its size on screen when zoomed out and shrinks with the zoom when zoomed in (the zoom counts as 1 below 1,
/// as in Excalidraw).
pub fn icon_rect(e: &Element, zoom: f64) -> Rect {
    let z = if zoom.is_finite() && zoom > 1.0 { zoom } else { 1.0 };
    let t = &e.transform;
    let size = ICON_SIZE_PX / z;
    let centering = (ICON_SIZE_PX - 8.0) / (2.0 * z);
    let margin = 4.0 / z;
    let x = t.x + t.w + margin - centering;
    let y = t.y - margin - size + centering;
    let centre = Point::new(t.x + t.w / 2.0, t.y + t.h / 2.0);
    let icon_centre = Point::new(x + size / 2.0, y + size / 2.0);
    let d = icon_centre - centre;
    let (sin, cos) = t.angle.sin_cos();
    let rotated = centre + Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos);
    Rect::new(rotated.x - size / 2.0, rotated.y - size / 2.0, rotated.x + size / 2.0, rotated.y + size / 2.0)
}

/// Whether a world point is on the link icon of `e` (with a small reach around it). False for an element without a
/// link or a deleted one.
pub fn hits_icon(e: &Element, p: Point, zoom: f64) -> bool {
    if e.deleted || e.link.is_none() || !p.x.is_finite() || !p.y.is_finite() {
        return false;
    }
    let reach = if zoom.is_finite() && zoom > 0.0 { ICON_REACH_PX / zoom } else { ICON_REACH_PX };
    icon_rect(e, zoom).inflate(reach, reach).contains(p)
}

/// Sets or clears the `link` of a live, unlocked element. Returns whether it changed. `link` must come from
/// [`parse_input`] and the session's resolution of an element reference.
pub fn set<T: SceneTx>(tx: &mut T, id: &ElementId, link: Option<String>) -> Result<bool, LinkError> {
    let current = tx.get(id).filter(|e| !e.deleted).ok_or_else(|| LinkError::NotFound(id.clone()))?;
    if current.locked {
        return Err(LinkError::Locked(id.clone()));
    }
    if current.link == link {
        return Ok(false);
    }
    tx.update(id, |e| e.link = link)?;
    Ok(true)
}

/// Whether a live element exists under `id`.
pub fn target_exists<R: SceneRead>(scene: &R, id: &ElementId) -> bool {
    scene.get(id).is_some_and(|e| !e.deleted)
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
