//! Tests of the `collab` family. Sessions talk through in-memory loopbacks to a [`FakeServer`], so the whole
//! path runs: command, client, transport, server, other client, document. Time is the fake clock; every
//! `settle` advances it in small steps.

use drafftink_scene::{ElementId, SceneRead};
use serde_json::{Value, json};

use super::fake_server::FakeServer;
use super::{CollabNotice, ConnectionState, Loading, RoomLink};
use crate::cmd::tab::RoomBinding;
use crate::ports::fake::Fakes;
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

const URL: &str = "ws://127.0.0.1:3030/ws";

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    s.execute(id, args).unwrap_or_else(|r| panic!("{id} was refused: {} ({})", r.message, r.code))
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = s.execute(id, args).expect_err("the command should be refused");
    assert_eq!(s.fingerprint(), before, "a refusal changes nothing");
    r
}

struct Client {
    s: Session,
    fakes: Fakes,
}

struct World {
    hub: FakeServer,
    c: Vec<Client>,
}

impl World {
    fn new(n: usize) -> Self {
        let c = (0..n)
            .map(|i| {
                let (mut s, fakes) = testing::session();
                // Element ids are sequential per session; skip ahead so two peers never make the same id.
                for _ in 0..i * 100 {
                    s.ports.ids.element_id();
                }
                Client { s, fakes }
            })
            .collect();
        Self { hub: FakeServer::new(), c }
    }

    /// Polls every session and lets the server answer, 40 ms of fake time per round.
    fn settle(&mut self) {
        for _ in 0..40 {
            for c in &mut self.c {
                c.fakes.clock.advance_ms(40);
                c.s.collab_poll();
                self.hub.attach(&c.fakes.net);
            }
            self.hub.step();
        }
    }

    fn join(&mut self, i: usize, room: &str, on_local: &str) {
        let s = &mut self.c[i].s;
        run(s, "collab.connect", json!({"url": URL}));
        run(s, "collab.join_room", json!({"room": room, "on_local": on_local}));
    }

    fn join_all(&mut self, room: &str) {
        for i in 0..self.c.len() {
            self.join(i, room, "refuse");
        }
        self.settle();
    }

    fn s(&mut self, i: usize) -> &mut Session {
        &mut self.c[i].s
    }
}

/// The document without local-only counters, for comparing two peers.
fn content(s: &Session) -> drafftink_scene::Document {
    let mut d = s.tab().doc.snapshot();
    for e in &mut d.elements {
        e.version = 0;
    }
    d
}

fn rect(s: &mut Session, x: f64) -> ElementId {
    testing::add_rect(s, x, 0.0, 40.0, 40.0).unwrap()
}

fn style(s: &mut Session, id: &ElementId, props: Value) {
    run(s, "style.set", json!({"ids": [id.as_str()], "props": props}));
}

fn fill_of(s: &Session, id: &ElementId) -> Option<String> {
    s.element(id).and_then(|e| e.style.fill_color).map(|c| c.to_hex())
}

// ---- collab.connect ----

#[test]
fn connect_checks_the_address() {
    let (mut s, _) = testing::session();
    let r = refuse(&mut s, "collab.connect", json!({"url": "https://example.com"}));
    assert_eq!(r.code, "invalid_url");
    assert!(
        r.message.starts_with("`https://example.com` is not a valid server address. Use a ws:// or wss:// URL such as `wss://host/ws`. A bare `/ws` works only in the web app, where it means this page's server."),
        "{}",
        r.message
    );
    let r = refuse(&mut s, "collab.connect", json!({"url": "ws://example.com/ws"}));
    assert_eq!(r.code, "invalid_url");
    assert!(r.message.contains("unencrypted"), "{}", r.message);
    let r = refuse(&mut s, "collab.connect", json!({"url": "/ws"}));
    assert_eq!(r.code, "invalid_url");
    assert!(r.message.contains("needs the web app"), "{}", r.message);
    let r = refuse(&mut s, "collab.connect", json!({"url": ""}));
    assert_eq!(r.code, "invalid_url");
}

