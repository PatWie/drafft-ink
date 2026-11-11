//! `settings`: persisted settings (docs/commands.md §2.17 and §5).
//!
//! The keys, their types, ranges and defaults are the table in [`keys`]. A write is checked against it,
//! stored in the session at once (so it is live) and queued for the `Storage` port (so it survives a
//! restart). [`store`] holds the schema-versioned record and `Session::load_settings`.
//!
//! Settings are outside the rollback transaction (docs/commands.md §1.7), so every handler checks all
//! of its input before the first change.

pub mod keys;
pub mod store;

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use self::keys::{Kind, KeyDef};
use crate::args;
use crate::refusal::{Refusal, closest, echo};
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::{Binding, CommandSpec, Facts, Mutates, PortSet, Redraw, Scope, Typed};

pub use self::store::{SettingsLoad, apply_live};

fn unknown_setting(id: &str, key: &str) -> Refusal {
    let near = closest(key, keys::KEYS.iter().map(|k| k.name), 1);
    let message = match near.first() {
        Some(a) => format!("Unknown setting `{}`. Did you mean `{a}`? List settings with `settings.get`.", echo(key)),
        None => format!("Unknown setting `{}`. List settings with `settings.get`.", echo(key)),
    };
    Refusal::custom("unknown_setting", id, message).suggesting("settings.get", None)
}

fn wrong_value(id: &str, key: &str, expected: &str, got: &str) -> Refusal {
    Refusal::custom("bad_args", id, format!("Setting `{key}` must be {expected}, and got {got}."))
        .with_details(serde_json::json!({ "key": key }))
}

fn read_only(id: &str, key: &str) -> Refusal {
    Refusal::custom(
        "bad_args",
        id,
        format!("Setting `{key}` is read only: `doc.open` and `doc.save` maintain it. Read it with `settings.get`."),
    )
    .with_details(serde_json::json!({ "key": key }))
}

fn key_def(id: &str, key: &str) -> Result<&'static KeyDef, Refusal> {
    keys::find(key).ok_or_else(|| unknown_setting(id, key))
}

/// The result of storing one value.
struct Stored {
    changed: bool,
    value: Value,
    previous: Value,
}

/// Checks `value` for `def`, stores it and applies it. The caller persists once after its last write.
fn store_checked(s: &mut Session, def: &'static KeyDef, value: &Value) -> Result<Stored, Refusal> {
    let normal = def.normalize(value).map_err(|m| wrong_value(s.cmd(), def.name, &m.expected, &m.got))?;
    let previous = s.settings.get(def.name).cloned().unwrap_or_else(|| def.default_value());
    let changed = s.settings.set_raw(def.name, normal.clone());
    if changed {
        apply_live(s, def.name);
    }
    Ok(Stored { changed, value: normal, previous })
}

/// Sets a setting from another family (`style.set` stores `style.defaults`, `doc.open` stores
/// `doc.recent`): checked like `settings.set`, but a read-only key may be written. Queues the write to
/// storage. Returns whether the value changed.
pub fn set_internal(s: &mut Session, key: &str, value: &Value) -> Result<bool, Refusal> {
    let def = key_def(s.cmd(), key)?;
    let stored = store_checked(s, def, value)?;
    if stored.changed {
        store::persist(s);
    }
    Ok(stored.changed)
}

// ---- settings.get ----

/// Arguments of `settings.get`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct GetArgs {
    /// One setting such as `snap.grid`. Omit it to get every setting.
    pub key: Option<String>,
}

/// Output of `settings.get`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct GetOut {
    /// Always false: reading changes nothing.
    pub changed: bool,
    /// The value of the asked setting, or of every setting, by key.
    pub settings: BTreeMap<String, Value>,
    /// With `key`: the default of that setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// With `key`: what the setting accepts, as a phrase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepts: Option<String>,
    /// With `key`: what the setting is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

fn get(s: &mut Session, a: GetArgs) -> Result<GetOut, Refusal> {
    let id = s.cmd();
    match a.key {
        Some(k) => {
            let def = key_def(id, &k)?;
            let value = s.settings.get(def.name).cloned().unwrap_or_else(|| def.default_value());
            Ok(GetOut {
                changed: false,
                settings: BTreeMap::from([(def.name.to_owned(), value)]),
                default: Some(def.default_value()),
                accepts: Some(def.expected()),
                summary: Some(def.summary.to_owned()),
            })
        }
        None => Ok(GetOut {
            changed: false,
            settings: s.settings.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            default: None,
            accepts: None,
            summary: None,
        }),
    }
}

