#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Pictures of an encrypted room: the sealed client against the real relay core (memory stores), in simulated time.
//! The websocket side is the in-memory loopback as in `tests/sealed.rs`. The HTTP side is `loopback_blobs` with a
//! small shim that turns each request into a call of `Relay::image_request`, as the real host does, and that can
//! hold requests, answer with a chosen status, or lose an answer.

mod common;

use std::sync::Arc;

use drafftink_collab::{
    AccessDenial, BlobAuth, BlobOp, BlobReply, BlobRequest, CollabClient, CollabEvent, CollabSession, ConnState,
    ImageEvent, ImageRefusal, LoopbackBlobServer, LoopbackServer, LoopbackTransport, MAX_DOWNLOADS, MAX_UPLOADS,
    Resume, RoomClient, SealedClient, SealedConfig, ServerUrl, StopReason, UrlPolicy, loopback, loopback_blobs,
};
use drafftink_doc::LoroStore;
use drafftink_protocol::v2::{ImageName, JoinMode, Proof, RoomLimits};
use drafftink_protocol::{ClientId, MAX_IMAGE_BYTES, SEAL_OVERHEAD};
use drafftink_relay::{
    BlobStore, ConnId, Effect, ImageBody, ImageOp, ImageRequest, ImageResponse, ImageStatus, MemoryBlobStore,
    MemoryRelayStore, Relay, RelayConfig,
};
use drafftink_scene::FileId;
use drafftink_seal::{RoomKey, RoomKeys};

/// One round of the simulation, in milliseconds.
const STEP_MS: u64 = 40;
const BASE: &str = "http://127.0.0.1:3030";

fn key() -> RoomKey {
    RoomKey::from_bytes([7; 32])
}

fn keys() -> RoomKeys {
    RoomKeys::new(&key())
}

