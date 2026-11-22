//! Tests of the encrypted rooms of the `collab` family: the command surface (secret arguments, refusals, the link),
//! two sessions through the real relay core, the workspace record of a room tab and the key staying out of every
//! output. Sessions talk through in-memory loopbacks to a [`FakeRelay`].

use drafftink_relay::RelayConfig;
use drafftink_scene::SceneRead;
use serde_json::{Value, json};

use super::fake_relay::FakeRelay;
use crate::cmd::tab::{RoomBinding, SealedBinding};
use crate::ports::{ClipPayload, Clock, Storage};
use crate::ports::fake::Fakes;
use crate::refusal::Refusal;
use crate::services::Services;
use crate::session::Session;
use crate::testing;

pub(super) const URL: &str = "ws://127.0.0.1:3030/ws";

pub(super) fn run(s: &mut Session, id: &str, args: Value) -> Value {
    s.execute(id, args).unwrap_or_else(|r| panic!("{id} was refused: {} ({})", r.message, r.code))
}

pub(super) fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    s.execute(id, args).expect_err("the command should be refused")
}

pub(super) struct World {
    pub(super) relay: FakeRelay,
    pub(super) c: Vec<(Session, Fakes)>,
}

impl World {
    pub(super) fn new(n: usize) -> Self {
        Self::with_config(n, RelayConfig::self_host())
    }

    /// `n` sessions and a relay with this policy.
    pub(super) fn with_config(n: usize, config: RelayConfig) -> Self {
        let c = (0..n)
            .map(|i| {
                let (mut s, fakes) = testing::session();
                for _ in 0..i * 100 {
                    s.ports.ids.element_id();
                }
                (s, fakes)
            })
            .collect();
        World { relay: FakeRelay::new(config), c }
    }

    pub(super) fn settle(&mut self) {
        self.settle_for(3200);
    }

    /// Runs the sessions and the relay for `ms` of simulated time, in steps of 40 ms.
    pub(super) fn settle_for(&mut self, ms: u64) {
        for _ in 0..ms / 40 {
            for (s, f) in &mut self.c {
                f.clock.advance_ms(40);
                s.collab_poll();
                self.relay.attach(&f.net);
            }
            let now = self.c.first().map_or(0, |(_, f)| f.clock.mono_ms());
            self.relay.step(now);
        }
    }

    pub(super) fn s(&mut self, i: usize) -> &mut Session {
        &mut self.c[i].0
    }

    /// Session `i` makes a new encrypted room and returns its link.
    pub(super) fn create(&mut self, i: usize) -> String {
        run(self.s(i), "collab.connect", json!({"url": URL}));
        let out = run(self.s(i), "collab.join_room", json!({}));
        assert_eq!(out["e2e"], true);
        self.settle();
        let shared = run(self.s(i), "collab.share_link", json!({"copy": false, "reveal": true}));
        shared["link"].as_str().expect("the link").to_owned()
    }

    /// Session `i` joins the room of `link`.
    pub(super) fn join(&mut self, i: usize, link: &str, extra: Value) {
        run(self.s(i), "collab.connect", json!({"url": URL}));
        let mut args = json!({"room": link});
        if let (Value::Object(a), Value::Object(e)) = (&mut args, extra) {
            a.extend(e);
        }
        run(self.s(i), "collab.join_room", args);
        self.settle();
    }
}

fn key_of(link: &str) -> String {
    link.split_once(',').expect("a link with a key").1.split('&').next().expect("key text").to_owned()
}

pub(super) fn elements(s: &Session) -> usize {
    s.store().iter().count()
}

#[test]
fn a_new_room_is_encrypted_has_no_link_in_its_output_and_shows_only_its_id() {
    let mut w = World::new(1);
    run(w.s(0), "collab.connect", json!({"url": URL}));
    let out = run(w.s(0), "collab.join_room", json!({}));
    assert_eq!(out["e2e"], true);
    assert!(out.get("link").is_none(), "{out}");
    let id = out["room"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 22, "the id of an encrypted room is 16 bytes of base64url");
    w.settle();
    let view = run(w.s(0), "collab.status", json!({}));
    assert_eq!((view["e2e"].clone(), view["room"].clone(), view["state"].clone()), (json!(true), json!(id), json!("connected")));
    assert_eq!(w.relay.members(), 1);
}