#[test]
fn connect_without_a_room_only_chooses_the_server() {
    let (mut s, fakes) = testing::session();
    // The default is the setting; its value is a loopback address.
    let out = run(&mut s, "collab.connect", json!({}));
    assert_eq!(out["state"], "ready");
    assert_eq!(out["url"], "ws://localhost:3030/ws");
    assert!(fakes.net.servers().is_empty(), "no socket opens before a room is known");
    assert_eq!(run(&mut s, "collab.status", json!({}))["state"], "ready");
    let r = refuse(&mut s, "collab.connect", json!({"url": URL}));
    assert_eq!(r.code, "already_connected");
    assert_eq!(r.message, "Already connected to `ws://localhost:3030/ws`. Disconnect first with `collab.disconnect`.");
    run(&mut s, "collab.disconnect", json!({}));
    assert_eq!(run(&mut s, "collab.status", json!({}))["state"], "disconnected");
    let r = refuse(&mut s, "collab.disconnect", json!({}));
    assert_eq!((r.code, r.message.as_str()), ("not_connected", "Not connected, so there is nothing to disconnect."));
}

#[test]
fn a_path_means_the_server_of_the_page_in_a_web_shell() {
    let (mut s, fakes) = testing::session();
    fakes.net.set_origin("ws://127.0.0.1:8080");
    let out = run(&mut s, "collab.connect", json!({"url": "/ws"}));
    assert_eq!(out["url"], "ws://127.0.0.1:8080/ws");
}

#[test]
fn connect_without_a_net_port_is_port_unavailable() {
    let mut s = testing::headless_session();
    let r = refuse(&mut s, "collab.connect", json!({"url": URL}));
    assert_eq!(r.code, "port_unavailable");
}

#[test]
fn connect_with_a_room_joins_it_and_returns_at_once() {
    let mut w = World::new(1);
    let out = run(w.s(0), "collab.connect", json!({"url": URL, "room": "team-1"}));
    assert_eq!(out["state"], "connecting");
    assert_eq!(out["room"], "team-1");
    assert_eq!(out["link"], "#room=team-1&server=ws:%2F%2F127.0.0.1:3030%2Fws".replace("%2F", "/"));
    assert_eq!(w.s(0).tab().ext.get::<RoomBinding>(), Some(&RoomBinding(Some("team-1".to_owned()))));
    w.settle();
    let v = run(w.s(0), "collab.status", json!({}));
    assert_eq!(v["state"], "connected");
    assert!(v.get("loading").is_none(), "{v}");
    assert!(w.hub.clients() == 1);
}

#[test]
fn a_shell_that_cannot_open_a_connection_makes_join_an_io_failure() {
    let (mut s, fakes) = testing::session();
    run(&mut s, "collab.connect", json!({"url": URL}));
    fakes.net.set_fail_open(true);
    let r = refuse(&mut s, "collab.join_room", json!({"room": "r1"}));
    assert_eq!(r.code, "io_failed");
    fakes.net.set_fail_open(false);
    run(&mut s, "collab.join_room", json!({"room": "r1"}));
}

#[test]
fn connect_with_a_bad_room_name_is_bad_args() {
    let (mut s, _) = testing::session();
    for room in ["", "has space", "dot.dot", &"x".repeat(65)] {
        let r = refuse(&mut s, "collab.connect", json!({"url": URL, "room": room}));
        assert_eq!(r.code, "bad_args", "{room:?}");
    }
}

// ---- collab.join_room and collab.leave_room ----

#[test]
fn join_room_needs_a_server_a_valid_name_and_no_current_room() {
    let mut w = World::new(1);
    let r = w.s(0).availability("collab.join_room").unwrap_err();
    assert_eq!(r.code, "not_connected");
    assert_eq!(r.message, "Not connected to a server. Connect first with `collab.connect`.");
    assert_eq!(refuse(w.s(0), "collab.join_room", json!({})).message, r.message);
    run(w.s(0), "collab.connect", json!({"url": URL}));
    assert_eq!(refuse(w.s(0), "collab.join_room", json!({"room": "no way"})).code, "bad_args");
    let out = run(w.s(0), "collab.join_room", json!({}));
    let room = out["room"].as_str().unwrap().to_owned();
    assert_eq!(room.len(), 10);
    assert!(room.chars().all(|c| c.is_ascii_alphanumeric()));
    assert_eq!(out["link"], format!("#room={room}&server=ws://127.0.0.1:3030/ws"));
    let r = refuse(w.s(0), "collab.join_room", json!({"room": "other"}));
    assert_eq!(r.code, "already_in_room");
    assert_eq!(r.message, format!("Already in room `{room}`. Leave it first with `collab.leave_room`."));
}

