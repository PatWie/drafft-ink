#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The client state machine against a scripted server: no sockets, no clock.

mod common;

use common::*;
use drafftink_collab::*;
use drafftink_protocol::{
    AWARENESS_HEARTBEAT_MS, AWARENESS_TIMEOUT_MS, AwarenessState, ClientMessage, ErrorCode, IDLE_TIMEOUT_MS, Limits,
    PING_INTERVAL_MS, PeerInfo, Point, ServerMessage, SyncKind, Viewport,
};

fn ana() -> PeerInfo {
    PeerInfo { client_id: cid(2), user: Some(user("ana")) }
}

fn closed(error: &str) -> TransportEvent {
    TransportEvent::Closed { error: Some(error.to_owned()) }
}

// ---- join ---------------------------------------------------------------------------------------------------------

#[test]
fn start_connects_then_joins_with_the_documents_version_and_the_token() {
    let mut h = Harness::new();
    add(&mut h.store, &["a"]);
    h.start();
    assert_eq!(h.connects, 1);
    assert_eq!(h.client.status().state, ConnState::Connecting);
    h.event(TransportEvent::Opened);
    let frames = h.take_frames();
    let [ClientMessage::Join { min_version, max_version, room, client_id, auth_token, version, user }] = &frames[..]
    else {
        panic!("expected one Join, got {frames:?}")
    };
    assert_eq!((*min_version, *max_version), (1, 1));
    assert_eq!((room, *client_id), (&common::room(), cid(1)));
    assert_eq!(auth_token.as_deref(), Some("s3cret-token"));
    assert_eq!(version, &h.store.version_vector().unwrap());
    assert_eq!(user.as_ref().map(|u| u.name.as_str()), Some("me"));
}

#[test]
fn nothing_happens_before_start() {
    let mut h = Harness::new();
    h.advance(100_000);
    assert_eq!((h.connects, h.client.status().state), (0, ConnState::Idle));
    assert_eq!(h.client.next_wake_ms(), None);
}

#[test]
fn joining_imports_the_servers_data_and_uploads_what_the_server_lacks() {
    let mut h = Harness::new();
    add(&mut h.server, &["from-server"]);
    add(&mut h.store, &["made-offline"]);
    h.start();
    h.event(TransportEvent::Opened);
    h.take_frames();
    let joined = h.joined(vec![ana()], h.server.version_vector().unwrap());
    h.feed(&joined);
    assert_eq!(h.client.status().state, ConnState::Syncing);
    assert_eq!(h.client.peers().len(), 1);
    let sync = h.sync_from(&h.store.version_vector().unwrap());
    h.feed(&sync);
    assert_eq!(h.client.status().state, ConnState::Online);
    let events = h.take_events();
    let batches = Harness::remote_batches(&events);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].kind, RemoteKind::CatchUp);
    assert_eq!(batches[0].counts(), (1, 0, 0));
    assert_eq!(ids(&h.store), ["from-server", "made-offline"].map(String::from));
    // The offline edit goes up.
    let frames = h.take_frames();
    h.server_applies(&frames);
    assert_eq!(ids(&h.server), ids(&h.store));
    let seq = Harness::updates(&frames)[0].0;
    assert!(h.client.status().unsynced);
    h.feed(&ServerMessage::Ack { seq });
    assert!(!h.client.status().unsynced);
}

#[test]
fn a_snapshot_goes_through_the_same_path_and_is_reported_as_one() {
    let mut h = Harness::new();
    add(&mut h.server, &["a", "b", "c"]);
    h.start();
    h.event(TransportEvent::Opened);
    let joined = h.joined(vec![], h.server.version_vector().unwrap());
    h.feed(&joined);
    h.feed(&ServerMessage::SyncResponse {
        kind: SyncKind::Snapshot,
        data: h.server.export_snapshot().unwrap(),
        server_version: h.server.version_vector().unwrap(),
    });
    let batches = Harness::remote_batches(&h.take_events());
    assert_eq!((batches[0].kind, batches[0].counts()), (RemoteKind::Snapshot, (3, 0, 0)));
    assert_eq!(ids(&h.store).len(), 3);
}

#[test]
fn a_join_answered_for_another_room_or_version_is_not_trusted() {
    for bad in 0..3 {
        let mut h = Harness::new();
        h.start();
        h.event(TransportEvent::Opened);
        let mut joined = h.joined(vec![], vec![]);
        if let ServerMessage::Joined { protocol_version, room, client_id, .. } = &mut joined {
            match bad {
                0 => *protocol_version = 9,
                1 => *room = drafftink_protocol::RoomId::new("other").unwrap(),
                _ => *client_id = cid(9),
            }
        }
        h.feed(&joined);
        assert_eq!(h.client.status().state, ConnState::Reconnecting, "case {bad}");
        assert!(h.closes >= 1);
    }
}

