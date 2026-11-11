#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The preferences dialog (`ui.open_preferences`): every control runs `settings.set` / `settings.toggle` with its key
//! and value on the real session, the collaboration profile runs `collab.set_user`, and the layout choice is UI state.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
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
