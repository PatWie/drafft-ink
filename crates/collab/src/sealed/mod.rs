//! The client of an end-to-end encrypted room (protocol v2, `docs/design/collab-e2e.md` sections 7 and 8). Sans-IO
//! like [`CollabClient`](crate::CollabClient): it owns no socket, no clock and no document, and the owner moves its
//! inputs and outputs. It shares the transport, the presence tables, the status types and the backoff with the
//! plaintext client.
//!
//! What differs is what the relay can do. A relay of sealed rooms reads nothing, so:
//!
//! * the client seals every update, snapshot and presence message, and opens everything it receives;
//! * the relay cannot say which operations a client lacks, so the client keeps `relay_vv`, the version of what the
//!   relay is known to hold. After every catch-up it uploads `export_updates_since(relay_vv)`, which repairs offline
//!   edits, lost acknowledgements, a crashed tab and a dropped tail with one step;
//! * the relay numbers records, and the client tracks the highest sequence number it holds without a gap
//!   (`last_seq`). A reconnect joins with it and gets only what is new;
//! * the relay cannot compact, so it asks one client to, and the client seals a snapshot that the relay swaps in by
//!   compare-and-set;
//! * an empty room (a new one, or an expired one restored from a local copy) is seeded with a snapshot.
//!
//! Not built yet (phase 2 and later): images as files, the peer-to-peer fallback when the relay's storage fails, and
//! the crash fence for a malformed update.

mod images;
mod inbound;
mod outbound;
mod problems;

use std::collections::{BTreeSet, HashMap, VecDeque};

use drafftink_doc::{DocError, LoroStore};
use drafftink_protocol::v2::{JoinMode, RoomLimits, RoomState};
use drafftink_protocol::{
    ClientId, IDLE_TIMEOUT_MS, KEEPALIVE_PING, PING_INTERVAL_MS, Point, UserInfo, Viewport, reconnect_delay_ms,
};
use drafftink_seal::{RoomKey, RoomKeys};
use loro::VersionVector;

use crate::awareness::{LocalPresence, PeerTable};
use crate::blobs::BlobAuth;
use crate::client::{
    CONNECT_TIMEOUT_MS, CollabEvent, FollowEnd, JOIN_TIMEOUT_MS, MAX_INFLIGHT, Output, SEND_DEBOUNCE_MS, STABLE_MS,
    Stats, TrimmedChoice,
};
use crate::status::{ConnState, Problem, ProblemAction, Status, StopReason};
use crate::transport::TransportEvent;
use crate::url::ServerUrl;

pub use images::{
    BLOB_TIMEOUT_MS, IMAGE_RETRY_BASE_MS, IMAGE_RETRY_MAX_MS, ImageEvent, ImageProgress, ImageRefusal, MAX_DOWNLOADS,
    MAX_UPLOADS,
};

/// A gap in the sequence numbers is only repaired when it stays for this long: the acknowledgement of this client's
/// own record, which fills one such gap, can wait for the relay's next write.
pub const GAP_GRACE_MS: u64 = 3_000;
/// A request for the next page of the catch-up that gets no answer for this long is asked again, in milliseconds.
pub const CATCHUP_REPLY_TIMEOUT_MS: u64 = 15_000;
/// The least gap between two compaction attempts, in milliseconds.
pub const COMPACT_RETRY_MS: u64 = 15_000;
/// A snapshot larger than this is exported without most of its history.
const FULL_SNAPSHOT_LIMIT: usize = 8 * 1024 * 1024;
/// Commits of history that a shallow snapshot keeps.
const SHALLOW_KEEP_COMMITS: usize = 200;

/// What a client remembers of a room between connections and, through the workspace, between runs of the app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resume {
    /// The highest relay sequence number held without a gap below it. 0 for none.
    pub last_seq: u64,
    /// The version of what the relay is known to hold, as `LoroStore::version_vector` encodes it. Empty for none.
    pub relay_vv: Vec<u8>,
}

