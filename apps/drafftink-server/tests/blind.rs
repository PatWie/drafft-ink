#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Real sockets and the real clock.
#![allow(clippy::disallowed_methods)]
//! The relay is blind: two clients draw text that holds a marker, name themselves, move a cursor and share a picture
//! that holds the marker. The picture goes through the real client path (a manifest in the document, `offer_image`,
//! `want_image`, the native picture transport) and, as an independent route, through a hand-made `PUT`. Then every
//! byte the relay stored and everything that crossed the wire (websocket frames, and the address, header and body of
//! every HTTP exchange) is searched for what must not be there: the marker, the user name, the room key in every
//! spelling, the keys derived from it, the file ids and hashes, the pictures, and the media type. Zero hits. The
//! keyed name of each picture must be in storage (the relay sees names) and must not look like the file id. The
//! membership token is allowed in the join frame and the `Authorization` header and nowhere else.

mod common;

use std::path::Path;

use base64ct::{Base64, Base64UrlUnpadded, Encoding};
use common::room_client::{BlobOptions, Clock, Taps, USER, peer, picture, pump_for, pump_until, raw_id};
use common::*;
use drafftink_protocol::Point;
use drafftink_protocol::v2::{ClientMessage, ImageName, JoinMode};
use drafftink_scene::{
    Element, ElementId, ElementKind, FileEntry, FileId, SceneRead, SceneStore, SceneTx, Transform, ZIndex,
};
use drafftink_seal::{Purpose, RoomKey, RoomKeys};
use drafftink_server::config::Config;
use drafftink_server::store::StorageConfig;

const MARKER: &str = "CANARY-7f3a91c2";
const MIME: &str = "image/png";

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|x| format!("{x:02x}")).collect()
}

/// Every file under `dir`: its content, and its path as text (a file name can give a secret away too).
fn files_under(dir: &Path) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push(bytes);
            out.push(path.to_string_lossy().into_owned().into_bytes());
        }
    }
    out
}

/// A picture the relay was given.
struct Shared {
    bytes: Vec<u8>,
    id: FileId,
    name: ImageName,
}

impl Shared {
    fn new(keys: &RoomKeys, bytes: Vec<u8>) -> Self {
        let id = FileId::of(&bytes);
        let name = keys.image_name(&raw_id(&id));
        Shared { bytes, id, name }
    }
}

/// Every spelling of `bytes` that a leak could take.
fn spellings(what: &str, bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    vec![
        (what.to_owned(), bytes.to_vec()),
        (format!("{what} in hex"), hex(bytes).into_bytes()),
        (format!("{what} in base64"), Base64::encode_string(bytes).into_bytes()),
        (format!("{what} in base64url"), Base64UrlUnpadded::encode_string(bytes).into_bytes()),
    ]
}

/// Everything the relay must never hold or see.
fn forbidden(key: &RoomKey, pictures: &[Shared]) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![
        ("the marker".to_owned(), MARKER.as_bytes().to_vec()),
        ("the user name".to_owned(), USER.as_bytes().to_vec()),
        ("the media type".to_owned(), MIME.as_bytes().to_vec()),
    ];
    out.extend(spellings("the key", key.expose_bytes()));
    for (n, purpose) in
        [Purpose::Content, Purpose::Presence, Purpose::Image, Purpose::ImageName].into_iter().enumerate()
    {
        out.extend(spellings(&format!("subkey {n}"), key.subkey(purpose).expose_bytes()));
    }
    for (n, p) in pictures.iter().enumerate() {
        out.push((format!("picture {n}"), p.bytes.clone()));
        out.extend(spellings(&format!("file id {n}"), &raw_id(&p.id)));
    }
    out
}

/// What a leak check looks at: the relay's storage and everything that crossed the wire.
struct Evidence {
    storage: Vec<Vec<u8>>,
    wire: Vec<Vec<u8>>,
    /// The `Authorization` headers, and nothing else of the HTTP side.
    headers: Vec<Vec<u8>>,
}

