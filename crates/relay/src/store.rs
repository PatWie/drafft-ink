//! The storage ports of the relay. Both are synchronous: a Durable Object's SQLite is, and the tokio host runs the
//! same calls on the task of the room. One store belongs to one room.

use drafftink_protocol::v2::{ImageName, StoredRecord};

/// A storage failure. The core treats every one of them as "this room cannot be served or saved now".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// A read or write failed (disk, database, object store).
    #[error("storage failed: {0}")]
    Backend(String),
    /// Stored data does not have the shape this build wrote.
    #[error("stored data is damaged: {0}")]
    Corrupt(String),
}

impl StoreError {
    /// A backend failure with a message.
    pub fn backend(message: impl Into<String>) -> Self {
        StoreError::Backend(message.into())
    }
}

/// What the relay knows about a room besides its records. Stores save it with each write that changes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoomMeta {
    /// The newest sequence number the room has handed out.
    pub head_seq: u64,
    /// The sequence number the stored snapshot covers. Meaningful only when [`has_snapshot`](Self::has_snapshot).
    pub snapshot_seq: u64,
    /// Whether the room holds a snapshot. A seed covers sequence number 0, so `snapshot_seq == 0` is not enough.
    pub has_snapshot: bool,
    /// Size of the sealed snapshot, in bytes.
    pub snapshot_bytes: u64,
    /// How many records are stored after the snapshot.
    pub log_records: u32,
    /// How many bytes the stored segments take.
    pub log_bytes: u64,
    /// When the room is deleted if nobody visits, in milliseconds since the epoch. 0 means never.
    pub deadline_ms: u64,
}

impl RoomMeta {
    /// The sealed bytes the room holds: the snapshot and the log.
    pub fn state_bytes(&self) -> u64 {
        self.snapshot_bytes.saturating_add(self.log_bytes)
    }

    /// Whether the room has neither a snapshot nor a record. Only such a room takes a seed.
    pub fn is_empty(&self) -> bool {
        self.head_seq == 0 && !self.has_snapshot
    }

    /// The compare-and-set rule for a snapshot (design section 8.4): the client must have seen the current snapshot
    /// and must cover a newer point that exists. The one more case is the seed of an empty room: it expects no
    /// snapshot and covers nothing.
    pub fn accepts_snapshot(&self, expected_snapshot_seq: u64, covers_seq: u64) -> bool {
        if self.is_empty() {
            return expected_snapshot_seq == 0 && covers_seq == 0;
        }
        expected_snapshot_seq == self.snapshot_seq && covers_seq > self.snapshot_seq && covers_seq <= self.head_seq
    }
}

/// A sealed snapshot with the sequence number it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSnapshot {
    /// The last sequence number the snapshot includes. It equals `covers_seq` in the record's header.
    pub covers_seq: u64,
    /// The sealed record, of kind `Snapshot`.
    pub record: Vec<u8>,
}

/// Records written together: one row in a database. The sequence numbers are contiguous and ascending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The records, oldest first. Never empty.
    pub records: Vec<StoredRecord>,
}

impl Segment {
    /// The sequence number of the first record.
    pub fn first_seq(&self) -> u64 {
        self.records.first().map_or(0, |r| r.seq)
    }

    /// The sequence number of the last record.
    pub fn last_seq(&self) -> u64 {
        self.records.last().map_or(0, |r| r.seq)
    }

    /// The sealed bytes of all records.
    pub fn bytes(&self) -> u64 {
        self.records.iter().map(|r| r.record.len() as u64).sum()
    }
}

/// How much of the log is left after a snapshot replaced its beginning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogCounts {
    /// Records after the snapshot.
    pub records: u32,
    /// Bytes of the stored segments.
    pub bytes: u64,
}

/// What a compare-and-set did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CasOutcome {
    /// The snapshot is stored and the segments it covers are gone.
    Swapped(LogCounts),
    /// The room was not in the state the client expected. Nothing changed.
    Stale,
}

/// The records and the snapshot of one room.
pub trait RelayStore {
    /// The room's meta, or `None` when the room does not exist.
    fn load_meta(&mut self) -> Result<Option<RoomMeta>, StoreError>;

    /// Creates the room, or saves a changed meta (a moved deadline).
    fn save_meta(&mut self, meta: &RoomMeta) -> Result<(), StoreError>;

    /// Writes a segment and the meta that goes with it in one transaction.
    fn append_segment(&mut self, segment: &Segment, meta: &RoomMeta) -> Result<(), StoreError>;

    /// The records with a sequence number above `after_seq`, oldest first: at most `max_records`, and not more
    /// bytes than `target_bytes` once there is one record (so a single large record is returned).
    fn read_since(
        &mut self,
        after_seq: u64,
        max_records: usize,
        target_bytes: usize,
    ) -> Result<Vec<StoredRecord>, StoreError>;

    /// The stored snapshot, if the room has one.
    fn read_snapshot(&mut self) -> Result<Option<StoredSnapshot>, StoreError>;

