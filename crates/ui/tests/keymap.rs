//! The keymap dispatcher: shortcuts are data on the registrations, one resolver turns key events into
//! command calls, scopes resolve in the order Text, PointEditor, Canvas, Always.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::Fixture;
use drafftink_engine::spec::{HostEffect, Scope};
use drafftink_engine::{Origin, Registry};
use drafftink_ui::keymap::{
    Chord, KeyContext, Pointer, VERB_BINDINGS, help_rows, parse_chord, resolve_keys, substitute, verb_key_label,
};
use drafftink_ui::{Model, UiAction, UiState, Verb};
use egui::{Event, Key, Modifiers};
use serde_json::{Value, json};

fn mods(mac: bool, c: &Chord) -> Modifiers {
    Modifiers {
        alt: c.alt,
        ctrl: if mac { c.ctrl } else { c.ctrl || c.mod_key },
        shift: c.shift,
        mac_cmd: mac && c.mod_key,
        command: c.mod_key || (!mac && c.ctrl),
    }
}

fn press(key: Key, physical: Option<Key>, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: physical, pressed: true, repeat: false, modifiers }
}

fn model(mac: bool) -> Model {
    let mut f = Fixture::new();
    f.model.mac = mac;
    f.refresh();
    f.model.clone()
}

fn resolve(m: &Model, st: &UiState, events: &[Event]) -> Vec<UiAction> {
    resolve_keys(
        events,
        &KeyContext {
            model: m,
            state: st,
            egui_wants_keyboard: false,
            pointer: Pointer::default(),
            host_clipboard: false,
        },
    )
}

fn one(m: &Model, st: &UiState, e: Event) -> Option<UiAction> {
    resolve(m, st, &[e]).into_iter().next()
}

fn call(a: &UiAction) -> (&str, &Value, Origin) {
    let c = a.as_call().expect("a command call");
    (c.id.as_str(), &c.args, c.origin)
}

#[test]
fn chords_parse() {
    let c = parse_chord("Mod+Shift+Z").unwrap();
    assert!(c.mod_key && c.shift && !c.alt && c.key == Key::Z);
    assert_eq!(parse_chord("Esc").unwrap().key, Key::Escape);
    assert_eq!(parse_chord("Mod++").unwrap().key, Key::Plus);
    assert_eq!(parse_chord("Mod+=").unwrap().key, Key::Equals);
    assert_eq!(parse_chord("PgUp").unwrap().key, Key::PageUp);
    assert_eq!(parse_chord("Mod+Shift+Left").unwrap().key, Key::ArrowLeft);
    assert_eq!(parse_chord("?").unwrap().key, Key::Questionmark);
    assert!(parse_chord("Mod+Banana").is_none());
}

#[test]
fn chords_are_shown_per_platform() {
    let c = parse_chord("Mod+Shift+Z").unwrap();
    assert_eq!(c.label(false), "Ctrl+Shift+Z");
    assert_eq!(c.label(true), "Cmd+Shift+Z");
    assert_eq!(parse_chord("Mod+Alt+C").unwrap().label(true), "Cmd+Opt+C");
    assert_eq!(parse_chord("Mod+Shift+Left").unwrap().label(false), "Ctrl+Shift+Left");
}

#[test]
fn a_tool_key_picks_tool_set_with_the_tool() {
    let m = model(false);
    let st = UiState::new();
    let a = one(&m, &st, press(Key::R, Some(Key::R), Modifiers::NONE)).unwrap();
    assert_eq!(call(&a), ("tool.set", &json!({"tool": "rectangle"}), Origin::Shortcut));
}

