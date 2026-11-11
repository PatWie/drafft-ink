//! Tests of the `settings` family: availability, exact refusal text, the effect on the session, the
//! stored record, and loading it again. No UI: a session over the fake ports.

use serde_json::{Value, json};

use super::keys::{self, KEYS};
use super::store;
use crate::ports::Ns;
use crate::ports::fake::Fakes;
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::Redraw;
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

fn stored(fakes: &Fakes) -> Option<Value> {
    fakes.storage.bytes(Ns::Settings, "settings").map(|b| serde_json::from_slice(&b).unwrap())
}

fn value(s: &Session, key: &str) -> Value {
    s.settings.get(key).cloned().unwrap()
}

const ALL: [&str; 5] = ["settings.get", "settings.set", "settings.toggle", "settings.cycle", "settings.reset"];

// ---- availability ----

#[test]
fn settings_commands_are_available_in_every_mode_and_never_touch_history() {
    let (mut s, _f) = testing::session();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    s.view_only = true;
    s.text_edit = Some(crate::state::TextEdit { element: None, draft: true });
    for id in ALL {
        assert!(s.availability(id).is_ok(), "{id} must be available in view-only mode and during a text edit");
    }
    s.view_only = false;
    s.text_edit = None;
    run(&mut s, "settings.set", json!({"key": "snap.grid", "value": true}));
    run(&mut s, "settings.toggle", json!({"key": "snap.angle"}));
    run(&mut s, "settings.cycle", json!({"key": "view.grid_style"}));
    run(&mut s, "settings.reset", json!({"all": true}));
    assert_eq!(s.tab().doc.undo_depth(), 0, "settings are never undoable");
}

#[test]
fn settings_commands_work_in_view_only_mode() {
    let (mut s, _f) = testing::session();
    s.view_only = true;
    run(&mut s, "settings.set", json!({"key": "theme", "value": "dark"}));
    assert_eq!(value(&s, "theme"), json!("dark"));
}

// ---- the table of keys ----

#[test]
fn every_key_default_passes_its_own_check_and_the_table_matches_the_catalog() {
    for k in KEYS {
        let d = k.default_value();
        assert_eq!(k.normalize(&d), Ok(d.clone()), "default of `{}`", k.name);
    }
    let names: Vec<&str> = KEYS.iter().map(|k| k.name).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "a key is listed twice");
    // The keys of docs/commands.md §5, plus `theme`.
    for k in [
        "view.grid_style", "snap.grid", "snap.smart_guides", "snap.angle", "snap.angle_step", "export.scale",
        "export.padding", "export.background", "export.dark", "export.embed_scene", "doc.default_background",
        "input.nudge_step", "input.box_select", "style.defaults", "arrow.binding", "arrow.midpoint_snap",
        "arrow.type", "arrow.start_head", "arrow.end_head",
        "frame.clip", "frame.show_names", "frame.outline", "history.depth", "autosave.enabled",
        "autosave.interval_s", "clipboard.excalidraw_compat", "collab.server_url", "collab.user_name",
        "collab.user_color", "doc.recent", "theme",
    ] {
        assert!(names.contains(&k), "`{k}` is missing from the table");
    }
    assert_eq!(names.len(), 31);
}

#[test]
fn a_new_session_has_the_decided_defaults() {
    let s = testing::headless_session();
    assert_eq!(value(&s, "history.depth"), json!(200), "decision §9 item 3");
    assert_eq!(s.settings.history_depth(), 200);
    assert_eq!(value(&s, "theme"), json!("system"), "decision §9 item 13");
    assert_eq!(value(&s, "view.grid_style"), json!("lines"));
    assert_eq!(value(&s, "snap.angle_step"), json!(15));
    assert_eq!(value(&s, "export.scale"), json!(2));
    assert_eq!(value(&s, "doc.default_background"), json!("#ffffff"));
    assert_eq!(value(&s, "collab.server_url"), json!("ws://localhost:3030/ws"));
    assert_eq!(value(&s, "doc.recent"), json!([]));
    assert_eq!(s.settings.revision, 0);
}

// ---- settings.get ----

