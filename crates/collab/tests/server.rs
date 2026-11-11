#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Real sockets and the real clock; natively only.
#![allow(clippy::disallowed_methods)]
//! The client against the real `drafftink-server` binary: real sockets, real threads, real wake-ups.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::live::*;
use common::*;
use drafftink_collab::*;
use drafftink_protocol::{AWARENESS_MIN_INTERVAL_MS, ClientMessage, ErrorCode, Point, ServerMessage, Viewport};
use drafftink_scene::*;

fn spec<'a>(n: u8, server: &'a str, room: &'a str) -> PeerSpec<'a> {
    PeerSpec::new(n, server, room)
}

fn color(hex: &str) -> Color {
    Color::parse(hex).unwrap()
}

#[test]
fn concurrent_edits_of_three_clients_converge() {
    let server = Server::start(&[]);
    let url = server.url();
    let mut w = World::new();
    for n in 1..=3 {
        w.spawn(spec(n, &url, "converge"));
    }
    w.wait_until("everyone online", |w| w.peers.iter().all(|p| p.state() == ConnState::Online));
    // The same starting scene for everyone.
    w.peers[0].edit(|tx| {
        tx.insert(rect("shared")).unwrap();
        tx.insert(rect("other")).unwrap();
    });
    w.converged();
    // Concurrent edits, all in the same instant, with no pumping in between: A moves `shared`, B recolors it,
    // C locks it; each also adds its own element.
    w.peers[0].edit(|tx| {
        tx.update(&ElementId::new("shared").unwrap(), |e| e.transform.x = 500.0).unwrap();
        tx.insert(rect("from-a")).unwrap();
    });
    w.peers[1].edit(|tx| {
        tx.update(&ElementId::new("shared").unwrap(), |e| e.style.stroke_color = color("#ff0000")).unwrap();
        tx.insert(rect("from-b")).unwrap();
    });
    w.peers[2].edit(|tx| {
        tx.update(&ElementId::new("shared").unwrap(), |e| e.locked = true).unwrap();
        tx.insert(rect("from-c")).unwrap();
    });
    w.converged();
    let shared = w.peers[0].store.get(&ElementId::new("shared").unwrap()).unwrap().clone();
    assert_eq!(shared.transform.x, 500.0, "the move survived");
    assert_eq!(shared.style.stroke_color, color("#ff0000"), "the recolor survived");
    assert!(shared.locked, "the lock survived");
    assert_eq!(w.peers[1].ids().len(), 5);
    // Every client saw the others' edits as remote batches, and none saw its own.
    for (i, p) in w.peers.iter().enumerate() {
        let from: Vec<_> = p.remote_batches().iter().filter_map(|b| b.from).collect();
        assert!(!from.contains(&p.client().config().client_id), "peer {i} got its own edit back");
        assert!(p.client().stats().remote_batches >= 2, "peer {i}: {:?}", p.client().stats());
    }
}

#[test]
fn a_late_joiner_catches_up_and_then_follows_live_edits() {
    let server = Server::start(&[]);
    let url = server.url();
    let mut w = World::new();
    let a = w.spawn(spec(1, &url, "late"));
    w.wait_until("A online", |w| w.peers[a].state() == ConnState::Online);
    for i in 0..20 {
        w.peers[a].add(&format!("e{i}"));
        w.pump_all();
    }
    w.converged();
    let b = w.spawn(spec(2, &url, "late"));
    w.wait_until("B has the scene", |w| w.peers[b].ids().len() == 20 && w.peers[b].state() == ConnState::Online);
    assert!(w.peers[b].remote_batches().iter().any(|b| matches!(b.kind, RemoteKind::CatchUp | RemoteKind::Snapshot)));
    w.peers[a].add("after");
    w.wait_until("B sees the live edit", |w| w.peers[b].ids().len() == 21);
    assert_eq!(w.peers[b].remote_batches().last().unwrap().kind, RemoteKind::Live);
    assert!(w.all_equal());
}

