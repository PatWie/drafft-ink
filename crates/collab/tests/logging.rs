#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The access token never reaches a log line, at any level the client itself logs at.

mod common;

use std::sync::Mutex;

use common::live::*;
use common::*;
use drafftink_collab::*;
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
    w.wait_until("online", 20, |w| w.peers[a].state() == ConnState::Online);
    w.peers[a].add("x");
    w.converged(20);
    let wrong = w.spawn(PeerSpec { token: Some("another-secret-ABC"), ..PeerSpec::new(2, &url, "log") });
    w.wait_until("stopped", 20, |w| w.peers[wrong].state() == ConnState::Stopped);

    let log = captured();
    assert!(!log.is_empty(), "the capture works");
    for secret in [TOKEN, "another-secret-ABC"] {
        assert!(!log.contains(secret), "a token appeared in the log:\n{log}");
    }
}