#[test]
fn get_returns_every_setting_or_one_with_its_default_and_what_it_accepts() {
    let mut s = testing::headless_session();
    let all = run(&mut s, "settings.get", json!({}));
    assert_eq!(all["settings"].as_object().unwrap().len(), KEYS.len());
    assert_eq!(all["settings"]["snap.angle_step"], json!(15));
    assert_eq!(all["changed"], json!(false));
    run(&mut s, "settings.set", json!({"key": "snap.angle_step", "value": 30}));
    let one = run(&mut s, "settings.get", json!({"key": "snap.angle_step"}));
    assert_eq!(one["settings"], json!({"snap.angle_step": 30}));
    assert_eq!(one["default"], json!(15));
    assert_eq!(one["accepts"], json!("a number from 1 to 90"));
    assert!(one["summary"].as_str().unwrap().contains("Angle step"));
}

#[test]
fn get_of_an_unknown_key_names_the_closest_one() {
    let mut s = testing::headless_session();
    let r = refuse(&mut s, "settings.get", json!({"key": "snap.gird"}));
    assert_eq!(r.code, "unknown_setting");
    assert_eq!(
        r.message,
        "Unknown setting `snap.gird`. Did you mean `snap.grid`? List settings with `settings.get`."
    );
    let r = refuse(&mut s, "settings.get", json!({"key": "qqqqqqqqqqqqqqqqqqqqqqqq"}));
    assert_eq!(r.message, "Unknown setting `qqqqqqqqqqqqqqqqqqqqqqqq`. List settings with `settings.get`.");
}

// ---- settings.set ----

#[test]
fn set_applies_at_once_and_stores_only_what_differs_from_the_default() {
    let (mut s, f) = testing::session();
    let out = run(&mut s, "settings.set", json!({"key": "snap.grid", "value": true}));
    assert_eq!(out["changed"], json!(true));
    assert_eq!(out["previous"], json!(false));
    assert_eq!(out["default"], json!(false));
    assert!(s.settings.bool_or("snap.grid", false), "live in the session");
    assert_eq!(stored(&f), Some(json!({"schema": 1, "values": {"snap.grid": true}})));
    run(&mut s, "settings.set", json!({"key": "theme", "value": "dark"}));
    assert_eq!(stored(&f), Some(json!({"schema": 1, "values": {"snap.grid": true, "theme": "dark"}})));
    // Back to the default: the key leaves the record, and an empty record is deleted.
    run(&mut s, "settings.set", json!({"key": "snap.grid", "value": false}));
    run(&mut s, "settings.set", json!({"key": "theme", "value": "system"}));
    assert_eq!(stored(&f), None);
}

#[test]
fn the_same_value_is_ok_unchanged_and_writes_nothing() {
    let (mut s, f) = testing::session();
    let rev = s.settings.revision;
    let out = run(&mut s, "settings.set", json!({"key": "snap.grid", "value": false}));
    assert_eq!(out["changed"], json!(false));
    assert_eq!(s.settings.revision, rev);
    assert_eq!(stored(&f), None);
    assert!(s.ring().iter().last().is_some_and(|e| e.id == "settings.set"));
    // 15.0 is the same number as 15.
    let out = run(&mut s, "settings.set", json!({"key": "snap.angle_step", "value": 15.0}));
    assert_eq!(out["changed"], json!(false));
}

#[test]
fn a_change_requests_a_redraw_and_an_unchanged_value_does_not() {
    let mut s = testing::headless_session();
    s.take_dirty();
    run(&mut s, "settings.set", json!({"key": "view.grid_style", "value": "dots"}));
    assert_eq!(s.take_dirty(), Some(Redraw::Scene));
    run(&mut s, "settings.set", json!({"key": "view.grid_style", "value": "dots"}));
    assert_eq!(s.take_dirty(), None);
}