// ---- local edits --------------------------------------------------------------------------------------------------

#[test]
fn local_commits_are_coalesced_into_one_update_after_the_debounce() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    add(&mut h.store, &["b"]);
    h.client.local_changed(h.now);
    h.advance(10);
    add(&mut h.store, &["c"]);
    h.client.local_changed(h.now);
    h.advance(10);
    assert!(Harness::updates(&h.frames).is_empty(), "inside the debounce window nothing leaves");
    assert!(h.client.status().unsynced);
    h.advance(SEND_DEBOUNCE_MS);
    let frames = h.take_frames();
    let updates = Harness::updates(&frames);
    assert_eq!(updates.len(), 1, "{frames:?}");
    h.server_applies(&frames);
    assert_eq!(ids(&h.server), ["b", "c"].map(String::from));
    h.feed(&ServerMessage::Ack { seq: updates[0].0 });
    assert!(!h.client.status().unsynced);
    // Idle: the only timer left is the next ping, far away.
    let wake = h.client.next_wake_ms().unwrap();
    assert!(wake >= h.now + u64::from(PING_INTERVAL_MS) - 1_000, "wake in {} ms", wake - h.now);
}

#[test]
fn updates_wait_when_too_many_are_unacknowledged_and_leave_as_one_when_acks_arrive() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    for i in 0..(MAX_INFLIGHT + 8) {
        add(&mut h.store, &[&format!("e{i}")]);
        h.client.local_changed(h.now);
        h.advance(100);
    }
    let frames = h.take_frames();
    let sent = Harness::updates(&frames);
    assert_eq!(sent.len(), MAX_INFLIGHT, "the window is full");
    h.advance(5_000);
    assert!(Harness::updates(&h.take_frames()).is_empty(), "no busy retry while the window is full");
    // Acknowledge one: the 8 waiting commits leave together.
    h.server_applies(&frames);
    h.feed(&ServerMessage::Ack { seq: sent[0].0 });
    h.advance(100);
    let more = h.take_frames();
    assert_eq!(Harness::updates(&more).len(), 1);
    h.server_applies(&more);
    assert_eq!(ids(&h.server).len(), MAX_INFLIGHT + 8);
}

#[test]
fn an_update_that_is_never_acknowledged_makes_the_client_reconcile() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    add(&mut h.store, &["x"]);
    h.client.local_changed(h.now);
    h.advance(SEND_DEBOUNCE_MS);
    h.take_frames();
    // Keep the connection alive with server pings but never ack.
    for _ in 0..(ACK_TIMEOUT_MS / 5_000 + 1) {
        h.advance(5_000);
        h.feed(&ServerMessage::Pong { nonce: 0 });
    }
    let frames = h.take_frames();
    assert!(frames.iter().any(|m| matches!(m, ClientMessage::RequestSync { .. })), "{frames:?}");
}

#[test]
fn remote_operations_are_never_echoed_back() {
    let mut h = Harness::new();
    h.go_online(vec![ana()]);
    let before = h.server.version_vector().unwrap();
    let ids_50: Vec<String> = (0..50).map(|i| format!("r{i}")).collect();
    add(&mut h.server, &ids_50.iter().map(String::as_str).collect::<Vec<_>>());
    let update = h.server.export_updates_since(&before).unwrap();
    h.feed(&ServerMessage::Update { from: cid(2), update });
    let batches = Harness::remote_batches(&h.take_events());
    assert_eq!(batches.len(), 1);
    assert_eq!(
        (batches[0].kind, batches[0].from, batches[0].who.as_deref()),
        (RemoteKind::Live, Some(cid(2)), Some("ana"))
    );
    assert_eq!(batches[0].counts(), (50, 0, 0));
    h.advance(500);
    assert!(Harness::updates(&h.take_frames()).is_empty(), "applying a remote update sends nothing");
    // A local commit afterwards carries only the local operations.
    add(&mut h.store, &["mine"]);
    h.client.local_changed(h.now);
    h.advance(SEND_DEBOUNCE_MS);
    let updates = Harness::updates(&h.take_frames());
    assert_eq!(updates.len(), 1);
    assert!(
        updates[0].1.len() < 600,
        "{} bytes: the update must not repeat the 50 remote elements",
        updates[0].1.len()
    );
}

