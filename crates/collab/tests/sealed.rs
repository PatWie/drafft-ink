#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The process variant uses real sockets and the real clock.
#![allow(clippy::disallowed_methods)]
//! The client of an encrypted room against the relay: the same scenarios on the in-memory loopback (the real relay
//! core, simulated time) and against the real `drafftink-headless serve` process (real sockets, real time).

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::live::{PATIENCE, Server, Waker};
use common::{ids, rect};
use drafftink_collab::{
    CollabEvent, CollabSession, ConnState, LoopbackServer, NativeTransport, Resume, SealedClient, SealedConfig,
    ServerUrl, StopReason, Transport, TransportEvent, UrlPolicy, loopback,
};
use drafftink_doc::LoroStore;
use drafftink_protocol::ClientId;
use drafftink_protocol::UserInfo;
use drafftink_protocol::v2::{JoinMode, RoomState};
use drafftink_relay::{ConnId, Effect, MemoryBlobStore, MemoryRelayStore, Relay, RelayConfig, Retention};
use drafftink_scene::{SceneStore, SceneTx};
use drafftink_seal::RoomKey;

const KEY: [u8; 32] = [7; 32];

fn key() -> RoomKey {
    RoomKey::from_bytes(KEY)
}

/// A transport that can be a loopback or a socket.
struct Dyn(Box<dyn Transport>);

impl Transport for Dyn {
    fn open(&mut self, url: &ServerUrl) -> Result<(), drafftink_collab::TransportError> {
        self.0.open(url)
    }
    fn send(&mut self, frame: Vec<u8>) -> Result<(), drafftink_collab::TransportError> {
        self.0.send(frame)
    }
    fn send_text(&mut self, text: &str) -> Result<(), drafftink_collab::TransportError> {
        self.0.send_text(text)
    }
    fn close(&mut self) {
        self.0.close();
    }
    fn poll(&mut self) -> Option<TransportEvent> {
        self.0.poll()
    }
}

struct Peer {
    n: u8,
    session: CollabSession<Dyn>,
    store: LoroStore,
    events: Vec<CollabEvent>,
}

impl Peer {
    fn client(&self) -> &SealedClient {
        self.session.client().sealed().expect("a sealed client")
    }

    fn add(&mut self, id: &str, now: u64) {
        let mut tx = self.store.begin();
        tx.insert(rect(id)).unwrap();
        tx.commit().unwrap();
        self.session.client_mut().local_changed(now);
    }

    fn ids(&self) -> Vec<String> {
        let mut ids = ids(&self.store);
        ids.sort();
        ids
    }

    fn stopped(&self) -> Option<StopReason> {
        self.client().status().stopped
    }

    fn online(&self) -> bool {
        self.client().status().state == ConnState::Online
    }

    fn settled(&self) -> bool {
        self.online() && !self.client().status().unsynced
    }
}

/// Where the scenarios run.
trait Rig {
    fn now(&self) -> u64;
    fn url(&self) -> ServerUrl;
    fn transport(&mut self) -> Box<dyn Transport>;
    /// Drives the peers until `done` holds. A bound ends a hung test.
    fn drive(&mut self, peers: &mut [&mut Peer], what: &str, done: &dyn Fn(&[&mut Peer]) -> bool);
    /// Lets `ms` pass for the room, with the peers driven.
    fn pass(&mut self, peers: &mut [&mut Peer], ms: u64);
    /// How long a room with nobody in it takes to be deleted on this rig, with a margin.
    fn expiry_ms(&self) -> u64;
}

fn peer(rig: &mut dyn Rig, n: u8, mode: JoinMode, store: Option<LoroStore>, resume: Resume) -> Peer {
    let cfg = SealedConfig {
        url: rig.url(),
        key: key(),
        client_id: ClientId::from_bytes([n; 16]),
        user: Some(UserInfo { name: format!("user{n}"), color: 0x336699 }),
        mode,
        resume,
        jitter_seed: u64::from(n) * 7919,
        replace_when_trimmed: false,
    };
    let transport = Dyn(rig.transport());
    let mut client = SealedClient::new(cfg).unwrap();
    client.start(rig.now());
    let store = store.unwrap_or_else(|| {
        LoroStore::with_config(drafftink_doc::LoroConfig { peer_id: Some(u64::from(n) + 1000), ..Default::default() })
            .unwrap()
    });
    Peer { n, session: CollabSession::new(client, transport), store, events: Vec::new() }
}

