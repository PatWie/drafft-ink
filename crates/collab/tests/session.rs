#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `CollabSession` joining a client to a transport, over the in-memory loopback.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use drafftink_collab::*;
use drafftink_protocol::{ClientMessage, ServerMessage};

fn session() -> (CollabSession<LoopbackTransport>, LoopbackServer, Arc<AtomicUsize>) {
    let (transport, server) = loopback();
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = Arc::clone(&wakes);
    let transport = transport.with_wake(Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    }));
    server.set_auto_accept(true);
    let client = CollabClient::new(config("ws://127.0.0.1:3030")).unwrap();
    (CollabSession::new(client, transport), server, wakes)
}

#[test]
fn pump_connects_joins_syncs_and_reports_what_happened() {
    let (mut s, server, wakes) = session();
    let mut store = store(1);
    let mut doc_server = store_peer(100);
    add(&mut doc_server, &["from-server"]);
    s.client_mut().start(0);
    let p = s.pump(0, &mut store);
    assert_eq!(server.opens(), 1);
    assert_eq!(server.last_url().as_deref(), Some("ws://127.0.0.1:3030/ws"));
    assert!(p.redraw && !p.events.is_empty());
    // The loopback accepted at once and queued Opened; the pump loop delivered it and sent the Join.
    let frames = server.recv_all();
    assert!(matches!(&frames[..], [ClientMessage::Join { .. }]), "{frames:?}");
    server.push(&ServerMessage::Joined {
        protocol_version: 1,
        server: "t".into(),
        room: room(),
        client_id: cid(1),
        peers: vec![],
        server_version: doc_server.version_vector().unwrap(),
        limits: Default::default(),
    });
    server.push(&ServerMessage::SyncResponse {
        kind: drafftink_protocol::SyncKind::Updates,
        data: doc_server.export_updates_since(&store.version_vector().unwrap()).unwrap(),
        server_version: doc_server.version_vector().unwrap(),
    });
    assert_eq!(wakes.load(Ordering::SeqCst), 2, "the transport woke its owner for each event");
    let p = s.pump(10, &mut store);
    assert_eq!(s.client().status().state, ConnState::Online);
    assert_eq!(ids(&store), vec!["from-server".to_owned()]);
    let batches = Harness::remote_batches(&p.events);
    assert_eq!(batches.len(), 1);
    // The upload of the document's own schema operations went out in the same pump.
    assert!(server.recv_all().iter().any(|m| matches!(m, ClientMessage::Update { .. })));
    assert!(p.next_wake_ms.is_some_and(|w| w > 10));
}

fn store_peer(n: u64) -> drafftink_doc::LoroStore {
    common::store(n)
}

#[test]
fn an_idle_pump_asks_for_no_redraw_and_no_early_wake() {
    let (mut s, server, _) = session();
    let mut store = store(1);
    s.client_mut().start(0);
    s.pump(0, &mut store);
    server.recv_all();
    let doc_server = store_peer(100);
    server.push(&ServerMessage::Joined {
        protocol_version: 1,
        server: "t".into(),
        room: room(),
        client_id: cid(1),
        peers: vec![],
        server_version: doc_server.version_vector().unwrap(),
        limits: Default::default(),
    });
    server.push(&ServerMessage::SyncResponse {
        kind: drafftink_protocol::SyncKind::Updates,
        data: vec![],
        server_version: doc_server.version_vector().unwrap(),
    });
    s.pump(5, &mut store);
    let acks: Vec<_> = server
        .recv_all()
        .iter()
        .filter_map(|m| if let ClientMessage::Update { seq, .. } = m { Some(*seq) } else { None })
        .collect();
    for seq in acks {
        server.push(&ServerMessage::Ack { seq });
    }
    s.pump(6, &mut store);
    let p = s.pump(7, &mut store);
    assert!(!p.redraw && p.events.is_empty(), "{:?}", p.events);
    assert!(p.next_wake_ms.unwrap() > 7);
    // Pumping again and again changes nothing.
    for t in 8..50 {
        let p = s.pump(t, &mut store);
        assert!(!p.redraw);
    }
}

#[test]
fn a_connection_that_fails_to_open_becomes_a_reconnect() {
    let (mut s, server, _) = session();
    server.set_auto_accept(false);
    let mut store = store(1);
    s.client_mut().start(0);
    s.pump(0, &mut store);
    assert_eq!(s.client().status().state, ConnState::Connecting);
    server.drop_connection("connection refused");
    let p = s.pump(1, &mut store);
    assert_eq!(s.client().status().state, ConnState::Reconnecting);
    assert!(p.redraw);
    let at = s.client().status().retry_at_ms.unwrap();
    assert_eq!(p.next_wake_ms, Some(at), "the only timer is the retry");
    s.pump(at, &mut store);
    assert_eq!(server.opens(), 2);
}

struct Broken;

impl Transport for Broken {
    fn open(&mut self, _: &ServerUrl) -> Result<(), TransportError> {
        Err(TransportError::Failed("no network".into()))
    }
    fn send(&mut self, _: Vec<u8>) -> Result<(), TransportError> {
        Err(TransportError::NotOpen)
    }
    fn close(&mut self) {}
    fn poll(&mut self) -> Option<TransportEvent> {
        None
    }
}

#[test]
fn a_transport_that_cannot_open_is_retried_with_backoff_not_in_a_loop() {
    let client = CollabClient::new(config("ws://127.0.0.1:3030")).unwrap();
    let mut s = CollabSession::new(client, Broken);
    let mut store = store(1);
    s.client_mut().start(0);
    let p = s.pump(0, &mut store);
    assert_eq!(s.client().status().state, ConnState::Reconnecting);
    assert!(p.next_wake_ms.unwrap() > 0);
    // Many pumps at the same instant do not try again.
    for _ in 0..50 {
        s.pump(1, &mut store);
    }
    assert_eq!(s.client().stats().connects, 1);
}

#[test]
fn leaving_through_the_session_sends_leave_and_closes_the_transport() {
    let (mut s, server, _) = session();
    let mut store = store(1);
    s.client_mut().start(0);
    s.pump(0, &mut store);
    server.recv_all();
    let doc_server = store_peer(100);
    server.push(&ServerMessage::Joined {
        protocol_version: 1,
        server: "t".into(),
        room: room(),
        client_id: cid(1),
        peers: vec![],
        server_version: doc_server.version_vector().unwrap(),
        limits: Default::default(),
    });
    server.push(&ServerMessage::SyncResponse {
        kind: drafftink_protocol::SyncKind::Updates,
        data: vec![],
        server_version: doc_server.version_vector().unwrap(),
    });
    s.pump(1, &mut store);
    server.recv_all();
    s.client_mut().leave(2, &mut store);
    s.pump(2, &mut store);
    assert!(server.recv_all().contains(&ClientMessage::Leave));
    assert!(!server.is_open());
    assert!(server.closes() >= 1);
}
