//! Limits, hostile input, Origin, auth, the JSON debug form, `/health` and static files.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code: a failed expectation is the failure

mod common;

use std::time::Duration;

use common::*;
use drafftink_protocol::{AwarenessState, ClientMessage, ErrorCode, ServerMessage, SyncKind, UserInfo};
use drafftink_server::config::Config;
use drafftink_server::start;
use loro::LoroValue;
use tokio_tungstenite::tungstenite::Error as WsError;

fn small_limits() -> Config {
    let mut c = test_config();
    c.limits.max_update_bytes = 64 * 1024;
    c.limits.max_frame_bytes = 128 * 1024;
    c
}

async fn assert_healthy(addr: std::net::SocketAddr) {
    let (status, _, body) = http_get(addr, "/health").await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&body).contains("\"status\":\"ok\""));
    let mut c = Client::connect(addr, 250, "health-check-room").await;
    c.join().await;
    c.edit_acked("k", text("v")).await;
}

#[tokio::test]
async fn first_message_must_be_join() {
    let server = start(test_config()).await.expect("start");
    let mut c = Client::connect(server.local_addr(), 1, "r1").await;
    c.send(&ClientMessage::Ping { nonce: 1 }).await;
    assert_eq!(c.expect_closed().await, [ErrorCode::NotJoined]);
    server.shutdown().await;
}

#[tokio::test]
async fn a_second_join_is_refused() {
    let server = start(test_config()).await.expect("start");
    let mut c = Client::connect(server.local_addr(), 1, "r2").await;
    c.join().await;
    let again = c.join_message(None);
    c.send(&again).await;
    assert_eq!(c.expect_closed().await, [ErrorCode::AlreadyJoined]);
    server.shutdown().await;
}

#[tokio::test]
async fn silence_before_join_times_out() {
    let mut config = test_config();
    config.timing.join_timeout = Duration::from_millis(250);
    let server = start(config).await.expect("start");
    let mut c = Client::connect(server.local_addr(), 1, "r3").await;
    assert_eq!(c.expect_closed().await, [ErrorCode::NotJoined]);
    server.shutdown().await;
}

#[tokio::test]
async fn an_unsupported_protocol_version_is_named_and_closed() {
    let server = start(test_config()).await.expect("start");
    let mut c = Client::connect(server.local_addr(), 1, "r4").await;
    let mut join = c.join_message(None);
    if let ClientMessage::Join { min_version, max_version, .. } = &mut join {
        (*min_version, *max_version) = (5, 9);
    }
    // Encoding refuses nothing about versions; the server must refuse it.
    c.send(&join).await;
    let m = c.next_within(WAIT).await;
    let Event::Msg(ServerMessage::Error { code, message, .. }) = m else { panic!("got {m:?}") };
    assert_eq!(code, ErrorCode::UnsupportedVersion);
    assert!(message.contains('1'), "names the versions the server speaks: {message}");
    c.expect_closed().await;
    server.shutdown().await;
}

#[tokio::test]
async fn garbage_frames_close_the_connection_with_an_error() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    for (name, frame) in [
        ("empty", vec![]),
        ("unknown tag", vec![0x7f, 1, 2, 3]),
        ("server tag sent by a client", vec![0x84, 0, 0, 0, 0]),
        ("truncated join", vec![0x01, 1, 0]),
        ("huge length prefix", vec![0x01, 1, 0, 1, 0, 0xff, 0xff, 0xff, 0xff, 0x0f]),
    ] {
        let mut c = Client::connect(addr, 1, "garbage").await;
        c.send_raw(frame).await;
        let codes = c.expect_closed().await;
        assert!(codes == [ErrorCode::Malformed] || codes == [ErrorCode::TooLarge], "{name}: got {codes:?}");
    }
    assert_healthy(addr).await;
    server.shutdown().await;
}

