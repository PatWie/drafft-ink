//! The canary of the blind-relay suites: what a relay must never hold, where to look for it, and a session of real
//! clients that puts all of it into a room. `blind.rs` runs it against the in-process server; the parity suite runs it
//! against every transport target, among them the Worker under `wrangler dev`, and reads that target's persisted files.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Real sockets and the real clock.
#![allow(clippy::disallowed_methods)]

use std::net::SocketAddr;
use std::path::Path;

use base64ct::{Base64, Base64UrlUnpadded, Encoding};
use drafftink_protocol::Point;
use drafftink_protocol::v2::{ClientMessage, ImageName, JoinMode};
use drafftink_scene::{
    Element, ElementId, ElementKind, FileEntry, FileId, SceneRead, SceneStore, SceneTx, Transform, ZIndex,
};
use drafftink_seal::{Purpose, RoomKey, RoomKeys};

use super::room_client::{BlobOptions, Clock, Taps, USER, peer, picture, pump_for, pump_until, raw_id};

pub const MARKER: &str = "CANARY-7f3a91c2";
pub const MIME: &str = "image/png";

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|x| format!("{x:02x}")).collect()
}

/// Every file under `dir`: its content, and its path as text (a file name can give a secret away too).
pub fn files_under(dir: &Path) -> Vec<Vec<u8>> {
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

/// The paths of the files under `dir`, for a look at what was searched.
fn list_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(list_files(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// A picture the relay was given.
pub struct Shared {
    pub bytes: Vec<u8>,
    pub id: FileId,
    pub name: ImageName,
}

impl Shared {
    pub fn new(keys: &RoomKeys, bytes: Vec<u8>) -> Self {
        let id = FileId::of(&bytes);
        let name = keys.image_name(&raw_id(&id));
        Shared { bytes, id, name }
    }
}

/// Every spelling of `bytes` that a leak could take.
pub fn spellings(what: &str, bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    vec![
        (what.to_owned(), bytes.to_vec()),
        (format!("{what} in hex"), hex(bytes).into_bytes()),
        (format!("{what} in base64"), Base64::encode_string(bytes).into_bytes()),
        (format!("{what} in base64url"), Base64UrlUnpadded::encode_string(bytes).into_bytes()),
    ]
}

/// Everything the relay must never hold or see.
pub fn forbidden(key: &RoomKey, pictures: &[Shared]) -> Vec<(String, Vec<u8>)> {
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
pub struct Evidence {
    pub storage: Vec<Vec<u8>>,
    pub wire: Vec<Vec<u8>>,
    /// The `Authorization` headers, and nothing else of the HTTP side.
    pub headers: Vec<Vec<u8>>,
}

impl Evidence {
    pub fn new(storage: Vec<Vec<u8>>, taps: &Taps) -> Self {
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
    pub fn assert_blind(&self, forbidden: &[(String, Vec<u8>)]) {
        for (what, needle) in forbidden {
            for (place, haystacks) in
                [("storage", &self.storage), ("the wire", &self.wire), ("a header", &self.headers)]
            {
                assert!(!haystacks.iter().any(|h| contains(h, needle)), "{what} is visible in {place}");
            }
        }
    }

    /// The token is in the join frames and the headers and nowhere else.
    pub fn assert_token_only_where_it_belongs(&self, keys: &RoomKeys) {
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
pub fn assert_names(storage: &[Vec<u8>], pictures: &[Shared]) {
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

/// The room key every canary session uses.
pub fn canary_key() -> RoomKey {
    RoomKey::from_bytes([0x5a; 32])
}

/// What a canary session put into the relay.
pub struct Session {
    /// The pictures the relay was given: the one the clients moved, and the one put by hand.
    pub pictures: [Shared; 2],
    /// Everything that crossed the wire.
    pub taps: Taps,
    /// More things the relay must never hold: the service token of a private server (it belongs to the gate in front
    /// of the relay, which keeps it to itself).
    pub extra: Vec<(String, Vec<u8>)>,
}

/// Two real clients draw text that holds the marker, name themselves, move a cursor and share a picture through the
/// real picture path; then a hand-made `PUT` stores a second picture as any client could. `url` is the address of the
/// relay's socket (`ws://host:port/ws`) and `addr` its host and port. The relay must be running and must have a
/// room for [`canary_key`] free to create. `taps` records the session and carries the service token, if the relay has
/// a gate.
pub fn run_session(url: &str, addr: SocketAddr, taps: Taps) -> Session {
    let key = canary_key();
    let keys = RoomKeys::new(&key);
    let real = Shared::new(&keys, picture(MARKER, 1, 4_000));
    let bytes = real.bytes.clone();

    // The real route: client A holds a manifest and offers the bytes, client B reads the manifest and wants them.
    let clock = Clock::new();
    let mut a = peer(1, &key, url, JoinMode::Create, &taps, BlobOptions::default());
    let mut b = peer(2, &key, url, JoinMode::Create, &taps, BlobOptions::default());
    let mut e =
        Element::new(ElementId::new(MARKER).unwrap(), ElementKind::Rectangle, ZIndex::rebalance(1).unwrap().remove(0));
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
    assert_eq!(got.to_vec(), real.bytes, "b got the bytes a offered");

    // The independent route: a hand-made sealed picture put through the image route, as any client could.
    let hand = Shared::new(&keys, picture(MARKER, 2, 700));
    let sealed = keys.seal_image(&hand.name, &hand.bytes).unwrap().into_bytes();
    let token = Base64UrlUnpadded::encode_string(keys.membership_token().expose_bytes());
    let mut headers = vec![("Authorization", format!("Drafft-Member {token}"))];
    headers.extend(taps.access_headers());
    let reply = super::http::request(addr, "PUT", &format!("/r/{}/f/{}", keys.room_id(), hand.name), &headers, &sealed);
    assert_eq!(reply.status, 201, "{}", String::from_utf8_lossy(&reply.body));
    taps.push_frame(sealed);
    taps.push_frame(reply.body);
    let extra = taps
        .access_headers()
        .into_iter()
        .flat_map(|(name, value)| spellings(&format!("the value of {name}"), value.as_bytes()))
        .collect();
    Session { pictures: [real, hand], taps, extra }
}

/// What a finished canary session leaves to check, once the relay has stopped: everything under `stored_under`
/// searched for what must not be there, names present, the token only where it belongs. Panics with the reason.
pub fn assert_blind_relay(session: &Session, stored_under: &Path) {
    let key = canary_key();
    let keys = RoomKeys::new(&key);
    let stored = files_under(stored_under);
    let size: usize = stored.iter().map(Vec::len).sum();
    assert!(
        size > 1_200,
        "the relay stored the room, its log and the pictures: {size} bytes in {} items under {}",
        stored.len(),
        stored_under.display()
    );
    eprintln!("canary: searched {} items, {size} bytes, under {}", stored.len(), stored_under.display());
    if std::env::var_os("DRAFFTINK_PARITY_LIST").is_some() {
        for path in list_files(stored_under) {
            eprintln!("canary:   {}", path.strip_prefix(stored_under).unwrap_or(&path).display());
        }
    }
    let evidence = Evidence::new(stored, &session.taps);
    assert!(evidence.wire.len() > 10, "frames were recorded: {}", evidence.wire.len());
    assert!(
        session.taps.count("PUT", Some(1)) >= 1 && session.taps.count("GET", Some(2)) >= 1,
        "the pictures went over HTTP"
    );
    let mut needles = forbidden(&key, &session.pictures);
    needles.extend(session.extra.iter().cloned());
    evidence.assert_blind(&needles);
    assert_names(&evidence.storage, &session.pictures);
    evidence.assert_token_only_where_it_belongs(&keys);
}

/// The search can fail on this target's files: a forbidden thing that is written into a file deep under `stored_under`
/// is found, and the folder is clean again afterwards.
pub fn assert_search_can_fail(session: &Session, stored_under: &Path) {
    let key = canary_key();
    let mut needles = forbidden(&key, &session.pictures);
    needles.extend(session.extra.iter().cloned());
    let deep = stored_under.join("proof").join("of").join("search");
    std::fs::create_dir_all(&deep).unwrap();
    for (what, needle) in &needles {
        let file = deep.join("leak.bin");
        std::fs::write(&file, [b"xx".as_slice(), needle, b"yy"].concat()).unwrap();
        let evidence = Evidence::new(files_under(stored_under), &Taps::default());
        let found = std::panic::catch_unwind(|| evidence.assert_blind(&[(what.clone(), needle.clone())]));
        assert!(found.is_err(), "{what} is found in a file under {}", stored_under.display());
    }
    std::fs::remove_dir_all(stored_under.join("proof")).unwrap();
    assert_blind_relay(session, stored_under);
}
