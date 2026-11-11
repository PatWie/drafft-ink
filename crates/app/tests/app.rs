//! The app without a window: start-up, restore, the file argument and the room link, over the fake ports.

use drafftink_app::{App, AppConfig, NoHost, Platform, SharedClipboard, StartupLink, StartupToken, WakeCell};
use drafftink_engine::cmd::collab::fake_server::FakeServer;
use drafftink_engine::cmd::collab::{ConnectionState, RoomLink};
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::ports::{FileRef, Files};
use drafftink_engine::services::LazyTypesetter;
use drafftink_scene::SceneRead;
use serde_json::json;

fn platform(fakes: &Fakes) -> Platform {
    Platform {
        ports: fakes.ports(),
        host: Box::new(NoHost),
        wake: WakeCell::new(),
        clipboard: SharedClipboard::new(),
        typesetter: LazyTypesetter::shared(),
        bridge: None,
        is_mac: false,
    }
}

fn name(app: &mut App) -> String {
    app.session_mut().tab().doc.store().meta().name.clone()
}

#[test]
fn a_first_run_shows_the_intro_document() {
    let fakes = Fakes::new();
    let mut app = App::new(platform(&fakes), AppConfig::default());
    assert_eq!(name(&mut app), "Welcome");
    assert!(app.session_mut().store().len() > 10);
}

#[test]
fn the_second_run_restores_the_workspace_instead_of_the_intro() {
    let fakes = Fakes::new();
    {
        let mut app = App::new(platform(&fakes), AppConfig::default());
        app.session_mut()
            .execute("element.create", json!({"kind": "rectangle", "bounds": {"x": 0, "y": 0, "w": 50, "h": 40}}))
            .unwrap();
        // Dropping the app writes the workspace, as closing the window does.
    }
    assert!(!fakes.storage.is_empty(), "the workspace was written when the app was dropped");
    let mut app = App::new(platform(&fakes), AppConfig::default());
    let before = app.session_mut().store().len();
    assert!(before > 10, "the intro document and the new rectangle came back ({before} elements)");
    assert_eq!(app.session_mut().tabs.len(), 1);
}

#[test]
fn the_file_argument_opens_in_place_of_the_intro() {
    let fakes = Fakes::new();
    let doc = r#"{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}"#;
    let file = FileRef::Path { path: "/tmp/x.excalidraw".into() };
    let mut files = fakes.files.clone();
    files.write(&file, doc.as_bytes(), "application/json").unwrap();
    let config = AppConfig { open: Some(file), ..AppConfig::default() };
    let mut app = App::new(platform(&fakes), config);
    assert_ne!(name(&mut app), "Welcome", "the file replaced the intro on a first run");
}

#[test]
fn a_room_link_joins_the_room_at_start() {
    let fakes = Fakes::new();
    let config = AppConfig {
        join: StartupLink {
            link: RoomLink { room: Some("team-1".into()), server: Some("ws://localhost:3030/ws".into()) },
        },
        ..AppConfig::default()
    };
    let mut app = App::new(platform(&fakes), config);
    let view = app.session_mut().collab_view();
    assert_eq!(view.room.as_deref(), Some("team-1"));
}

#[test]
fn an_unreadable_server_in_the_link_is_a_refusal_not_a_crash() {
    let fakes = Fakes::new();
    let config = AppConfig {
        join: StartupLink { link: RoomLink { room: Some("r".into()), server: Some("http://example.com".into()) } },
        ..AppConfig::default()
    };
    let mut app = App::new(platform(&fakes), config);
    assert!(app.session_mut().collab_view().room.is_none());
}

#[test]
fn a_room_link_keeps_the_work_the_person_already_has_in_its_own_tab() {
    let fakes = Fakes::new();
    {
        let _first = App::new(platform(&fakes), AppConfig::default());
    }
    let config = AppConfig {
        join: StartupLink {
            link: RoomLink { room: Some("team-2".into()), server: Some("ws://localhost:3030/ws".into()) },
        },
        ..AppConfig::default()
    };
    let mut app = App::new(platform(&fakes), config);
    assert_eq!(app.session_mut().tabs.len(), 2, "the restored intro stays in its tab");
    assert_eq!(app.session_mut().collab_view().room.as_deref(), Some("team-2"));
}

#[test]
fn the_token_of_the_launcher_reaches_a_server_that_asks_for_one_and_stays_out_of_the_ring() {
    let fakes = Fakes::new();
    let mut hub = FakeServer::new();
    hub.require_token("launch-token-1");
    let config = AppConfig {
        join: StartupLink {
            link: RoomLink { room: Some("team-3".into()), server: Some("ws://localhost:3030/ws".into()) },
        },
        collab_token: StartupToken::new("launch-token-1"),
        ..AppConfig::default()
    };
    assert!(!format!("{config:?}").contains("launch-token-1"), "the config must not print the token");
    let mut app = App::new(platform(&fakes), config);
    for _ in 0..40 {
        fakes.clock.advance_ms(40);
        app.session_mut().collab_poll();
        hub.attach(&fakes.net);
        hub.step();
    }
    assert_eq!(app.session_mut().collab_view().state, ConnectionState::Connected);
    let ring = serde_json::to_string(&app.session_mut().ring().recent(1000, false, None)).unwrap();
    assert!(ring.contains("collab.connect") && !ring.contains("launch-token-1"), "{ring}");
}

#[test]
fn a_token_that_is_empty_is_no_token() {
    assert!(StartupToken::new("").is_none());
    assert!(StartupToken::new("  \n").is_none());
}