#[test]
fn an_update_that_arrives_before_its_predecessor_triggers_a_sync_request() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    let v0 = h.server.version_vector().unwrap();
    add(&mut h.server, &["first"]);
    let v1 = h.server.version_vector().unwrap();
    add(&mut h.server, &["second"]);
    let late = h.server.export_updates_since(&v1).unwrap();
    h.feed(&ServerMessage::Update { from: cid(2), update: late });
    h.advance(10);
    let frames = h.take_frames();
    let [ClientMessage::RequestSync { version }] = &frames[..] else { panic!("{frames:?}") };
    assert_eq!(version, &h.store.version_vector().unwrap());
    assert!(ids(&h.store).is_empty(), "the update waits for its predecessor");
    // The answer fills the gap.
    let sync = h.sync_from(&v0);
    h.feed(&sync);
    assert_eq!(ids(&h.store), ["first", "second"].map(String::from));
}

#[test]
fn an_update_too_large_for_the_server_is_kept_and_reported_not_sent() {
    let mut h = Harness::new();
    h.start();
    h.event(TransportEvent::Opened);
    let small = Limits { max_update_bytes: 400, ..Limits::DEFAULT };
    let mut joined = h.joined(vec![], h.server.version_vector().unwrap());
    if let ServerMessage::Joined { limits, .. } = &mut joined {
        *limits = small;
    }
    h.feed(&joined);
    let sync = h.sync_response();
    h.feed(&sync);
    h.settle_uploads();
    h.take_events();
    let many: Vec<String> = (0..20).map(|i| format!("big{i}")).collect();
    add(&mut h.store, &many.iter().map(String::as_str).collect::<Vec<_>>());
    h.client.local_changed(h.now);
    h.advance(SEND_DEBOUNCE_MS);
    assert!(Harness::updates(&h.take_frames()).is_empty());
    let problems = Harness::problems(&h.take_events());
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].action, ProblemAction::ReduceSize);
    assert!(h.client.status().unsynced && h.client.status().problem.is_some());
    // Later small edits do not leapfrog the stuck one.
    add(&mut h.store, &["small"]);
    h.client.local_changed(h.now);
    h.advance(1000);
    assert!(Harness::updates(&h.take_frames()).is_empty());
}

// ---- reconnecting -------------------------------------------------------------------------------------------------

#[test]
fn a_dropped_connection_reconnects_with_growing_backoff() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    h.event(closed("reset"));
    let st = h.client.status().clone();
    assert_eq!(st.state, ConnState::Reconnecting);
    assert!(st.text.contains("kept on this device"), "{}", st.text);
    let delay = st.retry_at_ms.unwrap() - h.now;
    assert!((250..=500).contains(&delay), "first delay {delay}");
    let connects = h.connects;
    h.advance(delay - 1);
    assert_eq!(h.connects, connects, "not before the delay");
    h.advance(1);
    assert_eq!(h.connects, connects + 1);
    // The attempt fails: the next delay is longer.
    h.event(closed("refused"));
    let delay2 = h.client.status().retry_at_ms.unwrap() - h.now;
    assert!((500..=1000).contains(&delay2), "second delay {delay2}");
    h.event(TransportEvent::Closed { error: None });
    assert_eq!(h.client.status().state, ConnState::Reconnecting);
}

#[test]
fn the_backoff_is_capped_and_resets_after_a_stable_connection() {
    let mut h = Harness::new();
    h.start();
    for _ in 0..12 {
        let at = h.client.status().retry_at_ms.unwrap_or(h.now);
        h.now = at.max(h.now);
        h.tick();
        h.event(closed("down"));
    }
    let delay = h.client.status().retry_at_ms.unwrap() - h.now;
    assert!((15_000..=30_000).contains(&delay), "capped delay {delay}");
    // Now a connection that works and lasts.
    h.now = h.client.status().retry_at_ms.unwrap();
    h.tick();
    h.event(TransportEvent::Opened);
    let joined = h.joined(vec![], h.server.version_vector().unwrap());
    h.feed(&joined);
    let sync = h.sync_response();
    h.feed(&sync);
    h.settle_uploads();
    for _ in 0..12 {
        h.advance(1_000);
        h.feed(&ServerMessage::Pong { nonce: 0 });
    }
    h.event(closed("down"));
    let delay = h.client.status().retry_at_ms.unwrap() - h.now;
    assert!((250..=500).contains(&delay), "reset delay {delay}");
}

#[test]
fn a_server_retry_hint_overrides_a_shorter_backoff() {
    let mut h = Harness::new();
    h.start();
    h.event(TransportEvent::Opened);
    h.feed(&ServerMessage::Error {
        code: ErrorCode::Unavailable,
        message: "restarting".into(),
        retry_after_ms: Some(7_000),
    });
    let st = h.client.status();
    assert_eq!(st.state, ConnState::Reconnecting);
    assert!(st.retry_at_ms.unwrap() - h.now >= 7_000);
}