#[tokio::test]
async fn text_frames_are_refused_unless_debug_json_is_on() {
    let server = start(test_config()).await.expect("start");
    let mut c = Client::connect(server.local_addr(), 1, "r5").await;
    c.send_text(r#"{"type":"ping","nonce":1}"#).await;
    assert_eq!(c.expect_closed().await, [ErrorCode::Malformed]);
    server.shutdown().await;

    let mut config = test_config();
    config.debug_json = true;
    let server = start(config).await.expect("start");
    let mut c = Client::connect(server.local_addr(), 1, "json-room").await;
    c.json = true;
    c.join().await;
    c.edit_acked("k", text("v")).await; // acknowledged in JSON text frames
    c.send(&ClientMessage::Ping { nonce: 5 }).await;
    c.recv_until(|m| matches!(m, ServerMessage::Pong { nonce: 5 }).then_some(())).await;
    server.shutdown().await;
}

#[tokio::test]
async fn oversize_frames_and_updates_are_refused() {
    let server = start(small_limits()).await.expect("start");
    let addr = server.local_addr();
    // Larger than the frame limit: refused by the transport.
    let mut c = Client::connect(addr, 1, "big").await;
    c.join().await;
    c.send_raw(vec![0x02; 300 * 1024]).await;
    let codes = c.expect_closed().await;
    assert!(codes.is_empty() || codes == [ErrorCode::TooLarge], "{codes:?}");
    // Within the frame limit but above the update limit.
    let mut c = Client::connect(addr, 2, "big").await;
    c.join().await;
    c.send(&ClientMessage::Update { seq: 1, update: vec![0u8; 100 * 1024] }).await;
    assert_eq!(c.expect_closed().await, [ErrorCode::TooLarge]);
    assert_healthy(addr).await;
    server.shutdown().await;
}

#[tokio::test]
async fn invalid_update_bytes_are_refused_and_not_relayed_and_the_connection_stays() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "bad-update").await;
    let mut b = Client::connect(addr, 2, "bad-update").await;
    a.join().await;
    b.join().await;
    for junk in [vec![0u8; 50], b"loro but not really".to_vec(), vec![0xff; 4096]] {
        a.send(&ClientMessage::Update { seq: 9, update: junk }).await;
        let m = a
            .recv_until(|m| matches!(m, ServerMessage::Error { .. } | ServerMessage::Ack { .. }).then(|| m.clone()))
            .await;
        assert!(matches!(m, ServerMessage::Error { code: ErrorCode::InvalidUpdate, .. }), "{m:?}");
    }
    a.edit_acked("still", text("works")).await;
    // B only ever saw the one good update.
    b.converge_to(&state(&a.doc)).await;
    server.shutdown().await;
}

#[tokio::test]
async fn an_update_that_raises_the_schema_beyond_the_server_is_refused_rolled_back_and_not_relayed() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "newer-schema").await;
    let mut b = Client::connect(addr, 2, "newer-schema").await;
    a.join().await;
    b.join().await;
    // The schema this server serves is accepted, so a normal document is not affected.
    let ok = a.edit_meta("schema", LoroValue::from(1i64)).await;
    a.recv_until(|m| matches!(m, ServerMessage::Ack { seq } if *seq == ok).then_some(())).await;
    a.edit_acked("fine", text("v")).await;
    b.converge_to(&state(&a.doc)).await;
    let good = state(&b.doc);

    // A newer app (or a hostile client) raises it.
    a.edit_meta("schema", LoroValue::from(99i64)).await;
    let m =
        a.recv_until(|m| matches!(m, ServerMessage::Error { .. } | ServerMessage::Ack { .. }).then(|| m.clone())).await;
    let ServerMessage::Error { code, message, .. } = m else { panic!("expected an error, got {m:?}") };
    assert_eq!(code, ErrorCode::InvalidUpdate);
    assert!(message.contains("schema 99") && message.contains("Update the app"), "{message}");

    // Nobody else saw it, a client that joins later gets the room as it was, and the room keeps working.
    let mut c = Client::connect(addr, 3, "newer-schema").await;
    c.join().await;
    assert_eq!(state(&c.doc), good, "the room was rolled back");
    c.edit_acked("later", text("w")).await;
    let want = state(&c.doc);
    // B's document equals C's, so the refused update never reached it.
    b.converge_to(&want).await;
    server.shutdown().await;
}

