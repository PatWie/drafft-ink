//! The keymap: chords, and the one dispatcher that turns key events into actions.
//!
//! The bindings are data on the command registrations (`Binding` in `drafftink-engine`). This file
//! adds the few UI verbs that have keys (docs/commands.md §6.1, "UI verb" rows) and the code that
//! matches egui key events against both.
//!
//! Rules (docs/commands.md §1.12 and §6):
//!
//! * `Mod` is Cmd on macOS and Ctrl elsewhere. `Ctrl` is the physical Control key on macOS.
//! * Letters without Alt match the logical key (so AZERTY users get the letters they see). Digits,
//!   punctuation and every chord with Alt match the physical key, named by its US layout. `?` and `+`
//!   are produced with Shift and match the logical key.
//! * Scope order: `Text`, `PointEditor`, `Canvas`, `Always`. The first scope with a match wins.
//! * Nothing fires while an egui text field has focus.
//! * `Esc` closes an open overlay first (`ui.close_overlays`), else runs its registered binding.

use drafftink_engine::Origin;
use drafftink_engine::spec::{Scope, TextGate};
use egui::{Event, Key, Modifiers};
use serde_json::{Map, Value, json};

use crate::action::{Call, UiAction, Verb};
use crate::model::Model;
use crate::state::UiState;

/// One key combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    /// `Mod`: Cmd on macOS, Ctrl elsewhere.
    pub mod_key: bool,
    /// `Ctrl`: the physical Control key (only different from `Mod` on macOS).
    pub ctrl: bool,
    /// `Alt` (Option on macOS).
    pub alt: bool,
    /// `Shift`.
    pub shift: bool,
    /// The key.
    pub key: Key,
}

/// Parses a chord such as `Mod+Shift+Z`, `Esc`, `Mod++`, `Shift+PgUp`. `None` for text it cannot read.
pub fn parse_chord(text: &str) -> Option<Chord> {
    let mut rest = text;
    let mut chord = Chord { mod_key: false, ctrl: false, alt: false, shift: false, key: Key::Escape };
    loop {
        if let Some(r) = rest.strip_prefix("Mod+") {
            chord.mod_key = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Ctrl+") {
            chord.ctrl = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Alt+") {
            chord.alt = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Shift+") {
            chord.shift = true;
            rest = r;
        } else {
            break;
        }
    }
    let name = match rest {
        "PgUp" => "PageUp",
        "PgDn" => "PageDown",
        other => other,
    };
    chord.key = Key::from_name(name)?;
    Some(chord)
}

/// Keys that need Shift to be typed, so a chord with them ignores the Shift state.
fn shift_is_implied(key: Key) -> bool {
    matches!(key, Key::Questionmark | Key::Plus)
}

/// Keys matched by the logical key even though they are not letters.
fn is_logical(key: Key) -> bool {
    matches!(key, Key::Questionmark | Key::Plus)
}

impl Chord {
    /// True when the modifiers of an event are exactly this chord's.
    fn modifiers_match(&self, m: &Modifiers, mac: bool) -> bool {
        let want_command = self.mod_key || (!mac && self.ctrl);
        let want_ctrl = mac && self.ctrl;
        m.command == want_command
            && (!mac || m.ctrl == want_ctrl)
            && m.alt == self.alt
            && (shift_is_implied(self.key) || m.shift == self.shift)
    }

    /// True when a key event triggers this chord.
    pub fn matches(&self, key: Key, physical: Option<Key>, m: &Modifiers, mac: bool) -> bool {
        if !self.modifiers_match(m, mac) {
            return false;
        }
        let name = self.key.name();
        let letter = name.len() == 1 && name.chars().all(|c| c.is_ascii_alphabetic());
        if (letter && !self.alt) || is_logical(self.key) {
            key == self.key
        } else {
            physical.unwrap_or(key) == self.key
        }
    }

    /// How the chord is shown: `Ctrl+Shift+Z`, or `⌘⇧Z` on macOS.
    pub fn label(&self, mac: bool) -> String {
        let key = key_label(self.key);
        if mac {
            let mut s = String::new();
            if self.ctrl {
                s.push('⌃');
            }
            if self.alt {
                s.push('⌥');
            }
            if self.shift && !shift_is_implied(self.key) {
                s.push('⇧');
            }
            if self.mod_key {
                s.push('⌘');
            }
            s.push_str(&key);
            s
        } else {
            let mut parts: Vec<String> = Vec::new();
            if self.mod_key || self.ctrl {
                parts.push("Ctrl".to_owned());
            }
            if self.alt {
                parts.push("Alt".to_owned());
            }
            if self.shift && !shift_is_implied(self.key) {
                parts.push("Shift".to_owned());
            }
            parts.push(key);
            parts.join("+")
        }
    }
}

