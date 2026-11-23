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
    AccessDenial, CollabEvent, CollabSession, ConnState, LoopbackServer, NativeTransport, Resume, SealedClient,
    SealedConfig, ServerUrl, StopReason, Transport, TransportEvent, Unavailable, UrlPolicy, loopback,
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
    let store = store.unwrap_or_else(|| LoroStore::with_config(editor_config(n)).unwrap());
    Peer { n, session: CollabSession::new(client, transport), store, events: Vec::new() }
}

/// The document of an editor: big imports are staged, as the engine configures its tabs.
fn editor_config(n: u8) -> drafftink_doc::LoroConfig {
    drafftink_doc::LoroConfig {
        peer_id: Some(u64::from(n) + 1000),
        stage_import_bytes: 256 * 1024,
        ..Default::default()
    }
}

fn pump(p: &mut Peer, now: u64) -> Option<u64> {
    let out = p.session.pump(now, &mut p.store);
    p.events.extend(out.events);
    // The engine decodes a staged import slice by slice between frames; here it is finished at once.
    while p.store.import_progress().is_some() {
        p.store.import_step(4096).unwrap();
    }
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
        Self::with(&[])
    }

    /// The server with more command line options.
    fn with(extra: &[&str]) -> Process {
        let mut args = vec!["--room-ttl", "2s", "--room-grace", "1s"];
        args.extend_from_slice(extra);
        let server = Server::start(&args);
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
    // At most one of them lost the race for the seed, and it joined the room as the winner made it.
    let lost = b.client().stats().reconnects + c.client().stats().reconnects;
    assert!(lost <= 1, "{:?} {:?}", b.client().stats(), c.client().stats());
    assert_eq!(
        (b.ids(), c.ids()),
        (vec!["a1".to_owned()], vec!["a1".to_owned()]),
        "the copy is the room, and nothing is duplicated"
    );
    // A newcomer sees the restored content.
    let mut d = peer(rig, 6, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut b, &mut c, &mut d], "d to see the restored room", &|p| p[2].ids() == ["a1"]);
}

/// A host that does not delete rooms answers `Forbidden`: the person is told in plain words, nothing is lost, and
/// the client joins again.
fn a_host_that_refuses_to_delete_rooms_says_so_and_keeps_the_room(rig: &mut dyn Rig) {
    let mut a = peer(rig, 1, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a], "a online", &|p| p[0].online());
    let now = rig.now();
    a.add("a1", now);
    rig.drive(&mut [&mut a], "a1 stored", &|p| p[0].settled());
    assert_eq!(a.session.client_mut().delete_room(), Ok(()));
    rig.drive(&mut [&mut a], "the refusal", &|p| {
        p[0].events.iter().any(|e| matches!(e, CollabEvent::Problem(p) if p.text.contains("does not allow deleting")))
    });
    let told: Vec<_> = a
        .events
        .iter()
        .filter_map(|e| match e {
            CollabEvent::Problem(p) => Some((p.text.as_str(), p.action, p.fatal)),
            _ => None,
        })
        .collect();
    assert_eq!(
        told,
        [("This server does not allow deleting rooms.", drafftink_collab::ProblemAction::AskAdmin, false)]
    );
    // The room is as it was: this client joins it again, a newcomer finds the drawing, and nobody stopped.
    rig.drive(&mut [&mut a], "a back online", &|p| p[0].settled());
    let mut b = peer(rig, 2, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "b to see the drawing", &|p| p[1].ids() == ["a1"]);
    assert_eq!((a.stopped(), b.stopped()), (None, None));
    assert!(!a.events.iter().any(|e| matches!(e, CollabEvent::RoomDeleted)));
}

#[test]
fn a_host_that_refuses_to_delete_rooms_says_so_on_the_loopback() {
    a_host_that_refuses_to_delete_rooms_says_so_and_keeps_the_room(&mut Loop::new(RelayConfig::self_host()));
}

#[test]
fn a_host_that_refuses_to_delete_rooms_says_so_against_the_server_process() {
    a_host_that_refuses_to_delete_rooms_says_so_and_keeps_the_room(&mut Process::new());
}