#[test]
fn set_refuses_a_value_of_the_wrong_type_or_range_with_the_exact_text() {
    let mut s = testing::headless_session();
    let cases = [
        ("snap.angle_step", json!(100), "Setting `snap.angle_step` must be a number from 1 to 90, and got the number 100."),
        ("snap.angle_step", json!("fast"), "Setting `snap.angle_step` must be a number from 1 to 90, and got the text \"fast\"."),
        ("theme", json!("blue"), "Setting `theme` must be one of `light`, `dark` or `system`, and got the text \"blue\"."),
        ("history.depth", json!(5), "Setting `history.depth` must be a whole number from 10 to 1000, and got the number 5."),
        ("history.depth", json!(10.5), "Setting `history.depth` must be a whole number from 10 to 1000, and got the number 10.5."),
        ("snap.grid", json!("yes"), "Setting `snap.grid` must be true or false, and got the text \"yes\"."),
        ("snap.grid", json!(null), "Setting `snap.grid` must be true or false, and got null."),
        ("export.scale", json!(0.1), "Setting `export.scale` must be a number from 0.25 to 4, and got the number 0.1."),
    ];
    for (key, v, text) in cases {
        let before = s.fingerprint();
        let r = refuse(&mut s, "settings.set", json!({"key": key, "value": v}));
        assert_eq!(r.code, "bad_args", "{key}");
        assert_eq!(r.message, text, "{key}");
        assert_eq!(s.fingerprint(), before, "a refusal changes nothing ({key})");
    }
}

#[test]
fn long_text_is_refused_at_the_limit() {
    let mut s = testing::headless_session();
    run(&mut s, "settings.set", json!({"key": "collab.user_name", "value": "x".repeat(40)}));
    let r = refuse(&mut s, "settings.set", json!({"key": "collab.user_name", "value": "x".repeat(41)}));
    assert!(r.message.starts_with("Setting `collab.user_name` must be text of at most 40 characters, and got the text \""), "{}", r.message);
}

#[test]
fn colours_are_stored_as_hex_and_other_words_are_refused() {
    let (mut s, f) = testing::session();
    run(&mut s, "settings.set", json!({"key": "doc.default_background", "value": "#FAFAFA"}));
    assert_eq!(value(&s, "doc.default_background"), json!("#fafafa"));
    run(&mut s, "settings.set", json!({"key": "collab.user_color", "value": "#abc"}));
    assert_eq!(value(&s, "collab.user_color"), json!("#aabbcc"));
    let r = refuse(&mut s, "settings.set", json!({"key": "collab.user_color", "value": "blue"}));
    assert!(r.message.starts_with("Setting `collab.user_color` must be a colour such as `#6366f1`, or `transparent`, and got the text \"blue\""), "{}", r.message);
    assert!(stored(&f).is_some());
}

#[test]
fn server_urls_must_be_websocket_addresses_or_a_path() {
    let mut s = testing::headless_session();
    for ok in ["ws://example.org/ws", "wss://example.org/room", "/ws"] {
        run(&mut s, "settings.set", json!({"key": "collab.server_url", "value": ok}));
    }
    for bad in ["http://example.org", "ws://", "example.org/ws", "//evil.example/ws", "ws://a b/ws", ""] {
        let r = refuse(&mut s, "settings.set", json!({"key": "collab.server_url", "value": bad}));
        assert_eq!(r.code, "bad_args", "{bad:?}");
    }
}

#[test]
fn an_unknown_key_is_refused_with_the_catalog_text() {
    let mut s = testing::headless_session();
    let r = refuse(&mut s, "settings.set", json!({"key": "theem", "value": "dark"}));
    assert_eq!(r.code, "unknown_setting");
    assert_eq!(r.message, "Unknown setting `theem`. Did you mean `theme`? List settings with `settings.get`.");
    assert_eq!(r.suggest.first().map(|x| x.command.as_str()), Some("settings.get"));
}

#[test]
fn a_read_only_key_refuses_every_write() {
    let mut s = testing::headless_session();
    let text = "Setting `doc.recent` is read only: `doc.open` and `doc.save` maintain it. Read it with `settings.get`.";
    for (id, args) in [
        ("settings.set", json!({"key": "doc.recent", "value": []})),
        ("settings.reset", json!({"key": "doc.recent"})),
    ] {
        let r = refuse(&mut s, id, args);
        assert_eq!((r.code, r.message.as_str()), ("bad_args", text), "{id}");
    }
}

