//! Picture files in the Loro `files` root: the write policy, the stored layout and how an entry is read back.
//!
//! `files[id]` is a plain value, a map `{mime, size}` with an optional `bytes` binary (schema 2, `docs/crdt.md`
//! section 7). The id is the SHA-256 of the bytes. An *embedded* entry carries the bytes; a *manifest-only* entry
//! (written in a shared room, where the bytes travel as separate files) does not.

use std::collections::HashMap;
use std::sync::Arc;

use drafftink_scene::limits::MAX_FILE_BYTES;
use drafftink_scene::{FileEntry, FileManifest, is_image_media_type};
use loro::{LoroDoc, LoroValue, ValueOrContainer};

use crate::error::DocError;
use crate::schema::ROOT_FILES;

/// Key of the media type (string).
const MIME: &str = "mime";
/// Key of the byte length (integer).
const SIZE: &str = "size";
/// Key of the encoded picture (binary).
const BYTES: &str = "bytes";

/// Where the bytes of a picture go when this replica adds one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FilePolicy {
    /// The bytes are stored in the document next to the media type and size. The default: a local document is
    /// self-contained.
    #[default]
    Embedded,
    /// Only the media type and size are stored. The bytes stay in the view of this replica and travel by other
    /// means (the encrypted file store of a shared room).
    External,
}

/// A valid `files` entry as read from Loro.
#[derive(Debug)]
pub(crate) enum Stored {
    /// With bytes. The caller still checks that the bytes hash to the key.
    Full(FileEntry),
    /// Without bytes.
    Manifest(FileManifest),
}

/// The Loro value for `entry` under `policy`.
pub(crate) fn to_loro(entry: &FileEntry, policy: FilePolicy) -> LoroValue {
    let manifest = FileManifest::of(entry);
    let mut map = HashMap::from([
        (MIME.to_owned(), LoroValue::from(manifest.mime.as_str())),
        (SIZE.to_owned(), LoroValue::from(i64::try_from(manifest.size).unwrap_or(i64::MAX))),
    ]);
    if policy == FilePolicy::Embedded {
        map.insert(BYTES.to_owned(), LoroValue::from(entry.bytes.to_vec()));
    }
    LoroValue::from(map)
}

/// Reads one `files` value. `None` for anything that is not a valid entry: a wrong type, a media type that is not
/// `image/*`, a size over the limit or not matching the bytes, a manifest without a size.
///
/// An entry with bytes and no size is accepted: a peer that still runs schema 1 can write one before it has seen
/// the document move to schema 2, and the migration would derive the size from the bytes anyway.
pub(crate) fn from_loro(v: &LoroValue) -> Option<Stored> {
    let LoroValue::Map(m) = v else { return None };
    let mime = match m.get(MIME)? {
        LoroValue::String(s) if is_image_media_type(s) => s.to_string(),
        _ => return None,
    };
    let size = match m.get(SIZE) {
        None => None,
        Some(LoroValue::I64(n)) => Some(u64::try_from(*n).ok()?),
        Some(_) => return None,
    };
    match m.get(BYTES) {
        Some(LoroValue::Binary(b)) => {
            if b.len() > MAX_FILE_BYTES || size.is_some_and(|s| s != u64::try_from(b.len()).unwrap_or(u64::MAX)) {
                return None;
            }
            Some(Stored::Full(FileEntry { mime, bytes: Arc::from(b.to_vec()) }))
        }
        None => {
            let size = size?;
            (size <= MAX_FILE_BYTES as u64).then_some(Stored::Manifest(FileManifest { mime, size }))
        }
        Some(_) => None,
    }
}

/// The migration step from schema 1: writes `size` into every entry that has bytes and no size.
pub(crate) fn add_sizes(doc: &LoroDoc) -> Result<(), DocError> {
    let files = doc.get_map(ROOT_FILES);
    let mut missing = Vec::new();
    files.for_each(|key, value| {
        if let ValueOrContainer::Value(LoroValue::Map(m)) = value
            && !m.contains_key(SIZE)
            && let Some(LoroValue::Binary(b)) = m.get(BYTES)
        {
            let mut with_size: HashMap<String, LoroValue> = m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            with_size.insert(SIZE.to_owned(), LoroValue::from(i64::try_from(b.len()).unwrap_or(i64::MAX)));
            missing.push((key.to_owned(), LoroValue::from(with_size)));
        }
    });
    for (key, value) in missing {
        files.insert(&key, value)?;
    }
    Ok(())
}
