#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The relay is blind to pictures, in memory: two sealed clients share a picture that holds a marker through the real
//! client path (`offer_image`, `want_image`) against the relay core with its in-memory stores. Then the dump of both
//! stores, every websocket frame and every HTTP request and answer are searched. The same check against the
//! filesystem and SQLite relays of the real server is `apps/drafftink-server/tests/blind.rs`.

mod common;

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use drafftink_collab::{
    BlobOp, BlobReply, BlobRequest, CollabEvent, CollabSession, ImageEvent, LoopbackBlobServer, LoopbackServer,
    LoopbackTransport, Resume, SealedClient, SealedConfig, ServerUrl, UrlPolicy, loopback, loopback_blobs,
};
use drafftink_doc::{FilePolicy, LoroStore};
use drafftink_protocol::v2::{ClientMessage, ImageName, JoinMode, Proof};
use drafftink_protocol::{ClientId, UserInfo};
use drafftink_relay::{
    BlobStore, ConnId, Effect, ImageBody, ImageOp, ImageRequest, ImageResponse, ImageStatus, MemoryBlobStore,
    MemoryRelayStore, Relay, RelayConfig,
};
use drafftink_scene::{FileEntry, FileId, SceneRead, SceneStore, SceneTx};
use drafftink_seal::{Purpose, RoomKey, RoomKeys};

const MARKER: &str = "CANARY-7f3a91c2";
const USER: &str = "Zelda-Canary";
const BASE: &str = "http://127.0.0.1:3030";
const STEP_MS: u64 = 40;

struct Peer {
    session: CollabSession<LoopbackTransport>,
    store: LoroStore,
    arrived: Vec<(FileId, Arc<[u8]>)>,
    ws: Arc<LoopbackServer>,
    opens: usize,
    conn: Option<ConnId>,
    blobs: LoopbackBlobServer,
}

struct Rig {
    keys: RoomKeys,
    relay: Relay<MemoryRelayStore, MemoryBlobStore>,
    relay_store: MemoryRelayStore,
    blob_store: MemoryBlobStore,
    peers: Vec<Peer>,
    now: u64,
    next_conn: u64,
    /// Every websocket frame, both directions.
    frames: Vec<Vec<u8>>,
    /// Address, header, request body and answer body of every HTTP exchange.
    http: Vec<Vec<u8>>,
    headers: Vec<Vec<u8>>,
}

