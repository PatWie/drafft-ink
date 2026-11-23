//! The collaboration dialog and the presence panel.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
        e2e: false,
        invite_link: None,
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
    h.get_by_label("Start a private room").click();
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
    let create = h.get_by_label("Start a private room");
    assert!(create.accesskit_node().is_disabled());
    create.hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(&text);
    h.get_by_label("Start a private room").click();
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
    h.get_by_label("Start a private room").click();
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

// ---- the server of the page, and the invite link ----

fn web_fixture() -> Fixture {
    let mut f = dialog::fixture();
    f.fakes.net.set_origin("wss://canvas.example.com");
    assert!(f.session.load_settings().is_ok());
    f.refresh();
    f
}

#[test]
fn on_the_web_the_dialog_shows_the_server_of_the_page_and_never_localhost() {
    let h = open(web_fixture(), CollabView::default());
    assert_eq!(dialog::field(&h, "Server URL").value().as_deref(), Some("wss://canvas.example.com/ws"));
    h.get_by_label("This is the server of this page.");
    assert!(h.query_by_label_contains("localhost").is_none());
}

#[test]
fn on_native_the_dialog_shows_localhost_and_no_page_server() {
    let h = open(dialog::fixture(), CollabView::default());
    assert_eq!(dialog::field(&h, "Server URL").value().as_deref(), Some("ws://localhost:3030/ws"));
    assert!(h.query_by_label("This is the server of this page.").is_none());
}

#[test]
fn a_server_the_person_chose_is_shown_and_connected_to_instead_of_the_page() {
    let mut f = web_fixture();
    dialog::run(&mut f, "settings.set", json!({"key": "collab.server_url", "value": "wss://relay.example.org/ws"}));
    let mut h = open(f, CollabView::default());
    assert_eq!(dialog::field(&h, "Server URL").value().as_deref(), Some("wss://relay.example.org/ws"));
    assert!(h.query_by_label("This is the server of this page.").is_none());
    h.get_by_label("Connect").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.connect", json!({"url": "wss://relay.example.org/ws"}))]);
}

#[test]
fn the_page_server_connects_through_the_real_command_to_the_page() {
    let mut h = open(web_fixture(), CollabView::default());
    h.get_by_label("Connect").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(status(h.state_mut())["url"], json!("wss://canvas.example.com/ws"));
}

#[test]
fn the_room_field_says_what_each_kind_of_entry_does() {
    let mut h = open(dialog::connected(None), view(CollabStatus::Connected, None, 0));
    h.get_by_label_contains("Empty starts a private room");
    type_into(&mut h, "Room", "Test");
    h.get_by_label_contains("A name makes a plain room");
    h.get_by_label("Join room");
    dialog::field(&h, "Room").focus();
    h.run();
    dialog::field(&h, "Room").type_text(",key");
    h.run();
    h.get_by_label_contains("Joins the private room of this invite link");
}

#[test]
fn a_private_room_shows_its_whole_invite_link_and_a_copy_button() {
    let link = "https://canvas.example.com/#room=abc,KEY";
    let mut c = view(CollabStatus::Connected, Some("abc"), 1);
    c.e2e = true;
    c.invite_link = Some(link.to_owned());
    let mut h = open(dialog::connected(Some("abc")), c);
    assert_eq!(dialog::field(&h, "Invite link").value().as_deref(), Some(link));
    h.get_by_label("Copy invite link").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.share_link", json!({}))]);
}

// ---- sharing the canvas of a tab that has content ----

const SHARE_BUTTON: &str = "Share this canvas";
const EMPTY_ROOM_BUTTON: &str = "Start a new empty room in a new tab";

/// A rectangle on the active tab, so that the tab has content.
fn with_content(mut f: Fixture) -> Fixture {
    dialog::run(&mut f, "element.create", json!({"kind": "rectangle", "bounds": {"x": 0, "y": 0, "w": 50, "h": 40}}));
    f
}

/// The invite link of a private room that another session made (the id is derived from the key).
fn room_link() -> String {
    let mut other = dialog::connected(None);
    dialog::run(&mut other, "collab.join_room", json!({}));
    let shared = dialog::run(&mut other, "collab.share_link", json!({"copy": false, "reveal": true}));
    shared["link"].as_str().expect("the link").to_owned()
}

fn tab_count(h: &mut Harness<'static, Fixture>) -> usize {
    dialog::run(h.state_mut(), "tab.list", json!({}))["tabs"].as_array().map_or(0, Vec::len)
}

