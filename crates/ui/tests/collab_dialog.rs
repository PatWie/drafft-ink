//! The collaboration dialog and the presence panel.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_ui::{CollabStatus, CollabView, Peer, Verb};
use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};

fn open(mut f: Fixture, collab: CollabView) -> Harness<'static, Fixture> {
    f.model.collab = collab;
    f.state.collab_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h
}

fn view(status: CollabStatus, room: Option<&str>, peers: usize) -> CollabView {
    CollabView {
        status,
        room: room.map(str::to_owned),
        peers: (0..peers)
            .map(|i| Peer {
                id: format!("p{i}"),
                followed: false,
                name: if i == 0 { String::new() } else { format!("p{i}") },
                color: "#22c55e".into(),
                has_cursor: false,
            })
            .collect(),
        server_chosen: false,
        loading: None,
    }
}

fn calls(h: &Harness<'static, Fixture>) -> Vec<(String, Value, Origin)> {
    h.state().calls().into_iter().map(|c| (c.id.clone(), c.args.clone(), c.origin)).collect()
}

fn call(id: &str, args: Value) -> (String, Value, Origin) {
    (id.to_owned(), args, Origin::Ui)
}

fn type_into(h: &mut Harness<'static, Fixture>, label: &str, text: &str) {
    dialog::field(h, label).focus();
    h.run();
    dialog::field(h, label).type_text(text);
    h.run();
}