#[test]
fn a_relay_that_deletes_rooms_ends_the_request_with_one_event_and_forgets_everything() {
    let mut rig = Loop::new(RelayConfig { allow_delete_room: true, ..RelayConfig::self_host() });
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a], "a online", &|p| p[0].online());
    let now = rig.now();
    a.add("a1", now);
    rig.drive(&mut [&mut a], "a1 stored", &|p| p[0].settled());
    let mut b = peer(&mut rig, 2, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "b to see a1", &|p| p[1].ids() == ["a1"]);
    assert!(!rig.store.dump().is_empty());
    assert_eq!(a.session.client_mut().delete_room(), Ok(()));
    rig.drive(&mut [&mut a, &mut b], "a to hear that the room is gone", &|p| p[0].stopped().is_some());
    assert_eq!(a.stopped(), Some(StopReason::RoomDeleted));
    assert_eq!(a.events.iter().filter(|e| matches!(e, CollabEvent::RoomDeleted)).count(), 1);
    assert!(a.client().status().text.contains("deleted for everyone"), "{}", a.client().status().text);
    assert_eq!(a.ids(), ["a1"], "the drawing stays in this copy");
    assert!(rig.store.dump().is_empty(), "the relay holds nothing of the room");
    // The other member did not ask: it finds the room gone when it comes back.
    rig.drive(&mut [&mut a, &mut b], "b to find the room gone", &|p| p[1].stopped().is_some());
    assert_eq!(b.stopped(), Some(StopReason::RoomMissing));
    assert!(!b.events.iter().any(|e| matches!(e, CollabEvent::RoomDeleted)));
    // Time passes: the stopped client does not try again.
    rig.pass(&mut [&mut a], 60_000);
    assert_eq!(a.stopped(), Some(StopReason::RoomDeleted));
}

#[test]
fn a_room_cannot_be_deleted_while_offline_or_from_a_plaintext_room() {
    let mut rig = Loop::new(RelayConfig { allow_delete_room: true, ..RelayConfig::self_host() });
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    assert_eq!(a.session.client_mut().delete_room(), Err(drafftink_collab::DeleteRoomError::NotOnline));
    rig.drive(&mut [&mut a], "a online", &|p| p[0].online());
    let mut plain = drafftink_collab::RoomClient::from(
        drafftink_collab::CollabClient::new(common::config("ws://127.0.0.1:3030")).unwrap(),
    );
    assert_eq!(plain.delete_room(), Err(drafftink_collab::DeleteRoomError::NotEncrypted));
    assert_eq!(a.stopped(), None);
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
fn a_private_server_that_turns_the_connection_away_stops_the_client_and_says_what_to_do() {
    for (denial, words) in [
        (
            AccessDenial::Required,
            "This server is private. Open Preferences, Collaboration, and enter the access token for",
        ),
        (AccessDenial::Rejected, "The server did not accept the access token."),
    ] {
        let (transport, server) = loopback();
        server.set_auto_accept(true);
        let cfg = SealedConfig {
            url: ServerUrl::parse("ws://127.0.0.1:3030", UrlPolicy::default()).unwrap(),
            key: key(),
            client_id: ClientId::from_bytes([1; 16]),
            user: None,
            mode: JoinMode::Create,
            resume: Resume::default(),
            jitter_seed: 1,
            replace_when_trimmed: false,
        };
        let mut session = CollabSession::new(SealedClient::new(cfg).unwrap(), transport);
        let mut store = LoroStore::new();
        session.client_mut().start(1_000);
        session.pump(1_000, &mut store);
        server.deny(denial);
        let out = session.pump(1_040, &mut store);
        let client = session.client().sealed().unwrap();
        assert_eq!(client.status().stopped, Some(StopReason::AccessDenied), "{denial:?}");
        assert!(client.status().text.starts_with(words), "{}", client.status().text);
        let told: Vec<_> = out.events.iter().filter(|e| matches!(e, CollabEvent::Problem(p) if p.fatal)).collect();
        assert_eq!(told.len(), 1, "{:?}", out.events);
        // Time passes: no new connection is tried.
        let opens = server.opens();
        session.pump(120_000, &mut store);
        assert_eq!(server.opens(), opens);
    }
}

#[test]
fn a_page_without_a_relay_ends_the_connect_with_plain_words_and_no_retry() {
    let (transport, server) = loopback();
    server.set_auto_accept(true);
    let cfg = SealedConfig {
        url: ServerUrl::parse("wss://drafft.ink/ws", UrlPolicy::default()).unwrap(),
        key: key(),
        client_id: ClientId::from_bytes([1; 16]),
        user: None,
        mode: JoinMode::Create,
        resume: Resume::default(),
        jitter_seed: 1,
        replace_when_trimmed: false,
    };
    let mut session = CollabSession::new(SealedClient::new(cfg).unwrap(), transport);
    let mut store = LoroStore::new();
    session.client_mut().start(1_000);
    session.pump(1_000, &mut store);
    server.unavailable(Unavailable::NoRelay);
    let out = session.pump(1_040, &mut store);
    let client = session.client().sealed().unwrap();
    assert_eq!(client.status().stopped, Some(StopReason::NoRelay));
    let text = client.status().text.clone();
    assert!(text.starts_with("drafft.ink only serves the app and does not host shared rooms."), "{text}");
    assert!(text.contains("saved on this device") && !text.contains("1006"), "{text}");
    assert_eq!(out.events.iter().filter(|e| matches!(e, CollabEvent::Problem(p) if p.fatal)).count(), 1);
    let opens = server.opens();
    session.pump(120_000, &mut store);
    assert_eq!(server.opens(), opens, "asking again would get the same answer");
}

#[test]
fn a_relay_over_its_daily_limit_says_so_and_tries_again_later() {
    let (transport, server) = loopback();
    server.set_auto_accept(true);
    let cfg = SealedConfig {
        url: ServerUrl::parse("wss://canvas.example.com/ws", UrlPolicy::default()).unwrap(),
        key: key(),
        client_id: ClientId::from_bytes([1; 16]),
        user: None,
        mode: JoinMode::Create,
        resume: Resume::default(),
        jitter_seed: 1,
        replace_when_trimmed: false,
    };
    let mut session = CollabSession::new(SealedClient::new(cfg).unwrap(), transport);
    let mut store = LoroStore::new();
    session.client_mut().start(1_000);
    session.pump(1_000, &mut store);
    server.unavailable(Unavailable::DailyLimit { retry_after_ms: Some(60_000) });
    session.pump(1_040, &mut store);
    let client = session.client().sealed().unwrap();
    assert_eq!(client.status().state, ConnState::Reconnecting);
    assert!(client.status().text.starts_with("The server has reached its daily limit"), "{}", client.status().text);
    let opens = server.opens();
    session.pump(30_000, &mut store);
    assert_eq!(server.opens(), opens, "it waits as long as the server said");
    session.pump(70_000, &mut store);
    assert!(server.opens() > opens, "and then tries again");
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

#[test]
fn the_key_is_in_no_debug_output_and_no_status_text() {
    let mut rig = Loop::new(RelayConfig::self_host());
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a], "online", &|p| p[0].online());
    let link = drafftink_seal::RoomLink::new(key());
    let fragment = link.fragment();
    let key_text = fragment.rsplit(',').next().unwrap().to_owned();
    let shown = format!("{:?} {:?} {:?} {:?}", a.client(), a.client().status(), a.events, a.client().resume());
    assert!(!shown.contains(&key_text), "{shown}");
    let cfg = SealedConfig {
        url: rig.url(),
        key: key(),
        client_id: ClientId::from_bytes([1; 16]),
        user: None,
        mode: JoinMode::Open,
        resume: Resume::default(),
        jitter_seed: 1,
        replace_when_trimmed: false,
    };
    assert!(!format!("{cfg:?}").contains(&key_text));
}

