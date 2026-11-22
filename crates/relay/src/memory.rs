//! In-memory stores for tests. Clones share one state, so a test can "restart" a relay on the same data, make the
//! store fail, and dump every stored byte to look for what must not be there.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use drafftink_protocol::v2::{ImageName, StoredRecord};

use crate::store::{
    BlobInfo, BlobStats, BlobStore, CasOutcome, LogCounts, RelayStore, RoomMeta, Segment, StoreError, StoredSnapshot,
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Default)]
struct RelayState {
    meta: Option<RoomMeta>,
    snapshot: Option<StoredSnapshot>,
    segments: Vec<Segment>,
    /// Calls that fail before they change anything.
    failures: u32,
    /// Segments written, for the cost tests.
    segments_written: u32,
}

/// A [`RelayStore`] in memory. Clones share the data.
#[derive(Debug, Clone, Default)]
pub struct MemoryRelayStore {
    state: Arc<Mutex<RelayState>>,
}

impl MemoryRelayStore {
    /// An empty store: no room.
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes the next `calls` store calls fail with a backend error, without changing anything.
    pub fn fail_next(&self, calls: u32) {
        lock(&self.state).failures = calls;
    }

    /// How many segments were written so far.
    pub fn segments_written(&self) -> u32 {
        lock(&self.state).segments_written
    }

    /// How many segments the store holds now.
    pub fn segment_count(&self) -> usize {
        lock(&self.state).segments.len()
    }

    /// Every byte the store holds, as separate buffers: the snapshot and each record. For the blind-relay test.
    pub fn dump(&self) -> Vec<Vec<u8>> {
        let state = lock(&self.state);
        let snapshot = state.snapshot.iter().map(|s| s.record.clone());
        let records = state.segments.iter().flat_map(|s| s.records.iter().map(|r| r.record.clone()));
        snapshot.chain(records).collect()
    }

    fn with<T>(&self, f: impl FnOnce(&mut RelayState) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let mut state = lock(&self.state);
        if state.failures > 0 {
            state.failures -= 1;
            return Err(StoreError::backend("injected failure"));
        }
        f(&mut state)
    }
}

fn counts(segments: &[Segment], after_seq: u64) -> LogCounts {
    let records = segments.iter().flat_map(|s| &s.records).filter(|r| r.seq > after_seq).count();
    LogCounts { records: u32::try_from(records).unwrap_or(u32::MAX), bytes: segments.iter().map(Segment::bytes).sum() }
}

impl RelayStore for MemoryRelayStore {
    fn load_meta(&mut self) -> Result<Option<RoomMeta>, StoreError> {
        self.with(|s| Ok(s.meta))
    }

    fn save_meta(&mut self, meta: &RoomMeta) -> Result<(), StoreError> {
        self.with(|s| {
            s.meta = Some(*meta);
            Ok(())
        })
    }

    fn append_segment(&mut self, segment: &Segment, meta: &RoomMeta) -> Result<(), StoreError> {
        self.with(|s| {
            s.segments.push(segment.clone());
            s.segments_written += 1;
            s.meta = Some(*meta);
            Ok(())
        })
    }

    fn read_since(
        &mut self,
        after_seq: u64,
        max_records: usize,
        target_bytes: usize,
    ) -> Result<Vec<StoredRecord>, StoreError> {
        self.with(|s| {
            let mut out: Vec<StoredRecord> = Vec::new();
            let mut bytes = 0usize;
            for record in s.segments.iter().flat_map(|seg| &seg.records).filter(|r| r.seq > after_seq) {
                if out.len() >= max_records || (!out.is_empty() && bytes >= target_bytes) {
                    break;
                }
                bytes += record.record.len();
                out.push(record.clone());
            }
            Ok(out)
        })
    }

    fn read_snapshot(&mut self) -> Result<Option<StoredSnapshot>, StoreError> {
        self.with(|s| Ok(s.snapshot.clone()))
    }

    fn put_snapshot_cas(
        &mut self,
        expected_snapshot_seq: u64,
        snapshot: &StoredSnapshot,
    ) -> Result<CasOutcome, StoreError> {
        self.with(|s| {
            let Some(meta) = s.meta.as_mut() else { return Ok(CasOutcome::Stale) };
            if !meta.accepts_snapshot(expected_snapshot_seq, snapshot.covers_seq) {
                return Ok(CasOutcome::Stale);
            }
            s.segments.retain(|seg| seg.last_seq() > snapshot.covers_seq);
            let log = counts(&s.segments, snapshot.covers_seq);
            meta.snapshot_seq = snapshot.covers_seq;
            meta.has_snapshot = true;
            meta.snapshot_bytes = snapshot.record.len() as u64;
            meta.log_records = log.records;
            meta.log_bytes = log.bytes;
            s.snapshot = Some(snapshot.clone());
            Ok(CasOutcome::Swapped(log))
        })
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        self.with(|s| {
            s.meta = None;
            s.snapshot = None;
            s.segments.clear();
            Ok(())
        })
    }
}

#[derive(Debug, Default)]
struct BlobState {
    blobs: BTreeMap<ImageName, Vec<u8>>,
    failures: u32,
}

/// A [`BlobStore`] in memory. Clones share the data.
#[derive(Debug, Clone, Default)]
pub struct MemoryBlobStore {
    state: Arc<Mutex<BlobState>>,
}

impl MemoryBlobStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes the next `calls` store calls fail with a backend error, without changing anything.
    pub fn fail_next(&self, calls: u32) {
        lock(&self.state).failures = calls;
    }

    /// Every stored image, as stored.
    pub fn dump(&self) -> Vec<Vec<u8>> {
        lock(&self.state).blobs.values().cloned().collect()
    }

    fn with<T>(&self, f: impl FnOnce(&mut BTreeMap<ImageName, Vec<u8>>) -> T) -> Result<T, StoreError> {
        let mut state = lock(&self.state);
        if state.failures > 0 {
            state.failures -= 1;
            return Err(StoreError::backend("injected failure"));
        }
        Ok(f(&mut state.blobs))
    }
}

impl BlobStore for MemoryBlobStore {
    fn put(&mut self, name: &ImageName, sealed: &[u8]) -> Result<(), StoreError> {
        self.with(|b| {
            b.insert(*name, sealed.to_vec());
        })
    }

    fn get(&mut self, name: &ImageName) -> Result<Option<Vec<u8>>, StoreError> {
        self.with(|b| b.get(name).cloned())
    }

    fn head(&mut self, name: &ImageName) -> Result<Option<u64>, StoreError> {
        self.with(|b| b.get(name).map(|v| v.len() as u64))
    }

    fn list(&mut self) -> Result<Vec<BlobInfo>, StoreError> {
        self.with(|b| b.iter().map(|(name, v)| BlobInfo { name: *name, size: v.len() as u64 }).collect())
    }

    fn stats(&mut self) -> Result<BlobStats, StoreError> {
        self.with(|b| BlobStats {
            count: u32::try_from(b.len()).unwrap_or(u32::MAX),
            bytes: b.values().map(|v| v.len() as u64).sum(),
        })
    }

    fn delete_all(&mut self) -> Result<(), StoreError> {
        self.with(BTreeMap::clear)
    }
}
