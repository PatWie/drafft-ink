//! A relay of end-to-end encrypted rooms for tests: the real relay core ([`drafftink_relay::Relay`]) with memory
//! stores, joined to the loopback ends of several sessions. Like [`super::fake_server::FakeServer`] it is driven by
//! calling [`FakeRelay::step`] between the polls of the sessions it serves, with the time of the test.

use std::sync::Arc;

use drafftink_collab::LoopbackServer;
use drafftink_protocol::v2::RoomId;
use drafftink_relay::{ConnId, Effect, MemoryBlobStore, MemoryRelayStore, Output, Relay, RelayConfig};

use crate::ports::fake::FakeNet;

/// One loopback end and the connection of the core that belongs to it.
struct Link {
    end: Arc<LoopbackServer>,
    /// How many opens of the end were seen: a reconnect is a new connection on the same end.
    opens: usize,
    conn: Option<ConnId>,
}

/// The relay of one room. The room is the one of the first connection that is attached.
pub struct FakeRelay {
    config: RelayConfig,
    relay: Option<Relay<MemoryRelayStore, MemoryBlobStore>>,
    store: MemoryRelayStore,
    links: Vec<Link>,
    next_conn: u64,
    wake_at: Option<u64>,
}

impl FakeRelay {
    /// A relay with this policy and no room yet.
    pub fn new(config: RelayConfig) -> Self {
        Self { config, relay: None, store: MemoryRelayStore::new(), links: Vec::new(), next_conn: 0, wake_at: None }
    }

    /// What the relay stored, for a test that looks for what must not be there.
    pub fn store(&self) -> &MemoryRelayStore {
        &self.store
    }

    /// How many people are in the room.
    pub fn members(&self) -> usize {
        self.relay.as_ref().map_or(0, Relay::member_count)
    }

    /// Serves the connections `net` opened since the last call.
    pub fn attach(&mut self, net: &FakeNet) {
        for end in net.servers() {
            if !self.links.iter().any(|l| Arc::ptr_eq(&l.end, &end)) {
                self.links.push(Link { end, opens: 0, conn: None });
            }
        }
    }

    /// The id of the room in `/r/<id>/ws`.
    fn room_of(url: &str) -> Option<RoomId> {
        url.split("/r/").nth(1)?.split('/').next()?.parse().ok()
    }

    /// Handles what the clients sent and the time that passed. Returns true when it did anything.
    pub fn step(&mut self, now_ms: u64) -> bool {
        let mut busy = false;
        for i in 0..self.links.len() {
            busy |= self.step_link(i, now_ms);
        }
        if let (Some(relay), Some(at)) = (self.relay.as_mut(), self.wake_at)
            && at <= now_ms
        {
            let out = relay.on_wake(now_ms);
            busy = true;
            self.apply(out);
        }
        busy
    }

    fn step_link(&mut self, i: usize, now_ms: u64) -> bool {
        let Some(link) = self.links.get(i) else { return false };
        let (end, opens) = (Arc::clone(&link.end), link.opens);
        let mut busy = false;
        if end.opens() != opens {
            // A new connection on this end: the old one is gone.
            busy = true;
            self.drop_conn(i, now_ms);
            if self.relay.is_none()
                && let Some(room) = end.last_url().as_deref().and_then(Self::room_of)
            {
                self.relay = Some(Relay::new(room, self.config.clone(), self.store.clone(), MemoryBlobStore::new()));
            }
            self.next_conn += 1;
            let conn = ConnId(self.next_conn);
            if let Some(link) = self.links.get_mut(i) {
                link.opens = end.opens();
                link.conn = Some(conn);
            }
            if let Some(relay) = self.relay.as_mut() {
                let out = relay.connect(conn, now_ms);
                self.apply(out);
            }
        }
        let conn = self.links.get(i).and_then(|l| l.conn);
        if conn.is_some() && !end.is_open() {
            busy = true;
            self.drop_conn(i, now_ms);
            return busy;
        }
        let Some(conn) = conn else { return busy };
        while let Some(frame) = end.recv_raw() {
            busy = true;
            if let Some(relay) = self.relay.as_mut() {
                let out = relay.on_frame(conn, now_ms, &frame);
                self.apply(out);
            }
        }
        for text in end.take_texts() {
            busy = true;
            if let Some(relay) = self.relay.as_mut() {
                let out = relay.on_text(conn, now_ms, &text);
                self.apply(out);
            }
        }
        busy
    }

    fn drop_conn(&mut self, i: usize, now_ms: u64) {
        let Some(conn) = self.links.get_mut(i).and_then(|l| l.conn.take()) else { return };
        if let Some(relay) = self.relay.as_mut() {
            let out = relay.on_closed(conn, now_ms);
            self.apply(out);
        }
    }

    fn apply(&mut self, out: Output) {
        self.wake_at = out.wake_at;
        for effect in out.effects {
            match effect {
                Effect::Send { conn, frame } => {
                    if let Some(end) = self.end_of(conn) {
                        end.push_raw(frame.to_vec());
                    }
                }
                Effect::Text { conn, text } => {
                    if let Some(end) = self.end_of(conn) {
                        end.push_text(text);
                    }
                }
                Effect::Close { conn } => {
                    if let Some(end) = self.end_of(conn) {
                        end.close_cleanly();
                    }
                    if let Some(link) = self.links.iter_mut().find(|l| l.conn == Some(conn)) {
                        link.conn = None;
                    }
                }
            }
        }
    }

    fn end_of(&self, conn: ConnId) -> Option<Arc<LoopbackServer>> {
        self.links.iter().find(|l| l.conn == Some(conn)).map(|l| Arc::clone(&l.end))
    }
}