fn pump(p: &mut Peer, now: u64) -> Option<u64> {
    let out = p.session.pump(now, &mut p.store);
    p.events.extend(out.events);
    out.next_wake_ms
}

// ---- the loopback rig: the relay core, simulated time ----

struct Loop {
    relay: Relay<MemoryRelayStore, MemoryBlobStore>,
    store: MemoryRelayStore,
    links: Vec<(Arc<LoopbackServer>, usize, Option<ConnId>)>,
    now: u64,
    next_conn: u64,
    ttl_ms: u64,
    wake_at: Option<u64>,
    relay_advice: usize,
    /// Every frame that crossed the wire, in both directions.
    wire: Vec<Vec<u8>>,
}

impl Loop {
    fn new(config: RelayConfig) -> Loop {
        let ttl_ms = match config.retention {
            Retention::After { ttl, grace, .. } => u64::try_from((ttl + grace).as_millis()).unwrap() + 1000,
            Retention::Never => 0,
        };
        let store = MemoryRelayStore::new();
        let relay = Relay::new(key().room_id(), config, store.clone(), MemoryBlobStore::new());
        Loop {
            relay,
            store,
            links: Vec::new(),
            now: 1_000_000,
            next_conn: 0,
            ttl_ms,
            wake_at: None,
            relay_advice: 0,
            wire: Vec::new(),
        }
    }

    fn effects(&mut self, out: drafftink_relay::Output) {
        self.wake_at = out.wake_at;
        for effect in out.effects {
            let find = |links: &[(Arc<LoopbackServer>, usize, Option<ConnId>)], conn| {
                links.iter().position(|l| l.2 == Some(conn))
            };
            match effect {
                Effect::Send { conn, frame } => {
                    self.wire.push(frame.to_vec());
                    if matches!(
                        drafftink_protocol::v2::ServerMessage::decode(&frame),
                        Ok(drafftink_protocol::v2::ServerMessage::CompactAdvice { .. })
                    ) {
                        self.relay_advice += 1;
                    }
                    if let Some(i) = find(&self.links, conn) {
                        self.links[i].0.push_raw(frame.to_vec());
                    }
                }
                Effect::Text { conn, text } => {
                    if let Some(i) = find(&self.links, conn) {
                        self.links[i].0.push_text(text);
                    }
                }
                Effect::Close { conn } => {
                    if let Some(i) = find(&self.links, conn) {
                        self.links[i].0.close_cleanly();
                        self.links[i].2 = None;
                    }
                }
            }
        }
    }

    fn step_relay(&mut self) {
        let now = self.now;
        for i in 0..self.links.len() {
            let end = Arc::clone(&self.links[i].0);
            if end.opens() != self.links[i].1 {
                if let Some(old) = self.links[i].2.take() {
                    let out = self.relay.on_closed(old, now);
                    self.effects(out);
                }
                self.next_conn += 1;
                let conn = ConnId(self.next_conn);
                self.links[i].1 = end.opens();
                self.links[i].2 = Some(conn);
                let out = self.relay.connect(conn, now);
                self.effects(out);
            }
            let Some(conn) = self.links[i].2 else { continue };
            if !end.is_open() {
                self.links[i].2 = None;
                let out = self.relay.on_closed(conn, now);
                self.effects(out);
                continue;
            }
            while let Some(frame) = end.recv_raw() {
                self.wire.push(frame.clone());
                let out = self.relay.on_frame(conn, now, &frame);
                self.effects(out);
            }
            for text in end.take_texts() {
                let out = self.relay.on_text(conn, now, &text);
                self.effects(out);
            }
        }
        if self.wake_at.is_some_and(|at| at <= now) {
            let out = self.relay.on_wake(now);
            self.effects(out);
        }
    }

    fn round(&mut self, peers: &mut [&mut Peer], step_ms: u64) {
        self.now += step_ms;
        for p in peers.iter_mut() {
            pump(p, self.now);
        }
        self.step_relay();
    }
}