#[test]
fn edits_made_while_the_server_is_down_sync_after_it_returns() {
    let mut server = Server::start_persistent(&[]);
    let url = server.url();
    let mut w = World::new();
    let a = w.spawn(spec(1, &url, "outage"));
    let b = w.spawn(spec(2, &url, "outage"));
    w.wait_until("both online", |w| w.peers.iter().all(|p| p.state() == ConnState::Online));
    w.peers[a].add("before");
    w.converged();
    // An edit that was sent but may not have been acknowledged when the server dies.
    w.peers[a].add("in-flight");
    w.pump_all();
    server.kill();
    w.wait_until("both notice", |w| w.peers.iter().all(|p| p.state() != ConnState::Online));
    for p in &w.peers {
        assert!(matches!(p.state(), ConnState::Reconnecting | ConnState::Connecting));
    }
    // Both keep editing offline, including the same element.
    w.peers[a].add("a-offline");
    w.peers[a].edit(|tx| tx.update(&ElementId::new("before").unwrap(), |e| e.transform.x = 77.0).unwrap());
    w.peers[b].add("b-offline");
    w.peers[b].edit(|tx| tx.update(&ElementId::new("before").unwrap(), |e| e.locked = true).unwrap());
    w.wait_until("edits are kept and shown as unsynced", |w| {
        w.peers.iter().all(|p| p.client().status().unsynced) && w.peers[a].ids().len() >= 3
    });
    std::thread::sleep(Duration::from_millis(700));
    w.pump_all();
    server.restart();
    w.converged();
    let ids = w.peers[a].ids();
    for want in ["before", "in-flight", "a-offline", "b-offline"] {
        assert!(ids.iter().any(|i| i == want), "{want} missing from {ids:?}");
    }
    let before = w.peers[b].store.get(&ElementId::new("before").unwrap()).unwrap().clone();
    assert!(before.locked && before.transform.x == 77.0, "both offline field edits merged: {before:?}");
    assert!(w.peers[a].client().stats().reconnects >= 1);
    // A brand new client sees the same thing: the server holds everything.
    let c = w.spawn(spec(3, &url, "outage"));
    w.wait_until("C has everything", |w| w.peers[c].ids().len() == 4);
    assert!(w.all_equal());
}

#[test]
fn a_client_that_does_not_read_is_told_to_resync_and_recovers() {
    let server = Server::start(&[]);
    let url = server.url();
    let mut w = World::new();
    let writer = w.spawn(spec(1, &url, "slow"));
    let slow = w.spawn(PeerSpec {
        // A tiny queue and receive buffer, so a client that stops reading backs the server up quickly.
        options: NativeOptions {
            max_queued_bytes: 32 * 1024,
            recv_buffer_bytes: Some(4096),
            ..NativeOptions::default()
        },
        ..spec(2, &url, "slow")
    });
    w.wait_until("both online", |w| w.peers.iter().all(|p| p.state() == ConnState::Online));
    // The slow client stops pumping (a stalled UI thread). The writer sends 2200 updates of 4 KiB of random bytes.
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for i in 0..2200 {
        let bytes: Vec<u8> = (0..4 * 1024)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 24) as u8
            })
            .collect();
        w.peers[writer].skew_ms += 40;
        w.peers[writer].edit(|tx| {
            tx.put_file(FileEntry { mime: "image/png".to_owned(), bytes: Arc::from(bytes) }).unwrap();
        });
        // Let the writer send each commit as its own update: its clock runs 40 ms per commit.
        w.peers[writer].skew_ms += 40;
        w.peers[writer].pump();
        if i % 10 == 9 {
            w.waker.wait(Duration::from_millis(1));
        }
    }
    // Settle the writer without touching the slow client.
    let deadline = std::time::Instant::now() + PATIENCE;
    while w.peers[writer].client().status().unsynced && std::time::Instant::now() < deadline {
        w.peers[writer].pump();
        w.waker.wait(Duration::from_millis(5));
    }
    assert!(!w.peers[writer].client().status().unsynced, "{:?}", w.peers[writer].client().status());
    // Now the slow client reads again: it must see ResyncRequired, ask, and end up equal.
    w.wait_until("the slow client to catch up", |w| {
        w.peers[slow].store.files().count() == 2200 && w.peers[slow].state() == ConnState::Online
    });
    let stats = w.peers[slow].client().stats();
    assert!(stats.resync_required >= 1, "the server must say so when it drops relays: {stats:?}");
    assert!(stats.sync_requests >= 1, "{stats:?}");
    assert_eq!(w.peers[slow].client().stats().reconnects, 0, "a lagging client stays connected");
    assert!(w.all_equal());
}