#[test]
fn the_link_and_the_key_stay_out_of_the_ring_the_status_and_every_output() {
    let mut w = World::new(1);
    let link = w.create(0);
    let key = key_of(&link);
    let mut seen = vec![
        run(w.s(0), "collab.status", json!({})),
        run(w.s(0), "engine.recent", json!({"limit": 100})),
        run(w.s(0), "settings.get", json!({})),
        run(w.s(0), "collab.share_link", json!({"copy": false})),
    ];
    // A second session joins with the link as an argument: the ring records `<hidden>`.
    let mut b = World::new(1);
    run(b.s(0), "collab.connect", json!({"url": URL, "room": link, "on_local": "merge"}));
    seen.push(run(b.s(0), "engine.recent", json!({"limit": 100})));
    seen.push(run(b.s(0), "collab.status", json!({})));
    let ring = serde_json::to_string(&b.s(0).ring().recent(1000, false, None)).unwrap();
    let text = serde_json::to_string(&seen).unwrap();
    assert!(!ring.contains(&key) && !text.contains(&key), "the key leaked:\n{ring}\n{text}");
    assert!(ring.contains("<hidden>"), "the call is recorded without the room: {ring}");
    let debug = format!("{:?}{:?}", b.s(0).collab_view(), w.s(0).collab_view());
    assert!(!debug.contains(&key));
    let tab = format!("{:?}", w.s(0).tab().ext.get::<SealedBinding>());
    assert!(!tab.contains(&key), "{tab}");
}

#[test]
fn a_damaged_link_is_refused_in_plain_words_without_repeating_it() {
    let mut w = World::new(1);
    let link = w.create(0);
    let mut w2 = World::new(1);
    run(w2.s(0), "collab.connect", json!({"url": URL}));
    let (id, key) = link.split_once("room=").unwrap().1.split_once(',').unwrap();
    let key = key.split('&').next().unwrap();
    for bad in [format!("#room={id}"), format!("#room={id},{}", &key[..20]), format!("#room={id},{key}x"), format!("{id},{}", "A".repeat(43)), "room=a,b".to_owned(), "#room=,".to_owned()] {
        let r = refuse(w2.s(0), "collab.join_room", json!({"room": bad}));
        assert_eq!(r.code, "bad_room_link", "{bad}");
        assert!(r.message.contains("must include the key after the comma"), "{}", r.message);
        assert!(!r.message.contains(key) && !r.message.contains(&bad), "{}", r.message);
    }
    let mut w3 = World::new(1);
    let r = refuse(w3.s(0), "collab.connect", json!({"url": URL, "room": format!("#room={id}")}));
    assert_eq!(r.code, "bad_room_link");
    assert!(!r.message.contains(id));
    // A plain name stays a plaintext room.
    assert_eq!(refuse(w2.s(0), "collab.join_room", json!({"room": "no way"})).code, "bad_args");
}

#[test]
fn two_sessions_share_a_drawing_through_the_relay() {
    let mut w = World::new(2);
    let link = w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 100.0, 60.0).unwrap();
    w.settle();
    w.join(1, &link, json!({}));
    assert_eq!(elements(w.s(1)), 1, "the late joiner got the drawing");
    testing::add_rect(w.s(1), 200.0, 0.0, 50.0, 50.0).unwrap();
    w.settle();
    assert_eq!(elements(w.s(0)), 2, "an edit of the second session reached the first");
    let peers = run(w.s(0), "collab.status", json!({}))["peers"].as_array().unwrap().len();
    assert_eq!(peers, 1);
    assert!(w.relay.store().dump().iter().all(|record| !record.windows(4).any(|w| w == b"Rect")), "the relay holds no readable content");
}

#[test]
fn share_link_copies_by_default_and_reveals_only_when_asked() {
    let mut w = World::new(1);
    let link = w.create(0);
    let out = run(w.s(0), "collab.share_link", json!({}));
    assert_eq!(out["copied"], true);
    assert!(out.get("link").is_none(), "the link is not in the output unless asked: {out}");
    let copied = w.c[0].1.clipboard.content();
    assert_eq!(copied, Some(ClipPayload::Text(link.clone())));
    assert!(link.starts_with("https://drafft.ink/#room="), "{link}");
    assert!(link.ends_with("&server=ws://127.0.0.1:3030/ws"), "a server of its own goes into the link: {link}");
    let spec = w.s(0).registry().find("collab.share_link").unwrap();
    assert!(spec.secret_output && spec.reads_only);
}

#[test]
fn share_link_refuses_outside_an_encrypted_room() {
    let mut w = World::new(1);
    assert_eq!(refuse(w.s(0), "collab.share_link", json!({})).code, "not_in_room");
    run(w.s(0), "collab.connect", json!({"url": URL}));
    run(w.s(0), "collab.join_room", json!({"room": "plain-room"}));
    let r = refuse(w.s(0), "collab.share_link", json!({}));
    assert_eq!(r.code, "not_e2e");
    assert_eq!(r.message, "This room has no key, so it has no private link.");
}

