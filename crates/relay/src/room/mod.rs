//! The room core: [`Relay`]. See the crate docs for the shape of the interface.
//!
//! The module is split by concern. `mod.rs` holds the state, the entry points and the connection table; `join`
//! handles `Join`; `log` handles records, snapshots, catch-up and compaction advice; `presence` bundles presence;
//! `retention` applies the deadline policy; `image` serves the image routes.

mod conn;
mod image;
mod join;
mod log;
mod presence;
mod retention;

use std::collections::BTreeMap;
use std::sync::Arc;

use drafftink_protocol::v2::{ClientMessage, ErrorCode, RoomId, ServerMessage};
use drafftink_protocol::{ClientId, DecodeError, KEEPALIVE_PING, KEEPALIVE_PONG};

use self::conn::{Conn, Member, Phase};
use self::log::{Advice, Log};
use self::presence::Presence;
use crate::config::RelayConfig;
use crate::lifecycle::Lifecycle;
use crate::rate::TokenBucket;
use crate::store::{BlobStore, RelayStore, RoomMeta, StoreError};

/// Names one connection of a host. The host picks the numbers; the core only compares them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(pub u64);

/// An encoded frame, shared between the connections it goes to.
pub type Frame = Arc<[u8]>;

/// Something the host must do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Send this binary frame.
    Send {
        /// The connection.
        conn: ConnId,
        /// The encoded message.
        frame: Frame,
    },
    /// Send this text frame (the keepalive `pong`).
    Text {
        /// The connection.
        conn: ConnId,
        /// The text.
        text: &'static str,
    },
    /// Close the connection after the frames sent before.
    Close {
        /// The connection.
        conn: ConnId,
    },
}

/// A write the core made to its stores, for cost accounting (a Durable Object bills rows written).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOp {
    /// The room was created, or its deadline moved.
    SaveMeta,
    /// One segment of records was written.
    AppendSegment {
        /// Records in the segment.
        records: u32,
        /// Their sealed bytes.
        bytes: u64,
    },
    /// A snapshot was swapped in and the segments below it deleted.
    PutSnapshot {
        /// The sequence number it covers.
        covers_seq: u64,
        /// Its sealed bytes.
        bytes: u64,
    },
    /// The room, its records and its images were deleted.
    DeleteRoom,
    /// An image was stored.
    PutImage {
        /// Its sealed bytes.
        bytes: u64,
    },
}

/// What a call to the core produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Output {
    /// What to do, in order.
    pub effects: Vec<Effect>,
    /// What the core wrote to its stores during the call.
    pub store_ops: Vec<StoreOp>,
    /// When to call [`Relay::on_wake`] if nothing else happens before. `None`: no timer is needed. A time in the
    /// past means "now".
    pub wake_at: Option<u64>,
    /// The room was deleted during the call. The host forgets it.
    pub deleted: bool,
}

impl Output {
    fn send(&mut self, conn: ConnId, message: &ServerMessage) {
        if let Some(frame) = encode(message) {
            self.send_frame(conn, frame);
        }
    }

    fn send_frame(&mut self, conn: ConnId, frame: Frame) {
        self.effects.push(Effect::Send { conn, frame });
    }

    fn close(&mut self, conn: ConnId) {
        self.effects.push(Effect::Close { conn });
    }
}

fn encode(message: &ServerMessage) -> Option<Frame> {
    message.encode().ok().map(Frame::from)
}

fn retry_error(code: ErrorCode, text: &str, retry_after_ms: u32) -> ServerMessage {
    let mut message = ServerMessage::error(code, text);
    if let ServerMessage::Error { retry_after_ms: slot, .. } = &mut message {
        *slot = Some(retry_after_ms);
    }
    message
}

/// What the core knows about the stored room.
#[derive(Debug, Clone, Copy)]
enum MetaState {
    /// Not read from the store yet.
    Unloaded,
    /// The store has no such room.
    Missing,
    /// The room exists.
    Loaded(RoomMeta),
}

