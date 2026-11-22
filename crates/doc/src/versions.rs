//! Named versions in the Loro document (`docs/crdt.md` section 7a).
//!
//! The root map `versions` holds one child map per version, keyed by the version id. The child is a mergeable
//! container, so a rename (one key) and a delete (the key of the root) are separate operations that merge, and two
//! peers that save at the same moment add two children and keep both. The stored scene is one binary value that is
//! written once and never changed, so a rename adds a few bytes to the history and not a copy of the scene.
//!
//! | Key | Loro type | Meaning |
//! |---|---|---|
//! | `name` | string | the name |
//! | `at` | integer | when it was saved, milliseconds since the Unix epoch |
//! | `author` | string | who saved it; absent when unknown |
//! | `elements` | integer | live elements in the scene |
//! | `files` | string | the picture blobs it needs, ids joined by commas; absent when none |
//! | `state` | binary | the compressed scene (`drafftink_scene::NamedVersion::state`) |

use drafftink_scene::{FileId, NamedVersion, VersionId};
use loro::{LoroMap, LoroValue, ValueOrContainer};

use crate::error::DocError;

const KEY_NAME: &str = "name";
const KEY_AT: &str = "at";
const KEY_AUTHOR: &str = "author";
const KEY_ELEMENTS: &str = "elements";
const KEY_FILES: &str = "files";
const KEY_STATE: &str = "state";

fn value(map: &LoroMap, key: &str) -> Option<LoroValue> {
    match map.get(key)? {
        ValueOrContainer::Value(v) => Some(v),
        ValueOrContainer::Container(_) => None,
    }
}

fn text(map: &LoroMap, key: &str) -> Option<String> {
    match value(map, key)? {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn number(map: &LoroMap, key: &str) -> Option<u64> {
    match value(map, key)? {
        LoroValue::I64(n) => u64::try_from(n).ok(),
        _ => None,
    }
}

/// Reads one version. `None` when the entry is not a valid version: a missing or mistyped key, a name or a size that
/// breaks the rules. Such an entry stays in the CRDT and is hidden from the view, like an invalid element.
pub(crate) fn read(id: &VersionId, map: &LoroMap) -> Option<NamedVersion> {
    let files = match text(map, KEY_FILES) {
        Some(list) => list.split(',').map(FileId::new).collect::<Result<Vec<_>, _>>().ok()?,
        None => Vec::new(),
    };
    let state = match value(map, KEY_STATE)? {
        LoroValue::Binary(b) => b.to_vec(),
        _ => return None,
    };
    let version = NamedVersion {
        id: id.clone(),
        name: text(map, KEY_NAME)?,
        created_ms: number(map, KEY_AT)?,
        author: text(map, KEY_AUTHOR),
        elements: u32::try_from(number(map, KEY_ELEMENTS)?).ok()?,
        files,
        state: state.into(),
    };
    version.check().ok()?;
    Some(version)
}

/// Writes the keys of `version` that differ from what the document holds, so that putting back what is already there
/// writes nothing and a rename writes only the name. Returns the number of keys written.
pub(crate) fn write(root: &LoroMap, version: &NamedVersion) -> Result<usize, DocError> {
    let map = match root.ensure_mergeable_map(version.id.as_str()) {
        Ok(m) => m,
        Err(_) => {
            // The key holds something that is not a container (a foreign write): replace it.
            root.delete(version.id.as_str())?;
            root.ensure_mergeable_map(version.id.as_str())?
        }
    };
    let files =
        (!version.files.is_empty()).then(|| version.files.iter().map(FileId::as_str).collect::<Vec<_>>().join(","));
    let mut written = 0;
    let mut put = |key: &str, new: Option<LoroValue>| -> Result<(), DocError> {
        match (value(&map, key), new) {
            (Some(cur), Some(new)) if cur == new => {}
            (_, Some(new)) => {
                map.insert(key, new)?;
                written += 1;
            }
            (Some(_), None) => {
                map.delete(key)?;
                written += 1;
            }
            (None, None) => {}
        }
        Ok(())
    };
    put(KEY_NAME, Some(LoroValue::from(version.name.as_str())))?;
    put(KEY_AT, Some(LoroValue::from(i64::try_from(version.created_ms).unwrap_or(i64::MAX))))?;
    put(KEY_AUTHOR, version.author.as_deref().map(LoroValue::from))?;
    put(KEY_ELEMENTS, Some(LoroValue::from(i64::from(version.elements))))?;
    put(KEY_FILES, files.as_deref().map(LoroValue::from))?;
    put(KEY_STATE, Some(LoroValue::from(version.state.to_vec())))?;
    Ok(written)
}

/// Removes a version.
pub(crate) fn remove(root: &LoroMap, id: &VersionId) -> Result<(), DocError> {
    root.delete(id.as_str())?;
    Ok(())
}