#[test]
fn leaving_the_room_forgets_the_key_but_keeps_the_drawing() {
    let mut w = World::new(1);
    w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 10.0, 10.0).unwrap();
    w.settle();
    run(w.s(0), "collab.leave_room", json!({}));
    assert!(w.s(0).tab().ext.get::<SealedBinding>().is_none_or(|b| b.0.is_none()));
    assert!(w.s(0).tab().ext.get::<RoomBinding>().is_none_or(|b| b.0.is_none()));
    assert_eq!(elements(w.s(0)), 1);
    assert_eq!(refuse(w.s(0), "collab.share_link", json!({})).code, "not_in_room");
}

#[test]
fn only_share_link_has_a_secret_output_and_it_is_quiet() {
    let (s, _) = testing::session();
    let secret: Vec<&str> = s.registry().all().iter().filter(|spec| spec.secret_output).map(|spec| spec.id).collect();
    assert_eq!(secret, ["collab.share_link"]);
    for spec in s.registry().all().iter().filter(|spec| spec.secret_output) {
        assert_eq!(spec.ring, crate::spec::RingMode::Quiet, "{}", spec.id);
    }
    for id in ["collab.connect", "collab.join_room"] {
        assert_eq!(s.registry().find(id).unwrap().secret_args, ["token", "room"], "{id}");
    }
}

// ---- the workspace record ----

pub(super) fn reload(fakes: &Fakes) -> Session {
    Session::with_services(fakes.ports(), Services::fake())
}

#[test]
fn a_room_tab_is_saved_with_its_key_and_snapshot_and_a_reload_reconnects_the_same_tab() {
    let mut w = World::new(1);
    let link = w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 100.0, 60.0).unwrap();
    w.settle();
    let tab_id = w.s(0).tab().id;
    w.s(0).save_workspace().unwrap();
    let storage = &w.c[0].1.storage;
    let index: Value = serde_json::from_slice(&storage.bytes(crate::ports::Ns::Autosave, "workspace").unwrap()).unwrap();
    assert_eq!(index["schema"], 2);
    let room = &index["tabs"][0]["room"];
    assert_eq!(room["link"].as_str().unwrap().split_once(',').unwrap().1, key_of(&link));
    assert!(room["last_seq"].as_u64().unwrap() >= 1, "{room}");
    assert!(!room["relay_vv"].as_str().unwrap().is_empty());
    assert!(storage.bytes(crate::ports::Ns::Autosave, &format!("workspace.t{}.loro", tab_id.0)).is_some());

    // The page is reloaded: a new session on the same storage and network.
    let fakes = &w.c[0].1;
    let mut s = reload(fakes);
    let report = s.restore_workspace().unwrap();
    assert_eq!((report.restored, s.tabs.len()), (1, 1), "one tab, not two");
    assert_eq!(s.tab().id, tab_id);
    assert!(s.tab().ext.get::<RoomBinding>().is_some_and(|b| b.0.is_some()), "the tab is bound to its room again");
    assert_eq!(elements(&s), 1);
    w.c[0].0 = s;
    w.settle();
    let view = run(w.s(0), "collab.status", json!({}));
    assert_eq!((view["e2e"].clone(), view["state"].clone()), (json!(true), json!("connected")), "{view}");
    let again = run(w.s(0), "collab.share_link", json!({"copy": false, "reveal": true}));
    assert_eq!(again["link"], link, "the key came back from the record");
}

#[test]
fn a_restored_copy_shares_the_rooms_history_so_nothing_is_duplicated_on_rejoin() {
    let mut w = World::new(2);
    let link = w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 100.0, 60.0).unwrap();
    w.settle();
    w.join(1, &link, json!({}));
    w.s(1).save_workspace().unwrap();
    let fakes = &w.c[1].1;
    let mut restored = reload(fakes);
    restored.restore_workspace().unwrap();
    w.c[1].0 = restored;
    w.settle();
    assert_eq!((elements(w.s(0)), elements(w.s(1))), (1, 1), "the copy and the room hold the same single element");
}

#[test]
fn a_record_of_schema_one_is_still_read_and_has_no_room() {
    let (mut s, fakes) = testing::session();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    s.save_workspace().unwrap();
    let key = "workspace";
    let mut index: Value = serde_json::from_slice(&fakes.storage.bytes(crate::ports::Ns::Autosave, key).unwrap()).unwrap();
    index["schema"] = json!(1);
    fakes.storage.clone().put(crate::ports::Ns::Autosave, key, &serde_json::to_vec(&index).unwrap()).unwrap();
    let mut again = reload(&fakes);
    let report = again.restore_workspace().unwrap();
    assert_eq!(report.restored, 1);
    assert!(again.tab().ext.get::<SealedBinding>().is_none_or(|b| b.0.is_none()));
}