/// The name of a key on a key cap.
pub fn key_label(key: Key) -> String {
    match key {
        Key::Escape => "Esc".to_owned(),
        Key::ArrowLeft => "←".to_owned(),
        Key::ArrowRight => "→".to_owned(),
        Key::ArrowUp => "↑".to_owned(),
        Key::ArrowDown => "↓".to_owned(),
        Key::PageUp => "PgUp".to_owned(),
        Key::PageDown => "PgDn".to_owned(),
        other => other.symbol_or_name().to_owned(),
    }
}

/// A key of the UI itself: opens something, never runs a command.
#[derive(Debug, Clone, Copy)]
pub struct VerbBinding {
    /// The chord text, parsed with [`parse_chord`].
    pub keys: &'static [&'static str],
    /// Where it applies.
    pub scope: Scope,
    /// What it does.
    pub verb: Verb,
}

/// The UI verbs that have keys, from the "UI verb" rows of docs/commands.md §6.1. A test compares this
/// table with that document.
pub const VERB_BINDINGS: &[VerbBinding] = &[
    VerbBinding { keys: &["Mod+O"], scope: Scope::Always, verb: Verb::OpenFile },
    VerbBinding { keys: &["Mod+Shift+S"], scope: Scope::Always, verb: Verb::SaveAs },
    VerbBinding { keys: &["Mod+/", "Mod+Shift+P"], scope: Scope::Always, verb: Verb::OpenPalette },
    VerbBinding { keys: &["Mod+F"], scope: Scope::Canvas, verb: Verb::OpenSearch },
    VerbBinding { keys: &["?"], scope: Scope::Canvas, verb: Verb::OpenShortcuts },
    VerbBinding { keys: &["Alt+Z"], scope: Scope::Canvas, verb: Verb::ToggleZen },
    VerbBinding { keys: &["Alt+/"], scope: Scope::Canvas, verb: Verb::ToggleStats },
    VerbBinding { keys: &["S"], scope: Scope::Canvas, verb: Verb::OpenColorStroke },
    VerbBinding { keys: &["G"], scope: Scope::Canvas, verb: Verb::OpenColorFill },
    VerbBinding { keys: &["Shift+F"], scope: Scope::Canvas, verb: Verb::OpenFontPicker },
    VerbBinding { keys: &["I"], scope: Scope::Canvas, verb: Verb::InsertImage },
];

/// Where the pointer is, in world units, for the `$cursor` and `$viewport_center` placeholders.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pointer {
    /// The pointer over the canvas. `None` when it is elsewhere: the placeholder is then omitted.
    pub cursor: Option<(f64, f64)>,
    /// The middle of the canvas.
    pub viewport_center: Option<(f64, f64)>,
}

/// Replaces `"$cursor"` and `"$viewport_center"` in a binding's arguments by points. A key whose
/// value cannot be filled is dropped, as docs/commands.md §1.12 says.
pub fn substitute(args: &Value, pointer: &Pointer) -> Value {
    match args {
        Value::String(s) if s == "$cursor" => pointer.cursor.map_or(Value::Null, |(x, y)| json!({ "x": x, "y": y })),
        Value::String(s) if s == "$viewport_center" => {
            pointer.viewport_center.map_or(Value::Null, |(x, y)| json!({ "x": x, "y": y }))
        }
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, v) in m {
                let is_placeholder = matches!(v, Value::String(s) if s.starts_with('$'));
                let filled = substitute(v, pointer);
                if is_placeholder && filled.is_null() {
                    continue;
                }
                out.insert(k.clone(), filled);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| substitute(v, pointer)).collect()),
        other => other.clone(),
    }
}

/// What the dispatcher needs to know besides the events.
pub struct KeyContext<'a> {
    /// The model: bindings, text-edit state, point editor.
    pub model: &'a Model,
    /// The UI state: is an overlay open.
    pub state: &'a UiState,
    /// egui has a text field focused: nothing is handled.
    pub egui_wants_keyboard: bool,
    /// Pointer placeholders.
    pub pointer: Pointer,
}

/// The scopes to try, in order, for the current state.
fn scope_order(cx: &KeyContext<'_>) -> Vec<Scope> {
    if cx.model.text_editing {
        vec![Scope::Text, Scope::Always]
    } else if cx.state.modal_open() {
        vec![Scope::Always]
    } else if cx.model.point_selected {
        vec![Scope::PointEditor, Scope::Canvas, Scope::Always]
    } else {
        vec![Scope::Canvas, Scope::Always]
    }
}

/// While a text edit runs only the text scope applies, plus commands that commit the edit first
/// (`Mod` chords of `doc.*`, `tab.*` ...) and `Esc` (docs/commands.md §6.2).
fn allowed_in_text_edit(chord: &Chord, scope: Scope, gate: Option<TextGate>) -> bool {
    match scope {
        Scope::Text => true,
        Scope::Always | Scope::Canvas => chord.key == Key::Escape || (chord.mod_key && gate == Some(TextGate::Commit)),
        Scope::PointEditor => false,
    }
}