impl Rig for Loop {
    fn now(&self) -> u64 {
        self.now
    }
    fn url(&self) -> ServerUrl {
        ServerUrl::parse("ws://127.0.0.1:3030/ws", UrlPolicy::default()).unwrap()
    }
    fn transport(&mut self) -> Box<dyn Transport> {
        let (transport, server) = loopback();
        server.set_auto_accept(true);
        self.links.push((Arc::new(server), 0, None));
        Box::new(transport)
    }
    fn drive(&mut self, peers: &mut [&mut Peer], what: &str, done: &dyn Fn(&[&mut Peer]) -> bool) {
        for _ in 0..2000 {
            self.round(peers, 40);
            if done(peers) {
                return;
            }
        }
        panic!(
            "gave up waiting for {what}: {:?}",
            peers
                .iter()
                .map(|p| (p.n, p.client().status().state, p.client().status().text.clone()))
                .collect::<Vec<_>>()
        );
    }
    fn pass(&mut self, peers: &mut [&mut Peer], ms: u64) {
        let target = self.now + ms;
        while self.now < target {
            // Jump to the next thing that happens, but never past what the peers need to keep their connection alive.
            let step = (target - self.now).min(1_000);
            self.round(peers, step);
        }
    }
    fn expiry_ms(&self) -> u64 {
        self.ttl_ms
    }
}

// ---- the process rig: `drafftink-headless serve`, real time ----

struct Process {
    server: Server,
    waker: Arc<Waker>,
    started: Instant,
    ttl_ms: u64,
}

impl Process {
    fn new() -> Process {
        let server = Server::start(&["--room-ttl", "2s", "--room-grace", "1s"]);
        Process { server, waker: Arc::new(Waker::default()), started: Instant::now(), ttl_ms: 3000 + 1500 }
    }
}

impl Rig for Process {
    fn now(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap()
    }
    fn url(&self) -> ServerUrl {
        ServerUrl::parse(&self.server.url(), UrlPolicy::default()).unwrap()
    }
    fn transport(&mut self) -> Box<dyn Transport> {
        let waker = Arc::clone(&self.waker);
        Box::new(NativeTransport::new(Arc::new(move || waker.wake())))
    }
    fn drive(&mut self, peers: &mut [&mut Peer], what: &str, done: &dyn Fn(&[&mut Peer]) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let now = self.now();
            let wakes: Vec<Option<u64>> = peers.iter_mut().map(|p| pump(p, now)).collect();
            if done(peers) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            let wait =
                wakes.into_iter().flatten().map(|w| w.saturating_sub(now)).min().map_or(100, |ms| ms.clamp(1, 100));
            self.waker.wait(Duration::from_millis(wait));
        }
    }
    fn pass(&mut self, peers: &mut [&mut Peer], ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            let now = self.now();
            for p in peers.iter_mut() {
                pump(p, now);
            }
            self.waker.wait(Duration::from_millis(50));
        }
    }
    fn expiry_ms(&self) -> u64 {
        self.ttl_ms
    }
}

fn demo_like() -> RelayConfig {
    RelayConfig {
        retention: Retention::After {
            ttl: Duration::from_secs(2),
            grace: Duration::from_secs(1),
            recheck: Duration::from_secs(3600),
        },
        flush: drafftink_relay::FlushRules { interval_ms: 0, ..RelayConfig::self_host().flush },
        ..RelayConfig::self_host()
    }
}

// ---- the scenarios ----

fn join_and_edit(rig: &mut dyn Rig) {
    let mut a = peer(rig, 1, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a], "the first person to be online", &|p| p[0].online());
    assert_eq!(a.client().room_state(), Some(RoomState::Created));
    let now = rig.now();
    a.add("a1", now);
    let mut b = peer(rig, 2, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "b to see a's element", &|p| p[1].ids() == ["a1"] && p[0].settled());
    let now = rig.now();
    b.add("b1", now);
    rig.drive(&mut [&mut a, &mut b], "a to see b's element", &|p| p[0].ids() == ["a1", "b1"] && p[1].settled());
    assert_eq!(a.client().peers().len(), 1);
    assert!(b.client().resume().last_seq >= 2, "{:?}", b.client().resume());
}