/// A picture of `len` bytes that differs from every other `seed`.
fn picture(seed: u32, len: usize) -> (FileId, Arc<[u8]>) {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(88_172_645);
    let bytes: Vec<u8> = (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect();
    (FileId::of(&bytes), Arc::from(bytes))
}

/// The name the relay stores `id` under.
fn name_of(id: &FileId) -> ImageName {
    let mut raw = [0u8; 32];
    for (out, pair) in raw.iter_mut().zip(id.as_str().as_bytes().chunks(2)) {
        *out = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    keys().image_name(&raw)
}

/// The size of a sealed picture of `len` bytes.
fn sealed_len(len: usize) -> u64 {
    (len + SEAL_OVERHEAD) as u64
}

fn limits(max_image: u32, max_room_bytes: u64, max_room_images: u32) -> RelayConfig {
    RelayConfig {
        limits: RoomLimits {
            max_image_bytes: max_image,
            max_room_image_bytes: max_room_bytes,
            max_room_images,
            ..RoomLimits::DEFAULT
        },
        ..RelayConfig::self_host()
    }
}

// ---- what the blob server saw ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Put,
    Get,
    Head,
    List,
}

#[derive(Debug, Clone)]
struct Seen {
    at: u64,
    wire: usize,
    kind: Kind,
    name: Option<String>,
    status: Option<u16>,
}

/// What the shim does with a request instead of (or besides) serving it.
#[derive(Debug, Clone, Copy)]
enum Fault {
    Serve,
    /// Answer with this status and `Retry-After`, without asking the relay.
    Reply {
        status: u16,
        retry_after_s: Option<u64>,
    },
    /// Answer as the server's gate does (a `401` from Access, a redirect to its login), without asking the relay.
    Gate(u16, AccessDenial),
    /// The relay serves the request and the answer never arrives.
    Lose,
    /// The request never reaches the relay.
    Drop,
}

/// Decides what happens to a request.
type FaultFn = Box<dyn FnMut(&Seen) -> Fault>;

struct Wire {
    ws: Arc<LoopbackServer>,
    opens: usize,
    conn: Option<ConnId>,
    blobs: LoopbackBlobServer,
}

struct Room {
    relay: Relay<MemoryRelayStore, MemoryBlobStore>,
    blobs: MemoryBlobStore,
    wires: Vec<Wire>,
    now: u64,
    next_conn: u64,
    wake_at: Option<u64>,
    log: Vec<Seen>,
    hold: bool,
    held: Vec<(usize, usize, BlobRequest)>,
    peak_held: [usize; 4],
    fault: Option<FaultFn>,
}

impl Room {
    fn new(config: RelayConfig) -> Room {
        let blobs = MemoryBlobStore::new();
        let relay = Relay::new(keys().room_id(), config, MemoryRelayStore::new(), blobs.clone());
        Room {
            relay,
            blobs,
            wires: Vec::new(),
            now: 1_000_000,
            next_conn: 0,
            wake_at: None,
            log: Vec::new(),
            hold: false,
            held: Vec::new(),
            peak_held: [0; 4],
            fault: None,
        }
    }

    fn peer(&mut self, n: u8, key: RoomKey) -> Peer {
        let (transport, ws) = loopback();
        ws.set_auto_accept(true);
        let (blobs, blob_server) = loopback_blobs();
        self.wires.push(Wire { ws: Arc::new(ws), opens: 0, conn: None, blobs: blob_server });
        let cfg = SealedConfig {
            url: ServerUrl::parse("ws://127.0.0.1:3030/ws", UrlPolicy::default()).unwrap(),
            key,
            client_id: ClientId::from_bytes([n; 16]),
            user: None,
            mode: JoinMode::Create,
            resume: Resume::default(),
            jitter_seed: u64::from(n) * 7919,
            replace_when_trimmed: false,
        };
        let mut client = SealedClient::new(cfg).unwrap();
        client.start(self.now);
        let mut session = CollabSession::new(client, transport);
        session.set_blobs(Box::new(blobs));
        Peer { wire: self.wires.len() - 1, session, store: LoroStore::new(), events: Vec::new() }
    }

    fn effects(&mut self, out: drafftink_relay::Output) {
        self.wake_at = out.wake_at;
        for effect in out.effects {
            let find = |wires: &[Wire], conn| wires.iter().position(|w| w.conn == Some(conn));
            match effect {
                Effect::Send { conn, frame } => {
                    if let Some(i) = find(&self.wires, conn) {
                        self.wires[i].ws.push_raw(frame.to_vec());
                    }
                }
                Effect::Text { conn, text } => {
                    if let Some(i) = find(&self.wires, conn) {
                        self.wires[i].ws.push_text(text);
                    }
                }
                Effect::Close { conn } => {
                    if let Some(i) = find(&self.wires, conn) {
                        self.wires[i].ws.close_cleanly();
                        self.wires[i].conn = None;
                    }
                }
            }
        }
    }

    /// The relay and the blob server take what the peers sent.
    fn step(&mut self) {
        let now = self.now;
        for i in 0..self.wires.len() {
            let end = Arc::clone(&self.wires[i].ws);
            if end.opens() != self.wires[i].opens {
                if let Some(old) = self.wires[i].conn.take() {
                    let out = self.relay.on_closed(old, now);
                    self.effects(out);
                }
                self.next_conn += 1;
                let conn = ConnId(self.next_conn);
                self.wires[i].opens = end.opens();
                self.wires[i].conn = Some(conn);
                let out = self.relay.connect(conn, now);
                self.effects(out);
            }
            let Some(conn) = self.wires[i].conn else { continue };
            if !end.is_open() {
                self.wires[i].conn = None;
                let out = self.relay.on_closed(conn, now);
                self.effects(out);
                continue;
            }
            while let Some(frame) = end.recv_raw() {
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
        for w in 0..self.wires.len() {
            for request in self.wires[w].blobs.take_requests() {
                let seen = self.record(w, &request);
                if self.hold {
                    self.held.push((w, seen, request));
                    for kind in [Kind::Put, Kind::Get, Kind::Head, Kind::List] {
                        let n = self.held_of(kind);
                        self.peak_held[kind as usize] = self.peak_held[kind as usize].max(n);
                    }
                } else {
                    self.answer(w, seen, &request);
                }
            }
        }
    }

    fn record(&mut self, wire: usize, request: &BlobRequest) -> usize {
        let kind = match request.op {
            BlobOp::Put(_) => Kind::Put,
            BlobOp::Get => Kind::Get,
            BlobOp::Head => Kind::Head,
            BlobOp::List => Kind::List,
        };
        let name = request.url.rsplit_once("/f/").map(|(_, n)| n.to_owned());
        self.log.push(Seen { at: self.now, wire, kind, name, status: None });
        self.log.len() - 1
    }

    fn answer(&mut self, wire: usize, seen: usize, request: &BlobRequest) {
        let fault = match self.fault.as_mut() {
            Some(f) => f(&self.log[seen]),
            None => Fault::Serve,
        };
        let server = &self.wires[wire].blobs;
        let id = request.id;
        match fault {
            Fault::Drop => server.fail(id, "connection reset"),
            Fault::Reply { status, retry_after_s } => {
                self.log[seen].status = Some(status);
                server.respond(
                    id,
                    Ok(BlobReply {
                        status,
                        body: Vec::new(),
                        content_length: Some(0),
                        retry_after_ms: retry_after_s.map(|s| s * 1000),
                        ..BlobReply::default()
                    }),
                );
            }
            Fault::Gate(status, denial) => {
                self.log[seen].status = Some(status);
                server.respond(id, Ok(BlobReply { status, denial: Some(denial), ..BlobReply::default() }));
            }
            Fault::Serve | Fault::Lose => {
                let reply = self.apply(request);
                self.log[seen].status = Some(reply.status);
                let server = &self.wires[wire].blobs;
                if matches!(fault, Fault::Lose) {
                    server.fail(id, "the answer got lost");
                } else {
                    server.respond(id, Ok(reply));
                }
            }
        }
    }

    /// Serves the oldest `n` held requests.
    fn release(&mut self, n: usize) {
        let n = n.min(self.held.len());
        let batch: Vec<_> = self.held.drain(..n).collect();
        for (wire, seen, request) in batch {
            self.answer(wire, seen, &request);
        }
    }

    fn held_of(&self, kind: Kind) -> usize {
        self.held.iter().filter(|(_, seen, _)| self.log[*seen].kind == kind).count()
    }

    /// What the host does for `/r/<room>/f[/<name>]`.
    fn apply(&mut self, request: &BlobRequest) -> BlobReply {
        let status = |status: u16| BlobReply { status, ..BlobReply::default() };
        let prefix = format!("{BASE}/r/{}/f", keys().room_id());
        let Some(path) = request.url.strip_prefix(&prefix) else { return status(404) };
        let Some(token) = request.auth.token() else { return status(401) };
        let proof = Proof::token(&token);
        let name = path.strip_prefix('/').map(|n| n.parse::<ImageName>());
        let head = matches!(request.op, BlobOp::Head);
        let op = match (&request.op, name) {
            (BlobOp::Put(body), Some(Ok(name))) => ImageOp::Put { name, body },
            (BlobOp::Get | BlobOp::Head, Some(Ok(name))) => ImageOp::Get { name },
            (BlobOp::List, None) => ImageOp::List,
            _ => return status(400),
        };
        let (response, out) = self.relay.image_request(self.now, ImageRequest { proof: &proof, op });
        self.effects(out);
        render(response, head)
    }

    fn round(&mut self, peers: &mut [&mut Peer]) {
        self.now += STEP_MS;
        for p in peers.iter_mut() {
            p.pump(self.now);
        }
        self.step();
    }

    fn run(&mut self, peers: &mut [&mut Peer], ms: u64) {
        let until = self.now + ms;
        while self.now < until {
            self.round(peers);
        }
    }

    fn run_until(&mut self, peers: &mut [&mut Peer], what: &str, done: impl Fn(&[&mut Peer]) -> bool) {
        for _ in 0..5000 {
            self.round(peers);
            if done(peers) {
                return;
            }
        }
        panic!("gave up waiting for {what}: {:?}", peers.iter().map(|p| p.client().images()).collect::<Vec<_>>());
    }

    fn offer(&mut self, peer: &mut Peer, id: &FileId, bytes: &Arc<[u8]>) {
        peer.session.client_mut().offer_image(self.now, id, Arc::clone(bytes));
    }

    fn want(&mut self, peer: &mut Peer, id: &FileId) {
        peer.session.client_mut().want_image(self.now, id);
    }

    fn requests(&self, kind: Kind) -> Vec<&Seen> {
        self.log.iter().filter(|s| s.kind == kind).collect()
    }

    fn requests_for(&self, kind: Kind, id: &FileId) -> Vec<&Seen> {
        let name = name_of(id).to_string();
        self.log.iter().filter(|s| s.kind == kind && s.name.as_deref() == Some(name.as_str())).collect()
    }

    /// An online peer with nothing to send.
    fn online_peer(&mut self, n: u8) -> Peer {
        let mut p = self.peer(n, key());
        // A member with something on the canvas, so that its own edit is shared too.
        common::add(&mut p.store, &[&format!("rect-{n}")]);
        p.session.client_mut().local_changed(self.now);
        self.run_until(&mut [&mut p], "the peer is online", |p| p[0].settled());
        p
    }
}

fn render(response: ImageResponse, head: bool) -> BlobReply {
    let retry_after_ms = response.retry_after_ms.map(|ms| u64::from(ms).div_ceil(1000).max(1) * 1000);
    let json = |v: serde_json::Value| v.to_string().into_bytes();
    let body = match (response.status, response.body) {
        (ImageStatus::Ok, ImageBody::Sealed(bytes)) => bytes,
        (ImageStatus::Ok, ImageBody::List(list)) => {
            json(list.iter().map(|i| serde_json::json!({ "name": i.name.to_string(), "size": i.size })).collect())
        }
        (ImageStatus::Created, ImageBody::Totals(t)) => {
            json(serde_json::json!({ "room_images": t.count, "room_image_bytes": t.bytes }))
        }
        (status, _) => json(serde_json::json!({ "error": status.code() })),
    };
    let content_length = u64::try_from(body.len()).ok();
    BlobReply {
        status: response.status.http(),
        body: if head { Vec::new() } else { body },
        content_length,
        retry_after_ms,
        ..BlobReply::default()
    }
}

struct Peer {
    wire: usize,
    session: CollabSession<LoopbackTransport>,
    store: LoroStore,
    events: Vec<CollabEvent>,
}

impl Peer {
    fn pump(&mut self, now: u64) -> Option<u64> {
        let out = self.session.pump(now, &mut self.store);
        self.events.extend(out.events);
        out.next_wake_ms
    }

    fn client(&self) -> &SealedClient {
        self.session.client().sealed().expect("a sealed client")
    }

    fn settled(&self) -> bool {
        let s = self.client().status();
        s.state == ConnState::Online && !s.unsynced
    }

    fn image_events(&self) -> Vec<ImageEvent> {
        self.events
            .iter()
            .filter_map(|e| match e {
                CollabEvent::Image(i) => Some(i.clone()),
                _ => None,
            })
            .collect()
    }

    fn arrived(&self, id: &FileId) -> Option<Arc<[u8]>> {
        self.image_events().into_iter().find_map(|e| match e {
            ImageEvent::Arrived { id: i, bytes } if i == *id => Some(bytes),
            _ => None,
        })
    }

    fn arrivals(&self) -> usize {
        self.image_events().iter().filter(|e| matches!(e, ImageEvent::Arrived { .. })).count()
    }

    fn damaged(&self, id: &FileId) -> usize {
        self.image_events().iter().filter(|e| matches!(e, ImageEvent::Damaged { id: i } if i == id)).count()
    }

    fn rejected(&self, id: &FileId) -> Vec<ImageRefusal> {
        self.image_events()
            .into_iter()
            .filter_map(|e| match e {
                ImageEvent::Rejected { id: i, why } if i == *id => Some(why),
                _ => None,
            })
            .collect()
    }
}

// ---- limits ----

#[test]
fn a_picture_of_exactly_the_limit_is_stored_and_one_byte_more_is_rejected_without_a_request() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, MAX_IMAGE_BYTES);
    room.offer(&mut a, &id, &bytes);
    assert!(a.client().status().unsynced, "until the relay has it, the picture is not shared");
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled() && p[0].client().images().room_images == 1);
    let stored = room.blobs.dump();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].len(), MAX_IMAGE_BYTES + SEAL_OVERHEAD);
    assert_eq!(a.client().images().room_image_bytes, sealed_len(MAX_IMAGE_BYTES));

    let (big_id, big) = picture(2, MAX_IMAGE_BYTES + 1);
    room.offer(&mut a, &big_id, &big);
    room.run(&mut [&mut a], 2_000);
    assert_eq!(a.rejected(&big_id), [ImageRefusal::TooLarge]);
    assert_eq!(room.requests(Kind::Put).len(), 1, "no request for the picture that is too large");
    assert!(a.settled(), "a refused upload is not pending");
    assert_eq!(a.client().images().failed, 1);
    assert_eq!(room.blobs.dump().len(), 1);
}