// ---- large rooms ----

/// Adds `count` rectangles named `{prefix}{i}` to a store in one commit.
fn fill(store: &mut LoroStore, prefix: &str, count: usize) {
    let mut tx = store.begin();
    for i in 0..count {
        tx.insert(rect(&format!("{prefix}{i:05}"))).unwrap();
    }
    tx.commit().unwrap();
}

fn seeded(n: u8, count: usize) -> LoroStore {
    let mut store = LoroStore::with_config(editor_config(n)).unwrap();
    fill(&mut store, "e", count);
    store
}

/// How the room holds its drawing when the newcomer arrives.
#[derive(Clone, Copy, Debug)]
enum Held {
    /// The first person came with the whole drawing: the relay holds one snapshot and no log.
    Snapshot,
    /// The first person drew all of it in one commit in the empty room: a tiny snapshot of the empty room and one
    /// big record after it.
    OneRecord,
    /// The first person drew batch after batch, a few hundred records and no snapshot: the newcomer gets the log
    /// in pages.
    LogTail,
    /// As `LogTail`, but the relay asked for a compaction, and the newcomer gets a snapshot and a short tail.
    Compacted,
}

/// Records the first person leaves in the room for `LogTail`: just below the 500 that make the relay ask for a
/// compaction, so the newcomer gets all of them in pages.
const LOG_RECORDS: usize = 450;
/// Records for `Compacted`: past the 500 that make the relay ask for a compaction.
const COMPACTING_RECORDS: usize = 520;

