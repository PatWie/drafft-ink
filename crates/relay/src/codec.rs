//! The byte layout that the durable stores of a relay use for the room meta, a segment of records and a snapshot.
//! Plain fixed-width little-endian fields; every reader is bounds-checked, because a file on disk is input too.
//! The tokio server (files and SQLite) and the Durable Object host share these functions, so a room has one format.

use crate::store::{RoomMeta, StoreError, StoredSnapshot};
use drafftink_protocol::ClientId;
use drafftink_protocol::v2::StoredRecord;

/// The format version of a meta file.
const META_VERSION: u8 = 1;
const META_BYTES: usize = 1 + 8 + 8 + 1 + 8 + 4 + 8 + 8;
/// The fixed part of a record: sequence number, sender, counter and the length of the sealed bytes.
const RECORD_HEAD: usize = 8 + 16 + 8 + 4;

fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(what.to_owned())
}

/// A cursor over bytes that never panics.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], StoreError> {
        let (head, tail) = self.0.split_at_checked(n).ok_or_else(|| corrupt("a stored file is cut short"))?;
        self.0 = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], StoreError> {
        self.take(N)?.try_into().map_err(|_| corrupt("a stored file is cut short"))
    }

    fn u64(&mut self) -> Result<u64, StoreError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, StoreError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The bytes of a room meta, with a format version in front.
pub fn encode_meta(meta: &RoomMeta) -> Vec<u8> {
    let mut out = Vec::with_capacity(META_BYTES);
    out.push(META_VERSION);
    out.extend_from_slice(&meta.head_seq.to_le_bytes());
    out.extend_from_slice(&meta.snapshot_seq.to_le_bytes());
    out.push(u8::from(meta.has_snapshot));
    out.extend_from_slice(&meta.snapshot_bytes.to_le_bytes());
    out.extend_from_slice(&meta.log_records.to_le_bytes());
    out.extend_from_slice(&meta.log_bytes.to_le_bytes());
    out.extend_from_slice(&meta.deadline_ms.to_le_bytes());
    out
}

/// Reads a room meta written by [`encode_meta`]. A short, long or foreign-version input is
/// [`StoreError::Corrupt`].
pub fn decode_meta(bytes: &[u8]) -> Result<RoomMeta, StoreError> {
    let mut r = Reader(bytes);
    let [version] = r.array()?;
    if version != META_VERSION {
        return Err(corrupt("the room meta was written by another version"));
    }
    let meta = RoomMeta {
        head_seq: r.u64()?,
        snapshot_seq: r.u64()?,
        has_snapshot: r.array::<1>()?[0] != 0,
        snapshot_bytes: r.u64()?,
        log_records: r.u32()?,
        log_bytes: r.u64()?,
        deadline_ms: r.u64()?,
    };
    if r.is_empty() { Ok(meta) } else { Err(corrupt("the room meta has trailing bytes")) }
}

/// The bytes of a segment: its records one after the other, each with its sequence number, sender, counter and
/// sealed bytes.
pub fn encode_records(records: &[StoredRecord]) -> Vec<u8> {
    let size = records.iter().map(|r| RECORD_HEAD + r.record.len()).sum();
    let mut out = Vec::with_capacity(size);
    for record in records {
        out.extend_from_slice(&record.seq.to_le_bytes());
        out.extend_from_slice(record.from.as_bytes());
        out.extend_from_slice(&record.ctr.to_le_bytes());
        out.extend_from_slice(&u32::try_from(record.record.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(&record.record);
    }
    out
}

/// Reads the records of a segment written by [`encode_records`]. Any cut or hostile length is
/// [`StoreError::Corrupt`].
pub fn decode_records(bytes: &[u8]) -> Result<Vec<StoredRecord>, StoreError> {
    let mut r = Reader(bytes);
    let mut out = Vec::new();
    while !r.is_empty() {
        let seq = r.u64()?;
        let from = ClientId::from_bytes(r.array()?);
        let ctr = r.u64()?;
        let len = r.u32()? as usize;
        out.push(StoredRecord { seq, from, ctr, record: r.take(len)?.to_vec() });
    }
    Ok(out)
}

/// The bytes of a snapshot file or row: the sequence number it covers, then the sealed record.
pub fn encode_snapshot(covers_seq: u64, record: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + record.len());
    out.extend_from_slice(&covers_seq.to_le_bytes());
    out.extend_from_slice(record);
    out
}

/// Reads a snapshot written by [`encode_snapshot`].
pub fn decode_snapshot(bytes: &[u8]) -> Result<StoredSnapshot, StoreError> {
    let mut r = Reader(bytes);
    let covers_seq = r.u64()?;
    Ok(StoredSnapshot { covers_seq, record: r.0.to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_and_records_round_trip_and_truncations_are_errors_not_panics() {
        let meta = RoomMeta {
            head_seq: 9,
            snapshot_seq: 4,
            has_snapshot: true,
            snapshot_bytes: 77,
            log_records: 5,
            log_bytes: 1234,
            deadline_ms: 55,
        };
        let bytes = encode_meta(&meta);
        assert_eq!(decode_meta(&bytes).unwrap(), meta);
        for len in 0..bytes.len() {
            assert!(decode_meta(&bytes[..len]).is_err());
        }
        let records = vec![
            StoredRecord { seq: 1, from: ClientId::from_bytes([1; 16]), ctr: 7, record: vec![1, 2, 3] },
            StoredRecord { seq: 2, from: ClientId::from_bytes([2; 16]), ctr: 8, record: Vec::new() },
        ];
        let bytes = encode_records(&records);
        assert_eq!(decode_records(&bytes).unwrap(), records);
        for len in 0..bytes.len() {
            let cut = decode_records(&bytes[..len]);
            assert!(cut.is_err() || len == 0 || len == RECORD_HEAD + 3, "{len}");
        }
        let hostile = [0xFFu8; 40];
        assert!(decode_records(&hostile).is_err());
    }
}
