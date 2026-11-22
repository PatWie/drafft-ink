//! The stores of the relay on top of a host that does the SQL. The Durable Object's `sql.exec` is synchronous, so
//! the shell offers a few coarse calls ([`SqlHost`], [`BlobHost`]) and this module turns them into the
//! [`RelayStore`] and [`BlobStore`] ports. What is stored is bytes: the meta and the records are written with the
//! codec of `drafftink-relay`, which the other hosts use too, so one encoding exists.
//!
//! The coarse calls are the atomic units: `append_segment` writes a segment and the meta in one transaction, and
//! `put_snapshot` swaps the snapshot, the meta and the covered segments in one. The core never sees SQL.

use drafftink_protocol::v2::{ImageName, StoredRecord};
use drafftink_relay::codec::{decode_meta, decode_records, encode_meta, encode_records};
use drafftink_relay::{
    BlobInfo, BlobStats, BlobStore, CasOutcome, LogCounts, RelayStore, RoomMeta, Segment, StoreError, StoredSnapshot,
};

/// A failure of a call to the host.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// The host's daily capacity is used up until 00:00 UTC.
    #[error("the daily limit of the host is used up")]
    DailyLimit,
    /// Any other failure.
    #[error("{0}")]
    Failed(String),
}

impl From<HostError> for StoreError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::DailyLimit => StoreError::DailyLimit,
            HostError::Failed(text) => StoreError::Backend(text),
        }
    }
}

/// The room's rows, as the shell stores them. Every call is synchronous and all of them concern one room.
pub trait SqlHost {
    /// The encoded meta, if the room exists.
    fn load_meta(&mut self) -> Result<Option<Vec<u8>>, HostError>;

    /// Writes the encoded meta.
    fn save_meta(&mut self, meta: &[u8]) -> Result<(), HostError>;

    /// Writes one segment (the encoded records `first` to `last`) and the meta, in one transaction.
    fn append_segment(&mut self, first: u64, last: u64, data: &[u8], meta: &[u8]) -> Result<(), HostError>;

    /// The segments that hold a record above `after_seq`, oldest first, each as its encoded records. The list ends
    /// once `max_bytes` of segment data were returned, but holds at least one segment when there is one.
    /// `max_bytes == 0` means no limit. The first segment may start at or below `after_seq`.
    fn read_segments(&mut self, after_seq: u64, max_bytes: usize) -> Result<Vec<Vec<u8>>, HostError>;

    /// The snapshot: the sequence number it covers and the sealed record.
    fn read_snapshot(&mut self) -> Result<Option<(u64, Vec<u8>)>, HostError>;

    /// In one transaction: stores the snapshot and the meta and deletes every segment whose last record is at or
    /// below `covers`.
    fn put_snapshot(&mut self, covers: u64, record: &[u8], meta: &[u8]) -> Result<(), HostError>;

    /// Forgets the room. The Durable Object host does nothing here: the shell wipes the whole database when the
    /// core reports the deletion, which is one call and not a delete per row.
    fn delete_all(&mut self) -> Result<(), HostError>;
}

/// The sealed pictures of the room, as the shell stores them.
pub trait BlobHost {
    /// Stores a sealed picture, replacing one of the same name.
    fn put(&mut self, name: &ImageName, sealed: &[u8]) -> Result<(), HostError>;

    /// The sealed picture, if there is one.
    fn get(&mut self, name: &ImageName) -> Result<Option<Vec<u8>>, HostError>;

    /// The size of the sealed picture, if there is one.
    fn head(&mut self, name: &ImageName) -> Result<Option<u64>, HostError>;

    /// The names and sizes of every picture.
    fn list(&mut self) -> Result<Vec<(ImageName, u64)>, HostError>;

    /// The number of pictures and the sum of their sizes.
    fn stats(&mut self) -> Result<BlobStats, HostError>;

    /// Forgets every picture (see [`SqlHost::delete_all`]).
    fn delete_all(&mut self) -> Result<(), HostError>;
}

/// A [`RelayStore`] over a [`SqlHost`].
#[derive(Debug)]
pub struct HostStore<H> {
    host: H,
}

impl<H: SqlHost> HostStore<H> {
    /// The store of one room.
    pub fn new(host: H) -> Self {
        HostStore { host }
    }
}