#[test]
fn edits_made_offline_and_updates_lost_in_flight_are_uploaded_after_reconnect() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    // An update the server never saw: sent, not applied by the server, not acknowledged.
    add(&mut h.store, &["lost"]);
    h.client.local_changed(h.now);
    h.advance(SEND_DEBOUNCE_MS);
    assert_eq!(Harness::updates(&h.take_frames()).len(), 1);
    h.event(closed("reset"));
    // Offline editing.
    add(&mut h.store, &["offline1"]);
    h.client.local_changed(h.now);
    h.advance(100);
    add(&mut h.store, &["offline2"]);
    h.client.local_changed(h.now);
    assert!(h.client.status().unsynced);
    assert!(h.take_frames().is_empty(), "nothing is sent while offline");
    // Reconnect.
    let until = h.client.status().retry_at_ms.unwrap();
    h.now = until.max(h.now);
    h.tick();
    h.event(TransportEvent::Opened);
    let frames = h.take_frames();
    assert!(
        matches!(&frames[..], [ClientMessage::Join { version, .. }] if version == &h.store.version_vector().unwrap())
    );
    let joined = h.joined(vec![], h.server.version_vector().unwrap());
    h.feed(&joined);
    let sync = h.sync_from(&h.store.version_vector().unwrap());
    h.feed(&sync);
    let frames = h.take_frames();
    let updates = Harness::updates(&frames);
    assert_eq!(updates.len(), 1, "one catch-up upload: {frames:?}");
    h.server_applies(&frames);
    assert_eq!(ids(&h.server), ["lost", "offline1", "offline2"].map(String::from));
    h.feed(&ServerMessage::Ack { seq: updates[0].0 });
    assert!(!h.client.status().unsynced);
    // And nothing is sent twice later.
    h.advance(1_000);
    assert!(Harness::updates(&h.take_frames()).is_empty());
}

#[test]
fn a_silent_server_is_pinged_and_then_dropped() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    h.run_for(u64::from(PING_INTERVAL_MS) + 1_000, 1_000);
    let pings: Vec<_> = h.frames.iter().filter(|m| matches!(m, ClientMessage::Ping { .. })).collect();
    assert_eq!(pings.len(), 1, "{:?}", h.frames);
    h.run_for(u64::from(IDLE_TIMEOUT_MS), 1_000);
    assert!(h.closes >= 1, "the quiet connection was closed");
    assert!(h.connects >= 2, "and a new one was tried");
}

#[test]
fn any_frame_from_the_server_keeps_the_connection_alive() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    for _ in 0..6 {
        h.run_for(10_000, 1_000);
        h.feed(&ServerMessage::Pong { nonce: 1 });
    }
    assert_eq!(h.client.status().state, ConnState::Online);
}

#[test]
fn server_pings_get_a_pong_with_the_same_nonce() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    h.feed(&ServerMessage::Ping { nonce: 77 });
    assert!(h.take_frames().contains(&ClientMessage::Pong { nonce: 77 }));
}

#[test]
fn a_connection_that_never_opens_or_never_joins_times_out() {
    let mut h = Harness::new();
    h.start();
    h.run_for(CONNECT_TIMEOUT_MS, 1_000);
    assert_eq!(h.client.status().state, ConnState::Reconnecting);
    let until = h.client.status().retry_at_ms.unwrap();
    h.now = until;
    h.tick();
    h.event(TransportEvent::Opened);
    h.run_for(JOIN_TIMEOUT_MS, 1_000);
    assert_eq!(h.client.status().state, ConnState::Reconnecting);
}

#[test]
fn an_unreadable_frame_drops_the_connection() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    h.event(TransportEvent::Message(vec![0xff, 1, 2]));
    assert_eq!(h.client.status().state, ConnState::Reconnecting);
}

// ---- resync and errors --------------------------------------------------------------------------------------------

#[test]
fn resync_required_sends_the_current_version_and_imports_the_answer() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    add(&mut h.server, &["missed"]);
    h.feed(&error(ErrorCode::ResyncRequired));
    let frames = h.take_frames();
    let [ClientMessage::RequestSync { version }] = &frames[..] else { panic!("{frames:?}") };
    assert_eq!(version, &h.store.version_vector().unwrap());
    let sync = h.sync_from(version);
    h.feed(&sync);
    assert_eq!(ids(&h.store), vec!["missed".to_owned()]);
    assert_eq!(h.client.status().state, ConnState::Online);
    // Asking again at once is spaced out.
    h.feed(&error(ErrorCode::ResyncRequired));
    assert!(h.take_frames().is_empty());
    h.advance(RESYNC_MIN_GAP_MS);
    assert!(matches!(&h.take_frames()[..], [ClientMessage::RequestSync { .. }]));
}

