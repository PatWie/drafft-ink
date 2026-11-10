#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Helpers shared by the collab integration tests.

pub mod live;

use drafftink_collab::*;
use drafftink_doc::{LoroConfig, LoroStore};
use drafftink_protocol::{ClientId, ClientMessage, Limits, PeerInfo, RoomId, ServerMessage, SyncKind, UserInfo};
use drafftink_scene::*;

pub fn cid(n: u8) -> ClientId {
    ClientId::from_bytes([n; 16])
}

pub fn room() -> RoomId {
    RoomId::new("test-room").unwrap()
}

pub fn user(name: &str) -> UserInfo {
    UserInfo { name: name.to_owned(), color: 0x3366cc }
}

pub fn store(peer: u64) -> LoroStore {
    LoroStore::with_config(LoroConfig { peer_id: Some(peer), ..LoroConfig::default() })
}

pub fn rect(id: &str) -> Element {
    let mut e =
        Element::new(ElementId::new(id).unwrap(), ElementKind::Rectangle, ZIndex::rebalance(1).unwrap().remove(0));
    e.transform = Transform::new(10.0, 20.0, 100.0, 50.0);
    e
}

/// Inserts rectangles in one commit.
pub fn add(s: &mut LoroStore, ids: &[&str]) {
    let mut tx = s.begin();
    for id in ids {
        tx.insert(rect(id)).unwrap();
    }
    tx.commit().unwrap();
}

pub fn ids(s: &LoroStore) -> Vec<String> {
    s.iter_all().map(|e| e.id.to_string()).collect()
}

pub fn config(url: &str) -> CollabConfig {
    CollabConfig {
        url: ServerUrl::parse(url, UrlPolicy::default()).unwrap(),
        room: room(),
        client_id: cid(1),
        user: Some(user("me")),
        auth_token: Some(AuthToken::new("s3cret-token").unwrap()),
        jitter_seed: 7,
        replace_when_trimmed: false,
    }
}

/// A client wired to a scripted server. The harness plays the transport and the server by hand.
pub struct Harness {
    pub client: CollabClient,
    pub store: LoroStore,
    /// A second store that stands in for the server's document.
    pub server: LoroStore,
    pub now: u64,
    pub events: Vec<CollabEvent>,
    pub frames: Vec<ClientMessage>,
    pub connects: usize,
    pub closes: usize,
}

impl Harness {
    pub fn new() -> Self {
        Self::with(config("ws://127.0.0.1:3030"))
    }

    pub fn with(cfg: CollabConfig) -> Self {
        Harness {
            client: CollabClient::new(cfg).unwrap(),
            store: store(1),
            server: store(100),
            now: 1_000,
            events: Vec::new(),
            frames: Vec::new(),
            connects: 0,
            closes: 0,
        }
    }

    /// Moves outputs and events out of the client.
    pub fn drain(&mut self) {
        while let Some(out) = self.client.poll_output() {
            match out {
                Output::Connect(_) => self.connects += 1,
                Output::Close => self.closes += 1,
                Output::Send(bytes) => {
                    self.frames.push(ClientMessage::decode(&bytes).expect("the client sends valid frames"))
                }
            }
        }
        self.events.extend(self.client.take_events());
    }

    /// Ticks, drains, and checks that the client never asks to be woken in the past (that would be a busy loop).
    pub fn tick(&mut self) {
        self.client.tick(self.now, &mut self.store);
        self.drain();
        if let Some(w) = self.client.next_wake_ms() {
            assert!(w > self.now, "next wake {w} is not after now {} (client {:?})", self.now, self.client);
        }
    }

    pub fn advance(&mut self, ms: u64) {
        self.now += ms;
        self.tick();
    }

    /// Advances in steps of `step` until `total` has passed, ticking at each step.
    pub fn run_for(&mut self, total: u64, step: u64) {
        let end = self.now + total;
        while self.now < end {
            self.now = (self.now + step).min(end);
            self.tick();
        }
    }

    pub fn event(&mut self, e: TransportEvent) {
        self.client.on_event(self.now, e, &mut self.store);
        self.drain();
    }

    pub fn feed(&mut self, msg: &ServerMessage) {
        self.event(TransportEvent::Message(msg.encode().unwrap()));
        self.tick();
    }

    pub fn start(&mut self) {
        self.client.start(self.now);
        self.tick();
    }

    pub fn joined(&self, peers: Vec<PeerInfo>, server_version: Vec<u8>) -> ServerMessage {
        ServerMessage::Joined {
            protocol_version: 1,
            server: "test".into(),
            room: room(),
            client_id: cid(1),
            peers,
            server_version,
            limits: Limits::DEFAULT,
        }
    }

    pub fn sync_response(&self) -> ServerMessage {
        self.sync_from(&self.store.version_vector())
    }

    /// What the server would send a client at version `vv`.
    pub fn sync_from(&self, vv: &[u8]) -> ServerMessage {
        ServerMessage::SyncResponse {
            kind: SyncKind::Updates,
            data: self.server.export_updates_since(vv).unwrap(),
            server_version: self.server.version_vector(),
        }
    }

    /// Connect, open, join and sync: the client ends up online.
    pub fn go_online(&mut self, peers: Vec<PeerInfo>) {
        self.start();
        assert_eq!(self.client.status().state, ConnState::Connecting);
        self.event(TransportEvent::Opened);
        let joined = self.joined(peers, self.server.version_vector());
        self.feed(&joined);
        let sync = self.sync_response();
        self.feed(&sync);
        assert_eq!(self.client.status().state, ConnState::Online, "{:?}", self.client.status());
        self.settle_uploads();
        self.take_events();
    }

    /// Plays the server for the updates the client sent so far: applies and acknowledges each, and clears the frames.
    pub fn settle_uploads(&mut self) {
        let frames = self.take_frames();
        self.server_applies(&frames);
        for (seq, _) in Self::updates(&frames) {
            self.feed(&ServerMessage::Ack { seq });
        }
        self.take_frames();
    }

    pub fn take_frames(&mut self) -> Vec<ClientMessage> {
        std::mem::take(&mut self.frames)
    }

    pub fn take_events(&mut self) -> Vec<CollabEvent> {
        std::mem::take(&mut self.events)
    }

    /// The updates among the sent frames, as `(seq, bytes)`.
    pub fn updates(frames: &[ClientMessage]) -> Vec<(u32, Vec<u8>)> {
        frames
            .iter()
            .filter_map(|m| match m {
                ClientMessage::Update { seq, update } => Some((*seq, update.clone())),
                _ => None,
            })
            .collect()
    }

    pub fn awareness(frames: &[ClientMessage]) -> Vec<drafftink_protocol::AwarenessState> {
        frames
            .iter()
            .filter_map(|m| match m {
                ClientMessage::Awareness(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    /// Applies what the client sent to the server store, as the server would.
    pub fn server_applies(&mut self, frames: &[ClientMessage]) {
        for (_, bytes) in Self::updates(frames) {
            self.server.apply_remote(&bytes).unwrap();
        }
    }

    pub fn remote_batches(events: &[CollabEvent]) -> Vec<RemoteBatch> {
        events
            .iter()
            .filter_map(|e| match e {
                CollabEvent::Remote(b) => Some(b.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn problems(events: &[CollabEvent]) -> Vec<Problem> {
        events
            .iter()
            .filter_map(|e| match e {
                CollabEvent::Problem(p) => Some(p.clone()),
                _ => None,
            })
            .collect()
    }
}

pub fn error(code: drafftink_protocol::ErrorCode) -> ServerMessage {
    ServerMessage::Error { code, message: "x".into(), retry_after_ms: None }
}