#[test]
fn the_client_checks_the_limit_the_relay_announced() {
    let mut room = Room::new(limits(1000, 1 << 20, 10));
    let mut a = room.online_peer(1);
    let (ok_id, ok) = picture(1, 1000);
    let (big_id, big) = picture(2, 1001);
    room.offer(&mut a, &big_id, &big);
    room.offer(&mut a, &ok_id, &ok);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled() && p[0].client().images().room_images == 1);
    assert_eq!(a.rejected(&big_id), [ImageRefusal::TooLarge]);
    assert!(a.rejected(&ok_id).is_empty());
    assert_eq!(room.requests(Kind::Put).len(), 1);
}

#[test]
fn a_413_from_the_relay_is_final() {
    let mut room = Room::new(RelayConfig::self_host());
    room.fault = Some(Box::new(|s| {
        if s.kind == Kind::Put { Fault::Reply { status: 413, retry_after_s: None } } else { Fault::Serve }
    }));
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 2000);
    room.offer(&mut a, &id, &bytes);
    room.run(&mut [&mut a], 30_000);
    assert_eq!(a.rejected(&id), [ImageRefusal::TooLarge]);
    assert_eq!(room.requests(Kind::Put).len(), 1, "a refusal is not tried again");
    assert!(a.settled());
}

