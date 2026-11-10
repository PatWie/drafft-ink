//! Identifiers: [`RoomId`] and [`ClientId`].

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::ValidationError;
use crate::limits::MAX_ROOM_ID_BYTES;

/// A room name. 1 to 128 bytes of ASCII letters, digits, `-`, `_` or `.`.
///
/// Without auth the room id is the only secret that keeps strangers out, so clients generate
/// it from at least 128 random bits (see [`RoomId::from_random_bytes`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RoomId(String);

impl RoomId {
    /// Checks and wraps a room id.
    pub fn new(id: impl Into<String>) -> Result<Self, ValidationError> {
        let id = id.into();
        if id.is_empty() {
            return Err(ValidationError::new("room", "must not be empty"));
        }
        if id.len() > MAX_ROOM_ID_BYTES {
            return Err(ValidationError::new("room", "is too long"));
        }
        if !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(ValidationError::new(
                "room",
                "may only contain letters, digits, '-', '_' and '.'",
            ));
        }
        Ok(RoomId(id))
    }

    /// Makes an unguessable room id (32 lowercase hex characters) from 16 random bytes.
    pub fn from_random_bytes(bytes: [u8; 16]) -> Self {
        RoomId(hex_encode(&bytes))
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RoomId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for RoomId {
    type Error = ValidationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        RoomId::new(value)
    }
}

impl Serialize for RoomId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RoomId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        RoomId::new(s).map_err(serde::de::Error::custom)
    }
}

/// A client's identity inside a room: 16 bytes, chosen by the client and stable across
/// reconnects (a reconnecting client keeps its id so the server can replace the old socket).
/// A UUID v4 fits exactly.
///
/// The server stamps this id on every relayed message and never trusts one sent by a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId([u8; 16]);

impl ClientId {
    /// Wraps 16 bytes (for example the bytes of a UUID).
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        ClientId(bytes)
    }

    /// The raw bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Parses 32 hex characters.
    pub fn from_hex(text: &str) -> Result<Self, ValidationError> {
        let bytes = hex_decode(text, 16)
            .ok_or_else(|| ValidationError::new("client_id", "must be 32 hex characters"))?;
        let mut out = [0u8; 16];
        out.copy_from_slice(&bytes);
        Ok(ClientId(out))
    }
}

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex_encode(&self.0))
    }
}

impl Serialize for ClientId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex_encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for ClientId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        ClientId::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0f)]));
    }
    out
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decodes hex. When `exact` is not 0 the result must have exactly that many bytes.
pub(crate) fn hex_decode(text: &str, exact: usize) -> Option<Vec<u8>> {
    let raw = text.as_bytes();
    if !raw.len().is_multiple_of(2) || (exact != 0 && raw.len() != exact * 2) {
        return None;
    }
    raw.chunks_exact(2)
        .map(|pair| Some(nibble(pair[0])? << 4 | nibble(pair[1])?))
        .collect()
}