#[test]
fn rate_limited_pauses_updates_then_repairs_by_reconciling() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    add(&mut h.store, &["a"]);
    h.client.local_changed(h.now);
    h.advance(SEND_DEBOUNCE_MS);
    let mut all = h.take_frames(); // the server dropped this one
    h.feed(&ServerMessage::Error { code: ErrorCode::RateLimited, message: "slow".into(), retry_after_ms: Some(2_000) });
    add(&mut h.store, &["b"]);
    h.client.local_changed(h.now);
    h.advance(500);
    assert!(Harness::updates(&h.take_frames()).is_empty(), "paused while rate limited");
    h.advance(1_600);
    let frames = h.take_frames();
    assert!(frames.iter().any(|m| matches!(m, ClientMessage::RequestSync { .. })), "{frames:?}");
    all.extend(frames);
    let sync = h.sync_from(&h.store.version_vector().unwrap());
    h.feed(&sync);
    all.extend(h.take_frames());
    h.server_applies(&all);
    assert_eq!(ids(&h.server), ["a", "b"].map(String::from), "nothing was lost");
    assert_eq!(h.client.status().state, ConnState::Online);
}

#[test]
fn fatal_errors_stop_the_client_and_say_what_to_do() {
    let cases = [
        (ErrorCode::Unauthorized, StopReason::Unauthorized, ProblemAction::NewToken),
        (ErrorCode::Forbidden, StopReason::Forbidden, ProblemAction::AskAdmin),
        (ErrorCode::UnsupportedVersion, StopReason::UpdateApp, ProblemAction::UpdateApp),
        (ErrorCode::Replaced, StopReason::Replaced, ProblemAction::Reload),
        (ErrorCode::TooLarge, StopReason::TooLarge, ProblemAction::ReduceSize),
    ];
    for (code, stop, action) in cases {
        let mut h = Harness::new();
        h.start();
        h.event(TransportEvent::Opened);
        h.feed(&error(code));
        let st = h.client.status().clone();
        assert_eq!((st.state, st.stopped), (ConnState::Stopped, Some(stop)), "{code:?}");
        assert_eq!(st.problem.as_ref().map(|p| p.action), Some(action));
        assert!(st.text.len() > 30 && !st.text.contains("Unauthorized"), "{}", st.text);
        let problems = Harness::problems(&h.events);
        assert_eq!(problems.len(), 1, "{code:?}");
        assert!(problems[0].fatal);
        assert!(h.closes >= 1);
        let connects = h.connects;
        h.run_for(120_000, 10_000);
        assert_eq!(h.connects, connects, "{code:?}: a stopped client does not reconnect");
        // The server's close that follows is not a reason to reconnect either.
        h.event(TransportEvent::Closed { error: None });
        assert_eq!(h.client.status().state, ConnState::Stopped);
    }
}

#[test]
fn retryable_errors_reconnect_by_themselves() {
    for code in
        [ErrorCode::RoomFull, ErrorCode::Unavailable, ErrorCode::Internal, ErrorCode::Malformed, ErrorCode::NotJoined]
    {
        let mut h = Harness::new();
        h.go_online(vec![]);
        h.feed(&error(code));
        assert_eq!(h.client.status().state, ConnState::Reconnecting, "{code:?}");
        let until = h.client.status().retry_at_ms.unwrap();
        h.now = until;
        h.tick();
        assert_eq!(h.connects, 2, "{code:?}");
    }
}

#[test]
fn a_rejected_update_is_reported_and_uploads_stop_for_this_connection() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    add(&mut h.store, &["bad"]);
    h.client.local_changed(h.now);
    h.advance(SEND_DEBOUNCE_MS);
    h.take_frames();
    h.feed(&error(ErrorCode::InvalidUpdate));
    let problems = Harness::problems(&h.take_events());
    assert_eq!(problems[0].action, ProblemAction::SaveCopy);
    add(&mut h.store, &["after"]);
    h.client.local_changed(h.now);
    h.advance(1_000);
    assert!(Harness::updates(&h.take_frames()).is_empty());
    assert_eq!(h.client.status().state, ConnState::Online, "the connection stays");
    assert!(h.client.status().unsynced);
}

