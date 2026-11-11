//! The app without a window: start-up, restore, the file argument and the room link, over the fake ports.

use drafftink_app::{App, AppConfig, NoHost, Platform, SharedClipboard, StartupLink, WakeCell};
use drafftink_engine::cmd::collab::RoomLink;
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
        mcp_http: None,
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