fn is_on(h: &Harness<'static, Fixture>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

#[test]
fn connect_sends_the_url_of_the_setting() {
    let mut h = open(dialog::fixture(), CollabView::default());
    h.get_by_label("Not connected");
    let url = h.state().model.setting_str("collab.server_url").map(str::to_owned);
    assert_eq!(url.as_deref(), Some("ws://localhost:3030/ws"));
    h.get_by_label("Connect").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.connect", json!({"url": "ws://localhost:3030/ws"}))]);
}

fn status(f: &mut Fixture) -> Value {
    dialog::run(f, "collab.status", json!({}))
}

#[test]
fn connect_chooses_the_server_in_the_real_session() {
    let mut h = open(dialog::fixture(), CollabView::default());
    assert_eq!(status(h.state_mut())["state"], json!("disconnected"));
    h.get_by_label("Connect").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    let st = status(h.state_mut());
    assert_eq!((st["state"].clone(), st["url"].clone()), (json!("ready"), json!("ws://localhost:3030/ws")));
}

#[test]
fn a_connected_session_leaves_its_room_and_joins_again_through_the_buttons() {
    let mut h = open(dialog::connected(Some("abc")), view(CollabStatus::Connected, Some("abc"), 1));
    assert_eq!(status(h.state_mut())["room"], json!("abc"));
    h.get_by_label("Leave room").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert!(status(h.state_mut()).get("room").is_none_or(Value::is_null), "the room is gone");
    h.state_mut().state.collab_open = true;
    h.state_mut().model.collab = view(CollabStatus::Connected, None, 0);
    h.run();
    h.state_mut().clear();
    // The field keeps the name of the room that was left.
    h.get_by_label("Join room").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(status(h.state_mut())["room"], json!("abc"));
}

#[test]
fn the_name_and_colour_are_stored_by_the_real_command() {
    let mut h = open(dialog::fixture(), CollabView::default());
    type_into(&mut h, "Display name", "Ada");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    h.get_by_label("Green").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    let m = &h.state().model;
    assert_eq!((m.setting_str("collab.user_name"), m.setting_str("collab.user_color")), (Some("Ada"), Some("#22c55e")));
}

#[test]
fn connect_sends_the_edited_url() {
    let mut h = open(dialog::fixture(), CollabView::default());
    dialog::field(&h, "Server URL").focus();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run();
    dialog::field(&h, "Server URL").type_text("wss://example.org/ws");
    h.run();
    h.get_by_label("Connect").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.connect", json!({"url": "wss://example.org/ws"}))]);
}

#[test]
fn connecting_shows_disconnect() {
    let mut h = open(dialog::connected(None), view(CollabStatus::Connecting, None, 0));
    h.get_by_label("Connecting...");
    assert!(h.query_by_label("Connect").is_none());
    h.get_by_label("Disconnect").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.disconnect", json!({}))]);
}

#[test]
fn connected_without_a_room_can_join_or_create_one() {
    let mut h = open(dialog::connected(None), view(CollabStatus::Connected, None, 0));
    h.get_by_label("Connected (no room)");
    assert!(h.query_by_label("Leave room").is_none());
    // An empty name asks for a new room.
    h.get_by_label("Create room").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.join_room", json!({}))]);
    h.state_mut().clear();
    type_into(&mut h, "Room", "design-1");
    h.get_by_label("Join room").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.join_room", json!({"room": "design-1"}))]);
}

#[test]
fn in_a_room_the_button_is_leave() {
    let mut h = open(dialog::connected(Some("abc")), view(CollabStatus::Connected, Some("abc"), 2));
    h.get_by_label("Connected to room: abc");
    h.get_by_label("2 people online");
    assert!(h.query_by_label("Join room").is_none());
    h.get_by_label("Leave room").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.leave_room", json!({}))]);
}

#[test]
fn failure_shows_the_reason_and_allows_a_retry() {
    let mut h = open(dialog::fixture(), view(CollabStatus::Failed("timed out".into()), None, 0));
    h.get_by_label("Connection error: timed out");
    h.get_by_label("Connect").click();
    h.run();
    assert_eq!(calls(&h).len(), 1);
}

#[test]
fn the_name_is_sent_once_when_enter_is_pressed() {
    let mut h = open(dialog::fixture(), CollabView::default());
    type_into(&mut h, "Display name", "Ada");
    assert!(h.state().calls().is_empty(), "typing sends nothing");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(calls(&h), [call("collab.set_user", json!({"name": "Ada"}))]);
    // Leaving the field afterwards does not send it again.
    dialog::click_canvas(&mut h);
    h.run();
    assert_eq!(calls(&h).len(), 1);
}

#[test]
fn the_name_is_sent_when_focus_is_lost() {
    let mut h = open(dialog::fixture(), CollabView::default());
    type_into(&mut h, "Display name", "Bob");
    h.get_by_label("Cursor colour").click();
    h.run();
    h.run();
    assert_eq!(calls(&h), [call("collab.set_user", json!({"name": "Bob"}))]);
}

#[test]
fn an_unchanged_name_is_not_sent() {
    let mut f = dialog::fixture();
    f.model.settings.insert("collab.user_name".into(), json!("Ada"));
    let mut h = open(f, CollabView::default());
    dialog::field(&h, "Display name").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    dialog::click_canvas(&mut h);
    assert!(h.state().calls().is_empty(), "{:?}", h.state().calls());
}

#[test]
fn nine_colours_send_set_user_and_the_current_one_is_marked() {
    let mut f = dialog::fixture();
    f.model.settings.insert("collab.user_color".into(), json!("#a855f7"));
    let mut h = open(f, CollabView::default());
    assert!(is_on(&h, "Purple"));
    assert!(!is_on(&h, "Indigo"));
    for (name, hex) in [
        ("Red", "#ef4444"),
        ("Orange", "#f97316"),
        ("Yellow", "#eab308"),
        ("Green", "#22c55e"),
        ("Cyan", "#06b6d4"),
        ("Blue", "#3b82f6"),
        ("Indigo", "#6366f1"),
        ("Purple", "#a855f7"),
        ("Pink", "#ec4899"),
    ] {
        h.state_mut().clear();
        h.get_by_label(name).click();
        h.run();
        assert_eq!(calls(&h), [call("collab.set_user", json!({"color": hex}))], "{name}");
    }
}

#[test]
fn the_default_colour_is_indigo() {
    let h = open(dialog::fixture(), CollabView::default());
    assert_eq!(h.state().model.setting_str("collab.user_color"), Some("#6366f1"));
    assert!(is_on(&h, "Indigo"));
}

#[test]
fn unregistered_collab_commands_are_greyed_with_the_build_text() {
    // A build without the collab commands: the registry leaves them out.
    let f = Fixture::without(&["collab.connect", "collab.set_user"]);
    let text = f.model.refusal_text("collab.connect").expect("not registered in this build");
    assert_eq!(text, "`collab.connect` is not available in this build.");
    let mut h = open(f, CollabView::default());
    let connect = h.get_by_label("Connect");
    assert!(connect.accesskit_node().is_disabled());
    connect.hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(&text);
    h.get_by_label("Connect").click();
    h.run();
    assert!(h.state().calls().is_empty());
    assert!(h.get_by_label("Indigo").accesskit_node().is_disabled());
}

#[test]
fn a_refusal_greys_the_controls_with_its_text() {
    // Joining a room writes the tab, so the real command refuses in a view-only document.
    let mut f = dialog::connected(None);
    f.session.view_only = true;
    f.refresh();
    let text = f.model.refusal_text("collab.join_room").expect("refuses in a view-only document");
    assert!(!text.contains("not available in this build"), "{text}");
    let mut h = open(f, view(CollabStatus::Connected, None, 0));
    let create = h.get_by_label("Create room");
    assert!(create.accesskit_node().is_disabled());
    create.hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(&text);
    h.get_by_label("Create room").click();
    h.run();
    assert!(h.state().calls().is_empty());
}

#[test]
fn the_x_and_the_backdrop_close_the_dialog() {
    let mut h = open(dialog::fixture(), CollabView::default());
    h.get_by_label("Close dialogs and menus").click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
    h.state_mut().clear();
    dialog::click_at(&mut h, egui::pos2(60.0, 60.0));
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
    // The dialog closes through the verb, like every overlay.
    h.state_mut().dispatch();
    assert!(!h.state().state.collab_open);
}

#[test]
fn both_themes_draw_the_dialog() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut f = dialog::fixture();
        f.model.collab = view(CollabStatus::Connected, Some("abc"), 3);
        f.state.collab_open = true;
        let mut h = dialog::sized(f, egui::vec2(1280.0, 800.0), theme);
        h.run();
        h.get_by_label("Leave room");
    }
}

// ---- presence ----

