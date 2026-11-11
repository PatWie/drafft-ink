//! The sync state machine. Sans-IO: it owns no socket, no clock and no document.
//!
//! The owner (the engine, through [`crate::CollabSession`]) feeds it three kinds of input and reads two kinds of
//! output:
//!
//! * inputs: [`CollabClient::on_event`] (what the transport reported), [`CollabClient::tick`] (time passed), and the
//!   local calls ([`CollabClient::local_changed`], the presence setters, [`CollabClient::leave`]);
//! * outputs: [`CollabClient::poll_output`] (connect, send a frame, close) and [`CollabClient::take_events`] (what
//!   changed for the UI and the engine), plus [`CollabClient::next_wake_ms`] (when the owner must call `tick` again).
//!
//! The document is passed into the calls that touch it. The client never reads the scene and does not know about
//! commands: remote edits come back as a [`ChangeSet`](drafftink_scene::ChangeSet) in [`CollabEvent::Remote`], and
//! the engine decides what they mean.
//!
//! Sync follows `docs/protocol.md`. The client joins with the document's version vector and imports what the
//! server answers. It then uploads whatever the server's version lacks, which is the only repair needed after an
//! outage, a dropped update or a rate limit. Live edits go out in small coalesced updates and stay counted as
//! unsynced until the server acknowledges them.

use std::collections::VecDeque;
use std::fmt;

use drafftink_doc::{DocError, LoroStore};
use drafftink_protocol::{
    AwarenessState, ClientId, ClientMessage, ErrorCode, IDLE_TIMEOUT_MS, Limits, MAX_AUTH_TOKEN_BYTES,
    MAX_UPDATE_BYTES, MIN_PROTOCOL_VERSION, PING_INTERVAL_MS, PROTOCOL_VERSION, PeerInfo, Point, RoomId, ServerMessage,
    SyncKind, UserInfo, ValidationError, Viewport, reconnect_delay_ms,
};
use drafftink_scene::{ChangeKind, ChangeSet};
use loro::VersionVector;

mod parts;

use self::parts::{Health, Reconcile, Uploads};
use crate::awareness::{LocalPresence, Peer, PeerTable};
use crate::status::{ConnState, Problem, ProblemAction, Status, StopReason, explain, stop_reason};
use crate::transport::TransportEvent;
use crate::url::ServerUrl;

/// Local edits are coalesced for this long before they go out, in milliseconds.
pub const SEND_DEBOUNCE_MS: u64 = 30;
/// Most updates sent and not yet acknowledged. More edits wait in the document and leave as one update.
pub const MAX_INFLIGHT: usize = 32;
/// An update with no acknowledgement for this long makes the client reconcile with the server, in milliseconds.
pub const ACK_TIMEOUT_MS: u64 = 20_000;
/// The connection must open within this time, in milliseconds.
pub const CONNECT_TIMEOUT_MS: u64 = 15_000;
/// The server must answer the join and the first sync within this time, in milliseconds.
pub const JOIN_TIMEOUT_MS: u64 = 15_000;
/// A connection that stayed online this long resets the reconnect backoff, in milliseconds.
pub const STABLE_MS: u64 = 10_000;
/// The least gap between two sync requests, in milliseconds. It doubles while requests keep failing.
pub const RESYNC_MIN_GAP_MS: u64 = 1_000;
/// The longest gap between two sync requests, in milliseconds.
pub const RESYNC_MAX_GAP_MS: u64 = 30_000;
/// A sync request with no answer for this long may be asked again, in milliseconds.
pub const SYNC_REPLY_TIMEOUT_MS: u64 = 30_000;

/// A secret the server asked for. It is never shown: `Debug` prints a placeholder, there is no `Display`, and the
/// client logs no message that holds it.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthToken(String);

impl AuthToken {
    /// Wraps a token. Empty tokens and tokens over the protocol limit are refused.
    pub fn new(token: impl Into<String>) -> Result<Self, ConfigError> {
        let token = token.into();
        if token.is_empty() || token.len() > MAX_AUTH_TOKEN_BYTES {
            return Err(ConfigError::Token);
        }
        Ok(AuthToken(token))
    }
}

impl fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthToken(<hidden>)")
    }
}

/// Why a client could not be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The access token is empty or too long.
    #[error("The access token must be between 1 and {MAX_AUTH_TOKEN_BYTES} bytes")]
    Token,
    /// The display name or colour breaks a protocol rule.
    #[error("The display name or colour is not valid: {0}")]
    User(#[from] ValidationError),
}

/// What the client needs to join a room.
#[derive(Debug, Clone)]
pub struct CollabConfig {
    /// The server. Checked by [`ServerUrl::parse`].
    pub url: ServerUrl,
    /// The room.
    pub room: RoomId,
    /// This client's stable id. Keep it across reconnects.
    pub client_id: ClientId,
    /// Name and colour shown to the others.
    pub user: Option<UserInfo>,
    /// The access token, when the server asks for one.
    pub auth_token: Option<AuthToken>,
    /// Any random number. It spreads the reconnect times of clients that lost the server together.
    pub jitter_seed: u64,
    /// When the server offers a snapshot that cannot merge into this document (its history was trimmed), replace
    /// the local document without asking. Edits that were never sent are lost. Off by default: the client then
    /// raises [`CollabEvent::NeedsDecision`] and waits. A headless client may turn it on.
    pub replace_when_trimmed: bool,
}

/// What the owner must do for the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// Open a connection to this address.
    Connect(ServerUrl),
    /// Send this frame.
    Send(Vec<u8>),
    /// Close the connection after the frames queued before this.
    Close,
}

