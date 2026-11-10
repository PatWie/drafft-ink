//! Protocol behaviour end to end: join, catch-up, live edits, reconnect, awareness, shutdown.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code: a failed expectation is the failure

mod common;

use std::time::Duration;

use common::*;
use drafftink_protocol::{AwarenessState, ClientMessage, ErrorCode, Point, ServerMessage, SyncKind};
use drafftink_server::start;
use loro::LoroValue;

#[tokio::test]
async fn joining_a_new_room_gives_joined_and_an_empty_snapshot() {
    let server = start(test_config()).await.expect("start");
    let mut a = Client::connect(server.local_addr(), 1, "fresh-room").await;
    let (joined, sync) = a.join().await;
    let ServerMessage::Joined { protocol_version, room, client_id, peers, limits, .. } = joined else { panic!() };
    assert_eq!((protocol_version, room.as_str(), client_id), (1, "fresh-room", a.id));
    assert!(peers.is_empty());
    assert_eq!(limits.max_updates_per_sec, 60);
    assert_eq!(sync_kind(&sync), SyncKind::Snapshot, "an empty client gets a snapshot");
    server.shutdown().await;
}

#[tokio::test]
async fn a_client_with_an_empty_version_blob_also_gets_a_snapshot() {
    let server = start(test_config()).await.expect("start");
    let mut a = Client::connect(server.local_addr(), 1, "empty-blob").await;
    let mut join = a.join_message(None);
    if let ClientMessage::Join { version, .. } = &mut join {
        version.clear();
    }
    a.send(&join).await;
    assert!(matches!(a.recv().await, ServerMessage::Joined { .. }));
    assert_eq!(sync_kind(&a.recv().await), SyncKind::Snapshot);
    server.shutdown().await;
}

#[tokio::test]
async fn live_edits_converge_and_are_not_echoed_to_the_sender() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "live").await;
    let mut b = Client::connect(addr, 2, "live").await;
    a.join().await;
    let (joined_b, _) = b.join().await;
    let ServerMessage::Joined { peers, .. } = joined_b else { panic!() };
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].client_id, a.id);
    assert!(matches!(a.recv().await, ServerMessage::PeerJoined { peer } if peer.client_id == b.id));

    let seq = a.edit("rect-1", text("first")).await;
    // The sender gets exactly an Ack; the other peer gets the Update stamped with the sender.
    assert!(matches!(a.recv().await, ServerMessage::Ack { seq: s } if s == seq));
    let ServerMessage::Update { from, update } = b.recv().await else { panic!("expected a relayed update") };
    assert_eq!(from, a.id);
    b.doc.import(&update).expect("import");
    assert_eq!(state(&a.doc), state(&b.doc));

    b.edit_acked("rect-2", text("second")).await;
    a.converge_to(&state(&b.doc)).await;
    assert!(!a.next_within(Duration::from_millis(150)).await.is_msg(), "nothing is echoed");
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_edits_to_the_same_key_converge_on_every_peer() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut clients = Vec::new();
    for n in 1..=3u8 {
        let mut c = Client::connect(addr, n, "concurrent").await;
        c.join().await;
        clients.push(c);
    }
    for (i, c) in clients.iter_mut().enumerate() {
        c.edit("shared", text(&format!("writer-{i}"))).await;
        c.edit(&format!("own-{i}"), text("x")).await;
    }
    // Everyone drains until all three documents agree.
    let mut agreed = false;
    for _ in 0..100 {
        for c in clients.iter_mut() {
            while let Event::Msg(m) = c.next_within(Duration::from_millis(20)).await {
                if let ServerMessage::Update { update, .. } = m {
                    c.doc.import(&update).expect("import");
                }
            }
        }
        if state(&clients[0].doc) == state(&clients[1].doc) && state(&clients[1].doc) == state(&clients[2].doc) {
            agreed = true;
            break;
        }
    }
    assert!(agreed, "documents differ: {:?}", clients.iter().map(|c| state(&c.doc)).collect::<Vec<_>>());
    server.shutdown().await;
}

