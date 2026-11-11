#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The preferences dialog (`ui.open_preferences`): every control runs `settings.set` / `settings.toggle` with its key
//! and value on the real session, the collaboration profile runs `collab.set_user`, and the layout choice is UI state.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_ui::AgentServerView;
use drafftink_ui::state::LayoutPreference;
use drafftink_ui::{Call, Verb};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

type H = Harness<'static, Fixture>;

fn open() -> H {
    let mut f = dialog::fixture();
    f.state.apply_verb(Verb::OpenPreferences, &mut Vec::new());
    let mut h = dialog::harness(f);
    h.run();
    h
}

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

fn setting(h: &H, key: &str) -> Value {
    h.state().model.settings.get(key).cloned().unwrap_or(Value::Null)
}

/// Clicks the control, checks the one call it made, runs it and returns the new value of the key.
fn press(h: &mut H, label: &str, id: &str, args: Value) {
    h.state_mut().clear();
    // The dialog scrolls, and the bottom bar, below y 735, has icon buttons of the same names.
    // A control below the visible part of the dialog is the one that is not in the bar: scroll to the last match.
    let node = h.get_all_by_label(label).last().unwrap_or_else(|| panic!("no {label}"));
    node.scroll_to_me();
    h.run();
    let node = h.get_all_by_label(label).find(|n| n.rect().min.y < 735.0).unwrap();
    node.click();
    h.run();
    assert_eq!(calls(h), vec![Call::new(Origin::Ui, id, args)], "{label}");
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{label}: {:?}", report.ran);
    h.run();
}

#[test]
fn the_theme_buttons_set_the_theme() {
    let mut h = open();
    for (label, value) in [("Theme: Dark", "dark"), ("Theme: Light", "light"), ("Theme: System", "system")] {
        press(&mut h, label, "settings.set", json!({ "key": "theme", "value": value }));
        assert_eq!(setting(&h, "theme"), json!(value));
    }
}

/// The first font of the UI text family: the one the menus and bars are set in.
fn ui_font(h: &H) -> String {
    h.ctx.fonts(|f| f.definitions().families[&egui::FontFamily::Proportional][0].clone())
}

#[test]
fn gelpen_is_the_interface_font_until_another_is_chosen() {
    let h = open();
    assert_eq!(setting(&h, "ui.font"), json!("gelpen"));
    assert_eq!(ui_font(&h), drafftink_ui::theme::HAND_FONT);
    h.get_by_label("Interface font: GelPen");
}

#[test]
fn the_interface_font_buttons_set_ui_font_and_the_text_follows() {
    let mut h = open();
    for (label, value, family) in [
        ("Interface font: Vanilla Extract", "vanilla_extract", drafftink_ui::theme::PREVIEW_VANILLA),
        ("Interface font: GelPen Serif", "gelpen_serif", drafftink_ui::theme::PREVIEW_SERIF),
        ("Interface font: GelPen", "gelpen", drafftink_ui::theme::HAND_FONT),
    ] {
        press(&mut h, label, "settings.set", json!({ "key": "ui.font", "value": value }));
        assert_eq!(setting(&h, "ui.font"), json!(value));
        assert_eq!(ui_font(&h), family, "the text of the next frame is set in {value}");
    }
}

/// The width of a text in an egui font family, at the size of the body text.
fn width_in(h: &H, family: &str, text: &str) -> f32 {
    let font = egui::FontId::new(drafftink_ui::theme::TextSize::Body.points(), egui::FontFamily::Name(family.into()));
    h.ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, egui::Color32::BLACK).size().x)
}