#[test]
fn a_newer_schema_is_left_alone() {
    let (mut s, fakes) = testing::session();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    s.save_workspace().unwrap();
    let mut index: Value = serde_json::from_slice(&fakes.storage.bytes(crate::ports::Ns::Autosave, "workspace").unwrap()).unwrap();
    index["schema"] = json!(3);
    fakes.storage.clone().put(crate::ports::Ns::Autosave, "workspace", &serde_json::to_vec(&index).unwrap()).unwrap();
    let mut again = reload(&fakes);
    assert_eq!(again.restore_workspace().unwrap().newer_schema, Some(3));
}

#[test]
fn a_room_tab_without_its_snapshot_comes_back_as_a_local_copy() {
    let mut w = World::new(1);
    w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 10.0, 10.0).unwrap();
    w.settle();
    let tab_id = w.s(0).tab().id;
    w.s(0).save_workspace().unwrap();
    let fakes = &w.c[0].1;
    fakes.storage.clone().delete(crate::ports::Ns::Autosave, &format!("workspace.t{}.loro", tab_id.0)).unwrap();
    let mut s = reload(fakes);
    s.restore_workspace().unwrap();
    assert!(s.tab().ext.get::<SealedBinding>().is_none_or(|b| b.0.is_none()), "a copy without the room's history is never merged back");
    assert_eq!(elements(&s), 1);
}

#[test]
fn leaving_removes_the_stored_snapshot_with_the_next_save() {
    let mut w = World::new(1);
    w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 10.0, 10.0).unwrap();
    w.settle();
    let tab_id = w.s(0).tab().id;
    w.s(0).save_workspace().unwrap();
    run(w.s(0), "collab.leave_room", json!({}));
    w.s(0).save_workspace().unwrap();
    let storage = &w.c[0].1.storage;
    assert!(storage.bytes(crate::ports::Ns::Autosave, &format!("workspace.t{}.loro", tab_id.0)).is_none());
    let index = String::from_utf8(storage.bytes(crate::ports::Ns::Autosave, "workspace").unwrap()).unwrap();
    assert!(!index.contains("\"room\""), "the key is gone from the record: {index}");
}

// ---- an expired room ----

#[test]
fn a_link_finds_the_restored_tab_of_its_room() {
    let mut w = World::new(1);
    let link = w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 10.0, 10.0).unwrap();
    w.settle();
    w.s(0).save_workspace().unwrap();
    let mut s = reload(&w.c[0].1);
    s.restore_workspace().unwrap();
    assert_eq!(s.room_tab_for_link(&link), Some(0));
    assert_eq!(s.room_tab_for_link("#room=AAAAAAAAAAAAAAAAAAAAAA,x"), None);
    assert_eq!(s.room_tab_for_link("plain-room"), None, "a plain name is no encrypted room");
}

#[test]
fn an_expired_room_is_refused_by_default_and_restored_from_the_copy_on_request() {
    let mut w = World::new(2);
    let link = w.create(0);
    testing::add_rect(w.s(0), 0.0, 0.0, 100.0, 60.0).unwrap();
    w.settle();
    w.s(0).save_workspace().unwrap();
    // The relay forgot the room (it expired): a new relay with nothing in it.
    w.relay = FakeRelay::new(RelayConfig::self_host());
    let mut s = reload(&w.c[0].1);
    s.restore_workspace().unwrap();
    w.c[0].0 = s;
    w.settle();
    let view = run(w.s(0), "collab.status", json!({}));
    assert_eq!(view["state"], "error", "the reconnect did not make a room: {view}");
    assert!(view["error"].as_str().unwrap().contains("This room is gone"), "{view}");
    assert_eq!(w.relay.members(), 0);

    // The person asks for it to be restored from the copy this tab holds.
    run(w.s(0), "collab.leave_room", json!({}));
    run(w.s(0), "collab.join_room", json!({"room": link, "on_local": "merge", "if_missing": "restore"}));
    w.settle();
    assert_eq!(run(w.s(0), "collab.status", json!({}))["state"], "connected");
    // Somebody else opens the link and finds the restored drawing.
    w.join(1, &link, json!({}));
    assert_eq!(elements(w.s(1)), 1);
}

// ---- a big drawing meets a newcomer ----