#[test]
fn mod_is_ctrl_on_linux_and_windows_and_cmd_on_macos() {
    let (lin, mac) = (model(false), model(true));
    let st = UiState::new();
    let ctrl_z = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let cmd_z = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
    assert_eq!(call(&one(&lin, &st, press(Key::Z, Some(Key::Z), ctrl_z)).unwrap()).0, "history.undo");
    assert_eq!(
        call(&one(&lin, &st, press(Key::Z, Some(Key::Z), Modifiers::NONE)).unwrap()).0,
        "tool.set",
        "Z alone is the laser tool, not undo"
    );
    assert_eq!(call(&one(&mac, &st, press(Key::Z, Some(Key::Z), cmd_z)).unwrap()).0, "history.undo");
    // On macOS the physical Control key is not Mod: the old app only knew Ctrl, so Cmd did nothing there.
    let mac_ctrl = Modifiers { ctrl: true, ..Modifiers::NONE };
    assert!(one(&mac, &st, press(Key::Z, Some(Key::Z), mac_ctrl)).is_none());
}

#[test]
fn redo_has_two_chords() {
    let m = model(false);
    let st = UiState::new();
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let ctrl_shift = Modifiers { shift: true, ..ctrl };
    assert_eq!(call(&one(&m, &st, press(Key::Z, None, ctrl_shift)).unwrap()).0, "history.redo");
    assert_eq!(call(&one(&m, &st, press(Key::Y, None, ctrl)).unwrap()).0, "history.redo");
}

#[test]
fn letters_match_the_logical_key_so_azerty_users_get_the_letter_they_see() {
    let m = model(false);
    let st = UiState::new();
    // French layout: the key printed A sits where the US layout has Q.
    let a = one(&m, &st, press(Key::A, Some(Key::Q), Modifiers::NONE)).unwrap();
    assert_eq!(call(&a).1, &json!({"tool": "arrow"}));
    // The physical Q position produces the letter Q on AZERTY? No: it produces A, so Q is not asked for.
    assert!(one(&m, &st, press(Key::Q, Some(Key::A), Modifiers::NONE)).is_some(), "the key printed Q is tool lock");
}

#[test]
fn digits_and_shifted_digits_match_the_physical_key() {
    let m = model(false);
    let st = UiState::new();
    // US layout: Shift+1 types `!`, but the binding is "Shift+1": view.zoom_to_fit scope all.
    let shift = Modifiers { shift: true, ..Modifiers::NONE };
    let a = one(&m, &st, press(Key::Exclamationmark, Some(Key::Num1), shift)).unwrap();
    assert_eq!(call(&a), ("view.zoom_to_fit", &json!({"scope": "all"}), Origin::Shortcut));
    // French layout: digits need Shift, the unshifted key types `&`. Plain `1` is the select tool.
    let a = one(&m, &st, press(Key::Num1, Some(Key::Num1), Modifiers::NONE)).unwrap();
    assert_eq!(call(&a).1, &json!({"tool": "select"}));
}

#[test]
fn alt_chords_use_the_physical_key_because_option_changes_the_character_on_macos() {
    let m = model(true);
    let st = UiState::new();
    // Option+H types a different character on macOS; the physical key is KeyH.
    let alt = Modifiers { alt: true, ..Modifiers::NONE };
    let a = one(&m, &st, press(Key::Home, Some(Key::H), alt));
    let a = a.expect("Alt+H distributes horizontally");
    assert_eq!(call(&a).0, "arrange.distribute");
}

#[test]
fn question_mark_opens_the_shortcuts_dialog_with_shift_implied() {
    let m = model(false);
    let st = UiState::new();
    let shift = Modifiers { shift: true, ..Modifiers::NONE };
    let a = one(&m, &st, press(Key::Questionmark, Some(Key::Slash), shift)).unwrap();
    assert_eq!(a, UiAction::Verb(Verb::OpenShortcuts));
}

#[test]
fn delete_is_arrow_delete_point_in_the_point_editor_and_element_delete_otherwise() {
    let mut m = model(false);
    let st = UiState::new();
    let del = press(Key::Delete, Some(Key::Delete), Modifiers::NONE);
    m.point_selected = true;
    assert_eq!(call(&one(&m, &st, del.clone()).unwrap()).0, "arrow.delete_point", "PointEditor is tried before Canvas");
    m.point_selected = false;
    assert_eq!(call(&one(&m, &st, del).unwrap()).0, "element.delete");
}