#[test]
fn the_room_total_at_the_limit_is_accepted_and_over_it_is_room_full() {
    let sealed = sealed_len(1000);
    let mut room = Room::new(limits(4096, 3 * sealed, 200));
    let mut a = room.online_peer(1);
    let pics: Vec<_> = (1..=4).map(|n| picture(n, 1000)).collect();
    for (id, bytes) in &pics[..3] {
        room.offer(&mut a, id, bytes);
    }
    room.run_until(&mut [&mut a], "three pictures", |p| p[0].settled() && p[0].client().images().room_images == 3);
    assert_eq!(a.client().images().room_image_bytes, 3 * sealed);
    assert!(a.image_events().is_empty());

    room.offer(&mut a, &pics[3].0, &pics[3].1);
    room.run(&mut [&mut a], 3_000);
    assert_eq!(a.rejected(&pics[3].0), [ImageRefusal::RoomFull]);
    assert_eq!(room.blobs.dump().len(), 3);
    assert_eq!(room.requests_for(Kind::Put, &pics[3].0).len(), 1, "507 is final");
    assert!(a.settled());
    assert_eq!(a.client().images().failed, 1);
}

#[test]
fn the_image_count_limit_is_room_full() {
    let mut room = Room::new(limits(4096, 50 << 20, 2));
    let mut a = room.online_peer(1);
    let pics: Vec<_> = (1..=3).map(|n| picture(n, 500)).collect();
    for (id, bytes) in &pics[..2] {
        room.offer(&mut a, id, bytes);
    }
    room.run_until(&mut [&mut a], "two pictures", |p| p[0].settled() && p[0].client().images().room_images == 2);
    room.offer(&mut a, &pics[2].0, &pics[2].1);
    room.run(&mut [&mut a], 3_000);
    assert_eq!(a.rejected(&pics[2].0), [ImageRefusal::RoomFull]);
    assert_eq!(room.blobs.dump().len(), 2);
}

// ---- membership ----

#[test]
fn a_non_member_gets_401_for_every_request_and_nothing_is_stored_or_returned() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 800);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled() && p[0].client().images().room_images == 1);

    let stranger = BlobAuth::member(&RoomKey::from_bytes([9; 32]).membership_token());
    let url = |tail: &str| format!("{BASE}/r/{}/f{tail}", keys().room_id());
    let name = name_of(&id);
    let (other_id, other) = picture(2, 800);
    let other_name = name_of(&other_id);
    let attempts = [
        (url(&format!("/{other_name}")), BlobOp::Put(keys().seal_image(&other_name, &other).unwrap().into_bytes())),
        (url(&format!("/{name}")), BlobOp::Get),
        (url(&format!("/{name}")), BlobOp::Head),
        (url(""), BlobOp::List),
    ];
    for (n, (url, op)) in attempts.into_iter().enumerate() {
        let reply = room.apply(&BlobRequest { id: n as u64, url, op, auth: stranger.clone() });
        assert_eq!(reply.status, 401, "request {n}");
        assert!(reply.body.len() < 100, "no picture and no list: {} bytes", reply.body.len());
    }
    assert_eq!(room.blobs.dump().len(), 1, "the stranger stored nothing");
}

