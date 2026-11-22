//! What every scenario needs: a room with its key, the clients of the room, and a few ways to draw and to wait.

use std::collections::BTreeSet;
use std::time::Duration;

use drafftink_protocol::v2::JoinMode;
use drafftink_scene::{Element, ElementId, ElementKind, SceneRead, SceneStore, SceneTx, Transform};
use drafftink_seal::{RoomKey, RoomKeys};

use super::common::room_client::{BlobOptions, Clock, Peer, Taps, peer, pump_for, pump_until};
use super::relay::Relay;

/// One room on one relay, with the clients that take part.
pub struct Room<'a> {
    pub relay: &'a Relay,
    pub key: RoomKey,
    pub keys: RoomKeys,
    pub taps: Taps,
    pub clock: Clock,
}

impl<'a> Room<'a> {
    /// A room under a new random key, so that no two tests meet in a room.
    pub fn new(relay: &'a Relay) -> Self {
        let key = RoomKey::generate().expect("a random key");
        Room::with_key(relay, key)
    }

    pub fn with_key(relay: &'a Relay, key: RoomKey) -> Self {
        let keys = RoomKeys::new(&key);
        Room { relay, key, keys, taps: Taps::default(), clock: Clock::new() }
    }

    /// Client number `n`, joining as `mode`. It has started: pump it.
    pub fn client(&self, n: u8, mode: JoinMode) -> Peer {
        peer(n, &self.key, &self.relay.url, mode, &self.taps, BlobOptions::default())
    }

    /// Pumps `peers` until `done` holds.
    pub fn until(&self, peers: &mut [&mut Peer], what: &str, done: impl Fn(&[&mut Peer]) -> bool) {
        pump_until(self.clock, peers, what, done);
    }

    /// Pumps `peers` for `time` and keeps them alive.
    pub fn idle(&self, peers: &mut [&mut Peer], time: Duration) {
        pump_for(self.clock, peers, u64::try_from(time.as_millis()).expect("a short time"));
    }

    /// Waits until every peer is online with nothing left to send.
    pub fn settle(&self, peers: &mut [&mut Peer]) {
        self.until(peers, "every client to be online and in sync", |p| p.iter().all(|c| c.settled()));
    }

    /// Adds a rectangle called `id` at `x`.
    pub fn draw(&self, who: &mut Peer, id: &str, x: f64) {
        let mut tx = who.store.begin();
        let index = tx.next_top_index().expect("room above the top");
        let mut element = Element::new(ElementId::new(id).expect("an id"), ElementKind::Rectangle, index);
        element.transform = Transform::new(x, 20.0, 100.0, 50.0);
        tx.insert(element).expect("inserts");
        tx.commit().expect("commits");
        who.session.client_mut().local_changed(self.clock.now());
    }

    /// Moves the rectangle `id` to `x`.
    pub fn move_to(&self, who: &mut Peer, id: &str, x: f64) {
        let mut tx = who.store.begin();
        tx.update(&ElementId::new(id).expect("an id"), |e| e.transform.x = x).expect("updates");
        tx.commit().expect("commits");
        who.session.client_mut().local_changed(self.clock.now());
    }

    /// Deletes the rectangle `id` (a tombstone, as the app does).
    pub fn erase(&self, who: &mut Peer, id: &str) {
        let mut tx = who.store.begin();
        tx.update(&ElementId::new(id).expect("an id"), |e| e.deleted = true).expect("updates");
        tx.commit().expect("commits");
        who.session.client_mut().local_changed(self.clock.now());
    }
}

/// The live rectangles of a client as `(id, x)`, in a fixed order.
pub fn drawing(who: &Peer) -> BTreeSet<(String, i64)> {
    who.store.iter().map(|e| (e.id.to_string(), e.transform.x as i64)).collect()
}

/// A shorthand for a drawing: `[("a", 10), ("b", 20)]`.
pub fn expect(items: &[(&str, i64)]) -> BTreeSet<(String, i64)> {
    items.iter().map(|(id, x)| ((*id).to_owned(), *x)).collect()
}