/// Where a remote change came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteKind {
    /// A live edit relayed from another peer.
    Live,
    /// Updates the server sent in answer to a join or a sync request.
    CatchUp,
    /// A snapshot the server sent in answer to a join or a sync request, merged into the document.
    Snapshot,
    /// The local document was replaced by the server's snapshot. Selection and gestures on removed elements are
    /// stale; local undo history is gone.
    Replaced,
}

/// A batch of remote changes already applied to the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBatch {
    /// The sender of a live edit.
    pub from: Option<ClientId>,
    /// The sender's display name, when known.
    pub who: Option<String>,
    /// What kind of batch this is.
    pub kind: RemoteKind,
    /// What changed. The engine marks it as `origin = remote`.
    pub changes: ChangeSet,
}

/// How many elements a batch added, updated and removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChangeCounts {
    /// Elements that are new.
    pub added: usize,
    /// Elements that changed.
    pub updated: usize,
    /// Elements that are gone.
    pub removed: usize,
}

impl RemoteBatch {
    /// Elements added, updated and removed.
    pub fn counts(&self) -> ChangeCounts {
        let n = |k: ChangeKind| self.changes.elements.iter().filter(|c| c.kind == k).count();
        ChangeCounts { added: n(ChangeKind::Added), updated: n(ChangeKind::Updated), removed: n(ChangeKind::Removed) }
    }
}

/// How the user resolves [`CollabEvent::NeedsDecision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimmedChoice {
    /// Take the server's copy. Local edits that were never sent are lost.
    ReplaceWithServer,
    /// Keep the local copy and stay offline.
    KeepMine,
}

/// Why following a peer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowEnd {
    /// The peer left the room.
    PeerLeft,
    /// The connection to the server was lost.
    Disconnected,
}

/// Something the owner should react to.
#[derive(Debug, Clone, PartialEq)]
pub enum CollabEvent {
    /// The connection status changed.
    Status(Status),
    /// A failure to show once, in plain words (the status carries standing problems too).
    Problem(Problem),
    /// Remote changes were applied to the document.
    Remote(RemoteBatch),
    /// Somebody joined.
    PeerJoined {
        /// Their id.
        id: ClientId,
        /// Their display name.
        name: String,
    },
    /// Somebody left.
    PeerLeft {
        /// Their id.
        id: ClientId,
        /// Their display name.
        name: String,
    },
    /// A peer's cursor, selection or viewport changed, or a peer list changed. Read [`CollabClient::peers`].
    Presence,
    /// The followed peer moved its viewport.
    FollowViewport(Viewport),
    /// Following ended without the user asking.
    FollowEnded(FollowEnd),
    /// The server's snapshot cannot merge into this document. Resolve with [`CollabClient::resolve_trimmed`]. Sync
    /// waits for the answer.
    NeedsDecision {
        /// The server's snapshot is waiting; replacing loses edits that were never sent.
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Backoff { until: u64 },
    Connecting { since: u64 },
    Joining { since: u64 },
    Syncing { since: u64 },
    Online,
    Stopped(StopReason),
}

/// Counters for diagnostics and tests. They only grow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Connections opened.
    pub connects: u32,
    /// Joins completed (first sync done).
    pub joins: u32,
    /// Times the connection was lost or dropped and a new one scheduled.
    pub reconnects: u32,
    /// `Update` messages sent.
    pub updates_sent: u32,
    /// `Ack` messages received.
    pub acks: u32,
    /// Remote batches applied to the document.
    pub remote_batches: u32,
    /// `ResyncRequired` errors received.
    pub resync_required: u32,
    /// `RequestSync` messages sent.
    pub sync_requests: u32,
}

#[derive(Debug, Clone, Copy)]
struct Inflight {
    seq: u32,
    sent_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Oversize,
    Rejected,
}

#[derive(Debug)]
struct PendingSnapshot {
    bytes: Vec<u8>,
    server_version: Vec<u8>,
    joining: bool,
}

/// The collaboration client state machine. See the module docs.
#[derive(Debug)]
pub struct CollabClient {
    cfg: CollabConfig,
    phase: Phase,
    status: Status,
    out: VecDeque<Output>,
    events: Vec<CollabEvent>,
    presence: LocalPresence,
    peers: PeerTable,
    following: Option<ClientId>,
    presence_changed: bool,
    limits: Limits,

    health: Health,
    uploads: Uploads,
    reconcile: Reconcile,
    problem: Option<Problem>,
    stats: Stats,
}

impl CollabClient {
    /// Makes a client. It does nothing until [`start`](Self::start).
    pub fn new(cfg: CollabConfig) -> Result<Self, ConfigError> {
        if let Some(u) = &cfg.user {
            u.validate().map_err(ConfigError::User)?;
        }
        let presence = LocalPresence::new(cfg.user.clone());
        let rng = cfg.jitter_seed | 1;
        Ok(CollabClient {
            cfg,
            phase: Phase::Idle,
            status: Status::idle(),
            out: VecDeque::new(),
            events: Vec::new(),
            presence,
            peers: PeerTable::default(),
            following: None,
            presence_changed: false,
            limits: Limits::DEFAULT,
            health: Health::new(rng),
            uploads: Uploads::new(),
            reconcile: Reconcile::new(),
            problem: None,
            stats: Stats::default(),
        })
    }

    /// Counters of what happened so far.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    // ---- reading state ----------------------------------------------------------------------------------------

    /// The current status.
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// The other people in the room.
    pub fn peers(&self) -> &PeerTable {
        &self.peers
    }

    /// This client's own presence.
    pub fn presence(&self) -> &LocalPresence {
        &self.presence
    }

    /// The configuration.
    pub fn config(&self) -> &CollabConfig {
        &self.cfg
    }

    /// The peer being followed, if any.
    pub fn following(&self) -> Option<ClientId> {
        self.following
    }

    /// True while connected and joined (online or still catching up).
    pub fn is_connected(&self) -> bool {
        matches!(self.phase, Phase::Syncing { .. } | Phase::Online)
    }

