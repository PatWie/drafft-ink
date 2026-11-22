//! Tests of the private instance in the `collab` family: the service token (`collab.set_access`,
//! `collab.clear_access`, `collab.status`) and `collab.delete_room`. Sessions talk through in-memory loopbacks to a
//! [`FakeRelay`], which is the real relay core.

use drafftink_collab::{AccessOrigin, ProblemAction};
use drafftink_relay::RelayConfig;
use serde_json::{Value, json};

use super::CollabNotice;
use super::sealed_tests::{URL, World, elements, refuse, reload, run};
use crate::batch::{BatchOpts, BatchStep};
use crate::cmd::tab::{RoomBinding, SealedBinding};
use crate::ports::Ns;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

const ID: &str = "svc-id-0f3a.access";
const SECRET: &str = "svc-secret-9c41-NEVER-SHOW-ME";

fn origin() -> AccessOrigin {
    AccessOrigin::parse("http://127.0.0.1:3030").unwrap()
}

fn set(s: &mut Session) -> Value {
    run(s, "collab.set_access", json!({"client_id": ID, "client_secret": SECRET}))
}

/// Everything the session says about itself, as one text: outputs, the ring, the status, the settings.
fn everything_said(w: &mut World, outputs: &[Value]) -> String {
    let mut seen = outputs.to_vec();
    seen.push(run(w.s(0), "collab.status", json!({})));
    seen.push(run(w.s(0), "engine.recent", json!({"limit": 200})));
    seen.push(run(w.s(0), "settings.get", json!({})));
    seen.push(run(w.s(0), "engine.describe", json!({"id": "collab.set_access"})));
    let ring = serde_json::to_string(&w.s(0).ring().recent(1000, false, None)).unwrap();
    let notices = format!("{:?}", w.s(0).take_collab_notices());
    format!("{}{ring}{notices}{:?}", serde_json::to_string(&seen).unwrap(), w.s(0).collab_view())
}

// ---- the service token ----

#[test]
fn a_token_is_stored_for_the_chosen_server_and_the_status_says_only_that_there_is_one() {
    let mut w = World::new(1);
    run(w.s(0), "collab.connect", json!({"url": URL}));
    assert_eq!(run(w.s(0), "collab.status", json!({}))["access"], "none");
    let first = set(w.s(0));
    assert_eq!((first["changed"].clone(), first["server"].clone()), (json!(true), json!("http://127.0.0.1:3030")));
    assert_eq!(first["access"], "service_token");
    assert_eq!(w.c[0].1.secrets.stored(&origin()).map(|c| c.expose_secret().to_owned()), Some(SECRET.to_owned()));
    assert_eq!(run(w.s(0), "collab.status", json!({}))["access"], "service_token");
    assert_eq!(set(w.s(0))["changed"], false, "the same token again changes nothing");
    let again = run(w.s(0), "collab.set_access", json!({"client_id": ID, "client_secret": "another-secret-VALUE"}));
    assert_eq!(again["changed"], true);
    set(w.s(0));
    let cleared = run(w.s(0), "collab.clear_access", json!({}));
    assert_eq!((cleared["changed"].clone(), cleared["access"].clone()), (json!(true), json!("none")));
    assert!(w.c[0].1.secrets.is_empty());
    assert_eq!(run(w.s(0), "collab.status", json!({}))["access"], "none");
    let r = refuse(w.s(0), "collab.clear_access", json!({}));
    assert_eq!(r.code, "not_found");
    assert_eq!(r.message, "No access token is stored for http://127.0.0.1:3030, so there is nothing to remove.");
}

#[test]
fn without_a_chosen_server_the_setting_decides_and_a_server_argument_overrides_it() {
    let mut w = World::new(1);
    let out = set(w.s(0));
    assert_eq!(out["server"], "http://localhost:3030", "the default of the setting collab.server_url");
    for server in ["https://canvas.example.com", "wss://canvas.example.com/ws", "https://CANVAS.example.com:443"] {
        let out = run(w.s(0), "collab.set_access", json!({"server": server, "client_id": ID, "client_secret": SECRET}));
        assert_eq!(out["server"], "https://canvas.example.com", "{server}");
    }
    assert_eq!(w.c[0].1.secrets.len(), 2, "one token per origin");
    let out = run(w.s(0), "collab.clear_access", json!({"server": "wss://canvas.example.com/ws"}));
    assert_eq!(out["server"], "https://canvas.example.com");
    assert_eq!(w.c[0].1.secrets.len(), 1);
}

