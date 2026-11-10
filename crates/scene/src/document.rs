//! The document: the serialisable whole (elements in z-order, name, background, image blobs, schema version).
//!
//! `Document` is the file and exchange form. Live editing goes through a [`crate::store::SceneStore`], which can
//! import and export a `Document` with [`crate::store::load_document`] and [`crate::store::snapshot`].

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::color::Color;
use crate::element::Element;
use crate::id::FileId;
use crate::validate::{Report, validate_document};

/// The version of the native format this build writes and reads.
pub const SCHEMA_VERSION: u32 = 1;
/// Value of the `format` field of every native file.
pub const FORMAT_NAME: &str = "drafftink";

/// The constant `"drafftink"` in the `format` field, which makes a foreign JSON file fail early and clearly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FormatTag;

impl Serialize for FormatTag {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(FORMAT_NAME)
    }
}

impl<'de> Deserialize<'de> for FormatTag {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        if s == FORMAT_NAME {
            Ok(FormatTag)
        } else {
            Err(serde::de::Error::custom(format!("not a drafft.ink document: format is {s:?}, expected \"drafftink\"")))
        }
    }
}

impl JsonSchema for FormatTag {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FormatTag".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "const": FORMAT_NAME })
    }
}

/// An image blob. Stored once per content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    /// Media type, for example `image/png`.
    pub mime: String,
    /// The encoded picture, base64 (standard alphabet, padded) in JSON.
    #[serde(with = "b64")]
    #[schemars(with = "String")]
    pub bytes: Arc<[u8]>,
}

impl FileEntry {
    /// Makes an entry and its id from encoded bytes.
    pub fn new(mime: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> (FileId, FileEntry) {
        let bytes: Arc<[u8]> = bytes.into();
        (FileId::of(&bytes), FileEntry { mime: mime.into(), bytes })
    }
}

mod b64 {
    use std::sync::Arc;

    use super::*;

    pub fn serialize<S: Serializer>(v: &Arc<[u8]>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<[u8]>, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        let bytes = STANDARD.decode(s.as_bytes()).map_err(serde::de::Error::custom)?;
        Ok(Arc::from(bytes))
    }
}

/// The document-level properties a store keeps next to its elements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocMeta {
    /// Document name.
    pub name: String,
    /// Canvas background colour.
    pub background: Color,
}

impl Default for DocMeta {
    fn default() -> Self {
        Self { name: default_name(), background: default_background() }
    }
}

fn default_name() -> String {
    "Untitled".to_owned()
}

fn default_background() -> Color {
    Color::WHITE
}

fn current_version() -> u32 {
    SCHEMA_VERSION
}

/// A whole document in the native format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Always `"drafftink"`.
    #[serde(default)]
    pub format: FormatTag,
    /// Schema version of this file. A reader refuses a version newer than it knows.
    #[serde(rename = "version", default = "current_version")]
    pub schema_version: u32,
    /// Document name.
    #[serde(default = "default_name")]
    pub name: String,
    /// Canvas background.
    #[serde(default = "default_background")]
    pub background: Color,
    /// All elements, tombstones included, from back to front. Keys in `index` are strictly increasing.
    #[serde(default)]
    pub elements: Vec<Element>,
    /// Image blobs by content hash.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<FileId, FileEntry>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            format: FormatTag,
            schema_version: SCHEMA_VERSION,
            name: default_name(),
            background: default_background(),
            elements: Vec::new(),
            files: BTreeMap::new(),
        }
    }
}

/// Why a document could not be read or written.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DocumentError {
    /// The text is not JSON of the expected shape.
    #[error("the file is not a drafft.ink document: {0}")]
    Parse(String),
    /// The file was saved by a newer version of the format.
    #[error("this file uses format version {found}, and this build reads up to {max}. Update the app to open it")]
    NewerVersion {
        /// Version in the file.
        found: u32,
        /// Newest version this build reads.
        max: u32,
    },
    /// The content breaks a rule. The report lists every problem.
    #[error("the document breaks {} rule(s); the first is: {}", .0.errors().count(), first_error(.0))]
    Invalid(Report),
    /// The JSON writer failed.
    #[error("could not write the document: {0}")]
    Write(String),
}

fn first_error(r: &Report) -> String {
    r.errors().next().map(|i| i.to_string()).unwrap_or_default()
}

impl Document {
    /// An empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Name and background.
    pub fn meta(&self) -> DocMeta {
        DocMeta { name: self.name.clone(), background: self.background }
    }

    /// Checks every rule. See [`crate::validate`].
    pub fn validate(&self) -> Report {
        validate_document(self)
    }

    /// Reads a document. Fails on malformed JSON, a newer schema version, or any rule violation of error severity.
    /// Never panics, whatever the text.
    pub fn from_json(text: &str) -> Result<Document, DocumentError> {
        // Check the version first so that a newer file reports that, not a parse error on a field we do not know.
        #[derive(Deserialize)]
        struct Probe {
            version: Option<u32>,
        }
        if let Ok(p) = serde_json::from_str::<Probe>(text)
            && let Some(found) = p.version.filter(|v| *v > SCHEMA_VERSION)
        {
            return Err(DocumentError::NewerVersion { found, max: SCHEMA_VERSION });
        }
        // serde accepts a bare array for a struct; a document is always an object.
        if !text.trim_start().starts_with('{') {
            return Err(DocumentError::Parse("a document is a JSON object".to_owned()));
        }
        let doc: Document = serde_json::from_str(text).map_err(|e| DocumentError::Parse(e.to_string()))?;
        let report = doc.validate();
        if report.has_errors() {
            return Err(DocumentError::Invalid(report));
        }
        Ok(doc)
    }

    /// Writes compact JSON. Refuses a document that breaks a rule, so that a file we write can always be read back
    /// (the old app wrote `NaN` as `null` and could not reload its own file).
    pub fn to_json(&self) -> Result<String, DocumentError> {
        self.check_writable()?;
        serde_json::to_string(self).map_err(|e| DocumentError::Write(e.to_string()))
    }

    /// Like [`Document::to_json`], indented for humans.
    pub fn to_json_pretty(&self) -> Result<String, DocumentError> {
        self.check_writable()?;
        serde_json::to_string_pretty(self).map_err(|e| DocumentError::Write(e.to_string()))
    }

    fn check_writable(&self) -> Result<(), DocumentError> {
        let report = self.validate();
        if report.has_errors() { Err(DocumentError::Invalid(report)) } else { Ok(()) }
    }

    /// Ids of image blobs that no live or deleted element refers to.
    pub fn unused_files(&self) -> Vec<FileId> {
        let used: std::collections::HashSet<&FileId> = self
            .elements
            .iter()
            .filter_map(|e| match &e.kind {
                crate::element::ElementKind::Image(i) => i.file_id.as_ref(),
                _ => None,
            })
            .collect();
        self.files.keys().filter(|k| !used.contains(k)).cloned().collect()
    }
}

/// The JSON Schema of the native format, for agents and external tools.
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Document)).unwrap_or(serde_json::Value::Null)
}
