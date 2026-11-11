//! A room joined after start (the dialog's button, an agent's `collab.join_room`) is polled from then on.

use drafftink_engine::cmd::collab::ConnectionState;
use drafftink_engine::cmd::collab::fake_server::FakeServer;
use drafftink_engine::ports::fake::Fakes;
use serde_json::json;

use super::*;
use crate::platform::{NoHost, Platform, SharedClipboard};

fn app(fakes: &Fakes) -> App {
    let platform = Platform {
        ports: fakes.ports(),
        host: Box::new(NoHost),
        wake: WakeCell::new(),
        clipboard: SharedClipboard::new(),
        typesetter: drafftink_engine::services::LazyTypesetter::shared(),
        bridge: None,
        mcp_http: None,
        is_mac: false,
    };
    App::new(platform, AppConfig::default())
}

/// Runs the app's own poll, as its frames and wakes do, against a server that wants `token`.
fn poll_until_settled(app: &mut App, fakes: &Fakes, hub: &mut FakeServer) {
    let mut now = 1_000u64;
    for _ in 0..60 {
        now += 40;
        fakes.clock.advance_ms(40);
        app.poll_collab(now);
        hub.attach(&fakes.net);
        hub.step();
    }
}

#[test]
fn a_room_joined_by_command_after_a_start_without_one_gets_polled_and_connects() {
    let fakes = Fakes::new();
    let mut hub = FakeServer::new();
    hub.require_token("room-token-1");
    let mut app = app(&fakes);
    // The first poll at start finds no room and nothing due, as on a start without a room link.
    app.poll_collab(0);
    app.session.execute("tab.new", json!({})).expect("a new tab");
    app.session.execute("collab.connect", json!({ "url": "ws://localhost:3030/ws" })).expect("a server");
    app.session
        .execute("collab.join_room", json!({ "room": "later", "token": "room-token-1" }))
        .expect("a join with the token");
    poll_until_settled(&mut app, &fakes, &mut hub);
    assert_eq!(
        app.session.collab_view().state,
        ConnectionState::Connected,
        "the room joined after start is polled, and the token let it in"
    );
}

#[test]
fn without_the_token_the_same_join_is_refused_by_the_server_and_says_so() {
    let fakes = Fakes::new();
    let mut hub = FakeServer::new();
    hub.require_token("room-token-1");
    let mut app = app(&fakes);
    app.poll_collab(0);
    app.session.execute("tab.new", json!({})).expect("a new tab");
    app.session.execute("collab.connect", json!({ "url": "ws://localhost:3030/ws" })).expect("a server");
    app.session.execute("collab.join_room", json!({ "room": "later" })).expect("a join");
    poll_until_settled(&mut app, &fakes, &mut hub);
    assert_eq!(app.session.collab_view().state, ConnectionState::Error);
}
