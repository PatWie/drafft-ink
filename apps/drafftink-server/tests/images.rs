#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Real sockets and the real clock.
#![allow(clippy::disallowed_methods)]
//! Pictures of an encrypted room through the real client (`SealedClient` with `NativeBlobTransport`) against the
//! real server: limits, refusals, damaged downloads, restarts, a client that joins late, the same picture twice.
//! The HTTP routes alone are in `sealed.rs` and `blobs.rs`; the client alone against the relay core in simulated time
//! is `crates/collab/tests/images.rs`.

mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use base64ct::{Base64UrlUnpadded, Encoding};
use common::room_client::{BlobOptions, Clock, Peer, Taps, drive, drive_for, peer, picture, raw_id};
use common::*;
use drafftink_collab::{BlobAuth, ImageRefusal};
use drafftink_protocol::v2::JoinMode;
use drafftink_protocol::{MAX_IMAGE_BYTES, SEAL_OVERHEAD};
use drafftink_scene::FileId;
use drafftink_seal::{RoomKey, RoomKeys};
use drafftink_server::ServerHandle;
use drafftink_server::config::Config;
use drafftink_server::store::StorageConfig;

const MIME: &str = "image/png";
const MARKER: &str = "CANARY-7f3a91c2";

/// A room with the key `[n; 32]` on a server, and what a test needs to talk to it.
struct Rig {
    key: RoomKey,
    keys: RoomKeys,
    taps: Taps,
    clock: Clock,
}

impl Rig {
    fn new(n: u8) -> Rig {
        let key = RoomKey::from_bytes([n; 32]);
        Rig { keys: RoomKeys::new(&key), key, taps: Taps::default(), clock: Clock::new() }
    }

    fn peer(&self, n: u8, addr: SocketAddr, options: BlobOptions) -> Peer {
        peer(n, &self.key, &format!("ws://{addr}/ws"), JoinMode::Create, &self.taps, options)
    }

    fn header(&self) -> [(&'static str, String); 1] {
        let token = Base64UrlUnpadded::encode_string(self.keys.membership_token().expose_bytes());
        [("Authorization", format!("Drafft-Member {token}"))]
    }

    /// The names the relay lists for the room, asked as a member.
    async fn listed(&self, addr: SocketAddr) -> Vec<String> {
        let path = format!("/r/{}/f", self.keys.room_id());
        let (status, _, body) = http_request(addr, "GET", &path, &self.header(), &[]).await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
        list.as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap().to_owned()).collect()
    }

    fn name(&self, id: &FileId) -> String {
        self.keys.image_name(&raw_id(id)).to_string()
    }
}

/// A server with a small drawing budget for pictures, in memory.
fn memory_config(limit: impl FnOnce(&mut Config)) -> Config {
    let mut config = test_config();
    config.relay.max_image_puts_per_sec = 100;
    limit(&mut config);
    config
}

fn disk_config(dir: &Path) -> Config {
    memory_config(|c| c.storage = StorageConfig::Filesystem { dir: dir.to_path_buf() })
}

