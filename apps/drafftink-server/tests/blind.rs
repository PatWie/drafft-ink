#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Real sockets and the real clock.
#![allow(clippy::disallowed_methods)]
//! The relay is blind: two clients draw text that holds a marker, name themselves, move a cursor and upload a picture
//! that holds the marker. Then every byte the relay stored and every frame on the wire is searched for what must not be
//! there: the marker, the user name, the room key in every spelling, the keys derived from it, the file id and the
//! hash of the picture. Zero hits. The membership token is allowed in the join frame and nowhere else.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64ct::{Base64UrlUnpadded, Encoding};
use common::*;
use drafftink_collab::{
    CollabSession, NativeTransport, Resume, SealedClient, SealedConfig, ServerUrl, Transport, TransportError,
    TransportEvent, UrlPolicy,
};
use drafftink_doc::LoroStore;
use drafftink_protocol::v2::JoinMode;
use drafftink_protocol::{ClientId, Point, UserInfo};
use drafftink_scene::{Element, ElementId, ElementKind, SceneRead, SceneStore, SceneTx, Transform, ZIndex};
use drafftink_seal::{Purpose, RoomKey, RoomKeys};
use drafftink_server::config::Config;
use drafftink_server::store::StorageConfig;

const MARKER: &str = "CANARY-7f3a91c2";
const USER: &str = "Zelda-Canary";

/// A transport that keeps every frame that crosses it, in both directions.
struct Recording {
    inner: NativeTransport,
    wire: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Transport for Recording {
    fn open(&mut self, url: &ServerUrl) -> Result<(), TransportError> {
        self.inner.open(url)
    }
    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        self.wire.lock().unwrap().push(frame.clone());
        self.inner.send(frame)
    }
    fn send_text(&mut self, text: &str) -> Result<(), TransportError> {
        self.inner.send_text(text)
    }
    fn close(&mut self) {
        self.inner.close();
    }
    fn poll(&mut self) -> Option<TransportEvent> {
        let event = self.inner.poll()?;
        if let TransportEvent::Message(bytes) = &event {
            self.wire.lock().unwrap().push(bytes.clone());
        }
        Some(event)
    }
}

struct Peer {
    session: CollabSession<Recording>,
    store: LoroStore,
}

fn peer(n: u8, key: &RoomKey, url: &str, mode: JoinMode, wire: &Arc<Mutex<Vec<Vec<u8>>>>) -> Peer {
    let cfg = SealedConfig {
        url: ServerUrl::parse(url, UrlPolicy::default()).unwrap(),
        key: RoomKey::from_bytes(*key.expose_bytes()),
        client_id: ClientId::from_bytes([n; 16]),
        user: Some(UserInfo { name: USER.to_owned(), color: 0x336699 }),
        mode,
        resume: Resume::default(),
        jitter_seed: u64::from(n),
        replace_when_trimmed: false,
    };
    let transport = Recording { inner: NativeTransport::new(Arc::new(|| {})), wire: Arc::clone(wire) };
    let mut client = SealedClient::new(cfg).unwrap();
    client.start(0);
    let store =
        LoroStore::with_config(drafftink_doc::LoroConfig { peer_id: Some(u64::from(n) + 1000), ..Default::default() })
            .unwrap();
    Peer { session: CollabSession::new(client, transport), store }
}

fn pump_until(peers: &mut [&mut Peer], what: &str, done: impl Fn(&[&mut Peer]) -> bool) {
    let started = Instant::now();
    loop {
        let now = u64::try_from(started.elapsed().as_millis()).unwrap();
        for p in peers.iter_mut() {
            p.session.pump(now, &mut p.store);
        }
        if done(peers) {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn files_under(dir: &Path) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push(bytes);
        }
    }
    out
}