#[test]
fn a_client_with_another_key_stops_and_never_asks_for_or_receives_bytes() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 800);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled() && p[0].client().images().room_images == 1);

    let mut stranger = room.peer(2, RoomKey::from_bytes([9; 32]));
    room.want(&mut stranger, &id);
    room.run(&mut [&mut a, &mut stranger], 10_000);
    assert_eq!(stranger.client().status().stopped, Some(StopReason::Unauthorized));
    assert!(stranger.arrived(&id).is_none());
    assert!(room.log.iter().all(|s| s.wire != stranger.wire), "no picture request leaves a client that is not in");
}

#[test]
fn a_401_on_a_download_is_final_and_counts_as_failed() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 800);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled() && p[0].client().images().room_images == 1);
    room.fault = Some(Box::new(|s| {
        if s.kind == Kind::Get { Fault::Reply { status: 401, retry_after_s: None } } else { Fault::Serve }
    }));
    let mut b = room.online_peer(2);
    room.want(&mut b, &id);
    room.run(&mut [&mut a, &mut b], 60_000);
    assert!(b.arrived(&id).is_none());
    assert_eq!(room.requests_for(Kind::Get, &id).len(), 1);
    assert_eq!(b.client().images().failed, 1);
}

/// The problems a peer was told about, in order.
fn problems(p: &Peer) -> Vec<drafftink_collab::Problem> {
    p.events
        .iter()
        .filter_map(|e| match e {
            CollabEvent::Problem(problem) => Some(problem.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_gate_of_a_private_server_stops_the_picture_requests_and_is_told_once_in_plain_words() {
    for (status, denial, text) in [
        (
            302,
            AccessDenial::Required,
            "This server is private. Open Preferences, Collaboration, and enter the access token for 127.0.0.1. The \
             server's owner can create one.",
        ),
        (
            401,
            AccessDenial::Rejected,
            "The server did not accept the access token. It may have expired or been revoked. Ask the server's owner \
             for a new one.",
        ),
    ] {
        let mut room = Room::new(RelayConfig::self_host());
        let mut a = room.online_peer(1);
        room.fault = Some(Box::new(move |_| Fault::Gate(status, denial)));
        let (id, _) = picture(1, 800);
        room.want(&mut a, &id);
        room.run(&mut [&mut a], 60_000);
        let told = problems(&a);
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(told[0].text, text);
        assert_eq!((told[0].action, told[0].fatal), (drafftink_collab::ProblemAction::AskAdmin, false));
        assert_eq!(a.client().status().state, ConnState::Online, "the room itself keeps working");
        assert!(a.arrived(&id).is_none());
        let asked = room.log.len();
        room.run(&mut [&mut a], 120_000);
        assert_eq!(room.log.len(), asked, "a request the gate refused is not asked again");
        assert!(a.client().images().failed >= 1);
    }
}

// ---- damaged downloads ----

/// `a` uploads a picture, `corrupt` damages what the relay holds, `b` asks for it.
fn damaged_download(corrupt: impl FnOnce(&mut MemoryBlobStore, &FileId, &ImageName)) {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 3000);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled() && p[0].client().images().room_images == 1);
    let name = name_of(&id);
    let mut store = room.blobs.clone();
    corrupt(&mut store, &id, &name);

    let mut b = room.online_peer(2);
    room.want(&mut b, &id);
    room.run(&mut [&mut a, &mut b], 40_000);
    assert_eq!(b.damaged(&id), 1);
    assert!(b.arrived(&id).is_none(), "the bytes are discarded");
    assert_eq!(room.requests_for(Kind::Get, &id).len(), 1, "a damaged picture is not asked for again");
    assert_eq!(b.client().images().failed, 1);
    room.want(&mut b, &id);
    room.run(&mut [&mut a, &mut b], 5_000);
    assert_eq!(room.requests_for(Kind::Get, &id).len(), 1);
    assert_eq!(b.damaged(&id), 1);
}

#[test]
fn a_flipped_bit_is_a_damaged_picture() {
    damaged_download(|store, _, name| {
        let mut sealed = store.get(name).unwrap().unwrap();
        sealed[100] ^= 1;
        store.put(name, &sealed).unwrap();
    });
}

#[test]
fn a_picture_stored_under_the_wrong_name_is_a_damaged_picture() {
    damaged_download(|store, _, name| {
        // Another picture, sealed correctly for its own name, is moved under this name: the name is in the
        // associated data, so it does not open here.
        let (other_id, other) = picture(77, 3000);
        let sealed = keys().seal_image(&name_of(&other_id), &other).unwrap().into_bytes();
        store.put(name, &sealed).unwrap();
    });
}

#[test]
fn a_valid_seal_over_the_wrong_bytes_is_a_damaged_picture() {
    damaged_download(|store, _, name| {
        let wrong = keys().seal_image(name, b"these are not the bytes the id names").unwrap().into_bytes();
        store.put(name, &wrong).unwrap();
    });
}

#[test]
fn garbage_is_a_damaged_picture() {
    damaged_download(|store, _, name| store.put(name, &[1, 2, 3]).unwrap());
}

// ---- one name per picture ----

#[test]
fn the_same_picture_offered_twice_stores_one_name_and_sends_one_request() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 2000);
    room.offer(&mut a, &id, &bytes);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled() && p[0].client().images().room_images == 1);
    room.offer(&mut a, &id, &bytes);
    room.run(&mut [&mut a], 2_000);
    assert_eq!(room.blobs.dump().len(), 1);
    assert_eq!(room.requests(Kind::Put).len(), 1);

    // A second member who holds the same picture finds the name in the list and sends nothing.
    let mut b = room.online_peer(2);
    room.offer(&mut b, &id, &bytes);
    room.run_until(&mut [&mut a, &mut b], "b settles", |p| p[1].settled());
    room.run(&mut [&mut a, &mut b], 2_000);
    assert_eq!(room.blobs.dump().len(), 1);
    assert_eq!(room.requests(Kind::Put).len(), 1, "the relay already had the name");
}