// ---- settings.set ----

/// Arguments of `settings.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// The setting, such as `snap.angle_step`. List them with `settings.get`.
    pub key: String,
    /// The new value. Its type and range depend on the key.
    pub value: Value,
}

/// Output of the commands that change one setting.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SetOut {
    /// False when the setting already had this value.
    pub changed: bool,
    /// The setting.
    pub key: String,
    /// Its value now.
    pub value: Value,
    /// Its value before.
    pub previous: Value,
    /// Its default.
    pub default: Value,
}

fn user_only(id: &str, key: &str) -> Refusal {
    Refusal::custom(
        "bad_args",
        id,
        format!("Setting `{key}` can only be changed by the person at the app, not by an agent. Ask them to change it in Preferences."),
    )
    .with_details(serde_json::json!({ "key": key }))
}

/// The key, if the caller may write it: not read only, and not reserved for a person when an agent calls.
fn writable(s: &Session, key: &str) -> Result<&'static KeyDef, Refusal> {
    let id = s.cmd();
    let def = key_def(id, key)?;
    if def.read_only {
        Err(read_only(id, key))
    } else if def.user_only && s.run.origin == Origin::Agent {
        Err(user_only(id, key))
    } else {
        Ok(def)
    }
}

fn finish_one(s: &mut Session, def: &'static KeyDef, st: Stored) -> SetOut {
    if st.changed {
        store::persist(s);
    }
    SetOut {
        changed: st.changed,
        key: def.name.to_owned(),
        value: st.value,
        previous: st.previous,
        default: def.default_value(),
    }
}

fn set(s: &mut Session, a: SetArgs) -> Result<SetOut, Refusal> {
    let def = writable(s, &a.key)?;
    let st = store_checked(s, def, &a.value)?;
    Ok(finish_one(s, def, st))
}

// ---- settings.toggle ----

/// Arguments of `settings.toggle`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToggleArgs {
    /// An on/off setting such as `snap.grid`.
    pub key: String,
}

fn toggle(s: &mut Session, a: ToggleArgs) -> Result<SetOut, Refusal> {
    let id = s.cmd();
    let def = writable(s, &a.key)?;
    if def.kind != Kind::Bool {
        return Err(Refusal::custom(
            "bad_args",
            id,
            format!("Setting `{}` is not an on/off setting. Use `settings.set` or `settings.cycle`.", def.name),
        ));
    }
    let current = s.settings.get(def.name).and_then(Value::as_bool).unwrap_or_else(|| def.default_value() == Value::Bool(true));
    let st = store_checked(s, def, &Value::Bool(!current))?;
    Ok(finish_one(s, def, st))
}

// ---- settings.cycle ----

/// Arguments of `settings.cycle`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CycleArgs {
    /// A setting with a fixed list of values, such as `view.grid_style` or `theme`.
    pub key: String,
    /// How many places to move through the list. Negative goes back. The list wraps.
    #[serde(default = "one")]
    pub step: i64,
}

fn one() -> i64 {
    1
}

fn cycle(s: &mut Session, a: CycleArgs) -> Result<SetOut, Refusal> {
    let id = s.cmd();
    let def = writable(s, &a.key)?;
    let Kind::Choice(list) = def.kind else {
        return Err(Refusal::custom(
            "bad_args",
            id,
            format!("Setting `{}` has no fixed list of values. Use `settings.set`.", def.name),
        ));
    };
    #[allow(clippy::cast_precision_loss)]
    let step = args::in_range(id, "step", a.step as f64, -1.0e6, 1.0e6)? as i64;
    let current = s.settings.get(def.name).and_then(Value::as_str).and_then(|c| list.iter().position(|w| *w == c)).unwrap_or(0);
    let len = i64::try_from(list.len()).unwrap_or(1).max(1);
    let at = i64::try_from(current).unwrap_or(0);
    let next = usize::try_from((at + step).rem_euclid(len)).unwrap_or(0);
    let word = list.get(next).copied().unwrap_or("");
    let st = store_checked(s, def, &Value::String(word.to_owned()))?;
    Ok(finish_one(s, def, st))
}