#[test]
fn a_tab_with_content_offers_to_share_it_and_to_start_empty_in_a_new_tab() {
    let h = open(with_content(dialog::connected(None)), view(CollabStatus::Connected, None, 0));
    h.get_by_label(SHARE_BUTTON);
    h.get_by_label(EMPTY_ROOM_BUTTON);
    h.get_by_label_contains("The undo history of this tab is cleared");
    assert!(h.query_by_label("Start a private room").is_none(), "the plain start is replaced");
}

#[test]
fn an_empty_tab_keeps_the_plain_start_button() {
    let h = open(dialog::connected(None), view(CollabStatus::Connected, None, 0));
    h.get_by_label("Start a private room");
    assert!(h.query_by_label(SHARE_BUTTON).is_none());
    assert!(h.query_by_label(EMPTY_ROOM_BUTTON).is_none());
}

#[test]
fn share_this_canvas_joins_a_new_room_with_the_content_merged_and_no_room_given() {
    let mut h = open(with_content(dialog::connected(None)), view(CollabStatus::Connected, None, 0));
    h.get_by_label(SHARE_BUTTON).click();
    h.run();
    assert_eq!(calls(&h), [call("collab.join_room", json!({"on_local": "merge"}))]);
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    let status = status(h.state_mut());
    assert_eq!((status["e2e"].clone(), status["room"].is_string()), (json!(true), true), "{status}");
    assert_eq!(dialog::element_count(h.state_mut()), 1, "the canvas is still there");
}

#[test]
fn share_this_canvas_sends_the_token_the_person_typed() {
    let mut h = open(with_content(dialog::connected(None)), view(CollabStatus::Connected, None, 0));
    type_into(&mut h, "Access token", TOKEN);
    h.get_by_label(SHARE_BUTTON).click();
    h.run();
    assert_eq!(calls(&h), [call("collab.join_room", json!({"token": TOKEN, "on_local": "merge"}))]);
}

#[test]
fn a_new_empty_room_opens_a_new_tab_first_and_leaves_the_canvas_alone() {
    let mut h = open(with_content(dialog::connected(None)), view(CollabStatus::Connected, None, 0));
    h.get_by_label(EMPTY_ROOM_BUTTON).click();
    h.run();
    assert_eq!(calls(&h), [call("tab.new", json!({})), call("collab.join_room", json!({}))]);
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(tab_count(&mut h), 2);
    assert_eq!(dialog::element_count(h.state_mut()), 0, "the room starts empty in the new tab");
    assert_eq!(status(h.state_mut())["e2e"], json!(true));
}

#[test]
fn a_room_link_on_a_tab_with_content_asks_before_anything_is_merged() {
    let mut h = open(with_content(dialog::connected(None)), view(CollabStatus::Connected, None, 0));
    let link = room_link();
    type_into(&mut h, "Room", &link);
    h.get_by_label("Join room").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert!(h.state().state.room_choice.is_some(), "{:?}", report.ran);
    assert_eq!(report.refusals().count(), 1, "the engine refused the silent merge");
    assert!(h.state().state.toasts.is_empty(), "the question is the answer, not a toast");
    assert_eq!(status(h.state_mut())["room"], Value::Null, "nothing joined yet");
    h.run();
    h.get_by_label("Add this canvas to the room, or open the room in a new tab?");
    h.get_by_label("Add this canvas to the room");
    h.get_by_label("Open in a new tab");
    assert!(
        !h.state().state.room_choice.as_ref().is_some_and(|c| c.kind.adds_by_default()),
        "a new tab is the default"
    );
}

#[test]
fn the_answer_open_in_a_new_tab_joins_from_an_empty_tab_and_keeps_the_canvas_apart() {
    let mut h = open(with_content(dialog::connected(None)), view(CollabStatus::Connected, None, 0));
    let link = room_link();
    type_into(&mut h, "Room", &link);
    h.get_by_label("Join room").click();
    h.run();
    h.state_mut().dispatch();
    h.run();
    h.state_mut().clear();
    h.get_by_label("Open in a new tab").click();
    h.run();
    let ids: Vec<String> = h.state().calls().iter().map(|c| c.id.clone()).collect();
    assert_eq!(ids, ["tab.new", "collab.join_room"]);
    assert_eq!(h.state().calls()[1].args["room"], json!(link));
    assert!(h.state().calls()[1].args.get("on_local").is_none(), "nothing is merged");
    assert!(h.state().state.room_choice.is_none(), "the question is answered");
}

#[test]
fn the_answer_add_this_canvas_merges_it_into_the_room() {
    let mut h = open(with_content(dialog::connected(None)), view(CollabStatus::Connected, None, 0));
    let link = room_link();
    type_into(&mut h, "Room", &link);
    h.get_by_label("Join room").click();
    h.run();
    h.state_mut().dispatch();
    h.run();
    h.state_mut().clear();
    h.get_by_label("Add this canvas to the room").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.join_room", json!({"room": link, "on_local": "merge"}))]);
}

