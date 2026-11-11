//! The `drafftink` note inside an element's `customData`.
//!
//! Excalidraw has no place for some things this model keeps: a fifth sloppiness level, the dots, dashed and
//! zig-zag-line fills, an exact corner radius, serif and pixel fonts, per-character colours, math, an element name.
//! The exporter writes each of these into `customData.drafftink` **only when the plain Excalidraw fields would not
//! read back to the same value**, and the importer applies the note on top of the plain fields and removes it from
//! the element's own `custom` map. Excalidraw keeps `customData` untouched, so a drafftink to Excalidraw to drafftink
//! trip loses nothing even when Excalidraw edits the file in between.

use drafftink_scene::{Brush, FillPattern, FontFamily, FontWeight, InlineMath, Sloppiness, TextRun};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `customData` key that holds the note.
pub(super) const KEY: &str = "drafftink";

/// Values Excalidraw cannot hold. Every field is optional: absent means "the plain fields are right".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Note {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_pattern: Option<FillPattern>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sloppiness: Option<Sloppiness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<FontFamily>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<FontWeight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_size: Option<f64>,
    /// The element is a formula: the Excalidraw text holds its LaTeX source.
    #[serde(default, skip_serializing_if = "is_false")]
    pub math: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brush: Option<Brush>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlighter: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_clip: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<TextRun>,
    /// Inline math of a text. Excalidraw shows the source with its dollar signs; the note only says when it is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_math: Option<InlineMath>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Note {
    /// The note as a JSON value, or `None` when it holds nothing.
    pub(super) fn to_value(&self) -> Option<Value> {
        match serde_json::to_value(self) {
            Ok(Value::Object(m)) if !m.is_empty() => Some(Value::Object(m)),
            _ => None,
        }
    }

    /// Reads a note from the `drafftink` value. `Err` carries a plain-words reason.
    pub(super) fn from_value(v: &Value) -> Result<Note, String> {
        serde_json::from_value(v.clone()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_note_is_not_written_and_a_full_one_round_trips() {
        assert_eq!(Note::default().to_value(), None);
        let n = Note {
            fill_pattern: Some(FillPattern::Dots),
            sloppiness: Some(Sloppiness::Drunk),
            corner_radius: Some(7.5),
            font_family: Some(FontFamily::GelPenSerif),
            font_weight: Some(FontWeight::Light),
            head_size: Some(9.0),
            math: true,
            brush: Some(Brush::Calligraphy),
            highlighter: Some(true),
            closed: Some(true),
            frame_clip: Some(false),
            name: Some("n".into()),
            runs: vec![TextRun { start: 0, end: 2, color: None }],
            inline_math: Some(InlineMath::Off),
        };
        let v = n.to_value().unwrap();
        assert_eq!(Note::from_value(&v).unwrap(), n);
    }

    #[test]
    fn a_damaged_note_is_an_error_not_a_panic() {
        assert!(Note::from_value(&serde_json::json!({"fill_pattern": "sparkles"})).is_err());
        assert!(Note::from_value(&serde_json::json!([1])).is_err());
        // Unknown keys are ignored: a newer writer may add some.
        assert_eq!(Note::from_value(&serde_json::json!({"future": 1})).unwrap(), Note::default());
    }
}
