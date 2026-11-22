#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Sealed rooms on the server: `/r/<id>/ws` and the image routes, with real sealed records.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use base64ct::{Base64UrlUnpadded, Encoding};
use common::*;
use drafftink_protocol::ClientId;
use drafftink_protocol::v2::{
    ClientMessage, ErrorCode, JoinMode, MembershipToken, Proof, RecordHeader, RecordKind, RoomId, ServerMessage,
};
use drafftink_relay::{FlushRules, Retention};
use drafftink_seal::{RoomKey, RoomKeys};
use drafftink_server::config::Config;
use drafftink_server::store::StorageConfig;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

struct Room {
    key: RoomKey,
    keys: RoomKeys,
    id: RoomId,
}

fn room() -> Room {
    let key = RoomKey::from_bytes([42; 32]);
    let keys = RoomKeys::new(&key);
    Room { id: keys.room_id(), keys, key }
}

fn auth(keys: &RoomKeys) -> (&'static str, String) {
    (
        "Authorization",
        format!("Drafft-Member {}", Base64UrlUnpadded::encode_string(keys.membership_token().expose_bytes())),
    )
}

struct Sealed {
    ws: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    id: ClientId,
    ctr: u64,
}

impl Sealed {
    async fn connect(addr: SocketAddr, room: &RoomId, n: u8) -> Sealed {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/r/{room}/ws")).await.expect("connect");
        Sealed { ws, id: client_id(n), ctr: 0 }
    }

    async fn send(&mut self, message: &ClientMessage) {
        let _ = self.ws.send(Message::Binary(message.encode().expect("encodes").into())).await;
    }

    async fn join(&mut self, room: &Room, mode: JoinMode, last_seq: u64) {
        let message = ClientMessage::Join {
            min_version: 2,
            max_version: 2,
            room: room.id,
            client_id: self.id,
            mode,
            proof: Proof::token(room.keys.membership_token()),
            last_seq,
            captcha: None,
        };
        self.send(&message).await;
    }

    async fn append(&mut self, room: &Room, payload: &[u8]) -> u64 {
        self.ctr += 1;
        let header = RecordHeader::new(RecordKind::Update, self.id, self.ctr, 0).expect("header");
        let record = room.keys.seal(header, payload).expect("seals").into_bytes();
        self.send(&ClientMessage::Append { record }).await;
        self.ctr
    }

    async fn next(&mut self) -> Option<ServerMessage> {
        loop {
            match tokio::time::timeout(WAIT, self.ws.next()).await {
                Err(_) | Ok(None | Some(Err(_))) => return None,
                Ok(Some(Ok(Message::Binary(b)))) => {
                    return Some(ServerMessage::decode(&b).expect("relay frames decode"));
                }
                Ok(Some(Ok(Message::Close(_)))) => return None,
                Ok(Some(Ok(_))) => continue,
            }
        }
    }

    async fn until<T>(&mut self, mut pick: impl FnMut(&ServerMessage) -> Option<T>) -> T {
        loop {
            let message = self.next().await.expect("a message before the connection ended");
            if let Some(found) = pick(&message) {
                return found;
            }
        }
    }

    async fn expect_error(&mut self) -> ErrorCode {
        let code = self.until(|m| if let ServerMessage::Error { code, .. } = m { Some(*code) } else { None }).await;
        assert!(self.next().await.is_none(), "the relay closes after {code:?}");
        code
    }
}

fn config() -> Config {
    let mut config = test_config();
    config.relay.flush = FlushRules { interval_ms: 100, ..config.relay.flush };
    config
}

fn on_disk(dir: &std::path::Path) -> Config {
    Config { storage: StorageConfig::Filesystem { dir: dir.to_path_buf() }, ..config() }
}

#[tokio::test]
async fn two_members_exchange_records_and_the_relay_acks_after_storing() {
    let server = drafftink_server::start(config()).await.expect("starts");
    let (addr, r) = (server.local_addr(), room());
    let mut a = Sealed::connect(addr, &r.id, 1).await;
    a.join(&r, JoinMode::Create, 0).await;
    a.until(|m| matches!(m, ServerMessage::Joined { .. }).then_some(())).await;
    let mut b = Sealed::connect(addr, &r.id, 2).await;
    b.join(&r, JoinMode::Open, 0).await;
    b.until(|m| matches!(m, ServerMessage::Joined { .. }).then_some(())).await;
    let ctr = a.append(&r, b"hello from a").await;
    let ack = a.until(|m| if let ServerMessage::Ack { ctr, seq } = m { Some((*ctr, *seq)) } else { None }).await;
    assert_eq!(ack, (ctr, 1));
    let record = b.until(|m| if let ServerMessage::Record(r) = m { Some(r.clone()) } else { None }).await;
    assert_eq!((record.seq, record.from), (1, a.id));
    let sealed = drafftink_seal::SealedRecord::from_bytes(record.record).expect("a record");
    assert_eq!(r.keys.open(RecordKind::Update, &sealed).expect("opens"), b"hello from a");
    server.shutdown().await;
}