/// Adds `count` rectangles to the session's document, each as its own commit as a person drawing would.
fn add_many(s: &mut Session, count: usize) {
    for _ in 0..count {
        testing::add_rect(s, 0.0, 0.0, 100.0, 60.0).unwrap();
    }
}

/// How the room holds the drawing when the second session arrives.
#[derive(Clone, Copy, Debug)]
enum Held {
    /// The first session came with the whole drawing and merged it into the new room: one snapshot, no log.
    Snapshot,
    /// The first session drew it in the empty room before anything was sent: a tiny snapshot of the empty room and
    /// one big record.
    Record,
    /// The first session drew it in ten parts and the relay compacted: a snapshot and a short tail.
    Compacted,
}

fn big_room_joins(count: usize, held: Held) {
    let mut config = RelayConfig::self_host();
    config.flush.interval_ms = 0;
    config.compact.records = 6;
    config.compact.join_records = 100_000;
    let mut w = World::with_config(2, config);
    let link = match held {
        Held::Snapshot => {
            add_many(w.s(0), count);
            run(w.s(0), "collab.connect", json!({"url": URL}));
            run(w.s(0), "collab.join_room", json!({"on_local": "merge"}));
            w.settle();
            run(w.s(0), "collab.share_link", json!({"copy": false, "reveal": true}))["link"].as_str().unwrap().to_owned()
        }
        Held::Record => {
            let link = w.create(0);
            add_many(w.s(0), count);
            w.settle();
            link
        }
        Held::Compacted => {
            let link = w.create(0);
            for _ in 0..10 {
                add_many(w.s(0), count / 10);
                w.settle_for(400);
            }
            w.settle_for(20_000);
            let meta = drafftink_relay::RelayStore::load_meta(&mut w.relay.store().clone()).unwrap().unwrap();
            assert!(meta.has_snapshot && meta.snapshot_seq > 0, "the relay holds a snapshot: {meta:?}");
            link
        }
    };
    assert_eq!(elements(w.s(0)), count);
    w.join(1, &link, json!({}));
    w.settle_for(10_000);
    assert_eq!(elements(w.s(1)), count, "{held:?} {count}: {}", run(w.s(1), "collab.status", json!({}))["text"]);
    let status = run(w.s(1), "collab.status", json!({}));
    assert_eq!(status["state"], "connected", "{status}");
    assert_eq!(status["warnings"], json!([]), "{status}");
}

macro_rules! big_room_tests {
    ($($name:ident: $count:expr, $held:expr;)*) => { $(#[test] fn $name() { big_room_joins($count, $held); })* };
}

big_room_tests! {
    a_snapshot_of_1000_elements_reaches_a_newcomer: 1000, Held::Snapshot;
    a_snapshot_of_3000_elements_reaches_a_newcomer: 3000, Held::Snapshot;
    a_snapshot_of_10000_elements_reaches_a_newcomer: 10_000, Held::Snapshot;
    one_record_of_1000_elements_reaches_a_newcomer: 1000, Held::Record;
    one_record_of_3000_elements_reaches_a_newcomer: 3000, Held::Record;
    one_record_of_10000_elements_reaches_a_newcomer: 10_000, Held::Record;
    a_compacted_room_of_1000_elements_reaches_a_newcomer: 1000, Held::Compacted;
    a_compacted_room_of_3000_elements_reaches_a_newcomer: 3000, Held::Compacted;
    a_compacted_room_of_10000_elements_reaches_a_newcomer: 10_000, Held::Compacted;
}

#[test]
fn a_drawing_over_what_a_room_holds_is_refused_in_plain_words_and_stays_on_this_device() {
    let config = RelayConfig { max_room_state_bytes: 100_000, ..RelayConfig::self_host() };
    let mut w = World::with_config(2, config);
    let link = w.create(0);
    add_many(w.s(0), 3000);
    w.settle_for(10_000);
    let status = run(w.s(0), "collab.status", json!({}));
    assert_eq!(status["state"], "error", "{status}");
    assert_eq!(status["unsynced"], true, "{status}");
    let text = status["text"].as_str().unwrap();
    assert!(text.contains("holds as much as a room may") && text.contains("Save a copy"), "{text}");
    assert_eq!(status["error"], status["text"], "the error line says the same");
    let notices = w.s(0).take_collab_notices();
    assert!(notices.iter().any(|n| matches!(n, crate::cmd::collab::CollabNotice::Problem(p) if p.text == text)), "{notices:?}");
    assert_eq!(elements(w.s(0)), 3000, "the drawing stays on this device");
    w.join(1, &link, json!({}));
    assert_eq!(elements(w.s(1)), 0, "the room holds nothing of it");
}