#[test]
fn joining_with_content_asks_what_to_do_with_it() {
    let mut w = World::new(2);
    rect(w.s(0), 0.0);
    run(w.s(0), "collab.connect", json!({"url": URL}));
    let r = refuse(w.s(0), "collab.join_room", json!({"room": "room-a"}));
    assert_eq!(r.code, "local_content");
    assert_eq!(
        r.message,
        "This tab already has content, which joining would merge into room `room-a`. Pass on_local=\"merge\" to merge it, on_local=\"discard\" to replace it with the room's content, or open a new tab first with `tab.new`."
    );
    assert!(w.s(0).tab().ext.get::<RoomBinding>().is_none_or(|b| b.0.is_none()));
    // Merge: the content reaches the room and the other peer.
    run(w.s(0), "collab.join_room", json!({"room": "room-a", "on_local": "merge"}));
    w.join(1, "room-a", "refuse");
    w.settle();
    assert_eq!(content(&w.c[1].s).elements.len(), 1);
    assert_eq!(content(&w.c[0].s), content(&w.c[1].s));
}

#[test]
fn discard_replaces_the_local_content_with_the_rooms() {
    let mut w = World::new(2);
    w.join(0, "room-d", "refuse");
    let theirs = rect(w.s(0), 0.0);
    w.settle();
    let mine = rect(w.s(1), 100.0);
    run(w.s(1), "collab.connect", json!({"url": URL}));
    run(w.s(1), "collab.join_room", json!({"room": "room-d", "on_local": "discard"}));
    w.settle();
    assert!(w.s(1).element(&theirs).is_some());
    assert!(w.s(1).element(&mine).is_none(), "the local element is gone");
    assert_eq!(w.s(1).tab().doc.undo_depth(), 0, "discard clears the history");
    assert_eq!(content(&w.c[0].s), content(&w.c[1].s));
}

#[test]
fn leave_room_stops_syncing_and_keeps_the_copy_and_the_server() {
    let mut w = World::new(2);
    w.join_all("room-l");
    let a = rect(w.s(0), 0.0);
    w.settle();
    assert!(w.s(1).element(&a).is_some());
    let r = w.s(1).availability("collab.leave_room");
    assert!(r.is_ok());
    let out = run(w.s(1), "collab.leave_room", json!({}));
    assert_eq!(out["room"], "room-l");
    w.settle();
    assert_eq!(w.hub.clients(), 1);
    assert!(w.s(1).tab().ext.get::<RoomBinding>().is_some_and(|b| b.0.is_none()));
    assert!(w.s(1).element(&a).is_some(), "the local copy stays");
    let r = refuse(w.s(1), "collab.leave_room", json!({}));
    assert_eq!((r.code, r.message.as_str()), ("not_in_room", "This tab is not in a room, so there is nothing to leave."));
    assert_eq!(run(w.s(1), "collab.status", json!({}))["state"], "ready", "the server stays chosen");
    // The other peer no longer sees the leaver, and edits no longer travel.
    assert!(run(w.s(0), "collab.status", json!({}))["peers"].as_array().unwrap().is_empty());
    let b = rect(w.s(0), 50.0);
    w.settle();
    assert!(w.s(1).element(&b).is_none());
    // Joining again works without `collab.connect`.
    run(w.s(1), "collab.join_room", json!({"room": "room-l", "on_local": "merge"}));
    w.settle();
    assert!(w.s(1).element(&b).is_some());
}

#[test]
fn disconnect_leaves_the_room_and_forgets_the_server() {
    let mut w = World::new(2);
    w.join_all("room-x");
    let out = run(w.s(0), "collab.disconnect", json!({}));
    assert_eq!(out["left_room"], "room-x");
    w.settle();
    assert_eq!(w.hub.clients(), 1);
    let v = run(w.s(0), "collab.status", json!({}));
    assert_eq!(v["state"], "disconnected");
    assert!(v.get("room").is_none());
}

#[test]
fn a_room_tab_cannot_be_closed_without_leaving() {
    let mut w = World::new(1);
    w.join_all("room-t");
    run(w.s(0), "tab.new", json!({}));
    let r = refuse(w.s(0), "tab.close", json!({"tab": "t1"}));
    assert_eq!(r.code, "in_room");
    run(w.s(0), "tab.switch", json!({"tab": "t1"}));
    run(w.s(0), "tab.close", json!({"tab": "t1", "leave_room": true, "on_unsaved": "discard"}));
    w.settle();
    assert_eq!(w.hub.clients(), 0);
    assert_eq!(w.s(0).collab_view().state, ConnectionState::Ready);
}

