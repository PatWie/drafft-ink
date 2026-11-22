//! Sealed records: the bytes a relay stores and forwards.
//!
//! ```text
//! SealedRecord = header || nonce[24] || ciphertext || tag[16]
//! plaintext    = len u32 LE | payload | zero padding              (no padding in v1)
//! AAD          = "drafft.ink e2e v1" | room_id [16] | header [34] | image name [16]   (the name only for images)
//! ```
//!
//! Everything a relay or a receiver reads in plaintext is in the associated data, so changing the sender, the
//! counter, the kind, the snapshot cover, the room or the image name makes the record unreadable. See
//! `docs/design/collab-e2e.md` section 6.3.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroize;

use drafftink_protocol::v2::{ImageName, MembershipToken, RecordError, RecordHeader, RecordKind, RoomId};
use drafftink_protocol::{
    MAX_IMAGE_BYTES, MAX_PRESENCE_RECORD_BYTES, MAX_RECORD_BYTES, MAX_SNAPSHOT_RECORD_BYTES, RECORD_HEADER_BYTES,
    RECORD_LEN_PREFIX_BYTES, RECORD_NONCE_BYTES, RECORD_TAG_BYTES, SEAL_OVERHEAD,
};

use crate::key::{KEY_BYTES, Purpose, RoomKey, Subkey};
use crate::random::{NonceSource, OsNonces, RandomError};

/// The first bytes of every associated data: the format.
pub const AAD_LABEL: &[u8] = b"drafft.ink e2e v1";
/// Size of the SHA-256 hash that a keyed image name is made from, and of a file id.
pub const FILE_ID_BYTES: usize = 32;

/// A sealed record: header, nonce, ciphertext and tag. It is the byte string a relay stores. Making one with
/// [`SealedRecord::from_bytes`] checks the header only. Whether it is authentic, only a holder of the key learns by
/// opening it.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedRecord {
    bytes: Vec<u8>,
    header: RecordHeader,
    nonce: [u8; RECORD_NONCE_BYTES],
}

impl SealedRecord {
    /// Wraps the bytes of a record received from a relay or read from storage.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, RecordError> {
        let header = RecordHeader::peek(&bytes)?;
        let nonce = bytes
            .get(RECORD_HEADER_BYTES..RECORD_HEADER_BYTES + RECORD_NONCE_BYTES)
            .and_then(|n| <[u8; RECORD_NONCE_BYTES]>::try_from(n).ok())
            .ok_or(RecordError::TooShort { len: bytes.len() })?;
        Ok(Self { bytes, header, nonce })
    }

    /// The bytes to send or store.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes to send or store.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The plaintext header.
    pub const fn header(&self) -> &RecordHeader {
        &self.header
    }

    /// The nonce.
    pub const fn nonce(&self) -> &[u8; RECORD_NONCE_BYTES] {
        &self.nonce
    }
}

impl std::fmt::Debug for SealedRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedRecord").field("header", &self.header).field("len", &self.bytes.len()).finish()
    }
}

/// A record could not be sealed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SealError {
    /// The payload is larger than a record of this kind may be.
    #[error("This {kind:?} is {len} bytes, which is more than the {max} bytes a room can take.")]
    TooLarge {
        /// The kind of the record.
        kind: RecordKind,
        /// The payload size.
        len: usize,
        /// The most the kind takes.
        max: usize,
    },
    /// An image needs a name, so it is sealed with `seal_image`, and `seal` refuses the image kind.
    #[error("A picture is sealed with its name, with `seal_image`.")]
    ImageKind,
    /// No random bytes for the nonce.
    #[error(transparent)]
    Random(#[from] RandomError),
    /// The cipher refused the input. It does this only for inputs far larger than any limit here.
    #[error("The cipher refused to seal this record.")]
    Cipher,
}

/// A record could not be opened. None of these says why in more detail than "wrong": a receiver should not tell a
/// sender which part failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum OpenError {
    /// The record is of another kind than the caller expected.
    #[error("This record is a {found:?}, but a {expected:?} was expected.")]
    WrongKind {
        /// The kind the caller asked for.
        expected: RecordKind,
        /// The kind in the header.
        found: RecordKind,
    },
    /// An image is opened with its name, with `open_image`, and `open` refuses the image kind.
    #[error("A picture is opened with its name, with `open_image`.")]
    ImageKind,
    /// The key, the room, the header or the name does not match, or the record was changed. The cipher cannot
    /// tell these apart.
    #[error("This record was not sealed with this room's key for this room, or it was changed on the way.")]
    NotAuthentic,
    /// The record is authentic but its inner length or padding is wrong. Only a holder of the key can make this.
    #[error("This record is authentic but its contents are malformed.")]
    Malformed,
}

/// The cipher key of one purpose.
struct Cipher(XChaCha20Poly1305);

impl Cipher {
    fn new(subkey: &Subkey) -> Self {
        let mut key = Key::from(*subkey.expose_bytes());
        let cipher = XChaCha20Poly1305::new(&key);
        key.as_mut_slice().zeroize();
        Self(cipher)
    }
}