#[test]
fn a_value_that_does_not_fit_is_refused_without_being_repeated() {
    let mut w = World::new(1);
    let long = "x".repeat(5000);
    let cases = [
        json!({"client_id": "", "client_secret": SECRET}),
        json!({"client_id": ID, "client_secret": ""}),
        json!({"client_id": ID, "client_secret": long}),
        json!({"client_id": long, "client_secret": SECRET}),
        json!({"client_id": ID, "client_secret": "has a space-NEVER-SHOW-ME"}),
        json!({"client_id": ID, "client_secret": "line\nbreak-NEVER-SHOW-ME"}),
        json!({"client_id": ID, "client_secret": "naïve-NEVER-SHOW-ME"}),
        json!({"client_id": ID, "client_secret": 123_456_789}),
        json!({"client_id": ID, "client_secret": ["a-NEVER-SHOW-ME"]}),
        json!({"client_id": ID}),
        json!({"client_secret": SECRET}),
        json!({"client_id": ID, "client_secret": SECRET, "extra": "NEVER-SHOW-ME"}),
    ];
    for args in cases {
        let r = refuse(w.s(0), "collab.set_access", args.clone());
        assert_eq!(r.code, "bad_args", "{args}: {}", r.message);
        for secret in [SECRET, "NEVER-SHOW-ME", "123456789", &"x".repeat(100)] {
            assert!(!r.message.contains(secret), "the refusal repeats a value: {}", r.message);
        }
    }
    assert!(w.c[0].1.secrets.is_empty(), "nothing was stored");
    // A server that cannot be an origin.
    for server in ["", "canvas.example.com", "ftp://x", "https://u:p@host", "https://host/path", "ws://example.com/ws"] {
        let r = refuse(w.s(0), "collab.set_access", json!({"server": server, "client_id": ID, "client_secret": SECRET}));
        assert_eq!(r.code, "invalid_url", "{server}");
        assert!(!r.message.contains(SECRET));
    }
    assert!(w.c[0].1.secrets.is_empty());
}

#[test]
fn the_token_is_in_no_output_no_ring_entry_no_status_no_notice_and_no_refusal() {
    let mut w = World::new(1);
    run(w.s(0), "collab.connect", json!({"url": URL}));
    let mut seen = vec![set(w.s(0))];
    seen.push(run(w.s(0), "collab.clear_access", json!({})));
    seen.push(set(w.s(0)));
    // A refusal, a call in a batch, and a failing store.
    for args in [json!({"client_id": ID, "client_secret": ""}), json!({"client_id": "", "client_secret": SECRET})] {
        let r = refuse(w.s(0), "collab.set_access", args);
        seen.push(json!(r.message));
    }
    let step = BatchStep::new("collab.set_access", json!({"client_id": ID, "client_secret": SECRET}));
    let batch = w.s(0).execute_batch(Origin::Test, &[step], &BatchOpts::default());
    seen.push(batch.to_json());
    w.c[0].1.secrets.fail_calls(true);
    let r = refuse(w.s(0), "collab.set_access", json!({"client_id": ID, "client_secret": SECRET}));
    seen.push(json!(r.message));
    w.c[0].1.secrets.fail_calls(false);
    let text = everything_said(&mut w, &seen);
    assert!(!text.contains(SECRET), "the secret appears: {text}");
    assert!(!text.contains(ID), "the client id appears: {text}");
    let ring = serde_json::to_string(&w.s(0).ring().recent(1000, false, None)).unwrap();
    assert!(ring.contains("collab.set_access") && ring.contains("<hidden>"), "{ring}");
    // Debug of the pieces too.
    let args: super::access::SetAccessArgs =
        serde_json::from_value(json!({"client_id": ID, "client_secret": SECRET})).unwrap();
    assert!(!format!("{args:?} {}", serde_json::to_string(&args).unwrap()).contains(SECRET));
}

#[test]
fn a_token_cannot_be_part_of_a_batch_and_is_not_for_agents() {
    let mut w = World::new(1);
    let step = BatchStep::new("collab.set_access", json!({"client_id": ID, "client_secret": SECRET}));
    let out = w.s(0).execute_batch(Origin::Test, &[step], &BatchOpts::default());
    let failed = out.failed.expect("the batch must fail");
    assert_eq!(failed.refusal.code, "not_batchable", "{failed:?}");
    assert!(w.c[0].1.secrets.is_empty());
    for id in ["collab.set_access", "collab.clear_access", "collab.delete_room"] {
        let spec = w.s(0).registry().find(id).expect("registered");
        assert!(!spec.agent && !spec.palette, "{id}");
    }
}