#[test]
fn a_document_from_a_newer_schema_stops_sharing_with_an_update_hint() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    // A newer app wrote meta.schema = 99.
    let newer = {
        use loro::{LoroDoc, LoroValue};
        let doc = LoroDoc::new();
        doc.get_map("meta").insert("schema", LoroValue::from(99i64)).unwrap();
        doc.commit();
        doc.export(loro::ExportMode::all_updates()).unwrap()
    };
    h.feed(&ServerMessage::Update { from: cid(2), update: newer });
    let st = h.client.status().clone();
    assert_eq!((st.state, st.stopped), (ConnState::Stopped, Some(StopReason::DocumentTooNew)));
    assert_eq!(st.problem.unwrap().action, ProblemAction::UpdateApp);
}

#[test]
fn a_bad_update_asks_the_server_again_with_growing_gaps() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    for _ in 0..3 {
        h.feed(&ServerMessage::Update { from: cid(2), update: vec![1, 2, 3, 4] });
        h.advance(RESYNC_MAX_GAP_MS + 1);
    }
    let requests = h.frames.iter().filter(|m| matches!(m, ClientMessage::RequestSync { .. })).count();
    assert!((1..=3).contains(&requests), "{requests}");
    assert_eq!(h.client.status().state, ConnState::Online);
    assert!(!Harness::problems(&h.events).is_empty());
}

// ---- trimmed history ----------------------------------------------------------------------------------------------

fn trimmed_setup(replace: bool) -> Harness {
    let mut cfg = config("ws://127.0.0.1:3030");
    cfg.replace_when_trimmed = replace;
    let mut h = Harness::with(cfg);
    for i in 0..5 {
        add(&mut h.server, &[&format!("s{i}")]);
    }
    add(&mut h.store, &["mine"]);
    h.start();
    h.event(TransportEvent::Opened);
    let joined = h.joined(vec![], h.server.version_vector().unwrap());
    h.feed(&joined);
    h.feed(&ServerMessage::SyncResponse {
        kind: SyncKind::Snapshot,
        data: h.server.export_shallow_snapshot(0).unwrap(),
        server_version: h.server.version_vector().unwrap(),
    });
    h
}

#[test]
fn a_snapshot_that_cannot_merge_waits_for_the_user() {
    let mut h = trimmed_setup(false);
    let events = h.take_events();
    assert!(events.iter().any(|e| matches!(e, CollabEvent::NeedsDecision { .. })), "{events:?}");
    assert_eq!(ids(&h.store), vec!["mine".to_owned()], "nothing replaced yet");
    assert_eq!(h.client.status().state, ConnState::Syncing);
    assert_eq!(h.client.status().problem.as_ref().map(|p| p.action), Some(ProblemAction::Decide));
    // Waiting for a decision is not a join timeout.
    h.run_for(JOIN_TIMEOUT_MS * 2, 1_000);
    h.feed(&ServerMessage::Pong { nonce: 0 });
    assert_eq!(h.client.status().state, ConnState::Syncing);
    h.client.resolve_trimmed(h.now, &mut h.store, TrimmedChoice::ReplaceWithServer);
    h.drain();
    assert_eq!(ids(&h.store), ids(&h.server));
    assert_eq!(h.client.status().state, ConnState::Online);
    let batches = Harness::remote_batches(&h.take_events());
    assert_eq!(batches.last().map(|b| b.kind), Some(RemoteKind::Replaced));
}

#[test]
fn keeping_the_local_copy_leaves_the_room() {
    let mut h = trimmed_setup(false);
    h.client.resolve_trimmed(h.now, &mut h.store, TrimmedChoice::KeepMine);
    h.drain();
    assert_eq!(h.client.status().stopped, Some(StopReason::Declined));
    assert_eq!(ids(&h.store), vec!["mine".to_owned()]);
    assert!(h.closes >= 1);
}

#[test]
fn a_headless_client_can_replace_without_asking() {
    let h = trimmed_setup(true);
    assert_eq!(ids(&h.store), ids(&h.server));
    assert_eq!(h.client.status().state, ConnState::Online);
}

// ---- leaving ------------------------------------------------------------------------------------------------------

#[test]
fn leaving_sends_the_pending_edits_then_leave_then_closes_and_does_not_reconnect() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    add(&mut h.store, &["last"]);
    h.client.local_changed(h.now);
    h.client.leave(h.now, &mut h.store);
    h.drain();
    let frames = h.take_frames();
    assert!(matches!(&frames[..], [ClientMessage::Update { .. }, ClientMessage::Leave]), "{frames:?}");
    assert!(h.closes >= 1);
    assert_eq!(h.client.status().stopped, Some(StopReason::Left));
    let connects = h.connects;
    h.run_for(120_000, 10_000);
    assert_eq!(h.connects, connects);
    // Joining again is a new start.
    h.client.start(h.now);
    h.tick();
    assert_eq!(h.connects, connects + 1);
}