#[test]
fn a_family_can_write_a_read_only_key_with_the_same_checks() {
    let (mut s, f) = testing::session();
    let recent = json!([{"path": "/home/me/a.drafftink"}, {"stored": "Plan"}]);
    assert!(super::set_internal(&mut s, "doc.recent", &recent).unwrap());
    assert_eq!(value(&s, "doc.recent"), recent);
    // Outside a command nobody flushes the queued write; a command's executor does it.
    let writes = std::mem::take(&mut s.run.writes);
    assert!(s.flush_writes(writes).warnings.is_empty());
    assert_eq!(stored(&f).unwrap()["values"]["doc.recent"], recent);
    assert!(!super::set_internal(&mut s, "doc.recent", &recent).unwrap(), "same value: unchanged");
    let too_many: Vec<Value> = (0..11).map(|i| json!({"stored": format!("d{i}")})).collect();
    assert!(super::set_internal(&mut s, "doc.recent", &json!(too_many)).is_err());
    assert!(super::set_internal(&mut s, "doc.recent", &json!([{"nope": 1}])).is_err());
    assert_eq!(super::set_internal(&mut s, "no.such", &json!(1)).unwrap_err().code, "unknown_setting");
}

#[test]
fn history_depth_is_validated_and_read_live() {
    let mut s = testing::headless_session();
    run(&mut s, "settings.set", json!({"key": "history.depth", "value": 10}));
    assert_eq!(s.settings.history_depth(), 10);
    run(&mut s, "settings.set", json!({"key": "history.depth", "value": 1000.0}));
    assert_eq!(s.settings.history_depth(), 1000);
    assert!(s.execute("settings.set", json!({"key": "history.depth", "value": 1001})).is_err());
    assert!(s.execute("settings.set", json!({"key": "history.depth", "value": 9})).is_err());
    assert_eq!(s.settings.history_depth(), 1000);
}

#[test]
fn set_needs_both_arguments() {
    let mut s = testing::headless_session();
    assert_eq!(refuse(&mut s, "settings.set", json!({"key": "snap.grid"})).code, "bad_args");
    assert_eq!(refuse(&mut s, "settings.set", json!({"value": true})).code, "bad_args");
}

// ---- settings.toggle ----

#[test]
fn toggle_flips_a_boolean_and_stores_it() {
    let (mut s, f) = testing::session();
    let out = run(&mut s, "settings.toggle", json!({"key": "snap.smart_guides"}));
    assert_eq!((out["value"].clone(), out["previous"].clone()), (json!(true), json!(false)));
    assert_eq!(stored(&f), Some(json!({"schema": 1, "values": {"snap.smart_guides": true}})));
    let out = run(&mut s, "settings.toggle", json!({"key": "snap.smart_guides"}));
    assert_eq!(out["value"], json!(false));
    assert_eq!(stored(&f), None);
    // A setting that defaults to on flips off.
    let out = run(&mut s, "settings.toggle", json!({"key": "arrow.binding"}));
    assert_eq!(out["value"], json!(false));
}

#[test]
fn toggle_refuses_a_setting_that_is_not_on_or_off() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    let r = refuse(&mut s, "settings.toggle", json!({"key": "view.grid_style"}));
    assert_eq!(r.code, "bad_args");
    assert_eq!(r.message, "Setting `view.grid_style` is not an on/off setting. Use `settings.set` or `settings.cycle`.");
    assert_eq!(refuse(&mut s, "settings.toggle", json!({"key": "snap.gird"})).code, "unknown_setting");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn alt_s_toggles_smart_guides_and_mod_quote_cycles_the_grid() {
    let s = testing::headless_session();
    let t = s.spec("settings.toggle").unwrap();
    assert_eq!(t.shortcuts.len(), 1);
    assert_eq!(t.shortcuts[0].keys, &["Alt+S"]);
    assert_eq!(serde_json::from_str::<Value>(t.shortcuts[0].args).unwrap(), json!({"key": "snap.smart_guides"}));
    let c = s.spec("settings.cycle").unwrap();
    assert_eq!(c.shortcuts[0].keys, &["Mod+'"]);
    assert_eq!(serde_json::from_str::<Value>(c.shortcuts[0].args).unwrap(), json!({"key": "view.grid_style"}));
}

// ---- settings.cycle ----