#[test]
fn closing_the_room_tab_behind_the_back_of_the_family_drops_the_client() {
    let mut w = World::new(1);
    w.join_all("room-g");
    run(w.s(0), "tab.new", json!({}));
    w.s(0).tabs.remove(0);
    let p = w.s(0).collab_poll();
    assert!(!p.redraw);
    assert_eq!(w.s(0).collab_view().state, ConnectionState::Disconnected);
}

// ---- editing together ----

#[test]
fn concurrent_edits_made_with_commands_converge() {
    let mut w = World::new(2);
    w.join_all("room-c");
    let a = rect(w.s(0), 0.0);
    let b = rect(w.s(1), 100.0);
    w.settle();
    assert_eq!(content(&w.c[0].s), content(&w.c[1].s));
    assert_eq!(content(&w.c[0].s).elements.len(), 2);
    // No settle between these: all four edits are concurrent.
    style(w.s(0), &a, json!({"fill_color": "#ff0000"}));
    run(w.s(0), "tab.rename", json!({"name": "Plan"}));
    style(w.s(1), &b, json!({"stroke_width": 6}));
    style(w.s(1), &a, json!({"stroke_color": "#00ff00"}));
    w.settle();
    assert_eq!(content(&w.c[0].s), content(&w.c[1].s));
    for i in 0..2 {
        let s = w.s(i);
        let e = s.element(&a).unwrap();
        assert_eq!(e.style.fill_color.map(|c| c.to_hex()).as_deref(), Some("#ff0000"));
        assert_eq!(e.style.stroke_color.to_hex(), "#00ff00");
        assert_eq!(s.element(&b).unwrap().style.stroke_width, 6.0);
        assert_eq!(s.tab().doc.store().meta().name, "Plan");
    }
    let mut server = drafftink_scene::snapshot(w.hub.document());
    for e in &mut server.elements {
        e.version = 0;
    }
    assert_eq!(content(&w.c[0].s), server, "the server holds the same document");
}

#[test]
fn a_remote_edit_marks_the_scene_dirty_logs_one_ring_entry_and_is_not_an_undo_step() {
    let mut w = World::new(2);
    w.join_all("room-r");
    w.s(1).take_dirty();
    let a = rect(w.s(0), 0.0);
    w.settle();
    assert!(w.s(1).element(&a).is_some());
    assert_eq!(w.s(1).take_dirty(), Some(crate::spec::Redraw::Scene));
    assert_eq!(w.s(1).tab().doc.undo_depth(), 0);
    let recent = w.s(1).ring().recent(256, true, None);
    assert!(recent.iter().any(|e| e.id == "remote.apply" && e.source == Origin::Remote), "{recent:?}");
}

#[test]
fn undo_from_the_keyboard_is_sent_and_leaves_the_other_peers_edit_alone() {
    let mut w = World::new(2);
    w.join_all("room-u");
    let a = rect(w.s(0), 0.0);
    w.settle();
    style(w.s(0), &a, json!({"fill_color": "#ff0000"}));
    w.settle();
    assert_eq!(fill_of(&w.c[1].s, &a).as_deref(), Some("#ff0000"));
    // B edits another field of the same element.
    style(w.s(1), &a, json!({"stroke_color": "#00ff00"}));
    w.settle();
    // A undoes from the keyboard (the old app only broadcast undo from the menu).
    w.s(0).execute_from(Origin::Shortcut, "history.undo", json!({})).unwrap();
    w.settle();
    for i in 0..2 {
        let s = w.s(i);
        assert_eq!(fill_of(s, &a), None, "peer {i}: A's step is undone for everybody");
        assert_eq!(s.element(&a).unwrap().style.stroke_color.to_hex(), "#00ff00", "peer {i}: B's edit stays");
    }
    assert_eq!(w.s(1).tab().doc.undo_depth(), 1, "B can still undo its own step, and only that");
    assert_eq!(content(&w.c[0].s), content(&w.c[1].s));
    // Redo travels too.
    w.s(0).execute_from(Origin::Shortcut, "history.redo", json!({})).unwrap();
    w.settle();
    assert_eq!(fill_of(&w.c[1].s, &a).as_deref(), Some("#ff0000"));
    // B's undo takes back B's edit, not A's.
    w.s(1).execute_from(Origin::Ui, "history.undo", json!({})).unwrap();
    w.settle();
    assert_eq!(fill_of(&w.c[0].s, &a).as_deref(), Some("#ff0000"));
    assert_ne!(w.s(0).element(&a).unwrap().style.stroke_color.to_hex(), "#00ff00");
    assert_eq!(content(&w.c[0].s), content(&w.c[1].s));
}

