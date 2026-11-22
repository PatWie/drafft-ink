//! A harness for the relay core: connections, a clock the test moves, and decoded replies. Records are fakes: a
//! real header and filler bytes, because the relay reads nothing else.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use drafftink_protocol::v2::{
    ClientMessage, ErrorCode, JoinMode, MembershipToken, Proof, RecordHeader, RecordKind, RoomId, ServerMessage,
    StoredRecord,
};
use drafftink_protocol::{ClientId, SEAL_OVERHEAD};
use drafftink_relay::{
    ConnId, Effect, MemoryBlobStore, MemoryRelayStore, Output, Relay, RelayConfig, Retention, StoreOp,
};

pub const SECOND: u64 = 1000;
pub const HOUR: u64 = 3_600 * SECOND;

pub fn token() -> MembershipToken {
    MembershipToken::from_bytes([9; 32])
}

pub fn room_id() -> RoomId {
    RoomId::from_token(&token())
}

pub fn proof() -> Proof {
    Proof::token(&token())
}

pub fn client(n: u8) -> ClientId {
    ClientId::from_bytes([n; 16])
}

/// A fake sealed record: a valid header and `filler` bytes after the minimum size.
pub fn record(kind: RecordKind, sender: ClientId, ctr: u64, covers: u64, filler: usize) -> Vec<u8> {
    let header = RecordHeader::new(kind, sender, ctr, covers).expect("header");
    let mut bytes = header.to_bytes().to_vec();
    bytes.resize(SEAL_OVERHEAD + filler, 0xAB);
    bytes
}

pub fn update(sender: ClientId, ctr: u64) -> Vec<u8> {
    record(RecordKind::Update, sender, ctr, 0, 10)
}

pub fn snapshot(sender: ClientId, ctr: u64, covers: u64) -> Vec<u8> {
    record(RecordKind::Snapshot, sender, ctr, covers, 100)
}

pub fn presence(sender: ClientId) -> Vec<u8> {
    record(RecordKind::Presence, sender, 1, 0, 20)
}

pub fn join_message(client_id: ClientId, mode: JoinMode, last_seq: u64) -> ClientMessage {
    ClientMessage::Join {
        min_version: 2,
        max_version: 2,
        room: room_id(),
        client_id,
        mode,
        proof: proof(),
        last_seq,
        captcha: None,
    }
}

pub fn demo() -> RelayConfig {
    RelayConfig {
        flush: drafftink_relay::FlushRules { interval_ms: SECOND, max_bytes: 256 * 1024, retry_ms: SECOND },
        ..RelayConfig::demo()
    }
}

pub struct Harness {
    pub relay: Relay<MemoryRelayStore, MemoryBlobStore>,
    pub store: MemoryRelayStore,
    pub blobs: MemoryBlobStore,
    pub now: u64,
    pub inbox: BTreeMap<ConnId, Vec<ServerMessage>>,
    pub closed: BTreeSet<ConnId>,
    pub ops: Vec<StoreOp>,
    pub texts: Vec<(ConnId, &'static str)>,
    pub wake_at: Option<u64>,
    pub deleted: bool,
    /// Connections whose client pings every 20 s while the clock moves.
    pub pingers: Vec<ConnId>,
    next_conn: u64,
}

impl Harness {
    pub fn new(cfg: RelayConfig) -> Self {
        let store = MemoryRelayStore::new();
        let blobs = MemoryBlobStore::new();
        Self::on(cfg, store, blobs)
    }

    /// A relay on stores that already hold a room (a restart).
    pub fn on(cfg: RelayConfig, store: MemoryRelayStore, blobs: MemoryBlobStore) -> Self {
        let relay = Relay::new(room_id(), cfg, store.clone(), blobs.clone());
        Harness {
            relay,
            store,
            blobs,
            now: 1_000_000,
            inbox: BTreeMap::new(),
            closed: BTreeSet::new(),
            ops: Vec::new(),
            texts: Vec::new(),
            wake_at: None,
            deleted: false,
            pingers: Vec::new(),
            next_conn: 0,
        }
    }

    pub fn self_host() -> Self {
        Self::new(RelayConfig { retention: Retention::Never, ..demo() })
    }