fn key() -> RoomKey {
    RoomKey::from_bytes([0x5a; 32])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

impl Rig {
    fn new() -> Rig {
        let (relay_store, blob_store) = (MemoryRelayStore::new(), MemoryBlobStore::new());
        let keys = RoomKeys::new(&key());
        let relay = Relay::new(keys.room_id(), RelayConfig::self_host(), relay_store.clone(), blob_store.clone());
        Rig {
            keys,
            relay,
            relay_store,
            blob_store,
            peers: Vec::new(),
            now: 1_000_000,
            next_conn: 0,
            frames: Vec::new(),
            http: Vec::new(),
            headers: Vec::new(),
        }
    }

    fn add_peer(&mut self, n: u8) -> usize {
        let (transport, ws) = loopback();
        ws.set_auto_accept(true);
        let (blobs, blob_server) = loopback_blobs();
        let cfg = SealedConfig {
            url: ServerUrl::parse("ws://127.0.0.1:3030/ws", UrlPolicy::default()).unwrap(),
            key: key(),
            client_id: ClientId::from_bytes([n; 16]),
            user: Some(UserInfo { name: USER.to_owned(), color: 0x33_66_99 }),
            mode: JoinMode::Create,
            resume: Resume::default(),
            jitter_seed: u64::from(n),
            replace_when_trimmed: false,
        };
        let mut client = SealedClient::new(cfg).unwrap();
        client.start(self.now);
        let mut store = common::store(u64::from(n) + 1000);
        store.set_file_policy(FilePolicy::External);
        let mut session = CollabSession::new(client, transport);
        session.set_blobs(Box::new(blobs));
        self.peers.push(Peer {
            session,
            store,
            arrived: Vec::new(),
            ws: Arc::new(ws),
            opens: 0,
            conn: None,
            blobs: blob_server,
        });
        self.peers.len() - 1
    }

    fn effects(&mut self, out: drafftink_relay::Output) {
        for effect in out.effects {
            match effect {
                Effect::Send { conn, frame } => {
                    self.frames.push(frame.to_vec());
                    if let Some(p) = self.peers.iter().find(|p| p.conn == Some(conn)) {
                        p.ws.push_raw(frame.to_vec());
                    }
                }
                Effect::Text { conn, text } => {
                    if let Some(p) = self.peers.iter().find(|p| p.conn == Some(conn)) {
                        p.ws.push_text(text);
                    }
                }
                Effect::Close { .. } => {}
            }
        }
    }

    /// One round: the peers run, then the relay takes what they sent.
    fn round(&mut self) {
        self.now += STEP_MS;
        for p in &mut self.peers {
            let out = p.session.pump(self.now, &mut p.store);
            for event in out.events {
                if let CollabEvent::Image(ImageEvent::Arrived { id, bytes }) = event {
                    p.arrived.push((id, bytes));
                }
            }
        }
        for i in 0..self.peers.len() {
            let ws = Arc::clone(&self.peers[i].ws);
            if ws.opens() != self.peers[i].opens {
                self.next_conn += 1;
                let conn = ConnId(self.next_conn);
                self.peers[i].opens = ws.opens();
                self.peers[i].conn = Some(conn);
                let out = self.relay.connect(conn, self.now);
                self.effects(out);
            }
            let Some(conn) = self.peers[i].conn else { continue };
            while let Some(frame) = ws.recv_raw() {
                self.frames.push(frame.clone());
                let out = self.relay.on_frame(conn, self.now, &frame);
                self.effects(out);
            }
            for text in ws.take_texts() {
                let out = self.relay.on_text(conn, self.now, &text);
                self.effects(out);
            }
            for request in self.peers[i].blobs.take_requests() {
                let (id, reply) = (request.id, self.serve(&request));
                self.http.push(request.url.clone().into_bytes());
                if let BlobOp::Put(body) = &request.op {
                    self.http.push(body.clone());
                }
                self.http.push(reply.body.clone());
                self.headers.push(request.auth.header_value().as_bytes().to_vec());
                self.peers[i].blobs.respond(id, Ok(reply));
            }
        }
    }

    /// What the host does for `/r/<room>/f[/<name>]`.
    fn serve(&mut self, request: &BlobRequest) -> BlobReply {
        let status = |status: u16| BlobReply { status, ..BlobReply::default() };
        let prefix = format!("{BASE}/r/{}/f", self.keys.room_id());
        let Some(path) = request.url.strip_prefix(&prefix) else { return status(404) };
        let Some(token) = request.auth.token() else { return status(401) };
        let proof = Proof::token(&token);
        let name = path.strip_prefix('/').map(|n| n.parse::<ImageName>());
        let op = match (&request.op, name) {
            (BlobOp::Put(body), Some(Ok(name))) => ImageOp::Put { name, body },
            (BlobOp::Get | BlobOp::Head, Some(Ok(name))) => ImageOp::Get { name },
            (BlobOp::List, None) => ImageOp::List,
            _ => return status(400),
        };
        let (response, out) = self.relay.image_request(self.now, ImageRequest { proof: &proof, op });
        self.effects(out);
        render(response, matches!(request.op, BlobOp::Head))
    }

    fn run_until(&mut self, what: &str, done: impl Fn(&Rig) -> bool) {
        for _ in 0..5000 {
            self.round();
            if done(self) {
                return;
            }
        }
        panic!("gave up waiting for {what}");
    }

    fn run(&mut self, ms: u64) {
        let until = self.now + ms;
        while self.now < until {
            self.round();
        }
    }
}

fn render(response: ImageResponse, head: bool) -> BlobReply {
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
        retry_after_ms: response.retry_after_ms.map(|ms| u64::from(ms).div_ceil(1000).max(1) * 1000),
    }
}

fn spellings(what: &str, bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    vec![
        (what.to_owned(), bytes.to_vec()),
        (format!("{what} in hex"), hex(bytes).into_bytes()),
        (format!("{what} in base64"), STANDARD.encode(bytes).into_bytes()),
        (format!("{what} in base64url"), URL_SAFE_NO_PAD.encode(bytes).into_bytes()),
    ]
}

