//! Reading and checking what the relay's picture routes carry.

use std::str::FromStr as _;
use std::sync::Arc;

use drafftink_protocol::v2::ImageName;
use drafftink_scene::FileId;
use drafftink_seal::{FILE_ID_BYTES, RoomKeys, SealedRecord};
use serde_json::Value;

/// The most entries of a list that are read. A room holds at most 200 pictures; a relay that sends more is ignored.
const MAX_LIST_ENTRIES: usize = 4096;

/// The 32 raw bytes of a file id (it is their hex text).
pub(super) fn raw_file_id(id: &FileId) -> Option<[u8; FILE_ID_BYTES]> {
    let text = id.as_str().as_bytes();
    let mut raw = [0u8; FILE_ID_BYTES];
    if text.len() != FILE_ID_BYTES * 2 {
        return None;
    }
    for (out, pair) in raw.iter_mut().zip(text.chunks_exact(2)) {
        let hi = char::from(pair[0]).to_digit(16)?;
        let lo = char::from(pair[1]).to_digit(16)?;
        *out = u8::try_from(hi * 16 + lo).ok()?;
    }
    Some(raw)
}

/// Opens a downloaded sealed picture and checks that its bytes are the picture that `id` names. `None`: damaged. A
/// relay or another member can store anything under a name, so the hash decides.
pub(super) fn open_picture(keys: &RoomKeys, name: &ImageName, id: &FileId, body: Vec<u8>) -> Option<Arc<[u8]>> {
    let record = SealedRecord::from_bytes(body).ok()?;
    let bytes = keys.open_image(name, &record).ok()?;
    (FileId::of(&bytes) == *id).then(|| Arc::from(bytes))
}

/// What a room holds, as the relay counts it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct RoomTotals {
    pub(super) images: u32,
    pub(super) bytes: u64,
}

/// The body of a `201`: `{"room_images":n,"room_image_bytes":n}`.
pub(super) fn parse_totals(body: &[u8]) -> Option<RoomTotals> {
    let value: Value = serde_json::from_slice(body).ok()?;
    Some(RoomTotals {
        images: u32::try_from(value.get("room_images")?.as_u64()?).ok()?,
        bytes: value.get("room_image_bytes")?.as_u64()?,
    })
}

/// The body of the list: `[{"name":"...","size":n}, ...]`. An entry that cannot be read is skipped.
pub(super) fn parse_list(body: &[u8]) -> Option<Vec<(ImageName, u64)>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let entries = value.as_array()?;
    Some(
        entries
            .iter()
            .take(MAX_LIST_ENTRIES)
            .filter_map(|e| Some((ImageName::from_str(e.get("name")?.as_str()?).ok()?, e.get("size")?.as_u64()?)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use drafftink_seal::RoomKey;

    fn keys() -> RoomKeys {
        RoomKeys::new(&RoomKey::from_bytes([3; 32]))
    }

    #[test]
    fn a_file_id_is_its_hex_text() {
        let id = FileId::of(b"abc");
        let raw = raw_file_id(&id).unwrap();
        assert_eq!(raw[0], 0xba);
        assert_eq!(raw[31], 0xad);
    }

    #[test]
    fn a_picture_opens_under_its_name_only_and_must_hash_to_its_id() {
        let keys = keys();
        let bytes = b"picture".to_vec();
        let id = FileId::of(&bytes);
        let name = keys.image_name(&raw_file_id(&id).unwrap());
        let sealed = keys.seal_image(&name, &bytes).unwrap().into_bytes();
        assert_eq!(open_picture(&keys, &name, &id, sealed.clone()).as_deref(), Some(bytes.as_slice()));
        let other = keys.image_name(&[9; 32]);
        assert!(open_picture(&keys, &other, &id, sealed.clone()).is_none(), "the name is in the associated data");
        assert!(open_picture(&keys, &name, &FileId::of(b"other"), sealed.clone()).is_none(), "wrong hash");
        let mut flipped = sealed;
        flipped[60] ^= 1;
        assert!(open_picture(&keys, &name, &id, flipped).is_none());
        assert!(open_picture(&keys, &name, &id, Vec::new()).is_none());
    }

    #[test]
    fn totals_and_lists_are_read_defensively() {
        assert_eq!(
            parse_totals(br#"{"room_images":3,"room_image_bytes":1200}"#),
            Some(RoomTotals { images: 3, bytes: 1200 })
        );
        assert_eq!(parse_totals(b"{}"), None);
        assert_eq!(parse_totals(b"nope"), None);
        let name = ImageName::from_bytes([5; 16]);
        let body = format!(r#"[{{"name":"{name}","size":9}},{{"name":"bad!","size":1}},{{"size":2}}]"#);
        assert_eq!(parse_list(body.as_bytes()), Some(vec![(name, 9)]));
        assert_eq!(parse_list(b"{}"), None);
    }
}
