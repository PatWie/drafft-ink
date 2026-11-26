//! A web page that serves its own relay: the page's server (`/ws`) is the default, the sealed endpoint and the
//! picture address come from it, the invite link opens the same page, and a bare room id is not a plain room.

use serde_json::json;

use super::sealed_tests::{World, elements, refuse, reload, run};
use crate::cmd::collab::CollabNotice;
use crate::cmd::tab::SealedBinding;
use crate::ports::Ns;
use crate::testing;

const PAGE: &str = "wss://canvas.example.com";

/// One session in a web shell on `canvas.example.com` whose setting is the platform default, `/ws`.
fn web_world() -> World {
    let mut w = World::new(1);
    w.c[0].1.net.set_origin(PAGE);
    w.c[0].0.load_settings().unwrap();
    w.relay.hold_blobs(true);
    w
}

#[test]
fn a_room_started_from_the_default_server_talks_to_the_page_not_to_localhost() {
    let mut w = web_world();
    let out = run(w.s(0), "collab.connect", json!({}));
    assert_eq!(out["url"], "wss://canvas.example.com/ws", "the relative default is the page's server");
    let room = run(w.s(0), "collab.join_room", json!({}))["room"].as_str().unwrap().to_owned();
    w.settle();
    let socket = w.c[0].1.net.last_server().expect("a socket was opened").last_url().expect("with an address");
    assert_eq!(socket, format!("wss://canvas.example.com/r/{room}/ws"), "the path is the room's, the host the page's");
    let urls: Vec<String> = w.c[0].1.net.blob_servers().iter().flat_map(|b| b.take_requests()).map(|r| r.url).collect();
    assert!(!urls.is_empty(), "the client asked for the pictures of the room");
    let want = format!("https://canvas.example.com/r/{room}/f");
    assert!(urls.iter().all(|u| u.starts_with(&want)), "pictures come from the page's https origin: {urls:?}");
}

#[test]
fn the_invite_link_of_a_room_on_the_page_opens_the_page_and_names_no_server() {
    let mut w = web_world();
    run(w.s(0), "collab.connect", json!({}));
    run(w.s(0), "collab.join_room", json!({}));
    w.settle();
    let link = run(w.s(0), "collab.share_link", json!({"copy": false, "reveal": true}))["link"].as_str().unwrap().to_owned();
    assert!(link.starts_with("https://canvas.example.com/#room="), "{link}");
    assert!(!link.contains("&server="), "the page is the server: {link}");
    assert!(!link.contains("drafft.ink") && !link.contains("localhost"), "{link}");
}

#[test]
fn a_second_browser_opening_the_link_joins_the_same_room_on_the_same_server() {
    let mut w = World::new(2);
    for i in 0..2 {
        w.c[i].1.net.set_origin(PAGE);
        w.c[i].0.load_settings().unwrap();
    }
    run(w.s(0), "collab.connect", json!({}));
    run(w.s(0), "collab.join_room", json!({}));
    w.settle();
    let link = run(w.s(0), "collab.share_link", json!({"copy": false, "reveal": true}))["link"].as_str().unwrap().to_owned();
    run(w.s(1), "collab.connect", json!({}));
    run(w.s(1), "collab.join_room", json!({"room": link}));
    w.settle();
    let a = run(w.s(0), "collab.status", json!({}));
    let b = run(w.s(1), "collab.status", json!({}));
    assert_eq!((a["room"].clone(), a["state"].clone()), (b["room"].clone(), json!("connected")));
    assert_eq!(b["state"], "connected");
    assert_eq!(a["peers"].as_array().map(Vec::len), Some(1));
}