#[test]
fn auth_off_ignores_the_token_and_auth_on_checks_it() {
    // Off: a token is ignored.
    let off = Server::start(&[]);
    let off_url = off.url();
    let mut w = World::new();
    let a = w.spawn(PeerSpec { token: Some("whatever"), ..spec(1, &off_url, "auth") });
    w.wait_until("online without auth", |w| w.peers[a].state() == ConnState::Online);
    drop(w);
    drop(off);

    // On: the right token works, a wrong or missing token stops the client with a plain-words problem.
    let on = Server::start(&["--auth-token", "open-sesame"]);
    let url = on.url();
    let mut w = World::new();
    let good = w.spawn(PeerSpec { token: Some("open-sesame"), ..spec(1, &url, "auth") });
    let bad = w.spawn(PeerSpec { token: Some("wrong"), ..spec(2, &url, "auth") });
    let none = w.spawn(spec(3, &url, "auth"));
    w.wait_until("good online and bad ones stopped", |w| {
        w.peers[good].state() == ConnState::Online
            && w.peers[bad].state() == ConnState::Stopped
            && w.peers[none].state() == ConnState::Stopped
    });
    for i in [bad, none] {
        let st = w.peers[i].client().status();
        assert_eq!(st.stopped, Some(StopReason::Unauthorized));
        let p = st.problem.clone().unwrap();
        assert_eq!((p.code, p.action, p.fatal), (Some(ErrorCode::Unauthorized), ProblemAction::NewToken, true));
        assert!(!p.text.contains("open-sesame"), "the text must not echo a token");
    }
    // Stopped means stopped: no reconnect storm.
    let before = w.peers[bad].client().stats().connects;
    std::thread::sleep(Duration::from_millis(1500));
    w.pump_all();
    assert_eq!(w.peers[bad].client().stats().connects, before);
    // The good client edits normally.
    w.peers[good].add("x");
    w.wait_until("good client acknowledged", |w| !w.peers[good].client().status().unsynced);
}

#[test]
fn a_server_that_speaks_no_common_version_gets_a_clear_error() {
    // The client always offers version 1. To see the server's real "no common version" answer, send a Join that
    // asks for versions the server does not speak, with the same transport, and give the answer to a client.
    let server = Server::start(&[]);
    let waker = Arc::new(Waker::default());
    let w2 = Arc::clone(&waker);
    let mut transport = NativeTransport::new(Arc::new(move || w2.wake()));
    transport.open(&ServerUrl::parse(&server.url(), UrlPolicy::default()).unwrap()).unwrap();
    let mut got = None;
    for _ in 0..100 {
        match transport.poll() {
            Some(TransportEvent::Opened) => {
                let join = ClientMessage::Join {
                    min_version: 7,
                    max_version: 9,
                    room: drafftink_protocol::RoomId::new("v").unwrap(),
                    client_id: cid(1),
                    auth_token: None,
                    version: vec![],
                    user: None,
                };
                transport.send(join.encode().unwrap()).unwrap();
            }
            Some(TransportEvent::Message(bytes)) => {
                got = Some(bytes);
                break;
            }
            Some(TransportEvent::Closed { .. }) => break,
            None => waker.wait(Duration::from_millis(50)),
        }
    }
    let frame = got.expect("the server answers");
    let ServerMessage::Error { code, message, .. } = ServerMessage::decode(&frame).unwrap() else {
        panic!("not an error")
    };
    assert_eq!(code, ErrorCode::UnsupportedVersion);
    assert!(message.contains('1'), "the server names the versions it speaks: {message}");
    // A client that is given this answer while joining stops and says to update.
    let mut h = Harness::new();
    h.start();
    h.event(TransportEvent::Opened);
    h.event(TransportEvent::Message(frame));
    let st = h.client.status();
    assert_eq!((st.state, st.stopped), (ConnState::Stopped, Some(StopReason::UpdateApp)));
    assert_eq!(st.problem.as_ref().map(|p| p.action), Some(ProblemAction::UpdateApp));
    transport.close();
}