#[test]
fn plex_sans_is_an_interface_font_with_real_semibold_headings() {
    let mut h = open();
    assert!(drafftink_ui::theme::UiFontChoice::PlexSans.available());
    press(&mut h, "Interface font: IBM Plex Sans", "settings.set", json!({ "key": "ui.font", "value": "plex_sans" }));
    assert_eq!(setting(&h, "ui.font"), json!("plex_sans"));
    assert_eq!(ui_font(&h), drafftink_ui::theme::PREVIEW_PLEX, "the text of the next frame is set in Plex");
    // The UI really draws with it: the same words are another width than in GelPen.
    let text = "Handgloves and hamburgefonts";
    let plex = width_in(&h, drafftink_ui::theme::PREVIEW_PLEX, text);
    let gelpen = width_in(&h, drafftink_ui::theme::HAND_FONT, text);
    assert!((plex - gelpen).abs() > 1.0, "Plex {plex} and GelPen {gelpen} differ");
    // Headings are the real semibold of the same variable file, not GelPen and not a synthetic bold.
    let heading = drafftink_ui::theme::TextSize::Heading.hand();
    assert_eq!(heading.family, egui::FontFamily::Name("plex_sans_semibold".into()));
    let semibold = width_in(&h, "plex_sans_semibold", text);
    assert!(semibold > plex + 0.5, "semibold {semibold} is wider than regular {plex}");
    // Choosing another font brings the hand-drawn headings back.
    press(&mut h, "Interface font: GelPen", "settings.set", json!({ "key": "ui.font", "value": "gelpen" }));
    assert_eq!(ui_font(&h), drafftink_ui::theme::HAND_FONT);
    assert_eq!(drafftink_ui::theme::TextSize::Heading.hand().family, egui::FontFamily::Name("gelpen".into()));
}

#[test]
fn noto_sans_is_offered_only_where_the_build_has_the_file() {
    let h = open();
    let offered = h.query_by_label("Interface font: Noto Sans").is_some();
    assert_eq!(offered, drafftink_ui::theme::UiFontChoice::NotoSans.available());
    // The five words are the ones the canvas takes for `font_family`; a sixth is refused by the engine.
    let words: Vec<&str> = drafftink_ui::theme::UiFontChoice::ALL.iter().map(|c| c.setting()).collect();
    assert_eq!(words, ["gelpen", "gelpen_serif", "vanilla_extract", "noto_sans", "plex_sans"]);
}

#[test]
fn grid_style_buttons_set_the_grid_style() {
    let mut h = open();
    press(&mut h, "Grid: Dots", "settings.set", json!({ "key": "view.grid_style", "value": "dots" }));
    assert_eq!(setting(&h, "view.grid_style"), json!("dots"));
}

#[test]
fn the_snapping_switches_toggle_their_keys() {
    let mut h = open();
    for key_label in
        [("snap.grid", "Snap to grid"), ("snap.smart_guides", "Smart guides"), ("snap.angle", "Angle snap")]
    {
        let (key, label) = key_label;
        let was = setting(&h, key);
        press(&mut h, label, "settings.toggle", json!({ "key": key }));
        assert_ne!(setting(&h, key), was, "{key}");
    }
    press(&mut h, "Angle step: 45 deg", "settings.set", json!({ "key": "snap.angle_step", "value": 45.0 }));
    assert_eq!(setting(&h, "snap.angle_step").as_f64(), Some(45.0));
}

#[test]
fn input_arrow_and_frame_keys() {
    let mut h = open();
    press(&mut h, "Arrow keys move by: 5", "settings.set", json!({ "key": "input.nudge_step", "value": 5.0 }));
    assert_eq!(setting(&h, "input.nudge_step").as_f64(), Some(5.0));
    press(
        &mut h,
        "Box select takes what is: Inside",
        "settings.set",
        json!({ "key": "input.box_select", "value": "contain" }),
    );
    assert_eq!(setting(&h, "input.box_select"), json!("contain"));
    for (key, label) in [
        ("arrow.binding", "Arrows bind to shapes"),
        ("arrow.midpoint_snap", "Snap to side midpoints"),
        ("frame.clip", "Clip frame contents"),
        ("frame.show_names", "Frame names"),
        ("frame.outline", "Frame outline"),
    ] {
        let was = setting(&h, key);
        press(&mut h, label, "settings.toggle", json!({ "key": key }));
        assert_ne!(setting(&h, key), was, "{key}");
    }
}

#[test]
fn the_export_defaults() {
    let mut h = open();
    press(&mut h, "Export scale: 3x", "settings.set", json!({ "key": "export.scale", "value": 3.0 }));
    assert_eq!(setting(&h, "export.scale").as_f64(), Some(3.0));
    press(&mut h, "Export padding: 50", "settings.set", json!({ "key": "export.padding", "value": 50.0 }));
    assert_eq!(setting(&h, "export.padding").as_f64(), Some(50.0));
    for (key, label) in
        [("export.background", "Background"), ("export.dark", "Dark"), ("export.embed_scene", "Embed scene")]
    {
        let was = setting(&h, key);
        press(&mut h, label, "settings.toggle", json!({ "key": key }));
        assert_ne!(setting(&h, key), was, "{key}");
    }
}