/// The drawing of `count` rectangles reaches a newcomer whole, however the relay holds it.
fn large_join(rig: &mut dyn Rig, count: usize, held: Held) {
    let mut a = match held {
        Held::Snapshot => peer(rig, 1, JoinMode::Create, Some(seeded(1, count)), Resume::default()),
        _ => peer(rig, 1, JoinMode::Create, None, Resume::default()),
    };
    rig.drive(&mut [&mut a], "a to be online", &|p| p[0].online());
    match held {
        Held::Snapshot => {}
        Held::OneRecord => {
            fill(&mut a.store, "e", count);
            let now = rig.now();
            a.session.client_mut().local_changed(now);
        }
        Held::LogTail | Held::Compacted => {
            let records = if matches!(held, Held::Compacted) { COMPACTING_RECORDS } else { LOG_RECORDS };
            let per_record = count.div_ceil(records);
            for batch in 0..count.div_ceil(per_record) {
                fill(&mut a.store, &format!("e{batch:03}-"), per_record.min(count - batch * per_record));
                let now = rig.now();
                a.session.client_mut().local_changed(now);
                rig.drive(&mut [&mut a], "a batch to be stored", &|p| p[0].settled());
            }
        }
    }
    rig.drive(&mut [&mut a], "a to be settled", &|p| p[0].settled());
    assert_eq!(a.ids().len(), count);
    if matches!(held, Held::Compacted) {
        rig.drive(&mut [&mut a], "the relay to hold a snapshot", &|p| p[0].client().snapshot_seq() > 0);
    }
    let mut b = peer(rig, 2, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "b to hold the whole room", &|p| {
        p[1].ids().len() == count || p[1].stopped().is_some()
    });
    assert_eq!(b.stopped(), None, "{}", b.client().status().text);
    assert_eq!(b.ids(), a.ids(), "{held:?} {count}");
    rig.drive(&mut [&mut a, &mut b], "b to be settled", &|p| p[1].settled());
    assert!(b.client().status().problem.is_none(), "{held:?} {count}: {:?}", b.client().status().problem);
}

