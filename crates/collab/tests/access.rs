#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// The mock servers use real sockets and the real clock.
#![allow(clippy::disallowed_methods)]
//! The service token of a private server on the wire: the WebSocket handshake and the picture requests, against local
//! mock servers that record what they receive. The token goes only to the origin it was stored for, only over TLS or to
//! this machine, and a redirect is never followed.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use common::mock::*;
use drafftink_collab::*;
use drafftink_protocol::v2::MembershipToken;

const ID: &str = "abc123.access";
const SECRET: &str = "NEVER-SEND-ELSEWHERE-0123456789";

fn grant_for(origin: &str) -> AccessGrant {
    AccessGrant::new(AccessOrigin::parse(origin).unwrap(), AccessCredentials::new(ID, SECRET).unwrap())
}

fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Opens `url` with `access` and returns the first event.
fn connect(url: &ServerUrl, access: Option<AccessGrant>) -> TransportEvent {
    let mut transport = NativeTransport::new(Arc::new(|| {}));
    transport.set_access(access);
    transport.open(url).unwrap();
    wait_for("a transport event", || transport.poll())
}

#[test]
fn the_handshake_carries_the_token_to_the_origin_it_was_stored_for() {
    let server = MockSocket::start(Handshake::Accept);
    let event = connect(&server.url(), Some(grant_for(&server.origin())));
    assert_eq!(event, TransportEvent::Opened);
    let request = seen(&server.log).remove(0);
    assert_eq!(request.headers.get("cf-access-client-id").map(String::as_str), Some(ID));
    assert_eq!(request.headers.get("cf-access-client-secret").map(String::as_str), Some(SECRET));
}

#[test]
fn the_handshake_has_no_token_without_one() {
    let server = MockSocket::start(Handshake::Accept);
    assert_eq!(connect(&server.url(), None), TransportEvent::Opened);
    assert!(!seen(&server.log)[0].carries_token());
}

#[test]
fn the_handshake_has_no_token_for_another_origin() {
    let server = MockSocket::start(Handshake::Accept);
    let other = MockSocket::start(Handshake::Accept);
    // Stored for the other server's port, and for the same machine under another name.
    for stored in [other.origin(), format!("http://localhost:{}", server.addr.port())] {
        assert_eq!(connect(&server.url(), Some(grant_for(&stored))), TransportEvent::Opened);
    }
    assert!(seen(&server.log).iter().all(|r| !r.carries_token()), "{:?}", seen(&server.log));
    assert!(seen(&other.log).is_empty());
}

#[test]
fn a_gate_that_answers_401_is_a_denial_and_the_words_depend_on_the_token() {
    let required = r#"{"error":"access_required","message":"sign in"}"#;
    let invalid = r#"{"error":"access_invalid","message":"no"}"#;
    for (body, with_token, expected) in [
        (required, false, AccessDenial::Required),
        (required, true, AccessDenial::Rejected),
        (invalid, false, AccessDenial::Rejected),
        (invalid, true, AccessDenial::Rejected),
        ("", false, AccessDenial::Required),
        ("", true, AccessDenial::Rejected),
    ] {
        let server = MockSocket::start(refuse(401, None, body));
        let grant = with_token.then(|| grant_for(&server.origin()));
        assert_eq!(connect(&server.url(), grant), TransportEvent::Denied(expected), "{body:?} token={with_token}");
    }
}

#[test]
fn a_redirect_to_the_access_login_is_a_denial_and_is_not_followed() {
    let login = MockSocket::start(Handshake::Accept);
    let target = format!("http://{}/cdn-cgi/access/login?redirect_url=x", login.addr);
    let gate = MockSocket::start(refuse(302, Some(&target), ""));
    let event = connect(&gate.url(), Some(grant_for(&gate.origin())));
    assert_eq!(event, TransportEvent::Denied(AccessDenial::Rejected));
    assert_eq!(seen(&gate.log).len(), 1, "the handshake was sent once");
    thread::sleep(Duration::from_millis(200));
    assert!(seen(&login.log).is_empty(), "nothing went to the place the redirect named");
    // Without a token the same answer says the server is private.
    let event = connect(&gate.url(), None);
    assert_eq!(event, TransportEvent::Denied(AccessDenial::Required));
    assert!(seen(&login.log).is_empty());
}