#[tokio::test]
async fn update_flood_is_rate_limited_then_the_connection_is_closed() {
    let mut config = test_config();
    config.limits.max_updates_per_sec = 5;
    config.limits.max_breaches = 8;
    let server = start(config).await.expect("start");
    let mut c = Client::connect(server.local_addr(), 1, "flood").await;
    c.join().await;
    for i in 0..60 {
        c.edit(&format!("k{i}"), text("v")).await;
    }
    let mut limited = 0;
    let mut acks = 0;
    let mut retry = None;
    loop {
        match c.next_within(WAIT).await {
            Event::Msg(ServerMessage::Ack { .. }) => acks += 1,
            Event::Msg(ServerMessage::Error { code: ErrorCode::RateLimited, retry_after_ms, .. }) => {
                limited += 1;
                retry = retry.or(retry_after_ms);
            }
            Event::Msg(_) => {}
            Event::Closed => break,
            Event::Timeout => panic!("not closed"),
        }
    }
    assert!((1..=12).contains(&acks), "the burst of 2x the rate got through: {acks}");
    assert!(limited >= 8, "{limited}");
    assert!(retry.is_some_and(|ms| ms > 0 && ms <= 200), "retry_after_ms tells when to try again: {retry:?}");
    server.shutdown().await;
}

#[tokio::test]
async fn a_client_that_backs_off_after_rate_limited_keeps_its_connection() {
    let mut config = test_config();
    config.limits.max_updates_per_sec = 20;
    config.limits.max_breaches = 30;
    let server = start(config).await.expect("start");
    let mut c = Client::connect(server.local_addr(), 1, "polite").await;
    c.join().await;
    for i in 0..60 {
        c.edit(&format!("k{i}"), text("v")).await;
    }
    let mut limited = 0;
    while let Event::Msg(m) = c.next_within(Duration::from_millis(300)).await {
        if matches!(m, ServerMessage::Error { code: ErrorCode::RateLimited, .. }) {
            limited += 1;
        }
    }
    assert!(limited > 0 && limited < 30, "a burst of 60 is above 2x20 per second: {limited}");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    c.edit_acked("after-backoff", text("v")).await;
    server.shutdown().await;
}

#[tokio::test]
async fn awareness_over_the_rate_is_dropped_quietly() {
    let mut config = test_config();
    config.limits.max_awareness_per_sec = 5;
    let server = start(config).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "aw-rate").await;
    let mut b = Client::connect(addr, 2, "aw-rate").await;
    a.join().await;
    b.join().await;
    for i in 0..100 {
        let s = AwarenessState { user: Some(UserInfo { name: format!("n{i}"), color: 0 }), ..Default::default() };
        a.send(&ClientMessage::Awareness(s)).await;
    }
    let mut got = 0;
    while let Event::Msg(m) = b.next_within(Duration::from_millis(300)).await {
        if matches!(m, ServerMessage::PeerAwareness { .. }) {
            got += 1;
        }
    }
    assert!((5..=15).contains(&got), "burst only: {got}");
    // A is still connected and no error was sent to it.
    assert!(!matches!(a.next_within(Duration::from_millis(100)).await, Event::Msg(ServerMessage::Error { .. })));
    server.shutdown().await;
}

