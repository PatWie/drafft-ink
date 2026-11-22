//! The room link: `#room=<id>,<key>`, both in unpadded base64url (`docs/design/collab-e2e.md` section 5.1).
//!
//! The fragment after `#` never reaches a server. The page keeps it; "Copy invite link" hands it out.

use std::fmt;

use base64ct::{Base64UrlUnpadded, Encoding};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

use drafftink_protocol::v2::RoomId;

use crate::key::{KEY_BYTES, RoomKey};

/// A link is not a link to a sealed room. The text says what to do. It never repeats the link, so an error is safe
/// to show and to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LinkError {
    /// No `room=` part.
    #[error("This is not a room link. A room link ends with #room= followed by the room name, a comma and the key.")]
    NotARoomLink,
    /// A `room=` part with a name and no comma: the link of a plain room.
    #[error("This link names a room but has no key, so it is not an encrypted room. Use the full link you were sent.")]
    NoKey,
    /// The room name is not 22 characters of base64url.
    #[error("The room name in this link is damaged. Copy the whole link again.")]
    BadId,
    /// The key is not 43 characters of base64url.
    #[error("The key in this link is damaged or cut short. Copy the whole link again.")]
    BadKey,
    /// The room name is not the room that the key opens.
    #[error(
        "The key in this link does not belong to its room, so part of the link was changed or lost. Copy the whole link again."
    )]
    IdMismatch,
}

/// A parsed room link: the room's key and its id. `Debug` shows the id only, and there is no `Display`.
pub struct RoomLink {
    id: RoomId,
    key: RoomKey,
}

impl RoomLink {
    /// The link of the room that `key` opens.
    pub fn new(key: RoomKey) -> Self {
        Self { id: key.room_id(), key }
    }

    /// Reads a link. It accepts a whole address (`https://drafft.ink/#room=...`), a fragment (`#room=...`) or the
    /// text after the `#`. Other `&name=value` parts of the fragment (`&server=...`) are ignored, and the caller reads
    /// them. The key must open the room named in the link.
    pub fn parse(text: &str) -> Result<Self, LinkError> {
        let text = text.trim();
        let fragment = text.split_once('#').map_or(text, |(_, fragment)| fragment);
        let value = fragment.split('&').find_map(|part| part.strip_prefix("room=")).ok_or(LinkError::NotARoomLink)?;
        let (id, key) = value.split_once(',').ok_or(LinkError::NoKey)?;
        let id: RoomId = id.parse().map_err(|_| LinkError::BadId)?;
        let key = decode_key(key)?;
        if key.room_id() == id { Ok(Self { id, key }) } else { Err(LinkError::IdMismatch) }
    }

    /// The room id. It is public: it goes into the route and the `Join` frame.
    pub const fn id(&self) -> RoomId {
        self.id
    }

    /// The room key.
    pub const fn key(&self) -> &RoomKey {
        &self.key
    }

    /// The room key, by value.
    pub fn into_key(self) -> RoomKey {
        self.key
    }

    /// The fragment: `#room=<id>,<key>`. It is a secret and is wiped when dropped.
    pub fn fragment(&self) -> Zeroizing<String> {
        let mut encoded = Zeroizing::new(String::new());
        encoded.push_str("#room=");
        encoded.push_str(&self.id.to_string());
        encoded.push(',');
        let mut key = [0u8; 43];
        let text = Base64UrlUnpadded::encode(self.key.expose_bytes(), &mut key).unwrap_or_default();
        encoded.push_str(text);
        key.zeroize();
        encoded
    }

    /// The whole address on `origin` (`https://drafft.ink`, without a trailing slash): `<origin>/#room=<id>,<key>`.
    pub fn url(&self, origin: &str) -> Zeroizing<String> {
        let mut url = Zeroizing::new(String::from(origin.trim_end_matches('/')));
        url.push('/');
        url.push_str(&self.fragment());
        url
    }
}

fn decode_key(text: &str) -> Result<RoomKey, LinkError> {
    // One byte of slack: `decode` refuses a longer key, and a shorter one is checked below.
    let mut buf = Zeroizing::new([0u8; KEY_BYTES]);
    let decoded = Base64UrlUnpadded::decode(text, buf.as_mut_slice()).map_err(|_| LinkError::BadKey)?;
    if decoded.len() != KEY_BYTES {
        return Err(LinkError::BadKey);
    }
    Ok(RoomKey::from_bytes(*buf))
}

impl fmt::Debug for RoomLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomLink").field("id", &self.id).finish_non_exhaustive()
    }
}