#[test]
fn without_a_store_the_commands_say_so_and_the_status_says_browser_in_the_web_app() {
    let mut s = testing::headless_session();
    for (id, args) in [
        ("collab.set_access", json!({"client_id": ID, "client_secret": SECRET})),
        ("collab.clear_access", json!({})),
    ] {
        let r = refuse(&mut s, id, args);
        assert_eq!(r.code, "port_unavailable", "{id}");
        assert!(r.message.contains("browser") && !r.message.contains(SECRET), "{}", r.message);
    }
    assert_eq!(run(&mut s, "collab.status", json!({}))["access"], "none");
    // A web shell has a page server and no store.
    let fakes = crate::ports::fake::Fakes::new();
    fakes.net.set_origin("wss://canvas.example.com");
    let mut ports = fakes.ports();
    ports.secrets = None;
    let mut web = Session::with_services(ports, crate::services::Services::fake());
    assert_eq!(run(&mut web, "collab.status", json!({}))["access"], "browser");
}

#[test]
fn the_token_reaches_the_transport_of_its_own_server_and_of_no_other() {
    let mut w = World::new(3);
    // 0: a token for the server it joins. 1: a token for another server. 2: none.
    run(w.s(0), "collab.connect", json!({"url": URL}));
    set(w.s(0));
    run(w.s(1), "collab.connect", json!({"url": URL}));
    run(w.s(1), "collab.set_access", json!({"server": "https://other.example.com", "client_id": ID, "client_secret": SECRET}));
    run(w.s(2), "collab.connect", json!({"url": URL}));
    for i in 0..3 {
        run(w.s(i), "collab.join_room", json!({}));
    }
    w.settle();
    let sent: Vec<Vec<Option<String>>> = w
        .c
        .iter()
        .map(|(_, f)| f.net.servers().iter().flat_map(|server| server.access_client_ids()).collect())
        .collect();
    assert_eq!(sent[0], [Some(ID.to_owned())]);
    assert_eq!(sent[1], [None]);
    assert_eq!(sent[2], [None]);
    // The picture transport of the same room is given the same token.
    let blob_ids: Vec<Option<String>> = w.c[0].1.net.blob_servers().iter().flat_map(|b| b.access_client_ids()).collect();
    assert!(blob_ids.iter().all(|id| id.as_deref() == Some(ID)), "{blob_ids:?}");
}

#[test]
fn a_token_the_program_started_with_is_for_the_server_of_the_setting_only() {
    let mut w = World::new(2);
    let ambient = drafftink_collab::AccessCredentials::new(ID, SECRET).unwrap();
    for (_, f) in &w.c {
        f.secrets.set_ambient(Some(ambient.clone()));
    }
    // The default setting is the server of both sessions.
    run(w.s(0), "settings.set", json!({"key": "collab.server_url", "value": URL}));
    assert_eq!(run(w.s(0), "collab.status", json!({}))["access"], "service_token");
    run(w.s(0), "collab.connect", json!({"url": URL}));
    run(w.s(0), "collab.join_room", json!({}));
    // The second session names another server by hand: the token is not for it.
    run(w.s(1), "settings.set", json!({"key": "collab.server_url", "value": URL}));
    run(w.s(1), "collab.connect", json!({"url": "ws://localhost:3030/ws"}));
    assert_eq!(run(w.s(1), "collab.status", json!({}))["access"], "none");
    run(w.s(1), "collab.join_room", json!({}));
    w.settle();
    let sent = |w: &World, i: usize| -> Vec<Option<String>> {
        w.c[i].1.net.servers().iter().flat_map(|s| s.access_client_ids()).collect()
    };
    assert_eq!(sent(&w, 0), [Some(ID.to_owned())]);
    assert_eq!(sent(&w, 1), [None]);
    // Take it away and the status says so.
    w.c[0].1.secrets.set_ambient(None);
    assert_eq!(run(w.s(0), "collab.status", json!({}))["access"], "none");
}