macro_rules! large_join_tests {
    ($($name:ident: $count:expr, $held:expr;)*) => {
        mod large_join {
            use super::*;
            mod loopback {
                use super::*;
                $(#[test] fn $name() { large_join(&mut Loop::new(RelayConfig::self_host()), $count, $held); })*
            }
            mod process {
                use super::*;
                $(#[test] fn $name() { large_join(&mut Process::new(), $count, $held); })*
            }
        }
    };
}

large_join_tests! {
    snapshot_1000: 1000, Held::Snapshot;
    snapshot_3000: 3000, Held::Snapshot;
    snapshot_10000: 10_000, Held::Snapshot;
    one_record_1000: 1000, Held::OneRecord;
    one_record_3000: 3000, Held::OneRecord;
    one_record_10000: 10_000, Held::OneRecord;
    log_tail_1000: 1000, Held::LogTail;
    log_tail_3000: 3000, Held::LogTail;
    log_tail_10000: 10_000, Held::LogTail;
    compacted_1000: 1000, Held::Compacted;
    compacted_3000: 3000, Held::Compacted;
    compacted_10000: 10_000, Held::Compacted;
}

// ---- history the relay does not hold ----

impl Loop {
    /// Puts one sealed update from client `from` into the room's log, as a member that is gone again would have.
    fn plant(&mut self, from: u8, ctr: u64, update: &[u8]) {
        use drafftink_protocol::v2::{ClientMessage, JoinMode, Proof, RecordHeader, RecordKind};
        let sender = ClientId::from_bytes([from; 16]);
        let keys = drafftink_seal::RoomKeys::new(&key());
        let header = RecordHeader::new(RecordKind::Update, sender, ctr, 0).unwrap();
        let record = keys.seal(header, update).unwrap().into_bytes();
        let conn = ConnId(9000 + u64::from(from));
        let now = self.now;
        let join = ClientMessage::Join {
            min_version: 2,
            max_version: 2,
            room: key().room_id(),
            client_id: sender,
            mode: JoinMode::Open,
            proof: Proof::token(&key().membership_token()),
            last_seq: 0,
            captcha: None,
        };
        let out = self.relay.connect(conn, now);
        self.effects(out);
        for message in [join, ClientMessage::Append { record }] {
            let out = self.relay.on_frame(conn, now, &message.encode().unwrap());
            self.effects(out);
        }
        let out = self.relay.on_closed(conn, now);
        self.effects(out);
    }
}

#[test]
fn a_room_whose_log_lacks_the_history_of_a_record_is_said_to_be_incomplete_not_shown_as_empty() {
    let mut rig = Loop::new(RelayConfig::self_host());
    let mut a = peer(&mut rig, 1, JoinMode::Create, None, Resume::default());
    rig.drive(&mut [&mut a], "a online", &|p| p[0].online());
    // Somebody drew two things and only the second update ever reached the relay.
    let mut x = LoroStore::with_config(editor_config(7)).unwrap();
    fill(&mut x, "first", 1);
    let after_first = x.version_vector().unwrap();
    fill(&mut x, "second", 3);
    rig.plant(7, 1, &x.export_updates_since(&after_first).unwrap());

    let mut b = peer(&mut rig, 2, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "b to finish joining", &|p| p[1].online());
    let status = b.client().status();
    assert!(b.ids().is_empty(), "nothing of it can be shown");
    let problem = status.problem.as_ref().expect("the person is told");
    assert!(problem.text.contains("history is missing"), "{}", problem.text);
    assert_eq!(problem.action, drafftink_collab::ProblemAction::SaveCopy);
    assert_eq!(status.text, problem.text, "the status line says it too");
    assert!(b.events.iter().any(|e| matches!(e, CollabEvent::Problem(p) if p == problem)));
}

// ---- a drawing that is over what a room may hold ----

/// The size of the sealed snapshot with which `seeded(1, count)` fills an empty room.
fn seed_record_bytes(count: usize) -> u64 {
    let snapshot = seeded(1, count).export_snapshot().unwrap();
    u64::try_from(snapshot.len() + drafftink_protocol::SEAL_OVERHEAD).unwrap()
}

/// A drawing of `count` rectangles meets a room that holds `limit` bytes. Returns the first person and a newcomer.
fn seed_into_room_of(rig: &mut dyn Rig, count: usize) -> (Peer, Peer) {
    let mut a = peer(rig, 1, JoinMode::Create, Some(seeded(1, count)), Resume::default());
    rig.drive(&mut [&mut a], "a to settle or to be stopped", &|p| p[0].settled() || p[0].stopped().is_some());
    let mut b = peer(rig, 2, JoinMode::Open, None, Resume::default());
    rig.drive(&mut [&mut a, &mut b], "b to be online", &|p| p[1].online());
    (a, b)
}

fn a_room_takes_a_drawing_up_to_its_limit(count: usize, rig: &mut dyn Rig) {
    let (mut a, mut b) = seed_into_room_of(rig, count);
    assert_eq!(a.stopped(), None, "{}", a.client().status().text);
    rig.drive(&mut [&mut a, &mut b], "b to hold the drawing", &|p| p[1].ids().len() == count);
    assert_eq!(b.ids(), a.ids());
}

fn a_room_refuses_a_drawing_over_its_limit_in_plain_words(count: usize, rig: &mut dyn Rig) {
    let (a, b) = seed_into_room_of(rig, count);
    assert_eq!(a.stopped(), Some(StopReason::TooLarge), "{:?}", a.client().status());
    let text = &a.client().status().text;
    assert!(text.contains("holds as much as a room may") && text.contains("Save a copy"), "{text}");
    assert!(
        a.client().status().problem.as_ref().is_some_and(|p| p.action == drafftink_collab::ProblemAction::ReduceSize)
    );
    assert_eq!(a.ids().len(), count, "the drawing stays on this device");
    assert!(b.ids().is_empty(), "the room holds nothing: it was refused, not half taken");
    assert_eq!(b.stopped(), None);
}

/// The room's limit in bytes for the tests: the seed of `COUNT` rectangles fits exactly, or one byte short.
const COUNT: usize = 3000;

fn loop_with_room_of(limit: u64) -> Loop {
    Loop::new(RelayConfig { max_room_state_bytes: limit, ..RelayConfig::self_host() })
}

fn process_with_room_of(limit: u64) -> Process {
    Process::with(&["--max-room-bytes", &limit.to_string()])
}

#[test]
fn a_drawing_that_exactly_fills_the_room_is_taken_on_the_loopback() {
    a_room_takes_a_drawing_up_to_its_limit(COUNT, &mut loop_with_room_of(seed_record_bytes(COUNT)));
}

#[test]
fn a_drawing_that_exactly_fills_the_room_is_taken_by_the_server_process() {
    a_room_takes_a_drawing_up_to_its_limit(COUNT, &mut process_with_room_of(seed_record_bytes(COUNT)));
}

#[test]
fn a_drawing_one_byte_over_the_room_is_refused_in_plain_words_on_the_loopback() {
    a_room_refuses_a_drawing_over_its_limit_in_plain_words(COUNT, &mut loop_with_room_of(seed_record_bytes(COUNT) - 1));
}

#[test]
fn a_drawing_one_byte_over_the_room_is_refused_in_plain_words_by_the_server_process() {
    a_room_refuses_a_drawing_over_its_limit_in_plain_words(
        COUNT,
        &mut process_with_room_of(seed_record_bytes(COUNT) - 1),
    );
}
