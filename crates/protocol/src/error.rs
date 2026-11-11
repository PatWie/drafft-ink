//! Typed errors for encoding, decoding and validation.

use thiserror::Error;

/// Which rule a field broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Rule {
    /// The value is empty.
    #[error("must not be empty")]
    Empty,
    /// The value is longer than its limit.
    #[error("is too long")]
    TooLong,
    /// A character outside the allowed set (room ids: letters, digits, `-`, `_`, `.`).
    #[error("may only contain letters, digits, '-', '_' and '.'")]
    BadCharacter,
    /// Not exactly 32 hexadecimal digits.
    #[error("must be 32 hex characters")]
    NotHex128,
    /// A number is NaN or infinite.
    #[error("must be a finite number")]
    NotFinite,
    /// A colour has bits above `0xRRGGBB`.
    #[error("must fit in 0xRRGGBB")]
    ColorRange,
    /// A size is negative.
    #[error("size must not be negative")]
    NegativeSize,
    /// A limit is above the hard limit of the protocol.
    #[error("above the hard limit")]
    AboveHardLimit,
    /// A range has its minimum above its maximum.
    #[error("min is above max")]
    InvertedRange,
}

/// A field or value broke a protocol rule.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid {field}: {reason}")]
pub struct ValidationError {
    /// The field that broke the rule.
    pub field: &'static str,
    /// The rule it broke. Match on it; the `Display` text is for people.
    pub reason: Rule,
}

impl ValidationError {
    pub(crate) fn new(field: &'static str, reason: Rule) -> Self {
        Self { field, reason }
    }
}

/// A message could not be turned into bytes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EncodeError {
    /// The message breaks a protocol rule (for example, an update larger than the limit).
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    /// The finished frame is larger than [`crate::MAX_FRAME_BYTES`].
    #[error("frame of {len} bytes exceeds the {max} byte limit")]
    FrameTooLarge {
        /// Size of the finished frame.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// JSON serialization failed. Holds the message of the `serde_json` error (see [`DecodeError::Json`]).
    #[error("json: {0}")]
    Json(String),
}

/// Bytes or text could not be turned into a message. Every input is hostile: none of these
/// cases panics, and none allocates more than the input length.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// The frame has no bytes.
    #[error("empty frame")]
    Empty,
    /// The frame is larger than the limit.
    #[error("frame of {len} bytes exceeds the {max} byte limit")]
    FrameTooLarge {
        /// Size of the frame.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// The frame ends before the message does.
    #[error("frame ends early while reading {0}")]
    Truncated(&'static str),
    /// The message ended but bytes remain.
    #[error("{0} unexpected trailing bytes")]
    TrailingBytes(usize),
    /// The first byte is not a message tag of this protocol version.
    #[error("unknown message tag {0:#04x}")]
    UnknownTag(u8),
    /// An enum or flag byte has a value this version does not define.
    #[error("unknown value {value} for {field}")]
    UnknownValue {
        /// The field.
        field: &'static str,
        /// The value found.
        value: u32,
    },
    /// A length prefix is larger than the field allows.
    #[error("{field} has length {len}, the limit is {max}")]
    FieldTooLong {
        /// The field.
        field: &'static str,
        /// The declared length.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// A string is not valid UTF-8.
    #[error("{0} is not valid UTF-8")]
    InvalidUtf8(&'static str),
    /// A length prefix is not a valid variable-length integer.
    #[error("invalid length prefix in {0}")]
    BadVarint(&'static str),
    /// The message decoded but breaks a protocol rule.
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    /// The client's protocol version range does not overlap the one this build speaks.
    #[error(
        "no common protocol version: client speaks {client_min}..={client_max}, this side speaks {ours_min}..={ours_max}"
    )]
    UnsupportedVersion {
        /// Lowest version the client speaks.
        client_min: u16,
        /// Highest version the client speaks.
        client_max: u16,
        /// Lowest version this side speaks.
        ours_min: u16,
        /// Highest version this side speaks.
        ours_max: u16,
    },
    /// The JSON text is not a valid message. This holds the message of the `serde_json` error and not the error:
    /// `DecodeError` is `Clone + PartialEq + Eq`, which the tests and the server's error replies rely on.
    #[error("json: {0}")]
    Json(String),
}

impl DecodeError {
    /// The error code a server sends back for this failure.
    pub fn error_code(&self) -> crate::ErrorCode {
        match self {
            DecodeError::FrameTooLarge { .. } | DecodeError::FieldTooLong { .. } => crate::ErrorCode::TooLarge,
            DecodeError::UnsupportedVersion { .. } => crate::ErrorCode::UnsupportedVersion,
            DecodeError::Empty
            | DecodeError::Truncated(_)
            | DecodeError::TrailingBytes(_)
            | DecodeError::UnknownTag(_)
            | DecodeError::UnknownValue { .. }
            | DecodeError::InvalidUtf8(_)
            | DecodeError::BadVarint(_)
            | DecodeError::Invalid(_)
            | DecodeError::Json(_) => crate::ErrorCode::Malformed,
        }
    }
}