/// Turns the key events of one frame into actions, in event order. `events` are the raw egui events.
pub fn resolve_keys(events: &[Event], cx: &KeyContext<'_>) -> Vec<UiAction> {
    let mut out = Vec::new();
    if cx.egui_wants_keyboard {
        return out;
    }
    let mac = cx.model.mac;
    let scopes = scope_order(cx);
    for ev in events {
        let Event::Key { key, physical_key, pressed, repeat, modifiers } = ev else {
            continue;
        };
        if !*pressed {
            // Releasing Space ends a hold. The model knows whether one is running.
            if cx.model.tool_held && release_key(cx, *key, *physical_key) {
                out.push(UiAction::run_from(Origin::Shortcut, "tool.release", json!({})));
            }
            continue;
        }
        if let Some(action) = resolve_one(cx, &scopes, *key, *physical_key, modifiers, *repeat, mac) {
            out.push(action);
        }
    }
    out
}

fn release_key(cx: &KeyContext<'_>, key: Key, physical: Option<Key>) -> bool {
    cx.model.command("tool.hold").is_some_and(|c| {
        c.shortcuts.iter().flat_map(|b| b.chords.iter()).any(|ch| ch.key == key || Some(ch.key) == physical)
    })
}

fn resolve_one(
    cx: &KeyContext<'_>,
    scopes: &[Scope],
    key: Key,
    physical: Option<Key>,
    m: &Modifiers,
    repeat: bool,
    mac: bool,
) -> Option<UiAction> {
    // Esc closes an overlay before anything else. The equation editor is a text edit: Esc cancels it.
    if key == Key::Escape && cx.state.equation.is_some() {
        return Some(UiAction::run_from(Origin::Shortcut, "text.edit_end", json!({ "commit": false })));
    }
    if key == Key::Escape && cx.state.any_overlay_open() {
        return Some(UiAction::Verb(Verb::CloseOverlays));
    }
    for scope in scopes {
        // UI verbs.
        for vb in VERB_BINDINGS.iter().filter(|v| v.scope == *scope) {
            if repeat {
                continue;
            }
            let hit = vb.keys.iter().filter_map(|k| parse_chord(k)).any(|c| c.matches(key, physical, m, mac));
            if hit && (!cx.model.text_editing || vb_allowed_in_text(vb)) {
                return Some(UiAction::Verb(vb.verb));
            }
        }
        // Registered commands.
        for cmd in &cx.model.commands {
            for b in cmd.shortcuts.iter().filter(|b| b.scope == *scope) {
                let Some(chord) = b.chords.iter().find(|c| c.matches(key, physical, m, mac)) else {
                    continue;
                };
                if cx.model.text_editing && !allowed_in_text_edit(chord, b.scope, Some(cmd.while_text_edit)) {
                    continue;
                }
                if repeat && cmd.id == "tool.hold" {
                    return None;
                }
                let args = match &b.args {
                    Value::Null => json!({}),
                    a => substitute(a, &cx.pointer),
                };
                return Some(UiAction::Command(Call::new(Origin::Shortcut, cmd.id, args)));
            }
        }
    }
    None
}

/// UI verb keys during a text edit: the ones with `Mod` and Always scope (open, save as, palette).
fn vb_allowed_in_text(vb: &VerbBinding) -> bool {
    vb.scope == Scope::Always
}

/// One line of the shortcuts dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct HelpRow {
    /// What it does.
    pub title: String,
    /// The family (a heading) or `ui` for the interface's own keys.
    pub group: String,
    /// The chords, as shown.
    pub keys: Vec<String>,
}

/// Every binding in the registry and every UI verb key, for the help dialog. It is built from the
/// same data the dispatcher uses, so it cannot go stale.
pub fn help_rows(model: &Model) -> Vec<HelpRow> {
    let mut rows = Vec::new();
    for cmd in &model.commands {
        for b in &cmd.shortcuts {
            if b.chords.is_empty() {
                continue;
            }
            rows.push(HelpRow {
                title: b.title.unwrap_or(cmd.title).to_owned(),
                group: cmd.family.to_owned(),
                keys: b.chords.iter().map(|c| c.label(model.mac)).collect(),
            });
        }
    }
    for vb in VERB_BINDINGS {
        rows.push(HelpRow {
            title: vb.verb.title().to_owned(),
            group: "ui".to_owned(),
            keys: vb.keys.iter().filter_map(|k| parse_chord(k)).map(|c| c.label(model.mac)).collect(),
        });
    }
    rows.push(HelpRow {
        title: "Close dialogs, else cancel".to_owned(),
        group: "ui".to_owned(),
        keys: vec!["Esc".to_owned()],
    });
    rows
}
