#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::disallowed_methods)] // native-only test: `Instant` is fine
//! A client's day against a running Worker: join a room, append, read it back from a second connection, store and
//! fetch a picture, and keep alive with `ping`. It needs `wrangler dev`, so it is ignored by default:
//!
//! ```text
//! apps/drafftink-worker/scripts/dev.sh --port 8791 &
//! DRAFFTINK_WORKER_URL=http://127.0.0.1:8791 cargo test -p drafftink-worker --test smoke -- --ignored
//! ```
//!
//! Every run makes a new room key, so runs do not meet each other's rooms. (The transport parity suite builds on the
//! same way of starting the Worker.)

use std::net::TcpStream;
use std::time::{Duration, Instant};

use base64ct::{Base64UrlUnpadded, Encoding};
use drafftink_protocol::ClientId;
use drafftink_protocol::v2::{ClientMessage, ErrorCode, JoinMode, Proof, RecordHeader, RecordKind, ServerMessage};
use drafftink_seal::{RoomKey, RoomKeys, SealedRecord};
use sha2::{Digest, Sha256};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const WAIT: Duration = Duration::from_secs(10);

fn base_url() -> String {
    std::env::var("DRAFFTINK_WORKER_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".to_owned())
}

struct Room {
    keys: RoomKeys,
    id: String,
}

fn new_room() -> Room {
    let keys = RoomKeys::new(&RoomKey::generate().expect("random key"));
    Room { id: keys.room_id().to_string(), keys }
}

struct Peer {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    id: ClientId,
    ctr: u64,
}

impl Peer {
    fn connect(room: &Room, n: u8) -> Peer {
        let url = base_url().replacen("http", "ws", 1) + &format!("/r/{}/ws", room.id);
        let (ws, _) = tungstenite::connect(url).expect("the upgrade works");
        if let MaybeTlsStream::Plain(stream) = ws.get_ref() {
            stream.set_read_timeout(Some(Duration::from_millis(250))).expect("timeout");
        }
        Peer { ws, id: ClientId::from_bytes([n; 16]), ctr: 0 }
    }

    fn send(&mut self, message: &ClientMessage) {
        self.ws.send(Message::Binary(message.encode().expect("encodes").into())).expect("sends");
    }

    fn join(&mut self, room: &Room, mode: JoinMode) {
        let message = ClientMessage::Join {
            min_version: 2,
            max_version: 2,
            room: room.keys.room_id(),
            client_id: self.id,
            mode,
            proof: Proof::token(room.keys.membership_token()),
            last_seq: 0,
            captcha: None,
        };
        self.send(&message);
    }

    fn append(&mut self, room: &Room, payload: &[u8]) -> u64 {
        self.ctr += 1;
        let header = RecordHeader::new(RecordKind::Update, self.id, self.ctr, 0).expect("header");
        let record = room.keys.seal(header, payload).expect("seals").into_bytes();
        self.send(&ClientMessage::Append { record });
        self.ctr
    }

    /// The next message, or `None` when nothing arrives in time or the socket is gone.
    fn next(&mut self, deadline: Instant) -> Option<Result<ServerMessage, String>> {
        while Instant::now() < deadline {
            match self.ws.read() {
                Ok(Message::Binary(frame)) => {
                    return Some(Ok(ServerMessage::decode(&frame).expect("relay frames decode")));
                }
                Ok(Message::Text(text)) => return Some(Err(text.to_string())),
                Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => return None,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => panic!("socket failed: {e}"),
            }
        }
        None
    }

    fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&ServerMessage) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        while let Some(item) = self.next(deadline) {
            if let Ok(message) = item
                && let Some(found) = pick(&message)
            {
                return found;
            }
        }
        panic!("timed out waiting for {what}");
    }
}

fn http() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().into()
}

fn auth(room: &Room) -> String {
    format!("Drafft-Member {}", Base64UrlUnpadded::encode_string(room.keys.membership_token().expose_bytes()))
}

