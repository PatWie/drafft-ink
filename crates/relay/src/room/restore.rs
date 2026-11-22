//! Waking from hibernation. A Durable Object can be evicted while its sockets stay open; the host rebuilds the core
//! on the next event and tells it about every open socket, from what it saved with the socket. Nothing is sent to
//! anybody and the store is not read: the core only gets back what it knew about its connections.
//!
//! What is lost by design: presence that waited for a bundle, the fill of the rate buckets, the `expiring` flag
//! (the first [`Relay::on_wake`] works it out again from the stored deadline), the grace after the last close, and
//! a compaction advice that was pending.

use drafftink_protocol::ClientId;

use super::conn::{Conn, Member, Phase};
use super::{ConnId, Output, Relay};
use crate::store::{BlobStore, RelayStore};

impl<S: RelayStore, B: BlobStore> Relay<S, B> {
    /// Re-creates a joined connection exactly as it was: its client id, its join `order` and the time it was last
    /// `last_heard_ms`. Its rate buckets are full at `now_ms`, and the presence rate is the personal share for the
    /// members there are now, so call it for every member before the first [`on_wake`](Self::on_wake).
    ///
    /// It sends nothing (no `PeerJoined`, no `Joined`), does not read the store and does not move the room's
    /// deadline. The [`Output`] holds only `wake_at`, and a wake at `now_ms` is asked for so that retention is worked
    /// out again.
    pub fn restore_member(
        &mut self,
        conn: ConnId,
        client_id: ClientId,
        order: u64,
        last_heard_ms: u64,
        now_ms: u64,
    ) -> Output {
        let rate = self.presence_share(self.member_count().saturating_add(1));
        let member = Member::new(client_id, order, now_ms, &self.cfg, rate);
        self.conns.insert(conn, Conn { last_heard_ms, phase: Phase::Member(member) });
        self.next_order = self.next_order.max(order);
        self.share_presence_rate();
        self.restored(now_ms)
    }

    /// Re-creates a socket that had connected at `connected_at_ms` and had not joined when the object slept. It has
    /// until `connected_at_ms + join_timeout_ms` to send `Join`.
    pub fn restore_waiting(&mut self, conn: ConnId, connected_at_ms: u64, now_ms: u64) -> Output {
        let join_by_ms = connected_at_ms.saturating_add(self.cfg.join_timeout_ms);
        self.conns.insert(conn, Conn::new(connected_at_ms, join_by_ms));
        self.restored(now_ms)
    }

    /// Notes that `conn` was heard from at `at_ms`. A host that lets the runtime answer the client's `ping` frames
    /// (a Durable Object's automatic response) never shows them to the core, so it feeds the time of the runtime's
    /// last answer here. The time only moves forward. An unknown connection is ignored.
    pub fn heard(&mut self, conn: ConnId, at_ms: u64) {
        if let Some(entry) = self.conns.get_mut(&conn) {
            entry.last_heard_ms = entry.last_heard_ms.max(at_ms);
        }
    }

    /// The answer to a restore: the timers, and a retention check at the first wake.
    fn restored(&mut self, now_ms: u64) -> Output {
        self.retention_wake = Some(self.retention_wake.map_or(now_ms, |at| at.min(now_ms)));
        self.finish(now_ms, Output::default())
    }
}
