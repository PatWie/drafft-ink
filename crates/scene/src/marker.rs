//! Highlight: the text-marker band behind an element.
//!
//! A highlight is a palette token or a custom colour ([`MarkerColor`]: a family and a palette shade, `yellow-400`, or
//! `#rrggbb`). A palette token is not a raw colour, because the same marker has to read well on a white canvas and
//! on the dark one. The stored colours of strokes and fills are theme independent and the dark canvas derives its
//! look from them with a filter (`Color::for_dark_theme`); a palette marker has its own pair of values instead,
//! because that filter turns a pale yellow into a muddy olive. A custom colour is exactly what the person picked, on
//! both canvases. [`MarkerColor::rgb`] gives the value for a [`Tone`]; every band is drawn at [`MARKER_ALPHA`] as one
//! layer, so overlaps do not get darker.

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

/// A palette shade of a marker: one of the twelve of [`palette::SHADES`] (25, 50, 100 … 900, 950).
///
/// A palette marker has two values, one for each canvas. The dark one is the same family at the mirror shade
/// ([`Shade::dark`]), so a wash that is pale on white is deep on a dark page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Shade(u8);

impl Shade {
    /// Every shade, from the lightest (25) to the deepest (950).
    pub const ALL: [Shade; 12] = [
        Shade(0),
        Shade(1),
        Shade(2),
        Shade(3),
        Shade(4),
        Shade(5),
        Shade(6),
        Shade(7),
        Shade(8),
        Shade(9),
        Shade(10),
        Shade(11),
    ];

    /// The shade of the default marker and of the old names: 400.
    pub const DEFAULT: Shade = Shade(5);

    /// The shades the pickers offer first: 300 (a pale wash), 400 (the marker) and 500 (a strong one). Lighter
    /// shades make a band that hardly shows on a white page (yellow-200 at the marker's opacity is 1.08 to 1 against
    /// white) and darker ones cost the text on them its contrast, but both are allowed.
    pub const MARKER: [Shade; 3] = [Shade(4), Shade(5), Shade(6)];

    /// The label (`400` for yellow-400).
    pub fn number(self) -> u16 {
        palette::SHADES.get(usize::from(self.0)).copied().unwrap_or(400)
    }

    /// The shade labelled `n`, `None` for a number that is no palette shade.
    pub fn from_number(n: u16) -> Option<Shade> {
        let index = palette::SHADES.iter().position(|s| *s == n)?;
        u8::try_from(index).ok().map(Shade)
    }

    /// The position in [`palette::SHADES`] and in a family's `shades`.
    pub fn index(self) -> usize {
        usize::from(self.0)
    }

    /// The shade a palette marker has on the dark canvas: the mirror, `1000 - shade`, rounded to the nearest shade
    /// the palette has.
    ///
    /// | light | 25 | 50 | 100 | 200 | 300 | 400 | 500 | 600 | 700 | 800 | 900 | 950 |
    /// |---|---|---|---|---|---|---|---|---|---|---|---|---|
    /// | dark | 950 | 950 | 900 | 800 | 700 | 600 | 500 | 400 | 300 | 200 | 100 | 50 |
    ///
    /// 1000 − 25 is 975, which the palette lacks, so 25 takes 950 as well and 950 takes 50: that is the one place
    /// the mapping is not its own inverse. 300 to 500 keep the values they had when they were the only marker shades.
    pub fn dark(self) -> Shade {
        let wanted = 1000 - self.number();
        Shade::ALL.into_iter().min_by_key(|s| s.number().abs_diff(wanted)).unwrap_or(self)
    }

    /// The next lighter shade (25 stays 25).
    fn lighter(self) -> Shade {
        Shade(self.0.saturating_sub(1))
    }
}

impl fmt::Display for Shade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.number())
    }
}

/// A marker colour: a palette colour or a custom one.
///
/// * [`MarkerColor::Palette`] is a family and any of the twelve palette shades, written like a palette colour
///   (`"yellow-400"`). It is a token, not a `#rrggbb`: the same marker has a value on the light canvas and another
///   on the dark one ([`MarkerColor::rgb`]), because the dark filter that maps stored colours would turn a pale
///   yellow into olive. The dark value is the same family at the mirror shade, see [`Shade::dark`].
/// * [`MarkerColor::Custom`] is a colour of the person's choice, written `#rrggbb`. It is the same on both canvases.
///   Build it with [`MarkerColor::custom`], which drops the alpha.
///
/// Every band is drawn at [`MARKER_ALPHA`] as one layer, so overlaps do not get darker. The five names of the first
/// builds (`yellow`, `green`, `pink`, `blue`, `orange`) still read, each as the same family at shade 400
/// ([`MarkerColor::from_name`]); they are written as `yellow-400` and so on.
///
/// No colour is refused for its contrast: readability is the person's choice. The shades 300, 400 and 500 are the
/// ones the pickers offer first ([`Shade::MARKER`]), and `tests/highlight.rs` holds their contrast numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkerColor {
    /// A palette family in a palette shade; the dark canvas shows the mirror shade.
    Palette {
        /// Index into [`palette::FAMILIES`].
        family: u8,
        /// The shade.
        shade: Shade,
    },
    /// An opaque colour, the same on both canvases.
    Custom(Color),
}

/// The families the old five names were, and what they were called: bare names map to shade 400.
const LEGACY: [&str; 5] = ["yellow", "green", "pink", "blue", "orange"];

impl MarkerColor {
    /// The default marker: yellow-400.
    pub const YELLOW: MarkerColor = MarkerColor::Palette { family: 3, shade: Shade::DEFAULT };

