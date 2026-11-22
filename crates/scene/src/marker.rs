//! Highlight: the text-marker band behind an element.
//!
//! A highlight is a palette token ([`MarkerColor`]: a family and a marker shade, `yellow-400`), not a raw colour,
//! because the same marker has to read well on a white canvas and on the dark one. The stored colours of strokes and
//! fills are theme independent and the dark canvas derives its look from them with a filter
//! (`Color::for_dark_theme`); a marker has its own pair of values instead, because that filter turns a pale yellow
//! into a muddy olive. [`MarkerColor::rgb`] gives the value for a [`Tone`]; every band is drawn at [`MARKER_ALPHA`]
//! as one layer, so overlaps do not get darker.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::color::Color;
use crate::palette;

/// Opacity of a marker band, as a share of 1. The whole band is one layer at this opacity.
pub const MARKER_ALPHA: f64 = 0.5;

/// Band width as a multiple of the stroke width.
const WIDTH_PER_STROKE: f64 = 3.0;
/// The narrowest band, in world units.
const MIN_BAND_WIDTH: f64 = 10.0;
/// How far a text band reaches past its characters at each end, as a share of the line height.
pub const TEXT_END_PAD: f64 = 0.06;
/// Largest sideways wobble of a text band's ends, as a share of the line height.
pub const TEXT_END_WOBBLE: f64 = 0.10;

/// The width of the band of an outline with `stroke_width`: three strokes, and not less than 10 world units. This
/// is what the renderer draws and what hit testing and the selection box count.
pub fn band_width(stroke_width: f64) -> f64 {
    let sw = if stroke_width.is_finite() { stroke_width.clamp(0.0, crate::limits::MAX_STROKE_WIDTH) } else { 0.0 };
    (WIDTH_PER_STROKE * sw).max(MIN_BAND_WIDTH)
}

/// How far the band of a text or a formula reaches sideways past its characters, in world units, for lines
/// `line_height` tall.
pub fn text_band_overhang(line_height: f64) -> f64 {
    if line_height.is_finite() { (TEXT_END_PAD + TEXT_END_WOBBLE) * line_height.max(0.0) } else { 0.0 }
}

/// Whether anything of `e` is highlighted: the element itself, or (for a text) some of its characters.
pub fn has_highlight(e: &crate::Element) -> bool {
    e.style.highlight.is_some()
        || matches!(&e.kind, crate::ElementKind::Text(t) if t.runs.iter().any(|r| matches!(r.highlight, SpanHighlight::On(_))))
}

/// Which canvas a marker is drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    /// The light canvas (white page).
    Light,
    /// The dark canvas.
    Dark,
}

/// The shades of a marker, as palette shades. Three of the twelve: the lightest wash, the standard marker, and a
/// strong one. Lighter shades (200 and below) make a band that hardly shows on a white page (yellow-200 at the
/// marker's opacity is 1.08 to 1 against white); darker ones (600 and above) cost the text on them its contrast.
/// `tests/highlight.rs` holds the numbers for every family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MarkerShade {
    /// Palette shade 300: a pale wash.
    Soft,
    /// Palette shade 400: the marker. The default, and what the old yellow, green, pink, blue and orange map to.
    Standard,
    /// Palette shade 500: a strong marker.
    Strong,
}

impl MarkerShade {
    /// Every marker shade, from the lightest.
    pub const ALL: [MarkerShade; 3] = [MarkerShade::Soft, MarkerShade::Standard, MarkerShade::Strong];

    /// The palette shade (`"yellow-400"` has 400).
    pub const fn number(self) -> u16 {
        match self {
            MarkerShade::Soft => 300,
            MarkerShade::Standard => 400,
            MarkerShade::Strong => 500,
        }
    }

    /// The marker shade with this palette shade, `None` for the other nine.
    pub fn of_number(n: u16) -> Option<MarkerShade> {
        MarkerShade::ALL.into_iter().find(|s| s.number() == n)
    }

    /// The palette shade this marker shade has on the dark canvas: the mirror (`1000 - shade`: 300 is 700, 400 is 600,
    /// 500 stays), because a wash that is pale on white is deep on a dark page. A family that is nearly grey (slate,
    /// gray, zinc, neutral, stone) takes the next lighter shade (600, 500, 400): at 700 its band is only 1.3 to 1
    /// against the page, where the coloured families have 1.4 or more.
    fn dark_number(self, grey: bool) -> u16 {
        let mirrored = 1000 - self.number();
        if grey { mirrored - 100 } else { mirrored }
    }
}