/// Every key of one room, derived once from its [`RoomKey`]: the ciphers, the image-name key, the membership token
/// and the room id. `Debug` prints the room id only.
pub struct RoomKeys {
    room_id: RoomId,
    token: MembershipToken,
    content: Cipher,
    presence: Cipher,
    image: Cipher,
    name: Subkey,
}

impl std::fmt::Debug for RoomKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoomKeys").field("room_id", &self.room_id).finish_non_exhaustive()
    }
}

impl RoomKeys {
    /// Derives every key of the room that `key` opens.
    pub fn new(key: &RoomKey) -> Self {
        let token = key.membership_token();
        Self {
            room_id: RoomId::from_token(&token),
            token,
            content: Cipher::new(&key.subkey(Purpose::Content)),
            presence: Cipher::new(&key.subkey(Purpose::Presence)),
            image: Cipher::new(&key.subkey(Purpose::Image)),
            name: key.subkey(Purpose::ImageName),
        }
    }

    /// The id of the room.
    pub const fn room_id(&self) -> RoomId {
        self.room_id
    }

    /// The membership token to show a relay.
    pub const fn membership_token(&self) -> &MembershipToken {
        &self.token
    }

    /// The name a relay stores the image under: `HMAC-SHA256(K_name, file_id)[..16]`. `file_id` is the 32-byte
    /// SHA-256 of the picture's bytes. The same picture has the same name in one room and unrelated names in two.
    pub fn image_name(&self, file_id: &[u8; FILE_ID_BYTES]) -> ImageName {
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(self.name.expose_bytes()) else {
            // Unreachable: HMAC takes a key of any length.
            unreachable!("HMAC refused a {KEY_BYTES} byte key");
        };
        mac.update(file_id);
        let tag = mac.finalize().into_bytes();
        let mut name = [0u8; drafftink_protocol::v2::IMAGE_NAME_BYTES];
        name.copy_from_slice(&tag[..drafftink_protocol::v2::IMAGE_NAME_BYTES]);
        ImageName::from_bytes(name)
    }

    /// Seals `payload` under a fresh random nonce. The kind of the header decides the key: updates and snapshots
    /// use the content key, presence and direct messages the presence key.
    pub fn seal(&self, header: RecordHeader, payload: &[u8]) -> Result<SealedRecord, SealError> {
        self.seal_with(&mut OsNonces, header, payload)
    }

    /// Like [`seal`](Self::seal), with nonces from `nonces`. For tests that need a known nonce.
    pub fn seal_with(
        &self,
        nonces: &mut impl NonceSource,
        header: RecordHeader,
        payload: &[u8],
    ) -> Result<SealedRecord, SealError> {
        if header.kind == RecordKind::Image {
            return Err(SealError::ImageKind);
        }
        self.seal_inner(nonces, header, None, payload)
    }

    /// Seals a picture under a fresh random nonce. The name is part of the associated data, so the sealed picture
    /// opens only under the name it was stored with.
    pub fn seal_image(&self, name: &ImageName, payload: &[u8]) -> Result<SealedRecord, SealError> {
        self.seal_image_with(&mut OsNonces, name, payload)
    }

    /// Like [`seal_image`](Self::seal_image), with nonces from `nonces`. For tests that need a known nonce.
    pub fn seal_image_with(
        &self,
        nonces: &mut impl NonceSource,
        name: &ImageName,
        payload: &[u8],
    ) -> Result<SealedRecord, SealError> {
        self.seal_inner(nonces, RecordHeader::image(), Some(name), payload)
    }

    /// Opens a record of the `expected` kind and returns its payload.
    pub fn open(&self, expected: RecordKind, record: &SealedRecord) -> Result<Vec<u8>, OpenError> {
        match (expected, record.header.kind) {
            (RecordKind::Image, _) => Err(OpenError::ImageKind),
            (_, found) if found != expected => Err(OpenError::WrongKind { expected, found }),
            _ => self.open_inner(record, None),
        }
    }

    /// Opens a picture sealed under `name` and returns its bytes. The caller checks the SHA-256 of the bytes
    /// against the file id (a member can store junk under a real name).
    pub fn open_image(&self, name: &ImageName, record: &SealedRecord) -> Result<Vec<u8>, OpenError> {
        match record.header.kind {
            RecordKind::Image => self.open_inner(record, Some(name)),
            found => Err(OpenError::WrongKind { expected: RecordKind::Image, found }),
        }
    }

    fn cipher(&self, kind: RecordKind) -> &Cipher {
        match kind {
            RecordKind::Update | RecordKind::Snapshot => &self.content,
            RecordKind::Presence | RecordKind::Direct => &self.presence,
            RecordKind::Image => &self.image,
        }
    }