#[test]
fn esc_closes_an_overlay_first_and_otherwise_cancels() {
    let m = model(false);
    let mut st = UiState::new();
    let esc = press(Key::Escape, None, Modifiers::NONE);
    assert_eq!(call(&one(&m, &st, esc.clone()).unwrap()).0, "tool.cancel");
    st.menu_open = true;
    assert_eq!(one(&m, &st, esc.clone()).unwrap(), UiAction::Verb(Verb::CloseOverlays));
    st.menu_open = false;
    st.equation = Some(Default::default());
    let a = one(&m, &st, esc).unwrap();
    assert_eq!(
        call(&a),
        ("text.edit_end", &json!({"commit": false}), Origin::Shortcut),
        "Esc cancels the equation edit"
    );
}

#[test]
fn a_host_that_delivers_clipboard_events_gets_no_second_command_from_the_keys() {
    let m = model(false);
    let st = UiState::new();
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let shifted = Modifiers { shift: true, ..ctrl };
    let chords = [
        ("clipboard.copy", press(Key::C, Some(Key::C), ctrl)),
        ("clipboard.cut", press(Key::X, Some(Key::X), ctrl)),
        ("clipboard.paste", press(Key::V, Some(Key::V), ctrl)),
        ("clipboard.paste", press(Key::V, Some(Key::V), shifted)),
    ];
    for (id, event) in chords {
        let by_keys = resolve(&m, &st, std::slice::from_ref(&event));
        assert_eq!(by_keys.first().and_then(UiAction::as_call).map(|c| c.id.as_str()), Some(id), "{event:?}");
        let by_host = resolve_keys(
            std::slice::from_ref(&event),
            &KeyContext {
                model: &m,
                state: &st,
                egui_wants_keyboard: false,
                pointer: Pointer::default(),
                host_clipboard: true,
            },
        );
        assert!(by_host.is_empty(), "{id}: the clipboard event runs it, not the key: {by_host:?}");
    }
    // Chords the platform does not turn into a clipboard event keep their binding.
    let alt = Modifiers { alt: true, ..ctrl };
    let style = resolve_keys(
        &[press(Key::C, Some(Key::C), alt)],
        &KeyContext {
            model: &m,
            state: &st,
            egui_wants_keyboard: false,
            pointer: Pointer::default(),
            host_clipboard: true,
        },
    );
    assert_eq!(style.first().and_then(UiAction::as_call).map(|c| c.id.as_str()), Some("clipboard.copy_style"));
}

#[test]
fn nothing_fires_while_a_text_field_has_focus() {
    let m = model(false);
    let st = UiState::new();
    let events = [press(Key::R, Some(Key::R), Modifiers::NONE)];
    let out = resolve_keys(
        &events,
        &KeyContext {
            model: &m,
            state: &st,
            egui_wants_keyboard: true,
            pointer: Pointer::default(),
            host_clipboard: false,
        },
    );
    assert!(out.is_empty());
}

#[test]
fn while_a_dialog_is_open_canvas_keys_do_not_reach_the_canvas() {
    let m = model(false);
    let mut st = UiState::new();
    st.shortcuts_open = true;
    assert!(one(&m, &st, press(Key::R, Some(Key::R), Modifiers::NONE)).is_none());
    // Always-scope keys still work: Alt+R toggles view-only anywhere.
    let alt = Modifiers { alt: true, ..Modifiers::NONE };
    assert!(one(&m, &st, press(Key::R, Some(Key::R), alt)).is_some());
}