#[tokio::test]
async fn peers_per_room_rooms_and_connections_are_limited() {
    let mut config = test_config();
    config.limits.max_peers_per_room = 2;
    config.limits.max_rooms = 2;
    config.limits.max_connections = 5;
    let server = start(config).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "full").await;
    let mut b = Client::connect(addr, 2, "full").await;
    a.join().await;
    b.join().await;
    let mut c = Client::connect(addr, 3, "full").await;
    let j = c.join_message(None);
    c.send(&j).await;
    assert_eq!(c.expect_closed().await, [ErrorCode::RoomFull]);
    // A reconnect of an existing id is a replacement, so it is allowed even when full.
    let mut a2 = Client::connect(addr, 1, "full").await;
    a2.join().await;

    let mut d = Client::connect(addr, 4, "second-room").await;
    d.join().await;
    let mut e = Client::connect(addr, 5, "third-room").await;
    let j = e.join_message(None);
    e.send(&j).await;
    let m = e.next_within(WAIT).await;
    assert!(
        matches!(&m, Event::Msg(ServerMessage::Error { code: ErrorCode::Unavailable, retry_after_ms: Some(_), .. })),
        "{m:?}"
    );

    // Connections: 5 allowed. a, a2 (a is closing), b, c(closed), d, e(closed) ...
    drop((a, c, e));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut held = vec![b, a2, d];
    let mut refused = 0;
    for n in 10..16u8 {
        match Client::try_connect(addr, None, n, "conn-limit").await {
            Ok(c) => held.push(c),
            Err(WsError::Http(r)) if r.status() == 503 => refused += 1,
            Err(e) => panic!("unexpected {e:?}"),
        }
    }
    assert!(refused >= 1, "the connection limit refuses with HTTP 503");
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn random_frames_never_hurt_the_server() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    // Valid join frame, then mutations of it, then random bytes.
    let valid = {
        let c = Client::connect(addr, 1, "fuzz").await;
        c.join_message(None).encode().expect("encode")
    };
    for i in 0..300 {
        let mut c = Client::connect(addr, 1, "fuzz").await;
        let frame = match i % 3 {
            0 => {
                let mut f = valid.clone();
                let at = (rng.next() as usize) % f.len();
                f[at] ^= 1 << (rng.next() % 8);
                f
            }
            1 => valid[..(rng.next() as usize) % valid.len()].to_vec(),
            _ => {
                let n = (rng.next() % 200) as usize;
                rng.bytes(n)
            }
        };
        c.send_raw(frame).await;
        // Either it was still a valid join (a flipped payload bit) or the server closes.
        let _ = c.next_within(Duration::from_millis(500)).await;
    }
    assert_healthy(addr).await;
    server.shutdown().await;
}

#[tokio::test]
async fn hostile_frames_after_join_do_not_corrupt_the_room() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut good = Client::connect(addr, 1, "hostile-after").await;
    good.join().await;
    good.edit_acked("k", text("v")).await;
    let mut rng = Rng(42);
    for _ in 0..50 {
        let mut bad = Client::connect(addr, 9, "hostile-after").await;
        bad.join().await;
        let n = (rng.next() % 300) as usize;
        bad.send_raw(rng.bytes(n + 1)).await;
        let _ = bad.next_within(Duration::from_millis(200)).await;
    }
    let mut fresh = Client::connect(addr, 2, "hostile-after").await;
    fresh.join().await;
    assert_eq!(state(&fresh.doc), state(&good.doc));
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_reader_is_told_to_resync_and_recovers() {
    let mut config = test_config();
    config.limits.max_updates_per_sec = 10_000;
    config.limits.outbox_messages = 8;
    config.limits.outbox_bytes = 512 * 1024;
    config.timing.flush_interval = Duration::from_secs(60);
    let server = start(config).await.expect("start");
    let addr = server.local_addr();
    let mut writer = Client::connect(addr, 1, "slow").await;
    let mut slow = Client::connect_small_buffer(addr, 2, "slow").await;
    writer.join().await;
    slow.join().await;
    let mut rng = Rng(7);
    // `slow` reads nothing while 8 MiB of incompressible updates flow past it.
    for i in 0..80 {
        let blob = rng.bytes(100 * 1024);
        writer.edit(&format!("blob-{i}"), LoroValue::from(blob)).await;
        if i % 8 == 7 {
            // Keep the writer's own socket drained.
            while let Event::Msg(_) = writer.next_within(Duration::from_millis(1)).await {}
        }
    }
    while let Event::Msg(_) = writer.next_within(Duration::from_millis(200)).await {}
    // Now the slow reader reads everything that is left. It must see ResyncRequired.
    let mut resync_seen = false;
    loop {
        match slow.next_within(Duration::from_millis(500)).await {
            Event::Msg(ServerMessage::Error { code: ErrorCode::ResyncRequired, .. }) => resync_seen = true,
            Event::Msg(ServerMessage::Update { update, .. }) => {
                slow.doc.import(&update).expect("import");
            }
            Event::Msg(_) => {}
            Event::Closed => panic!("a lagging client stays connected"),
            Event::Timeout => break,
        }
    }
    assert!(resync_seen, "the server must say so instead of dropping silently");
    slow.send(&ClientMessage::RequestSync { version: slow.doc.oplog_vv().encode() }).await;
    let sync = slow.recv_until(|m| matches!(m, ServerMessage::SyncResponse { .. }).then(|| m.clone())).await;
    slow.apply_sync(&sync);
    assert_eq!(state(&slow.doc), state(&writer.doc), "after the resync the documents are equal");
    server.shutdown().await;
}

