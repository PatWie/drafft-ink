#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Collaboration: the peer list with colours and the follow button. Two real sessions meet in a room on the
//! in-memory server; the button runs `collab.follow` and the other person's camera drives ours.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::Session;
use drafftink_engine::cmd::collab::fake_server::FakeServer;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::Services;
use drafftink_ui::{Call, CollabStatus, CollabView, Peer};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

const URL: &str = "ws://127.0.0.1:3030/ws";

/// Ana (`a`) and the user of the UI (`b`) in one room.
struct Room {
    hub: FakeServer,
    ana: Session,
    ana_fakes: Fakes,
    ui: Fixture,
}

impl Room {
    fn new() -> Self {
        let ana_fakes = Fakes::new();
        let mut ana = Session::with_services(ana_fakes.ports(), Services::fake());
        let ui_fakes = Fakes::new();
        let mut b = Session::with_services(ui_fakes.ports(), Services::fake());
        for s in [&mut ana, &mut b] {
            s.execute("collab.connect", json!({ "url": URL })).unwrap();
            s.execute("collab.join_room", json!({ "room": "r1" })).unwrap();
        }
        ana.execute("collab.set_user", json!({ "name": "Ana", "color": "#ef4444" })).unwrap();
        let mut room = Room { hub: FakeServer::new(), ana, ana_fakes, ui: Fixture::over(b, ui_fakes) };
        room.settle();
        room
    }

    fn settle(&mut self) {
        for _ in 0..40 {
            self.ana_fakes.clock.advance_ms(40);
            self.ui.fakes.clock.advance_ms(40);
            self.ana.collab_poll();
            self.ui.session.collab_poll();
            self.hub.attach(&self.ana_fakes.net);
            self.hub.attach(&self.ui.fakes.net);
            self.hub.step();
        }
        self.refresh_collab();
    }

    /// What the app does every frame: build the view from the engine's `collab.status`.
    fn refresh_collab(&mut self) {
        let status = self.ui.session.query("collab.status", json!({})).unwrap();
        self.ui.model.collab = CollabView::from_status(&status);
    }
}

fn calls(h: &Harness<'static, Fixture>) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

#[test]
fn the_view_is_built_from_collab_status_with_ids_names_colours_and_follow_state() {
    let room = Room::new();
    let view = &room.ui.model.collab;
    assert_eq!(view.status, CollabStatus::Connected);
    assert_eq!(view.room.as_deref(), Some("r1"));
    let [ana] = view.peers.as_slice() else { panic!("{:?}", view.peers) };
    assert_eq!((ana.name.as_str(), ana.color.as_str()), ("Ana", "#ef4444"));
    assert!(!ana.id.is_empty() && !ana.followed);
}

#[test]
fn collab_status_states_map_to_the_ui_status() {
    let state = |state: &str, extra: Value| {
        let mut v = json!({ "state": state, "peers": [] });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        CollabView::from_status(&v).status
    };
    assert_eq!(state("disconnected", json!({})), CollabStatus::Disconnected);
    assert_eq!(state("ready", json!({})), CollabStatus::Disconnected);
    assert_eq!(state("connecting", json!({})), CollabStatus::Connecting);
    assert_eq!(state("connected", json!({})), CollabStatus::Connected);
    assert_eq!(state("error", json!({ "error": "timed out", "text": "x" })), CollabStatus::Failed("timed out".into()));
    assert_eq!(state("error", json!({ "text": "refused" })), CollabStatus::Failed("refused".into()));
    // An entry without an id cannot be followed, so it is left out.
    let v = CollabView::from_status(
        &json!({ "state": "connected", "peers": [{ "name": "x" }, { "id": "p", "name": "y" }] }),
    );
    assert_eq!(v.peers.len(), 1);
    assert_eq!(
        Peer::from_status(&json!({ "id": "p", "pointer": [1, 2], "followed": true }))
            .map(|p| (p.has_cursor, p.followed)),
        Some((true, true))
    );
}

#[test]
fn each_peer_has_a_follow_button_that_follows_and_then_stops() {
    let mut room = Room::new();
    let ana_id = room.ui.model.collab.peers[0].id.clone();
    room.ana.execute("view.set", json!({ "scroll": [500, -200], "zoom": 3 })).unwrap();
    room.settle();
    let mut h = dialog::harness(std::mem::replace(&mut room.ui, Fixture::new()));
    h.run();
    h.get_by_label("Follow Ana");
    h.state_mut().clear();
    h.get_by_label("Follow Ana").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "collab.follow", json!({ "peer": ana_id }))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);

    // Hand the fixture back to the room, let the camera follow, and show the new state.
    room.ui = std::mem::replace(h.state_mut(), Fixture::new());
    room.settle();
    let view = *room.ui.session.view();
    assert!((view.scroll.x - 500.0).abs() < 1e-6 && (view.zoom - 3.0).abs() < 1e-6, "{view:?}");
    assert!(room.ui.model.collab.peers[0].followed);

    let mut h = dialog::harness(std::mem::replace(&mut room.ui, Fixture::new()));
    h.run();
    h.state_mut().clear();
    h.get_by_label("Stop following Ana").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "collab.follow", json!({}))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn a_peer_who_left_makes_the_real_command_refuse_with_its_words() {
    let mut room = Room::new();
    let gone = room.ui.model.collab.peers[0].clone();
    let mut h = dialog::harness(std::mem::replace(&mut room.ui, Fixture::new()));
    h.run();
    // The peer list is stale: the room has nobody called this.
    h.state_mut().session.execute("collab.leave_room", json!({})).unwrap();
    let call = drafftink_ui::UiAction::run("collab.follow", json!({ "peer": gone.id }));
    let f = h.state_mut();
    let report = drafftink_ui::dispatch(&mut f.session, &mut f.state, vec![call], 0.0);
    assert_eq!(report.refusals().next().map(|r| r.code), Some("not_in_room"));
    assert_eq!(f.state.toasts.len(), 1);
}

#[test]
fn the_panel_shows_each_peer_in_their_colour() {
    let room = Room::new();
    let mut h = dialog::harness(room.ui);
    h.run();
    // The dot is drawn with the peer's colour; the accessible name is the follow button's.
    h.get_by_label("Follow Ana");
    assert_eq!(h.state().model.collab.peers[0].color, "#ef4444");
}