    /// The next frame to send, connect or close request.
    pub fn poll_output(&mut self) -> Option<Output> {
        self.out.pop_front()
    }

    /// Everything that happened since the last call, in order.
    pub fn take_events(&mut self) -> Vec<CollabEvent> {
        std::mem::take(&mut self.events)
    }

    /// When the owner must call [`tick`](Self::tick) next. `None` means nothing is scheduled: the next input will
    /// come from the transport or from the owner. A time in the past means "now".
    pub fn next_wake_ms(&self) -> Option<u64> {
        let mut wake: Option<u64> = None;
        let mut at = |t: u64| wake = Some(wake.map_or(t, |w| w.min(t)));
        if !self.out.is_empty() {
            at(0);
        }
        match self.phase {
            Phase::Idle | Phase::Stopped(_) => {}
            Phase::Backoff { until } => at(until),
            Phase::Connecting { since } => at(since.saturating_add(CONNECT_TIMEOUT_MS)),
            Phase::Joining { since } | Phase::Syncing { since } => {
                if self.reconcile.pending_snapshot.is_none() {
                    at(since.saturating_add(JOIN_TIMEOUT_MS));
                }
            }
            Phase::Online => {}
        }
        if self.is_connected() || matches!(self.phase, Phase::Joining { .. }) {
            at(self.health.last_rx_ms.max(self.health.last_ping_ms).saturating_add(u64::from(PING_INTERVAL_MS)));
            at(self.health.last_rx_ms.saturating_add(u64::from(IDLE_TIMEOUT_MS)));
        }
        if matches!(self.phase, Phase::Online) {
            if let Some(t) = self.uploads.flush_at.filter(|_| self.uploads.blocked.is_none()) {
                at(t.max(self.uploads.send_not_before));
            }
            if let Some(t) = self.reconcile.resync_due {
                at(t);
            }
            if let Some(f) = self.uploads.inflight.front() {
                at(f.sent_at.saturating_add(ACK_TIMEOUT_MS));
            }
            if let Some(t) =
                self.presence.next_due(self.health.last_tick_ms, self.awareness_interval(), !self.peers.is_empty())
            {
                at(t);
            }
            if let Some(t) = self.health.stable_since.filter(|_| self.health.attempt > 0) {
                at(t.saturating_add(STABLE_MS));
            }
        }
        if let Some(t) = self.peers.next_expiry() {
            at(t);
        }
        if let Some(t) = self.peers.next_trail_expiry(self.health.last_tick_ms) {
            at(t);
        }
        wake
    }

    // ---- lifecycle --------------------------------------------------------------------------------------------

    /// Starts connecting, or connects again after a stop (for example with a new token in a new client). The
    /// first attempt happens at the next [`tick`](Self::tick).
    pub fn start(&mut self, now_ms: u64) {
        if matches!(self.phase, Phase::Idle | Phase::Stopped(_)) {
            self.problem = None;
            self.health.attempt = 0;
            self.phase = Phase::Backoff { until: now_ms };
            self.health.offline_reason = String::new();
            self.refresh_status();
        }
    }

    /// Tries again at once instead of waiting out the backoff.
    pub fn retry_now(&mut self, now_ms: u64) {
        if let Phase::Backoff { .. } = self.phase {
            self.phase = Phase::Backoff { until: now_ms };
        }
    }

    /// Leaves the room: sends the edits still waiting, says goodbye, closes. No reconnect follows.
    pub fn leave(&mut self, now_ms: u64, store: &mut LoroStore) {
        if matches!(self.phase, Phase::Online) && self.uploads.blocked.is_none() {
            self.flush_updates(now_ms, store);
        }
        if self.is_connected() {
            self.send(&ClientMessage::Leave);
        }
        if !matches!(self.phase, Phase::Idle | Phase::Stopped(_)) {
            if !matches!(self.phase, Phase::Backoff { .. }) {
                self.out.push_back(Output::Close);
            }
            self.enter_stopped(StopReason::Left, None);
        }
    }

    // ---- local inputs -----------------------------------------------------------------------------------------

    /// The document got a local commit. Call it after every commit; it is cheap. The update goes out within
    /// [`SEND_DEBOUNCE_MS`] while online, and after the next sync otherwise.
    pub fn local_changed(&mut self, now_ms: u64) {
        self.uploads.local_dirty = true;
        if matches!(self.phase, Phase::Online) && self.uploads.flush_at.is_none() && self.uploads.blocked.is_none() {
            let gap = self.update_interval_ms();
            let earliest = self.uploads.last_update_sent_ms.map_or(0, |t| t.saturating_add(gap));
            self.uploads.flush_at = Some(now_ms.saturating_add(SEND_DEBOUNCE_MS).max(earliest));
        }
        self.refresh_status();
    }

    /// Sets the pointer position in world units; `None` hides it.
    pub fn set_cursor(&mut self, now_ms: u64, cursor: Option<Point>) {
        self.presence.set_cursor(now_ms, cursor);
    }

    /// Sets the selected element ids.
    pub fn set_selection(&mut self, ids: impl IntoIterator<Item = String>) {
        self.presence.set_selection(ids);
    }

    /// Sets the visible world rectangle.
    pub fn set_viewport(&mut self, viewport: Option<Viewport>) {
        self.presence.set_viewport(viewport);
    }

    /// Sets name and colour.
    pub fn set_user(&mut self, user: Option<UserInfo>) {
        self.presence.set_user(user);
    }

    /// Turns the local laser pointer on or off.
    pub fn set_laser(&mut self, on: bool) {
        self.presence.set_laser(on);
    }