#[test]
fn edits_made_while_not_connected_are_not_lost() {
    let mut w = World::new(2);
    w.join(0, "room-o", "refuse");
    w.settle();
    let a = rect(w.s(1), 0.0);
    w.join(1, "room-o", "merge");
    w.settle();
    assert!(w.s(0).element(&a).is_some());
}

// ---- presence ----

#[test]
fn pointers_names_and_selections_reach_the_other_peer() {
    let mut w = World::new(2);
    w.join_all("room-p");
    run(w.s(0), "collab.set_user", json!({"name": "Ana", "color": "#22c55e"}));
    let a = rect(w.s(0), 0.0);
    w.s(0).tab_mut().observed.selection.insert(a.clone());
    let out = run(w.s(0), "collab.set_pointer", json!({"point": [12.5, -3]}));
    assert_eq!(out["published"], true);
    w.settle();
    let v = w.s(1).collab_view();
    assert_eq!(v.peers.len(), 1);
    let p = &v.peers[0];
    assert_eq!((p.name.as_str(), p.color.as_str()), ("Ana", "#22c55e"));
    assert_eq!(p.pointer, Some([12.5, -3.0]));
    assert_eq!(p.selection, [a.as_str().to_owned()]);
    assert!(p.viewport.is_some());
    // Leaving the canvas clears the remote cursor (the old app never did).
    run(w.s(0), "collab.set_pointer", json!({"point": null}));
    w.settle();
    assert_eq!(w.s(1).collab_view().peers[0].pointer, None);
    assert!(w.s(0).take_collab_notices().iter().any(|n| matches!(n, CollabNotice::Info(t) if t == "Ana joined." || t.ends_with(" joined."))));
}

#[test]
fn a_peer_becomes_a_remote_user_with_pointer_name_selection_and_trail() {
    let mut w = World::new(2);
    w.join_all("room-r");
    run(w.s(0), "collab.set_user", json!({"name": "Ana", "color": "#22c55e"}));
    let a = rect(w.s(0), 0.0);
    w.s(0).tab_mut().observed.selection.insert(a.clone());
    run(w.s(0), "collab.laser", json!({"points": [[1.0, 2.0], [4.0, 6.0]]}));
    run(w.s(0), "collab.set_pointer", json!({"point": [12.5, -3]}));
    w.settle();
    let user = w.s(1).collab_view().peers[0].to_remote_user();
    assert_eq!(user.name.as_deref(), Some("Ana"));
    assert_eq!(user.color, drafftink_scene::Color::rgb(0x22, 0xc5, 0x5e));
    assert_eq!(user.cursor, Some(drafftink_geom::kurbo::Point::new(12.5, -3.0)));
    assert_eq!(user.selection, [a]);
    assert!(!user.trail.is_empty(), "the laser trail comes along");
    // The canvas library turns it into what is drawn: a cursor, a box for the selection and the trail.
    let parts = drafftink_canvas::remote_parts(w.s(1).store(), &[user], 8);
    assert_eq!((parts.cursors.len(), parts.selections[0].boxes.len(), parts.trails.len()), (1, 1, 1));
}

#[test]
fn the_pointer_is_quiet_in_the_ring_and_works_outside_a_room() {
    let (mut s, _) = testing::session();
    let out = run(&mut s, "collab.set_pointer", json!({"point": [1, 2], "tool": "laser"}));
    assert_eq!(out["published"], false);
    assert!(s.ring().recent(16, false, None).iter().all(|e| e.id != "collab.set_pointer"));
    let r = refuse(&mut s, "collab.set_pointer", json!({"point": [1e300, 0]}));
    assert_eq!(r.code, "bad_args");
    assert!(s.ring().recent(16, false, None).iter().any(|e| e.id == "collab.set_pointer"), "a refusal is recorded");
}