#[test]
fn during_a_text_edit_only_text_always_and_commit_gate_chords_apply() {
    let mut m = model(false);
    let st = UiState::new();
    m.text_editing = true;
    // Tool keys are ordinary text now.
    assert!(one(&m, &st, press(Key::R, Some(Key::R), Modifiers::NONE)).is_none());
    // Esc ends the edit through tool.cancel (docs/commands.md §6.2).
    assert_eq!(call(&one(&m, &st, press(Key::Escape, None, Modifiers::NONE)).unwrap()).0, "tool.cancel");
    // Undo is the editor's own inside a text edit, not history.undo.
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    assert!(one(&m, &st, press(Key::Z, Some(Key::Z), ctrl)).is_none());
}

#[test]
fn holding_space_holds_the_hand_tool_and_releasing_ends_it() {
    let mut m = model(false);
    let st = UiState::new();
    let down = press(Key::Space, Some(Key::Space), Modifiers::NONE);
    let a = one(&m, &st, down.clone()).unwrap();
    assert_eq!(call(&a), ("tool.hold", &json!({"tool": "hand"}), Origin::Shortcut));
    // Auto-repeat of the held key does nothing.
    let repeat = Event::Key {
        key: Key::Space,
        physical_key: Some(Key::Space),
        pressed: true,
        repeat: true,
        modifiers: Modifiers::NONE,
    };
    assert!(one(&m, &st, repeat).is_none());
    m.tool_held = true;
    let up = Event::Key {
        key: Key::Space,
        physical_key: Some(Key::Space),
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    assert_eq!(call(&one(&m, &st, up).unwrap()).0, "tool.release");
}

#[test]
fn all_events_of_a_frame_resolve_in_order() {
    let m = model(false);
    let st = UiState::new();
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let out = resolve(
        &m,
        &st,
        &[
            press(Key::R, Some(Key::R), Modifiers::NONE),
            press(Key::Z, Some(Key::Z), ctrl),
            press(Key::O, Some(Key::O), Modifiers::NONE),
        ],
    );
    let ids: Vec<&str> = out.iter().map(|a| a.as_call().unwrap().id.as_str()).collect();
    assert_eq!(ids, ["tool.set", "history.undo", "tool.set"]);
}

#[test]
fn placeholders_are_filled_or_the_key_is_dropped() {
    let args = json!({"at": "$cursor", "plain_text": true});
    let with = Pointer { cursor: Some((10.0, 20.0)), viewport_center: Some((0.0, 0.0)) };
    assert_eq!(substitute(&args, &with), json!({"at": [10.0, 20.0], "plain_text": true}));
    assert_eq!(
        substitute(&args, &Pointer::default()),
        json!({"plain_text": true}),
        "outside the canvas the placeholder is omitted"
    );
}

/// Every default binding of every registered command, replayed as the key event it names, resolves to
/// exactly that command with exactly those arguments, on both platforms. This is the dispatcher's
/// per-scope test: the scope decides which state the model must be in.
#[test]
fn every_registered_binding_dispatches_to_its_command_on_both_platforms() {
    let mut checked = 0;
    for mac in [false, true] {
        let m = model(mac);
        for cmd in &m.commands {
            for b in &cmd.shortcuts {
                for chord in &b.chords {
                    let mut m2 = m.clone();
                    let mut st = UiState::new();
                    match b.scope {
                        Scope::Canvas | Scope::Always => {}
                        Scope::PointEditor => m2.point_selected = true,
                        Scope::Text => {
                            m2.text_editing = true;
                        }
                    }
                    let _ = &mut st;
                    let key = chord.key;
                    let ev = press(key, Some(key), mods(mac, chord));
                    let got = resolve(&m2, &st, &[ev]);
                    let first = got
                        .first()
                        .unwrap_or_else(|| panic!("{} {:?} {}: nothing resolved", cmd.id, chord, chord.label(mac)));
                    let c = first
                        .as_call()
                        .unwrap_or_else(|| panic!("{} {}: resolved to {first:?}", cmd.id, chord.label(mac)));
                    let want_args = if b.args.is_null() { json!({}) } else { substitute(&b.args, &Pointer::default()) };
                    let what = format!("chord {} (mac={mac}, scope {:?})", chord.label(mac), b.scope);
                    if cmd.host.effect == HostEffect::CompletesExport {
                        // An export started by a key is the export of the dialog: the binding's arguments plus
                        // the ones the dialog would send (the download name, the scope and the look).
                        assert_eq!(c.id, cmd.id, "{what}");
                        let given = want_args.as_object().expect("binding arguments are an object");
                        assert!(given.iter().all(|(k, v)| c.args.get(k) == Some(v)), "{what}: {:?}", c.args);
                        assert!(
                            c.args.get("to").is_some_and(|to| to.get("download").is_some()),
                            "{what}: {:?}",
                            c.args
                        );
                    } else {
                        assert_eq!((c.id.as_str(), &c.args), (cmd.id, &want_args), "{what}");
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 100, "only {checked} bindings checked");
}

#[test]
fn ui_verb_keys_do_not_collide_with_command_keys() {
    let m = model(false);
    for vb in VERB_BINDINGS {
        for k in vb.keys {
            let vc = parse_chord(k).unwrap_or_else(|| panic!("verb key {k} does not parse"));
            for cmd in &m.commands {
                for b in cmd.shortcuts.iter().filter(|b| b.scope == vb.scope) {
                    assert!(!b.chords.contains(&vc), "{k} is bound to the verb {:?} and to {}", vb.verb, cmd.id);
                }
            }
        }
    }
}

/// The verb table is a copy of the "UI verb" rows of docs/commands.md §6.1; this test keeps them equal.
#[test]
fn verb_bindings_match_the_catalog() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/commands.md")).unwrap();
    let start = doc.find("### 6.1 Default bindings").unwrap();
    let end = doc[start..].find("### 6.2").map(|i| start + i).unwrap();
    let mut from_doc: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = Default::default();
    for line in doc[start..end].lines().filter(|l| l.starts_with('|') && l.contains("UI verb `ui.")) {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let verb = cells[2].split("UI verb `").nth(1).and_then(|r| r.split('`').next()).unwrap().to_owned();
        let keys: std::collections::BTreeSet<String> =
            cells[1].split('`').enumerate().filter(|(i, _)| i % 2 == 1).map(|(_, k)| k.to_owned()).collect();
        from_doc.entry(verb).or_default().extend(keys);
    }
    let mut from_code: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = Default::default();
    for vb in VERB_BINDINGS {
        from_code.entry(vb.verb.id().to_owned()).or_default().extend(vb.keys.iter().map(|k| (*k).to_owned()));
    }
    // The doc cell for `Mod+O` is written `Mod+O`; compare after removing keys the doc lists as words.
    let normalise = |m: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>| m;
    assert_eq!(normalise(from_code), normalise(from_doc));
}

#[test]
fn every_verb_has_an_id_and_a_title_and_ids_are_in_the_catalog() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/commands.md")).unwrap();
    for v in Verb::ALL {
        assert!(doc.contains(&format!("`{}`", v.id())), "{} is not in docs/commands.md", v.id());
        assert!(!v.title().is_empty());
    }
}

#[test]
fn the_help_rows_list_every_binding_and_every_verb_key() {
    let m = model(false);
    let rows = help_rows(&m);
    let reg = Registry::global();
    for spec in reg.all() {
        for b in spec.shortcuts {
            let title = b.title.unwrap_or(spec.title);
            assert!(rows.iter().any(|r| r.title == title), "no help row for {title} ({})", spec.id);
        }
    }
    for vb in VERB_BINDINGS {
        assert!(rows.iter().any(|r| r.title == vb.verb.title()), "no help row for {:?}", vb.verb);
        assert!(verb_key_label(vb.verb, false).is_some());
    }
}
