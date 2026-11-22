//! Names and secrets of a sealed room: [`RoomId`], [`ImageName`], [`MembershipToken`] and [`Proof`].
//!
//! The room id is a hash of the membership token (`docs/design/collab-e2e.md` section 5.3), so a relay checks
//! membership without storing anything and without ever seeing the room key.

use std::fmt;
use std::str::FromStr;

use base64ct::{Base64UrlUnpadded, Encoding};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::error::{Rule, ValidationError};

/// Domain label hashed in front of the membership token to make a room id.
pub const ROOM_ID_LABEL: &[u8] = b"drafft v1 room-id";
/// Size of a room id.
pub const ROOM_ID_BYTES: usize = 16;
/// Size of a keyed image name.
pub const IMAGE_NAME_BYTES: usize = 16;
/// Size of a membership token.
pub const MEMBERSHIP_TOKEN_BYTES: usize = 32;

/// Decodes unpadded base64url into exactly `N` bytes. `None` for any other length, character or padding.
fn decode_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    let mut out = [0u8; N];
    let decoded = Base64UrlUnpadded::decode(text, &mut out).ok()?;
    (decoded.len() == N).then_some(out)
}

macro_rules! base64url_id {
    ($(#[$meta:meta])* $name:ident, $len:expr, $field:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name([u8; $len]);

        impl $name {
            /// Wraps the raw bytes.
            pub const fn from_bytes(bytes: [u8; $len]) -> Self {
                Self(bytes)
            }

            /// The raw bytes.
            pub const fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }
        }

        /// The unpadded base64url text used in links and routes.
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&Base64UrlUnpadded::encode_string(&self.0))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self)
            }
        }

        impl FromStr for $name {
            type Err = ValidationError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                decode_array(text).map(Self).ok_or_else(|| ValidationError::new($field, Rule::NotBase64Url))
            }
        }
    };
}

base64url_id!(
    /// The id of a sealed room: the first 16 bytes of `SHA-256("drafft v1 room-id" || membership token)`.
    ///
    /// It is public: it goes into the link, the route (`/r/<id>/ws`) and the `Join` frame. It is not related to
    /// [`crate::RoomId`], the name of a plaintext (v1) room.
    RoomId,
    ROOM_ID_BYTES,
    "room"
);

base64url_id!(
    /// The name under which a relay stores a sealed image: `HMAC-SHA256(K_name, file_id)[..16]`. The relay never
    /// sees the content hash.
    ImageName,
    IMAGE_NAME_BYTES,
    "image name"
);

impl RoomId {
    /// The id of the room that `token` belongs to.
    pub fn from_token(token: &MembershipToken) -> Self {
        Self::hash(&token.0)
    }

    fn hash(token: &[u8]) -> Self {
        let digest = Sha256::new().chain_update(ROOM_ID_LABEL).chain_update(token).finalize();
        let mut id = [0u8; ROOM_ID_BYTES];
        id.copy_from_slice(&digest[..ROOM_ID_BYTES]);
        Self(id)
    }

    /// Whether `proof` is a membership proof for this room. Compares in constant time. A proof of the wrong
    /// scheme or length is simply not a proof.
    pub fn accepts(&self, proof: &Proof) -> bool {
        let token_ok = proof.scheme == ProofScheme::Token && proof.bytes.len() == MEMBERSHIP_TOKEN_BYTES;
        // Hash whatever was sent so that the time does not depend on the length check above.
        let derived = Self::hash(&proof.bytes);
        bool::from(derived.0.ct_eq(&self.0)) & token_ok
    }
}

/// A 32-byte secret derived from the room key. Whoever holds it can join the room and fetch its ciphertext, but
/// cannot decrypt anything. It is wiped on drop, and `Debug` does not print it.
#[derive(Clone)]
pub struct MembershipToken([u8; MEMBERSHIP_TOKEN_BYTES]);

impl Drop for MembershipToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl MembershipToken {
    /// Wraps 32 bytes.
    pub const fn from_bytes(bytes: [u8; MEMBERSHIP_TOKEN_BYTES]) -> Self {
        Self(bytes)
    }

    /// The raw bytes. Handle them as a secret.
    pub const fn expose_bytes(&self) -> &[u8; MEMBERSHIP_TOKEN_BYTES] {
        &self.0
    }
}

impl PartialEq for MembershipToken {
    fn eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl Eq for MembershipToken {}

impl fmt::Debug for MembershipToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MembershipToken(..)")
    }
}

/// How a joiner proves that it belongs to a room. The number is the `scheme` byte of the `Join` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProofScheme {
    /// The membership token itself, checked against the room id by hashing.
    Token = 1,
}

impl ProofScheme {
    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(ProofScheme::Token),
            _ => None,
        }
    }
}

/// The `scheme` and `proof` fields of a `Join`. The bytes are a secret: they are wiped on drop and `Debug` does not
/// print them. A proof of the wrong length is representable, so that a relay can answer it with `Unauthorized`
/// and not with a decoding error.
#[derive(Clone)]
pub struct Proof {
    scheme: ProofScheme,
    bytes: Vec<u8>,
}

impl Proof {
    /// A proof by membership token.
    pub fn token(token: &MembershipToken) -> Self {
        Self { scheme: ProofScheme::Token, bytes: token.0.to_vec() }
    }

    /// A proof of any scheme and length. The membership token is [`MEMBERSHIP_TOKEN_BYTES`] long, and a relay
    /// that gets another length answers `Unauthorized`.
    pub fn new(scheme: ProofScheme, bytes: Vec<u8>) -> Self {
        Self { scheme, bytes }
    }

    /// The scheme.
    pub const fn scheme(&self) -> ProofScheme {
        self.scheme
    }

    /// The proof bytes. Handle them as a secret.
    pub fn expose_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Proof {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

impl PartialEq for Proof {
    fn eq(&self, other: &Self) -> bool {
        self.scheme == other.scheme && bool::from(self.bytes.as_slice().ct_eq(other.bytes.as_slice()))
    }
}

impl Eq for Proof {}

impl fmt::Debug for Proof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Proof").field("scheme", &self.scheme).field("bytes", &"..").finish()
    }
}