#[test]
fn two_members_offering_the_same_picture_at_once_store_one_name() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let mut b = room.online_peer(2);
    let (id, bytes) = picture(1, 2000);
    room.offer(&mut a, &id, &bytes);
    room.offer(&mut b, &id, &bytes);
    room.run_until(&mut [&mut a, &mut b], "both settle", |p| p[0].settled() && p[1].settled());
    assert_eq!(room.blobs.dump().len(), 1, "same picture, same name, overwritten in place");
}

// ---- connection loss ----

#[test]
fn an_upload_resumes_after_the_connection_drops_and_returns() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 5000);
    room.hold = true;
    room.offer(&mut a, &id, &bytes);
    room.run(&mut [&mut a], 200);
    assert_eq!(room.held_of(Kind::Put), 1);
    assert_eq!(a.client().images().uploading, 1);
    assert!(a.client().status().unsynced);

    // The connection drops while the upload is in flight: the request is lost with it.
    room.wires[a.wire].ws.drop_connection("the network went away");
    room.held.clear();
    room.hold = false;
    room.run(&mut [&mut a], 200);
    assert_eq!(a.client().status().state, ConnState::Reconnecting);
    assert!(a.client().status().unsynced, "the picture is not shared yet");
    assert_eq!(a.client().images().uploading, 0);
    assert_eq!(a.client().images().waiting, 1, "the offer is kept");

    room.run_until(&mut [&mut a], "the upload after the reconnect", |p| p[0].settled());
    assert_eq!(room.blobs.dump().len(), 1);
    assert_eq!(room.requests(Kind::List).len(), 2, "one list per connection");
    assert_eq!(a.client().images().room_images, 1);
}

#[test]
fn an_offer_made_while_offline_goes_out_when_the_connection_is_back() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    room.wires[a.wire].ws.drop_connection("the network went away");
    room.run(&mut [&mut a], 80);
    let (id, bytes) = picture(1, 5000);
    room.offer(&mut a, &id, &bytes);
    assert!(a.client().status().unsynced);
    assert!(room.requests(Kind::Put).is_empty());
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled());
    assert_eq!(room.blobs.dump().len(), 1);
}

#[test]
fn a_want_survives_a_reconnect_and_ends_in_arrived() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let mut b = room.online_peer(2);
    let (id, bytes) = picture(1, 4000);
    room.want(&mut b, &id);
    room.run(&mut [&mut a, &mut b], 1_500);
    room.wires[b.wire].ws.drop_connection("the network went away");
    room.run(&mut [&mut a, &mut b], 200);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a, &mut b], "the picture arrives", |p| p[1].arrived(&id).is_some());
    assert_eq!(b.arrived(&id).unwrap().as_ref(), bytes.as_ref());
}

// ---- retries ----

fn gaps(times: impl Iterator<Item = u64>) -> Vec<u64> {
    let times: Vec<u64> = times.collect();
    times.windows(2).map(|w| w[1] - w[0]).collect()
}

#[test]
fn a_404_followed_by_a_late_upload_ends_in_arrived_after_a_backoff_of_1_2_4_seconds() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let mut b = room.online_peer(2);
    let (id, bytes) = picture(1, 4000);
    room.want(&mut b, &id);
    while room.requests_for(Kind::Get, &id).iter().filter(|s| s.status == Some(404)).count() < 3 {
        room.round(&mut [&mut a, &mut b]);
    }
    room.round(&mut [&mut a, &mut b]);
    assert_eq!(b.client().images().waiting, 1);
    // The manifest was there first. Now the upload happens.
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a, &mut b], "arrival", |p| p[1].arrived(&id).is_some());
    assert_eq!(b.arrived(&id).unwrap().as_ref(), bytes.as_ref());

    let gets = room.requests_for(Kind::Get, &id);
    assert_eq!(gets.iter().map(|s| s.status).collect::<Vec<_>>(), [Some(404), Some(404), Some(404), Some(200)]);
    let waits = gaps(gets.iter().map(|s| s.at));
    for (wait, expected) in waits.iter().zip([1_000, 2_000, 4_000]) {
        assert!((expected..expected + 2 * STEP_MS).contains(wait), "waited {wait} ms, expected {expected}");
    }
    let progress = b.client().images();
    assert_eq!((progress.downloading, progress.waiting, progress.failed), (0, 0, 0));
}

#[test]
fn the_retry_timers_are_in_next_wake_ms() {
    // The owner sleeps until `next_wake_ms` and is woken by the transport when an answer arrives: nothing else.
    let mut room = Room::new(RelayConfig::self_host());
    let mut b = room.online_peer(2);
    let (id, _) = picture(1, 1000);
    room.want(&mut b, &id);
    let mut wake = b.pump(room.now);
    while room.requests_for(Kind::Get, &id).len() < 5 {
        room.now = wake.expect("a timer is set while a picture is wanted").max(room.now + 1);
        b.pump(room.now);
        room.step();
        wake = b.pump(room.now);
    }
    let waits = gaps(room.requests_for(Kind::Get, &id).iter().map(|s| s.at));
    assert_eq!(waits, [1_000, 2_000, 4_000, 8_000]);
}

