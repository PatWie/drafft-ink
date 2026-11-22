//! Named versions: a name, a time and a compressed copy of the scene at that moment.
//!
//! A version is a snapshot, not a pointer into the CRDT history, so it survives everything that rewrites history: a
//! saved file (which holds the scene and no history), the workspace autosave, trimming and compaction of a room, and
//! export to a format that has no versions. The design and its measurements are in `docs/design/versions.md`.
//!
//! The scene is the JSON of its elements (tombstones included, so that a restore can bring a deleted element back
//! exactly) and the background, deflated, behind one byte that names the codec. Pictures are not part of it: an image
//! element names its blob by [`FileId`], and the blob stays in the document's `files`, once for all versions.
//! [`NamedVersion::files`] lists the blobs a version needs, so nothing that cleans up unused blobs may drop them.

use std::io::Read;
use std::sync::Arc;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::document::b64;
use crate::element::{Element, ElementKind};
use crate::id::{FileId, VersionId};
use crate::limits::{
    MAX_ELEMENTS, MAX_VERSION_AUTHOR_CHARS, MAX_VERSION_INFLATED_BYTES, MAX_VERSION_NAME_CHARS, MAX_VERSION_STATE_BYTES,
};
use crate::store::SceneRead;
use crate::validate::{ElementError, validate_element};

/// The codec byte of a stored scene: zlib-compressed JSON.
const CODEC_ZLIB_JSON: u8 = 1;

/// Why a version could not be made, read or accepted.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum VersionError {
    /// The name is empty after trimming, too long, or has a control character.
    #[error(
        "a version name has 1 to {MAX_VERSION_NAME_CHARS} characters and no control characters such as line breaks"
    )]
    BadName,
    /// The author is too long or has a control character.
    #[error("a version author has at most {MAX_VERSION_AUTHOR_CHARS} characters and no control characters")]
    BadAuthor,
    /// The stored scene is over [`MAX_VERSION_STATE_BYTES`].
    #[error("the scene is {len} bytes once compressed, and a version holds at most {max}")]
    StateTooLarge {
        /// Size of the compressed scene.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// The scene could not be written.
    #[error("the scene could not be written as a version: {0}")]
    Encode(String),
    /// The stored scene is not something this build can read.
    #[error("the stored scene of this version cannot be read: {0}")]
    Corrupt(String),
    /// An element of the stored scene breaks a rule.
    #[error("the stored scene of this version holds an invalid element {id}: {error}")]
    InvalidElement {
        /// The element.
        id: crate::id::ElementId,
        /// The broken rule.
        error: ElementError,
    },
}

/// A name in normal form: trimmed, 1 to [`MAX_VERSION_NAME_CHARS`] characters, no control characters.
///
/// # Errors
/// [`VersionError::BadName`] when nothing is left or the name breaks the rule.
pub fn normalize_version_name(raw: &str) -> Result<String, VersionError> {
    let t = raw.trim();
    if t.is_empty() || t.chars().count() > MAX_VERSION_NAME_CHARS || t.chars().any(char::is_control) {
        return Err(VersionError::BadName);
    }
    Ok(t.to_owned())
}

fn normalize_author(raw: &str) -> Result<Option<String>, VersionError> {
    let t = raw.trim();
    if t.chars().count() > MAX_VERSION_AUTHOR_CHARS || t.chars().any(char::is_control) {
        return Err(VersionError::BadAuthor);
    }
    Ok((!t.is_empty()).then(|| t.to_owned()))
}

/// A named version of a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamedVersion {
    /// Identity. Random, so two peers that save at once both keep theirs.
    pub id: VersionId,
    /// The name the user gave it. Not unique: two peers can pick the same one.
    pub name: String,
    /// When it was saved, in milliseconds since the Unix epoch (the saving peer's clock).
    pub created_ms: u64,
    /// The name of the person who saved it, when they set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// How many live elements the scene had.
    pub elements: u32,
    /// The picture blobs the scene refers to, sorted. The blobs are in the document's `files`, not here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileId>,
    /// The scene, compressed (base64 in JSON). Read it with [`NamedVersion::open`].
    #[serde(with = "b64")]
    #[schemars(with = "String")]
    pub state: Arc<[u8]>,
}

/// What a version holds once opened.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionScene {
    /// The canvas background.
    pub background: Color,
    /// Every element, tombstones included, back to front. Each one passed `validate_element`, and its `version`
    /// counter is 0.
    pub elements: Vec<Element>,
}

#[derive(Serialize)]
struct WireOut<'a> {
    background: Color,
    elements: Vec<&'a Element>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireIn {
    background: Color,
    elements: Vec<Element>,
}

