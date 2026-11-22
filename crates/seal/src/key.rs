//! The room key and the subkeys derived from it (`docs/design/collab-e2e.md` section 5).
//!
//! `K_root` is 32 random bytes. `PRK = HKDF-Extract(salt = "drafft.ink room v1", IKM = K_root)`, and each subkey is
//! `HKDF-Expand(PRK, info, 32)` with its own `info` label, so a key made for one purpose is never an oracle for another.

use std::fmt;

use hkdf::Hkdf;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use drafftink_protocol::v2::{MEMBERSHIP_TOKEN_BYTES, MembershipToken, RoomId};

use crate::random::{self, RandomError};

/// Size of the room key and of every subkey.
pub const KEY_BYTES: usize = 32;
/// The HKDF salt. It names the format, so a new format derives different keys from the same room key.
pub const HKDF_SALT: &[u8] = b"drafft.ink room v1";

// HKDF-Expand makes at most 255 blocks of the hash size, and a subkey is one.
const _: () = assert!(KEY_BYTES <= 255 * 32);
// A membership token is a subkey.
const _: () = assert!(MEMBERSHIP_TOKEN_BYTES == KEY_BYTES);

/// What a subkey is for. Each purpose has its own `info` label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// `K_content`: seals updates and snapshots.
    Content,
    /// `K_presence`: seals presence and direct peer messages.
    Presence,
    /// `K_image`: seals image files.
    Image,
    /// `K_name`: the HMAC key that turns a file id into the name a relay sees.
    ImageName,
    /// The membership token: proves to a relay that the joiner holds the key.
    Membership,
}

impl Purpose {
    /// The HKDF `info` string.
    pub const fn label(self) -> &'static [u8] {
        match self {
            Purpose::Content => b"drafft v1 content",
            Purpose::Presence => b"drafft v1 presence",
            Purpose::Image => b"drafft v1 image",
            Purpose::ImageName => b"drafft v1 image-name",
            Purpose::Membership => b"drafft v1 membership",
        }
    }
}

/// A 32-byte key derived from the room key. It is wiped on drop, and `Debug` does not print it.
pub struct Subkey([u8; KEY_BYTES]);

impl Subkey {
    /// The key bytes. Handle them as a secret.
    pub const fn expose_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

impl Drop for Subkey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Subkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Subkey(..)")
    }
}

/// The root key of a room: 256 random bits. Whoever holds it can read and write the room.
///
/// It lives in the link after `#`, so no server ever sees it. It is wiped on drop. `Debug` does not print it, and
/// there is no `Display`. [`RoomKey::expose_bytes`] is the one way to read it.
pub struct RoomKey([u8; KEY_BYTES]);

impl RoomKey {
    /// A new random key.
    pub fn generate() -> Result<Self, RandomError> {
        let mut bytes = [0u8; KEY_BYTES];
        random::fill(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// Wraps 32 bytes that came from a link or from the local workspace.
    pub const fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    /// The key bytes. Handle them as a secret.
    pub const fn expose_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }

    /// Derives the subkey for `purpose`.
    pub fn subkey(&self, purpose: Purpose) -> Subkey {
        let hkdf = Hkdf::<Sha256>::new(Some(HKDF_SALT), &self.0);
        let mut okm = [0u8; KEY_BYTES];
        if hkdf.expand(purpose.label(), &mut okm).is_err() {
            // Unreachable: `KEY_BYTES` is at most 255 blocks (checked at compile time above).
            unreachable!("HKDF-Expand refused to make {KEY_BYTES} bytes");
        }
        Subkey(okm)
    }

    /// The membership token: what a joiner shows a relay.
    pub fn membership_token(&self) -> MembershipToken {
        let subkey = self.subkey(Purpose::Membership);
        MembershipToken::from_bytes(*subkey.expose_bytes())
    }

    /// The id of the room this key opens: a hash of the membership token.
    pub fn room_id(&self) -> RoomId {
        RoomId::from_token(&self.membership_token())
    }
}

impl Drop for RoomKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl PartialEq for RoomKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl Eq for RoomKey {}

impl fmt::Debug for RoomKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RoomKey(..)")
    }
}