impl Evidence {
    fn new(storage: Vec<Vec<u8>>, taps: &Taps) -> Self {
        let mut wire = taps.frames();
        let mut headers = Vec::new();
        for e in taps.exchanges() {
            wire.push(e.url.into_bytes());
            wire.push(e.request);
            wire.push(e.response);
            headers.push(e.auth.into_bytes());
        }
        Evidence { storage, wire, headers }
    }

    /// Nothing forbidden is in storage, on the wire or in a header.
    fn assert_blind(&self, forbidden: &[(String, Vec<u8>)]) {
        for (what, needle) in forbidden {
            for (place, haystacks) in
                [("storage", &self.storage), ("the wire", &self.wire), ("a header", &self.headers)]
            {
                assert!(!haystacks.iter().any(|h| contains(h, needle)), "{what} is visible in {place}");
            }
        }
    }

    /// The token is in the join frames and the headers and nowhere else.
    fn assert_token_only_where_it_belongs(&self, keys: &RoomKeys) {
        let token = keys.membership_token().expose_bytes().to_vec();
        let spelled = spellings("the token", &token);
        let in_frames: Vec<&Vec<u8>> = self.wire.iter().filter(|f| contains(f, &token)).collect();
        assert!(!in_frames.is_empty(), "the join frame carries the token");
        for frame in in_frames {
            assert!(
                matches!(ClientMessage::decode(frame), Ok(ClientMessage::Join { .. })),
                "the token is in a frame that is no join"
            );
        }
        let b64 = Base64UrlUnpadded::encode_string(&token);
        assert!(
            !self.headers.is_empty() && self.headers.iter().all(|h| h == format!("Drafft-Member {b64}").as_bytes())
        );
        for (what, needle) in &spelled {
            assert!(!self.storage.iter().any(|h| contains(h, needle)), "{what} is stored");
        }
        // No picture body or address carries the token in any spelling.
        for (what, needle) in spelled.iter().skip(1) {
            let leaks = self
                .wire
                .iter()
                .any(|h| contains(h, needle) && !matches!(ClientMessage::decode(h), Ok(ClientMessage::Join { .. })));
            assert!(!leaks, "{what} is on the wire outside a join");
        }
    }
}

/// The name of a picture is in storage as the relay keeps it (hex in a file name, 16 bytes in a database), and
/// looks nothing like the file id.
fn assert_names(storage: &[Vec<u8>], pictures: &[Shared]) {
    for p in pictures {
        let raw = p.name.as_bytes();
        assert!(
            storage.iter().any(|h| contains(h, raw) || contains(h, hex(raw).as_bytes())),
            "the keyed name of a picture is stored"
        );
        let id_hex = p.id.as_str();
        let name_text = p.name.to_string();
        assert_ne!(name_text, id_hex);
        assert_ne!(hex(raw), id_hex);
        for end in 1..=id_hex.len() {
            assert_ne!(name_text, &id_hex[..end], "the name is no prefix of the file id");
            assert_ne!(hex(raw), &id_hex[..end.min(32)], "the name is no prefix of the file id");
        }
        assert_ne!(raw.as_slice(), &raw_id(&p.id)[..raw.len()]);
        assert!(!name_text.contains(id_hex));
    }
}