impl NamedVersion {
    /// Captures the scene of `scene` as a version.
    ///
    /// # Errors
    /// A bad name or author, a scene that is too large for one version, or a scene that cannot be written.
    pub fn capture<R: SceneRead>(
        id: VersionId,
        name: &str,
        created_ms: u64,
        author: Option<&str>,
        scene: &R,
    ) -> Result<Self, VersionError> {
        let name = normalize_version_name(name)?;
        let author = author.map(normalize_author).transpose()?.flatten();
        let elements: Vec<&Element> = scene.iter_all().collect();
        let mut files: Vec<FileId> = elements
            .iter()
            .filter_map(|e| match &e.kind {
                ElementKind::Image(i) => i.file_id.clone(),
                _ => None,
            })
            .collect();
        files.sort();
        files.dedup();
        let live = elements.iter().filter(|e| !e.deleted).count();
        let wire = WireOut { background: scene.meta().background, elements };
        let mut enc = ZlibEncoder::new(vec![CODEC_ZLIB_JSON], Compression::default());
        serde_json::to_writer(&mut enc, &wire).map_err(|e| VersionError::Encode(e.to_string()))?;
        let state = enc.finish().map_err(|e| VersionError::Encode(e.to_string()))?;
        if state.len() > MAX_VERSION_STATE_BYTES {
            return Err(VersionError::StateTooLarge { len: state.len(), max: MAX_VERSION_STATE_BYTES });
        }
        Ok(Self {
            id,
            name,
            created_ms,
            author,
            elements: u32::try_from(live).unwrap_or(u32::MAX),
            files,
            state: Arc::from(state),
        })
    }

    /// Checks everything except the stored scene: the name, the author and the size of the scene. A version that
    /// came from a file or a peer passes through here before a store takes it.
    ///
    /// # Errors
    /// The rule the version breaks.
    pub fn check(&self) -> Result<(), VersionError> {
        if normalize_version_name(&self.name)? != self.name {
            return Err(VersionError::BadName);
        }
        if let Some(a) = &self.author
            && normalize_author(a)?.as_deref() != Some(a.as_str())
        {
            return Err(VersionError::BadAuthor);
        }
        if self.state.len() > MAX_VERSION_STATE_BYTES {
            return Err(VersionError::StateTooLarge { len: self.state.len(), max: MAX_VERSION_STATE_BYTES });
        }
        Ok(())
    }

    /// Reads the stored scene. Every input is hostile: the inflated size, the number of elements and each element are
    /// checked.
    ///
    /// # Errors
    /// [`VersionError::Corrupt`] for data that is not a scene, and [`VersionError::InvalidElement`] for an element
    /// that breaks a rule.
    pub fn open(&self) -> Result<VersionScene, VersionError> {
        self.open_within(MAX_VERSION_INFLATED_BYTES)
    }

    /// [`NamedVersion::open`] with the inflated size limit as a parameter, so that a test of the limit does not have
    /// to inflate a quarter of a gigabyte.
    fn open_within(&self, max_inflated: usize) -> Result<VersionScene, VersionError> {
        let corrupt = |why: &str| VersionError::Corrupt(why.to_owned());
        let (codec, body) = self.state.split_first().ok_or_else(|| corrupt("it is empty"))?;
        if *codec != CODEC_ZLIB_JSON {
            return Err(VersionError::Corrupt(format!("it uses codec {codec}, which this build does not know")));
        }
        let mut json = Vec::new();
        let limit = u64::try_from(max_inflated).unwrap_or(u64::MAX);
        ZlibDecoder::new(body)
            .take(limit.saturating_add(1))
            .read_to_end(&mut json)
            .map_err(|e| VersionError::Corrupt(e.to_string()))?;
        if json.len() > max_inflated {
            return Err(corrupt("it expands to more than the limit"));
        }
        let wire: WireIn = serde_json::from_slice(&json).map_err(|e| VersionError::Corrupt(e.to_string()))?;
        if wire.elements.len() > MAX_ELEMENTS {
            return Err(corrupt("it holds more elements than a document may"));
        }
        let mut elements = wire.elements;
        for e in &mut elements {
            validate_element(e).map_err(|error| VersionError::InvalidElement { id: e.id.clone(), error })?;
            e.version = 0;
        }
        Ok(VersionScene { background: wire.background, elements })
    }

    /// The size of the stored scene in bytes.
    pub fn stored_bytes(&self) -> usize {
        self.state.len()
    }

    /// The same version under another name.
    ///
    /// # Errors
    /// [`VersionError::BadName`].
    pub fn renamed(&self, name: &str) -> Result<Self, VersionError> {
        Ok(Self { name: normalize_version_name(name)?, ..self.clone() })
    }
}