#[test]
fn the_laser_fades_locally_and_reaches_peers_as_a_trail() {
    let mut w = World::new(2);
    w.join_all("room-z");
    let out = run(w.s(0), "collab.laser", json!({"points": [[0, 0], [10, 5], [30, 9]], "fade_ms": 200}));
    assert_eq!((out["changed"].as_bool(), out["points"].as_u64()), (Some(true), Some(3)));
    assert_eq!(w.s(0).collab_view().laser.len(), 3);
    let p = w.s(0).collab_poll();
    assert!(p.redraw && p.wake_in_ms.is_some_and(|ms| ms <= 16), "{p:?}");
    w.c[0].fakes.clock.advance_ms(250);
    let p = w.s(0).collab_poll();
    assert!(p.redraw, "one last frame clears the trail");
    assert!(w.s(0).collab_view().laser.is_empty());
    let p = w.s(0).collab_poll();
    assert!(!p.redraw && p.wake_in_ms.is_none_or(|ms| ms > 16));
    let r = refuse(w.s(0), "collab.laser", json!({"points": vec![[0, 0]; 1025]}));
    assert_eq!(r.code, "limit_exceeded");
    assert_eq!(refuse(w.s(0), "collab.laser", json!({"points": [[0, 0]], "fade_ms": 0})).code, "bad_args");
    assert_eq!(run(w.s(0), "collab.laser", json!({"points": []}))["changed"], false);
}

#[test]
fn set_user_stores_the_settings_and_checks_them() {
    let (mut s, _) = testing::session();
    let out = run(&mut s, "collab.set_user", json!({"name": "Ana", "color": "#abc"}));
    assert_eq!((out["name"].as_str(), out["color"].as_str(), out["changed"].as_bool()), (Some("Ana"), Some("#aabbcc"), Some(true)));
    assert_eq!(s.settings.get("collab.user_name"), Some(&json!("Ana")));
    assert_eq!(s.settings.get("collab.user_color"), Some(&json!("#aabbcc")));
    assert_eq!(run(&mut s, "collab.set_user", json!({"name": "Ana"}))["changed"], false);
    assert_eq!(refuse(&mut s, "collab.set_user", json!({"color": "blue"})).code, "bad_args");
    assert_eq!(refuse(&mut s, "collab.set_user", json!({"name": "x".repeat(41)})).code, "bad_args");
}

#[test]
fn following_moves_the_camera_until_the_user_moves_it() {
    let mut w = World::new(2);
    w.join_all("room-f");
    run(w.s(0), "collab.set_user", json!({"name": "Ana"}));
    let r = refuse(w.s(1), "collab.follow", json!({"peer": "nobody"}));
    assert_eq!(r.code, "no_such_peer");
    assert_eq!(r.message, "There is nobody called `nobody` in this room. List the people with `collab.status`.");
    run(w.s(0), "view.set", json!({"scroll": [500, -200], "zoom": 3}));
    w.settle();
    let out = run(w.s(1), "collab.follow", json!({"peer": "ana"}));
    assert_eq!(out["following"], "Ana");
    w.settle();
    let v = *w.s(1).view();
    assert!((v.scroll.x - 500.0).abs() < 1e-6 && (v.scroll.y + 200.0).abs() < 1e-6, "{v:?}");
    assert!((v.zoom - 3.0).abs() < 1e-6, "{v:?}");
    // A is followed on: a new viewport moves B again.
    run(w.s(0), "view.set", json!({"scroll": [0, 0]}));
    w.settle();
    assert!(w.s(1).view().scroll.x.abs() < 1e-6);
    // B moves the view by itself: following ends and A no longer drags the camera.
    run(w.s(1), "view.pan", json!({"dx": 30, "dy": 0}));
    w.settle();
    assert!(w.s(1).collab_view().peers.iter().all(|p| !p.followed));
    assert!(w.s(1).take_collab_notices().iter().any(|n| matches!(n, CollabNotice::Info(t) if t.contains("Stopped following"))));
    let moved = *w.s(1).view();
    run(w.s(0), "view.set", json!({"scroll": [900, 900]}));
    w.settle();
    assert_eq!(*w.s(1).view(), moved);
    // `peer` omitted stops following without error.
    assert_eq!(run(w.s(1), "collab.follow", json!({}))["changed"], false);
}

#[test]
fn follow_needs_a_room() {
    let (mut s, _) = testing::session();
    let r = refuse(&mut s, "collab.follow", json!({}));
    assert_eq!(r.code, "not_in_room");
}