/// A marker colour: a palette family and a marker shade, written like a palette colour (`"yellow-400"`).
///
/// It is a token, not a `#rrggbb`: the same marker has a value on the light canvas and another on the dark one
/// ([`MarkerColor::rgb`]), because the dark filter that maps stored colours would turn a pale yellow into olive.
/// Every band is drawn at [`MARKER_ALPHA`] as one layer, so overlaps do not get darker.
///
/// The five names of the first builds (`yellow`, `green`, `pink`, `blue`, `orange`) still read, each as the same family
/// at the standard shade ([`MarkerColor::from_name`]); they are written as `yellow-400` and so on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MarkerColor {
    /// Index into [`palette::FAMILIES`].
    family: u8,
    shade: MarkerShade,
}

/// The families the old five names were, and what they were called: bare names map to the standard shade.
const LEGACY: [&str; 5] = ["yellow", "green", "pink", "blue", "orange"];

impl MarkerColor {
    /// The default marker: yellow at the standard shade.
    pub const YELLOW: MarkerColor = MarkerColor { family: 3, shade: MarkerShade::Standard };

    /// The marker of a palette family (`"amber"`, ignoring case) in a shade.
    pub fn new(family: &str, shade: MarkerShade) -> Option<MarkerColor> {
        let family = palette::FAMILIES.iter().position(|f| f.name.eq_ignore_ascii_case(family.trim()))?;
        Some(MarkerColor { family: u8::try_from(family).ok()?, shade })
    }

    /// Every marker: each family of the palette in each marker shade, families in palette order.
    pub fn all() -> impl Iterator<Item = MarkerColor> {
        (0..palette::FAMILIES.len())
            .filter_map(|f| u8::try_from(f).ok())
            .flat_map(|family| MarkerShade::ALL.into_iter().map(move |shade| MarkerColor { family, shade }))
    }

    /// The palette family.
    pub fn family(self) -> &'static palette::Family {
        // The index comes from `new`, `all` or `YELLOW`, which keep it inside the table (`marker_names` tests it).
        &palette::FAMILIES[usize::from(self.family)]
    }

    /// The name of the family (`"yellow"`).
    pub fn family_name(self) -> &'static str {
        self.family().name
    }

    /// The marker shade.
    pub fn shade(self) -> MarkerShade {
        self.shade
    }

    /// The name used in files and commands: `<family>-<shade>`, for example `yellow-400`.
    pub fn name(self) -> String {
        format!("{}-{}", self.family_name(), self.shade.number())
    }

    /// The marker called `name`: `<family>-<shade>` with a family of the palette and a marker shade, ignoring case, or
    /// one of the five old names (`yellow`, `green`, `pink`, `blue`, `orange`), which mean the standard shade of that
    /// family.
    pub fn from_name(name: &str) -> Option<MarkerColor> {
        let name = name.trim();
        if let Some((family, shade)) = name.rsplit_once('-') {
            return MarkerColor::new(family, MarkerShade::of_number(shade.parse().ok()?)?);
        }
        LEGACY.iter().find(|l| l.eq_ignore_ascii_case(name)).and_then(|l| MarkerColor::new(l, MarkerShade::Standard))
    }

    /// The marker that a colour stands for, for a tool that takes any colour the stroke picker offers.
    ///
    /// * A marker name (`yellow-400`, or one of the five old names) is that marker.
    /// * Another palette colour `<family>-<shade>` is the marker of that family at the nearest marker shade: the
    ///   shades below 300 (25 to 200) become 300, the shades above 500 (600 to 950) become 500.
    /// * Any other colour (`#rrggbb`, `#rgb`, an alpha is ignored) is the marker, of every family in every marker
    ///   shade, whose ink on the light canvas is nearest in RGB.
    ///
    /// `None` when the text is no colour at all.
    pub fn nearest(text: &str) -> Option<MarkerColor> {
        let text = text.trim();
        if let Some(marker) = MarkerColor::from_name(text) {
            return Some(marker);
        }
        if let Some((family, shade)) = text.rsplit_once('-')
            && let Ok(number) = shade.parse::<u16>()
            && palette::SHADES.contains(&number)
        {
            let marker_shade = MarkerShade::ALL.into_iter().find(|s| s.number() == number.clamp(300, 500))?;
            return MarkerColor::new(family, marker_shade);
        }
        let wanted = Color::parse(text).ok()?;
        let distance = |m: &MarkerColor| {
            let c = m.rgb(Tone::Light);
            let d = |a: u8, b: u8| u32::from(a.abs_diff(b)).pow(2);
            d(c.r, wanted.r) + d(c.g, wanted.g) + d(c.b, wanted.b)
        };
        MarkerColor::all().min_by_key(distance)
    }

    /// True for a family that is nearly grey (slate, gray, zinc, neutral, stone).
    fn is_grey(self) -> bool {
        matches!(self.family_name(), "slate" | "gray" | "zinc" | "neutral" | "stone")
    }

    /// The opaque ink of the marker on a canvas of `tone`. The band is drawn with it at [`MARKER_ALPHA`].
    ///
    /// On the light canvas it is the palette colour of the marker's shade. On the dark canvas it is the same family at
    /// the mirrored shade (see [`MarkerShade`]); the dark palette shades are deep, so the band shows against the page
    /// and the light text on it stays readable. `tests/highlight.rs` holds both contrasts for every marker.
    pub fn rgb(self, tone: Tone) -> Color {
        let number = match tone {
            Tone::Light => self.shade.number(),
            Tone::Dark => self.shade.dark_number(self.is_grey()),
        };
        let column = palette::SHADES.iter().position(|s| *s == number);
        let (r, g, b) = column.and_then(|c| self.family().shades.get(c)).copied().unwrap_or((128, 128, 128));
        Color::rgb(r, g, b)
    }
}