/// What the client needs to join a room.
pub struct SealedConfig {
    /// The server. Its path is replaced by the room's (`/r/<id>/ws`).
    pub url: ServerUrl,
    /// The room key: the one secret. It is wiped when the client is dropped.
    pub key: RoomKey,
    /// This client's id. Keep it across reconnects.
    pub client_id: ClientId,
    /// Name and colour shown to the others (sent sealed, inside presence).
    pub user: Option<UserInfo>,
    /// `Create` makes the room when it does not exist (a new room, or restoring an expired one from the local copy).
    /// `Open` refuses a missing room, so a mistyped link makes nothing.
    pub mode: JoinMode,
    /// What an earlier connection to this room left.
    pub resume: Resume,
    /// Any random number. It spreads the reconnect times of clients that lost the relay together.
    pub jitter_seed: u64,
    /// When the relay's snapshot cannot merge into this document (its history was trimmed), replace the local
    /// document without asking. Edits that were never sent are lost. Off: the client stops and keeps the local copy.
    pub replace_when_trimmed: bool,
}

impl std::fmt::Debug for SealedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedConfig")
            .field("url", &self.url)
            .field("key", &"..")
            .field("client_id", &self.client_id)
            .field("mode", &self.mode)
            .field("resume", &self.resume)
            .finish_non_exhaustive()
    }
}

/// Why a client could not be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SealedConfigError {
    /// The display name or colour breaks a protocol rule.
    #[error("The display name or colour is not valid: {0}")]
    User(#[from] drafftink_protocol::ValidationError),
    /// The remembered version of the relay's content cannot be read.
    #[error("The saved state of this room cannot be read, so it is not used")]
    Resume,
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

/// An update sent and not yet acknowledged.
#[derive(Debug)]
struct Inflight {
    ctr: u64,
    sent_at: u64,
    /// The version the document had when the update was exported: what the relay holds once it is acknowledged.
    end: VersionVector,
}

/// Why uploads wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Oversize,
}

/// Local edits on their way to the relay.
#[derive(Debug, Default)]
struct Uploads {
    inflight: VecDeque<Inflight>,
    flush_at: Option<u64>,
    want_flush: bool,
    last_update_sent_ms: Option<u64>,
    send_not_before: u64,
    blocked: Option<Block>,
    local_dirty: bool,
    /// A seed snapshot is on its way: the version it holds.
    seed: Option<VersionVector>,
}

/// Where the client is in the relay's log.
#[derive(Debug, Default)]
struct Position {
    /// The highest sequence number held with none missing below it.
    last_seq: u64,
    /// Sequence numbers above `last_seq` that were applied: records that arrived before an earlier one.
    held: BTreeSet<u64>,
    /// The relay's newest sequence number as last announced.
    head_seq: u64,
    /// The sequence number the relay's snapshot covers, as last announced.
    snapshot_seq: u64,
    /// Whether the relay said its catch-up has more pages after the last one.
    more: bool,
    /// The relay handed out a snapshot or a record: the room is not empty.
    saw_content: bool,
    /// A request for the next page is out since this time.
    catchup_asked: Option<u64>,
    /// A gap was seen at this time and is repaired if it stays.
    gap_since: Option<u64>,
    /// The highest update counter seen per sender, to notice a record that never came.
    sender_ctrs: HashMap<ClientId, u64>,
    /// The highest presence counter seen per sender: older presence is a replay.
    presence_ctrs: HashMap<ClientId, u64>,
}

/// The collaboration client of one sealed room. See the module docs.
pub struct SealedClient {
    cfg: SealedConfig,
    keys: RoomKeys,
    endpoint: ServerUrl,
    phase: Phase,
    status: Status,
    out: VecDeque<Output>,
    events: Vec<CollabEvent>,
    presence: LocalPresence,
    peers: PeerTable,
    following: Option<ClientId>,
    presence_changed: bool,
    limits: RoomLimits,
    /// The version of what the relay is known to hold.
    relay_vv: VersionVector,
    pos: Position,
    uploads: Uploads,
    /// The pictures of the room: offers, wants and their requests.
    images: images::Images,
    /// Upload what the relay lacks at this time: a record was dropped by its rate limit.
    reupload_at: Option<u64>,
    /// The relay asked for a compaction up to this sequence number.
    compact_for: Option<u64>,
    compact_after_ms: u64,
    /// A snapshot is out, covering this sequence number.
    snapshot_out: Option<u64>,
    room_state: Option<RoomState>,
    expires_at_ms: u64,
    expiring: bool,
    skipped: u32,
    // Counters of the sealed records this client makes, one per kind: receivers only look at the one of updates.
    update_ctr: u64,
    presence_ctr: u64,
    snapshot_ctr: u64,
    // Connection health.
    last_tick_ms: u64,
    last_rx_ms: u64,
    last_ping_ms: u64,
    stable_since: Option<u64>,
    attempt: u32,
    rng: u64,
    offline_reason: String,
    server_retry_ms: Option<u64>,
    problem: Option<Problem>,
    stats: Stats,
}