// ---- loading ----

#[test]
fn a_big_catch_up_shows_a_loading_state_before_it_blocks() {
    let mut w = World::new(2);
    w.join(0, "room-big", "refuse");
    w.settle();
    // 4000 elements in one commit make an update far above the hold threshold.
    let ids: Vec<ElementId> = (0..4000).map(|_| w.s(0).ports.ids.element_id()).collect();
    let n = ids.len();
    {
        use drafftink_scene::{Element, ElementKind, SceneTx, StoreError, Transform};
        let s = w.s(0);
        s.tab_mut()
            .doc
            .edit::<(), StoreError>(|tx| {
                for (i, id) in ids.into_iter().enumerate() {
                    let z = tx.next_top_index().map_err(StoreError::backend)?;
                    let mut e = Element::new(id, ElementKind::Rectangle, z);
                    e.transform = Transform::new(i as f64, 0.0, 10.0, 10.0);
                    tx.insert(e)?;
                }
                Ok(())
            })
            .unwrap();
        s.tab_mut().doc.commit_untracked().unwrap();
    }
    w.settle();
    assert_eq!(w.hub.document().iter().count(), n, "the server has the big document");
    // B joins. Poll by hand to see the phases.
    run(w.s(1), "collab.connect", json!({"url": URL, "room": "room-big"}));
    assert_eq!(w.s(1).collab_view().loading, Some(Loading::Connecting));
    let mut seen_receiving = None;
    for _ in 0..40 {
        w.c[1].fakes.clock.advance_ms(40);
        let p = w.s(1).collab_poll();
        w.hub.attach(&w.c[1].fakes.net);
        w.hub.step();
        if let Some(Loading::Receiving { bytes }) = w.s(1).collab_view().loading {
            assert!(p.redraw && p.wake_in_ms.is_some(), "{p:?}");
            assert_eq!(w.s(1).store().iter().count(), 0, "nothing is applied yet: the UI can draw first");
            seen_receiving = Some(bytes);
            w.c[1].fakes.clock.advance_ms(20);
            w.s(1).collab_poll();
            break;
        }
    }
    assert!(seen_receiving.is_some_and(|b| b >= 256 * 1024), "{seen_receiving:?}");
    assert_eq!(w.s(1).store().iter().count(), n, "the next poll applied it");
    for _ in 0..10 {
        w.c[1].fakes.clock.advance_ms(40);
        w.s(1).collab_poll();
    }
    assert_eq!(w.s(1).collab_view().loading, None);
    assert_eq!(w.s(1).collab_view().state, ConnectionState::Connected);
}

// ---- the room link ----

#[test]
fn room_links_follow_the_old_apps_forms() {
    let l = RoomLink::parse("https://drafft.ink/?room=abc123&server=localhost:3030");
    assert_eq!((l.room.as_deref(), l.server.as_deref()), (Some("abc123"), Some("ws://localhost:3030")));
    let l = RoomLink::parse("https://drafft.ink/#room=r_1&server=wss%3A%2F%2Fhost%2Fws");
    assert_eq!((l.room.as_deref(), l.server.as_deref()), (Some("r_1"), Some("wss://host/ws")));
    let l = RoomLink::parse("?room=q#room=h&server=ws://a:1/ws");
    assert_eq!(l.room.as_deref(), Some("q"), "the query wins over the fragment");
    assert_eq!(l.server.as_deref(), Some("ws://a:1/ws"));
    assert_eq!(RoomLink::parse("https://drafft.ink/"), RoomLink::default());
    assert_eq!(RoomLink::parse("#room=&server=").room, None);
}

// ---- status ----

#[test]
fn status_lists_the_people_and_reads_only() {
    let mut w = World::new(2);
    w.join_all("room-s");
    let before = w.s(0).fingerprint();
    let v = run(w.s(0), "collab.status", json!({}));
    assert_eq!(w.s(0).fingerprint(), before);
    assert_eq!(v["state"], "connected");
    assert_eq!(v["room"], "room-s");
    assert_eq!(v["tab"], "t1");
    assert_eq!(v["peers"].as_array().unwrap().len(), 1);
    assert_eq!(w.s(0).state_json()["collab"], json!({"state": "connected", "room": "room-s"}));
    let (idle, _) = testing::session();
    assert_eq!(idle.state_json()["collab"], json!({"state": "disconnected", "room": null}));
}