// ---- origin and auth ---------------------------------------------------------------------------

#[tokio::test]
async fn origin_check_on_the_websocket_upgrade() {
    let mut config = test_config();
    config.allowed_origins = vec!["https://draw.example".into()];
    let server = start(config).await.expect("start");
    let addr = server.local_addr();
    let refused = |r: Result<Client, WsError>| match r {
        Err(WsError::Http(resp)) => assert_eq!(resp.status(), 403),
        other => panic!("expected HTTP 403, got {:?}", other.map(|_| "a connection")),
    };
    refused(Client::try_connect(addr, Some("https://evil.example"), 1, "o").await);
    refused(Client::try_connect(addr, Some("null"), 1, "o").await);
    refused(Client::try_connect(addr, Some("http://draw.example"), 1, "o").await);
    // Allowed: listed origin, own origin on loopback, and no Origin at all (native clients).
    for origin in [Some("https://draw.example"), Some(&format!("http://{addr}") as &str), None] {
        let mut c = Client::try_connect(addr, origin, 1, "o").await.expect("allowed origin");
        c.join().await;
    }
    // A DNS-rebinding style request: Host and Origin agree but are not loopback names.
    let mut req =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!("ws://{addr}/ws"))
            .expect("request");
    req.headers_mut().insert("Host", "evil.example:80".parse().expect("value"));
    req.headers_mut().insert("Origin", "http://evil.example:80".parse().expect("value"));
    assert!(matches!(tokio_tungstenite::connect_async(req).await, Err(WsError::Http(r)) if r.status() == 403));
    server.shutdown().await;
}

#[tokio::test]
async fn star_origin_allows_any_page() {
    let mut config = test_config();
    config.allowed_origins = vec!["*".into()];
    let server = start(config).await.expect("start");
    let mut c =
        Client::try_connect(server.local_addr(), Some("https://anywhere.example"), 1, "o").await.expect("allowed");
    c.join().await;
    server.shutdown().await;
}

#[tokio::test]
async fn auth_is_off_by_default_and_ignores_tokens() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "noauth").await;
    a.join_with(None).await;
    let mut b = Client::connect(addr, 2, "noauth").await;
    b.join_with(Some("a token nobody asked for")).await;
    server.shutdown().await;
}

