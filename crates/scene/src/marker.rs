//! Highlight: the text-marker band behind an element.
//!
//! A highlight is a named token ([`MarkerColor`]), not a raw colour, because the same marker has to read well on a
//! white canvas and on the dark one. The stored colours of strokes and fills are theme independent and the dark
//! canvas derives its look from them with a filter (`Color::for_dark_theme`); a marker has its own pair of values
//! instead, because that filter turns a pale yellow into a muddy olive. [`MarkerColor::rgb`] gives the value for a
//! [`Tone`]; every band is drawn at [`MARKER_ALPHA`] as one layer, so overlaps do not get darker.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::color::Color;

/// Opacity of a marker band, as a share of 1. The whole band is one layer at this opacity.
pub const MARKER_ALPHA: f64 = 0.5;

/// Which canvas a marker is drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    /// The light canvas (white page).
    Light,
    /// The dark canvas.
    Dark,
}

/// A named marker colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MarkerColor {
    /// Yellow, the classic highlighter.
    Yellow,
    /// Green.
    Green,
    /// Pink.
    Pink,
    /// Blue.
    Blue,
    /// Orange.
    Orange,
}

impl MarkerColor {
    /// Every marker colour, in the order a picker lists them.
    pub const ALL: [MarkerColor; 5] =
        [MarkerColor::Yellow, MarkerColor::Green, MarkerColor::Pink, MarkerColor::Blue, MarkerColor::Orange];

    /// The name used in files and commands (`yellow`).
    pub fn name(self) -> &'static str {
        match self {
            MarkerColor::Yellow => "yellow",
            MarkerColor::Green => "green",
            MarkerColor::Pink => "pink",
            MarkerColor::Blue => "blue",
            MarkerColor::Orange => "orange",
        }
    }

    /// The marker called `name`.
    pub fn from_name(name: &str) -> Option<MarkerColor> {
        MarkerColor::ALL.into_iter().find(|m| m.name() == name)
    }

    /// The opaque ink of the marker on a canvas of `tone`. The band is drawn with it at [`MARKER_ALPHA`].
    ///
    /// On the light canvas the inks are bright, so the band is a pale wash that keeps black text at a contrast of
    /// 13 or more. On the dark canvas they are deep, so the band shows against the page (at least 1.7 to 1) and the
    /// light text on it keeps a contrast of 5.9 or more. `tests/marker.rs` holds both numbers.
    pub const fn rgb(self, tone: Tone) -> Color {
        match (tone, self) {
            (Tone::Light, MarkerColor::Yellow) => Color::rgb(255, 222, 23),
            (Tone::Light, MarkerColor::Green) => Color::rgb(120, 220, 100),
            (Tone::Light, MarkerColor::Pink) => Color::rgb(255, 120, 190),
            (Tone::Light, MarkerColor::Blue) => Color::rgb(100, 190, 255),
            (Tone::Light, MarkerColor::Orange) => Color::rgb(255, 160, 60),
            (Tone::Dark, MarkerColor::Yellow) => Color::rgb(190, 160, 0),
            (Tone::Dark, MarkerColor::Green) => Color::rgb(40, 150, 70),
            (Tone::Dark, MarkerColor::Pink) => Color::rgb(200, 50, 130),
            (Tone::Dark, MarkerColor::Blue) => Color::rgb(40, 110, 200),
            (Tone::Dark, MarkerColor::Orange) => Color::rgb(210, 100, 20),
        }
    }
}

impl fmt::Display for MarkerColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a run of text says about its highlight, on top of the element's own.
///
/// This is the rule for text (`docs/model.md`, "Highlight"): the element's `style.highlight` is the default for
/// every character, and a run that carries a value of its own replaces it for the characters of the run.
/// [`SpanHighlight::Off`] is a value too: it takes the highlight away from part of a text that is highlighted as a
/// whole. A character in no run, or in a run that says [`SpanHighlight::Inherit`], follows the element.
///
/// In files and in the shared document it is a string: `"yellow"` (any marker colour) or `"none"`; absent is
/// inherit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SpanHighlight {
    /// No value of its own: the element's highlight applies.
    #[default]
    Inherit,
    /// Not highlighted, whatever the element says.
    Off,
    /// Highlighted with this marker.
    On(MarkerColor),
}

impl SpanHighlight {
    /// The name in files: `none` or a marker colour. `Inherit` has none (it is the absent field).
    pub fn name(self) -> Option<&'static str> {
        match self {
            SpanHighlight::Inherit => None,
            SpanHighlight::Off => Some("none"),
            SpanHighlight::On(m) => Some(m.name()),
        }
    }

    /// The value written as `name`.
    pub fn from_name(name: &str) -> Option<SpanHighlight> {
        if name == "none" { Some(SpanHighlight::Off) } else { MarkerColor::from_name(name).map(SpanHighlight::On) }
    }

    /// True for [`SpanHighlight::Inherit`]; the field is left out of files then.
    pub fn is_inherit(&self) -> bool {
        *self == SpanHighlight::Inherit
    }

    /// The highlight of a character in this run, given the element's.
    pub fn resolve(self, element: Option<MarkerColor>) -> Option<MarkerColor> {
        match self {
            SpanHighlight::Inherit => element,
            SpanHighlight::Off => None,
            SpanHighlight::On(m) => Some(m),
        }
    }

    /// The value that makes a character look highlighted with `wanted` (`None`: not highlighted) when the element's
    /// own highlight is `element`: `Inherit` when the element already says that, an explicit value otherwise.
    pub fn for_result(wanted: Option<MarkerColor>, element: Option<MarkerColor>) -> SpanHighlight {
        match (wanted, element) {
            (w, e) if w == e => SpanHighlight::Inherit,
            (Some(m), _) => SpanHighlight::On(m),
            (None, _) => SpanHighlight::Off,
        }
    }
}

impl Serialize for SpanHighlight {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.name() {
            Some(n) => s.serialize_str(n),
            None => s.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for SpanHighlight {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Some(text) = Option::<std::borrow::Cow<str>>::deserialize(d)? else { return Ok(SpanHighlight::Inherit) };
        SpanHighlight::from_name(&text).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{text:?} is not a highlight; use \"none\", \"yellow\", \"green\", \"pink\", \"blue\" or \"orange\""
            ))
        })
    }
}

impl JsonSchema for SpanHighlight {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SpanHighlight".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "enum": ["none", "yellow", "green", "pink", "blue", "orange"] })
    }
}
