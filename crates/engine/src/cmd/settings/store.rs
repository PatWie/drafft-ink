//! Settings in the `Storage` port: the schema-versioned record, loading it at start, writing it after a
//! change, and applying a changed value to the session at once.
//!
//! The record is JSON in the settings namespace under the key `settings`:
//!
//! ```json
//! { "schema": 1, "values": { "snap.grid": true, "theme": "dark" } }
//! ```
//!
//! `values` holds only the keys that differ from their default, so a default that changes in a later
//! release reaches everyone who never chose a value. A record with a newer `schema` than this build
//! knows is read as far as it validates and is never overwritten, so a downgrade cannot destroy
//! settings that a newer build wrote.

use drafftink_edit::arrow_defaults::{self, ArrowDefaults};
use drafftink_scene::Style;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::keys;
use crate::ports::{Ns, PortWrite};
use crate::refusal::Refusal;
use crate::session::Session;
use crate::state::Settings;

/// The schema version this build reads and writes.
pub const SCHEMA: u32 = 1;
/// The storage key of the record.
pub const RECORD_KEY: &str = "settings";

/// Session-wide facts about the stored record, kept in `Session::ext`.
#[derive(Debug, Default)]
pub struct StoreState {
    /// Set when the stored record has a newer schema: writes are skipped.
    pub newer_schema: Option<u32>,
}

/// A stored value that was not used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ignored {
    /// The key.
    pub key: String,
    /// Why.
    pub reason: String,
}

/// What loading the stored settings did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SettingsLoad {
    /// A record was found in storage.
    pub found: bool,
    /// Keys that were set from the record.
    pub applied: Vec<String>,
    /// Entries that were dropped: an unknown key or a value that fails its check.
    pub ignored: Vec<Ignored>,
    /// The record could not be read at all (damaged JSON, missing schema); defaults are in use and the
    /// record is replaced by the next change.
    pub unreadable: Option<String>,
    /// The record was written by a newer build with this schema; it is read but not overwritten.
    pub newer_schema: Option<u32>,
}

/// The record for the current values: only keys that differ from the default.
pub fn encode(settings: &Settings) -> Option<Vec<u8>> {
    let mut values = Map::new();
    for (k, v) in settings.iter() {
        if let Some(def) = keys::find(k)
            && def.default_value() != *v
        {
            values.insert(k.clone(), v.clone());
        }
    }
    if values.is_empty() {
        return None;
    }
    serde_json::to_vec(&json!({ "schema": SCHEMA, "values": values })).ok()
}

/// Reads a record into `settings`. `Err` when the bytes are not a record at all.
pub fn decode_into(settings: &mut Settings, bytes: &[u8]) -> Result<SettingsLoad, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("not valid JSON ({e})"))?;
    let obj = v.as_object().ok_or("not a JSON object")?;
    let schema = obj
        .get("schema")
        .and_then(Value::as_u64)
        .ok_or("it has no `schema` number")?;
    let values = obj.get("values").and_then(Value::as_object).ok_or("it has no `values` object")?;
    let mut out = SettingsLoad { found: true, ..SettingsLoad::default() };
    let schema = u32::try_from(schema).unwrap_or(u32::MAX);
    if schema == 0 {
        return Err("its `schema` is 0".to_owned());
    }
    if schema > SCHEMA {
        out.newer_schema = Some(schema);
    }
    for (k, val) in values {
        let Some(def) = keys::find(k) else {
            out.ignored.push(Ignored { key: crate::refusal::echo(k), reason: "no such setting in this version".to_owned() });
            continue;
        };
        match def.normalize(val) {
            Ok(n) => {
                settings.set_raw(k, n);
                out.applied.push(k.clone());
            }
            Err(m) => out.ignored.push(Ignored {
                key: k.clone(),
                reason: format!("must be {}, and got {}", m.expected, m.got),
            }),
        }
    }
    Ok(out)
}