/// The core of one sealed room. See the crate docs.
pub struct Relay<S, B> {
    room: RoomId,
    cfg: RelayConfig,
    store: S,
    blobs: B,
    meta: MetaState,
    life: Lifecycle,
    retention_wake: Option<u64>,
    /// A deletion that failed is not tried again before this time.
    delete_retry_ms: Option<u64>,
    conns: BTreeMap<ConnId, Conn>,
    next_order: u64,
    log: Log,
    presence: Presence,
    room_updates: TokenBucket,
    image_puts: TokenBucket,
    advice: Option<Advice>,
}

impl<S: RelayStore, B: BlobStore> Relay<S, B> {
    /// The core of the room `room`, with its stores.
    pub fn new(room: RoomId, cfg: RelayConfig, store: S, blobs: B) -> Self {
        let life = Lifecycle::new(cfg.retention);
        let room_updates =
            TokenBucket::new(u32::from(cfg.max_room_updates_per_sec), u32::from(cfg.max_room_updates_per_sec) * 2, 0);
        let image_puts =
            TokenBucket::new(u32::from(cfg.max_image_puts_per_sec), u32::from(cfg.max_image_puts_per_sec) * 2, 0);
        Relay {
            room,
            cfg,
            store,
            blobs,
            meta: MetaState::Unloaded,
            life,
            retention_wake: None,
            delete_retry_ms: None,
            conns: BTreeMap::new(),
            next_order: 0,
            log: Log::default(),
            presence: Presence::default(),
            room_updates,
            image_puts,
            advice: None,
        }
    }

    /// The room this core serves.
    pub fn room_id(&self) -> RoomId {
        self.room
    }

    /// The policy.
    pub fn config(&self) -> &RelayConfig {
        &self.cfg
    }

    /// How many connections the core knows, joined or not.
    pub fn connection_count(&self) -> usize {
        self.conns.len()
    }

    /// How many people have joined.
    pub fn member_count(&self) -> usize {
        self.members().count()
    }

    /// The store, for the host and for tests.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The image store, for the host and for tests.
    pub fn blobs(&self) -> &B {
        &self.blobs
    }

    /// A new connection. It has [`RelayConfig::join_timeout_ms`] to send `Join`.
    pub fn connect(&mut self, conn: ConnId, now_ms: u64) -> Output {
        let join_by_ms = now_ms.saturating_add(self.cfg.join_timeout_ms);
        self.conns.insert(conn, Conn::new(now_ms, join_by_ms));
        self.finish(now_ms, Output::default())
    }

    /// A binary frame arrived. Every byte of it is hostile: a frame that does not decode closes the connection
    /// with an error, and nothing panics.
    pub fn on_frame(&mut self, conn: ConnId, now_ms: u64, frame: &[u8]) -> Output {
        let mut out = Output::default();
        let Some(entry) = self.conns.get_mut(&conn) else { return self.finish(now_ms, out) };
        entry.last_heard_ms = now_ms;
        if frame.len() > self.cfg.limits.max_frame_bytes as usize {
            self.refuse(
                conn,
                now_ms,
                ServerMessage::error(ErrorCode::TooLarge, "the message is larger than this relay accepts"),
                true,
                &mut out,
            );
            return self.finish(now_ms, out);
        }
        match ClientMessage::decode(frame) {
            Ok(message) => self.on_message(conn, now_ms, message, &mut out),
            Err(e) => {
                let code = match e {
                    DecodeError::UnsupportedVersion { .. } | DecodeError::WrongLayout { .. } => {
                        ErrorCode::UnsupportedVersion
                    }
                    DecodeError::FrameTooLarge { .. } | DecodeError::FieldTooLong { .. } => ErrorCode::TooLarge,
                    _ => ErrorCode::Malformed,
                };
                let text = if code == ErrorCode::UnsupportedVersion {
                    "This relay only hosts end-to-end encrypted rooms (protocol version 2). Use a room link that has a key."
                } else {
                    "the message could not be read"
                };
                self.refuse(conn, now_ms, ServerMessage::error(code, text), true, &mut out);
            }
        }
        self.finish(now_ms, out)
    }