#[test]
fn autosave_and_history_depth() {
    let mut h = open();
    let was = setting(&h, "autosave.enabled");
    press(&mut h, "Autosave", "settings.toggle", json!({ "key": "autosave.enabled" }));
    assert_ne!(setting(&h, "autosave.enabled"), was);
    press(&mut h, "Autosave every: 30 s", "settings.set", json!({ "key": "autosave.interval_s", "value": 30 }));
    assert_eq!(setting(&h, "autosave.interval_s"), json!(30));
    press(&mut h, "Undo steps kept: 500", "settings.set", json!({ "key": "history.depth", "value": 500 }));
    assert_eq!(setting(&h, "history.depth"), json!(500));
    let was = setting(&h, "clipboard.excalidraw_compat");
    press(&mut h, "Copy for Excalidraw too", "settings.toggle", json!({ "key": "clipboard.excalidraw_compat" }));
    assert_ne!(setting(&h, "clipboard.excalidraw_compat"), was);
}

#[test]
fn the_collaboration_name_and_colour_run_collab_set_user() {
    let mut h = open();
    dialog::field(&h, "Display name").scroll_to_me();
    h.run();
    press(&mut h, "Blue", "collab.set_user", json!({ "color": "#3b82f6" }));
    assert_eq!(setting(&h, "collab.user_color"), json!("#3b82f6"));
    dialog::field(&h, "Display name").focus();
    h.run();
    dialog::field(&h, "Display name").type_text("Ada");
    h.run();
    h.state_mut().clear();
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "collab.set_user", json!({ "name": "Ada" }))]);
    h.state_mut().dispatch();
    assert_eq!(setting(&h, "collab.user_name"), json!("Ada"));
}

#[test]
fn the_layout_choice_is_ui_state_and_runs_no_command() {
    let mut h = open();
    h.state_mut().clear();
    h.get_by_label("Touch").click();
    h.run();
    assert!(calls(&h).is_empty());
    assert_eq!(h.state().state.layout, LayoutPreference::Touch);
    h.get_by_label("Desktop").click();
    h.run();
    assert_eq!(h.state().state.layout, LayoutPreference::Desktop);
    h.get_by_label("Automatic").click();
    h.run();
    assert_eq!(h.state().state.layout, LayoutPreference::Auto);
}

#[test]
fn the_menu_opens_the_dialog_and_escape_closes_it() {
    let mut f = dialog::fixture();
    f.state.menu_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h.state_mut().clear();
    h.get_by_label("Preferences…").click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), vec![Verb::OpenPreferences]);
    h.state_mut().dispatch();
    assert!(h.state().state.preferences_open);
    assert!(h.state().state.modal_open());
    h.state_mut().state.close_overlays();
    assert!(!h.state().state.preferences_open);
}

fn open_with(view: Option<AgentServerView>) -> H {
    let mut f = dialog::fixture();
    f.model.agent_server = view;
    f.state.apply_verb(Verb::OpenPreferences, &mut Vec::new());
    let mut h = dialog::harness(f);
    h.run();
    h
}

#[test]
fn the_web_has_no_agents_section() {
    let h = open_with(None);
    assert!(h.query_by_label("Serve MCP over HTTP").is_none());
}

#[test]
fn the_desktop_switch_toggles_the_setting_and_shows_where_to_connect() {
    let mut h = open_with(Some(AgentServerView::default()));
    assert_eq!(setting(&h, "agent.mcp_http"), json!(false), "off by default");
    press(&mut h, "Serve MCP over HTTP", "settings.toggle", json!({ "key": "agent.mcp_http" }));
    assert_eq!(setting(&h, "agent.mcp_http"), json!(true));

    h.state_mut().model.agent_server = Some(AgentServerView {
        endpoint: Some("http://127.0.0.1:3737/mcp".to_owned()),
        token_file: Some("/run/user/1000/drafftink/mcp-http/3737.token".to_owned()),
        problem: None,
    });
    h.run();
    assert!(h.query_by_label_contains("http://127.0.0.1:3737/mcp").is_some());
    assert!(h.query_by_label_contains("3737.token").is_some());

    h.state_mut().model.agent_server =
        Some(AgentServerView { problem: Some("Port 3737 is in use.".to_owned()), ..AgentServerView::default() });
    h.run();
    assert!(h.query_by_label_contains("Port 3737 is in use.").is_some());
}