#[test]
fn cancel_closes_the_question_and_runs_nothing() {
    let mut h = open(with_content(dialog::connected(None)), view(CollabStatus::Connected, None, 0));
    let link = room_link();
    type_into(&mut h, "Room", &link);
    h.get_by_label("Join room").click();
    h.run();
    h.state_mut().dispatch();
    h.run();
    h.state_mut().clear();
    h.get_by_label("Cancel").click();
    h.run();
    assert!(h.state().calls().is_empty());
    assert!(h.state().state.room_choice.is_none());
}

#[test]
fn the_palette_row_of_the_same_action_is_called_share_this_canvas() {
    let f = dialog::fixture();
    let rows = drafftink_ui::surfaces::palette::rows(&f.model);
    let row = rows.iter().find(|r| r.title == SHARE_BUTTON).expect("a palette row");
    assert_eq!(row.id, "collab.join_room");
}

// ---- a document dropped on a tab in a room ----

/// A drafft.ink document with one rectangle, as a file.
fn document_file() -> Value {
    use base64::Engine as _;
    let mut other = dialog::fixture();
    dialog::run(
        &mut other,
        "element.create",
        json!({"kind": "rectangle", "bounds": {"x": 0, "y": 0, "w": 30, "h": 30}}),
    );
    let out = dialog::run(&mut other, "doc.export", json!({"format": "json", "scope": "all"}));
    let text = out["bytes"]["base64"].as_str().expect("inline bytes").to_owned();
    assert!(base64::engine::general_purpose::STANDARD.decode(&text).is_ok());
    json!({"items": [{"name": "plan.drafftink", "bytes": {"base64": text, "name": "plan.drafftink"}}], "at": [400.0, 300.0]})
}

fn asking_about_a_drop() -> Harness<'static, Fixture> {
    let mut f = with_content(dialog::connected(Some("plans")));
    f.state.ask_drop_in_room(
        drafftink_ui::Call::new(Origin::Ui, "doc.drop", document_file()),
        vec!["plan.drafftink".to_owned()],
    );
    let mut h = dialog::harness(f);
    h.run();
    h
}

#[test]
fn a_document_dropped_on_a_room_tab_asks_and_adding_is_the_default() {
    let h = asking_about_a_drop();
    h.get_by_label("Add `plan.drafftink` to this room, or open it in a new tab?");
    h.get_by_label("Add to this room");
    h.get_by_label("Open in a new tab");
    assert!(h.state().state.room_choice.as_ref().is_some_and(|c| c.kind.adds_by_default()));
}

#[test]
fn adding_the_dropped_document_runs_doc_drop_with_merge_at_the_drop_point_in_one_undo_step() {
    let mut h = asking_about_a_drop();
    let before = dialog::element_count(h.state_mut());
    h.get_by_label("Add to this room").click();
    h.run();
    let calls = h.state().calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "doc.drop");
    assert_eq!(calls[0].args["documents"], json!("merge"));
    assert_eq!(calls[0].args["at"], json!([400.0, 300.0]));
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(dialog::element_count(h.state_mut()), before + 1);
    assert_eq!(tab_count(&mut h), 1, "no new tab");
    dialog::run(h.state_mut(), "history.undo", json!({}));
    assert_eq!(dialog::element_count(h.state_mut()), before, "one undo step takes the whole file back");
}

#[test]
fn opening_the_dropped_document_in_a_new_tab_leaves_the_room_alone() {
    let mut h = asking_about_a_drop();
    h.get_by_label("Open in a new tab").click();
    h.run();
    assert_eq!(h.state().calls()[0].args["documents"], json!("new_tab"));
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(tab_count(&mut h), 2);
}

#[test]
fn opening_a_file_in_a_room_tab_asks_instead_of_a_refusal_toast() {
    let mut f = dialog::connected(Some("plans"));
    dialog::store_document(&f, "plan", 2);
    f.state.collab_open = false;
    let mut h = dialog::harness(f);
    let open =
        drafftink_ui::Call::new(Origin::Ui, "doc.open", json!({"source": {"stored": "plan"}, "mode": "replace"}));
    h.state_mut().actions.push(drafftink_ui::UiAction::Command(open));
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 1);
    assert!(h.state().state.room_choice.is_some());
    assert!(h.state().state.toasts.is_empty());
    h.run();
    h.get_by_label("Add this file to this room, or open it in a new tab?");
    h.get_by_label("Add to this room").click();
    h.run();
    assert_eq!(h.state().calls()[0].args["mode"], json!("merge"));
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(dialog::element_count(h.state_mut()), 2);
}