fn offline_edits_merge(rig: &mut dyn Rig) {
    let mut a = peer(rig, 1, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a], "a online", &|p| p[0].online());
    let now = rig.now();
    a.add("a1", now);
    rig.drive(&mut [&mut a], "a1 stored", &|p| p[0].settled());
    // b drew before it ever connected, and a keeps editing meanwhile.
    let mut b = peer(rig, 2, JoinMode::Open, None, Resume::default());
    b.add("b1", 0);
    let now = rig.now();
    a.add("a2", now);
    rig.drive(&mut [&mut a, &mut b], "both to hold all three", &|p| {
        p[0].ids() == ["a1", "a2", "b1"] && p[1].ids() == ["a1", "a2", "b1"] && p[0].settled() && p[1].settled()
    });
}

fn expired_room_is_restored_by_one_copy_only(rig: &mut dyn Rig) {
    let mut a = peer(rig, 1, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a], "a online", &|p| p[0].online());
    let now = rig.now();
    a.add("a1", now);
    rig.drive(&mut [&mut a], "a1 stored", &|p| p[0].settled());
    let snapshot = a.store.export_snapshot().unwrap();
    let resume = a.client().resume();
    let now = rig.now();
    a.session.client_mut().leave(now, &mut a.store);
    pump(&mut a, now);
    drop(a);
    rig.pass(&mut [], rig.expiry_ms());
    // Somebody opens the link with no copy: the room is gone and nothing is made.
    let mut empty = peer(rig, 3, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut empty], "the refusal", &|p| p[0].stopped().is_some());
    assert_eq!(empty.stopped(), Some(StopReason::RoomMissing));
    assert!(empty.client().status().text.contains("This room is gone"), "{}", empty.client().status().text);
    // Two holders of the same cached copy race to restore it.
    let copy = |n: u8| {
        LoroStore::from_snapshot_with(
            &snapshot,
            drafftink_doc::LoroConfig { peer_id: Some(u64::from(n) + 1000), ..Default::default() },
        )
        .unwrap()
    };
    let mut b = peer(rig, 4, JoinMode::Create, Some(copy(4)), resume.clone());
    let mut c = peer(rig, 5, JoinMode::Create, Some(copy(5)), resume);
    rig.drive(&mut [&mut b, &mut c], "both online and settled", &|p| p[0].settled() && p[1].settled());
    let created =
        [b.client().room_state(), c.client().room_state()].iter().filter(|s| **s == Some(RoomState::Created)).count();
    assert!(created >= 1, "one of them made the room");
    assert_eq!(
        (b.ids(), c.ids()),
        (vec!["a1".to_owned()], vec!["a1".to_owned()]),
        "the copy is the room, and nothing is duplicated"
    );
    // A newcomer sees the restored content.
    let mut d = peer(rig, 6, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut b, &mut c, &mut d], "d to see the restored room", &|p| p[2].ids() == ["a1"]);
}

#[test]
fn join_and_edit_on_the_loopback() {
    join_and_edit(&mut Loop::new(RelayConfig::self_host()));
}

#[test]
fn join_and_edit_against_the_server_process() {
    join_and_edit(&mut Process::new());
}

#[test]
fn offline_edits_merge_on_the_loopback() {
    offline_edits_merge(&mut Loop::new(RelayConfig::self_host()));
}

#[test]
fn offline_edits_merge_against_the_server_process() {
    offline_edits_merge(&mut Process::new());
}

#[test]
fn an_expired_room_is_restored_by_one_copy_on_the_loopback() {
    expired_room_is_restored_by_one_copy_only(&mut Loop::new(demo_like()));
}

#[test]
fn an_expired_room_is_restored_by_one_copy_against_the_server_process() {
    expired_room_is_restored_by_one_copy_only(&mut Process::new());
}

// ---- only the loopback: it needs the relay's internals or time that real clocks do not give ----

#[test]
fn the_relay_compacts_on_advice_and_a_joiner_converges_from_the_snapshot() {
    let mut config = RelayConfig::self_host();
    config.flush.interval_ms = 0;
    config.compact.records = 6;
    config.compact.join_records = 1000;
    let mut rig = Loop::new(config);
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    let mut b = peer(&mut rig, 2, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "both online", &|p| p[0].online() && p[1].online());
    for i in 0..10 {
        let now = rig.now();
        a.add(&format!("a{i}"), now);
        rig.pass(&mut [&mut a, &mut b], 200);
    }
    rig.pass(&mut [&mut a, &mut b], 20_000);
    let advised = rig.relay_advice;
    assert!(advised >= 1);
    let meta = drafftink_relay::RelayStore::load_meta(&mut rig.store.clone()).unwrap().unwrap();
    assert!(
        meta.has_snapshot && meta.snapshot_seq > 0,
        "a client compacted: {meta:?} a={:?} b={:?} {:?}",
        a.client().resume(),
        b.client().resume(),
        a.client().status()
    );
    assert!(rig.store.segment_count() < 10, "the segments below the snapshot are gone");
    let mut c = peer(&mut rig, 3, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b, &mut c], "c to converge", &|p| p[2].ids().len() == 10);
    assert_eq!(c.ids(), a.ids());
}