#[test]
fn a_store_that_cannot_be_read_means_no_token_is_sent_and_the_person_is_told() {
    let mut w = World::new(1);
    run(w.s(0), "collab.connect", json!({"url": URL}));
    set(w.s(0));
    w.c[0].1.secrets.fail_calls(true);
    run(w.s(0), "collab.join_room", json!({}));
    w.settle();
    let sent: Vec<Option<String>> = w.c[0].1.net.servers().iter().flat_map(|s| s.access_client_ids()).collect();
    assert_eq!(sent, [None]);
    let notices = w.s(0).take_collab_notices();
    assert!(
        notices.iter().any(|n| matches!(n, CollabNotice::Info(t) if t.starts_with("The stored access token could not be read, so none is sent"))),
        "{notices:?}"
    );
    assert!(!format!("{notices:?}").contains(SECRET));
}

#[test]
fn hostile_arguments_never_panic_and_never_store_anything() {
    let mut w = World::new(1);
    let long = "y".repeat(5000);
    let deep = (0..60).fold(json!(1), |acc, _| json!([acc]));
    let values = [
        json!(null),
        json!(true),
        json!(-1),
        json!(1.5e300),
        json!(""),
        json!(long),
        json!("\u{0}\u{7f}\u{202e}"),
        json!([]),
        json!({}),
        json!(deep),
        json!({"a": {"b": [null]}}),
    ];
    for id in ["collab.set_access", "collab.clear_access", "collab.delete_room"] {
        for v in &values {
            for args in [
                v.clone(),
                json!({"client_id": v, "client_secret": v}),
                json!({"client_id": ID, "client_secret": v}),
                json!({"server": v, "client_id": ID, "client_secret": SECRET}),
                json!({"server": v}),
                json!({"confirm": v}),
            ] {
                let before = w.s(0).fingerprint();
                match w.s(0).execute(id, args.clone()) {
                    Ok(_) => {}
                    Err(r) => {
                        assert_ne!(r.code, "internal", "{id} on {args}: {}", r.message);
                        assert!(r.message.chars().count() < 1500 && !r.message.contains(SECRET), "{id}: {}", r.message);
                        assert_eq!(w.s(0).fingerprint(), before, "{id} refused {args} but changed state");
                    }
                }
            }
        }
    }
}

// ---- collab.delete_room ----

fn deleting() -> World {
    World::with_config(2, RelayConfig { allow_delete_room: true, ..RelayConfig::self_host() })
}

#[test]
fn deleting_needs_a_room_a_key_and_the_word_confirm_and_a_connection() {
    let mut w = World::new(1);
    let r = refuse(w.s(0), "collab.delete_room", json!({"confirm": true}));
    assert_eq!(r.code, "not_in_room");
    assert_eq!(r.message, "This tab is not in a room, so there is no room to delete.");
    assert_eq!(w.s(0).availability("collab.delete_room").unwrap_err().code, "not_in_room");
    run(w.s(0), "collab.connect", json!({"url": URL}));
    run(w.s(0), "collab.join_room", json!({}));
    // Not joined yet: the relay takes the request only from a member that is online.
    let r = refuse(w.s(0), "collab.delete_room", json!({"confirm": true}));
    assert_eq!(r.code, "room_offline", "{}", r.message);
    w.settle();
    for args in [json!({}), json!({"confirm": false})] {
        let r = refuse(w.s(0), "collab.delete_room", args);
        assert_eq!(r.code, "not_confirmed");
        assert_eq!(
            r.message,
            "This deletes the room and all its pictures for everyone. Run it again with confirm=true."
        );
    }
    let status = run(w.s(0), "collab.status", json!({}));
    assert_eq!((status["state"].clone(), status["room_deleted"].clone()), (json!("connected"), json!(false)));
    assert_eq!(w.relay.members(), 1, "nothing was sent");
}

