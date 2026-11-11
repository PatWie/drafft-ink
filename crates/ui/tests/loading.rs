//! The loading banner of a room that is joining.

mod common;
mod dialog;

use drafftink_ui::{CollabStatus, CollabView, RoomLoading};
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn with_loading(loading: Option<RoomLoading>) -> egui_kittest::Harness<'static, common::Fixture> {
    let mut f = dialog::fixture();
    f.model.collab =
        CollabView { status: CollabStatus::Connected, room: Some("team".into()), loading, ..CollabView::default() };
    let mut h = dialog::harness(f);
    h.run();
    h
}

#[test]
fn the_status_object_of_each_phase_becomes_a_loading_state() {
    let phase = |v: serde_json::Value| RoomLoading::from_status(&v);
    assert_eq!(phase(json!({ "phase": "connecting" })), Some(RoomLoading::Connecting));
    assert_eq!(phase(json!({ "phase": "syncing" })), Some(RoomLoading::Syncing));
    assert_eq!(phase(json!({ "phase": "receiving", "bytes": 4096 })), Some(RoomLoading::Receiving { bytes: 4096 }));
    assert_eq!(
        phase(json!({ "phase": "importing", "done": 3, "total": 12 })),
        Some(RoomLoading::Importing { done: 3, total: 12 })
    );
    for hostile in
        [json!({}), json!({ "phase": 3 }), json!({ "phase": "warp" }), json!({ "phase": "importing" }), json!("x")]
    {
        assert_eq!(phase(hostile.clone()), None, "{hostile}");
    }
}

#[test]
fn a_collab_status_with_loading_reaches_the_view() {
    let view = CollabView::from_status(&json!({
        "state": "connected", "room": "r", "peers": [], "loading": { "phase": "importing", "done": 1, "total": 4 }
    }));
    assert_eq!(view.loading, Some(RoomLoading::Importing { done: 1, total: 4 }));
    let done = CollabView::from_status(&json!({ "state": "connected", "room": "r", "peers": [] }));
    assert_eq!(done.loading, None);
}

#[test]
fn the_percentage_is_the_share_of_items_shown_rounded_down() {
    let text = |done, total| RoomLoading::Importing { done, total }.text();
    assert_eq!(text(0, 200), "Loading the room: 0%");
    assert_eq!(text(1, 3), "Loading the room: 33%");
    assert_eq!(text(199, 200), "Loading the room: 99%");
    assert_eq!(text(200, 200), "Loading the room: 100%");
    assert_eq!(text(5, 0), "Loading the room...", "an empty room has no percentage");
    assert_eq!(RoomLoading::Importing { done: 9, total: 3 }.fraction(), Some(1.0), "never above a whole");
    assert_eq!(RoomLoading::Syncing.fraction(), None);
    assert_eq!(RoomLoading::Receiving { bytes: 3 * 1024 * 1024 }.text(), "Receiving the room (3.0 MB)...");
}

#[test]
fn the_banner_shows_while_the_room_loads_and_not_after() {
    let h = with_loading(Some(RoomLoading::Importing { done: 50, total: 200 }));
    h.get_by_label("Loading the room: 25%");
    let h = with_loading(Some(RoomLoading::Connecting));
    h.get_by_label("Connecting to the room...");
    let h = with_loading(None);
    assert!(h.query_by_label_contains("the room").is_none(), "no banner once the room is in");
}