impl std::fmt::Debug for SealedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedClient")
            .field("room", &self.keys.room_id())
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl SealedClient {
    /// Makes a client. It does nothing until [`start`](Self::start).
    pub fn new(cfg: SealedConfig) -> Result<Self, SealedConfigError> {
        if let Some(user) = &cfg.user {
            user.validate()?;
        }
        let relay_vv = if cfg.resume.relay_vv.is_empty() {
            VersionVector::default()
        } else {
            VersionVector::decode(&cfg.resume.relay_vv).map_err(|_| SealedConfigError::Resume)?
        };
        let keys = RoomKeys::new(&cfg.key);
        let endpoint = cfg.url.with_path(format!("/r/{}/ws", keys.room_id()));
        let presence = LocalPresence::new(cfg.user.clone());
        let rng = cfg.jitter_seed | 1;
        let pos = Position { last_seq: cfg.resume.last_seq, ..Position::default() };
        let images =
            images::Images::new(endpoint.http_base(), keys.room_id(), BlobAuth::member(keys.membership_token()));
        Ok(SealedClient {
            cfg,
            keys,
            endpoint,
            phase: Phase::Idle,
            status: Status::idle(),
            out: VecDeque::new(),
            events: Vec::new(),
            presence,
            peers: PeerTable::default(),
            following: None,
            presence_changed: false,
            limits: RoomLimits::DEFAULT,
            relay_vv,
            pos,
            uploads: Uploads::default(),
            images,
            reupload_at: None,
            compact_for: None,
            compact_after_ms: 0,
            snapshot_out: None,
            room_state: None,
            expires_at_ms: 0,
            expiring: false,
            skipped: 0,
            update_ctr: 0,
            presence_ctr: 0,
            snapshot_ctr: 0,
            last_tick_ms: 0,
            last_rx_ms: 0,
            last_ping_ms: 0,
            stable_since: None,
            attempt: 0,
            rng,
            offline_reason: String::new(),
            server_retry_ms: None,
            problem: None,
            stats: Stats::default(),
        })
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

    /// The server address the person gave (the room's path is added when connecting).
    pub fn url(&self) -> &ServerUrl {
        &self.cfg.url
    }

    /// The id of the room.
    pub fn room_id(&self) -> drafftink_protocol::v2::RoomId {
        self.keys.room_id()
    }

    /// This client's id.
    pub fn client_id(&self) -> ClientId {
        self.cfg.client_id
    }

    /// The peer being followed, if any.
    pub fn following(&self) -> Option<ClientId> {
        self.following
    }

    /// True while connected and joined (online or still catching up).
    pub fn is_connected(&self) -> bool {
        matches!(self.phase, Phase::Syncing { .. } | Phase::Online)
    }

    /// Counters of what happened so far.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// What to keep so that the next run joins from here: the sequence number and the relay's version.
    pub fn resume(&self) -> Resume {
        Resume { last_seq: self.pos.last_seq, relay_vv: self.relay_vv.encode() }
    }

    /// When the relay deletes the room if nobody visits, in milliseconds since the epoch. 0: never or not known.
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }

    /// The room is past its deadline and is deleted when the last person leaves.
    pub fn expiring(&self) -> bool {
        self.expiring
    }

    /// How many received changes could not be read and were skipped (a damaged record, or a malformed update).
    pub fn skipped(&self) -> u32 {
        self.skipped
    }

    /// Whether the relay created the room for this join. `None` before the first join.
    pub fn room_state(&self) -> Option<RoomState> {
        self.room_state
    }

    /// The next frame to send, connect or close request.
    pub fn poll_output(&mut self) -> Option<Output> {
        self.out.pop_front()
    }

    /// Everything that happened since the last call, in order.
    pub fn take_events(&mut self) -> Vec<CollabEvent> {
        std::mem::take(&mut self.events)
    }

    /// When the owner must call [`tick`](Self::tick) next. `None`: nothing is scheduled. A time in the past means
    /// "now".
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
            Phase::Joining { since } | Phase::Syncing { since } => at(since.saturating_add(JOIN_TIMEOUT_MS)),
            Phase::Online => {}
        }
        if self.is_connected() || matches!(self.phase, Phase::Joining { .. }) {
            at(self.last_rx_ms.max(self.last_ping_ms).saturating_add(u64::from(PING_INTERVAL_MS)));
            at(self.last_rx_ms.saturating_add(u64::from(IDLE_TIMEOUT_MS)));
        }
        if self.is_connected() {
            if let Some(t) = self.pos.gap_since {
                at(t.saturating_add(GAP_GRACE_MS));
            }
            if let Some(t) = self.pos.catchup_asked {
                at(t.saturating_add(CATCHUP_REPLY_TIMEOUT_MS));
            }
        }
        if matches!(self.phase, Phase::Online) {
            if let Some(t) = self.reupload_at {
                at(t);
            }
            if let Some(t) = self.uploads.flush_at.filter(|_| self.uploads.blocked.is_none()) {
                at(t.max(self.uploads.send_not_before));
            }
            if let Some(f) = self.uploads.inflight.front() {
                at(f.sent_at.saturating_add(crate::client::ACK_TIMEOUT_MS));
            }
            if self.compact_for.is_some() && self.snapshot_out.is_none() {
                at(self.compact_after_ms);
            }
            if let Some(t) = self.images.next_wake() {
                at(t);
            }
            if let Some(t) = self.presence.next_due(self.last_tick_ms, self.presence_interval(), !self.peers.is_empty())
            {
                at(t);
            }
            if let Some(t) = self.stable_since.filter(|_| self.attempt > 0) {
                at(t.saturating_add(STABLE_MS));
            }
        }
        if let Some(t) = self.peers.next_expiry() {
            at(t);
        }
        if let Some(t) = self.peers.next_trail_expiry(self.last_tick_ms) {
            at(t);
        }
        wake
    }

    // ---- lifecycle --------------------------------------------------------------------------------------------

    /// Starts connecting, or connects again after a stop. The first attempt happens at the next
    /// [`tick`](Self::tick).
    pub fn start(&mut self, now_ms: u64) {
        if matches!(self.phase, Phase::Idle | Phase::Stopped(_)) {
            self.problem = None;
            self.attempt = 0;
            self.phase = Phase::Backoff { until: now_ms };
            self.offline_reason = String::new();
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
            self.send(&drafftink_protocol::v2::ClientMessage::Leave);
        }
        if !matches!(self.phase, Phase::Idle | Phase::Stopped(_)) {
            if !matches!(self.phase, Phase::Backoff { .. }) {
                self.out.push_back(Output::Close);
            }
            self.enter_stopped(StopReason::Left, None);
        }
    }

    // ---- local inputs -----------------------------------------------------------------------------------------

    /// The document got a local commit. Call it after every commit; it is cheap.
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

    /// Follows a peer. Returns false for an unknown peer.
    pub fn follow(&mut self, id: ClientId) -> bool {
        let Some(peer) = self.peers.get(&id) else { return false };
        self.following = Some(id);
        if let Some(v) = peer.viewport {
            self.events.push(CollabEvent::FollowViewport(v));
        }
        true
    }

    /// Stops following.
    pub fn stop_following(&mut self) {
        self.following = None;
    }

    /// Answers [`CollabEvent::NeedsDecision`]. A sealed client has already stopped and kept the local copy when it
    /// raised it, so there is nothing to resolve: the owner joins again with `replace_when_trimmed` to take the
    /// room's content.
    pub fn resolve_trimmed(&mut self, _now_ms: u64, _store: &mut LoroStore, _choice: TrimmedChoice) {}

    // ---- time -------------------------------------------------------------------------------------------------

    /// Advances time: starts connections, times out silent ones, sends due updates, requests and presence.
    pub fn tick(&mut self, now_ms: u64, store: &mut LoroStore) {
        match self.phase {
            Phase::Backoff { until } if now_ms >= until => {
                self.phase = Phase::Connecting { since: now_ms };
                self.stats.connects += 1;
                self.out.push_back(Output::Connect(self.endpoint.clone()));
            }
            Phase::Connecting { since } if now_ms.saturating_sub(since) >= CONNECT_TIMEOUT_MS => {
                self.fail_connection(now_ms, "The server did not answer in time");
            }
            Phase::Joining { since } | Phase::Syncing { since } if now_ms.saturating_sub(since) >= JOIN_TIMEOUT_MS => {
                self.fail_connection(now_ms, "The server did not finish the join in time");
            }
            _ => {}
        }
        if self.is_connected() || matches!(self.phase, Phase::Joining { .. }) {
            if now_ms.saturating_sub(self.last_rx_ms) >= u64::from(IDLE_TIMEOUT_MS) {
                self.fail_connection(now_ms, "The connection went quiet");
            } else if now_ms.saturating_sub(self.last_rx_ms.max(self.last_ping_ms)) >= u64::from(PING_INTERVAL_MS) {
                self.last_ping_ms = now_ms;
                self.out.push_back(Output::SendText(KEEPALIVE_PING));
            }
        }
        if self.is_connected() {
            self.tick_log(now_ms);
        }
        if matches!(self.phase, Phase::Online) {
            self.tick_online(now_ms, store);
        }
        self.expire_peers(now_ms);
        self.last_tick_ms = now_ms;
        self.flush_events();
    }

    fn tick_online(&mut self, now_ms: u64, store: &mut LoroStore) {
        if self.stable_since.is_some_and(|t| now_ms.saturating_sub(t) >= STABLE_MS) {
            self.attempt = 0;
        }
        let waiting = self.uploads.inflight.len();
        self.uploads.inflight.retain(|f| now_ms.saturating_sub(f.sent_at) < crate::client::ACK_TIMEOUT_MS);
        if self.uploads.inflight.len() < waiting {
            // Updates with no acknowledgement: the relay may have dropped them. Upload what it lacks.
            self.upload_missing(now_ms, store);
        }
        if self.reupload_at.is_some_and(|t| t <= now_ms) {
            self.reupload_at = None;
            self.upload_missing(now_ms, store);
        }
        if self.uploads.blocked.is_none() && now_ms >= self.uploads.send_not_before {
            let due = self.uploads.flush_at.is_some_and(|t| t <= now_ms)
                || (self.uploads.want_flush && self.uploads.inflight.len() < MAX_INFLIGHT);
            if due {
                self.flush_updates(now_ms, store);
            }
        }
        self.maybe_compact(now_ms, store);
        self.send_presence_if_due(now_ms);
        self.images_changed(now_ms);
    }

    /// Repairs a gap in the sequence numbers that stayed, and asks again when a page of the catch-up got no answer.
    fn tick_log(&mut self, now_ms: u64) {
        if self.pos.gap_since.is_some_and(|t| now_ms.saturating_sub(t) >= GAP_GRACE_MS) {
            self.pos.gap_since = None;
            self.ask_catchup(now_ms);
        }
        // A snapshot that gets no answer is forgotten, so that the next advice is acted on.
        if self.snapshot_out.is_some() && self.uploads.seed.is_none() && now_ms >= self.compact_after_ms {
            self.snapshot_out = None;
        }
        if self.pos.catchup_asked.is_some_and(|t| now_ms.saturating_sub(t) >= CATCHUP_REPLY_TIMEOUT_MS) {
            self.pos.catchup_asked = None;
            self.ask_catchup(now_ms);
        }
    }

    // ---- transport input --------------------------------------------------------------------------------------

    /// Handles what the transport reported.
    pub fn on_event(&mut self, now_ms: u64, event: TransportEvent, store: &mut LoroStore) {
        match event {
            TransportEvent::Opened => self.on_opened(now_ms),
            TransportEvent::Message(bytes) => self.on_frame(now_ms, &bytes, store),
            TransportEvent::Text(_) => self.on_text(now_ms),
            TransportEvent::Closed { error } => self.on_closed(now_ms, error),
        }
        self.flush_events();
    }

    fn on_text(&mut self, now_ms: u64) {
        if self.is_connected() || matches!(self.phase, Phase::Joining { .. }) {
            // The only text a relay sends is `pong`: the connection is alive.
            self.last_rx_ms = now_ms;
        }
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

    // ---- connection loss, stopping ----------------------------------------------------------------------------

    fn fail_connection(&mut self, now_ms: u64, why: &str) {
        self.drop_connection(now_ms, why, true);
    }

    fn drop_connection(&mut self, now_ms: u64, why: &str, close: bool) {
        if close {
            self.out.push_back(Output::Close);
        }
        let jitter = self.next_random();
        let mut delay = u64::from(reconnect_delay_ms(self.attempt, jitter));
        if let Some(hint) = self.server_retry_ms.take() {
            delay = delay.max(hint);
        }
        self.attempt = self.attempt.saturating_add(1);
        self.stats.reconnects += 1;
        self.phase = Phase::Backoff { until: now_ms.saturating_add(delay) };
        self.offline_reason = why.to_owned();
        self.reset_session();
        self.refresh_status();
    }

    /// Forgets everything that belongs to one connection. The document, `relay_vv` and unsent edits stay.
    fn reset_session(&mut self) {
        self.uploads.inflight.clear();
        self.uploads.flush_at = None;
        self.uploads.want_flush = false;
        self.uploads.blocked = None;
        self.uploads.send_not_before = 0;
        self.uploads.seed = None;
        self.pos.held.clear();
        self.pos.more = false;
        self.pos.saw_content = false;
        self.reupload_at = None;
        self.pos.catchup_asked = None;
        self.pos.gap_since = None;
        self.compact_for = None;
        self.snapshot_out = None;
        self.stable_since = None;
        self.limits = RoomLimits::DEFAULT;
        self.images.reset();
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

    /// The document is unusable for sharing: stop and say why.
    fn doc_failure(&mut self, e: &DocError) {
        let (reason, text, action) = match e {
            DocError::NewerSchema { .. } => (
                StopReason::DocumentTooNew,
                "This room was saved by a newer version of the app. Update the app to open it.".to_owned(),
                ProblemAction::UpdateApp,
            ),
            DocError::Poisoned => (
                StopReason::NeedsReload,
                "Sharing hit an internal error and cannot continue. Save a copy, then reload the document from the server."
                    .to_owned(),
                ProblemAction::Reload,
            ),
            other => (StopReason::NeedsReload, format!("Sharing stopped: {other}"), ProblemAction::Reload),
        };
        self.out.push_back(Output::Close);
        self.enter_stopped(reason, Some(Problem::local(text, action, true)));
    }

    fn expire_peers(&mut self, now_ms: u64) {
        if !self.peers.expire(now_ms).is_empty() {
            self.presence_changed = true;
        }
        if self.peers.next_trail_expiry(self.last_tick_ms).is_some_and(|t| t <= now_ms) {
            self.presence_changed = true;
        }
    }

    fn next_random(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
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
            || self.uploads.want_flush
            || self.uploads.seed.is_some()
            || self.images.uploads_pending();
        let (state, stopped, text, retry_at_ms) = match self.phase {
            Phase::Idle => return Status::idle(),
            Phase::Backoff { until } => {
                let hint = if self.offline_reason.is_empty() { "Offline" } else { self.offline_reason.as_str() };
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
                let text = match (&self.problem, self.expiring, unsynced) {
                    (Some(p), ..) => p.text.clone(),
                    (None, true, _) => {
                        "This room is past its time limit and will be deleted when the last person leaves. Save a copy."
                            .to_owned()
                    }
                    (None, false, true) => "Connected. Sharing your latest changes.".to_owned(),
                    (None, false, false) => "Connected. All changes are shared.".to_owned(),
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