#[test]
fn a_host_that_allows_it_deletes_the_room_and_the_tab_becomes_a_local_copy() {
    let mut w = deleting();
    let link = w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 100.0, 60.0).unwrap();
    w.join(1, &link, json!({}));
    w.settle();
    assert_eq!(elements(w.s(1)), 1);
    w.s(0).take_collab_notices();
    w.s(0).save_workspace().unwrap();
    let saved = |w: &World| -> Value {
        serde_json::from_slice(&w.c[0].1.storage.bytes(Ns::Autosave, "workspace").unwrap()).unwrap()
    };
    assert!(saved(&w)["tabs"][0]["room"]["link"].is_string(), "the room is in the workspace record");
    assert!(!w.relay.store().dump().is_empty());

    let out = run(w.s(0), "collab.delete_room", json!({"confirm": true}));
    assert_eq!((out["requested"].clone(), out["changed"].clone()), (json!(true), json!(false)));
    assert_eq!(out["room"].as_str().map(str::len), Some(22), "the id, never the key");
    w.settle();

    // The tab is a local copy: the drawing stays, the room and its key are gone.
    assert_eq!(elements(w.s(0)), 1);
    assert!(w.s(0).tab().ext.get::<RoomBinding>().is_none_or(|b| b.0.is_none()));
    assert!(w.s(0).tab().ext.get::<SealedBinding>().is_none_or(|b| b.0.is_none()));
    let r = refuse(w.s(0), "collab.share_link", json!({"copy": false, "reveal": true}));
    assert_eq!(r.code, "not_in_room");
    // The status says what happened, and the server stays chosen for the next room.
    let status = run(w.s(0), "collab.status", json!({}));
    assert_eq!(status["room_deleted"], true, "{status}");
    assert_eq!(status["state"], "ready", "{status}");
    assert_eq!(status["text"], "The room was deleted for everyone. This tab is a local copy now.");
    assert!(status.get("room").is_none(), "{status}");
    let notices = w.s(0).take_collab_notices();
    assert!(
        notices.iter().any(|n| matches!(n, CollabNotice::Info(t) if t == "The room was deleted for everyone. This tab is a local copy now.")),
        "{notices:?}"
    );
    // The saved record no longer has the room link, and the relay holds nothing.
    w.s(0).save_workspace().unwrap();
    let index = saved(&w);
    assert!(index["tabs"][0].get("room").is_none_or(Value::is_null), "{index}");
    assert!(!serde_json::to_string(&index).unwrap().contains(link.split_once(',').unwrap().1));
    assert!(w.relay.store().dump().is_empty(), "the relay forgot the room");
    // The other member finds the room gone, in the usual words.
    let other = run(w.s(1), "collab.status", json!({}));
    assert!(other["text"].as_str().unwrap().contains("This room is gone"), "{other}");
    // The same session can make a new room, and the status forgets the deletion.
    run(w.s(0), "collab.join_room", json!({"on_local": "merge"}));
    w.settle();
    assert_eq!(run(w.s(0), "collab.status", json!({}))["room_deleted"], false);
}

#[test]
fn a_host_that_does_not_allow_it_says_so_and_nothing_changes() {
    let mut w = World::new(2);
    let link = w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 100.0, 60.0).unwrap();
    w.join(1, &link, json!({}));
    w.settle();
    w.s(0).take_collab_notices();
    let out = run(w.s(0), "collab.delete_room", json!({"confirm": true}));
    assert_eq!(out["requested"], true);
    w.settle();
    let notices = w.s(0).take_collab_notices();
    let problems: Vec<_> = notices
        .iter()
        .filter_map(|n| match n {
            CollabNotice::Problem(p) => Some((p.text.as_str(), p.action, p.fatal)),
            CollabNotice::Info(_) => None,
        })
        .collect();
    assert_eq!(problems, [("This server does not allow deleting rooms.", ProblemAction::AskAdmin, false)]);
    // The room is as it was: same tab, same key, connected again, drawing intact, the other member still there.
    let status = run(w.s(0), "collab.status", json!({}));
    assert_eq!((status["state"].clone(), status["room_deleted"].clone()), (json!("connected"), json!(false)), "{status}");
    assert_eq!(run(w.s(0), "collab.share_link", json!({"copy": false, "reveal": true}))["link"], link);
    assert_eq!((elements(w.s(0)), elements(w.s(1))), (1, 1));
    assert!(!w.relay.store().dump().is_empty());
    // Edits still flow both ways.
    testing::add_rect(w.s(1), 300.0, 0.0, 10.0, 10.0).unwrap();
    w.settle();
    assert_eq!(elements(w.s(0)), 2);
}

#[test]
fn a_reloaded_session_keeps_no_trace_of_a_deleted_room() {
    let mut w = deleting();
    w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 100.0, 60.0).unwrap();
    w.settle();
    run(w.s(0), "collab.delete_room", json!({"confirm": true}));
    w.settle();
    w.s(0).save_workspace().unwrap();
    let mut s = reload(&w.c[0].1);
    s.restore_workspace().unwrap();
    assert_eq!(elements(&s), 1, "the drawing came back");
    assert!(s.tab().ext.get::<RoomBinding>().is_none_or(|b| b.0.is_none()));
    assert!(s.tab().ext.get::<SealedBinding>().is_none_or(|b| b.0.is_none()), "no key, so no room to find again");
}
