//! drafftink-seal: end-to-end encryption for collaboration rooms (layer L1).
//!
//! The room key lives in the link after `#`, so no server ever sees it. Everything a relay stores or forwards is
//! a [`SealedRecord`]. The design is `docs/design/collab-e2e.md`; the wire messages are `drafftink_protocol::v2`.
//!
//! - [`RoomKey`] is 256 random bits. [`RoomKey::subkey`] derives one key per [`Purpose`] with HKDF-SHA256.
//! - [`RoomKeys`] holds the keys of one room and seals and opens records with XChaCha20-Poly1305. Each record has
//!   a random 24-byte nonce, and its header, room and (for pictures) name are authenticated.
//! - [`RoomLink`] reads and writes `#room=<id>,<key>`.
//!
//! Key material is wiped on drop. No type prints a key, a token or a link in `Debug`, and none has `Display`.
//! Errors never contain input text. The crate builds for `wasm32-unknown-unknown` and for native targets.

#![warn(missing_docs)]

mod key;
mod link;
mod random;
mod record;

pub use key::{HKDF_SALT, KEY_BYTES, Purpose, RoomKey, Subkey};
pub use link::{LinkError, RoomLink};
pub use random::{NonceSource, OsNonces, RandomError};
pub use record::{AAD_LABEL, FILE_ID_BYTES, OpenError, RoomKeys, SealError, SealedRecord};