// ---- settings.reset ----

/// Arguments of `settings.reset`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ResetArgs {
    /// One setting to return to its default.
    pub key: Option<String>,
    /// Return every writable setting to its default. Give this or `key`, not both.
    pub all: bool,
}

/// Output of `settings.reset`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ResetOut {
    /// False when every named setting already had its default.
    pub changed: bool,
    /// The settings that changed.
    pub reset: Vec<String>,
}

fn reset(s: &mut Session, a: ResetArgs) -> Result<ResetOut, Refusal> {
    let id = s.cmd();
    let defs: Vec<&'static KeyDef> = match (&a.key, a.all) {
        (Some(k), false) => vec![writable(s, k)?],
        (None, true) => keys::KEYS.iter().filter(|k| writable(s, k.name).is_ok()).collect(),
        _ => return Err(Refusal::custom("bad_args", id, "Give either `key` or all=true.")),
    };
    let mut done = Vec::new();
    for def in defs {
        let st = store_checked(s, def, &def.default_value())?;
        if st.changed {
            done.push(def.name.to_owned());
        }
    }
    if !done.is_empty() {
        store::persist(s);
    }
    Ok(ResetOut { changed: !done.is_empty(), reset: done })
}

// ---- registration ----

/// A command that writes settings: no undo, no document, allowed in every mode. The redraw is `Scene`
/// because grid style, frame rendering and snap overlays change what is drawn.
const WRITE: Facts = Facts {
    undo: crate::spec::Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::SETTINGS,
    needs_selection: crate::spec::Selection::No,
    ports: PortSet::STORAGE,
    batch: crate::spec::Batch::Allowed,
    view_only: crate::spec::Gate::Allowed,
    while_gesture: crate::spec::Gate::Allowed,
    while_text_edit: crate::spec::TextGate::Allowed,
    redraw: Redraw::Scene,
    ring: crate::spec::RingMode::Always,
    palette: false,
    agent: true,
    doc_write: crate::spec::DocWrite::NotDocument,
};

const TOGGLE_KEYS: &[Binding] =
    &[Binding::new(&["Alt+S"], Scope::Canvas).with_args(r#"{"key":"snap.smart_guides"}"#).titled("Toggle smart guides")];
const CYCLE_KEYS: &[Binding] =
    &[Binding::new(&["Mod+'"], Scope::Canvas).with_args(r#"{"key":"view.grid_style"}"#).titled("Cycle grid style")];

/// The settings family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "settings.get",
        "settings",
        "Get settings",
        "Returns one setting with its default and what it accepts, or every setting.",
        &Typed::new(get),
        Facts::READ_ONLY,
    )
    .refusals(&["unknown_setting"])
    .example(r#"{"key":"snap.grid"}"#),
    CommandSpec::new(
        "settings.set",
        "settings",
        "Set setting",
        "Checks a value against the type and range of its key, applies it at once and stores it.",
        &Typed::new(set),
        WRITE,
    )
    .refusals(&["unknown_setting"])
    .example(r#"{"key":"snap.angle_step","value":30}"#),
    CommandSpec::new(
        "settings.toggle",
        "settings",
        "Toggle setting",
        "Flips an on/off setting and stores it.",
        &Typed::new(toggle),
        Facts { palette: true, ..WRITE },
    )
    .refusals(&["unknown_setting"])
    .shortcuts(TOGGLE_KEYS)
    .example(r#"{"key":"snap.grid"}"#),
    CommandSpec::new(
        "settings.cycle",
        "settings",
        "Cycle setting",
        "Moves an enumerated setting to its next value, wrapping at the end of the list.",
        &Typed::new(cycle),
        Facts { palette: true, ..WRITE },
    )
    .refusals(&["unknown_setting"])
    .shortcuts(CYCLE_KEYS)
    .example(r#"{"key":"view.grid_style","step":1}"#),
    CommandSpec::new(
        "settings.reset",
        "settings",
        "Reset settings",
        "Returns one setting, or every writable setting, to its default. Never changes the document.",
        &Typed::new(reset),
        WRITE,
    )
    .refusals(&["unknown_setting"])
    .example(r#"{"key":"snap.grid"}"#),
];

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