/// Every file under `dir`.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// Both peers are online, and `a` has nothing left to send.
fn both_settled(p: &[&mut Peer]) -> bool {
    p.iter().all(|p| p.settled())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_picture_of_exactly_the_limit_arrives_and_one_byte_more_is_refused_by_the_client() {
    let server = drafftink_server::start(memory_config(|_| {})).await.expect("starts");
    let (addr, rig) = (server.local_addr(), Rig::new(1));
    let (mut a, mut b) = (rig.peer(1, addr, BlobOptions::default()), rig.peer(2, addr, BlobOptions::default()));
    drive(rig.clock, &mut [&mut a, &mut b], "both online", both_settled).await;

    let exact = picture(MARKER, 1, MAX_IMAGE_BYTES);
    let (id, bytes) = a.put_picture(rig.clock, MIME, &exact);
    a.offer(rig.clock, &id, &bytes);
    drive(rig.clock, &mut [&mut a, &mut b], "the manifest", |p| p[1].store.manifest(&id).is_some()).await;
    b.want(rig.clock, &id);
    drive(rig.clock, &mut [&mut a, &mut b], "the biggest picture", |p| p[1].arrived(&id).is_some()).await;
    assert_eq!(b.arrived(&id).unwrap().as_ref(), exact.as_slice());
    let put = rig.taps.exchanges().into_iter().find(|e| e.method == "PUT").unwrap();
    assert_eq!((put.status, put.request.len()), (Some(201), MAX_IMAGE_BYTES + SEAL_OVERHEAD));

    let over = picture(MARKER, 2, MAX_IMAGE_BYTES + 1);
    let (id2, bytes2) = a.put_picture(rig.clock, MIME, &over);
    let puts = rig.taps.count("PUT", None);
    a.offer(rig.clock, &id2, &bytes2);
    drive(rig.clock, &mut [&mut a, &mut b], "the refusal", |p| p[0].rejected(&id2).is_some()).await;
    assert_eq!(a.rejected(&id2), Some(ImageRefusal::TooLarge));
    drive_for(rig.clock, &mut [&mut a, &mut b], 300).await;
    assert_eq!(rig.taps.count("PUT", None), puts, "the client refuses before it sends anything");
    assert_eq!(a.session.client().images().failed, 1);
    // The manifest is still on its way to the relay for a moment; once it is acknowledged nothing is pending, because
    // a refused picture is not an upload that waits.
    drive(rig.clock, &mut [&mut a, &mut b], "nothing pending", |p| p[0].settled()).await;
    assert_eq!(rig.listed(addr).await, [rig.name(&id)], "only the picture within the limit is stored");
    server.shutdown().await;
}

async fn room_full(limit: impl FnOnce(&mut Config), sizes: [usize; 3]) {
    let server = drafftink_server::start(memory_config(limit)).await.expect("starts");
    let (addr, rig) = (server.local_addr(), Rig::new(2));
    let mut a = rig.peer(1, addr, BlobOptions::default());
    drive(rig.clock, &mut [&mut a], "online", |p| p[0].settled()).await;
    let mut ids = Vec::new();
    for (seed, len) in (1u8..).zip(sizes) {
        let (id, bytes) = a.put_picture(rig.clock, MIME, &picture(MARKER, seed, len));
        a.offer(rig.clock, &id, &bytes);
        ids.push(id);
    }
    drive(rig.clock, &mut [&mut a], "every offer to be settled", |p| {
        let images = p[0].session.client().images();
        p[0].settled() && images.uploading == 0 && images.waiting == 0
    })
    .await;
    let refused: Vec<_> = ids.iter().filter(|id| a.rejected(id).is_some()).collect();
    assert_eq!(refused.len(), 1, "exactly one picture does not fit: {:?}", a.images);
    assert_eq!(a.rejected(refused[0]), Some(ImageRefusal::RoomFull));
    let sent = rig.taps.exchanges().into_iter().filter(|e| e.method == "PUT").collect::<Vec<_>>();
    assert_eq!(sent.iter().filter(|e| e.status == Some(507)).count(), 1, "the relay said 507 once");
    let stored = rig.listed(addr).await;
    assert_eq!(stored.len(), 2);
    assert!(!stored.contains(&rig.name(refused[0])), "the refused picture is not stored");
    let images = a.session.client().images();
    assert_eq!((images.room_images, images.failed), (2, 1));
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_room_over_its_byte_budget_answers_507_and_the_client_reports_room_full() {
    // Two sealed pictures of 1000 bytes fit, a third does not.
    let budget = 2 * (1_000 + SEAL_OVERHEAD) as u64 + 10;
    room_full(|c| c.relay.limits.max_room_image_bytes = budget, [1_000, 1_000, 1_000]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_room_over_its_picture_count_answers_507_and_the_client_reports_room_full() {
    room_full(|c| c.relay.limits.max_room_images = 2, [300, 400, 500]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_of_another_room_is_refused_with_401_and_gets_and_stores_nothing() {
    let server = drafftink_server::start(memory_config(|_| {})).await.expect("starts");
    let (addr, rig) = (server.local_addr(), Rig::new(3));
    let mut a = rig.peer(1, addr, BlobOptions::default());
    drive(rig.clock, &mut [&mut a], "online", |p| p[0].settled()).await;
    let (id, bytes) = a.put_picture(rig.clock, MIME, &picture(MARKER, 1, 2_000));
    a.offer(rig.clock, &id, &bytes);
    drive(rig.clock, &mut [&mut a], "the upload", |p| p[0].session.client().images().room_images == 1).await;

    // This client holds the room's key for the socket, but its picture requests carry a token of another room.
    let other = RoomKeys::new(&RoomKey::from_bytes([99; 32]));
    let options = BlobOptions { auth_override: Some(BlobAuth::member(other.membership_token())), ..Default::default() };
    let mut s = rig.peer(9, addr, options);
    drive(rig.clock, &mut [&mut a, &mut s], "online", both_settled).await;
    let (id2, bytes2) = s.put_picture(rig.clock, MIME, &picture(MARKER, 2, 2_000));
    s.offer(rig.clock, &id2, &bytes2);
    s.want(rig.clock, &id);
    drive(rig.clock, &mut [&mut a, &mut s], "both refused", |p| p[1].session.client().images().failed == 2).await;
    drive_for(rig.clock, &mut [&mut a, &mut s], 300).await;
    assert!(s.images.is_empty(), "no event for a 401, and no bytes: {:?}", s.images);
    assert_eq!(s.session.client().images().failed, 2, "a 401 ends the retries");

    let sealed = rig.keys.seal_image(&rig.keys.image_name(&raw_id(&id)), &picture(MARKER, 1, 2_000)).unwrap();
    for e in rig.taps.exchanges().iter().filter(|e| e.who == 9) {
        assert_eq!(e.status, Some(401), "{} {}", e.method, e.url);
        assert!(e.response.len() < 300, "an error text and nothing else: {} bytes", e.response.len());
        assert!(!e.response.windows(MARKER.len()).any(|w| w == MARKER.as_bytes()));
        assert!(!e.response.windows(64).any(|w| sealed.as_bytes().windows(64).any(|s| s == w)));
    }
    assert!(rig.taps.exchanges().iter().any(|e| e.who == 9 && e.method == "GET"));
    assert_eq!(rig.listed(addr).await, [rig.name(&id)], "the stranger stored nothing");
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_damaged_download_is_reported_once_and_not_asked_for_again() {
    let dir = tempfile::tempdir().unwrap();
    let server = drafftink_server::start(disk_config(dir.path())).await.expect("starts");
    let (addr, rig) = (server.local_addr(), Rig::new(4));
    let mut a = rig.peer(1, addr, BlobOptions::default());
    drive(rig.clock, &mut [&mut a], "online", |p| p[0].settled()).await;
    let mut ids = Vec::new();
    for seed in 1..=3 {
        let (id, bytes) = a.put_picture(rig.clock, MIME, &picture(MARKER, seed, 3_000));
        a.offer(rig.clock, &id, &bytes);
        ids.push(id);
    }
    drive(rig.clock, &mut [&mut a], "three uploads", |p| p[0].session.client().images().room_images == 3).await;
    let [flipped, planted, intact] = [&ids[0], &ids[1], &ids[2]];

    // A bit flips in the stored bytes of the first picture.
    let hex_name: String =
        rig.keys.image_name(&raw_id(flipped)).as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let path = files(dir.path())
        .into_iter()
        .find(|p| p.file_name().is_some_and(|n| *n == *hex_name))
        .expect("stored as a file");
    let mut stored = std::fs::read(&path).unwrap();
    let middle = stored.len() / 2;
    stored[middle] ^= 0x01;
    std::fs::write(&path, stored).unwrap();

    // A valid seal over other bytes is put under the name of the second: the right name, the wrong hash.
    let name = rig.keys.image_name(&raw_id(planted));
    let forged = rig.keys.seal_image(&name, &picture(MARKER, 9, 3_000)).unwrap().into_bytes();
    let (status, _, answer) =
        http_request(addr, "PUT", &format!("/r/{}/f/{name}", rig.keys.room_id()), &rig.header(), &forged).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&answer));

    let mut b = rig.peer(2, addr, BlobOptions::default());
    drive(rig.clock, &mut [&mut a, &mut b], "b online", both_settled).await;
    for id in &ids {
        b.want(rig.clock, id);
    }
    drive(rig.clock, &mut [&mut a, &mut b], "all three answered", |p| {
        p[1].damaged(flipped) && p[1].damaged(planted) && p[1].arrived(intact).is_some()
    })
    .await;
    drive_for(rig.clock, &mut [&mut a, &mut b], 1_500).await;
    for id in [flipped, planted] {
        let gets = rig
            .taps
            .exchanges()
            .into_iter()
            .filter(|e| e.who == 2 && e.method == "GET" && e.name() == rig.name(id))
            .count();
        assert_eq!(gets, 1, "a damaged picture is not asked for again");
        assert!(b.arrived(id).is_none());
    }
    assert_eq!(b.session.client().images().failed, 2);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upload_waits_for_a_server_that_is_down_and_goes_out_after_the_restart() {
    let dir = tempfile::tempdir().unwrap();
    let server = drafftink_server::start(disk_config(dir.path())).await.expect("starts");
    let (addr, rig) = (server.local_addr(), Rig::new(5));
    let mut a = rig.peer(1, addr, BlobOptions::default());
    drive(rig.clock, &mut [&mut a], "online", |p| p[0].settled()).await;
    server.shutdown().await;
    drive(rig.clock, &mut [&mut a], "the loss of the connection", |p| !p[0].online()).await;

    // The picture is added while the relay is away.
    let (id, bytes) = a.put_picture(rig.clock, MIME, &picture(MARKER, 1, 5_000));
    a.offer(rig.clock, &id, &bytes);
    drive_for(rig.clock, &mut [&mut a], 300).await;
    assert!(a.session.client().status().unsynced, "the picture is pending, and the status says so");
    assert_eq!(rig.taps.count("PUT", None), 0);

    let server = restart(disk_config(dir.path()), addr).await;
    drive(rig.clock, &mut [&mut a], "the upload after the reconnect", |p| {
        p[0].settled() && p[0].session.client().images().room_images == 1
    })
    .await;
    assert_eq!(rig.taps.count("PUT", None), 1, "one upload, after the restart");
    assert_eq!(rig.listed(addr).await, [rig.name(&id)]);

    let mut b = rig.peer(2, addr, BlobOptions::default());
    drive(rig.clock, &mut [&mut a, &mut b], "the manifest", |p| p[1].store.manifest(&id).is_some()).await;
    b.want(rig.clock, &id);
    drive(rig.clock, &mut [&mut a, &mut b], "the picture", |p| p[1].arrived(&id).is_some()).await;
    assert_eq!(b.arrived(&id).unwrap().as_ref(), bytes.as_ref());
    server.shutdown().await;
}

/// Starts a server again on the address its predecessor had.
async fn restart(mut config: Config, addr: SocketAddr) -> ServerHandle {
    config.bind = addr;
    for _ in 0..50 {
        if let Ok(server) = drafftink_server::start(config.clone()).await {
            return server;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the port {addr} did not come back");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upload_whose_answer_was_lost_is_checked_with_head_and_not_repeated() {
    let server = drafftink_server::start(memory_config(|_| {})).await.expect("starts");
    let (addr, rig) = (server.local_addr(), Rig::new(6));
    let mut a = rig.peer(1, addr, BlobOptions { lose_first_put_answer: true, ..Default::default() });
    drive(rig.clock, &mut [&mut a], "online", |p| p[0].settled()).await;
    let (id, bytes) = a.put_picture(rig.clock, MIME, &picture(MARKER, 1, 4_000));
    a.offer(rig.clock, &id, &bytes);
    drive(rig.clock, &mut [&mut a], "the upload to be known", |p| {
        p[0].settled() && p[0].session.client().images().waiting == 0 && p[0].session.client().images().uploading == 0
    })
    .await;
    drive_for(rig.clock, &mut [&mut a], 300).await;
    assert_eq!(
        (rig.taps.count("PUT", None), rig.taps.count("HEAD", None)),
        (1, 1),
        "PUT, then HEAD, and no second PUT"
    );
    assert_eq!(rig.listed(addr).await, [rig.name(&id)]);
    assert_eq!(a.session.client().images().failed, 0);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_picture_offered_while_another_member_is_away_is_fetched_when_that_member_joins() {
    let dir = tempfile::tempdir().unwrap();
    let server = drafftink_server::start(disk_config(dir.path())).await.expect("starts");
    let (addr, rig) = (server.local_addr(), Rig::new(7));
    let (mut a, mut b) = (rig.peer(1, addr, BlobOptions::default()), rig.peer(2, addr, BlobOptions::default()));
    drive(rig.clock, &mut [&mut a, &mut b], "both online", both_settled).await;
    drop(b);

    let (id, bytes) = a.put_picture(rig.clock, MIME, &picture(MARKER, 1, 6_000));
    a.offer(rig.clock, &id, &bytes);
    drive(rig.clock, &mut [&mut a], "the upload", |p| {
        p[0].settled() && p[0].session.client().images().room_images == 1
    })
    .await;

    // A member that was not there joins: it learns the manifest from the document and the bytes from the relay.
    let mut c = rig.peer(3, addr, BlobOptions::default());
    drive(rig.clock, &mut [&mut a, &mut c], "the manifest", |p| p[1].store.manifest(&id).is_some()).await;
    assert!(c.session.client().images().room_images <= 1);
    c.want(rig.clock, &id);
    drive(rig.clock, &mut [&mut a, &mut c], "the picture", |p| p[1].arrived(&id).is_some()).await;
    assert_eq!(c.arrived(&id).unwrap().as_ref(), bytes.as_ref());
    assert_eq!(c.session.client().images().room_images, 1, "the list told the late joiner what the room holds");
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_picture_twice_stores_one_name_and_a_member_who_sees_it_listed_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let server = drafftink_server::start(disk_config(dir.path())).await.expect("starts");
    let (addr, rig) = (server.local_addr(), Rig::new(8));
    let (mut a, mut b) = (rig.peer(1, addr, BlobOptions::default()), rig.peer(2, addr, BlobOptions::default()));
    drive(rig.clock, &mut [&mut a, &mut b], "both online", both_settled).await;
    let data = picture(MARKER, 1, 3_500);
    let (id, bytes) = a.put_picture(rig.clock, MIME, &data);
    a.offer(rig.clock, &id, &bytes);
    a.offer(rig.clock, &id, &bytes);
    drive(rig.clock, &mut [&mut a, &mut b], "the upload", |p| p[0].session.client().images().room_images == 1).await;

    // A member that comes later holds the same picture (a copy of the same file) and offers it too. It reads the list
    // when it connects, finds the name and sends nothing.
    let mut c = rig.peer(3, addr, BlobOptions::default());
    drive(rig.clock, &mut [&mut a, &mut c], "the late member", both_settled).await;
    let (id_c, bytes_c) = c.put_picture(rig.clock, MIME, &data);
    assert_eq!(id_c, id);
    c.offer(rig.clock, &id_c, &bytes_c);
    drive_for(rig.clock, &mut [&mut a, &mut c], 1_000).await;
    assert_eq!(rig.taps.count("PUT", None), 1, "one upload for three offers by two members");
    assert_eq!(rig.taps.count("PUT", Some(3)), 0);
    assert_eq!(c.session.client().images().failed, 0);

    // A member that was online before the upload read an empty list. It uploads, the relay overwrites the same name.
    let (id_b, bytes_b) = b.put_picture(rig.clock, MIME, &data);
    b.offer(rig.clock, &id_b, &bytes_b);
    drive(rig.clock, &mut [&mut a, &mut b], "the second upload", |p| {
        p[1].settled() && p[1].session.client().images().uploading == 0 && rig.taps.count("PUT", Some(2)) == 1
    })
    .await;
    assert_eq!(rig.listed(addr).await, [rig.name(&id)], "still one name");
    let on_disk = files(dir.path()).into_iter().filter(|p| p.parent().is_some_and(|d| d.ends_with("images"))).count();
    assert_eq!(on_disk, 1, "one file for the picture");
    assert_eq!(b.session.client().images().room_images, 1, "and the room counts it once");
    server.shutdown().await;
}