/// The versions of a document in listing order: oldest first, ties broken by id.
pub fn sort_versions(versions: &mut [NamedVersion]) {
    versions.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::Transform;
    use crate::id::{IdSource, SeqIds};
    use crate::memory::MemoryStore;
    use crate::store::{SceneStore, SceneTx};
    use crate::zindex::ZIndex;

    fn store_with_rect() -> MemoryStore {
        let mut s = MemoryStore::new();
        let mut ids = SeqIds::new();
        let mut e = Element::new(ids.element_id(), ElementKind::Rectangle, ZIndex::first());
        e.transform = Transform::new(1.0, 2.0, 30.0, 40.0);
        let mut tx = s.begin();
        tx.insert(e).unwrap();
        tx.commit().unwrap();
        s
    }

    #[test]
    fn a_version_round_trips_its_scene() {
        let s = store_with_rect();
        let v = NamedVersion::capture(SeqIds::new().version_id(), "  First  ", 7, Some(" Ana "), &s).unwrap();
        assert_eq!((v.name.as_str(), v.created_ms, v.author.as_deref(), v.elements), ("First", 7, Some("Ana"), 1));
        let scene = v.open().unwrap();
        assert_eq!(scene.elements.len(), 1);
        assert_eq!(scene.elements[0].transform, Transform::new(1.0, 2.0, 30.0, 40.0));
        assert_eq!(scene.elements[0].version, 0, "the counter of the store is not part of the content");
    }

    #[test]
    fn names_and_authors_are_checked() {
        let s = store_with_rect();
        let id = || SeqIds::new().version_id();
        assert_eq!(NamedVersion::capture(id(), "   ", 0, None, &s).unwrap_err(), VersionError::BadName);
        assert_eq!(NamedVersion::capture(id(), "a\nb", 0, None, &s).unwrap_err(), VersionError::BadName);
        let long = "x".repeat(MAX_VERSION_NAME_CHARS + 1);
        assert_eq!(NamedVersion::capture(id(), &long, 0, None, &s).unwrap_err(), VersionError::BadName);
        assert!(NamedVersion::capture(id(), &"x".repeat(MAX_VERSION_NAME_CHARS), 0, None, &s).is_ok());
        assert_eq!(NamedVersion::capture(id(), "ok", 0, Some("a\tb"), &s).unwrap_err(), VersionError::BadAuthor);
        assert_eq!(NamedVersion::capture(id(), "ok", 0, Some("  "), &s).unwrap().author, None);
    }

    #[test]
    fn hostile_state_is_refused_without_a_panic() {
        let s = store_with_rect();
        let v = NamedVersion::capture(SeqIds::new().version_id(), "v", 0, None, &s).unwrap();
        let with = |bytes: &[u8]| NamedVersion { state: Arc::from(bytes), ..v.clone() };
        assert!(matches!(with(&[]).open(), Err(VersionError::Corrupt(_))));
        assert!(matches!(with(&[9, 1, 2]).open(), Err(VersionError::Corrupt(_))));
        assert!(matches!(with(&[1, 0xff, 0xff]).open(), Err(VersionError::Corrupt(_))));
        let mut cut = v.state.to_vec();
        cut.truncate(cut.len() / 2);
        assert!(matches!(with(&cut).open(), Err(VersionError::Corrupt(_))));
        let too_big = NamedVersion { state: Arc::from(vec![1; MAX_VERSION_STATE_BYTES + 1]), ..v.clone() };
        assert!(matches!(too_big.check(), Err(VersionError::StateTooLarge { .. })));
    }

    #[test]
    fn a_decompression_bomb_is_stopped() {
        const LIMIT: usize = 1 << 20;
        let mut enc = ZlibEncoder::new(vec![CODEC_ZLIB_JSON], Compression::best());
        std::io::copy(&mut std::io::repeat(b' ').take(u64::try_from(LIMIT).unwrap() + 10), &mut enc).unwrap();
        let state = enc.finish().unwrap();
        assert!(state.len() < 4096, "a megabyte of spaces compresses to {}", state.len());
        let v = NamedVersion {
            id: SeqIds::new().version_id(),
            name: "bomb".into(),
            created_ms: 0,
            author: None,
            elements: 0,
            files: Vec::new(),
            state: Arc::from(state),
        };
        assert!(matches!(v.open_within(LIMIT), Err(VersionError::Corrupt(why)) if why.contains("limit")));
        assert!(v.open().is_err(), "and with the real limit it is still not a scene");
    }

    #[test]
    fn an_invalid_element_inside_the_state_is_named() {
        let json = br##"{"background":"#ffffff","elements":[{"id":"bad","kind":{"type":"rectangle"},"transform":{"x":0,"y":0,"w":-5,"h":1},"index":"a0"}]}"##;
        let mut enc = ZlibEncoder::new(vec![CODEC_ZLIB_JSON], Compression::default());
        std::io::Write::write_all(&mut enc, json).unwrap();
        let v = NamedVersion {
            id: SeqIds::new().version_id(),
            name: "x".into(),
            created_ms: 0,
            author: None,
            elements: 1,
            files: Vec::new(),
            state: Arc::from(enc.finish().unwrap()),
        };
        assert!(matches!(v.open(), Err(VersionError::InvalidElement { id, .. }) if id.as_str() == "bad"));
    }
}