#[test]
fn a_bare_room_id_is_refused_in_plain_words_and_nothing_is_joined() {
    let mut w = web_world();
    run(w.s(0), "collab.connect", json!({}));
    let id = run(w.s(0), "collab.join_room", json!({}))["room"].as_str().unwrap().to_owned();
    run(w.s(0), "collab.leave_room", json!({}));
    let words = "This is a private room. Paste the whole invite link (it contains the key after the comma).";
    let said = refuse(w.s(0), "collab.join_room", json!({"room": id}));
    assert_eq!((said.code, said.message.as_str()), ("bare_room_id", words));
    // `collab.connect` with the room reads it the same way (a session that has not chosen a server yet).
    let mut fresh = web_world();
    let said = refuse(fresh.s(0), "collab.connect", json!({"room": id}));
    assert_eq!((said.code, said.message.as_str()), ("bare_room_id", words));
    let status = run(w.s(0), "collab.status", json!({}));
    assert!(status.get("room").is_none_or(|r| r.is_null()), "no plain room was started: {status}");
}

// ---- a site without rooms, and a room on another server ----

/// A room tab that a page with rooms saved: its key is in the workspace record.
fn saved_room_tab(w: &mut World) -> String {
    let link = w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 100.0, 60.0).unwrap();
    w.settle();
    w.s(0).save_workspace().unwrap();
    link
}

fn index_of(w: &World) -> String {
    String::from_utf8(w.c[0].1.storage.bytes(Ns::Autosave, "workspace").unwrap()).unwrap()
}

fn key_of(link: &str) -> String {
    link.split_once(',').unwrap().1.split('&').next().unwrap().to_owned()
}

#[test]
fn a_saved_room_tab_comes_back_local_on_a_site_without_rooms_and_nothing_connects() {
    let mut w = World::new(1);
    let link = saved_room_tab(&mut w);
    assert!(index_of(&w).contains(&key_of(&link)), "the page that had rooms saved the key");
    let connections = w.c[0].1.net.servers().len();

    // The same storage, opened by a page that has no relay.
    w.c[0].1.net.set_rooms_available(false);
    let mut s = reload(&w.c[0].1);
    let report = s.restore_workspace().unwrap();
    assert_eq!((report.restored, report.rooms_dropped), (1, 1));
    assert!(s.tab().ext.get::<SealedBinding>().is_none_or(|b| b.0.is_none()), "a plain local copy: no key is kept");
    assert_eq!(elements(&s), 1, "the drawing is there");
    w.c[0].0 = s;
    w.settle();
    assert_eq!(w.c[0].1.net.servers().len(), connections, "no connection was attempted");
    assert_eq!(run(w.s(0), "collab.status", json!({}))["state"], "disconnected");

    // The record is written again without the key, without the person doing anything.
    for _ in 0..8 {
        w.c[0].1.clock.advance_ms(1000);
        w.s(0).workspace_tick();
    }
    let index = index_of(&w);
    assert!(!index.contains(&key_of(&link)) && !index.contains("\"room\""), "the key is dropped on the next save: {index}");
    assert!(w.c[0].1.net.servers().len() == connections, "saving connects to nothing");
}

#[test]
fn a_record_without_rooms_is_not_written_again_for_nothing() {
    let (mut s, fakes) = testing::session();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    s.save_workspace().unwrap();
    fakes.net.set_rooms_available(false);
    let mut again = reload(&fakes);
    assert_eq!(again.restore_workspace().unwrap().rooms_dropped, 0);
    fakes.clock.advance_ms(60_000);
    assert!(!again.workspace_tick(), "nothing changed, so nothing is written");
}