    /// A text frame arrived. `ping` is answered with `pong`; anything else is refused.
    pub fn on_text(&mut self, conn: ConnId, now_ms: u64, text: &str) -> Output {
        let mut out = Output::default();
        let Some(entry) = self.conns.get_mut(&conn) else { return self.finish(now_ms, out) };
        entry.last_heard_ms = now_ms;
        if text == KEEPALIVE_PING {
            out.effects.push(Effect::Text { conn, text: KEEPALIVE_PONG });
        } else {
            let message =
                ServerMessage::error(ErrorCode::Malformed, "text frames are not accepted; send binary frames");
            self.refuse(conn, now_ms, message, true, &mut out);
        }
        self.finish(now_ms, out)
    }

    /// The connection closed, cleanly or not.
    pub fn on_closed(&mut self, conn: ConnId, now_ms: u64) -> Output {
        let mut out = Output::default();
        self.remove_conn(conn, now_ms, &mut out);
        self.run_retention(now_ms, &mut out);
        self.finish(now_ms, out)
    }

    /// The host is stopping: write what is buffered, tell everybody to come back later and close every connection.
    /// The room stays as it is on disk.
    pub fn shutdown(&mut self, now_ms: u64) -> Output {
        let mut out = Output::default();
        self.flush(now_ms, &mut out);
        let open: Vec<ConnId> = self.conns.keys().copied().collect();
        for conn in open {
            let text = "The relay is shutting down. Trying again soon.";
            out.send(conn, &retry_error(ErrorCode::Unavailable, text, 3000));
            out.close(conn);
        }
        self.conns.clear();
        self.finish(now_ms, out)
    }

    /// The time in `wake_at` of an earlier [`Output`] has come. Calling it early or late does no harm.
    pub fn on_wake(&mut self, now_ms: u64) -> Output {
        let mut out = Output::default();
        self.expire_joins(now_ms, &mut out);
        self.flush_if_due(now_ms, &mut out);
        self.bundle_presence(now_ms, &mut out);
        self.advise_if_due(now_ms, &mut out);
        self.run_retention(now_ms, &mut out);
        self.finish(now_ms, out)
    }

    // ---- dispatch ---------------------------------------------------------------------------------------------

    fn on_message(&mut self, conn: ConnId, now_ms: u64, message: ClientMessage, out: &mut Output) {
        let joined = self.conns.get(&conn).is_some_and(|c| matches!(c.phase, Phase::Member(_)));
        if !joined {
            match message {
                ClientMessage::Join { .. } => self.join(conn, now_ms, message, out),
                _ => self.refuse(
                    conn,
                    now_ms,
                    ServerMessage::error(ErrorCode::NotJoined, "the first message must be Join"),
                    true,
                    out,
                ),
            }
            return;
        }
        match message {
            ClientMessage::Join { .. } => {
                let text = "this connection already joined a room";
                self.refuse(conn, now_ms, ServerMessage::error(ErrorCode::AlreadyJoined, text), true, out);
            }
            ClientMessage::Append { record } => self.append(conn, now_ms, record, out),
            ClientMessage::PutSnapshot { expected_snapshot_seq, record } => {
                self.put_snapshot(conn, now_ms, expected_snapshot_seq, record, out);
            }
            ClientMessage::Presence { record } => self.presence_in(conn, now_ms, &record),
            ClientMessage::Catchup { after_seq } => self.catchup_request(conn, now_ms, after_seq, out),
            ClientMessage::Direct { to, record } => self.direct(conn, now_ms, to, &record, out),
            ClientMessage::DeleteRoom => self.delete_requested(conn, now_ms, out),
            ClientMessage::Leave => {
                self.remove_conn(conn, now_ms, out);
                out.close(conn);
                self.run_retention(now_ms, out);
            }
        }
    }

    // ---- connections ------------------------------------------------------------------------------------------

    fn members(&self) -> impl Iterator<Item = (ConnId, &Member)> {
        self.conns.iter().filter_map(|(id, c)| match &c.phase {
            Phase::Member(m) => Some((*id, m)),
            Phase::Waiting { .. } => None,
        })
    }

    fn member_mut(&mut self, conn: ConnId) -> Option<&mut Member> {
        match &mut self.conns.get_mut(&conn)?.phase {
            Phase::Member(m) => Some(m),
            Phase::Waiting { .. } => None,
        }
    }

    /// Counts a rate-limit breach of `conn`. True when the connection has had enough.
    fn breach(&mut self, conn: ConnId, now_ms: u64) -> bool {
        let max_breaches = self.cfg.max_breaches;
        self.member_mut(conn).is_some_and(|m| m.breach(now_ms, max_breaches))
    }