#[test]
fn a_reconnect_resumes_from_the_last_sequence_number_and_uploads_what_the_relay_lacks() {
    let mut rig = Loop::new(RelayConfig::self_host());
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    let mut b = peer(&mut rig, 2, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "both online", &|p| p[0].online() && p[1].online());
    let now = rig.now();
    a.add("a1", now);
    rig.drive(&mut [&mut a, &mut b], "b sees a1", &|p| p[1].ids() == ["a1"] && p[0].settled());
    // b loses its connection, edits offline, and a edits too.
    rig.links[1].0.drop_connection("the network went away");
    let now = rig.now();
    b.add("b1", now);
    a.add("a2", now);
    rig.drive(&mut [&mut a, &mut b], "everything to converge", &|p| {
        p[0].ids() == ["a1", "a2", "b1"] && p[1].ids() == ["a1", "a2", "b1"] && p[0].settled() && p[1].settled()
    });
    assert!(b.client().stats().reconnects >= 1);
}

#[test]
fn the_wrong_key_is_unauthorized_and_stops_the_client() {
    let mut rig = Loop::new(RelayConfig::self_host());
    // The link names the right room only for the right key: a different key makes another room id, which this relay
    // does not serve.
    let cfg = SealedConfig {
        url: rig.url(),
        key: RoomKey::from_bytes([9; 32]),
        client_id: ClientId::from_bytes([1; 16]),
        user: None,
        mode: JoinMode::Create,
        resume: Resume::default(),
        jitter_seed: 1,
        replace_when_trimmed: false,
    };
    let mut p = Peer {
        n: 1,
        session: CollabSession::new(SealedClient::new(cfg).unwrap(), Dyn(rig.transport())),
        store: LoroStore::new(),
        events: Vec::new(),
    };
    p.session.client_mut().start(rig.now());
    rig.drive(&mut [&mut p], "the refusal", &|p| p[0].stopped().is_some());
    assert_eq!(p.stopped(), Some(StopReason::Unauthorized));
    assert!(p.client().status().text.contains("does not open this room"), "{}", p.client().status().text);
}

#[test]
fn retention_reaches_the_client_and_a_visit_clears_it() {
    let mut rig = Loop::new(demo_like());
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a], "online", &|p| p[0].online());
    assert!(a.client().expires_at_ms() > rig.now(), "the deadline is known");
    // a stays past the deadline: the relay says so, once.
    rig.pass(&mut [&mut a], 2_100);
    rig.drive(&mut [&mut a], "the notice", &|p| p[0].client().expiring());
    let tail = a.events.iter().filter(|e| matches!(e, CollabEvent::Retention { expiring: true, .. })).count();
    assert_eq!(tail, 1);
    assert!(a.client().expiring());
    assert!(a.client().status().text.contains("past its time limit"));
    let mut b = peer(&mut rig, 2, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "a to hear of the new visit", &|p| !p[0].client().expiring() && p[1].online());
}

#[test]
fn hostile_frames_never_panic_and_the_client_reconnects() {
    let mut rig = Loop::new(RelayConfig::self_host());
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a], "online", &|p| p[0].online());
    let mut seed = 99u64;
    for _ in 0..300 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let len = usize::try_from(seed >> 58).unwrap() + 1;
        let bytes: Vec<u8> = (0..len).map(|i| u8::try_from((seed >> (i % 7 * 8)) & 0xFF).unwrap()).collect();
        let now = rig.now();
        a.session.client_mut().on_event(now, TransportEvent::Message(bytes), &mut a.store);
        a.session.client_mut().on_event(now, TransportEvent::Text("pong".to_owned()), &mut a.store);
        rig.round(&mut [&mut a], 40);
    }
    rig.drive(&mut [&mut a], "back online", &|p| p[0].online());
}

