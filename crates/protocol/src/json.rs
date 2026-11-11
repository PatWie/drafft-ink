//! The JSON debug form. Byte fields are hex strings. It is for logs, tests and `curl`-style
//! debugging; production traffic uses the binary form.

use crate::error::{DecodeError, EncodeError};
use crate::limits::MAX_JSON_BYTES;
use crate::message::{ClientMessage, ServerMessage};

/// Serde adapter: `Vec<u8>` as a hex string.
pub(crate) mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::ids::{hex_decode, hex_encode};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex_encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        hex_decode(&text, None).ok_or_else(|| serde::de::Error::custom("expected a hex string"))
    }
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, DecodeError> {
    if text.len() > MAX_JSON_BYTES {
        return Err(DecodeError::FrameTooLarge { len: text.len(), max: MAX_JSON_BYTES });
    }
    serde_json::from_str(text).map_err(|e| DecodeError::Json(e.to_string()))
}

impl ClientMessage {
    /// The JSON debug form.
    pub fn to_json(&self) -> Result<String, EncodeError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|e| EncodeError::Json(e.to_string()))
    }

    /// Parses the JSON debug form and validates it like a binary frame.
    pub fn from_json(text: &str) -> Result<Self, DecodeError> {
        let msg: ClientMessage = parse(text)?;
        if let ClientMessage::Join { min_version, max_version, .. } = &msg {
            crate::binary::check_version_range(*min_version, *max_version)?;
        }
        msg.validate()?;
        Ok(msg)
    }
}

impl ServerMessage {
    /// The JSON debug form.
    pub fn to_json(&self) -> Result<String, EncodeError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|e| EncodeError::Json(e.to_string()))
    }

    /// Parses the JSON debug form and validates it like a binary frame.
    pub fn from_json(text: &str) -> Result<Self, DecodeError> {
        let msg: ServerMessage = parse(text)?;
        msg.validate()?;
        Ok(msg)
    }
}