    fn absorb(&mut self, out: Output) {
        self.ops.extend(out.store_ops);
        self.wake_at = out.wake_at;
        self.deleted |= out.deleted;
        for effect in out.effects {
            match effect {
                Effect::Send { conn, frame } => {
                    let message = ServerMessage::decode(&frame).expect("the relay sends frames it can decode");
                    self.inbox.entry(conn).or_default().push(message);
                }
                Effect::Text { conn, text } => self.texts.push((conn, text)),
                Effect::Close { conn } => {
                    self.closed.insert(conn);
                }
            }
        }
    }

    pub fn connect(&mut self) -> ConnId {
        self.next_conn += 1;
        let conn = ConnId(self.next_conn);
        let out = self.relay.connect(conn, self.now);
        self.absorb(out);
        conn
    }

    pub fn send(&mut self, conn: ConnId, message: &ClientMessage) {
        let frame = message.encode().expect("encodes");
        self.send_raw(conn, &frame);
    }

    pub fn send_raw(&mut self, conn: ConnId, frame: &[u8]) {
        let out = self.relay.on_frame(conn, self.now, frame);
        self.absorb(out);
    }

    pub fn close(&mut self, conn: ConnId) {
        let out = self.relay.on_closed(conn, self.now);
        self.absorb(out);
    }

    /// Connects and joins. Returns the connection.
    pub fn join(&mut self, client_id: ClientId, mode: JoinMode, last_seq: u64) -> ConnId {
        let conn = self.connect();
        self.send(conn, &join_message(client_id, mode, last_seq));
        conn
    }

    pub fn open(&mut self, n: u8) -> ConnId {
        self.join(client(n), JoinMode::Create, 0)
    }

    /// Moves the clock and runs every wake that falls due on the way. Clients in `pingers` ping every 20 s.
    pub fn advance(&mut self, ms: u64) {
        let target = self.now + ms;
        while self.now < target && !self.pingers.is_empty() {
            let step = (target - self.now).min(20 * SECOND);
            self.run_to(self.now + step);
            for conn in self.pingers.clone() {
                if !self.closed.contains(&conn) {
                    let out = self.relay.on_text(conn, self.now, "ping");
                    self.absorb(out);
                }
            }
        }
        self.run_to(target);
    }

    fn run_to(&mut self, target: u64) {
        while let Some(at) = self.wake_at.filter(|at| *at <= target) {
            self.now = at.max(self.now);
            let out = self.relay.on_wake(self.now);
            let before = self.wake_at;
            self.absorb(out);
            assert!(
                self.wake_at != before || self.wake_at.is_none_or(|w| w > self.now),
                "a wake that does not move on would loop"
            );
        }
        self.now = target.max(self.now);
        let out = self.relay.on_wake(self.now);
        self.absorb(out);
    }

    /// Runs the wake that is due now, if any.
    pub fn tick(&mut self) {
        self.advance(0);
    }

    pub fn take(&mut self, conn: ConnId) -> Vec<ServerMessage> {
        self.inbox.remove(&conn).unwrap_or_default()
    }

    pub fn is_closed(&self, conn: ConnId) -> bool {
        self.closed.contains(&conn)
    }

    pub fn rows_written(&self) -> usize {
        self.ops.iter().filter(|op| !matches!(op, StoreOp::PutImage { .. })).count()
    }
}

pub fn error_code(messages: &[ServerMessage]) -> Option<ErrorCode> {
    messages.iter().find_map(|m| match m {
        ServerMessage::Error { code, .. } => Some(*code),
        _ => None,
    })
}

pub fn acks(messages: &[ServerMessage]) -> Vec<(u64, u64)> {
    messages
        .iter()
        .filter_map(|m| if let ServerMessage::Ack { ctr, seq } = m { Some((*ctr, *seq)) } else { None })
        .collect()
}

pub fn records(messages: &[ServerMessage]) -> Vec<StoredRecord> {
    messages.iter().filter_map(|m| if let ServerMessage::Record(r) = m { Some(r.clone()) } else { None }).collect()
}

pub fn catchups(messages: &[ServerMessage]) -> Vec<&ServerMessage> {
    messages.iter().filter(|m| matches!(m, ServerMessage::Catchup { .. })).collect()
}

/// The sequence numbers of the records in the first catch-up of `messages`.
pub fn catchup_seqs(messages: &[ServerMessage]) -> Vec<u64> {
    match catchups(messages).first() {
        Some(ServerMessage::Catchup { records, .. }) => records.iter().map(|r| r.seq).collect(),
        _ => Vec::new(),
    }
}