#[tokio::test]
async fn late_joiner_gets_a_snapshot_and_an_old_client_gets_only_what_it_lacks() {
    let mut config = test_config();
    config.limits.max_updates_per_sec = 5000; // 300 quick edits must not hit the rate limit
    let server = start(config).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "catchup").await;
    a.join().await;
    // A makes a document big enough that a snapshot is much larger than a small diff.
    for i in 0..300 {
        a.edit_acked(&format!("el-{i}"), text(&"x".repeat(200))).await;
    }
    // C joins now and is current.
    let mut c = Client::connect(addr, 3, "catchup").await;
    let (_, sync) = c.join().await;
    assert_eq!(sync_kind(&sync), SyncKind::Snapshot);
    let ServerMessage::SyncResponse { data: snapshot, .. } = &sync else { panic!() };
    assert_eq!(state(&c.doc), state(&a.doc));
    c.close().await;

    // C's copy as of now; then A makes 3 more edits while C is away.
    let mut c = Client::connect(addr, 3, "catchup").await;
    let old = loro::LoroDoc::new();
    old.import(snapshot).expect("import");
    c.doc = old;
    for i in 0..3 {
        a.edit_acked(&format!("late-{i}"), text("y")).await;
    }
    let (_, sync) = c.join().await;
    assert_eq!(sync_kind(&sync), SyncKind::Updates);
    let ServerMessage::SyncResponse { data, .. } = &sync else { panic!() };
    assert!(data.len() * 10 < snapshot.len(), "diff {} bytes vs snapshot {} bytes", data.len(), snapshot.len());
    assert_eq!(state(&c.doc), state(&a.doc));

    // A client that is already current gets an empty response.
    c.close().await;
    let mut c = Client::connect(addr, 3, "catchup").await;
    let cur = loro::LoroDoc::new();
    cur.import(&a.doc.export(loro::ExportMode::Snapshot).expect("export")).expect("import");
    c.doc = cur;
    let (_, sync) = c.join().await;
    assert!(matches!(&sync, ServerMessage::SyncResponse { kind: SyncKind::Updates, data, .. } if data.is_empty()));
    server.shutdown().await;
}

#[tokio::test]
async fn reconnecting_resumes_from_the_version_and_uploads_offline_edits() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "resume").await;
    let mut b = Client::connect(addr, 2, "resume").await;
    a.join().await;
    b.join().await;
    b.edit_acked("before", text("1")).await;
    a.converge_to(&state(&b.doc)).await;

    // B drops. While it is away, A edits, and B edits offline.
    b.close_now();
    a.edit_acked("while-away-a", text("2")).await;
    let mut b = reconnect(addr, b_doc_of(&a).await, 2, "resume").await;
    b.edit_offline("while-away-b", text("3"));
    let (joined, sync) = b.join().await;
    assert_eq!(sync_kind(&sync), SyncKind::Updates, "a returning client is resumed, not rebuilt");
    let ServerMessage::Joined { server_version, .. } = joined else { panic!() };
    // A knows nothing of "while-away-b" yet; B uploads what the server lacks.
    let seq = b.upload_since(&server_version).await.expect("B has changes to upload");
    b.recv_until(|m| matches!(m, ServerMessage::Ack { seq: s } if *s == seq).then_some(())).await;
    a.converge_to(&state(&b.doc)).await;
    assert_eq!(state(&a.doc), state(&b.doc));
    server.shutdown().await;
}

/// B's document before it dropped: it had "before" but not A's later edit.
async fn b_doc_of(a: &Client) -> loro::LoroDoc {
    // Rebuild from A's history up to (but excluding) its last edit.
    let doc = loro::LoroDoc::new();
    let vv_all = a.doc.oplog_vv();
    let mut frontier_vv = vv_all.clone();
    // Drop the last op of every peer by one counter: B saw everything but A's last change.
    let own = a.doc.peer_id();
    if let Some(c) = frontier_vv.get(&own).copied() {
        frontier_vv.insert(own, c - 1);
    }
    let bytes = a.doc.export(loro::ExportMode::updates_till(&frontier_vv)).expect("export");
    doc.import(&bytes).expect("import");
    doc
}

async fn reconnect(addr: std::net::SocketAddr, doc: loro::LoroDoc, n: u8, room: &str) -> Client {
    let mut c = Client::connect(addr, n, room).await;
    c.doc = doc;
    c
}

#[tokio::test]
async fn a_second_connection_with_the_same_client_id_replaces_the_first() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut old = Client::connect(addr, 7, "replace").await;
    old.join().await;
    let mut other = Client::connect(addr, 8, "replace").await;
    other.join().await;
    let mut new = Client::connect(addr, 7, "replace").await;
    new.join().await;
    let codes = old.expect_closed().await;
    assert_eq!(codes, [ErrorCode::Replaced]);
    // The newcomer works and the other peer saw the id leave and come back.
    new.edit_acked("k", text("v")).await;
    other.converge_to(&state(&new.doc)).await;
    server.shutdown().await;
}