    /// The marker of a palette family (`"amber"`, ignoring case) in a shade.
    pub fn new(family: &str, shade: Shade) -> Option<MarkerColor> {
        let family = palette::FAMILIES.iter().position(|f| f.name.eq_ignore_ascii_case(family.trim()))?;
        Some(MarkerColor::Palette { family: u8::try_from(family).ok()?, shade })
    }

    /// The custom marker of `color`: its red, green and blue, with the alpha dropped (the band's opacity is
    /// [`MARKER_ALPHA`], whatever the colour says).
    pub fn custom(color: Color) -> MarkerColor {
        MarkerColor::Custom(Color::rgb(color.r, color.g, color.b))
    }

    /// Every palette marker: each family in each of the twelve shades, families in palette order.
    pub fn palette() -> impl Iterator<Item = MarkerColor> {
        (0..palette::FAMILIES.len())
            .filter_map(|f| u8::try_from(f).ok())
            .flat_map(|family| Shade::ALL.into_iter().map(move |shade| MarkerColor::Palette { family, shade }))
    }

    /// The palette family, `None` for a custom colour.
    pub fn family(self) -> Option<&'static palette::Family> {
        match self {
            // The index comes from `new` or `palette()`, which keep it inside the table (`marker_names` tests it).
            MarkerColor::Palette { family, .. } => palette::FAMILIES.get(usize::from(family)),
            MarkerColor::Custom(_) => None,
        }
    }

    /// The name of the palette family (`"yellow"`), `None` for a custom colour.
    pub fn family_name(self) -> Option<&'static str> {
        self.family().map(|f| f.name)
    }

    /// The palette shade, `None` for a custom colour.
    pub fn shade(self) -> Option<Shade> {
        match self {
            MarkerColor::Palette { shade, .. } => Some(shade),
            MarkerColor::Custom(_) => None,
        }
    }

    /// The name used in files and commands: `<family>-<shade>` (`yellow-400`) for a palette colour, `#rrggbb` for a
    /// custom one.
    pub fn name(self) -> String {
        self.to_string()
    }

    /// The marker called `name`, ignoring case and surrounding space:
    ///
    /// * `<family>-<shade>` with a family of the palette and one of the twelve shades;
    /// * `#rgb`, `#rrggbb` or `#rrggbbaa`, a custom colour (the alpha is dropped);
    /// * one of the five old names (`yellow`, `green`, `pink`, `blue`, `orange`): that family at shade 400.
    ///
    /// `None` for anything else.
    pub fn from_name(name: &str) -> Option<MarkerColor> {
        let name = name.trim();
        if name.starts_with('#') {
            return Color::parse(name).ok().map(MarkerColor::custom);
        }
        if let Some((family, shade)) = name.rsplit_once('-') {
            return MarkerColor::new(family, Shade::from_number(shade.parse().ok()?)?);
        }
        LEGACY.iter().find(|l| l.eq_ignore_ascii_case(name)).and_then(|l| MarkerColor::new(l, Shade::DEFAULT))
    }

    /// True for a family that is nearly grey (slate, gray, zinc, neutral, stone).
    fn is_grey(self) -> bool {
        matches!(self.family_name(), Some("slate" | "gray" | "zinc" | "neutral" | "stone"))
    }

    /// The opaque ink of the marker on a canvas of `tone`. The band is drawn with it at [`MARKER_ALPHA`].
    ///
    /// A palette marker is the palette colour of its shade on the light canvas and of [`Shade::dark`] on the dark
    /// one. A family that is nearly grey (slate, gray, zinc, neutral, stone) takes the next lighter shade on the dark
    /// canvas: at the mirror of 300, 700, its band is only 1.3 to 1 against the page, where the coloured families
    /// have 1.4 or more. A custom marker is the same on both canvases.
    pub fn rgb(self, tone: Tone) -> Color {
        match self {
            MarkerColor::Custom(c) => Color::rgb(c.r, c.g, c.b),
            MarkerColor::Palette { shade, .. } => {
                let shade = match tone {
                    Tone::Light => shade,
                    Tone::Dark if self.is_grey() => shade.dark().lighter(),
                    Tone::Dark => shade.dark(),
                };
                let (r, g, b) =
                    self.family().and_then(|f| f.shades.get(shade.index())).copied().unwrap_or((128, 128, 128));
                Color::rgb(r, g, b)
            }
        }
    }
}

impl fmt::Display for MarkerColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MarkerColor::Palette { shade, .. } => {
                write!(f, "{}-{}", self.family_name().unwrap_or_default(), shade.number())
            }
            MarkerColor::Custom(c) => f.write_str(&c.to_hex()),
        }
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
        "{shown:?} is not a highlight colour. Use `<family>-<shade>` with a shade of 25, 50, 100, 200, 300, 400, 500, \
         600, 700, 800, 900 or 950 (for example `yellow-400`), or a `#rrggbb` colour. The families are: {}",
        palette::family_names().join(", ")
    )
}

impl JsonSchema for MarkerColor {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MarkerColor".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let families = palette::family_names().join("|");
        let shades = palette::SHADES.map(|s| s.to_string()).join("|");
        let pattern = format!(
            "^(({families})-({shades})|yellow|green|pink|blue|orange|#[0-9a-fA-F]{{3}}|#[0-9a-fA-F]{{6}}|#[0-9a-fA-F]{{8}})$"
        );
        schemars::json_schema!({
            "type": "string",
            "pattern": pattern,
            "description": "A highlight colour: `<family>-<shade>`, a palette family and any palette shade (`yellow-400`), or `#rrggbb`. The old names yellow, green, pink, blue and orange mean the 400 shade."
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
