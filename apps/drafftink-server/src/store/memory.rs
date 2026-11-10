//! The in-memory provider: for tests and throwaway servers.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;
use drafftink_protocol::RoomId;

use super::{RoomMeta, RoomStore, StoreError, StoredRoom, check_room, lock};
use crate::clock::now_ms;

#[derive(Default)]
struct Entry {
    snapshot: Option<Vec<u8>>,
    updates: VecDeque<Vec<u8>>,
    modified_ms: u64,
}

/// Rooms in process memory. Everything is lost on restart.
#[derive(Default)]
pub struct MemoryStore {
    rooms: Mutex<BTreeMap<RoomId, Entry>>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RoomStore for MemoryStore {
    fn name(&self) -> &'static str {
        "memory"
    }

    async fn load(&self, room: &RoomId) -> Result<Option<StoredRoom>, StoreError> {
        check_room(room)?;
        Ok(lock(&self.rooms)
            .get(room)
            .map(|e| StoredRoom { snapshot: e.snapshot.clone(), updates: e.updates.iter().cloned().collect() }))
    }

    async fn append_update(&self, room: &RoomId, update: &[u8]) -> Result<(), StoreError> {
        check_room(room)?;
        let mut rooms = lock(&self.rooms);
        let entry = rooms.entry(room.clone()).or_default();
        entry.updates.push_back(update.to_vec());
        entry.modified_ms = now_ms();
        Ok(())
    }

    async fn compact(&self, room: &RoomId, snapshot: &[u8], covered: u64) -> Result<(), StoreError> {
        check_room(room)?;
        let mut rooms = lock(&self.rooms);
        let entry = rooms.entry(room.clone()).or_default();
        entry.snapshot = Some(snapshot.to_vec());
        let drop_n = usize::try_from(covered).unwrap_or(usize::MAX).min(entry.updates.len());
        entry.updates.drain(..drop_n);
        entry.modified_ms = now_ms();
        Ok(())
    }

    async fn delete(&self, room: &RoomId) -> Result<(), StoreError> {
        check_room(room)?;
        lock(&self.rooms).remove(room);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<RoomMeta>, StoreError> {
        Ok(lock(&self.rooms)
            .iter()
            .map(|(room, e)| RoomMeta { room: room.clone(), modified_ms: e.modified_ms })
            .collect())
    }
}
