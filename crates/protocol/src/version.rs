//! Version negotiation: which protocol version two sides speak, and which decoder reads a `Join`.
//!
//! The first five bytes of a `Join` (tag, `min_version`, `max_version`) have the same layout in every protocol
//! version. A host reads them with [`JoinVersions::peek`], negotiates, and hands the frame to the decoder of the
//! negotiated version: [`ClientMessage::decode`](crate::ClientMessage::decode) for 1,
//! [`v2::ClientMessage::decode`](crate::v2::ClientMessage::decode) for 2. A client lays out the rest of its `Join`
//! for one version, so a client sends `min_version == max_version` (the kind of room it joins decides).

use std::ops::RangeInclusive;

use crate::error::DecodeError;
use crate::limits::{MIN_PROTOCOL_VERSION, PROTOCOL_VERSION};

/// The tag of a `Join` in every protocol version.
pub(crate) const JOIN_TAG: u8 = 0x01;

/// Picks the protocol version both sides speak: the highest in both ranges, or `None`. This build speaks
/// [`MIN_PROTOCOL_VERSION`] to [`PROTOCOL_VERSION`].
pub fn negotiate_version(client_min: u16, client_max: u16) -> Option<u16> {
    negotiate_version_within(client_min, client_max, MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION)
}

/// Like [`negotiate_version`], for a host that speaks only the versions in `ours` (a relay that hosts sealed rooms
/// only, or a server that has not enabled them).
pub fn negotiate_version_within(client_min: u16, client_max: u16, ours: RangeInclusive<u16>) -> Option<u16> {
    if client_min > client_max {
        return None;
    }
    let high = client_max.min(*ours.end());
    let low = client_min.max(*ours.start());
    (low <= high).then_some(high)
}

/// The version range in the first five bytes of a `Join`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinVersions {
    /// Lowest protocol version the client speaks.
    pub min: u16,
    /// Highest protocol version the client speaks.
    pub max: u16,
}

impl JoinVersions {
    /// Reads the version range of a first frame without reading the rest, which a newer client may lay out
    /// differently. A frame that is not a `Join` is [`DecodeError::UnknownTag`] (the host answers `NotJoined`).
    pub fn peek(frame: &[u8]) -> Result<Self, DecodeError> {
        let too_short = DecodeError::Truncated("min_version");
        let (&tag, rest) = frame.split_first().ok_or(DecodeError::Empty)?;
        if tag != JOIN_TAG {
            return Err(DecodeError::UnknownTag(tag));
        }
        let (min, rest) = rest.split_first_chunk::<2>().ok_or(too_short)?;
        let (max, _) = rest.split_first_chunk::<2>().ok_or(DecodeError::Truncated("max_version"))?;
        Ok(Self { min: u16::from_le_bytes(*min), max: u16::from_le_bytes(*max) })
    }

    /// The version this build negotiates with this client.
    pub fn negotiate(self) -> Option<u16> {
        negotiate_version(self.min, self.max)
    }

    /// Checks that the decoder of version `codec` may read a `Join` with this range: the ranges overlap, and the
    /// client speaks `codec`.
    pub(crate) fn check_codec(self, codec: u16) -> Result<(), DecodeError> {
        if self.negotiate().is_none() {
            return Err(DecodeError::UnsupportedVersion {
                client_min: self.min,
                client_max: self.max,
                ours_min: MIN_PROTOCOL_VERSION,
                ours_max: PROTOCOL_VERSION,
            });
        }
        if (self.min..=self.max).contains(&codec) {
            Ok(())
        } else {
            Err(DecodeError::WrongLayout { client_min: self.min, client_max: self.max, codec })
        }
    }
}
