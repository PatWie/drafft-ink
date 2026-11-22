#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The mock servers use real sockets and the real clock.
#![allow(clippy::disallowed_methods)]
//! The access token and the service token of a private server never reach a log line, at any level.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::live::*;
use common::mock::*;
use common::*;
use drafftink_collab::*;
use drafftink_protocol::v2::MembershipToken;
use drafftink_protocol::{ErrorCode, ServerMessage};

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, r: &log::Record<'_>) {
        LOG.lock().unwrap().push(format!("{} {}: {}", r.level(), r.target(), r.args()));
    }
    fn flush(&self) {}
}

fn captured() -> String {
    LOG.lock().unwrap().join("\n")
}

const TOKEN: &str = "tok-9f3a-NEVER-LOG-ME";
/// The service token of a private server.
const ACCESS_ID: &str = "svc-id-7c1e.access";
const ACCESS_SECRET: &str = "svc-secret-4b8d-NEVER-LOG-ME";

fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < end, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs the service token through the real transports against mock servers: an accepted handshake, a refused one, a
/// redirect to a login page, and picture requests that are served and turned away.
fn use_the_service_token() {
    let grant = |origin: &str| {
        AccessGrant::new(
            AccessOrigin::parse(origin).unwrap(),
            AccessCredentials::new(ACCESS_ID, ACCESS_SECRET).unwrap(),
        )
    };
    for answer in [
        Handshake::Accept,
        refuse(401, None, r#"{"error":"access_invalid"}"#),
        refuse(302, Some("http://127.0.0.1:9/login"), ""),
    ] {
        let server = MockSocket::start(answer);
        let mut transport = NativeTransport::new(Arc::new(|| {}));
        transport.set_access(Some(grant(&server.origin())));
        let mut session = CollabSession::new(CollabClient::new(config(&server.url().to_string())).unwrap(), transport);
        session.client_mut().start(0);
        let mut store = store(1);
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(400) {
            let now = u64::try_from(started.elapsed().as_millis()).unwrap();
            session.pump(now, &mut store);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!seen(&server.log).is_empty(), "the handshake reached the mock");
        assert!(format!("{:?}", session.client()).contains("hidden"));
    }
    for (status, location) in [(200, None), (401, None), (302, Some("http://127.0.0.1:9/login".to_owned()))] {
        let server = MockHttp::start(status, location, r#"{"error":"access_required"}"#);
        let mut blobs = NativeBlobTransport::new(Arc::new(|| {}));
        blobs.set_access(Some(grant(&server.origin())));
        let auth = BlobAuth::member(&MembershipToken::from_bytes([5; 32]));
        let url = server.url("/r/abc/f");
        blobs.send(BlobRequest { id: 1, url, op: BlobOp::List, auth }).unwrap();
        let answer = wait_for(|| blobs.poll());
        assert!(!format!("{answer:?}").contains(ACCESS_SECRET));
    }
    let credentials = AccessCredentials::new(ACCESS_ID, ACCESS_SECRET).unwrap();
    let shown = format!(
        "{credentials:?} {:?} {:?}",
        grant("https://canvas.example.com"),
        AccessError::TooLong(AccessField::ClientSecret)
    );
    assert!(shown.contains("AccessCredentials(<hidden>)"));
    log::debug!("{shown}");
}

#[test]
fn the_token_is_not_logged_by_the_client_or_the_transport() {
    log::set_logger(&Capture).unwrap();
    log::set_max_level(log::LevelFilter::Debug);

    // Through the state machine: join, a bad frame, errors, reconnects, a stop.
    let mut cfg = config("ws://127.0.0.1:3030");
    cfg.auth_token = Some(AuthToken::new(TOKEN).unwrap());
    let mut h = Harness::with(cfg);
    h.start();
    h.event(TransportEvent::Opened);
    h.event(TransportEvent::Message(vec![0xff, 0xfe]));
    h.advance(60_000);
    h.event(TransportEvent::Opened);
    h.feed(&ServerMessage::Error { code: ErrorCode::Unauthorized, message: "bad token".into(), retry_after_ms: None });
    assert!(format!("{:?}", h.client).contains("hidden"));

    // Through the real transport and server, with auth on.
    let server = Server::start(&["--auth-token", TOKEN]);
    let url = server.url();
    let mut w = World::new();
    let a = w.spawn(PeerSpec { token: Some(TOKEN), ..PeerSpec::new(1, &url, "log") });
    w.wait_until("online", |w| w.peers[a].state() == ConnState::Online);
    w.peers[a].add("x");
    w.converged();
    let wrong = w.spawn(PeerSpec { token: Some("another-secret-ABC"), ..PeerSpec::new(2, &url, "log") });
    w.wait_until("stopped", |w| w.peers[wrong].state() == ConnState::Stopped);

    use_the_service_token();

    let log = captured();
    assert!(!log.is_empty(), "the capture works");
    for secret in [TOKEN, "another-secret-ABC", ACCESS_ID, ACCESS_SECRET] {
        assert!(!log.contains(secret), "a token appeared in the log:\n{log}");
    }
}
