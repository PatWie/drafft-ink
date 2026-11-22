//! The plaintext header of a sealed record. A relay reads the header and nothing else of a record.
//!
//! ```text
//! SealedRecord = header || nonce[24] || ciphertext || tag[16]
//! header       = version u8 (=1) | kind u8 | sender [16] | ctr u64 LE | covers_seq u64 LE      (34 bytes)
//! ```
//!
//! The header is part of the associated data of the cipher (`drafftink-seal`), so changing a byte of it makes the
//! record unreadable. See `docs/design/collab-e2e.md` section 6.3.

use thiserror::Error;

use crate::ids::ClientId;
use crate::limits::{RECORD_HEADER_BYTES, SEAL_OVERHEAD};

/// The format version of a sealed record, the first byte of its header.
pub const RECORD_FORMAT_VERSION: u8 = 1;

/// What a sealed record carries. The number is the `kind` byte of the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RecordKind {
    /// A Loro update, relayed live and kept in the log.
    Update = 1,
    /// A full Loro snapshot that replaces the log up to `covers_seq`.
    Snapshot = 2,
    /// A cursor, a selection or a name. Relayed, never stored.
    Presence = 3,
    /// A peer-to-peer request or reply (state transfer when the relay's storage fails).
    Direct = 4,
    /// An image file. Its header has no sender, no counter and no cover.
    Image = 5,
}

impl RecordKind {
    /// The `kind` byte.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses the `kind` byte.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(RecordKind::Update),
            2 => Some(RecordKind::Snapshot),
            3 => Some(RecordKind::Presence),
            4 => Some(RecordKind::Direct),
            5 => Some(RecordKind::Image),
            _ => None,
        }
    }
}

/// Why a header or a record is not acceptable. The text is for people and carries no record bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RecordError {
    /// The record is shorter than the smallest sealed record.
    #[error("a sealed record is at least {SEAL_OVERHEAD} bytes, this one has {len}")]
    TooShort {
        /// Length found.
        len: usize,
    },
    /// The format version is not 1. The record comes from a newer build.
    #[error("unknown record format version {0}")]
    UnknownVersion(u8),
    /// The kind byte is not one of the five kinds.
    #[error("unknown record kind {0}")]
    UnknownKind(u8),
    /// A field that must be zero for this kind is not.
    #[error("a {kind:?} record must not set {field}")]
    MustBeZero {
        /// The kind of the record.
        kind: RecordKind,
        /// The field.
        field: &'static str,
    },
}

/// The 34 plaintext bytes in front of every sealed record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHeader {
    /// What the record carries.
    pub kind: RecordKind,
    /// The client that sealed it. All zeros for an image.
    pub sender: ClientId,
    /// The sender's counter: 1, 2, 3, ... per sender. A receiver notices a gap. Zero for an image.
    pub ctr: u64,
    /// For a snapshot: the last relay sequence number it includes. Zero for every other kind.
    pub covers_seq: u64,
}

impl RecordHeader {
    /// A header with every field checked: only snapshots have a cover, and an image has no sender and no counter.
    pub fn new(kind: RecordKind, sender: ClientId, ctr: u64, covers_seq: u64) -> Result<Self, RecordError> {
        let zero = |field, is_zero: bool| if is_zero { Ok(()) } else { Err(RecordError::MustBeZero { kind, field }) };
        zero("covers_seq", kind == RecordKind::Snapshot || covers_seq == 0)?;
        zero("sender", kind != RecordKind::Image || sender.as_bytes() == &[0; 16])?;
        zero("ctr", kind != RecordKind::Image || ctr == 0)?;
        Ok(Self { kind, sender, ctr, covers_seq })
    }

    /// The header of an image record.
    pub const fn image() -> Self {
        Self { kind: RecordKind::Image, sender: ClientId::from_bytes([0; 16]), ctr: 0, covers_seq: 0 }
    }

    /// The 34 bytes, as they appear in the record and in the associated data.
    pub fn to_bytes(&self) -> [u8; RECORD_HEADER_BYTES] {
        let mut out = [0u8; RECORD_HEADER_BYTES];
        out[0] = RECORD_FORMAT_VERSION;
        out[1] = self.kind.as_u8();
        out[2..18].copy_from_slice(self.sender.as_bytes());
        out[18..26].copy_from_slice(&self.ctr.to_le_bytes());
        out[26..34].copy_from_slice(&self.covers_seq.to_le_bytes());
        out
    }

    /// Reads the header of a whole sealed record. It checks the length (the smallest record is
    /// [`SEAL_OVERHEAD`] bytes), the format version, the kind and the fields that must be zero. It does not and
    /// cannot check that the record is authentic: only a holder of the key can.
    pub fn peek(record: &[u8]) -> Result<Self, RecordError> {
        let too_short = RecordError::TooShort { len: record.len() };
        if record.len() < SEAL_OVERHEAD {
            return Err(too_short);
        }
        let (&[version], rest) = record.split_first_chunk().ok_or(too_short)?;
        if version != RECORD_FORMAT_VERSION {
            return Err(RecordError::UnknownVersion(version));
        }
        let (&[kind], rest) = rest.split_first_chunk().ok_or(too_short)?;
        let kind = RecordKind::from_u8(kind).ok_or(RecordError::UnknownKind(kind))?;
        let (sender, rest) = rest.split_first_chunk::<16>().ok_or(too_short)?;
        let (ctr, rest) = rest.split_first_chunk::<8>().ok_or(too_short)?;
        let (covers_seq, _) = rest.split_first_chunk::<8>().ok_or(too_short)?;
        Self::new(kind, ClientId::from_bytes(*sender), u64::from_le_bytes(*ctr), u64::from_le_bytes(*covers_seq))
    }
}