#[test]
fn leaving_while_offline_just_stops() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    h.event(closed("down"));
    h.client.leave(h.now, &mut h.store);
    h.drain();
    assert_eq!(h.client.status().stopped, Some(StopReason::Left));
    assert!(h.take_frames().is_empty());
}

// ---- presence -----------------------------------------------------------------------------------------------------

fn at(x: f64, y: f64) -> Point {
    Point { x, y }
}

#[test]
fn presence_is_not_sent_while_alone_and_goes_out_at_once_when_someone_joins() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    h.client.set_cursor(h.now, Some(at(1.0, 1.0)));
    h.run_for(5_000, 100);
    assert!(Harness::awareness(&h.take_frames()).is_empty(), "nobody listens");
    h.feed(&ServerMessage::PeerJoined { peer: ana() });
    let aw = Harness::awareness(&h.take_frames());
    assert_eq!(aw.len(), 1);
    assert_eq!(aw[0].cursor, Some(at(1.0, 1.0)));
    assert_eq!(aw[0].user.as_ref().map(|u| u.name.as_str()), Some("me"));
    let events = h.take_events();
    assert!(events.iter().any(|e| matches!(e, CollabEvent::PeerJoined { name, .. } if name == "ana")));
    assert!(events.contains(&CollabEvent::Presence));
}

#[test]
fn presence_is_throttled_and_only_the_latest_state_goes_out() {
    let mut h = Harness::new();
    h.go_online(vec![ana()]);
    assert_eq!(Harness::awareness(&h.take_frames()).len(), 0, "taken by go_online");
    h.run_for(100, 10);
    h.take_frames();
    let mut sent = Vec::new();
    for i in 0..100u64 {
        h.client.set_cursor(h.now, Some(at(i as f64, 0.0)));
        h.advance(10);
        sent.extend(Harness::awareness(&h.take_frames()));
    }
    // 1000 ms at one message per 50 ms.
    assert!((15..=21).contains(&sent.len()), "{} awareness messages in one second", sent.len());
    h.advance(100);
    sent.extend(Harness::awareness(&h.take_frames()));
    assert_eq!(sent.last().unwrap().cursor, Some(at(99.0, 0.0)));
}

#[test]
fn an_idle_client_sends_one_heartbeat_per_interval_and_wakes_only_for_it() {
    let mut h = Harness::new();
    h.go_online(vec![ana()]);
    h.run_for(100, 50);
    h.take_frames();
    let wake = h.client.next_wake_ms().unwrap();
    assert!(wake <= h.now + u64::from(AWARENESS_HEARTBEAT_MS));
    let mut count = 0;
    let end = h.now + 3 * u64::from(AWARENESS_HEARTBEAT_MS);
    while h.now < end {
        h.now = h.client.next_wake_ms().unwrap().max(h.now + 1);
        h.tick();
        h.feed(&ServerMessage::Pong { nonce: 0 });
        count += Harness::awareness(&h.take_frames()).len();
    }
    assert!((2..=4).contains(&count), "{count} heartbeats in three intervals");
}

#[test]
fn peer_presence_is_kept_replaced_and_expires() {
    let mut h = Harness::new();
    h.go_online(vec![ana()]);
    h.take_events();
    let state = AwarenessState {
        cursor: Some(at(5.0, 6.0)),
        selection: vec!["a".into(), "b".into()],
        viewport: Some(Viewport { x: 0.0, y: 0.0, width: 100.0, height: 50.0 }),
        ..Default::default()
    };
    h.feed(&ServerMessage::PeerAwareness { from: cid(2), state });
    let p = h.client.peers().get(&cid(2)).unwrap().clone();
    assert_eq!((p.cursor, p.selection.len(), p.user.map(|u| u.name)), (Some(at(5.0, 6.0)), 2, Some("ana".into())));
    assert_eq!(h.take_events(), vec![CollabEvent::Presence]);
    // Presence never enters the document.
    assert_eq!(ids(&h.store).len(), 0);
    assert!(h.client.status().peers == 1);
    // Silence for the timeout clears the cursor but keeps the peer.
    h.run_for(u64::from(AWARENESS_TIMEOUT_MS) + 1_000, 1_000);
    let p = h.client.peers().get(&cid(2)).unwrap();
    assert_eq!((p.cursor, p.selection.len()), (None, 0));
    assert!(h.events.contains(&CollabEvent::Presence));
    // Leaving removes the peer.
    h.feed(&ServerMessage::PeerLeft { client_id: cid(2) });
    assert!(h.client.peers().is_empty());
    assert!(h.events.iter().any(|e| matches!(e, CollabEvent::PeerLeft { name, .. } if name == "ana")));
}

