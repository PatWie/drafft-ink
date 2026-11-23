//! A web page that serves its own relay: the page's server (`/ws`) is the default, the sealed endpoint and the
//! picture address come from it, the invite link opens the same page, and a bare room id is not a plain room.

use serde_json::json;

use super::sealed_tests::{World, refuse, run};

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