async fn canary(config: Config, stored_under: &Path) {
    let key = RoomKey::from_bytes([0x5a; 32]);
    let keys = RoomKeys::new(&key);
    let server = drafftink_server::start(config).await.expect("starts");
    let addr = server.local_addr();
    let url = format!("ws://{addr}/ws");
    let taps = Taps::default();
    let (url2, taps2, key2) = (url.clone(), taps.clone(), RoomKey::from_bytes(*key.expose_bytes()));
    let real = Shared::new(&keys, picture(MARKER, 1, 4_000));
    let bytes = real.bytes.clone();

    // The real route: client A holds a manifest and offers the bytes, client B reads the manifest and wants them.
    let arrived = tokio::task::spawn_blocking(move || {
        let clock = Clock::new();
        let mut a = peer(1, &key2, &url2, JoinMode::Create, &taps2, BlobOptions::default());
        let mut b = peer(2, &key2, &url2, JoinMode::Create, &taps2, BlobOptions::default());
        let mut e = Element::new(
            ElementId::new(MARKER).unwrap(),
            ElementKind::Rectangle,
            ZIndex::rebalance(1).unwrap().remove(0),
        );
        e.transform = Transform::new(1.0, 2.0, 3.0, 4.0);
        let mut tx = a.store.begin();
        tx.insert(e).unwrap();
        tx.commit().unwrap();
        let (id, shared) = a.put_picture(clock, MIME, &bytes);
        assert_eq!(a.store.manifest(&id).map(|m| m.size), Some(bytes.len() as u64), "a manifest, in the document");
        a.offer(clock, &id, &shared);
        a.session.client_mut().local_changed(clock.now());
        a.session.client_mut().set_cursor(clock.now(), Some(Point { x: 1.0, y: 2.0 }));
        pump_until(clock, &mut [&mut a, &mut b], "b to see the drawing and the manifest", |p| {
            p[1].store.iter().count() == 1 && p[1].store.manifest(&id).is_some() && p[0].settled()
        });
        assert!(b.store.file(&id).is_none(), "b has the manifest and not the bytes");
        assert_eq!(a.session.client().images().room_images, 1, "the relay reported one picture");
        b.want(clock, &id);
        pump_until(clock, &mut [&mut a, &mut b], "b to download the picture", |p| p[1].arrived(&id).is_some());
        let got = b.arrived(&id).unwrap();
        let supplied = b.store.supply_file(FileEntry { mime: MIME.to_owned(), bytes: got.clone() }).unwrap();
        assert!(supplied, "the downloaded bytes fit the manifest");
        // Let the presence of both go out.
        pump_for(clock, &mut [&mut a, &mut b], 1_500);
        got.to_vec()
    })
    .await
    .unwrap();
    assert_eq!(arrived, real.bytes, "b got the bytes a offered");

    // The independent route: a hand-made sealed picture put through the image route, as any client could.
    let hand = Shared::new(&keys, picture(MARKER, 2, 700));
    let sealed = keys.seal_image(&hand.name, &hand.bytes).unwrap().into_bytes();
    let token = Base64UrlUnpadded::encode_string(keys.membership_token().expose_bytes());
    let header = [("Authorization", format!("Drafft-Member {token}"))];
    let (status, _, answer) =
        http_request(addr, "PUT", &format!("/r/{}/f/{}", keys.room_id(), hand.name), &header, &sealed).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&answer));
    taps.push_frame(sealed);
    taps.push_frame(answer);
    server.shutdown().await;

    let pictures = [real, hand];
    let stored = files_under(stored_under);
    let size: usize = stored.iter().map(Vec::len).sum();
    assert!(
        size > 1_200,
        "the relay stored the room, its log and the pictures: {size} bytes in {} items",
        stored.len()
    );
    let evidence = Evidence::new(stored, &taps);
    assert!(evidence.wire.len() > 10, "frames were recorded: {}", evidence.wire.len());
    assert!(taps.count("PUT", Some(1)) >= 1 && taps.count("GET", Some(2)) >= 1, "the pictures went over HTTP");
    evidence.assert_blind(&forbidden(&key, &pictures));
    assert_names(&evidence.storage, &pictures);
    evidence.assert_token_only_where_it_belongs(&keys);
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

/// The check can fail: a picture that is stored in the clear is found.
#[test]
fn the_search_finds_what_it_looks_for() {
    let key = RoomKey::from_bytes([0x5a; 32]);
    let keys = RoomKeys::new(&key);
    let leaked = Shared::new(&keys, picture(MARKER, 3, 500));
    let taps = Taps::default();
    let needles = forbidden(&key, std::slice::from_ref(&leaked));
    for (what, needle) in &needles {
        let clean = Evidence::new(vec![b"nothing to see".to_vec()], &taps);
        clean.assert_blind(std::slice::from_ref(&(what.clone(), needle.clone())));
        let dirty = Evidence::new(vec![[b"xx".as_slice(), needle, b"yy"].concat()], &taps);
        let found = std::panic::catch_unwind(|| dirty.assert_blind(&[(what.clone(), needle.clone())]));
        assert!(found.is_err(), "{what} is found in storage");
    }
}