    /// Follows a peer: the client reports their viewport as it changes. Returns false for an unknown peer. The
    /// current viewport is reported at once when the peer has one.
    pub fn follow(&mut self, id: ClientId) -> bool {
        let Some(peer) = self.peers.get(&id) else { return false };
        self.following = Some(id);
        if let Some(v) = peer.viewport {
            self.events.push(CollabEvent::FollowViewport(v));
        }
        true
    }

    /// Stops following (the user moved the view themselves, or pressed stop).
    pub fn stop_following(&mut self) {
        self.following = None;
    }

    /// Answers [`CollabEvent::NeedsDecision`].
    pub fn resolve_trimmed(&mut self, now_ms: u64, store: &mut LoroStore, choice: TrimmedChoice) {
        let Some(pending) = self.reconcile.pending_snapshot.take() else { return };
        match choice {
            TrimmedChoice::KeepMine => {
                self.out.push_back(Output::Close);
                self.enter_stopped(StopReason::Declined, None);
            }
            TrimmedChoice::ReplaceWithServer => match store.replace_from_snapshot(&pending.bytes) {
                Ok(cs) => {
                    self.push_remote(None, RemoteKind::Replaced, cs);
                    self.problem = None;
                    self.after_sync(now_ms, store, pending.joining, &pending.server_version);
                }
                Err(e) => self.doc_failure(now_ms, &e, pending.joining),
            },
        }
        self.flush_events();
    }

    // ---- time -------------------------------------------------------------------------------------------------

    /// Advances time: starts connections, times out silent ones, sends due updates, requests and presence.
    pub fn tick(&mut self, now_ms: u64, store: &mut LoroStore) {
        match self.phase {
            Phase::Backoff { until } if now_ms >= until => {
                self.phase = Phase::Connecting { since: now_ms };
                self.stats.connects += 1;
                self.out.push_back(Output::Connect(self.cfg.url.clone()));
            }
            Phase::Connecting { since } if now_ms.saturating_sub(since) >= CONNECT_TIMEOUT_MS => {
                self.fail_connection(now_ms, "The server did not answer in time");
            }
            Phase::Joining { since } | Phase::Syncing { since }
                if self.reconcile.pending_snapshot.is_none() && now_ms.saturating_sub(since) >= JOIN_TIMEOUT_MS =>
            {
                self.fail_connection(now_ms, "The server did not finish the join in time");
            }
            _ => {}
        }
        if self.is_connected() || matches!(self.phase, Phase::Joining { .. }) {
            if now_ms.saturating_sub(self.health.last_rx_ms) >= u64::from(IDLE_TIMEOUT_MS) {
                self.fail_connection(now_ms, "The connection went quiet");
            } else if now_ms.saturating_sub(self.health.last_rx_ms.max(self.health.last_ping_ms))
                >= u64::from(PING_INTERVAL_MS)
            {
                self.health.ping_nonce = self.health.ping_nonce.wrapping_add(1);
                self.health.last_ping_ms = now_ms;
                self.send(&ClientMessage::Ping { nonce: self.health.ping_nonce });
            }
        }
        if matches!(self.phase, Phase::Online) {
            self.tick_online(now_ms, store);
        }
        self.expire_peers(now_ms);
        self.health.last_tick_ms = now_ms;
        self.flush_events();
    }

    fn tick_online(&mut self, now_ms: u64, store: &mut LoroStore) {
        if self.health.stable_since.is_some_and(|t| now_ms.saturating_sub(t) >= STABLE_MS) {
            self.health.attempt = 0;
        }
        let waiting = self.uploads.inflight.len();
        self.uploads.inflight.retain(|f| now_ms.saturating_sub(f.sent_at) < ACK_TIMEOUT_MS);
        if self.uploads.inflight.len() < waiting {
            // Updates with no acknowledgement: the server may have dropped them. Reconcile instead of waiting.
            self.want_resync(now_ms);
        }
        if self.reconcile.resync_due.is_some_and(|t| t <= now_ms) {
            self.send_resync(now_ms, store);
        }
        if self.uploads.blocked.is_none() && now_ms >= self.uploads.send_not_before {
            let due = self.uploads.flush_at.is_some_and(|t| t <= now_ms)
                || (self.uploads.want_flush && self.uploads.inflight.len() < MAX_INFLIGHT);
            if due {
                self.flush_updates(now_ms, store);
            }
        }
        let interval = self.awareness_interval();
        if let Some(state) = self.presence.due(now_ms, interval, !self.peers.is_empty()).cloned() {
            self.send(&ClientMessage::Awareness(state));
            self.presence.mark_sent(now_ms);
        }
    }

    // ---- transport input --------------------------------------------------------------------------------------

    /// Handles what the transport reported.
    pub fn on_event(&mut self, now_ms: u64, event: TransportEvent, store: &mut LoroStore) {
        match event {
            TransportEvent::Opened => self.on_opened(now_ms, store),
            TransportEvent::Message(bytes) => self.on_frame(now_ms, &bytes, store),
            TransportEvent::Closed { error } => self.on_closed(now_ms, error),
        }
        self.flush_events();
    }

    fn on_opened(&mut self, now_ms: u64, store: &LoroStore) {
        if !matches!(self.phase, Phase::Connecting { .. }) {
            return;
        }
        self.health.last_rx_ms = now_ms;
        self.health.last_ping_ms = now_ms;
        let version = match store.version_vector() {
            Ok(v) => v,
            Err(e) => {
                self.doc_failure(now_ms, &e, true);
                return;
            }
        };
        self.phase = Phase::Joining { since: now_ms };
        let join = ClientMessage::Join {
            min_version: MIN_PROTOCOL_VERSION,
            max_version: PROTOCOL_VERSION,
            room: self.cfg.room.clone(),
            client_id: self.cfg.client_id,
            auth_token: self.cfg.auth_token.as_ref().map(|t| t.0.clone()),
            version,
            user: self.presence.state().user.clone(),
        };
        self.send(&join);
        self.refresh_status();
    }