#[tokio::test]
async fn awareness_is_relayed_stamped_and_replayed_to_late_joiners() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "aware").await;
    let mut b = Client::connect(addr, 2, "aware").await;
    a.join().await;
    b.join().await;
    let state_a =
        AwarenessState { cursor: Some(Point { x: 1.5, y: -2.0 }), selection: vec!["e1".into()], ..Default::default() };
    a.send(&ClientMessage::Awareness(state_a.clone())).await;
    let got = b
        .recv_until(|m| match m {
            ServerMessage::PeerAwareness { from, state } => Some((*from, state.clone())),
            _ => None,
        })
        .await;
    assert_eq!(got, (a.id, state_a.clone()));
    // A late joiner is told the latest state of the peers already there.
    let mut c = Client::connect(addr, 3, "aware").await;
    c.join().await;
    let got = c
        .recv_until(|m| match m {
            ServerMessage::PeerAwareness { from, state } => Some((*from, state.clone())),
            _ => None,
        })
        .await;
    assert_eq!(got, (a.id, state_a));
    // Leaving is announced.
    a.send(&ClientMessage::Leave).await;
    b.recv_until(|m| matches!(m, ServerMessage::PeerLeft { client_id } if *client_id == a.id).then_some(())).await;
    server.shutdown().await;
}

#[tokio::test]
async fn ping_is_answered_and_the_server_pings_idle_clients() {
    let mut config = test_config();
    config.timing.ping_interval = Duration::from_millis(100);
    let server = start(config).await.expect("start");
    let mut a = Client::connect(server.local_addr(), 1, "ping").await;
    a.join().await;
    a.send(&ClientMessage::Ping { nonce: 77 }).await;
    a.recv_until(|m| matches!(m, ServerMessage::Pong { nonce: 77 }).then_some(())).await;
    let nonce = a.recv_until(|m| if let ServerMessage::Ping { nonce } = m { Some(*nonce) } else { None }).await;
    a.send(&ClientMessage::Pong { nonce }).await;
    server.shutdown().await;
}

#[tokio::test]
async fn silent_connections_are_closed_after_the_idle_timeout() {
    let mut config = test_config();
    config.timing.idle_timeout = Duration::from_millis(300);
    config.timing.ping_interval = Duration::from_secs(60);
    let server = start(config).await.expect("start");
    let mut a = Client::connect(server.local_addr(), 1, "idle-conn").await;
    a.join().await;
    a.expect_closed().await;
    server.shutdown().await;
}

#[tokio::test]
async fn shutdown_tells_peers_and_saves_the_room() {
    let server = start(test_config()).await.expect("start");
    let mut a = Client::connect(server.local_addr(), 1, "bye").await;
    a.join().await;
    a.edit_acked("k", text("v")).await;
    let handle = tokio::spawn(server.shutdown());
    let codes = a.expect_closed().await;
    assert_eq!(codes, [ErrorCode::Unavailable]);
    handle.await.expect("shutdown finishes");
}

#[tokio::test]
async fn request_sync_returns_what_the_client_lacks() {
    let server = start(test_config()).await.expect("start");
    let addr = server.local_addr();
    let mut a = Client::connect(addr, 1, "reqsync").await;
    let mut b = Client::connect(addr, 2, "reqsync").await;
    a.join().await;
    b.join().await;
    for i in 0..5 {
        a.edit_acked(&format!("k{i}"), LoroValue::from(i as i64)).await;
    }
    // B ignores the relays (as after a gap) and asks for a sync from its version.
    b.send(&ClientMessage::RequestSync { version: b.doc.oplog_vv().encode() }).await;
    let sync = b.recv_until(|m| matches!(m, ServerMessage::SyncResponse { .. }).then(|| m.clone())).await;
    b.apply_sync(&sync);
    assert_eq!(state(&a.doc), state(&b.doc));
    // A garbage version vector is refused.
    b.send(&ClientMessage::RequestSync { version: vec![0xff; 40] }).await;
    assert_eq!(b.expect_closed().await, [ErrorCode::Malformed]);
    server.shutdown().await;
}

trait IsMsg {
    fn is_msg(&self) -> bool;
}
impl IsMsg for Event {
    fn is_msg(&self) -> bool {
        matches!(self, Event::Msg(_))
    }
}