#[test]
fn cycle_walks_the_grid_styles_in_the_old_order_and_wraps() {
    let mut s = testing::headless_session();
    let mut seen = vec![value(&s, "view.grid_style")];
    for _ in 0..5 {
        let out = run(&mut s, "settings.cycle", json!({"key": "view.grid_style"}));
        seen.push(out["value"].clone());
    }
    assert_eq!(
        seen,
        [json!("lines"), json!("horizontal_lines"), json!("crosses"), json!("dots"), json!("none"), json!("lines")]
    );
}

#[test]
fn cycle_goes_back_with_a_negative_step_and_survives_a_huge_one() {
    let mut s = testing::headless_session();
    run(&mut s, "settings.set", json!({"key": "view.grid_style", "value": "none"}));
    let out = run(&mut s, "settings.cycle", json!({"key": "view.grid_style", "step": -1}));
    assert_eq!(out["value"], json!("dots"));
    let out = run(&mut s, "settings.cycle", json!({"key": "view.grid_style", "step": 7}));
    assert_eq!(out["value"], json!("lines"), "dots is index 4; 4 + 7 = 11, and 11 mod 5 = 1");
    let before = s.fingerprint();
    let r = refuse(&mut s, "settings.cycle", json!({"key": "view.grid_style", "step": i64::MIN}));
    assert_eq!(r.code, "bad_args");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn cycle_refuses_a_setting_without_a_fixed_list() {
    let mut s = testing::headless_session();
    let r = refuse(&mut s, "settings.cycle", json!({"key": "snap.angle_step"}));
    assert_eq!(r.code, "bad_args");
    assert_eq!(r.message, "Setting `snap.angle_step` has no fixed list of values. Use `settings.set`.");
    // The theme is a list: light, dark, system.
    let out = run(&mut s, "settings.cycle", json!({"key": "theme"}));
    assert_eq!(out["value"], json!("light"), "system wraps to light");
}

// ---- settings.reset ----

#[test]
fn reset_restores_one_key_or_all_and_updates_the_record() {
    let (mut s, f) = testing::session();
    run(&mut s, "settings.set", json!({"key": "snap.grid", "value": true}));
    run(&mut s, "settings.set", json!({"key": "theme", "value": "dark"}));
    let out = run(&mut s, "settings.reset", json!({"key": "snap.grid"}));
    assert_eq!(out["reset"], json!(["snap.grid"]));
    assert_eq!(stored(&f), Some(json!({"schema": 1, "values": {"theme": "dark"}})));
    let out = run(&mut s, "settings.reset", json!({"all": true}));
    assert_eq!(out["reset"], json!(["theme"]));
    assert_eq!(stored(&f), None);
    let out = run(&mut s, "settings.reset", json!({"all": true}));
    assert_eq!(out["changed"], json!(false));
}

#[test]
fn reset_all_leaves_the_read_only_list_alone() {
    let mut s = testing::headless_session();
    super::set_internal(&mut s, "doc.recent", &json!([{"stored": "a"}])).unwrap();
    run(&mut s, "settings.reset", json!({"all": true}));
    assert_eq!(value(&s, "doc.recent"), json!([{"stored": "a"}]));
}

#[test]
fn reset_needs_exactly_one_of_key_and_all() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    for args in [json!({}), json!({"all": false}), json!({"key": "theme", "all": true})] {
        let r = refuse(&mut s, "settings.reset", args.clone());
        assert_eq!((r.code, r.message.as_str()), ("bad_args", "Give either `key` or all=true."), "{args}");
    }
    assert_eq!(refuse(&mut s, "settings.reset", json!({"key": "nope"})).code, "unknown_setting");
    assert_eq!(s.fingerprint(), before);
}

// ---- style.defaults and the tool state ----