#[tokio::test]
async fn auth_on_requires_a_listed_token() {
    let mut config = test_config();
    config.auth_tokens = vec!["secret-one".into(), "secret-two".into()];
    let server = start(config).await.expect("start");
    let addr = server.local_addr();
    for token in [None, Some(""), Some("secret"), Some("secret-one "), Some("SECRET-ONE")] {
        let mut c = Client::connect(addr, 1, "authed").await;
        let j = c.join_message(token);
        c.send(&j).await;
        assert_eq!(c.expect_closed().await, [ErrorCode::Unauthorized], "token {token:?}");
    }
    for token in ["secret-one", "secret-two"] {
        let mut c = Client::connect(addr, 2, "authed").await;
        c.join_with(Some(token)).await;
        c.edit_acked("k", text("v")).await;
    }
    // Everything else before a valid Join is refused too.
    let mut c = Client::connect(addr, 3, "authed").await;
    c.send(&ClientMessage::Update { seq: 1, update: vec![1, 2, 3] }).await;
    assert_eq!(c.expect_closed().await, [ErrorCode::NotJoined]);
    server.shutdown().await;
}

#[tokio::test]
async fn sync_kind_is_snapshot_for_new_clients_in_an_authed_room() {
    let mut config = test_config();
    config.auth_tokens = vec!["t".into()];
    let server = start(config).await.expect("start");
    let mut c = Client::connect(server.local_addr(), 1, "authed2").await;
    let (_, sync) = c.join_with(Some("t")).await;
    assert_eq!(sync_kind(&sync), SyncKind::Snapshot);
    server.shutdown().await;
}

// ---- http ---------------------------------------------------------------------------------------

#[tokio::test]
async fn health_reports_status_rooms_and_connections() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut c = Client::connect(addr, 1, "health-room").await;
    c.join().await;
    let (status, headers, body) = http_get(addr, "/health").await;
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "content-type"), Some("application/json"));
    let text = String::from_utf8(body).expect("utf8");
    assert!(text.contains("\"rooms\":1") && text.contains("\"connections\":1"), "{text}");
    assert!(!text.contains("health-room"), "room ids are capabilities and must not be listed");
    server.shutdown().await;
}

#[tokio::test]
async fn static_files_are_served_from_the_web_dir_without_escaping_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let web = dir.path().join("web-dist");
    std::fs::create_dir_all(web.join("assets")).expect("dirs");
    std::fs::write(web.join("index.html"), "<!doctype html><title>drafft</title>").expect("write");
    std::fs::write(web.join("assets").join("app.wasm"), [0, 0x61, 0x73, 0x6d]).expect("write");
    std::fs::write(dir.path().join("secret.txt"), "top secret").expect("write");
    let mut config = test_config();
    config.web_dir = web;
    let server = start(config).await.expect("start");
    let addr = server.local_addr();

    let (status, headers, body) = http_get(addr, "/").await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&body).contains("<title>drafft</title>"));
    assert_eq!(header(&headers, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header(&headers, "referrer-policy"), Some("no-referrer"));

    let (status, headers, body) = http_get(addr, "/assets/app.wasm").await;
    assert_eq!((status, body), (200, vec![0, 0x61, 0x73, 0x6d]));
    assert_eq!(header(&headers, "content-type"), Some("application/wasm"));

    for path in
        ["/../secret.txt", "/%2e%2e/secret.txt", "/assets/../../secret.txt", "/..%2fsecret.txt", "/%2e%2e%2fsecret.txt"]
    {
        let (status, _, body) = http_get(addr, path).await;
        assert!(!String::from_utf8_lossy(&body).contains("top secret"), "{path} leaked a file (status {status})");
    }
    // /ws is not shadowed by the static files, and an unknown path is not a server error.
    let (status, _, _) = http_get(addr, "/no/such/page").await;
    assert!(status == 404 || status == 200, "{status}");
    server.shutdown().await;
}

#[tokio::test]
async fn a_missing_web_dir_does_not_stop_the_server() {
    let server = start(test_config()).await.expect("start");
    let (status, _, _) = http_get(server.local_addr(), "/").await;
    assert_eq!(status, 404);
    let (status, _, _) = http_get(server.local_addr(), "/health").await;
    assert_eq!(status, 200);
    server.shutdown().await;
}
