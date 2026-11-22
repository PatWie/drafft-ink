//! `Join`: the membership check, creating or opening the room, the visit that moves the deadline, and the first
//! answers (`Joined`, then `Catchup`).

use drafftink_protocol::v2::{ClientMessage, ErrorCode, JoinMode, Proof, RoomId, RoomState, ServerMessage};
use drafftink_protocol::{ClientId, PROTOCOL_V2, negotiate_version_within};

use super::conn::{Member, Phase};
use super::{ConnId, Output, Relay, StoreOp};
use crate::store::{BlobStore, RelayStore, RoomMeta};

impl<S: RelayStore, B: BlobStore> Relay<S, B> {
    pub(super) fn join(&mut self, conn: ConnId, now_ms: u64, message: ClientMessage, out: &mut Output) {
        let ClientMessage::Join { min_version, max_version, room, client_id, mode, proof, last_seq, captcha: _ } =
            message
        else {
            return;
        };
        match self.admit(now_ms, (min_version, max_version), room, &proof, (client_id, mode), out) {
            Ok(state) => self.welcome(conn, now_ms, client_id, state, last_seq, out),
            Err(refusal) => self.refuse(conn, now_ms, refusal, true, out),
        }
    }

    /// Everything that can refuse a join, in the order of the design: version, membership, capacity, the room.
    /// A successful join is a visit: it has moved the deadline when this returns.
    fn admit(
        &mut self,
        now_ms: u64,
        versions: (u16, u16),
        room: RoomId,
        proof: &Proof,
        (client_id, mode): (ClientId, JoinMode),
        out: &mut Output,
    ) -> Result<RoomState, ServerMessage> {
        if negotiate_version_within(versions.0, versions.1, PROTOCOL_V2..=PROTOCOL_V2).is_none() {
            let text = "This relay only hosts end-to-end encrypted rooms (protocol version 2). Update the app.";
            return Err(ServerMessage::error(ErrorCode::UnsupportedVersion, text));
        }
        if room != self.room || !room.accepts(proof) {
            return Err(ServerMessage::error(ErrorCode::Unauthorized, "The room link is not valid for this room."));
        }
        let rejoining = self.members().any(|(_, m)| m.client_id == client_id);
        if !rejoining && self.member_count() >= self.cfg.max_peers {
            return Err(ServerMessage::error(ErrorCode::RoomFull, "This room is full. Trying again in a moment."));
        }
        let stored = self.load_meta().map_err(|_| Self::unavailable())?;
        let (state, meta) = match (stored, mode) {
            (Some(meta), _) => (RoomState::Existing, meta),
            (None, JoinMode::Open) => {
                let text = "This room does not exist. Rooms are deleted after a period without visitors, or the link is incomplete.";
                return Err(ServerMessage::error(ErrorCode::RoomMissing, text));
            }
            (None, JoinMode::Create) => (RoomState::Created, RoomMeta::default()),
        };
        let visited = RoomMeta { deadline_ms: self.life.deadline_after_visit(now_ms), ..meta };
        if state == RoomState::Created || visited != meta {
            self.store.save_meta(&visited).map_err(|_| Self::unavailable())?;
            out.store_ops.push(StoreOp::SaveMeta);
        }
        self.meta = super::MetaState::Loaded(visited);
        Ok(state)
    }

    fn welcome(
        &mut self,
        conn: ConnId,
        now_ms: u64,
        client_id: ClientId,
        state: RoomState,
        last_seq: u64,
        out: &mut Output,
    ) {
        let other = self.members().find(|(id, m)| *id != conn && m.client_id == client_id).map(|(id, _)| id);
        if let Some(old) = other {
            let text = "This room was opened again with the same identity, so this connection is closed.";
            self.refuse(old, now_ms, ServerMessage::error(ErrorCode::Replaced, text), true, out);
        }
        // The answer to the join needs the records on disk, so write what is buffered first.
        if !self.flush(now_ms, out) {
            self.refuse(conn, now_ms, Self::unavailable(), true, out);
            return;
        }
        let Some(meta) = self.meta_now() else { return };
        self.next_order += 1;
        let order = self.next_order;
        let members_after = self.member_count() + 1;
        let rate = self.presence_share(members_after);
        let peers: Vec<ClientId> = self.members().map(|(_, m)| m.client_id).collect();
        let member = Member::new(client_id, order, now_ms, &self.cfg, rate);
        if let Some(entry) = self.conns.get_mut(&conn) {
            entry.phase = Phase::Member(member);
        }
        self.share_presence_rate();
        let limits = drafftink_protocol::v2::RoomLimits { max_presence_per_sec: rate, ..self.cfg.limits };
        out.send(
            conn,
            &ServerMessage::Joined {
                protocol_version: PROTOCOL_V2,
                room_state: state,
                head_seq: meta.head_seq,
                snapshot_seq: meta.snapshot_seq,
                log_records: meta.log_records,
                log_bytes: u32::try_from(meta.log_bytes).unwrap_or(u32::MAX),
                expires_at_ms: meta.deadline_ms,
                peers,
                limits,
            },
        );
        match self.build_catchup(last_seq, true) {
            Ok(catchup) => out.send(conn, &catchup),
            Err(_) => {
                self.refuse(conn, now_ms, Self::unavailable(), true, out);
                return;
            }
        }
        self.broadcast(out, Some(conn), &ServerMessage::PeerJoined { client_id, max_presence_per_sec: rate });
        let was_expiring = self.life.visited();
        if meta.deadline_ms != 0 || was_expiring {
            let retention = ServerMessage::Retention { expires_at_ms: meta.deadline_ms, expiring: false };
            self.broadcast(out, Some(conn), &retention);
        }
        if meta.log_records > self.cfg.compact.join_records {
            self.advise(conn, now_ms, out);
        }
        self.run_retention(now_ms, out);
    }
}