impl<H: SqlHost> RelayStore for HostStore<H> {
    fn load_meta(&mut self) -> Result<Option<RoomMeta>, StoreError> {
        self.host.load_meta()?.map(|bytes| decode_meta(&bytes)).transpose()
    }

    fn save_meta(&mut self, meta: &RoomMeta) -> Result<(), StoreError> {
        Ok(self.host.save_meta(&encode_meta(meta))?)
    }

    fn append_segment(&mut self, segment: &Segment, meta: &RoomMeta) -> Result<(), StoreError> {
        let data = encode_records(&segment.records);
        Ok(self.host.append_segment(segment.first_seq(), segment.last_seq(), &data, &encode_meta(meta))?)
    }

    fn read_since(
        &mut self,
        after_seq: u64,
        max_records: usize,
        target_bytes: usize,
    ) -> Result<Vec<StoredRecord>, StoreError> {
        let mut out: Vec<StoredRecord> = Vec::new();
        let mut bytes = 0usize;
        let mut cursor = after_seq;
        loop {
            let segments = self.host.read_segments(cursor, target_bytes.saturating_sub(bytes).max(1))?;
            let mut newest = cursor;
            for data in &segments {
                for record in decode_records(data)? {
                    newest = newest.max(record.seq);
                    if record.seq <= after_seq {
                        continue;
                    }
                    if out.len() >= max_records || (!out.is_empty() && bytes >= target_bytes) {
                        return Ok(out);
                    }
                    bytes += record.record.len();
                    out.push(record);
                }
            }
            // Stop when the host has nothing more, or when it gave nothing new (damaged rows must not loop).
            if segments.is_empty() || newest <= cursor {
                return Ok(out);
            }
            cursor = newest;
        }
    }

    fn read_snapshot(&mut self) -> Result<Option<StoredSnapshot>, StoreError> {
        Ok(self.host.read_snapshot()?.map(|(covers_seq, record)| StoredSnapshot { covers_seq, record }))
    }

    fn put_snapshot_cas(
        &mut self,
        expected_snapshot_seq: u64,
        snapshot: &StoredSnapshot,
    ) -> Result<CasOutcome, StoreError> {
        let Some(mut meta) = self.load_meta()? else { return Ok(CasOutcome::Stale) };
        if !meta.accepts_snapshot(expected_snapshot_seq, snapshot.covers_seq) {
            return Ok(CasOutcome::Stale);
        }
        // What stays in the log once the covered segments are gone: the segments that end above the cover.
        let mut log = LogCounts { records: 0, bytes: 0 };
        for data in self.host.read_segments(snapshot.covers_seq, 0)? {
            for record in decode_records(&data)? {
                log.bytes += record.record.len() as u64;
                log.records += u32::from(record.seq > snapshot.covers_seq);
            }
        }
        meta.snapshot_seq = snapshot.covers_seq;
        meta.has_snapshot = true;
        meta.snapshot_bytes = snapshot.record.len() as u64;
        meta.log_records = log.records;
        meta.log_bytes = log.bytes;
        self.host.put_snapshot(snapshot.covers_seq, &snapshot.record, &encode_meta(&meta))?;
        Ok(CasOutcome::Swapped(log))
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        Ok(self.host.delete_all()?)
    }
}

/// A [`BlobStore`] over a [`BlobHost`].
#[derive(Debug)]
pub struct HostBlobs<H> {
    host: H,
}

impl<H: BlobHost> HostBlobs<H> {
    /// The picture store of one room.
    pub fn new(host: H) -> Self {
        HostBlobs { host }
    }
}

impl<H: BlobHost> BlobStore for HostBlobs<H> {
    fn put(&mut self, name: &ImageName, sealed: &[u8]) -> Result<(), StoreError> {
        Ok(self.host.put(name, sealed)?)
    }

    fn get(&mut self, name: &ImageName) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.host.get(name)?)
    }

    fn head(&mut self, name: &ImageName) -> Result<Option<u64>, StoreError> {
        Ok(self.host.head(name)?)
    }

    fn list(&mut self) -> Result<Vec<BlobInfo>, StoreError> {
        Ok(self.host.list()?.into_iter().map(|(name, size)| BlobInfo { name, size }).collect())
    }

    fn stats(&mut self) -> Result<BlobStats, StoreError> {
        Ok(self.host.stats()?)
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        Ok(self.host.delete_all()?)
    }
}