#[test]
#[ignore = "needs a running `wrangler dev`: see the module docs"]
fn a_client_joins_appends_reads_back_and_stores_a_picture() {
    let room = new_room();
    let (mut a, mut b) = (Peer::connect(&room, 1), Peer::connect(&room, 2));

    a.join(&room, JoinMode::Create);
    a.until("Joined", |m| matches!(m, ServerMessage::Joined { .. }).then_some(()));
    b.join(&room, JoinMode::Open);
    b.until("Joined", |m| matches!(m, ServerMessage::Joined { .. }).then_some(()));

    let ctr = a.append(&room, b"hello from the smoke test");
    let ack = a.until("Ack", |m| if let ServerMessage::Ack { ctr, seq } = m { Some((*ctr, *seq)) } else { None });
    assert_eq!(ack, (ctr, 1), "the record is stored when it is acknowledged");
    let live = b.until("the live Record", |m| if let ServerMessage::Record(r) = m { Some(r.clone()) } else { None });
    assert_eq!((live.seq, live.from), (1, a.id));

    // A second connection reads it back from storage.
    let mut c = Peer::connect(&room, 3);
    c.join(&room, JoinMode::Open);
    let records =
        c.until(
            "Catchup",
            |m| if let ServerMessage::Catchup { records, .. } = m { Some(records.clone()) } else { None },
        );
    assert_eq!(records.len(), 1);
    let sealed = SealedRecord::from_bytes(records[0].record.clone()).expect("a sealed record");
    assert_eq!(room.keys.open(RecordKind::Update, &sealed).expect("opens"), b"hello from the smoke test");

    // The keepalive is answered with text, and does not disturb the room.
    c.ws.send(Message::text("ping")).expect("sends ping");
    let deadline = Instant::now() + WAIT;
    let mut pong = false;
    while let Some(item) = c.next(deadline) {
        if item == Err("pong".to_owned()) {
            pong = true;
            break;
        }
    }
    assert!(pong, "ping is answered with pong");

    // A picture: stored, listed, read back, and refused to a stranger.
    let png = b"\x89PNG\r\n\x1a\nsmoke test pixels".to_vec();
    let file_id: [u8; 32] = Sha256::digest(&png).into();
    let name = room.keys.image_name(&file_id);
    let sealed_image = room.keys.seal_image(&name, &png).expect("seals").into_bytes();
    let url = format!("{}/r/{}/f/{name}", base_url(), room.id);
    let agent = http();
    let put = agent.put(&url).header("authorization", auth(&room)).send(&sealed_image[..]).expect("put");
    assert_eq!(put.status().as_u16(), 201);
    let list_url = format!("{}/r/{}/f", base_url(), room.id);
    let mut list = agent.get(&list_url).header("authorization", auth(&room)).call().expect("list");
    let listed: serde_json::Value =
        serde_json::from_str(&list.body_mut().read_to_string().expect("body")).expect("json");
    assert_eq!(listed[0]["name"], name.to_string());
    assert_eq!(listed[0]["size"], sealed_image.len());
    let mut get = agent.get(&url).header("authorization", auth(&room)).call().expect("get");
    assert_eq!(get.status().as_u16(), 200);
    let fetched = get.body_mut().read_to_vec().expect("bytes");
    assert_eq!(fetched, sealed_image);
    let opened = room.keys.open_image(&name, &SealedRecord::from_bytes(fetched).expect("sealed")).expect("opens");
    assert_eq!(opened, png);
    let head = agent.head(&url).header("authorization", auth(&room)).call().expect("head");
    assert_eq!(head.status().as_u16(), 200);
    let stranger = new_room();
    let refused = agent.get(&url).header("authorization", auth(&stranger)).call().expect("answer");
    assert_eq!(refused.status().as_u16(), 401);

    // The edge answers the quota probe by itself.
    let mut status = agent.get(format!("{}/api/status", base_url())).call().expect("status");
    let body: serde_json::Value =
        serde_json::from_str(&status.body_mut().read_to_string().expect("body")).expect("json");
    assert_eq!(body["status"], "ok");
}

#[test]
#[ignore = "needs a running `wrangler dev`: see the module docs"]
fn a_stranger_is_refused_at_join_and_an_open_join_of_a_missing_room_is_room_missing() {
    let room = new_room();
    let mut stranger = Peer::connect(&room, 9);
    let other = new_room();
    let message = ClientMessage::Join {
        min_version: 2,
        max_version: 2,
        room: room.keys.room_id(),
        client_id: stranger.id,
        mode: JoinMode::Create,
        proof: Proof::token(other.keys.membership_token()),
        last_seq: 0,
        captcha: None,
    };
    stranger.send(&message);
    let code = stranger.until("Error", |m| if let ServerMessage::Error { code, .. } = m { Some(*code) } else { None });
    assert_eq!(code, ErrorCode::Unauthorized);

    let mut opener = Peer::connect(&room, 8);
    opener.join(&room, JoinMode::Open);
    let code = opener.until("Error", |m| if let ServerMessage::Error { code, .. } = m { Some(*code) } else { None });
    assert_eq!(code, ErrorCode::RoomMissing);
}

/// Needs the Worker to run with a short life for rooms:
/// `scripts/dev.sh --port 8792 -- --var ROOM_TTL_SECS:3 --var ROOM_LEAVE_GRACE_SECS:1`, then
/// `DRAFFTINK_WORKER_URL=http://127.0.0.1:8792 DRAFFTINK_WORKER_SHORT_TTL=1 cargo test ... expires -- --ignored`.
#[test]
#[ignore = "needs a Worker with ROOM_TTL_SECS=3 and ROOM_LEAVE_GRACE_SECS=1: see the doc comment"]
fn a_room_expires_after_its_ttl_when_nobody_is_there() {
    assert_eq!(
        std::env::var("DRAFFTINK_WORKER_SHORT_TTL").as_deref(),
        Ok("1"),
        "set DRAFFTINK_WORKER_SHORT_TTL=1 (see the doc comment)"
    );
    let room = new_room();
    let mut a = Peer::connect(&room, 1);
    a.join(&room, JoinMode::Create);
    let expires = a.until("Joined", |m| {
        if let ServerMessage::Joined { expires_at_ms, .. } = m { Some(*expires_at_ms) } else { None }
    });
    assert!(expires > 0, "the room has a deadline");
    a.send(&ClientMessage::Leave);
    drop(a);

    // The alarm is armed for the deadline. A visit would move it, so wait without one, then look.
    std::thread::sleep(Duration::from_secs(8));
    let mut b = Peer::connect(&room, 2);
    b.join(&room, JoinMode::Open);
    let code = b.until("an answer", |m| if let ServerMessage::Error { code, .. } = m { Some(*code) } else { None });
    assert_eq!(code, ErrorCode::RoomMissing, "the room was deleted at its deadline");
}
