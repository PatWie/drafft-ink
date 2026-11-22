//! An in-memory host that behaves like the SQL of the shell (`src/sql-store.ts`): the same calls, the same
//! atomic units. Clones share the data, so a test can build a second core over the same rows (a wake after a
//! hibernation).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use drafftink_protocol::v2::ImageName;
use drafftink_relay::BlobStats;

use crate::host::{BlobHost, HostError, SqlHost};

#[derive(Debug, Default)]
struct Rows {
    meta: Option<Vec<u8>>,
    snapshot: Option<(u64, Vec<u8>)>,
    /// `first` -> (`last`, data), like the primary key of the segments table.
    segments: BTreeMap<u64, (u64, Vec<u8>)>,
    images: BTreeMap<[u8; 16], Vec<u8>>,
    /// The next call fails with this error.
    fail: Option<HostError>,
    /// Calls so far, by name.
    calls: BTreeMap<&'static str, u32>,
}

/// The rows of one room.
#[derive(Debug, Clone, Default)]
pub struct FakeHost {
    rows: Rc<RefCell<Rows>>,
}

impl FakeHost {
    /// Makes every following call fail with `error`.
    pub fn fail_with(&self, error: HostError) {
        self.rows.borrow_mut().fail = Some(error);
    }

    /// How many segment rows exist.
    pub fn segment_rows(&self) -> usize {
        self.rows.borrow().segments.len()
    }

    /// How often a call of this name was made.
    pub fn calls(&self, name: &str) -> u32 {
        self.rows.borrow().calls.get(name).copied().unwrap_or(0)
    }

    /// Everything the host holds, as separate buffers.
    pub fn dump(&self) -> Vec<Vec<u8>> {
        let rows = self.rows.borrow();
        let mut out: Vec<Vec<u8>> = rows.meta.iter().cloned().collect();
        out.extend(rows.snapshot.iter().map(|(_, record)| record.clone()));
        out.extend(rows.segments.values().map(|(_, data)| data.clone()));
        out.extend(rows.images.values().cloned());
        out
    }

    fn enter(&self, call: &'static str) -> Result<std::cell::RefMut<'_, Rows>, HostError> {
        let mut rows = self.rows.borrow_mut();
        *rows.calls.entry(call).or_default() += 1;
        match rows.fail.clone() {
            Some(error) => Err(error),
            None => Ok(rows),
        }
    }
}

impl SqlHost for FakeHost {
    fn load_meta(&mut self) -> Result<Option<Vec<u8>>, HostError> {
        Ok(self.enter("load_meta")?.meta.clone())
    }

    fn save_meta(&mut self, meta: &[u8]) -> Result<(), HostError> {
        self.enter("save_meta")?.meta = Some(meta.to_vec());
        Ok(())
    }

    fn append_segment(&mut self, first: u64, last: u64, data: &[u8], meta: &[u8]) -> Result<(), HostError> {
        let mut rows = self.enter("append_segment")?;
        rows.segments.insert(first, (last, data.to_vec()));
        rows.meta = Some(meta.to_vec());
        Ok(())
    }

    fn read_segments(&mut self, after_seq: u64, max_bytes: usize) -> Result<Vec<Vec<u8>>, HostError> {
        let rows = self.enter("read_segments")?;
        // Like the SQL: start at the segment that holds `after_seq + 1`, then go on in order.
        let start = rows.segments.range(..=after_seq.saturating_add(1)).next_back().map_or(0, |(first, _)| *first);
        let mut out = Vec::new();
        let mut bytes = 0usize;
        for (_, (last, data)) in rows.segments.range(start..) {
            if *last <= after_seq {
                continue;
            }
            out.push(data.clone());
            bytes += data.len();
            if max_bytes != 0 && bytes >= max_bytes {
                break;
            }
        }
        Ok(out)
    }

    fn read_snapshot(&mut self) -> Result<Option<(u64, Vec<u8>)>, HostError> {
        Ok(self.enter("read_snapshot")?.snapshot.clone())
    }

    fn put_snapshot(&mut self, covers: u64, record: &[u8], meta: &[u8]) -> Result<(), HostError> {
        let mut rows = self.enter("put_snapshot")?;
        rows.snapshot = Some((covers, record.to_vec()));
        rows.meta = Some(meta.to_vec());
        rows.segments.retain(|_, (last, _)| *last > covers);
        Ok(())
    }

    fn delete_all(&mut self) -> Result<(), HostError> {
        self.enter("delete_all")?;
        Ok(())
    }
}

impl BlobHost for FakeHost {
    fn put(&mut self, name: &ImageName, sealed: &[u8]) -> Result<(), HostError> {
        self.enter("blob_put")?.images.insert(*name.as_bytes(), sealed.to_vec());
        Ok(())
    }

    fn get(&mut self, name: &ImageName) -> Result<Option<Vec<u8>>, HostError> {
        Ok(self.enter("blob_get")?.images.get(name.as_bytes()).cloned())
    }

    fn head(&mut self, name: &ImageName) -> Result<Option<u64>, HostError> {
        Ok(self.enter("blob_head")?.images.get(name.as_bytes()).map(|d| d.len() as u64))
    }

    fn list(&mut self) -> Result<Vec<(ImageName, u64)>, HostError> {
        let rows = self.enter("blob_list")?;
        Ok(rows.images.iter().map(|(name, data)| (ImageName::from_bytes(*name), data.len() as u64)).collect())
    }

    fn stats(&mut self) -> Result<BlobStats, HostError> {
        let rows = self.enter("blob_stats")?;
        let count = u32::try_from(rows.images.len()).unwrap_or(u32::MAX);
        Ok(BlobStats { count, bytes: rows.images.values().map(|d| d.len() as u64).sum() })
    }

    fn delete_all(&mut self) -> Result<(), HostError> {
        self.enter("blob_delete_all")?;
        Ok(())
    }
}