    /// Compare-and-set, in one transaction: if [`RoomMeta::accepts_snapshot`] holds for the stored meta, store the
    /// snapshot, delete every segment that lies entirely at or below its cover, and save the new meta. Otherwise
    /// change nothing and answer [`CasOutcome::Stale`].
    fn put_snapshot_cas(
        &mut self,
        expected_snapshot_seq: u64,
        snapshot: &StoredSnapshot,
    ) -> Result<CasOutcome, StoreError>;

    /// Deletes everything of the room: meta, snapshot and segments.
    fn delete_all(&mut self) -> Result<(), StoreError>;
}

/// A stored image: its keyed name and the size of the sealed bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobInfo {
    /// The keyed name.
    pub name: ImageName,
    /// Size of the sealed bytes.
    pub size: u64,
}

/// How many images a room holds and how many sealed bytes they take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlobStats {
    /// Number of images.
    pub count: u32,
    /// Sum of their sealed sizes.
    pub bytes: u64,
}

/// The sealed images of one room, by keyed name.
pub trait BlobStore {
    /// Stores the sealed image under `name`, replacing an earlier one.
    fn put(&mut self, name: &ImageName, sealed: &[u8]) -> Result<(), StoreError>;

    /// The sealed image, if there is one.
    fn get(&mut self, name: &ImageName) -> Result<Option<Vec<u8>>, StoreError>;

    /// The size of the sealed image, if there is one.
    fn head(&mut self, name: &ImageName) -> Result<Option<u64>, StoreError>;

    /// Every image of the room.
    fn list(&mut self) -> Result<Vec<BlobInfo>, StoreError>;

    /// The count and total size of the room's images.
    fn stats(&mut self) -> Result<BlobStats, StoreError>;

    /// Deletes every image of the room.
    fn delete_all(&mut self) -> Result<(), StoreError>;
}

impl<T: RelayStore + ?Sized> RelayStore for Box<T> {
    fn load_meta(&mut self) -> Result<Option<RoomMeta>, StoreError> {
        (**self).load_meta()
    }

    fn save_meta(&mut self, meta: &RoomMeta) -> Result<(), StoreError> {
        (**self).save_meta(meta)
    }

    fn append_segment(&mut self, segment: &Segment, meta: &RoomMeta) -> Result<(), StoreError> {
        (**self).append_segment(segment, meta)
    }

    fn read_since(
        &mut self,
        after_seq: u64,
        max_records: usize,
        target_bytes: usize,
    ) -> Result<Vec<StoredRecord>, StoreError> {
        (**self).read_since(after_seq, max_records, target_bytes)
    }

    fn read_snapshot(&mut self) -> Result<Option<StoredSnapshot>, StoreError> {
        (**self).read_snapshot()
    }

    fn put_snapshot_cas(
        &mut self,
        expected_snapshot_seq: u64,
        snapshot: &StoredSnapshot,
    ) -> Result<CasOutcome, StoreError> {
        (**self).put_snapshot_cas(expected_snapshot_seq, snapshot)
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        (**self).delete_all()
    }
}

impl<T: BlobStore + ?Sized> BlobStore for Box<T> {
    fn put(&mut self, name: &ImageName, sealed: &[u8]) -> Result<(), StoreError> {
        (**self).put(name, sealed)
    }

    fn get(&mut self, name: &ImageName) -> Result<Option<Vec<u8>>, StoreError> {
        (**self).get(name)
    }

    fn head(&mut self, name: &ImageName) -> Result<Option<u64>, StoreError> {
        (**self).head(name)
    }

    fn list(&mut self) -> Result<Vec<BlobInfo>, StoreError> {
        (**self).list()
    }

    fn stats(&mut self) -> Result<BlobStats, StoreError> {
        (**self).stats()
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        (**self).delete_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(head: u64, snapshot: Option<u64>) -> RoomMeta {
        RoomMeta {
            head_seq: head,
            snapshot_seq: snapshot.unwrap_or(0),
            has_snapshot: snapshot.is_some(),
            ..RoomMeta::default()
        }
    }

    #[test]
    fn only_an_empty_room_takes_a_seed() {
        assert!(meta(0, None).accepts_snapshot(0, 0));
        assert!(!meta(0, None).accepts_snapshot(0, 1), "nothing to cover in an empty room");
        assert!(!meta(3, None).accepts_snapshot(0, 0), "a room with records is not seeded");
        assert!(!meta(0, Some(0)).accepts_snapshot(0, 0), "a seeded room is not seeded twice");
    }

    #[test]
    fn a_compaction_expects_the_current_snapshot_and_covers_something_newer() {
        let room = meta(10, Some(4));
        assert!(room.accepts_snapshot(4, 10));
        assert!(room.accepts_snapshot(4, 5));
        assert!(!room.accepts_snapshot(0, 10), "stale expectation");
        assert!(!room.accepts_snapshot(4, 4), "covers nothing new");
        assert!(!room.accepts_snapshot(4, 11), "covers a record that does not exist");
        assert!(meta(10, None).accepts_snapshot(0, 6), "the first compaction expects no snapshot");
        assert!(meta(10, Some(0)).accepts_snapshot(0, 6), "a seeded room compacts like any other");
    }
}