#[tokio::test]
async fn a_wrong_token_is_refused_before_any_room_is_loaded() {
    let server = drafftink_server::start(config()).await.expect("starts");
    let (addr, r) = (server.local_addr(), room());
    for proof in [
        Proof::token(&MembershipToken::from_bytes([1; 32])),
        Proof::new(drafftink_protocol::v2::ProofScheme::Token, vec![0; 5]),
    ] {
        let mut c = Sealed::connect(addr, &r.id, 1).await;
        let join = ClientMessage::Join {
            min_version: 2,
            max_version: 2,
            room: r.id,
            client_id: c.id,
            mode: JoinMode::Create,
            proof,
            last_seq: 0,
            captcha: None,
        };
        c.send(&join).await;
        assert_eq!(c.expect_error().await, ErrorCode::Unauthorized);
    }
    let mut c = Sealed::connect(addr, &r.id, 1).await;
    c.send(&ClientMessage::Catchup { after_seq: 0 }).await;
    assert_eq!(c.expect_error().await, ErrorCode::NotJoined);
    assert_eq!(server.loaded_sealed_rooms().await, 0, "no room task was started for a non-member");
    server.shutdown().await;
}

#[tokio::test]
async fn open_on_a_missing_room_creates_nothing_on_disk() {
    let dir = tempfile::tempdir().expect("dir");
    let server = drafftink_server::start(on_disk(dir.path())).await.expect("starts");
    let (addr, r) = (server.local_addr(), room());
    let mut c = Sealed::connect(addr, &r.id, 1).await;
    c.join(&r, JoinMode::Open, 0).await;
    assert_eq!(c.expect_error().await, ErrorCode::RoomMissing);
    server.shutdown().await;
    let files: Vec<_> = walk(dir.path());
    assert!(files.is_empty(), "nothing was written: {files:?}");
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[tokio::test]
async fn a_room_survives_a_restart_and_a_late_joiner_catches_up_from_disk() {
    let dir = tempfile::tempdir().expect("dir");
    let r = room();
    let server = drafftink_server::start(on_disk(dir.path())).await.expect("starts");
    let mut a = Sealed::connect(server.local_addr(), &r.id, 1).await;
    a.join(&r, JoinMode::Create, 0).await;
    for n in 0..3 {
        a.append(&r, format!("edit {n}").as_bytes()).await;
    }
    a.until(|m| matches!(m, ServerMessage::Ack { seq: 3, .. }).then_some(())).await;
    server.shutdown().await;
    let server = drafftink_server::start(on_disk(dir.path())).await.expect("restarts");
    let mut b = Sealed::connect(server.local_addr(), &r.id, 2).await;
    b.join(&r, JoinMode::Open, 0).await;
    let seqs = b
        .until(|m| {
            if let ServerMessage::Catchup { records, .. } = m {
                Some(records.iter().map(|r| r.seq).collect::<Vec<_>>())
            } else {
                None
            }
        })
        .await;
    assert_eq!(seqs, [1, 2, 3]);
    server.shutdown().await;
}

#[tokio::test]
async fn shutting_down_writes_what_the_flush_window_still_holds() {
    let dir = tempfile::tempdir().expect("dir");
    let mut cfg = on_disk(dir.path());
    cfg.relay.flush.interval_ms = 60_000;
    let r = room();
    let server = drafftink_server::start(cfg.clone()).await.expect("starts");
    let mut a = Sealed::connect(server.local_addr(), &r.id, 1).await;
    a.join(&r, JoinMode::Create, 0).await;
    a.until(|m| matches!(m, ServerMessage::Joined { .. }).then_some(())).await;
    a.append(&r, b"not yet written").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    server.shutdown().await;
    let server = drafftink_server::start(cfg).await.expect("restarts");
    let mut b = Sealed::connect(server.local_addr(), &r.id, 2).await;
    b.join(&r, JoinMode::Open, 0).await;
    let seqs = b
        .until(|m| {
            if let ServerMessage::Catchup { records, .. } = m {
                Some(records.iter().map(|r| r.seq).collect::<Vec<_>>())
            } else {
                None
            }
        })
        .await;
    assert_eq!(seqs, [1]);
    server.shutdown().await;
}

#[tokio::test]
async fn no_plaintext_rooms_refuses_v1_and_keeps_sealed_rooms() {
    let mut cfg = config();
    cfg.plaintext_rooms = false;
    let server = drafftink_server::start(cfg).await.expect("starts");
    let mut v1 = Client::connect(server.local_addr(), 1, "plain-room").await;
    v1.send(&v1.join_message(None)).await;
    let codes = v1.expect_closed().await;
    assert_eq!(codes, [drafftink_protocol::ErrorCode::UnsupportedVersion]);
    let r = room();
    let mut c = Sealed::connect(server.local_addr(), &r.id, 1).await;
    c.join(&r, JoinMode::Create, 0).await;
    c.until(|m| matches!(m, ServerMessage::Joined { .. }).then_some(())).await;
    server.shutdown().await;
}

#[tokio::test]
async fn plaintext_rooms_still_work_by_default_next_to_sealed_ones() {
    let server = drafftink_server::start(config()).await.expect("starts");
    let mut v1 = Client::connect(server.local_addr(), 1, "plain-room").await;
    let (joined, _) = v1.join().await;
    assert!(matches!(joined, drafftink_protocol::ServerMessage::Joined { .. }));
    server.shutdown().await;
}

#[tokio::test]
async fn a_v1_client_on_the_sealed_route_gets_a_plain_refusal() {
    let server = drafftink_server::start(config()).await.expect("starts");
    let r = room();
    let mut c = Sealed::connect(server.local_addr(), &r.id, 1).await;
    let v1 = drafftink_protocol::ClientMessage::Join {
        min_version: 1,
        max_version: 1,
        room: drafftink_protocol::RoomId::new("plain-room").expect("room"),
        client_id: c.id,
        auth_token: None,
        version: Vec::new(),
        user: None,
    };
    let _ = c.ws.send(Message::Binary(v1.encode().expect("encodes").into())).await;
    let message = c.next().await.expect("an answer");
    let ServerMessage::Error { code: ErrorCode::UnsupportedVersion, message, .. } = message else {
        panic!("{message:?}")
    };
    assert!(message.contains("end-to-end encrypted"), "{message}");
    server.shutdown().await;
}

#[tokio::test]
async fn hostile_frames_never_start_a_room_or_hurt_the_server() {
    let server = drafftink_server::start(config()).await.expect("starts");
    let (addr, r) = (server.local_addr(), room());
    let mut rng = Rng(7);
    for n in 0..40 {
        let mut c = Sealed::connect(addr, &r.id, 1).await;
        let bytes = rng.bytes(1 + n * 7);
        let _ = c.ws.send(Message::Binary(bytes.into())).await;
        let _ = c.next().await;
    }
    let mut c = Sealed::connect(addr, &r.id, 1).await;
    let _ = c.ws.send(Message::Text("hello".into())).await;
    assert!(c.next().await.is_some());
    assert_eq!(server.loaded_sealed_rooms().await, 0);
    let (status, _, _) = http_get(addr, "/health").await;
    assert_eq!(status, 200);
    match tokio_tungstenite::connect_async(format!("ws://{addr}/r/not-a-room/ws")).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), 404, "an address that is no room id")
        }
        other => panic!("{other:?}"),
    }
    server.shutdown().await;
}

