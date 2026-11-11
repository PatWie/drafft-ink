//! The table of loaded rooms: load on demand, join, expire, shut down.

use super::actor::RoomActor;
use super::*;

/// Loads rooms on demand and keeps the table of loaded rooms.
pub struct RoomManager {
    pub(super) config: Arc<Config>,
    pub(super) store: Arc<dyn RoomStore>,
    pub(super) rooms: Mutex<HashMap<RoomId, RoomHandle>>,
    pub(super) next_generation: AtomicU64,
    pub(super) shutting_down: AtomicBool,
}

impl RoomManager {
    /// A manager with no rooms loaded.
    pub fn new(config: Arc<Config>, store: Arc<dyn RoomStore>) -> Arc<Self> {
        Arc::new(RoomManager {
            config,
            store,
            rooms: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
            shutting_down: AtomicBool::new(false),
        })
    }

    /// The number of loaded rooms.
    pub async fn loaded_rooms(&self) -> usize {
        self.rooms.lock().await.len()
    }

    /// Joins `peer` to `room`, loading the room when needed. On success the room has queued
    /// `Joined` and the first `SyncResponse` to the peer's outbox.
    pub async fn join(self: &Arc<Self>, room: &RoomId, peer: PeerSetup) -> Result<RoomHandle, Refusal> {
        let peer = Box::new(peer);
        for _ in 0..4 {
            let handle = self.get_or_load(room).await?;
            let (reply, answer) = oneshot::channel();
            if handle.tx.send(RoomCmd::Join { peer: peer.clone(), reply }).await.is_err() {
                continue; // the room unloaded between lookup and send
            }
            match answer.await {
                Ok(Ok(())) => return Ok(handle),
                Ok(Err(msg)) => return Err(msg),
                Err(_) => continue, // the room task ended before it answered
            }
        }
        Err(unavailable("the room is being restarted; try again", 1000))
    }

    async fn get_or_load(self: &Arc<Self>, room: &RoomId) -> Result<RoomHandle, Refusal> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(unavailable("the server is shutting down", 5000));
        }
        let mut rooms = self.rooms.lock().await;
        if let Some(handle) = rooms.get(room) {
            return Ok(handle.clone());
        }
        if rooms.len() >= self.config.limits.max_rooms {
            warn!("room limit of {} reached; refusing a new room", self.config.limits.max_rooms);
            return Err(unavailable("the server has reached its limit of active rooms", 30_000));
        }
        let (tx, rx) = mpsc::channel(COMMAND_QUEUE);
        let handle = RoomHandle { tx, generation: self.next_generation.fetch_add(1, Ordering::SeqCst) };
        rooms.insert(room.clone(), handle.clone());
        let actor = RoomActor::new(self.clone(), room.clone(), handle.generation);
        tokio::spawn(actor.run(rx));
        Ok(handle)
    }

    pub(super) async fn remove(&self, room: &RoomId, generation: u64) {
        let mut rooms = self.rooms.lock().await;
        if rooms.get(room).is_some_and(|h| h.generation == generation) {
            rooms.remove(room);
        }
    }

    /// Stops accepting joins, tells every peer, saves every room and waits for the room tasks.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let handles: Vec<RoomHandle> = self.rooms.lock().await.values().cloned().collect();
        let mut waits = Vec::new();
        for handle in handles {
            let (done, wait) = oneshot::channel();
            if handle.tx.send(RoomCmd::Shutdown { done }).await.is_ok() {
                waits.push(wait);
            }
        }
        for wait in waits {
            let _ = wait.await;
        }
    }

    /// Deletes stored rooms that nobody wrote to for `expire_after`. Loaded rooms are skipped.
    /// The room table stays locked while a room is deleted, so no room loads half-deleted.
    pub async fn expire_rooms(&self, expire_after: Duration, now_ms: u64) -> usize {
        let listed = match self.store.list().await {
            Ok(l) => l,
            Err(e) => {
                warn!("expiry: cannot list rooms: {e}");
                return 0;
            }
        };
        let cutoff = now_ms.saturating_sub(u64::try_from(expire_after.as_millis()).unwrap_or(u64::MAX));
        let mut deleted = 0;
        for meta in listed.into_iter().filter(|m| m.modified_ms < cutoff) {
            let rooms = self.rooms.lock().await;
            if rooms.contains_key(&meta.room) {
                continue;
            }
            match self.store.delete(&meta.room).await {
                Ok(()) => deleted += 1,
                Err(e) => warn!("expiry: cannot delete room {}: {e}", room_tag(&meta.room)),
            }
        }
        if deleted > 0 {
            info!("expired {deleted} room(s) untouched for {expire_after:?}");
        }
        deleted
    }
}