    fn on_closed(&mut self, now_ms: u64, error: Option<String>) {
        if matches!(self.phase, Phase::Idle | Phase::Stopped(_) | Phase::Backoff { .. }) {
            return;
        }
        let why = match (&self.phase, error) {
            (Phase::Connecting { .. }, _) => "Cannot reach the server".to_owned(),
            (_, Some(e)) => {
                log::debug!("collab: connection closed: {e}");
                "The connection was lost".to_owned()
            }
            (_, None) => "The server closed the connection".to_owned(),
        };
        // The transport already closed this connection; there is nothing to close.
        self.drop_connection(now_ms, &why, false);
    }

    fn on_frame(&mut self, now_ms: u64, bytes: &[u8], store: &mut LoroStore) {
        if matches!(self.phase, Phase::Idle | Phase::Stopped(_) | Phase::Backoff { .. } | Phase::Connecting { .. }) {
            return;
        }
        self.health.last_rx_ms = now_ms;
        let msg = match ServerMessage::decode(bytes) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("collab: the server sent a frame that cannot be read: {e}");
                self.fail_connection(now_ms, "The server sent something the app cannot read");
                return;
            }
        };
        match msg {
            ServerMessage::Joined { protocol_version, room, client_id, peers, limits, .. } => {
                self.on_joined(now_ms, protocol_version, &room, client_id, &peers, limits);
            }
            ServerMessage::SyncResponse { kind, data, server_version } => {
                self.on_sync_response(now_ms, store, kind, &data, server_version);
            }
            ServerMessage::Update { from, update } => self.on_live_update(now_ms, store, from, &update),
            ServerMessage::Ack { seq } => self.on_ack(now_ms, seq),
            ServerMessage::PeerJoined { peer } => self.on_peer_joined(&peer),
            ServerMessage::PeerLeft { client_id } => self.on_peer_left(client_id),
            ServerMessage::PeerAwareness { from, state } => self.on_peer_awareness(now_ms, from, state),
            ServerMessage::Error { code, retry_after_ms, .. } => self.on_server_error(now_ms, code, retry_after_ms),
            ServerMessage::Ping { nonce } => self.send(&ClientMessage::Pong { nonce }),
            ServerMessage::Pong { .. } => {}
        }
    }

    fn on_joined(
        &mut self,
        now_ms: u64,
        version: u16,
        room: &RoomId,
        client_id: ClientId,
        peers: &[PeerInfo],
        limits: Limits,
    ) {
        if !matches!(self.phase, Phase::Joining { .. }) {
            return;
        }
        let version_ok = (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version);
        if !version_ok || *room != self.cfg.room || client_id != self.cfg.client_id {
            log::warn!("collab: the server answered the join with a different version, room or id");
            self.fail_connection(now_ms, "The server's answer did not match the request");
            return;
        }
        self.limits = limits;
        self.peers.replace(self.cfg.client_id, peers);
        self.phase = Phase::Syncing { since: now_ms };
        self.uploads.blocked = None;
        self.presence_changed = true;
        self.refresh_status();
    }

    fn on_sync_response(&mut self, now_ms: u64, store: &mut LoroStore, kind: SyncKind, data: &[u8], sv: Vec<u8>) {
        let joining = match self.phase {
            Phase::Syncing { .. } => true,
            Phase::Online => false,
            _ => return,
        };
        self.reconcile.sync_outstanding = None;
        if !data.is_empty() {
            let batch = if kind == SyncKind::Snapshot { RemoteKind::Snapshot } else { RemoteKind::CatchUp };
            match self.apply(now_ms, store, data, None, batch) {
                Applied::Ok => {}
                Applied::Failed => {
                    if joining {
                        self.fail_connection(now_ms, "The room data from the server could not be read");
                    } else {
                        self.resync_failed(now_ms);
                    }
                    return;
                }
                Applied::Stopped => return,
                Applied::NeedsDecision => {
                    self.reconcile.pending_snapshot =
                        Some(PendingSnapshot { bytes: data.to_vec(), server_version: sv, joining });
                    return;
                }
            }
        }
        self.after_sync(now_ms, store, joining, &sv);
    }

    /// The server's data is in the document: finish the join, upload what the server lacks.
    fn after_sync(&mut self, now_ms: u64, store: &mut LoroStore, joining: bool, server_version: &[u8]) {
        if joining {
            self.stats.joins += 1;
            self.phase = Phase::Online;
            self.health.stable_since = Some(now_ms);
            self.presence.reset_sent();
            self.problem = None;
        }
        if store.has_pending_remote() {
            self.resync_failed(now_ms);
        } else {
            self.reconcile.resync_gap_ms = RESYNC_MIN_GAP_MS;
        }
        self.upload_missing(now_ms, store, server_version);
        self.refresh_status();
    }

    fn on_live_update(&mut self, now_ms: u64, store: &mut LoroStore, from: ClientId, update: &[u8]) {
        if !matches!(self.phase, Phase::Syncing { .. } | Phase::Online) || self.reconcile.pending_snapshot.is_some() {
            return;
        }
        match self.apply(now_ms, store, update, Some(from), RemoteKind::Live) {
            Applied::Ok | Applied::Stopped => {}
            Applied::Failed => self.resync_failed(now_ms),
            Applied::NeedsDecision => {
                self.reconcile.pending_snapshot =
                    Some(PendingSnapshot { bytes: update.to_vec(), server_version: Vec::new(), joining: false });
            }
        }
    }

    fn on_ack(&mut self, now_ms: u64, seq: u32) {
        self.stats.acks += 1;
        if let Some(i) = self.uploads.inflight.iter().position(|f| f.seq == seq) {
            self.uploads.inflight.remove(i);
        }
        if self.uploads.inflight.is_empty()
            && self.uploads.flush_at.is_none()
            && !self.uploads.want_flush
            && self.reconcile.sync_outstanding.is_none()
        {
            self.uploads.local_dirty = false;
        }
        if self.uploads.want_flush && self.uploads.inflight.len() < MAX_INFLIGHT {
            self.uploads.flush_at = Some(now_ms);
        }
        self.refresh_status();
    }

    fn on_peer_joined(&mut self, peer: &PeerInfo) {
        let was_empty = self.peers.is_empty();
        if self.peers.join(self.cfg.client_id, peer) {
            let name = self.peers.get(&peer.client_id).map(Peer::display_name).unwrap_or_default();
            self.events.push(CollabEvent::PeerJoined { id: peer.client_id, name });
            self.presence_changed = true;
        }
        if was_empty && !self.peers.is_empty() {
            // The newcomer has not seen our presence; the heartbeat would take up to 10 s.
            self.presence.reset_sent();
        }
        self.refresh_status();
    }

    fn on_peer_left(&mut self, id: ClientId) {
        if let Some(p) = self.peers.leave(id) {
            self.events.push(CollabEvent::PeerLeft { id, name: p.display_name() });
            self.presence_changed = true;
            if self.following == Some(id) {
                self.following = None;
                self.events.push(CollabEvent::FollowEnded(FollowEnd::PeerLeft));
            }
        }
        self.refresh_status();
    }

    fn on_peer_awareness(&mut self, now_ms: u64, from: ClientId, state: AwarenessState) {
        if state.validate().is_err() {
            return;
        }
        let viewport = state.viewport;
        if self.peers.awareness(self.cfg.client_id, now_ms, from, state) {
            self.presence_changed = true;
            if self.following == Some(from)
                && let Some(v) = viewport
            {
                self.events.push(CollabEvent::FollowViewport(v));
            }
        }
    }

    fn on_server_error(&mut self, now_ms: u64, code: ErrorCode, retry_after_ms: Option<u32>) {
        let joining = !matches!(self.phase, Phase::Online);
        let problem = explain(code, joining);
        let retry = retry_after_ms.map(u64::from);
        match code {
            ErrorCode::ResyncRequired => {
                self.stats.resync_required += 1;
                self.want_resync(now_ms);
                self.reconcile.resync_gap_ms = RESYNC_MIN_GAP_MS;
            }
            ErrorCode::RateLimited => {
                let wait = retry.unwrap_or(1000).clamp(50, RESYNC_MAX_GAP_MS);
                self.uploads.send_not_before = now_ms.saturating_add(wait);
                self.reconcile.resync_due = Some(
                    self.reconcile
                        .resync_due
                        .map_or(self.uploads.send_not_before, |d| d.max(self.uploads.send_not_before)),
                );
            }
            ErrorCode::InvalidUpdate => {
                self.uploads.blocked = Some(Block::Rejected);
                self.problem = Some(problem.clone());
                self.events.push(CollabEvent::Problem(problem));
            }
            ErrorCode::TooLarge if !joining => {
                self.uploads.blocked = Some(Block::Oversize);
                self.problem = Some(problem.clone());
                self.events.push(CollabEvent::Problem(problem));
            }
            _ => {
                if let Some(reason) =
                    stop_reason(code).or((code == ErrorCode::TooLarge).then_some(StopReason::TooLarge))
                {
                    self.out.push_back(Output::Close);
                    self.enter_stopped(reason, Some(problem));
                } else {
                    // The server closes the connection after these; reconnect with its hint, if it gave one.
                    self.health.server_retry_ms = retry;
                    self.events.push(CollabEvent::Problem(problem.clone()));
                    self.fail_connection(now_ms, &problem.text);
                }
            }
        }
        self.refresh_status();
    }

    // ---- applying remote data ---------------------------------------------------------------------------------

    fn apply(
        &mut self,
        now_ms: u64,
        store: &mut LoroStore,
        bytes: &[u8],
        from: Option<ClientId>,
        kind: RemoteKind,
    ) -> Applied {
        let _span = drafftink_profile::span("collab_apply");
        match store.apply_remote(bytes) {
            Ok(cs) => {
                self.push_remote(from, kind, cs);
                if store.has_pending_remote() && kind == RemoteKind::Live {
                    self.want_resync(now_ms);
                }
                Applied::Ok
            }
            Err(DocError::SnapshotStartsLater) => {
                if self.cfg.replace_when_trimmed {
                    match store.replace_from_snapshot(bytes) {
                        Ok(cs) => {
                            self.push_remote(None, RemoteKind::Replaced, cs);
                            Applied::Ok
                        }
                        Err(e) => {
                            self.doc_failure(now_ms, &e, false);
                            Applied::Stopped
                        }
                    }
                } else {
                    let message = "The server's copy of this room starts after the history this copy has. Replacing \
                                   this copy with the server's loses changes made here that were never shared."
                        .to_owned();
                    self.events.push(CollabEvent::NeedsDecision { message });
                    self.problem = Some(Problem::local(
                        "This copy of the document cannot be merged with the server's. Choose which one to keep.",
                        ProblemAction::Decide,
                        false,
                    ));
                    Applied::NeedsDecision
                }
            }
            Err(e @ (DocError::NewerSchema { .. } | DocError::Poisoned)) => {
                self.doc_failure(now_ms, &e, false);
                Applied::Stopped
            }
            Err(e) => {
                log::warn!("collab: remote data could not be applied: {e}");
                self.events.push(CollabEvent::Problem(Problem::local(
                    "Changes from the server could not be read. Asking the server again.",
                    ProblemAction::Wait,
                    false,
                )));
                Applied::Failed
            }
        }
    }

    fn doc_failure(&mut self, _now_ms: u64, e: &DocError, _joining: bool) {
        let (reason, text, action) = match e {
            DocError::NewerSchema { .. } => (
                StopReason::DocumentTooNew,
                "This room was saved by a newer version of the app. Update the app to open it.".to_owned(),
                ProblemAction::UpdateApp,
            ),
            DocError::Poisoned => (
                StopReason::NeedsReload,
                "Sharing hit an internal error and cannot continue. Save a copy, then reload the document from the server.".to_owned(),
                ProblemAction::Reload,
            ),
            other => (StopReason::NeedsReload, format!("Sharing stopped: {other}"), ProblemAction::Reload),
        };
        self.out.push_back(Output::Close);
        self.enter_stopped(reason, Some(Problem::local(text, action, true)));
    }

    fn push_remote(&mut self, from: Option<ClientId>, kind: RemoteKind, changes: ChangeSet) {
        if changes.is_empty() && kind != RemoteKind::Replaced {
            return;
        }
        let who = from.and_then(|id| self.peers.get(&id)).map(Peer::display_name);
        self.stats.remote_batches += 1;
        self.events.push(CollabEvent::Remote(RemoteBatch { from, who, kind, changes }));
    }

    // ---- sending ----------------------------------------------------------------------------------------------

    fn update_interval_ms(&self) -> u64 {
        1000u64.div_ceil(u64::from(self.limits.max_updates_per_sec.max(1))).max(SEND_DEBOUNCE_MS)
    }

    fn awareness_interval(&self) -> u64 {
        LocalPresence::min_interval_ms(self.limits.max_awareness_per_sec)
    }

    fn max_update_bytes(&self) -> usize {
        (self.limits.max_update_bytes as usize).min(MAX_UPDATE_BYTES)
    }

    fn send(&mut self, msg: &ClientMessage) {
        match msg.encode() {
            Ok(bytes) => self.out.push_back(Output::Send(bytes)),
            // Not `{msg:?}`: a `Join` holds the access token.
            Err(e) => log::warn!("collab: a message was not sent: {e}"),
        }
    }

    /// Sends the document's unsent operations as one update.
    fn flush_updates(&mut self, now_ms: u64, store: &mut LoroStore) {
        self.uploads.flush_at = None;
        if self.uploads.inflight.len() >= MAX_INFLIGHT {
            self.uploads.want_flush = true;
            return;
        }
        self.uploads.want_flush = false;
        match store.take_unsent_update() {
            Ok(Some(bytes)) => self.send_update(now_ms, bytes),
            Ok(None) => {
                if self.uploads.inflight.is_empty() {
                    self.uploads.local_dirty = false;
                }
            }
            Err(e) => self.doc_failure(now_ms, &e, false),
        }
        self.refresh_status();
    }

    fn send_update(&mut self, now_ms: u64, bytes: Vec<u8>) {
        if bytes.len() > self.max_update_bytes() {
            let problem = explain(ErrorCode::TooLarge, false);
            self.uploads.blocked = Some(Block::Oversize);
            self.problem = Some(problem.clone());
            self.events.push(CollabEvent::Problem(problem));
            return;
        }
        let seq = self.uploads.next_seq;
        let msg = ClientMessage::Update { seq, update: bytes };
        match msg.encode() {
            Ok(frame) => {
                self.uploads.next_seq = self.uploads.next_seq.wrapping_add(1);
                self.uploads.inflight.push_back(Inflight { seq, sent_at: now_ms });
                self.uploads.last_update_sent_ms = Some(now_ms);
                self.stats.updates_sent += 1;
                self.out.push_back(Output::Send(frame));
            }
            Err(e) => {
                log::warn!("collab: an update was not sent: {e}");
                self.uploads.blocked = Some(Block::Oversize);
            }
        }
    }

    /// Uploads what the server's version lacks. This single step repairs reconnects, dropped updates and rate limits.
    fn upload_missing(&mut self, now_ms: u64, store: &mut LoroStore, server_version: &[u8]) {
        self.uploads.inflight.clear();
        self.uploads.flush_at = None;
        self.uploads.want_flush = false;
        let local = match store.version_vector() {
            Ok(v) => v,
            Err(e) => {
                self.doc_failure(now_ms, &e, false);
                return;
            }
        };
        let server_has_all = match (VersionVector::decode(server_version), VersionVector::decode(&local)) {
            (Ok(server), Ok(local)) => server.includes_vv(&local),
            _ => false,
        };
        if server_has_all {
            // Whatever the document still counts as unsent is already on the server.
            let _ = store.take_unsent_update();
            self.uploads.local_dirty = false;
            return;
        }
        let bytes = match store.export_updates_since(server_version) {
            Ok(b) => b,
            Err(DocError::HistoryTrimmed) => match store.export_snapshot() {
                Ok(b) => b,
                Err(e) => return self.doc_failure(now_ms, &e, false),
            },
            Err(e) => return self.doc_failure(now_ms, &e, false),
        };
        // The export covers everything the document counts as unsent.
        let _ = store.take_unsent_update();
        self.uploads.local_dirty = true;
        if self.uploads.blocked.is_none() {
            self.send_update(now_ms, bytes);
        }
    }

    // ---- reconciling ------------------------------------------------------------------------------------------

    fn want_resync(&mut self, now_ms: u64) {
        if self.reconcile.sync_outstanding.is_some() {
            return;
        }
        let earliest = self.reconcile.last_resync_ms.map_or(0, |t| t.saturating_add(self.reconcile.resync_gap_ms));
        let at = now_ms.max(earliest);
        self.reconcile.resync_due = Some(self.reconcile.resync_due.map_or(at, |d| d.min(at)));
    }

    fn resync_failed(&mut self, now_ms: u64) {
        self.reconcile.resync_gap_ms = (self.reconcile.resync_gap_ms.saturating_mul(2)).min(RESYNC_MAX_GAP_MS);
        self.want_resync(now_ms);
    }

    fn send_resync(&mut self, now_ms: u64, store: &LoroStore) {
        self.reconcile.resync_due = None;
        if let Some(t) = self.reconcile.sync_outstanding
            && now_ms.saturating_sub(t) < SYNC_REPLY_TIMEOUT_MS
        {
            return;
        }
        self.reconcile.sync_outstanding = Some(now_ms);
        self.reconcile.last_resync_ms = Some(now_ms);
        self.stats.sync_requests += 1;
        match store.version_vector() {
            Ok(version) => self.send(&ClientMessage::RequestSync { version }),
            Err(e) => self.doc_failure(now_ms, &e, false),
        }
    }

    // ---- connection loss, stopping ----------------------------------------------------------------------------

    /// The connection is unusable: close it and try again after the backoff.
    fn fail_connection(&mut self, now_ms: u64, why: &str) {
        self.drop_connection(now_ms, why, true);
    }

    fn drop_connection(&mut self, now_ms: u64, why: &str, close: bool) {
        if close {
            self.out.push_back(Output::Close);
        }
        let jitter = self.next_random();
        let mut delay = u64::from(reconnect_delay_ms(self.health.attempt, jitter));
        if let Some(hint) = self.health.server_retry_ms.take() {
            delay = delay.max(hint);
        }
        self.health.attempt = self.health.attempt.saturating_add(1);
        self.stats.reconnects += 1;
        self.phase = Phase::Backoff { until: now_ms.saturating_add(delay) };
        self.health.offline_reason = why.to_owned();
        self.reset_session();
        self.refresh_status();
    }

    /// Forget everything that belongs to one connection. The document and unsent edits stay.
    fn reset_session(&mut self) {
        self.uploads.inflight.clear();
        self.uploads.flush_at = None;
        self.uploads.want_flush = false;
        self.reconcile.resync_due = None;
        self.reconcile.sync_outstanding = None;
        self.reconcile.pending_snapshot = None;
        self.health.stable_since = None;
        self.uploads.blocked = None;
        self.uploads.send_not_before = 0;
        self.limits = Limits::DEFAULT;
        self.presence.reset_sent();
        if !self.peers.is_empty() {
            self.peers.clear();
            self.presence_changed = true;
        }
        if self.following.take().is_some() {
            self.events.push(CollabEvent::FollowEnded(FollowEnd::Disconnected));
        }
    }

    fn enter_stopped(&mut self, reason: StopReason, problem: Option<Problem>) {
        self.phase = Phase::Stopped(reason);
        self.reset_session();
        if let Some(p) = problem {
            self.problem = Some(p.clone());
            self.events.push(CollabEvent::Problem(p));
        }
        self.refresh_status();
    }

    fn expire_peers(&mut self, now_ms: u64) {
        if !self.peers.expire(now_ms).is_empty() {
            self.presence_changed = true;
        }
        // A trail whose last point just expired needs one more redraw.
        if self.peers.next_trail_expiry(self.health.last_tick_ms).is_some_and(|t| t <= now_ms) {
            self.presence_changed = true;
        }
    }

    fn next_random(&mut self) -> u32 {
        let mut x = self.health.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.health.rng = x;
        (x >> 16) as u32
    }

    // ---- status -----------------------------------------------------------------------------------------------

    fn flush_events(&mut self) {
        if self.presence_changed {
            self.presence_changed = false;
            self.events.push(CollabEvent::Presence);
        }
        self.refresh_status();
    }

    fn refresh_status(&mut self) {
        let status = self.build_status();
        if status != self.status {
            self.status = status.clone();
            self.events.push(CollabEvent::Status(status));
        }
    }

    fn build_status(&self) -> Status {
        let unsynced = self.uploads.local_dirty
            || !self.uploads.inflight.is_empty()
            || self.uploads.flush_at.is_some()
            || self.uploads.want_flush;
        let (state, stopped, text, retry_at_ms) = match self.phase {
            Phase::Idle => return Status::idle(),
            Phase::Backoff { until } => {
                let hint =
                    if self.health.offline_reason.is_empty() { "Offline" } else { self.health.offline_reason.as_str() };
                (
                    ConnState::Reconnecting,
                    None,
                    format!(
                        "{hint}. Your changes are kept on this device and will be shared when the connection is back."
                    ),
                    Some(until),
                )
            }
            Phase::Connecting { .. } | Phase::Joining { .. } => {
                (ConnState::Connecting, None, "Connecting to the server.".to_owned(), None)
            }
            Phase::Syncing { .. } => (ConnState::Syncing, None, "Joined. Catching up with the room.".to_owned(), None),
            Phase::Online => {
                let text = match (&self.problem, unsynced) {
                    (Some(p), _) => p.text.clone(),
                    (None, true) => "Connected. Sharing your latest changes.".to_owned(),
                    (None, false) => "Connected. All changes are shared.".to_owned(),
                };
                (ConnState::Online, None, text, None)
            }
            Phase::Stopped(reason) => {
                let text = match (&self.problem, reason) {
                    (Some(p), _) => p.text.clone(),
                    (None, StopReason::Left) => "Left the room. Working on this device only.".to_owned(),
                    (None, StopReason::Declined) => "Kept this copy. Not shared with the room.".to_owned(),
                    (None, _) => "Sharing stopped.".to_owned(),
                };
                (ConnState::Stopped, Some(reason), text, None)
            }
        };
        Status { state, stopped, text, unsynced, retry_at_ms, peers: self.peers.len(), problem: self.problem.clone() }
    }
}

enum Applied {
    Ok,
    Failed,
    Stopped,
    NeedsDecision,
}
