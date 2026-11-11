//! Bytes in the arguments of a command: `{base64, name?, mime?}` or `{path}`. Shared by `image.insert`,
//! `clipboard.paste` and `doc.drop`; each maps a [`BytesError`] to the refusal of its own words.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ports::FileRef;
use crate::session::Session;

/// Bytes given inline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InlineBytes {
    /// The bytes as base64 (standard alphabet, padding optional). A `data:...;base64,` prefix is accepted.
    pub base64: String,
    /// The file name, used in messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The media type the sender claims. It is not trusted: the content decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

impl InlineBytes {
    /// The size the decoded bytes will have, to refuse a payload before decoding it. Never smaller than the truth.
    pub(crate) fn decoded_size_bound(&self) -> usize {
        self.payload().len() / 4 * 3 + 3
    }

    /// The base64 text without a `data:` prefix.
    fn payload(&self) -> &str {
        let text = self.base64.trim();
        match text.find("base64,") {
            Some(i) if text.starts_with("data:") => text.get(i + 7..).unwrap_or(""),
            _ => text,
        }
    }

    /// Decodes the text. White space inside is ignored and the padding is optional.
    pub(crate) fn decode(&self) -> Result<Vec<u8>, base64::DecodeError> {
        let clean: String = self.payload().chars().filter(|c| !c.is_whitespace()).collect();
        STANDARD.decode(clean.as_bytes()).or_else(|_| STANDARD_NO_PAD.decode(clean.as_bytes()))
    }
}

/// Bytes read from a file through the `Files` port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathBytes {
    /// A path the session's file access can read.
    pub path: String,
}

/// Bytes: inline as base64, or a path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Bytes {
    /// `{base64, name?, mime?}`.
    Inline(InlineBytes),
    /// `{path}`.
    Path(PathBytes),
}

/// Bytes read and named.
pub(crate) struct Loaded {
    pub bytes: Vec<u8>,
    pub name: String,
}

/// Why [`Bytes::load`] failed.
#[derive(Debug)]
pub(crate) enum BytesError {
    /// The base64 text is damaged.
    Base64 { name: String, source: base64::DecodeError },
    /// More bytes than the caller allows.
    TooLarge { size: usize },
    /// The session has no file access for a `{path}`.
    NoFiles,
    /// The file could not be read.
    Read { path: String, message: String },
}

impl Bytes {
    /// The bytes. `fallback` names bytes that carry no name; `max` is the most bytes accepted, checked before
    /// an inline payload is decoded.
    pub(crate) fn load(&self, s: &Session, fallback: &str, max: usize) -> Result<Loaded, BytesError> {
        match self {
            Bytes::Inline(b) => {
                let name = b.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| fallback.to_owned());
                let bound = b.decoded_size_bound();
                if bound > max.saturating_add(3) {
                    return Err(BytesError::TooLarge { size: bound - 3 });
                }
                let bytes = b.decode().map_err(|source| BytesError::Base64 { name: name.clone(), source })?;
                Ok(Loaded { bytes, name })
            }
            Bytes::Path(p) => {
                let name = p.path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or(fallback).to_owned();
                let files = s.ports.files.as_ref().ok_or(BytesError::NoFiles)?;
                let bytes = files
                    .read(&FileRef::Path { path: p.path.clone() })
                    .map_err(|e| BytesError::Read { path: p.path.clone(), message: e.to_string() })?;
                if bytes.len() > max {
                    return Err(BytesError::TooLarge { size: bytes.len() });
                }
                Ok(Loaded { bytes, name })
            }
        }
    }
}