#[tokio::test]
async fn keepalive_text_is_answered_with_pong() {
    let server = drafftink_server::start(config()).await.expect("starts");
    let r = room();
    let mut c = Sealed::connect(server.local_addr(), &r.id, 1).await;
    c.join(&r, JoinMode::Create, 0).await;
    c.until(|m| matches!(m, ServerMessage::Joined { .. }).then_some(())).await;
    c.ws.send(Message::Text("ping".into())).await.expect("send");
    loop {
        match tokio::time::timeout(WAIT, c.ws.next()).await.expect("an answer").expect("open").expect("frame") {
            Message::Text(t) => {
                assert_eq!(t.as_str(), "pong");
                break;
            }
            _ => continue,
        }
    }
    server.shutdown().await;
}

#[tokio::test]
async fn a_room_with_a_ttl_is_deleted_after_the_last_visit_and_its_files_go() {
    let dir = tempfile::tempdir().expect("dir");
    let mut cfg = on_disk(dir.path());
    cfg.relay.retention = Retention::After {
        ttl: Duration::from_millis(600),
        grace: Duration::from_millis(200),
        recheck: Duration::from_secs(3600),
    };
    let r = room();
    let server = drafftink_server::start(cfg).await.expect("starts");
    let mut a = Sealed::connect(server.local_addr(), &r.id, 1).await;
    a.join(&r, JoinMode::Create, 0).await;
    a.until(|m| matches!(m, ServerMessage::Joined { expires_at_ms, .. } if *expires_at_ms > 0).then_some(())).await;
    a.append(&r, b"short lived").await;
    a.until(|m| matches!(m, ServerMessage::Ack { .. }).then_some(())).await;
    assert!(!walk(dir.path()).is_empty());
    drop(a);
    tokio::time::sleep(Duration::from_millis(1800)).await;
    assert!(walk(dir.path()).is_empty(), "the room is deleted from disk: {:?}", walk(dir.path()));
    let mut b = Sealed::connect(server.local_addr(), &r.id, 2).await;
    b.join(&r, JoinMode::Open, 0).await;
    assert_eq!(b.expect_error().await, ErrorCode::RoomMissing);
    server.shutdown().await;
}

