//! The collaboration dialog and the presence panel.

mod common;
mod synth;

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
    let mut h = synth::harness(f);
    h.run();
    h
}

fn view(status: CollabStatus, room: Option<&str>, peers: usize) -> CollabView {
    CollabView {
        status,
        room: room.map(str::to_owned),
        peers: (0..peers)
            .map(|i| Peer {
                name: if i == 0 { String::new() } else { format!("p{i}") },
                color: "#22c55e".into(),
                has_cursor: false,
            })
            .collect(),
    }
}

fn calls(h: &Harness<'static, Fixture>) -> Vec<(String, Value, Origin)> {
    h.state().calls().into_iter().map(|c| (c.id.clone(), c.args.clone(), c.origin)).collect()
}

fn call(id: &str, args: Value) -> (String, Value, Origin) {
    (id.to_owned(), args, Origin::Ui)
}

fn type_into(h: &mut Harness<'static, Fixture>, label: &str, text: &str) {
    synth::field(h, label).focus();
    h.run();
    synth::field(h, label).type_text(text);
    h.run();
}

fn is_on(h: &Harness<'static, Fixture>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

#[test]
fn connect_sends_the_url_of_the_setting() {
    let mut h = open(synth::fixture(), CollabView::default());
    h.get_by_label("Not connected");
    let url = h.state().model.setting_str("collab.server_url").map(str::to_owned);
    assert_eq!(url.as_deref(), Some("ws://localhost:3030/ws"));
    h.get_by_label("Connect").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.connect", json!({"url": "ws://localhost:3030/ws"}))]);
}

#[test]
fn connect_sends_the_edited_url() {
    let mut h = open(synth::fixture(), CollabView::default());
    synth::field(&h, "Server URL").focus();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run();
    synth::field(&h, "Server URL").type_text("wss://example.org/ws");
    h.run();
    h.get_by_label("Connect").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.connect", json!({"url": "wss://example.org/ws"}))]);
}

#[test]
fn connecting_shows_disconnect() {
    let mut h = open(synth::fixture(), view(CollabStatus::Connecting, None, 0));
    h.get_by_label("Connecting...");
    assert!(h.query_by_label("Connect").is_none());
    h.get_by_label("Disconnect").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.disconnect", json!({}))]);
}

#[test]
fn connected_without_a_room_can_join_or_create_one() {
    let mut h = open(synth::fixture(), view(CollabStatus::Connected, None, 0));
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
    let mut h = open(synth::fixture(), view(CollabStatus::Connected, Some("abc"), 2));
    h.get_by_label("Connected to room: abc");
    h.get_by_label("2 people online");
    assert!(h.query_by_label("Join room").is_none());
    h.get_by_label("Leave room").click();
    h.run();
    assert_eq!(calls(&h), [call("collab.leave_room", json!({}))]);
}

#[test]
fn failure_shows_the_reason_and_allows_a_retry() {
    let mut h = open(synth::fixture(), view(CollabStatus::Failed("timed out".into()), None, 0));
    h.get_by_label("Connection error: timed out");
    h.get_by_label("Connect").click();
    h.run();
    assert_eq!(calls(&h).len(), 1);
}

#[test]
fn the_name_is_sent_once_when_enter_is_pressed() {
    let mut h = open(synth::fixture(), CollabView::default());
    type_into(&mut h, "Display name", "Ada");
    assert!(h.state().calls().is_empty(), "typing sends nothing");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(calls(&h), [call("collab.set_user", json!({"name": "Ada"}))]);
    // Leaving the field afterwards does not send it again.
    synth::click_canvas(&mut h);
    h.run();
    assert_eq!(calls(&h).len(), 1);
}

#[test]
fn the_name_is_sent_when_focus_is_lost() {
    let mut h = open(synth::fixture(), CollabView::default());
    type_into(&mut h, "Display name", "Bob");
    h.get_by_label("Cursor colour").click();
    h.run();
    h.run();
    assert_eq!(calls(&h), [call("collab.set_user", json!({"name": "Bob"}))]);
}

#[test]
fn an_unchanged_name_is_not_sent() {
    let mut f = synth::fixture();
    f.model.settings.insert("collab.user_name".into(), json!("Ada"));
    let mut h = open(f, CollabView::default());
    synth::field(&h, "Display name").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    synth::click_canvas(&mut h);
    assert!(h.state().calls().is_empty(), "{:?}", h.state().calls());
}

#[test]
fn nine_colours_send_set_user_and_the_current_one_is_marked() {
    let mut f = synth::fixture();
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
    let h = open(synth::fixture(), CollabView::default());
    assert_eq!(h.state().model.setting_str("collab.user_color"), Some("#6366f1"));
    assert!(is_on(&h, "Indigo"));
}

#[test]
fn unregistered_collab_commands_are_greyed_with_the_build_text() {
    // The real engine registers no collab.* command yet; when it does, use the view-only test below.
    let f = Fixture::new();
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
    let mut f = synth::fixture();
    f.session.view_only = true;
    f.refresh();
    let text = f.model.refusal_text("collab.connect").expect("refuses in a view-only document");
    assert!(!text.contains("not available in this build"), "{text}");
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
}

#[test]
fn the_x_and_the_backdrop_close_the_dialog() {
    let mut h = open(synth::fixture(), CollabView::default());
    h.get_by_label("Close dialogs and menus").click();
    h.run();
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
    h.state_mut().clear();
    synth::click_at(&mut h, egui::pos2(60.0, 60.0));
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
    // The dialog closes through the verb, like every overlay.
    h.state_mut().dispatch();
    assert!(!h.state().state.collab_open);
}

#[test]
fn both_themes_draw_the_dialog() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut f = synth::fixture();
        f.model.collab = view(CollabStatus::Connected, Some("abc"), 3);
        f.state.collab_open = true;
        let mut h = synth::sized(f, egui::vec2(1280.0, 800.0), theme);
        h.run();
        h.get_by_label("Leave room");
    }
}

// ---- presence ----

#[test]
fn presence_lists_the_peers_with_anonymous_for_empty_names() {
    let mut f = synth::fixture();
    f.model.collab = view(CollabStatus::Connected, Some("abc"), 3);
    let mut h = synth::harness(f);
    h.run();
    h.get_by_label("Anonymous");
    h.get_by_label("p1");
    h.get_by_label("p2");
    let rect = h.get_by_label("p1").rect();
    assert!(rect.right() > 1000.0 && rect.bottom() > 600.0, "bottom right: {rect:?}");
}

#[test]
fn presence_shows_eight_and_the_rest_as_more() {
    let mut f = synth::fixture();
    f.model.collab = view(CollabStatus::Connected, Some("abc"), 11);
    let mut h = synth::harness(f);
    h.run();
    h.get_by_label("p7");
    assert!(h.query_by_label("p8").is_none());
    h.get_by_label("+3 more");
}

#[test]
fn presence_is_hidden_without_a_room_or_peers() {
    for c in [view(CollabStatus::Connected, None, 3), view(CollabStatus::Connected, Some("abc"), 0)] {
        let mut f = synth::fixture();
        f.model.collab = c;
        let mut h = synth::harness(f);
        h.run();
        assert!(h.query_by_label("p1").is_none());
        assert!(h.query_by_label("Anonymous").is_none());
    }
}

#[test]
fn presence_is_hidden_in_zen_mode() {
    let mut f = synth::fixture();
    f.model.collab = view(CollabStatus::Connected, Some("abc"), 3);
    f.state.zen = true;
    let mut h = synth::harness(f);
    h.run();
    assert!(h.query_by_label("p1").is_none());
}