#[test]
fn following_a_peer_reports_their_viewport_until_they_leave() {
    let mut h = Harness::new();
    h.go_online(vec![ana()]);
    assert!(!h.client.follow(cid(9)), "unknown peer");
    assert!(h.client.follow(cid(2)));
    h.take_events();
    let v = |x: f64| Viewport { x, y: 0.0, width: 10.0, height: 10.0 };
    h.feed(&ServerMessage::PeerAwareness {
        from: cid(2),
        state: AwarenessState { viewport: Some(v(1.0)), ..Default::default() },
    });
    h.feed(&ServerMessage::PeerAwareness {
        from: cid(2),
        state: AwarenessState { viewport: Some(v(2.0)), ..Default::default() },
    });
    let events = h.take_events();
    let followed: Vec<_> =
        events.iter().filter_map(|e| if let CollabEvent::FollowViewport(v) = e { Some(*v) } else { None }).collect();
    assert_eq!(followed, vec![v(1.0), v(2.0)]);
    h.client.stop_following();
    h.feed(&ServerMessage::PeerAwareness {
        from: cid(2),
        state: AwarenessState { viewport: Some(v(3.0)), ..Default::default() },
    });
    assert!(!h.take_events().iter().any(|e| matches!(e, CollabEvent::FollowViewport(_))));
    assert!(h.client.follow(cid(2)));
    h.take_events();
    h.feed(&ServerMessage::PeerLeft { client_id: cid(2) });
    assert!(h.take_events().contains(&CollabEvent::FollowEnded(FollowEnd::PeerLeft)));
    assert_eq!(h.client.following(), None);
}

#[test]
fn losing_the_connection_forgets_the_peers_and_ends_following() {
    let mut h = Harness::new();
    h.go_online(vec![ana()]);
    h.client.follow(cid(2));
    h.take_events();
    h.event(closed("down"));
    assert!(h.client.peers().is_empty());
    let events = h.take_events();
    assert!(events.contains(&CollabEvent::FollowEnded(FollowEnd::Disconnected)));
    assert!(events.contains(&CollabEvent::Presence));
}

#[test]
fn a_peer_trail_follows_its_cursor_and_fades() {
    let mut h = Harness::new();
    h.go_online(vec![ana()]);
    for i in 0..5 {
        h.feed(&ServerMessage::PeerAwareness {
            from: cid(2),
            state: AwarenessState { cursor: Some(at(f64::from(i), 0.0)), ..Default::default() },
        });
        h.advance(50);
    }
    let p = h.client.peers().get(&cid(2)).unwrap();
    assert_eq!(p.trail().points(h.now).len(), 5);
    assert_eq!(p.trail().points(h.now + TRAIL_TTL_MS).len(), 0);
}

// ---- status and wake-ups ------------------------------------------------------------------------------------------

#[test]
fn status_changes_are_events_and_repeat_nothing() {
    let mut h = Harness::new();
    h.start();
    h.event(TransportEvent::Opened);
    let joined = h.joined(vec![], h.server.version_vector().unwrap());
    h.feed(&joined);
    let sync = h.sync_response();
    h.feed(&sync);
    let states: Vec<_> =
        h.events.iter().filter_map(|e| if let CollabEvent::Status(s) = e { Some(s.state) } else { None }).collect();
    assert_eq!(states, vec![ConnState::Reconnecting, ConnState::Connecting, ConnState::Syncing, ConnState::Online]);
    h.take_events();
    h.advance(1);
    h.advance(1);
    assert!(h.take_events().is_empty());
}

#[test]
fn an_online_idle_client_wakes_only_for_pings_and_never_in_the_past() {
    let mut h = Harness::new();
    h.go_online(vec![]);
    for _ in 0..200 {
        let w = h.client.next_wake_ms().unwrap();
        assert!(w > h.now);
        h.now = w;
        h.tick();
        h.feed(&ServerMessage::Pong { nonce: 0 });
        h.take_frames();
    }
}

#[test]
fn debug_output_never_contains_the_token() {
    let mut h = Harness::new();
    h.go_online(vec![ana()]);
    let text = format!("{:?} {:?} {:?}", h.client, h.client.config(), h.client.status());
    assert!(!text.contains("s3cret-token"), "{text}");
    assert!(text.contains("hidden"));
}