#[test]
fn the_backoff_is_capped_at_two_minutes() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut b = room.online_peer(2);
    let (id, _) = picture(1, 4000);
    room.want(&mut b, &id);
    room.run(&mut [&mut b], 600_000);
    let waits = gaps(room.requests_for(Kind::Get, &id).iter().map(|s| s.at));
    let expected = [1_000u64, 2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 120_000, 120_000];
    for (wait, expected) in waits.iter().zip(expected) {
        assert!((expected..expected + 2 * STEP_MS).contains(wait), "waited {wait} ms, expected {expected}");
    }
    assert!(waits.len() >= expected.len());
}

#[test]
fn a_429_is_tried_again_after_the_retry_after_the_relay_named() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let mut b = room.online_peer(2);
    let (mut put_limited, mut get_limited) = (false, false);
    room.fault = Some(Box::new(move |s| {
        let limited = match s.kind {
            Kind::Put => &mut put_limited,
            Kind::Get => &mut get_limited,
            _ => return Fault::Serve,
        };
        if std::mem::replace(limited, true) {
            Fault::Serve
        } else {
            Fault::Reply { status: 429, retry_after_s: Some(7) }
        }
    }));
    let (id, bytes) = picture(1, 4000);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a, &mut b], "the upload", |p| p[0].settled());
    room.want(&mut b, &id);
    room.run_until(&mut [&mut a, &mut b], "the download", |p| p[1].arrived(&id).is_some());
    for (kind, ok) in [(Kind::Put, 201), (Kind::Get, 200)] {
        let seen = room.requests_for(kind, &id);
        assert_eq!(seen.iter().map(|s| s.status).collect::<Vec<_>>(), [Some(429), Some(ok)], "{kind:?}");
        let wait = seen[1].at - seen[0].at;
        assert!((7_000..7_000 + 2 * STEP_MS).contains(&wait), "{kind:?} waited {wait} ms, not the 7 s that were asked");
    }
}

#[test]
fn the_rate_limit_of_the_real_relay_slows_uploads_and_all_of_them_arrive() {
    let mut room = Room::new(RelayConfig { max_image_puts_per_sec: 1, ..RelayConfig::self_host() });
    let mut a = room.online_peer(1);
    let pics: Vec<_> = (1..=8).map(|n| picture(n, 300)).collect();
    for (id, bytes) in &pics {
        room.offer(&mut a, id, bytes);
    }
    room.run_until(&mut [&mut a], "all uploads", |p| p[0].settled() && p[0].client().images().room_images == 8);
    assert!(room.requests(Kind::Put).iter().any(|s| s.status == Some(429)), "the relay limited the uploads");
    assert_eq!(room.blobs.dump().len(), 8);
    assert_eq!(a.client().images().failed, 0);
}

#[test]
fn a_503_is_tried_again_with_the_backoff() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut once = true;
    room.fault = Some(Box::new(move |s| {
        if s.kind == Kind::Put && std::mem::take(&mut once) {
            Fault::Reply { status: 503, retry_after_s: None }
        } else {
            Fault::Serve
        }
    }));
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 1000);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled());
    let puts = room.requests_for(Kind::Put, &id);
    assert_eq!(puts.iter().map(|s| s.status).collect::<Vec<_>>(), [Some(503), Some(201)]);
    assert!((1_000..1_000 + 2 * STEP_MS).contains(&(puts[1].at - puts[0].at)));
}

#[test]
fn a_lost_put_response_is_checked_with_head_before_the_put_is_repeated() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut once = true;
    room.fault =
        Some(Box::new(
            move |s| {
                if s.kind == Kind::Put && std::mem::take(&mut once) { Fault::Lose } else { Fault::Serve }
            },
        ));
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 1000);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled());
    let kinds: Vec<_> = room.log.iter().filter(|s| s.kind != Kind::List).map(|s| (s.kind, s.status)).collect();
    assert_eq!(kinds, [(Kind::Put, Some(201)), (Kind::Head, Some(200))], "the HEAD found it: no second PUT");
    assert_eq!(room.blobs.dump().len(), 1);
}

#[test]
fn a_put_that_never_arrived_is_sent_again_after_head_says_404() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut once = true;
    room.fault =
        Some(Box::new(
            move |s| {
                if s.kind == Kind::Put && std::mem::take(&mut once) { Fault::Drop } else { Fault::Serve }
            },
        ));
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 1000);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled());
    let kinds: Vec<_> = room.log.iter().filter(|s| s.kind != Kind::List).map(|s| (s.kind, s.status)).collect();
    assert_eq!(kinds, [(Kind::Put, None), (Kind::Head, Some(404)), (Kind::Put, Some(201))]);
    assert_eq!(room.blobs.dump().len(), 1);
}

#[test]
fn a_head_that_finds_a_name_with_other_content_is_overwritten_by_the_put() {
    let mut room = Room::new(RelayConfig::self_host());
    let (id, bytes) = picture(1, 1000);
    let name = name_of(&id);
    let mut store = room.blobs.clone();
    let mut once = true;
    room.fault = Some(Box::new(move |s| {
        if s.kind == Kind::Put && std::mem::take(&mut once) {
            // The first upload is lost on the way, and somebody puts junk under the name meanwhile.
            store.put(&name, b"junk").unwrap();
            Fault::Drop
        } else {
            Fault::Serve
        }
    }));
    let mut a = room.online_peer(1);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled());
    let kinds: Vec<_> = room.log.iter().filter(|s| s.kind != Kind::List).map(|s| (s.kind, s.status)).collect();
    assert_eq!(kinds, [(Kind::Put, None), (Kind::Head, Some(200)), (Kind::Put, Some(201))], "the length is not ours");
    let held = room.blobs.clone().get(&name_of(&id)).unwrap().unwrap();
    assert_eq!(held.len() as u64, sealed_len(1000));
}