#[test]
fn the_same_identity_joining_again_replaces_the_first_connection_and_the_first_stops() {
    let server = Server::start(&[]);
    let url = server.url();
    let mut w = World::new();
    let first = w.spawn(spec(1, &url, "twin"));
    w.wait_until("first online", |w| w.peers[first].state() == ConnState::Online);
    let second = w.spawn(spec(1, &url, "twin"));
    w.wait_until("first replaced, second online", |w| {
        w.peers[second].state() == ConnState::Online && w.peers[first].state() == ConnState::Stopped
    });
    let st = w.peers[first].client().status();
    assert_eq!(st.stopped, Some(StopReason::Replaced));
    assert_eq!(st.problem.as_ref().map(|p| p.action), Some(ProblemAction::Reload));
    std::thread::sleep(Duration::from_millis(1200));
    w.pump_all();
    assert_eq!(w.peers[first].state(), ConnState::Stopped, "it does not fight for the slot");
    assert_eq!(w.peers[second].state(), ConnState::Online);
}

#[test]
fn presence_travels_between_clients_and_never_enters_the_document() {
    let server = Server::start(&[]);
    let url = server.url();
    let mut w = World::new();
    let a = w.spawn(spec(1, &url, "presence"));
    let b = w.spawn(spec(2, &url, "presence"));
    let c = w.spawn(spec(3, &url, "presence"));
    w.wait_until("everyone sees two peers", |w| w.peers.iter().all(|p| p.client().peers().len() == 2));
    let names: Vec<_> = w.peers[a].client().peers().iter().filter_map(|p| p.user.clone().map(|u| u.name)).collect();
    assert_eq!(names, vec!["user2".to_owned(), "user3".to_owned()]);
    w.converged();
    let sent_before = w.peers[a].client().stats().updates_sent;
    let me = w.peers[a].now();
    w.peers[a].session.client_mut().set_cursor(me, Some(Point { x: 12.5, y: -3.0 }));
    w.peers[a].session.client_mut().set_selection(["e1".to_owned(), "e2".to_owned()]);
    w.peers[a].session.client_mut().set_viewport(Some(Viewport { x: 0.0, y: 0.0, width: 800.0, height: 600.0 }));
    let id_a = w.peers[a].client().config().client_id;
    w.wait_until("B and C see A's presence", |w| {
        [b, c].iter().all(|i| {
            w.peers[*i].client().peers().get(&id_a).is_some_and(|p| {
                p.cursor == Some(Point { x: 12.5, y: -3.0 }) && p.selection.len() == 2 && p.viewport.is_some()
            })
        })
    });
    // Follow mode reports the viewport the followed peer moves to.
    assert!(w.peers[b].session.client_mut().follow(id_a));
    w.peers[b].events.clear();
    let now = w.peers[a].now();
    w.peers[a].session.client_mut().set_viewport(Some(Viewport { x: 100.0, y: 50.0, width: 800.0, height: 600.0 }));
    let _ = now;
    w.wait_until("B is told where A went", |w| {
        w.peers[b].events.iter().any(|e| matches!(e, CollabEvent::FollowViewport(v) if v.x == 100.0))
    });
    // Presence is not the document: nobody's store changed and no update was sent for it.
    for p in &w.peers {
        assert_eq!(p.store.len_all(), 0);
    }
    assert_eq!(w.peers[a].client().stats().updates_sent, sent_before, "{:?}", w.peers[a].client().stats());
    // Throttle: a burst of 200 cursor moves in 100 ms produces a handful of messages.
    let before = awareness_count(&w, b);
    let t0 = std::time::Instant::now();
    let mut i = 0.0;
    while t0.elapsed() < Duration::from_millis(400) {
        let n = w.peers[a].now();
        w.peers[a].session.client_mut().set_cursor(n, Some(Point { x: i, y: 0.0 }));
        w.peers[a].pump();
        i += 1.0;
        std::thread::sleep(Duration::from_millis(1));
    }
    w.wait_until("B sees the last cursor", |w| {
        w.peers[b].client().peers().get(&id_a).and_then(|p| p.cursor).is_some_and(|c| c.x >= i - 3.0)
    });
    let received = awareness_count(&w, b) - before;
    let max = 400 / u64::from(AWARENESS_MIN_INTERVAL_MS) + 4;
    assert!(received <= max, "{received} presence updates reached B in 400 ms, the limit is {max}");
}