#[test]
fn style_defaults_reach_the_style_of_new_elements_at_once() {
    use drafftink_scene::{Color, FillPattern, Sloppiness, Style};
    let mut s = testing::headless_session();
    assert_eq!(s.tool.style_defaults, Style::default());
    run(
        &mut s,
        "settings.set",
        json!({"key": "style.defaults", "value": {"stroke_color": "#FF0000", "stroke_width": 4, "fill_color": "#00ff00", "fill_pattern": "hachure", "sloppiness": "drunk", "font_size": 28}}),
    );
    let st = &s.tool.style_defaults;
    assert_eq!(st.stroke_color, Color::rgb(255, 0, 0));
    assert_eq!(st.stroke_width, 4.0);
    assert_eq!(st.fill_color, Some(Color::rgb(0, 255, 0)));
    assert_eq!(st.fill_pattern, FillPattern::Hachure);
    assert_eq!(st.sloppiness, Sloppiness::Drunk);
    assert_eq!(st.opacity, 1.0, "what the object does not name keeps the built-in value");
    assert_eq!(value(&s, "style.defaults")["stroke_color"], json!("#ff0000"), "stored in normal form");
    assert_eq!(value(&s, "style.defaults")["font_size"], json!(28), "font properties are kept for the text tools");
    run(&mut s, "settings.reset", json!({"key": "style.defaults"}));
    assert_eq!(s.tool.style_defaults, Style::default());
}

#[test]
fn style_defaults_null_fill_means_no_fill() {
    let mut s = testing::headless_session();
    run(&mut s, "settings.set", json!({"key": "style.defaults", "value": {"fill_color": "#00ff00"}}));
    assert!(s.tool.style_defaults.fill_color.is_some());
    run(&mut s, "settings.set", json!({"key": "style.defaults", "value": {"fill_color": null}}));
    assert_eq!(s.tool.style_defaults.fill_color, None);
}

#[test]
fn style_defaults_refuse_unknown_properties_and_bad_values_by_name() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    let r = refuse(&mut s, "settings.set", json!({"key": "style.defaults", "value": {"stroke_width": 0}}));
    assert_eq!(
        r.message,
        "Setting `style.defaults` must be an object whose `stroke_width` is a number above 0 and at most 1000, and got `stroke_width` set to the number 0."
    );
    let r = refuse(&mut s, "settings.set", json!({"key": "style.defaults", "value": {"sloppiness": "wild"}}));
    assert!(r.message.contains("`sloppiness` is one of `architect`, `artist`, `cartoonist` or `drunk`"), "{}", r.message);
    let r = refuse(&mut s, "settings.set", json!({"key": "style.defaults", "value": {"colour": "#fff"}}));
    assert!(r.message.contains("the unknown property `colour`"), "{}", r.message);
    let r = refuse(&mut s, "settings.set", json!({"key": "style.defaults", "value": "red"}));
    assert!(r.message.ends_with("and got the text \"red\"."), "{}", r.message);
    assert_eq!(s.fingerprint(), before);
}

// ---- the stored record: loading ----

fn put(f: &Fakes, text: &str) {
    use crate::ports::Storage;
    f.storage.clone().put(Ns::Settings, "settings", text.as_bytes()).unwrap();
}

#[test]
fn settings_survive_a_restart() {
    let (mut s, f) = testing::session();
    run(&mut s, "settings.set", json!({"key": "theme", "value": "dark"}));
    run(&mut s, "settings.set", json!({"key": "snap.angle_step", "value": 45}));
    run(&mut s, "settings.set", json!({"key": "style.defaults", "value": {"stroke_width": 8}}));
    let mut again = Session::new(f.ports());
    let report = again.load_settings().unwrap();
    assert!(report.found && report.ignored.is_empty() && report.unreadable.is_none());
    let mut applied = report.applied.clone();
    applied.sort();
    assert_eq!(applied, ["snap.angle_step", "style.defaults", "theme"]);
    assert_eq!(value(&again, "theme"), json!("dark"));
    assert_eq!(value(&again, "snap.angle_step"), json!(45));
    assert_eq!(again.tool.style_defaults.stroke_width, 8.0, "the style defaults are live after loading");
    assert_eq!(value(&again, "snap.grid"), json!(false), "keys the record does not name keep their default");
}

#[test]
fn loading_skips_what_it_cannot_use_and_says_why() {
    let (mut s, f) = testing::session();
    put(
        &f,
        r#"{"schema":1,"values":{"theme":"dark","removed.key":1,"snap.angle_step":500,"history.depth":"many","doc.recent":[{"stored":"a"}]}}"#,
    );
    let report = s.load_settings().unwrap();
    assert_eq!(value(&s, "theme"), json!("dark"));
    assert_eq!(value(&s, "snap.angle_step"), json!(15));
    assert_eq!(value(&s, "history.depth"), json!(200));
    assert_eq!(value(&s, "doc.recent"), json!([{"stored": "a"}]), "a read-only key loads from the record");
    let mut ignored: Vec<&str> = report.ignored.iter().map(|i| i.key.as_str()).collect();
    ignored.sort_unstable();
    assert_eq!(ignored, ["history.depth", "removed.key", "snap.angle_step"]);
    let step = report.ignored.iter().find(|i| i.key == "snap.angle_step").unwrap();
    assert_eq!(step.reason, "must be a number from 1 to 90, and got the number 500");
}