impl Session {
    /// Loads the stored settings. Call it once at start, before the first command. A session without a
    /// storage port keeps the defaults. A damaged record is reported in `unreadable` and replaced by
    /// the next change; it is not an error.
    pub fn load_settings(&mut self) -> Result<SettingsLoad, Refusal> {
        let Some(storage) = self.ports.storage.as_ref() else {
            return Ok(SettingsLoad::default());
        };
        let bytes = storage
            .get(Ns::Settings, RECORD_KEY)
            .map_err(|e| Refusal::io_failed("settings.get", "read", "the stored settings", &e.to_string()))?;
        let Some(bytes) = bytes else {
            return Ok(SettingsLoad::default());
        };
        let before = self.settings.revision;
        let mut fresh = Settings::default();
        let out = match decode_into(&mut fresh, &bytes) {
            Ok(out) => out,
            Err(why) => {
                return Ok(SettingsLoad { found: true, unreadable: Some(why), ..SettingsLoad::default() });
            }
        };
        // Replace the whole set at once, so a key absent from the record returns to its default.
        let changed: Vec<String> = fresh
            .iter()
            .filter(|(k, v)| self.settings.get(k) != Some(*v))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &changed {
            if let Some(v) = fresh.get(k) {
                self.settings.set_raw(k, v.clone());
            }
        }
        for k in &changed {
            apply_live(self, k);
        }
        self.ext.with::<StoreState, _>(|st| st.newer_schema = out.newer_schema);
        if self.settings.revision != before {
            self.mark_dirty(crate::spec::Redraw::Scene);
        }
        Ok(out)
    }
}

/// Queues the write of the current record (or the delete of it when every value is a default). Nothing
/// is queued in a session without storage, or while a newer build's record must be kept.
pub(super) fn persist(s: &mut Session) {
    if !s.has_port(crate::ports::PortKind::Storage) {
        return;
    }
    if s.ext.with::<StoreState, _>(|st| st.newer_schema).is_some() {
        s.warn("The stored settings were written by a newer version of drafft.ink, so this change is kept for this session only.");
        return;
    }
    let w = match encode(&s.settings) {
        Some(bytes) => PortWrite::StoragePut { ns: Ns::Settings, key: RECORD_KEY.to_owned(), bytes },
        None => PortWrite::StorageDelete { ns: Ns::Settings, key: RECORD_KEY.to_owned() },
    };
    s.queue_write(w);
}

/// Makes a changed value take effect in the session. Most settings are read where they are used
/// (`s.settings.bool_or(..)`), so a change is live by being stored. Two have state elsewhere:
/// `style.defaults` is the style new elements get (`s.tool.style_defaults`), and `history.depth` is read
/// by the executor at every commit.
pub fn apply_live(s: &mut Session, key: &str) {
    match key {
        "style.defaults" => {
            let props = s.settings.get(key).cloned().unwrap_or(Value::Null);
            s.tool.style_defaults = style_from_props(&props);
        }
        "arrow.type" | "arrow.start_head" | "arrow.end_head" => s.tool.arrow_default = arrow_defaults_of(&s.settings),
        _ => {}
    }
}

/// The path type and heads of new arrows that the three `arrow.*` settings describe. A value that is not a
/// known word (a stored record from another version) keeps the built-in default for that part.
fn arrow_defaults_of(settings: &Settings) -> ArrowDefaults {
    let word = |key: &str| settings.get(key).and_then(Value::as_str);
    let built_in = ArrowDefaults::default();
    ArrowDefaults {
        path: word("arrow.type").and_then(arrow_defaults::path_named).unwrap_or(built_in.path),
        start_head: word("arrow.start_head").and_then(arrow_defaults::head_named).unwrap_or(built_in.start_head),
        end_head: word("arrow.end_head").and_then(arrow_defaults::head_named).unwrap_or(built_in.end_head),
    }
}

/// The style a defaults object describes: the built-in style with the shared properties replaced.
/// Font properties are not part of `Style`; they stay in the stored object for the text tools.
pub fn style_from_props(props: &Value) -> Style {
    let mut merged = match serde_json::to_value(Style::default()) {
        Ok(Value::Object(m)) => m,
        _ => return Style::default(),
    };
    if let Some(obj) = props.as_object() {
        for (k, v) in obj {
            if !matches!(
                k.as_str(),
                "stroke_color" | "fill_color" | "fill_pattern" | "stroke_width" | "stroke_style" | "sloppiness" | "corner_radius" | "opacity"
            ) {
                continue;
            }
            if v.is_null() {
                merged.remove(k);
            } else {
                merged.insert(k.clone(), v.clone());
            }
        }
    }
    serde_json::from_value(Value::Object(merged)).unwrap_or_default()
}