    fn aad(&self, header: &[u8; RECORD_HEADER_BYTES], name: Option<&ImageName>) -> Vec<u8> {
        let mut aad = Vec::with_capacity(AAD_LABEL.len() + 16 + RECORD_HEADER_BYTES + 16);
        aad.extend_from_slice(AAD_LABEL);
        aad.extend_from_slice(self.room_id.as_bytes());
        aad.extend_from_slice(header);
        if let Some(name) = name {
            aad.extend_from_slice(name.as_bytes());
        }
        aad
    }

    fn seal_inner(
        &self,
        nonces: &mut impl NonceSource,
        header: RecordHeader,
        name: Option<&ImageName>,
        payload: &[u8],
    ) -> Result<SealedRecord, SealError> {
        let max = max_payload(header.kind);
        let too_large = || SealError::TooLarge { kind: header.kind, len: payload.len(), max };
        if payload.len() > max {
            return Err(too_large());
        }
        let len = u32::try_from(payload.len()).map_err(|_| too_large())?;
        let mut plaintext = Vec::with_capacity(RECORD_LEN_PREFIX_BYTES + payload.len());
        plaintext.extend_from_slice(&len.to_le_bytes());
        plaintext.extend_from_slice(payload);

        let header_bytes = header.to_bytes();
        let nonce = nonces.next_nonce()?;
        let aad = self.aad(&header_bytes, name);
        let ciphertext = self
            .cipher(header.kind)
            .0
            .encrypt(&XNonce::from(nonce), Payload { msg: &plaintext, aad: &aad })
            .map_err(|_| SealError::Cipher)?;

        let mut bytes = Vec::with_capacity(RECORD_HEADER_BYTES + RECORD_NONCE_BYTES + ciphertext.len());
        bytes.extend_from_slice(&header_bytes);
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&ciphertext);
        Ok(SealedRecord { bytes, header, nonce })
    }

    fn open_inner(&self, record: &SealedRecord, name: Option<&ImageName>) -> Result<Vec<u8>, OpenError> {
        let ciphertext = record.bytes.get(RECORD_HEADER_BYTES + RECORD_NONCE_BYTES..).ok_or(OpenError::NotAuthentic)?;
        // The header was parsed strictly, so writing it again gives the bytes that were sent.
        let aad = self.aad(&record.header.to_bytes(), name);
        let plaintext = self
            .cipher(record.header.kind)
            .0
            .decrypt(&XNonce::from(record.nonce), Payload { msg: ciphertext, aad: &aad })
            .map_err(|_| OpenError::NotAuthentic)?;
        unwrap_payload(&plaintext)
    }
}

/// Cuts the length prefix and the zero padding off an authenticated plaintext.
fn unwrap_payload(plaintext: &[u8]) -> Result<Vec<u8>, OpenError> {
    let (prefix, rest) = plaintext.split_first_chunk::<RECORD_LEN_PREFIX_BYTES>().ok_or(OpenError::Malformed)?;
    let len = usize::try_from(u32::from_le_bytes(*prefix)).map_err(|_| OpenError::Malformed)?;
    let (payload, padding) = rest.split_at_checked(len).ok_or(OpenError::Malformed)?;
    if padding.iter().any(|&b| b != 0) {
        return Err(OpenError::Malformed);
    }
    Ok(payload.to_vec())
}

/// The largest payload of a record of `kind`: what the relay accepts, less the seal.
const fn max_payload(kind: RecordKind) -> usize {
    match kind {
        RecordKind::Update | RecordKind::Direct => MAX_RECORD_BYTES - SEAL_OVERHEAD,
        RecordKind::Snapshot => MAX_SNAPSHOT_RECORD_BYTES - SEAL_OVERHEAD,
        RecordKind::Presence => MAX_PRESENCE_RECORD_BYTES - SEAL_OVERHEAD,
        RecordKind::Image => MAX_IMAGE_BYTES,
    }
}

// The tag is the last 16 bytes of the ciphertext; the constants must add up to the overhead.
const _: () =
    assert!(RECORD_HEADER_BYTES + RECORD_NONCE_BYTES + RECORD_LEN_PREFIX_BYTES + RECORD_TAG_BYTES == SEAL_OVERHEAD);

#[cfg(test)]
mod tests {
    use super::*;

    /// The room id is in the associated data on its own, not only through the key it is derived from: the same
    /// keys under another room id open nothing.
    #[test]
    fn the_room_id_is_authenticated() {
        let key = RoomKey::from_bytes([1; KEY_BYTES]);
        let here = RoomKeys::new(&key);
        let mut elsewhere = RoomKeys::new(&key);
        elsewhere.room_id = RoomId::from_bytes([9; 16]);
        let header =
            RecordHeader::new(RecordKind::Update, drafftink_protocol::ClientId::from_bytes([2; 16]), 1, 0).unwrap();
        let sealed = here.seal(header, b"payload").unwrap();
        assert_eq!(here.open(sealed.header().kind, &sealed).as_deref(), Ok(&b"payload"[..]));
        assert_eq!(elsewhere.open(sealed.header().kind, &sealed), Err(OpenError::NotAuthentic));
    }
}