impl fmt::Display for MarkerColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.family_name(), self.shade.number())
    }
}

impl Serialize for MarkerColor {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.name())
    }
}

impl<'de> Deserialize<'de> for MarkerColor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<str>::deserialize(d)?;
        MarkerColor::from_name(&text).ok_or_else(|| serde::de::Error::custom(not_a_marker(&text)))
    }
}

/// The words for a name that is no marker.
fn not_a_marker(text: &str) -> String {
    let shown: String = text.chars().take(40).collect();
    format!(
        "{shown:?} is not a highlight colour. Use `<family>-<shade>` with shade 300, 400 or 500, for example `yellow-400`, \
         and a palette family: {}",
        palette::family_names().join(", ")
    )
}

impl JsonSchema for MarkerColor {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MarkerColor".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let families = palette::family_names().join("|");
        let pattern = format!("^(({families})-(300|400|500)|yellow|green|pink|blue|orange)$");
        schemars::json_schema!({
            "type": "string",
            "pattern": pattern,
            "description": "A highlight colour: `<family>-<shade>`, a palette family and shade 300, 400 or 500 (`yellow-400`). The old names yellow, green, pink, blue and orange mean the 400 shade."
        })
    }
}

/// What a run of text says about its highlight, on top of the element's own.
///
/// This is the rule for text (`docs/model.md`, "Highlight"): the element's `style.highlight` is the default for
/// every character, and a run that carries a value of its own replaces it for the characters of the run.
/// [`SpanHighlight::Off`] is a value too: it takes the highlight away from part of a text that is highlighted as a
/// whole. A character in no run, or in a run that says [`SpanHighlight::Inherit`], follows the element.
///
/// In files and in the shared document it is a string: `"yellow-400"` (any marker colour) or `"none"`; absent is
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
    pub fn name(self) -> Option<String> {
        match self {
            SpanHighlight::Inherit => None,
            SpanHighlight::Off => Some("none".to_owned()),
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
            Some(n) => s.serialize_str(&n),
            None => s.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for SpanHighlight {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Some(text) = Option::<std::borrow::Cow<str>>::deserialize(d)? else { return Ok(SpanHighlight::Inherit) };
        SpanHighlight::from_name(&text).ok_or_else(|| serde::de::Error::custom(not_a_marker(&text)))
    }
}

impl JsonSchema for SpanHighlight {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SpanHighlight".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let marker = MarkerColor::json_schema(g);
        schemars::json_schema!({ "anyOf": [marker, { "type": "string", "const": "none" }] })
    }
}