#[test]
fn presence_lists_the_peers_with_anonymous_for_empty_names() {
    let mut f = dialog::fixture();
    f.model.collab = view(CollabStatus::Connected, Some("abc"), 3);
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("Anonymous");
    h.get_by_label("p1");
    h.get_by_label("p2");
    let rect = h.get_by_label("p1").rect();
    assert!(rect.right() > 1000.0 && rect.bottom() > 600.0, "bottom right: {rect:?}");
}

#[test]
fn presence_shows_eight_and_the_rest_as_more() {
    let mut f = dialog::fixture();
    f.model.collab = view(CollabStatus::Connected, Some("abc"), 11);
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("p7");
    assert!(h.query_by_label("p8").is_none());
    h.get_by_label("+3 more");
}

#[test]
fn presence_is_hidden_without_a_room_or_peers() {
    for c in [view(CollabStatus::Connected, None, 3), view(CollabStatus::Connected, Some("abc"), 0)] {
        let mut f = dialog::fixture();
        f.model.collab = c;
        let mut h = dialog::harness(f);
        h.run();
        assert!(h.query_by_label("p1").is_none());
        assert!(h.query_by_label("Anonymous").is_none());
    }
}

#[test]
fn presence_is_hidden_in_zen_mode() {
    let mut f = dialog::fixture();
    f.model.collab = view(CollabStatus::Connected, Some("abc"), 3);
    f.state.zen = true;
    let mut h = dialog::harness(f);
    h.run();
    assert!(h.query_by_label("p1").is_none());
}

const TOKEN: &str = "s3cret-room-token";

#[test]
fn the_token_goes_to_join_room_and_is_not_kept_anywhere_else() {
    let mut h = open(dialog::connected(None), view(CollabStatus::Connected, None, 0));
    type_into(&mut h, "Room", "plans");
    type_into(&mut h, "Access token", TOKEN);
    h.get_by_label("Join room").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.join_room", json!({"room": "plans", "token": TOKEN}))]);
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(status(h.state_mut())["room"], json!("plans"));
    // Nothing the session or the model keeps holds it: not the settings, not the ring, not the status.
    let f = h.state();
    let settings = format!("{:?}", f.model.settings);
    let ring = serde_json::to_string(&f.session.ring().recent(1000, false, None)).unwrap();
    assert!(!settings.contains(TOKEN), "settings: {settings}");
    assert!(!ring.contains(TOKEN) && ring.contains("<hidden>"), "ring: {ring}");
    assert!(!status(h.state_mut()).to_string().contains(TOKEN));
}

#[test]
fn the_token_field_is_masked_and_gone_once_in_a_room() {
    let mut h = open(dialog::connected(None), view(CollabStatus::Connected, None, 0));
    type_into(&mut h, "Access token", TOKEN);
    // A plain field exposes what was typed; the token field does not (egui draws dots).
    type_into(&mut h, "Display name", "Ada Lovelace");
    assert!(format!("{:?}", dialog::field(&h, "Display name").accesskit_node().data()).contains("Ada Lovelace"));
    let node = dialog::field(&h, "Access token");
    assert_eq!(node.accesskit_node().role(), egui::accesskit::Role::TextInput);
    assert!(
        !format!("{:?}", node.accesskit_node().data()).contains(TOKEN),
        "a masked field does not expose what was typed"
    );
    // In a room there is no field, and a new dialog starts empty.
    let mut h = open(dialog::connected(Some("abc")), view(CollabStatus::Connected, Some("abc"), 1));
    assert!(h.query_by_label("Access token").is_none());
    h.get_by_label("Leave room").click();
    h.run();
    h.state_mut().dispatch();
    h.state_mut().state.collab_open = true;
    h.state_mut().model.collab = view(CollabStatus::Connected, None, 0);
    h.run();
    h.state_mut().clear();
    h.get_by_label("Join room").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.join_room", json!({"room": "abc"}))], "no token stayed behind");
}

#[test]
fn a_new_room_with_a_token_needs_no_name() {
    let mut h = open(dialog::connected(None), view(CollabStatus::Connected, None, 0));
    type_into(&mut h, "Access token", TOKEN);
    h.get_by_label("Create room").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.join_room", json!({"token": TOKEN}))]);
}

#[test]
fn once_connect_has_chosen_the_server_the_dialog_offers_the_room_and_the_token() {
    // The real flow: `collab.connect` only chooses the server (state `ready`), and the app feeds the view from
    // `collab.status`. The room and the token are for the person to fill in then, not only after a room is joined.
    let mut f = dialog::connected(None);
    let chosen = dialog::run(&mut f, "collab.status", json!({}));
    assert_eq!(chosen["state"], json!("ready"));
    let view = CollabView::from_status(&chosen);
    assert_eq!(view.status, CollabStatus::Disconnected);
    let mut h = open(f, view);
    h.get_by_label("Server chosen. Not in a room");
    h.get_by_label("Disconnect");
    type_into(&mut h, "Room", "plans");
    type_into(&mut h, "Access token", TOKEN);
    h.get_by_label("Join room").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.join_room", json!({"room": "plans", "token": TOKEN}))]);
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(status(h.state_mut())["room"], json!("plans"));
}
