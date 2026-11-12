//! Style: how an element looks. Plain values, no behaviour besides small lookups.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::color::Color;

/// Dash pattern of a stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StrokeStyle {
    /// Continuous line.
    #[default]
    Solid,
    /// Dashes.
    Dashed,
    /// Dots.
    Dotted,
}

impl StrokeStyle {
    /// The on and off lengths of the dash pattern for a line `width` wide, `None` for a continuous line. Plain
    /// outlines (an image's, a frame's) use this; sketched shapes get their dashes from the sketch generator.
    pub fn dash(self, width: f64) -> Option<[f64; 2]> {
        match self {
            StrokeStyle::Solid => None,
            StrokeStyle::Dashed => Some([width * 4.0, width * 2.0]),
            StrokeStyle::Dotted => Some([width, width * 2.0]),
        }
    }
}

/// How a filled area is painted. The fill colour itself is `Style::fill_color`; `None` there means no fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
pub enum FillPattern {
    /// Flat colour.
    #[default]
    #[serde(rename = "solid")]
    Solid,
    /// Parallel hand-drawn lines.
    #[serde(rename = "hachure")]
    Hachure,
    /// A continuous zig-zag.
    #[serde(rename = "zigzag")]
    ZigZag,
    /// Two crossing sets of lines.
    #[serde(rename = "cross_hatch")]
    CrossHatch,
    /// Dots.
    #[serde(rename = "dots")]
    Dots,
    /// Dashed hatch lines.
    #[serde(rename = "dashed")]
    Dashed,
    /// Zig-zag drawn as separate line pieces.
    #[serde(rename = "zigzag_line")]
    ZigZagLine,
}

/// How hand-drawn the outline looks. The names are the old app's; `roughness` gives the numeric value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sloppiness {
    /// Roughness 0: clean lines.
    Architect,
    /// Roughness 1.
    #[default]
    Artist,
    /// Roughness 2.
    Cartoonist,
    /// Roughness 3.5.
    Drunk,
}

impl Sloppiness {
    /// The roughness value the sketch generator takes.
    pub fn roughness(self) -> f64 {
        match self {
            Sloppiness::Architect => 0.0,
            Sloppiness::Artist => 1.0,
            Sloppiness::Cartoonist => 2.0,
            Sloppiness::Drunk => 3.5,
        }
    }
}

/// Visual style shared by every element kind.
///
/// Text uses `stroke_color` as its text colour; images use the stroke as an optional outline, and a new image has
/// none ([`Style::for_image`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Style {
    /// Outline colour (text colour for text and math).
    pub stroke_color: Color,
    /// Outline width in world units.
    pub stroke_width: f64,
    /// Dash pattern of the outline.
    pub stroke_style: StrokeStyle,
    /// Fill colour. `None` is no fill.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<Color>,
    /// Fill pattern. Ignored while there is no fill colour.
    pub fill_pattern: FillPattern,
    /// How hand-drawn the outline is.
    pub sloppiness: Sloppiness,
    /// Opacity of the whole element, 0 (invisible) to 1.
    pub opacity: f64,
    /// Corner radius in world units, 0 for sharp corners. Used by rectangle, diamond, frame, line and arrow.
    pub corner_radius: f64,
    /// Seed of the hand-drawn jitter, so the same element looks the same on every peer and every render.
    pub seed: u32,
}

impl Default for Style {
    /// The old app's defaults for new shapes: indigo-500 stroke, width 2, no fill, artist, opaque, sharp.
    fn default() -> Self {
        Self {
            stroke_color: Color::rgb(0x63, 0x66, 0xf1),
            stroke_width: 2.0,
            stroke_style: StrokeStyle::Solid,
            fill_color: None,
            fill_pattern: FillPattern::Solid,
            sloppiness: Sloppiness::Artist,
            opacity: 1.0,
            corner_radius: 0.0,
            seed: 0,
        }
    }
}

impl Style {
    /// The style of a new image: no outline, and the defaults otherwise.
    ///
    /// A picture is visible without a stroke, so the stroke colour is clear ("no stroke"). The width and dash stay
    /// at the defaults: when someone picks a stroke colour for the image, the outline shows at once at the usual
    /// width. `seed` is the element's own seed.
    pub fn for_image(seed: u32) -> Self {
        Self { stroke_color: Color::TRANSPARENT, seed, ..Self::default() }
    }
}

/// Font family. `GelPen`, `GelPenSerif`, `VanillaExtract` and `PlexSans` ship with the app; `NotoSans` is the old
/// fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
pub enum FontFamily {
    /// Hand-written sans (the default).
    #[default]
    #[serde(rename = "gelpen")]
    GelPen,
    /// Noto Sans.
    #[serde(rename = "noto_sans")]
    NotoSans,
    /// Hand-written serif.
    #[serde(rename = "gelpen_serif")]
    GelPenSerif,
    /// Vanilla Extract.
    #[serde(rename = "vanilla_extract")]
    VanillaExtract,
    /// IBM Plex Sans, a plain sans with real light, regular and bold weights.
    #[serde(rename = "plex_sans")]
    PlexSans,
}

impl FontFamily {
    /// The family name people and exported files see (SVG `font-family`, the old app's `font_family`). The text
    /// layout has its own table of face names in `typeset`; this is not what Parley is asked for.
    pub fn display_name(self) -> &'static str {
        match self {
            FontFamily::GelPen => "GelPen",
            FontFamily::NotoSans => "Noto Sans",
            FontFamily::GelPenSerif => "GelPen Serif",
            FontFamily::VanillaExtract => "Vanilla Extract",
            FontFamily::PlexSans => "IBM Plex Sans",
        }
    }
}

/// Font weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FontWeight {
    /// Light.
    Light,
    /// Regular.
    #[default]
    Regular,
    /// Heavy.
    Heavy,
}

/// Horizontal text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    /// Left.
    #[default]
    Left,
    /// Centre.
    Center,
    /// Right.
    Right,
}

/// Vertical alignment of text inside its box or container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerticalAlign {
    /// Top.
    #[default]
    Top,
    /// Middle.
    Middle,
    /// Bottom.
    Bottom,
}

/// Whether `$...$` in a text is typeset as inline math.
///
/// The text stays plain: the dollar signs are part of `content`, and so is the escape `\$`. This setting only says how
/// the text is drawn and measured (`docs/model.md` section 4, "Inline math").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InlineMath {
    /// A matched `$...$` pair is a formula when the first `$` is followed by a non-space and the second is preceded
    /// by one and not followed by a digit (prices stay prices). `\$` is a literal dollar sign.
    #[default]
    Auto,
    /// Every character is drawn as written.
    Off,
}

impl InlineMath {
    /// Whether this is the default (`Auto`); a default is not written to files.
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}

/// Font settings of a text element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TextStyle {
    /// Font family.
    pub family: FontFamily,
    /// Font size in world units.
    pub size: f64,
    /// Font weight.
    pub weight: FontWeight,
    /// Horizontal alignment.
    pub align: TextAlign,
    /// Vertical alignment inside the element box (matters for text bound to a container).
    pub vertical_align: VerticalAlign,
    /// Line height as a multiple of the font size.
    pub line_height: f64,
    /// Whether `$...$` in the text is typeset as math. Default `auto`.
    #[serde(skip_serializing_if = "InlineMath::is_auto")]
    pub inline_math: InlineMath,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            family: FontFamily::GelPen,
            size: 20.0,
            weight: FontWeight::Regular,
            align: TextAlign::Left,
            vertical_align: VerticalAlign::Top,
            line_height: 1.25,
            inline_math: InlineMath::Auto,
        }
    }
}