    fn client_of(&self, conn: ConnId) -> Option<ClientId> {
        self.members().find(|(id, _)| *id == conn).map(|(_, m)| m.client_id)
    }

    /// Connections heard from within the stale limit. A socket that died without a close event does not count.
    fn live_count(&self, now_ms: u64) -> usize {
        self.conns.values().filter(|c| now_ms.saturating_sub(c.last_heard_ms) <= self.cfg.stale_after_ms).count()
    }

    /// Sends `message` and, when `close` or the code says so, ends the connection.
    fn refuse(&mut self, conn: ConnId, now_ms: u64, message: ServerMessage, close: bool, out: &mut Output) {
        out.send(conn, &message);
        if close {
            self.remove_conn(conn, now_ms, out);
            out.close(conn);
            self.run_retention(now_ms, out);
        }
    }

    /// Forgets a connection. Others hear that the person left.
    fn remove_conn(&mut self, conn: ConnId, now_ms: u64, out: &mut Output) {
        let Some(entry) = self.conns.remove(&conn) else { return };
        self.log.forget_acks(conn);
        if self.advice.as_ref().is_some_and(|a| a.conn == conn) {
            self.advice = None;
        }
        if let Phase::Member(member) = entry.phase {
            self.broadcast(out, None, &ServerMessage::PeerLeft { client_id: member.client_id });
            self.share_presence_rate();
        }
        let live = self.live_count(now_ms);
        self.life.connection_closed(now_ms, live);
    }

    /// Encodes `message` once and sends it to every member except `except`.
    fn broadcast(&self, out: &mut Output, except: Option<ConnId>, message: &ServerMessage) {
        let Some(frame) = encode(message) else { return };
        for (id, _) in self.members().filter(|(id, _)| Some(*id) != except) {
            out.send_frame(id, frame.clone());
        }
    }

    fn expire_joins(&mut self, now_ms: u64, out: &mut Output) {
        let late: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(c.phase, Phase::Waiting { join_by_ms } if join_by_ms <= now_ms))
            .map(|(id, _)| *id)
            .collect();
        for conn in late {
            self.refuse(conn, now_ms, ServerMessage::error(ErrorCode::NotJoined, "no Join arrived in time"), true, out);
        }
    }

    // ---- store access -----------------------------------------------------------------------------------------

    /// The room's meta, read from the store on first use. `Ok(None)`: the room does not exist.
    fn load_meta(&mut self) -> Result<Option<RoomMeta>, StoreError> {
        match self.meta {
            MetaState::Loaded(meta) => Ok(Some(meta)),
            MetaState::Missing => Ok(None),
            MetaState::Unloaded => {
                let loaded = self.store.load_meta()?;
                self.meta = loaded.map_or(MetaState::Missing, MetaState::Loaded);
                Ok(loaded)
            }
        }
    }

    fn meta_now(&self) -> Option<RoomMeta> {
        match self.meta {
            MetaState::Loaded(meta) => Some(meta),
            MetaState::Unloaded | MetaState::Missing => None,
        }
    }

    /// The error a client sees when the store fails.
    fn unavailable() -> ServerMessage {
        retry_error(
            ErrorCode::Unavailable,
            "The relay cannot read or write this room right now. Trying again soon.",
            1000,
        )
    }

    // ---- time -------------------------------------------------------------------------------------------------

    fn finish(&self, now_ms: u64, mut out: Output) -> Output {
        out.wake_at = self.next_wake(now_ms);
        out
    }

    fn next_wake(&self, _now_ms: u64) -> Option<u64> {
        let joins = self.conns.values().filter_map(|c| match c.phase {
            Phase::Waiting { join_by_ms } => Some(join_by_ms),
            Phase::Member(_) => None,
        });
        [
            joins.min(),
            self.log.next_flush(&self.cfg.flush),
            self.presence.next_bundle(),
            self.advice.as_ref().map(|a| a.sent_ms.saturating_add(self.cfg.compact.advice_timeout_ms)),
            self.retention_wake,
        ]
        .into_iter()
        .flatten()
        .min()
    }
}
