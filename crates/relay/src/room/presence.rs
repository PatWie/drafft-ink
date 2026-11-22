//! Presence and direct messages. Presence is never stored: the newest record of each person waits for the next
//! bundle, and the whole room may send only [`RelayConfig::presence_budget`] messages a second.

use std::collections::BTreeMap;

use drafftink_protocol::v2::{ErrorCode, PresenceEntry, RecordHeader, ServerMessage};
use drafftink_protocol::{ClientId, PRESENCE_TICK_MS};

use super::{ConnId, Output, Relay, retry_error};
use crate::store::{BlobStore, RelayStore};

#[derive(Debug, Default)]
pub(super) struct Presence {
    /// The newest record of each sender since the last bundle.
    pending: BTreeMap<ClientId, Vec<u8>>,
    /// Bundles go out at most once per tick: not before this time.
    next_bundle_ms: u64,
    /// Start of the tick window in which presence is counted against the room budget.
    window_start_ms: u64,
    window_count: u32,
}

impl Presence {
    pub(super) fn next_bundle(&self) -> Option<u64> {
        (!self.pending.is_empty()).then_some(self.next_bundle_ms)
    }

    /// Counts a message against the room budget: at most `budget * tick` messages per tick window.
    fn within_budget(&mut self, now_ms: u64, budget_per_sec: u16) -> bool {
        let tick = u64::from(PRESENCE_TICK_MS);
        if now_ms.saturating_sub(self.window_start_ms) >= tick {
            self.window_start_ms = now_ms;
            self.window_count = 0;
        }
        let per_tick = (u32::from(budget_per_sec) * PRESENCE_TICK_MS).div_ceil(1000).max(1);
        self.window_count += 1;
        self.window_count <= per_tick
    }
}

impl<S: RelayStore, B: BlobStore> Relay<S, B> {
    /// A person's share of the room's presence budget (design section 11.3).
    pub(super) fn presence_share(&self, people: usize) -> u16 {
        let share = usize::from(self.cfg.presence_budget) / people.max(1);
        u16::try_from(share).unwrap_or(u16::MAX).clamp(1, self.cfg.limits.max_presence_per_sec.max(1))
    }

    /// Sets everyone's presence rate to their share of the budget now.
    pub(super) fn share_presence_rate(&mut self) {
        let share = u32::from(self.presence_share(self.member_count()));
        for entry in self.conns.values_mut() {
            if let super::conn::Phase::Member(member) = &mut entry.phase {
                member.presence.set_rate(share);
            }
        }
    }

    /// Presence over the personal rate or the room budget is dropped without a word: it is the newest state that
    /// counts, and the next message carries it.
    pub(super) fn presence_in(&mut self, conn: ConnId, now_ms: u64, record: &[u8]) {
        let Some(client_id) = self.client_of(conn) else { return };
        let Ok(header) = RecordHeader::peek(record) else { return };
        if header.sender != client_id || self.member_count() < 2 {
            return;
        }
        let Some(member) = self.member_mut(conn) else { return };
        if member.presence.take(now_ms).is_err() || !self.presence.within_budget(now_ms, self.cfg.presence_budget) {
            return;
        }
        self.presence.next_bundle_ms = self.presence.next_bundle_ms.max(now_ms.min(self.presence.window_start_ms));
        self.presence.pending.insert(client_id, record.to_vec());
    }

    /// Sends the waiting presence to everybody else, if the tick has come.
    pub(super) fn bundle_presence(&mut self, now_ms: u64, out: &mut Output) {
        if self.presence.pending.is_empty() || now_ms < self.presence.next_bundle_ms {
            return;
        }
        let pending = std::mem::take(&mut self.presence.pending);
        self.presence.next_bundle_ms = now_ms.saturating_add(u64::from(PRESENCE_TICK_MS));
        let recipients: Vec<(ConnId, ClientId)> = self.members().map(|(id, m)| (id, m.client_id)).collect();
        for (conn, me) in recipients {
            let entries: Vec<PresenceEntry> = pending
                .iter()
                .filter(|(from, _)| **from != me)
                .map(|(from, record)| PresenceEntry { from: *from, record: record.clone() })
                .collect();
            if !entries.is_empty() {
                out.send(conn, &ServerMessage::PresenceBundle(entries));
            }
        }
    }

    pub(super) fn direct(&mut self, conn: ConnId, now_ms: u64, to: Option<ClientId>, record: &[u8], out: &mut Output) {
        let Some(from) = self.client_of(conn) else { return };
        let Ok(header) = RecordHeader::peek(record) else { return };
        if header.sender != from {
            let text = "a record names another sender than the connection";
            self.refuse(conn, now_ms, ServerMessage::error(ErrorCode::Forbidden, text), true, out);
            return;
        }
        if let Some(Err(wait)) = self.member_mut(conn).map(|m| m.direct.take(now_ms)) {
            let close = self.breach(conn, now_ms);
            self.refuse(
                conn,
                now_ms,
                retry_error(ErrorCode::RateLimited, "slow down; this message was dropped", wait),
                close,
                out,
            );
            return;
        }
        let message = ServerMessage::Direct { from, record: record.to_vec() };
        match to {
            None => self.broadcast(out, Some(conn), &message),
            Some(target) => {
                let receiver = self.members().find(|(_, m)| m.client_id == target).map(|(id, _)| id);
                if let Some(receiver) = receiver.filter(|id| *id != conn) {
                    out.send(receiver, &message);
                }
            }
        }
    }
}