// ---- concurrency and progress ----

#[test]
fn at_most_three_downloads_and_two_uploads_are_in_flight() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let mut b = room.online_peer(2);
    let pics: Vec<_> = (1..=8).map(|n| picture(n, 600)).collect();

    room.hold = true;
    for (id, bytes) in &pics {
        room.offer(&mut a, id, bytes);
    }
    room.run(&mut [&mut a, &mut b], 400);
    assert_eq!(room.held_of(Kind::Put), MAX_UPLOADS);
    let p = a.client().images();
    assert_eq!((p.uploading, p.waiting), (MAX_UPLOADS as u32, 6));
    while a.client().images().room_images < 8 {
        room.release(1);
        room.run(&mut [&mut a, &mut b], 80);
        assert!(room.held_of(Kind::Put) <= MAX_UPLOADS);
    }
    room.release(usize::MAX);
    room.run_until(&mut [&mut a, &mut b], "a settles", |p| p[0].settled());
    assert_eq!(room.peak_held[Kind::Put as usize], MAX_UPLOADS);
    assert_eq!(room.blobs.dump().len(), 8);

    for (id, _) in &pics {
        room.want(&mut b, id);
    }
    room.run(&mut [&mut a, &mut b], 400);
    assert_eq!(room.held_of(Kind::Get), MAX_DOWNLOADS);
    let p = b.client().images();
    assert_eq!((p.downloading, p.waiting), (MAX_DOWNLOADS as u32, 5));
    while b.arrivals() < 8 {
        room.release(1);
        room.run(&mut [&mut a, &mut b], 80);
        assert!(room.held_of(Kind::Get) <= MAX_DOWNLOADS);
    }
    assert_eq!(room.peak_held[Kind::Get as usize], MAX_DOWNLOADS);
    let p = b.client().images();
    assert_eq!((p.downloading, p.waiting, p.failed), (0, 0, 0));
    let p = a.client().images();
    assert_eq!((p.room_images, p.room_image_bytes), (8, 8 * sealed_len(600)));
    for (id, bytes) in &pics {
        assert_eq!(b.arrived(id).unwrap().as_ref(), bytes.as_ref());
    }
}

#[test]
fn a_late_joiner_reads_the_totals_from_the_list() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    for n in 1..=3 {
        let (id, bytes) = picture(n, 500);
        room.offer(&mut a, &id, &bytes);
    }
    room.run_until(&mut [&mut a], "uploads", |p| p[0].settled() && p[0].client().images().room_images == 3);
    let mut b = room.online_peer(2);
    room.run(&mut [&mut a, &mut b], 500);
    let p = b.client().images();
    assert_eq!((p.room_images, p.room_image_bytes), (3, 3 * sealed_len(500)));
}

// ---- idle and the plaintext client ----

#[test]
fn an_idle_client_with_every_picture_settled_asks_for_no_wake_in_the_past() {
    let mut room = Room::new(RelayConfig::self_host());
    let mut a = room.online_peer(1);
    let (id, bytes) = picture(1, 500);
    room.offer(&mut a, &id, &bytes);
    room.run_until(&mut [&mut a], "the upload", |p| p[0].settled());
    room.run(&mut [&mut a], 2_000);
    let now = room.now + STEP_MS;
    let wake = a.pump(now);
    assert!(wake.is_none_or(|w| w > now), "{wake:?} at {now}");
    assert!(!a.session.pump(now + 1, &mut a.store).redraw, "an idle pump causes no frame");
}

#[test]
fn a_plaintext_client_ignores_pictures() {
    let mut client = RoomClient::from(CollabClient::new(common::config("ws://127.0.0.1:3030")).unwrap());
    let (id, bytes) = picture(1, 100);
    client.offer_image(0, &id, bytes);
    client.want_image(0, &id);
    assert!(client.poll_blob_request().is_none());
    assert_eq!(client.images(), drafftink_collab::ImageProgress::default());
}

#[test]
fn a_session_without_a_picture_transport_fails_requests_and_backs_off() {
    let mut room = Room::new(RelayConfig::self_host());
    let (transport, ws) = loopback();
    ws.set_auto_accept(true);
    room.wires.push(Wire { ws: Arc::new(ws), opens: 0, conn: None, blobs: loopback_blobs().1 });
    let cfg = SealedConfig {
        url: ServerUrl::parse("ws://127.0.0.1:3030/ws", UrlPolicy::default()).unwrap(),
        key: key(),
        client_id: ClientId::from_bytes([1; 16]),
        user: None,
        mode: JoinMode::Create,
        resume: Resume::default(),
        jitter_seed: 1,
        replace_when_trimmed: false,
    };
    let mut client = SealedClient::new(cfg).unwrap();
    client.start(room.now);
    let mut p =
        Peer { wire: 0, session: CollabSession::new(client, transport), store: LoroStore::new(), events: Vec::new() };
    room.run_until(&mut [&mut p], "online", |p| p[0].client().status().state == ConnState::Online);
    let (id, _) = picture(1, 100);
    room.want(&mut p, &id);
    room.run(&mut [&mut p], 20_000);
    assert_eq!(p.arrivals(), 0);
    assert_eq!(p.client().images().waiting, 1, "the want stays and is retried by the backoff");
}