#[tokio::test]
async fn a_room_past_its_deadline_is_deleted_by_a_restarted_server() {
    let dir = tempfile::tempdir().expect("dir");
    let mut cfg = on_disk(dir.path());
    cfg.relay.retention = Retention::After {
        ttl: Duration::from_millis(500),
        grace: Duration::from_millis(100),
        recheck: Duration::from_secs(3600),
    };
    let r = room();
    let server = drafftink_server::start(cfg.clone()).await.expect("starts");
    let mut a = Sealed::connect(server.local_addr(), &r.id, 1).await;
    a.join(&r, JoinMode::Create, 0).await;
    a.until(|m| matches!(m, ServerMessage::Joined { .. }).then_some(())).await;
    drop(a);
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.shutdown().await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    let server = drafftink_server::start(cfg).await.expect("restarts");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(walk(dir.path()).is_empty(), "{:?}", walk(dir.path()));
    server.shutdown().await;
}

mod images {
    use super::*;
    use drafftink_protocol::{MAX_IMAGE_BYTES, SEAL_OVERHEAD};

    fn sealed_image(r: &Room, name: &drafftink_protocol::v2::ImageName, size: usize) -> Vec<u8> {
        let payload = vec![0x5a; size];
        r.keys.seal_image(name, &payload).expect("seals").into_bytes()
    }

    async fn make_room(server: &drafftink_server::ServerHandle, r: &Room) -> Sealed {
        let mut a = Sealed::connect(server.local_addr(), &r.id, 1).await;
        a.join(r, JoinMode::Create, 0).await;
        a.until(|m| matches!(m, ServerMessage::Joined { .. }).then_some(())).await;
        a
    }

    #[tokio::test]
    async fn put_get_and_list_with_the_membership_token() {
        let server = drafftink_server::start(config()).await.expect("starts");
        let (addr, r) = (server.local_addr(), room());
        let _a = make_room(&server, &r).await;
        let name = r.keys.image_name(&[7; 32]);
        let body = sealed_image(&r, &name, 1000);
        let path = format!("/r/{}/f/{name}", r.id);
        let header = auth(&r.keys);
        let (status, _, answer) = http_request(addr, "PUT", &path, &[(header.0, header.1.clone())], &body).await;
        assert_eq!(status, 201, "{}", String::from_utf8_lossy(&answer));
        assert!(String::from_utf8_lossy(&answer).contains("\"room_image_bytes\""));
        let (status, _, got) = http_request(addr, "GET", &path, &[(header.0, header.1.clone())], &[]).await;
        assert_eq!((status, got), (200, body.clone()));
        let (status, _, list) =
            http_request(addr, "GET", &format!("/r/{}/f", r.id), &[(header.0, header.1.clone())], &[]).await;
        assert_eq!(status, 200);
        let list = String::from_utf8_lossy(&list).into_owned();
        assert!(list.contains(&name.to_string()) && list.contains(&body.len().to_string()), "{list}");
        let other = r.keys.image_name(&[8; 32]);
        let (status, _, missing) =
            http_request(addr, "GET", &format!("/r/{}/f/{other}", r.id), &[(header.0, header.1)], &[]).await;
        assert_eq!(status, 404);
        assert!(String::from_utf8_lossy(&missing).contains("image_missing"));
        server.shutdown().await;
    }