/// How many `Presence` events peer `i` has seen.
fn awareness_count(w: &World, i: usize) -> u64 {
    w.peers[i].events.iter().filter(|e| matches!(e, CollabEvent::Presence)).count() as u64
}

#[test]
fn leaving_flushes_the_last_edit_and_the_others_see_the_peer_go() {
    let server = Server::start(&[]);
    let url = server.url();
    let mut w = World::new();
    let a = w.spawn(spec(1, &url, "leave"));
    let b = w.spawn(spec(2, &url, "leave"));
    w.wait_until("both online", |w| {
        w.peers.iter().all(|p| p.state() == ConnState::Online && p.client().peers().len() == 1)
    });
    w.peers[a].add("last-words");
    let now = w.peers[a].now();
    let p = &mut w.peers[a];
    p.session.client_mut().leave(now, &mut p.store);
    p.pump();
    w.wait_until("B has the edit and sees A leave", |w| {
        w.peers[b].ids() == vec!["last-words".to_owned()] && w.peers[b].client().peers().is_empty()
    });
    assert!(w.peers[b].events.iter().any(|e| matches!(e, CollabEvent::PeerLeft { name, .. } if name == "user1")));
    assert_eq!(w.peers[a].client().status().stopped, Some(StopReason::Left));
}

#[test]
fn an_idle_connected_client_causes_no_redraw_requests() {
    let server = Server::start(&[]);
    let url = server.url();
    let mut w = World::new();
    let a = w.spawn(spec(1, &url, "idle"));
    w.wait_until("online", |w| w.peers[a].state() == ConnState::Online);
    w.converged();
    let before = w.peers[a].redraws;
    let wakes_before = w.peers[a].client().stats();
    // Two seconds with nothing to do: the owner sleeps on the wake hook and the pump asks for no frame.
    let end = std::time::Instant::now() + Duration::from_secs(2);
    let mut pumps = 0;
    while std::time::Instant::now() < end {
        w.waker.wait(Duration::from_millis(250));
        w.peers[a].pump();
        pumps += 1;
    }
    assert_eq!(w.peers[a].redraws, before, "an idle client must not request frames");
    assert!(pumps <= 16, "pumped {pumps} times in 2 s");
    assert_eq!(w.peers[a].client().stats().updates_sent, wakes_before.updates_sent);
}

#[test]
fn a_server_that_is_not_there_is_retried_with_backoff_and_not_in_a_loop() {
    // Find a port nobody listens on.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let url = format!("ws://127.0.0.1:{port}/ws");
    let mut w = World::new();
    let a = w.spawn(spec(1, &url, "nobody"));
    let end = std::time::Instant::now() + Duration::from_millis(3_500);
    while std::time::Instant::now() < end {
        w.pump_all();
        let wait = w.peers[a].next_wake.map_or(200, |t| t.saturating_sub(w.peers[a].now()).clamp(1, 200));
        w.waker.wait(Duration::from_millis(wait));
    }
    let stats = w.peers[a].client().stats();
    // Delays of 0.25-0.5 s, 0.5-1 s, 1-2 s: three to five attempts in 3.5 s.
    assert!((3..=6).contains(&stats.connects), "{stats:?}");
    assert_eq!(w.peers[a].state(), ConnState::Reconnecting);
    let st = w.peers[a].client().status();
    assert!(st.text.contains("Cannot reach the server") && st.text.contains("kept on this device"), "{}", st.text);
}