#[test]
fn every_room_command_refuses_in_plain_words_on_a_site_without_rooms() {
    let mut w = World::new(1);
    w.c[0].1.net.set_rooms_available(false);
    for (id, args) in [
        ("collab.connect", json!({ "url": "wss://canvas.example.com/ws" })),
        ("collab.disconnect", json!({})),
        ("collab.join_room", json!({})),
        ("collab.leave_room", json!({})),
        ("collab.resume_room", json!({})),
        ("collab.share_link", json!({})),
        ("collab.delete_room", json!({ "confirm": true })),
    ] {
        let said = refuse(w.s(0), id, args);
        assert_eq!((said.code, said.message.as_str()), ("rooms_unavailable", "Rooms are not available on this site."), "{id}");
    }
    assert_eq!(w.c[0].1.net.servers().len(), 0);
    // Reading the status and naming yourself still work: nothing there needs a room.
    run(w.s(0), "collab.status", json!({}));
    run(w.s(0), "collab.set_user", json!({ "name": "Ana" }));
}

#[test]
fn a_room_on_another_server_than_the_page_waits_for_the_person_and_connects_when_asked() {
    let mut w = World::new(1);
    saved_room_tab(&mut w);
    // The page is canvas.example.com; the room is on 127.0.0.1:3030.
    w.c[0].1.net.set_origin(PAGE);
    let connections = w.c[0].1.net.servers().len();
    let mut s = reload(&w.c[0].1);
    s.restore_workspace().unwrap();
    w.c[0].0 = s;
    w.settle();
    assert_eq!(w.c[0].1.net.servers().len(), connections, "nothing connected by itself");
    assert!(w.s(0).tab().ext.get::<SealedBinding>().is_some_and(|b| b.0.is_some()), "the tab is still the room's copy");
    let view = run(w.s(0), "collab.status", json!({}));
    assert_eq!((view["state"].clone(), view["reconnect"].clone()), (json!("disconnected"), json!("http://127.0.0.1:3030")));
    let told: Vec<String> = w
        .s(0)
        .take_collab_notices()
        .into_iter()
        .filter_map(|n| if let CollabNotice::Info(t) = n { Some(t) } else { None })
        .collect();
    assert!(told.iter().any(|t| t.contains("http://127.0.0.1:3030") && t.contains("not this page's server")), "{told:?}");

    let out = run(w.s(0), "collab.resume_room", json!({}));
    assert_eq!(out["url"], "ws://127.0.0.1:3030/ws");
    w.settle();
    let view = run(w.s(0), "collab.status", json!({}));
    assert_eq!(view["state"], "connected", "{view}");
    assert!(view.get("reconnect").is_none(), "{view}");
    assert_eq!(elements(w.s(0)), 1);
    let again = refuse(w.s(0), "collab.resume_room", json!({}));
    assert_eq!(again.code, "already_in_room");
}

#[test]
fn a_room_on_the_pages_own_server_comes_back_by_itself() {
    let mut w = web_world();
    run(w.s(0), "collab.connect", json!({}));
    run(w.s(0), "collab.join_room", json!({}));
    w.settle();
    testing::add_rect(w.s(0), 0.0, 0.0, 100.0, 60.0).unwrap();
    w.settle();
    w.s(0).save_workspace().unwrap();
    let mut s = reload(&w.c[0].1);
    s.restore_workspace().unwrap();
    w.c[0].0 = s;
    w.settle();
    let view = run(w.s(0), "collab.status", json!({}));
    assert_eq!(view["state"], "connected", "the page's own server is no stranger: {view}");
    assert!(view.get("reconnect").is_none());
}

#[test]
fn a_room_counts_as_joined_only_after_it_has_caught_up() {
    let mut w = World::new(1);
    run(w.s(0), "collab.connect", json!({ "url": super::sealed_tests::URL }));
    assert_eq!(w.s(0).joined_room_id(), None, "no room yet");
    run(w.s(0), "collab.join_room", json!({}));
    let id = w.s(0).sealed_room_id().expect("the tab is bound to the room it is connecting to");
    assert_eq!(w.s(0).joined_room_id(), None, "connecting is not joined");
    w.settle();
    assert_eq!(w.s(0).joined_room_id(), Some(id), "joined");
    run(w.s(0), "collab.leave_room", json!({}));
    assert_eq!(w.s(0).joined_room_id(), None);
}