#[test]
fn a_record_of_the_wrong_room_or_a_damaged_record_is_skipped_once_and_the_document_is_untouched() {
    let mut rig = Loop::new(RelayConfig::self_host());
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    let mut b = peer(&mut rig, 2, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "both online", &|p| p[0].online() && p[1].online());
    // The relay hands b a record that was sealed under another key (a hostile member, or a damaged store).
    let other = drafftink_seal::RoomKeys::new(&RoomKey::from_bytes([1; 32]));
    let now = rig.now();
    for seq in 1..=3 {
        let header = drafftink_protocol::v2::RecordHeader::new(
            drafftink_protocol::v2::RecordKind::Update,
            ClientId::from_bytes([1; 16]),
            seq,
            0,
        )
        .unwrap();
        let record = other.seal(header, b"not for this room").unwrap().into_bytes();
        let stored =
            drafftink_protocol::v2::StoredRecord { seq, from: ClientId::from_bytes([1; 16]), ctr: seq, record };
        let frame = drafftink_protocol::v2::ServerMessage::Record(stored).encode().unwrap();
        b.session.client_mut().on_event(now, TransportEvent::Message(frame), &mut b.store);
    }
    pump(&mut b, now);
    assert_eq!(b.client().skipped(), 3);
    let problems = b
        .events
        .iter()
        .filter(|e| matches!(e, CollabEvent::Problem(p) if p.text.contains("could not be read")))
        .count();
    assert_eq!(problems, 1, "shown once, not once per record");
    assert!(b.ids().is_empty());
}

#[test]
fn the_relay_sees_nothing_it_can_read() {
    let mut rig = Loop::new(RelayConfig::self_host());
    let marker = "CANARY-7f3a91c2";
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    let mut b = peer(&mut rig, 2, JoinMode::Open, None, Resume::default());
    a.session.client_mut().set_user(Some(UserInfo { name: "Zelda-Canary".to_owned(), color: 1 }));
    rig.drive(&mut [&mut a, &mut b], "both online", &|p| p[0].online() && p[1].online());
    let now = rig.now();
    a.add(marker, now);
    a.session.client_mut().set_cursor(now, Some(drafftink_protocol::Point { x: 1.0, y: 2.0 }));
    rig.drive(&mut [&mut a, &mut b], "b to see the drawing", &|p| p[1].ids() == [marker.to_owned()] && p[0].settled());
    rig.pass(&mut [&mut a, &mut b], 3_000);
    assert!(rig.wire.len() > 10, "frames were recorded: {}", rig.wire.len());
    let secrets = secrets_of(&key());
    let mut stores = rig.store.dump();
    stores.extend(rig.wire.iter().cloned());
    for (what, needle) in std::iter::once(("the marker", marker.as_bytes().to_vec()))
        .chain([("the user name", b"Zelda-Canary".to_vec())])
        .chain(secrets)
    {
        for haystack in &stores {
            assert!(!haystack.windows(needle.len()).any(|w| w == needle.as_slice()), "{what} is visible to the relay");
        }
    }
    // The membership token is the one secret the relay sees, and only in the join frame, not in storage.
    let token = key().membership_token();
    let token = token.expose_bytes().to_vec();
    assert!(rig.store.dump().iter().all(|d| !d.windows(32).any(|w| w == token.as_slice())));
}

/// Every form of the key and of what is derived from it that a relay must never hold, except the membership token.
fn secrets_of(key: &RoomKey) -> Vec<(&'static str, Vec<u8>)> {
    let raw = key.expose_bytes().to_vec();
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let url = drafftink_seal::RoomLink::new(RoomKey::from_bytes(*key.expose_bytes()));
    let base64url = url.fragment().rsplit(',').next().unwrap_or_default().to_owned();
    let mut out = vec![
        ("the key", raw.clone()),
        ("the key in hex", hex.into_bytes()),
        ("the key in base64", base64(&raw).into_bytes()),
        ("the key in base64url", base64url.into_bytes()),
    ];
    for purpose in [
        drafftink_seal::Purpose::Content,
        drafftink_seal::Purpose::Presence,
        drafftink_seal::Purpose::Image,
        drafftink_seal::Purpose::ImageName,
    ] {
        out.push(("a subkey", key.subkey(purpose).expose_bytes().to_vec()));
    }
    out
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}