/// Everything the relay must never hold or see.
fn forbidden(key: &RoomKey, picture: &[u8], file_id: &[u8; 32]) -> Vec<(&'static str, Vec<u8>)> {
    let raw = key.expose_bytes().to_vec();
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let mut out = vec![
        ("the marker", MARKER.as_bytes().to_vec()),
        ("the user name", USER.as_bytes().to_vec()),
        ("the key", raw.clone()),
        ("the key in hex", hex(&raw).into_bytes()),
        ("the key in base64url", Base64UrlUnpadded::encode_string(&raw).into_bytes()),
        ("the file id", file_id.to_vec()),
        ("the file id in hex", hex(file_id).into_bytes()),
        ("the picture", picture.to_vec()),
    ];
    for purpose in [Purpose::Content, Purpose::Presence, Purpose::Image, Purpose::ImageName] {
        out.push(("a subkey", key.subkey(purpose).expose_bytes().to_vec()));
    }
    out
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

async fn canary(config: Config, stored_under: &Path) {
    let key = RoomKey::from_bytes([0x5a; 32]);
    let keys = RoomKeys::new(&key);
    let server = drafftink_server::start(config).await.expect("starts");
    let addr = server.local_addr();
    let url = format!("ws://{addr}/ws");
    let wire = Arc::new(Mutex::new(Vec::new()));
    let (url2, wire2, key2) = (url.clone(), Arc::clone(&wire), RoomKey::from_bytes(*key.expose_bytes()));
    tokio::task::spawn_blocking(move || {
        let mut a = peer(1, &key2, &url2, JoinMode::Create, &wire2);
        let mut b = peer(2, &key2, &url2, JoinMode::Create, &wire2);
        let mut e = Element::new(
            ElementId::new(MARKER).unwrap(),
            ElementKind::Rectangle,
            ZIndex::rebalance(1).unwrap().remove(0),
        );
        e.transform = Transform::new(1.0, 2.0, 3.0, 4.0);
        let mut tx = a.store.begin();
        tx.insert(e).unwrap();
        tx.commit().unwrap();
        a.session.client_mut().local_changed(0);
        a.session.client_mut().set_cursor(0, Some(Point { x: 1.0, y: 2.0 }));
        pump_until(&mut [&mut a, &mut b], "b to see the drawing", |p| {
            p[1].store.iter().count() == 1 && !p[0].session.client().status().unsynced
        });
        // Let the presence of both go out.
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            for p in [&mut a, &mut b] {
                p.session.pump(5_000, &mut p.store);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })
    .await
    .unwrap();

    // A picture whose bytes hold the marker, uploaded through the image route like a client does.
    let mut picture = b"\x89PNG\r\n\x1a\n".to_vec();
    picture.extend_from_slice(MARKER.as_bytes());
    picture.extend_from_slice(&[7; 300]);
    let file_id = sha256(&picture);
    let name = keys.image_name(&file_id);
    let body = keys.seal_image(&name, &picture).unwrap().into_bytes();
    let token = Base64UrlUnpadded::encode_string(keys.membership_token().expose_bytes());
    let header = [("Authorization", format!("Drafft-Member {token}"))];
    let (status, _, answer) =
        http_request(addr, "PUT", &format!("/r/{}/f/{name}", keys.room_id()), &header, &body).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&answer));
    wire.lock().unwrap().push(body.clone());
    server.shutdown().await;

    let stored = files_under(stored_under);
    let bytes: usize = stored.iter().map(Vec::len).sum();
    assert!(bytes > 600, "the relay stored the room, its log and the picture: {bytes} bytes in {} files", stored.len());
    let frames = wire.lock().unwrap().clone();
    assert!(frames.len() > 10, "frames were recorded: {}", frames.len());
    for (what, needle) in forbidden(&key, &picture, &file_id) {
        for (place, haystacks) in [("storage", &stored), ("the wire", &frames)] {
            assert!(!haystacks.iter().any(|h| contains(h, &needle)), "{what} is visible in {place}");
        }
    }
    // The membership token is the one thing the relay sees, in the join frame. It is not in storage.
    let token_bytes = keys.membership_token().expose_bytes().to_vec();
    assert!(frames.iter().any(|f| contains(f, &token_bytes)), "the join frame carries the token");
    assert!(!stored.iter().any(|f| contains(f, &token_bytes)), "the token is not stored");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_filesystem_relay_stores_and_sees_nothing_readable() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.storage = StorageConfig::Filesystem { dir: dir.path().to_path_buf() };
    canary(config, dir.path()).await;
}

#[cfg(feature = "sqlite")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sqlite_relay_stores_and_sees_nothing_readable() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.storage = StorageConfig::Sqlite { path: dir.path().join("relay.sqlite3") };
    canary(config, dir.path()).await;
}