#[test]
fn another_refusal_is_a_plain_failure_and_not_a_denial() {
    for (status, body) in [(403, ""), (404, ""), (500, ""), (401, r#"{"error":"unauthorized"}"#)] {
        let server = MockSocket::start(refuse(status, None, body));
        match connect(&server.url(), Some(grant_for(&server.origin()))) {
            TransportEvent::Closed { error: Some(text) } => {
                assert!(!text.contains(SECRET) && !text.contains(ID), "{text}");
            }
            other => panic!("status {status}: {other:?}"),
        }
    }
}

// ---- the picture requests -------------------------------------------------------------------------------------

fn request(id: u64, url: String, op: BlobOp) -> BlobRequest {
    BlobRequest { id, url, op, auth: BlobAuth::member(&MembershipToken::from_bytes([3; 32])) }
}

fn ask(access: Option<AccessGrant>, url: String, op: BlobOp) -> BlobReply {
    let mut transport = NativeBlobTransport::with_options(Arc::new(|| {}), NativeBlobOptions::default());
    transport.set_access(access);
    transport.send(request(1, url, op)).unwrap();
    wait_for("an answer", || transport.poll()).result.unwrap()
}

#[test]
fn every_kind_of_picture_request_carries_the_token_to_its_origin() {
    let server = MockHttp::start(200, None, "[]");
    for op in [BlobOp::List, BlobOp::Get, BlobOp::Head, BlobOp::Put(vec![1, 2, 3])] {
        let reply = ask(Some(grant_for(&server.origin())), server.url("/r/abc/f/name"), op);
        assert_eq!(reply.status, 200);
        assert_eq!(reply.denial, None);
    }
    let log = seen(&server.log);
    assert_eq!(log.len(), 4);
    for r in &log {
        assert_eq!(r.headers.get("cf-access-client-id").map(String::as_str), Some(ID), "{r:?}");
        assert_eq!(r.headers.get("cf-access-client-secret").map(String::as_str), Some(SECRET), "{r:?}");
        assert!(r.headers.get("authorization").is_some_and(|a| a.starts_with("Drafft-Member ")), "{r:?}");
    }
    assert_eq!(
        log.iter().map(|r| r.first_line.split(' ').next().unwrap()).collect::<Vec<_>>(),
        ["GET", "GET", "HEAD", "PUT"]
    );
}

#[test]
fn picture_requests_have_no_token_without_one_or_for_another_origin() {
    let server = MockHttp::start(200, None, "[]");
    let other = MockHttp::start(200, None, "[]");
    ask(None, server.url("/r/abc/f"), BlobOp::List);
    ask(Some(grant_for(&other.origin())), server.url("/r/abc/f"), BlobOp::List);
    ask(Some(grant_for(&format!("http://localhost:{}", server.addr.port()))), server.url("/r/abc/f"), BlobOp::List);
    assert!(seen(&server.log).iter().all(|r| !r.carries_token()), "{:?}", seen(&server.log));
    assert!(seen(&other.log).is_empty());
}

#[test]
fn a_redirect_to_the_access_login_is_not_followed_and_is_marked_as_a_denial() {
    let login = MockHttp::start(200, None, "login page");
    let target = login.url("/cdn-cgi/access/login");
    let gate = MockHttp::start(302, Some(target), "");
    for op in [BlobOp::List, BlobOp::Get, BlobOp::Head, BlobOp::Put(vec![9; 4])] {
        let reply = ask(Some(grant_for(&gate.origin())), gate.url("/r/abc/f/n"), op);
        assert_eq!((reply.status, reply.denial), (302, Some(AccessDenial::Rejected)));
    }
    let reply = ask(None, gate.url("/r/abc/f/n"), BlobOp::Get);
    assert_eq!((reply.status, reply.denial), (302, Some(AccessDenial::Required)));
    thread::sleep(Duration::from_millis(200));
    assert_eq!(login.connections.load(Ordering::SeqCst), 0, "the redirect was followed");
    assert!(seen(&login.log).is_empty());
}

#[test]
fn the_workers_401_json_decides_the_words() {
    for (body, with_token, expected) in [
        (r#"{"error":"access_required","message":"m"}"#, false, Some(AccessDenial::Required)),
        (r#"{"error":"access_required","message":"m"}"#, true, Some(AccessDenial::Rejected)),
        (r#"{"error":"access_invalid","message":"m"}"#, false, Some(AccessDenial::Rejected)),
        (r#"{"error":"access_invalid","message":"m"}"#, true, Some(AccessDenial::Rejected)),
        // The relay's own 401: this room's membership token was not accepted. Not the gate.
        (r#"{"error":"unauthorized","message":"m"}"#, true, None),
    ] {
        let server = MockHttp::start(401, None, body);
        let grant = with_token.then(|| grant_for(&server.origin()));
        let reply = ask(grant, server.url("/r/abc/f"), BlobOp::List);
        assert_eq!((reply.status, reply.denial), (401, expected), "{body} token={with_token}");
    }
}

#[test]
fn plain_http_to_another_machine_is_still_refused_with_a_token() {
    let mut transport = NativeBlobTransport::new(Arc::new(|| {}));
    transport.set_access(Some(grant_for("http://203.0.113.9:8080")));
    let refused = transport.send(request(1, "http://203.0.113.9:8080/r/abc/f".to_owned(), BlobOp::List));
    assert!(matches!(refused, Err(BlobTransportError::Refused(_))), "{refused:?}");
}