    #[tokio::test]
    async fn a_non_member_gets_401_and_a_room_that_does_not_exist_404() {
        let server = drafftink_server::start(config()).await.expect("starts");
        let (addr, r) = (server.local_addr(), room());
        let _a = make_room(&server, &r).await;
        let name = r.keys.image_name(&[7; 32]);
        let path = format!("/r/{}/f/{name}", r.id);
        let stranger = RoomKeys::new(&RoomKey::from_bytes([1; 32]));
        let wrong = auth(&stranger);
        for headers in [
            vec![],
            vec![(wrong.0, wrong.1)],
            vec![("Authorization", "Drafft-Member short".to_owned())],
            vec![("Authorization", "Bearer x".to_owned())],
        ] {
            let (status, _, body) = http_request(addr, "GET", &path, &headers, &[]).await;
            assert_eq!(status, 401, "{}", String::from_utf8_lossy(&body));
            let (status, _, _) = http_request(addr, "PUT", &path, &headers, &sealed_image(&r, &name, 10)).await;
            assert_eq!(status, 401);
        }
        let ghost = Room {
            key: RoomKey::from_bytes([3; 32]),
            keys: RoomKeys::new(&RoomKey::from_bytes([3; 32])),
            id: RoomKeys::new(&RoomKey::from_bytes([3; 32])).room_id(),
        };
        let header = auth(&ghost.keys);
        let (status, _, body) =
            http_request(addr, "GET", &format!("/r/{}/f/{name}", ghost.id), &[(header.0, header.1)], &[]).await;
        assert_eq!(status, 404, "{}", String::from_utf8_lossy(&body));
        assert!(String::from_utf8_lossy(&body).contains("room_missing"));
        let _ = ghost.key;
        server.shutdown().await;
    }

    #[tokio::test]
    async fn the_size_limit_is_exact_and_a_bad_body_is_a_400() {
        let server = drafftink_server::start(config()).await.expect("starts");
        let (addr, r) = (server.local_addr(), room());
        let _a = make_room(&server, &r).await;
        let header = auth(&r.keys);
        let name = r.keys.image_name(&[1; 32]);
        let exact = sealed_image(&r, &name, MAX_IMAGE_BYTES);
        assert_eq!(exact.len(), MAX_IMAGE_BYTES + SEAL_OVERHEAD);
        let path = format!("/r/{}/f/{name}", r.id);
        let (status, _, _) = http_request(addr, "PUT", &path, &[(header.0, header.1.clone())], &exact).await;
        assert_eq!(status, 201);
        let name2 = r.keys.image_name(&[2; 32]);
        let mut over = exact.clone();
        over.push(0);
        let (status, _, _) =
            http_request(addr, "PUT", &format!("/r/{}/f/{name2}", r.id), &[(header.0, header.1.clone())], &over).await;
        assert_eq!(status, 413);
        let (status, _, _) = http_request(
            addr,
            "PUT",
            &format!("/r/{}/f/{name2}", r.id),
            &[(header.0, header.1.clone())],
            b"not a record",
        )
        .await;
        assert_eq!(status, 400);
        let (status, _, _) =
            http_request(addr, "PUT", &format!("/r/{}/f/not-a-name", r.id), &[(header.0, header.1)], b"x").await;
        assert_eq!(status, 400);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn the_room_quota_answers_507_and_images_persist_across_a_restart() {
        let dir = tempfile::tempdir().expect("dir");
        let mut cfg = on_disk(dir.path());
        cfg.relay.limits.max_room_images = 2;
        cfg.relay.max_image_puts_per_sec = 100;
        let r = room();
        let server = drafftink_server::start(cfg.clone()).await.expect("starts");
        let _a = make_room(&server, &r).await;
        let header = auth(&r.keys);
        for n in 0..3u8 {
            let name = r.keys.image_name(&[n; 32]);
            let (status, _, _) = http_request(
                server.local_addr(),
                "PUT",
                &format!("/r/{}/f/{name}", r.id),
                &[(header.0, header.1.clone())],
                &sealed_image(&r, &name, 100),
            )
            .await;
            assert_eq!(status, if n < 2 { 201 } else { 507 }, "image {n}");
        }
        server.shutdown().await;
        let server = drafftink_server::start(cfg).await.expect("restarts");
        let name = r.keys.image_name(&[1; 32]);
        let (status, _, body) =
            http_request(server.local_addr(), "GET", &format!("/r/{}/f/{name}", r.id), &[(header.0, header.1)], &[])
                .await;
        assert_eq!(status, 200);
        let sealed = drafftink_seal::SealedRecord::from_bytes(body).expect("a sealed record");
        assert_eq!(r.keys.open_image(&name, &sealed).expect("opens"), vec![0x5a; 100]);
        server.shutdown().await;
    }
}