#[test]
fn the_in_memory_relay_holds_and_sees_nothing_readable_of_a_picture() {
    let mut rig = Rig::new();
    let (a, b) = (rig.add_peer(1), rig.add_peer(2));
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(MARKER.as_bytes());
    data.extend((0..600u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8));
    let (id, entry) = FileEntry::new("image/png", data.clone());

    // A holds the manifest and the bytes and offers them. B learns the manifest from the document and asks.
    let mut tx = rig.peers[a].store.begin();
    tx.put_file(entry.clone()).unwrap();
    tx.commit().unwrap();
    common::add(&mut rig.peers[a].store, &[MARKER]);
    let now = rig.now;
    rig.peers[a].session.client_mut().local_changed(now);
    rig.peers[a].session.client_mut().offer_image(now, &id, Arc::clone(&entry.bytes));
    rig.peers[a].session.client_mut().set_cursor(now, Some(drafftink_protocol::Point { x: 1.0, y: 2.0 }));
    rig.run_until("b to see the manifest", |r| r.peers[b].store.manifest(&id).is_some());
    assert!(rig.peers[b].store.file(&id).is_none(), "the document holds no bytes");
    let now = rig.now;
    rig.peers[b].session.client_mut().want_image(now, &id);
    rig.run_until("b to download the picture", |r| !r.peers[b].arrived.is_empty());
    assert_eq!(rig.peers[b].arrived[0].1.as_ref(), data.as_slice());
    rig.run(3_000);

    // What the relay holds and what crossed the wire.
    let name = rig.keys.image_name(&raw(&id));
    let names: Vec<_> = rig.blob_store.clone().list().unwrap().into_iter().map(|i| i.name).collect();
    assert_eq!(names, [name], "one keyed name is stored");
    let mut stores = rig.relay_store.dump();
    stores.extend(rig.blob_store.dump());
    let mut wire = rig.frames.clone();
    wire.extend(rig.http.iter().cloned());
    assert!(wire.len() > 10 && stores.len() > 1);

    let k = key();
    let mut forbidden = vec![
        ("the marker".to_owned(), MARKER.as_bytes().to_vec()),
        ("the user name".to_owned(), USER.as_bytes().to_vec()),
        ("the media type".to_owned(), b"image/png".to_vec()),
        ("the picture".to_owned(), data),
    ];
    forbidden.extend(spellings("the key", k.expose_bytes()));
    forbidden.extend(spellings("the file id", &raw(&id)));
    for purpose in [Purpose::Content, Purpose::Presence, Purpose::Image, Purpose::ImageName] {
        forbidden.extend(spellings("a subkey", k.subkey(purpose).expose_bytes()));
    }
    for (what, needle) in &forbidden {
        for (place, haystacks) in [("the stores", &stores), ("the wire", &wire), ("a header", &rig.headers)] {
            assert!(!haystacks.iter().any(|h| contains(h, needle)), "{what} is visible in {place}");
        }
    }
    // The name is not the id, nor a prefix of it, in either spelling.
    let (name_hex, name_text, id_hex) = (hex(name.as_bytes()), name.to_string(), id.as_str().to_owned());
    for end in 1..=id_hex.len() {
        assert_ne!(name_text, id_hex[..end]);
        assert_ne!(name_hex, id_hex[..end]);
    }

    // The token is in the join frames and the headers, not in the stores and not in any picture request or answer.
    let token = rig.keys.membership_token().expose_bytes().to_vec();
    let joins: Vec<_> = rig.frames.iter().filter(|f| contains(f, &token)).collect();
    assert!(
        !joins.is_empty() && joins.iter().all(|f| matches!(ClientMessage::decode(f), Ok(ClientMessage::Join { .. })))
    );
    let header = format!("Drafft-Member {}", URL_SAFE_NO_PAD.encode(&token));
    assert!(!rig.headers.is_empty() && rig.headers.iter().all(|h| h == header.as_bytes()));
    for (what, needle) in spellings("the token", &token) {
        assert!(!stores.iter().any(|h| contains(h, &needle)), "{what} is stored");
        assert!(!rig.http.iter().any(|h| contains(h, &needle)), "{what} is in a picture request or answer");
    }
}

fn raw(id: &FileId) -> [u8; 32] {
    let mut raw = [0u8; 32];
    for (out, pair) in raw.iter_mut().zip(id.as_str().as_bytes().chunks(2)) {
        *out = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    raw
}