#[test]
fn a_damaged_record_gives_defaults_and_is_replaced_by_the_next_change() {
    for bad in ["not json", "[]", r#"{"values":{}}"#, r#"{"schema":0,"values":{}}"#, r#"{"schema":1}"#] {
        let (mut s, f) = testing::session();
        put(&f, bad);
        let report = s.load_settings().unwrap();
        assert!(report.found && report.unreadable.is_some(), "{bad}");
        assert_eq!(s.settings.revision, 0, "{bad}");
        run(&mut s, "settings.set", json!({"key": "theme", "value": "light"}));
        assert_eq!(stored(&f), Some(json!({"schema": 1, "values": {"theme": "light"}})), "{bad}");
    }
}

#[test]
fn a_record_from_a_newer_version_is_read_but_never_overwritten() {
    let (mut s, f) = testing::session();
    let newer = r#"{"schema":7,"values":{"theme":"dark","future.key":true}}"#;
    put(&f, newer);
    let report = s.load_settings().unwrap();
    assert_eq!(report.newer_schema, Some(7));
    assert_eq!(value(&s, "theme"), json!("dark"));
    let out = run(&mut s, "settings.set", json!({"key": "snap.grid", "value": true}));
    assert!(s.settings.bool_or("snap.grid", false), "the change still applies for this session");
    assert!(out["warnings"][0].as_str().unwrap().contains("newer version"), "{out}");
    assert_eq!(f.storage.bytes(Ns::Settings, "settings").unwrap(), newer.as_bytes(), "the newer record is untouched");
}

#[test]
fn a_session_without_storage_keeps_settings_for_the_session() {
    let mut s = testing::headless_session();
    let report = s.load_settings().unwrap();
    assert_eq!(report, store::SettingsLoad::default());
    let out = run(&mut s, "settings.set", json!({"key": "theme", "value": "dark"}));
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(value(&s, "theme"), json!("dark"));
}

#[test]
fn loading_with_nothing_stored_changes_nothing() {
    let (mut s, _f) = testing::session();
    let report = s.load_settings().unwrap();
    assert!(!report.found);
    assert_eq!(s.settings.revision, 0);
}

#[test]
fn the_record_holds_the_schema_number_and_only_non_default_values() {
    let mut st = crate::state::Settings::default();
    assert_eq!(store::encode(&st), None);
    st.set_raw("theme", json!("dark"));
    st.set_raw("not.a.key", json!(1));
    let bytes = store::encode(&st).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), json!({"schema": 1, "values": {"theme": "dark"}}));
    assert_eq!(keys::find("theme").unwrap().default_value(), json!("system"));
}

#[test]
fn a_batch_writes_the_record_once_after_the_last_step() {
    use crate::batch::{BatchOpts, BatchStep};
    let (mut s, f) = testing::session();
    let steps = [
        BatchStep::new("settings.set", json!({"key": "theme", "value": "dark"})),
        BatchStep::new("settings.toggle", json!({"key": "snap.grid"})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.failed.is_none());
    assert_eq!(stored(&f), Some(json!({"schema": 1, "values": {"snap.grid": true, "theme": "dark"}})));
    // A batch that fails on its second step leaves the settings it already changed (settings are outside
    // the rollback transaction), and the write of the first step is not flushed.
    let (mut s, f) = testing::session();
    let steps = [
        BatchStep::new("settings.set", json!({"key": "theme", "value": "dark"})),
        BatchStep::new("settings.set", json!({"key": "theme", "value": "purple"})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.failed.is_some());
    assert_eq!(stored(&f), None, "port writes of a rolled-back batch are dropped");
    assert_eq!(value(&s, "theme"), json!("dark"), "the setting itself is not rolled back");
}
