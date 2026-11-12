//! Style: which style property applies to which element, how a change is validated and written, the defaults for new
//! elements, and the named themes.
//!
//! The `style.*` commands validate their arguments and delegate here. A rule lives in this file only.
//!
//! * [`StyleProps`] is the argument bundle: every property is optional and at least one must be set. [`Prop`] names a
//!   single property.
//! * [`applies`] is the applicability table of `docs/commands.md` §2.7. A property an element does not support is
//!   skipped for it and listed in [`StyleReport::skipped`].
//! * Text properties on a shape or arrow that has a bound label reach the label ([`apply`] with `include_bound_text`).
//! * [`apply`] writes a change in the caller's transaction and refuses locked elements before it writes anything.
//! * [`StyleDefaults`] is the style new elements get. [`themes`] are named bundles.
//!
//! Text boxes are measured by layout when their content changes. [`apply`] changes font properties but does not
//! re-measure, because this crate has no text measurer. The caller that owns a measurer re-lays out the elements in
//! [`StyleReport::applied`] when a font property is among them.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

use drafftink_scene::limits::{MAX_CORNER_RADIUS, MAX_FONT_SIZE, MAX_STROKE_WIDTH};
use drafftink_scene::palette;
use drafftink_scene::words::Article;
use drafftink_scene::{
    Brush, Color, Element, ElementId, ElementKind, FillPattern, FontFamily, FontWeight, Kind, PressureMode, SceneRead,
    SceneTx, Sloppiness, StoreError, StrokeStyle, Style, TextAlign, TextStyle, VerticalAlign,
};

use crate::target::TargetError;
use crate::text_bind::labels_of;

/// Smallest line height a command accepts, as a multiple of the font size.
pub const MIN_LINE_HEIGHT: f64 = 1.0;
/// Largest line height a command accepts.
pub const MAX_LINE_HEIGHT: f64 = 3.0;

// ---------------------------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------------------------

/// One style property. The declaration order is the order used in every list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Prop {
    StrokeColor,
    FillColor,
    FillPattern,
    StrokeWidth,
    StrokeStyle,
    Sloppiness,
    CornerRadius,
    Opacity,
    FreehandStyle,
    PressureMode,
    FontFamily,
    FontWeight,
    FontSize,
    TextAlign,
    VerticalAlign,
    LineHeight,
}

impl Prop {
    /// Every property, in canonical order.
    pub const ALL: [Prop; 16] = [
        Prop::StrokeColor,
        Prop::FillColor,
        Prop::FillPattern,
        Prop::StrokeWidth,
        Prop::StrokeStyle,
        Prop::Sloppiness,
        Prop::CornerRadius,
        Prop::Opacity,
        Prop::FreehandStyle,
        Prop::PressureMode,
        Prop::FontFamily,
        Prop::FontWeight,
        Prop::FontSize,
        Prop::TextAlign,
        Prop::VerticalAlign,
        Prop::LineHeight,
    ];

    /// The name used in JSON (`stroke_color`).
    pub fn name(self) -> &'static str {
        match self {
            Prop::StrokeColor => "stroke_color",
            Prop::FillColor => "fill_color",
            Prop::FillPattern => "fill_pattern",
            Prop::StrokeWidth => "stroke_width",
            Prop::StrokeStyle => "stroke_style",
            Prop::Sloppiness => "sloppiness",
            Prop::CornerRadius => "corner_radius",
            Prop::Opacity => "opacity",
            Prop::FreehandStyle => "freehand_style",
            Prop::PressureMode => "pressure_mode",
            Prop::FontFamily => "font_family",
            Prop::FontWeight => "font_weight",
            Prop::FontSize => "font_size",
            Prop::TextAlign => "text_align",
            Prop::VerticalAlign => "vertical_align",
            Prop::LineHeight => "line_height",
        }
    }

    /// The property called `name`.
    pub fn from_name(name: &str) -> Option<Prop> {
        Prop::ALL.into_iter().find(|p| p.name() == name)
    }

    /// True for the properties that belong to text (and reach a bound label through its container).
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Prop::FontFamily
                | Prop::FontWeight
                | Prop::FontSize
                | Prop::TextAlign
                | Prop::VerticalAlign
                | Prop::LineHeight
        )
    }

    /// The elements the property applies to, in words, for messages.
    pub fn applies_to(self) -> &'static str {
        match self {
            Prop::StrokeColor => "every element",
            Prop::FillColor | Prop::FillPattern => {
                "rectangles, diamonds, ellipses, frames, freehand strokes and closed lines"
            }
            Prop::StrokeWidth => "every element except text and math",
            Prop::StrokeStyle => "rectangles, diamonds, ellipses, frames, images, lines, arrows and freehand strokes",
            Prop::Sloppiness => "rectangles, diamonds, ellipses, frames, lines and arrows",
            Prop::CornerRadius => "rectangles, diamonds, frames, lines and arrows",
            Prop::Opacity => "every element",
            Prop::FreehandStyle | Prop::PressureMode => "freehand strokes",
            Prop::FontFamily | Prop::FontWeight | Prop::TextAlign | Prop::LineHeight => {
                "text, and shapes and arrows that have a label"
            }
            Prop::FontSize => "text and math, and shapes and arrows that have a label",
            Prop::VerticalAlign => "text bound to a shape or arrow, and shapes and arrows that have a label",
        }
    }
}

/// True when `prop` applies to `e` itself. Text properties of a container are not decided here: see [`apply`].
pub fn applies(e: &Element, prop: Prop) -> bool {
    let k = e.tag();
    let closed_line = matches!(&e.kind, ElementKind::Line(d) if d.closed);
    let fillable =
        matches!(k, Kind::Rectangle | Kind::Diamond | Kind::Ellipse | Kind::Frame | Kind::Freehand) || closed_line;
    match prop {
        Prop::StrokeColor => true,
        Prop::FillColor | Prop::FillPattern => fillable,
        Prop::StrokeWidth | Prop::StrokeStyle => !matches!(k, Kind::Text | Kind::Math),
        Prop::Sloppiness => !matches!(k, Kind::Image | Kind::Text | Kind::Math | Kind::Freehand),
        Prop::CornerRadius => matches!(k, Kind::Rectangle | Kind::Diamond | Kind::Line | Kind::Arrow | Kind::Frame),
        Prop::Opacity => true,
        Prop::FreehandStyle | Prop::PressureMode => k == Kind::Freehand,
        Prop::FontFamily | Prop::FontWeight | Prop::TextAlign | Prop::LineHeight => k == Kind::Text,
        Prop::FontSize => matches!(k, Kind::Text | Kind::Math),
        Prop::VerticalAlign => e.container_id().is_some(),
    }
}

/// True when a fill can paint something for an element of `kind`: shapes, and closed lines and pencil strokes.
fn fill_paints(kind: Kind, closed: bool) -> bool {
    match kind {
        Kind::Rectangle | Kind::Diamond | Kind::Ellipse => true,
        Kind::Line | Kind::Freehand => closed,
        _ => false,
    }
}

/// Whether an element of `kind` shows anything with `style`. A stroke colour with alpha 0 is "no stroke" (what
/// Excalidraw calls `transparent`), and no fill is a missing or fully clear fill colour.
///
/// Rectangles, diamonds, ellipses, closed lines and closed pencil strokes show with a fill alone. Everything else
/// is drawn in its stroke colour: text, formulas, open lines, arrows, open pencil strokes, and frames (outline and
/// name). Images always show: their stroke is an optional outline, so "no stroke" is allowed for them.
pub fn style_shows(kind: Kind, closed: bool, style: &Style) -> bool {
    if kind == Kind::Image {
        return true;
    }
    let fills = fill_paints(kind, closed) && style.fill_color.is_some_and(|c| c.a > 0);
    style.stroke_color.a > 0 || fills
}

fn is_closed(e: &Element) -> bool {
    match &e.kind {
        ElementKind::Line(d) | ElementKind::Arrow(d) => d.closed,
        ElementKind::Freehand(f) => f.closed && !f.highlighter,
        _ => false,
    }
}

/// True when `e` shows anything (see [`style_shows`]).
pub fn is_visible(e: &Element) -> bool {
    style_shows(e.tag(), is_closed(e), &e.style)
}

/// Refuses a style that would leave an element of `kind` showing nothing (see [`style_shows`]).
pub fn check_shows(kind: Kind, closed: bool, style: &Style) -> Result<(), StyleError> {
    if style_shows(kind, closed, style) {
        Ok(())
    } else {
        Err(StyleError::Invisible { kind, fill_helps: fill_paints(kind, closed) })
    }
}

/// Refuses the write of `part` to `old` when it touches a colour and `old` would then show nothing.
fn check_visible(old: &Element, part: &StyleProps) -> Result<(), StyleError> {
    if part.stroke_color.is_none() && part.fill_color.is_none() {
        return Ok(());
    }
    let mut new = old.clone();
    part.write(&mut new);
    check_shows(new.tag(), is_closed(&new), &new.style)
}

/// Why `prop` (the stroke or the fill) cannot be set to none on `targets` now: the refusal `style.set` would give.
/// `None` when it can, and for every other property. A target that is missing, deleted or locked is left to the
/// refusal of the write itself.
pub fn none_refusal<R: SceneRead>(r: &R, targets: &[ElementId], prop: Prop) -> Option<StyleError> {
    let none = match prop {
        Prop::StrokeColor => StyleProps { stroke_color: Some(ColorArg(Color::TRANSPARENT)), ..StyleProps::default() },
        Prop::FillColor => StyleProps { fill_color: Some(None), ..StyleProps::default() },
        _ => return None,
    };
    targets
        .iter()
        .filter_map(|id| r.get(id))
        .filter(|e| !e.deleted && !e.locked && applies(e, prop))
        .find_map(|e| check_visible(e, &none).err())
}

// ---------------------------------------------------------------------------------------------
// Colour argument
// ---------------------------------------------------------------------------------------------

/// A colour as a command argument: `#rgb`, `#rrggbb`, `#rrggbbaa`, `transparent`, or a palette name such as
/// `blue-500` (22 families by shades 25 to 950). It is stored as plain `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorArg(pub Color);

impl ColorArg {
    /// Parses a colour or a palette name.
    pub fn parse(s: &str) -> Result<Self, String> {
        if let Some(c) = palette::lookup(s) {
            return Ok(Self(c));
        }
        Color::parse(s).map(Self).map_err(|_| {
            let shown: String = s.chars().take(40).collect();
            format!(
                "`{shown}` is not a colour. Use #rrggbb, #rrggbbaa, `transparent`, or a palette name such as `blue-500` \
                 (families: {}; shades 25, 50, 100 to 900, 950)",
                palette::family_names().join(", ")
            )
        })
    }
}

impl Serialize for ColorArg {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0.a == 0 { s.serialize_str("transparent") } else { s.serialize_str(&self.0.to_hex()) }
    }
}

impl<'de> Deserialize<'de> for ColorArg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        ColorArg::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ColorArg {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ColorArg".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "#rrggbb, #rrggbbaa, #rgb, 'transparent', or a palette name '<family>-<shade>' such as 'blue-500' (22 Tailwind families, shades 25, 50, 100-900, 950)."
        })
    }
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

// ---------------------------------------------------------------------------------------------
// The argument bundle
// ---------------------------------------------------------------------------------------------

/// A set of style properties to change. Absent properties stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct StyleProps {
    /// Outline colour; the text colour of text and math.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke_color: Option<ColorArg>,
    /// Fill colour. `null` removes the fill.
    #[serde(deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ColorArg>")]
    pub fill_color: Option<Option<ColorArg>>,
    /// How the fill is painted: solid, hachure, zigzag, cross_hatch, dots, dashed or zigzag_line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_pattern: Option<FillPattern>,
    /// Outline width in world units, above 0 (presets 1, 2, 4, 8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f64>,
    /// solid, dashed or dotted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke_style: Option<StrokeStyle>,
    /// How hand-drawn the outline is: architect, artist, cartoonist or drunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sloppiness: Option<Sloppiness>,
    /// Corner radius in world units, 0 or more (0 is sharp).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<f64>,
    /// Opacity from 0 (invisible) to 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Freehand brush: normal or calligraphy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freehand_style: Option<Brush>,
    /// Freehand pressure: uniform, simulated or device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_mode: Option<PressureMode>,
    /// Font: gelpen, gelpen_serif, vanilla_extract, noto_sans or plex_sans.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_family: Option<FontFamily>,
    /// Font weight: light, regular or heavy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<FontWeight>,
    /// Font size in world units, above 0 (presets 16, 20, 28, 36).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    /// Horizontal text alignment: left, center or right.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_align: Option<TextAlign>,
    /// Vertical alignment of text bound to a container: top, middle or bottom.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical_align: Option<VerticalAlign>,
    /// Line height as a multiple of the font size, 1.0 to 3.0 (default 1.25).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
}

/// Why a style change was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StyleError {
    /// No property was given.
    #[error("no style property was given")]
    Empty,
    /// A number is outside its range.
    #[error("`{}` {expected}", prop.name())]
    OutOfRange {
        /// The property.
        prop: Prop,
        /// What is accepted, as a phrase ("must be a number above 0 and at most 1000").
        expected: String,
    },
    /// An element does not exist, is deleted, or is locked.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// No target supports any of the properties.
    #[error("none of the targets support {}", names(.props))]
    NotApplicable {
        /// The properties that were asked for.
        props: Vec<Prop>,
    },
    /// The write would leave an element with nothing to see (see [`is_visible`]).
    #[error("{}", invisible_text(*kind, *fill_helps))]
    Invisible {
        /// The kind of the element that would vanish.
        kind: Kind,
        /// A fill could keep this kind visible (a shape or a closed line), so the advice is about the fill.
        fill_helps: bool,
    },
    /// The defaults for new elements cannot have no stroke.
    #[error(
        "New elements always get a stroke, because arrows, lines and text are drawn with it. To draw shapes without an outline, select one that has a fill and set its stroke to none."
    )]
    DefaultStroke,
    /// The store refused the write.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The words of [`StyleError::Invisible`].
fn invisible_text(kind: Kind, fill_helps: bool) -> String {
    let this = Article::new(kind.name()).capitalized();
    if fill_helps {
        format!("{this} with neither a stroke nor a fill shows nothing. Keep the stroke, or give it a fill first.")
    } else {
        format!("{this} is drawn in its stroke colour, so the stroke cannot be none. Pick a colour for it instead.")
    }
}

fn names(props: &[Prop]) -> String {
    props.iter().map(|p| format!("`{}`", p.name())).collect::<Vec<_>>().join(", ")
}

fn range(prop: Prop, v: f64, min: f64, max: f64, lo_open: bool) -> Result<(), StyleError> {
    let ok = v.is_finite() && v <= max && if lo_open { v > min } else { v >= min };
    if ok {
        return Ok(());
    }
    let expected = if lo_open {
        format!("must be a number above {min} and at most {max}")
    } else {
        format!("must be a number from {min} to {max}")
    };
    Err(StyleError::OutOfRange { prop, expected })
}

impl StyleProps {
    /// The properties that are set, in canonical order.
    pub fn set_props(&self) -> Vec<Prop> {
        Prop::ALL.into_iter().filter(|p| self.value(*p).is_some()).collect()
    }

    /// True when no property is set.
    pub fn is_empty(&self) -> bool {
        self.set_props().is_empty()
    }

    /// The value of a property as JSON, `None` when it is not set. A removed fill is `Some(null)`.
    pub fn value(&self, p: Prop) -> Option<Value> {
        match p {
            Prop::StrokeColor => self.stroke_color.and_then(|c| serde_json::to_value(c).ok()),
            Prop::FillColor => {
                self.fill_color.and_then(|c| c.map_or(Some(Value::Null), |c| serde_json::to_value(c).ok()))
            }
            Prop::FillPattern => self.fill_pattern.and_then(|v| serde_json::to_value(v).ok()),
            Prop::StrokeWidth => self.stroke_width.map(|v| json!(v)),
            Prop::StrokeStyle => self.stroke_style.and_then(|v| serde_json::to_value(v).ok()),
            Prop::Sloppiness => self.sloppiness.and_then(|v| serde_json::to_value(v).ok()),
            Prop::CornerRadius => self.corner_radius.map(|v| json!(v)),
            Prop::Opacity => self.opacity.map(|v| json!(v)),
            Prop::FreehandStyle => self.freehand_style.and_then(|v| serde_json::to_value(v).ok()),
            Prop::PressureMode => self.pressure_mode.and_then(|v| serde_json::to_value(v).ok()),
            Prop::FontFamily => self.font_family.and_then(|v| serde_json::to_value(v).ok()),
            Prop::FontWeight => self.font_weight.and_then(|v| serde_json::to_value(v).ok()),
            Prop::FontSize => self.font_size.map(|v| json!(v)),
            Prop::TextAlign => self.text_align.and_then(|v| serde_json::to_value(v).ok()),
            Prop::VerticalAlign => self.vertical_align.and_then(|v| serde_json::to_value(v).ok()),
            Prop::LineHeight => self.line_height.map(|v| json!(v)),
        }
    }

    /// True when the stroke is set to a fully clear colour: "no stroke".
    pub fn clears_stroke(&self) -> bool {
        self.stroke_color.is_some_and(|c| c.0.a == 0)
    }

    /// Checks that at least one property is set and that every number is in range.
    pub fn validate(&self) -> Result<(), StyleError> {
        if self.is_empty() {
            return Err(StyleError::Empty);
        }
        if let Some(v) = self.stroke_width {
            range(Prop::StrokeWidth, v, 0.0, MAX_STROKE_WIDTH, true)?;
        }
        if let Some(v) = self.corner_radius {
            range(Prop::CornerRadius, v, 0.0, MAX_CORNER_RADIUS, false)?;
        }
        if let Some(v) = self.opacity {
            range(Prop::Opacity, v, 0.0, 1.0, false)?;
        }
        if let Some(v) = self.font_size {
            range(Prop::FontSize, v, 0.0, MAX_FONT_SIZE, true)?;
        }
        if let Some(v) = self.line_height {
            range(Prop::LineHeight, v, MIN_LINE_HEIGHT, MAX_LINE_HEIGHT, false)?;
        }
        Ok(())
    }

    /// Keeps only the properties in `only`.
    fn restricted(&self, only: &BTreeSet<Prop>) -> StyleProps {
        let keep = |p: Prop| only.contains(&p);
        StyleProps {
            stroke_color: self.stroke_color.filter(|_| keep(Prop::StrokeColor)),
            fill_color: self.fill_color.filter(|_| keep(Prop::FillColor)),
            fill_pattern: self.fill_pattern.filter(|_| keep(Prop::FillPattern)),
            stroke_width: self.stroke_width.filter(|_| keep(Prop::StrokeWidth)),
            stroke_style: self.stroke_style.filter(|_| keep(Prop::StrokeStyle)),
            sloppiness: self.sloppiness.filter(|_| keep(Prop::Sloppiness)),
            corner_radius: self.corner_radius.filter(|_| keep(Prop::CornerRadius)),
            opacity: self.opacity.filter(|_| keep(Prop::Opacity)),
            freehand_style: self.freehand_style.filter(|_| keep(Prop::FreehandStyle)),
            pressure_mode: self.pressure_mode.filter(|_| keep(Prop::PressureMode)),
            font_family: self.font_family.filter(|_| keep(Prop::FontFamily)),
            font_weight: self.font_weight.filter(|_| keep(Prop::FontWeight)),
            font_size: self.font_size.filter(|_| keep(Prop::FontSize)),
            text_align: self.text_align.filter(|_| keep(Prop::TextAlign)),
            vertical_align: self.vertical_align.filter(|_| keep(Prop::VerticalAlign)),
            line_height: self.line_height.filter(|_| keep(Prop::LineHeight)),
        }
    }

    /// Writes the set properties into `e`. The caller has checked [`applies`] for each of them.
    fn write(&self, e: &mut Element) {
        if let Some(c) = self.stroke_color {
            e.style.stroke_color = c.0;
        }
        if let Some(c) = self.fill_color {
            e.style.fill_color = c.map(|c| c.0);
        }
        if let Some(v) = self.fill_pattern {
            e.style.fill_pattern = v;
        }
        if let Some(v) = self.stroke_width {
            e.style.stroke_width = v;
        }
        if let Some(v) = self.stroke_style {
            e.style.stroke_style = v;
        }
        // An image has no outline until it is given one: choosing a width or a dash for it means "outline it", so
        // a clear stroke colour becomes the default one. (Text and shapes always have a colour to apply them to.)
        if e.tag() == Kind::Image
            && self.stroke_color.is_none()
            && (self.stroke_width.is_some() || self.stroke_style.is_some())
            && e.style.stroke_color.a == 0
        {
            e.style.stroke_color = Style::default().stroke_color;
        }
        if let Some(v) = self.sloppiness {
            e.style.sloppiness = v;
        }
        if let Some(v) = self.corner_radius {
            e.style.corner_radius = v;
        }
        if let Some(v) = self.opacity {
            e.style.opacity = v;
        }
        match &mut e.kind {
            ElementKind::Freehand(d) => {
                if let Some(v) = self.freehand_style {
                    d.brush = v;
                }
                if let Some(v) = self.pressure_mode {
                    d.pressure_mode = v;
                }
            }
            ElementKind::Text(t) => {
                if let Some(v) = self.font_family {
                    t.font.family = v;
                }
                if let Some(v) = self.font_weight {
                    t.font.weight = v;
                }
                if let Some(v) = self.font_size {
                    t.font.size = v;
                }
                if let Some(v) = self.text_align {
                    t.font.align = v;
                }
                if let Some(v) = self.vertical_align {
                    t.font.vertical_align = v;
                }
                if let Some(v) = self.line_height {
                    t.font.line_height = v;
                }
            }
            ElementKind::Math(m) => {
                if let Some(v) = self.font_size {
                    m.font_size = v;
                }
            }
            _ => {}
        }
    }
}

/// The properties `e` supports, with their current values. A removed fill is `fill_color: Some(None)`.
pub fn props_of(e: &Element) -> StyleProps {
    let mut p = StyleProps::default();
    let has = |q: Prop| applies(e, q);
    if has(Prop::StrokeColor) {
        p.stroke_color = Some(ColorArg(e.style.stroke_color));
    }
    if has(Prop::FillColor) {
        p.fill_color = Some(e.style.fill_color.map(ColorArg));
    }
    if has(Prop::FillPattern) {
        p.fill_pattern = Some(e.style.fill_pattern);
    }
    if has(Prop::StrokeWidth) {
        p.stroke_width = Some(e.style.stroke_width);
    }
    if has(Prop::StrokeStyle) {
        p.stroke_style = Some(e.style.stroke_style);
    }
    if has(Prop::Sloppiness) {
        p.sloppiness = Some(e.style.sloppiness);
    }
    if has(Prop::CornerRadius) {
        p.corner_radius = Some(e.style.corner_radius);
    }
    p.opacity = Some(e.style.opacity);
    match &e.kind {
        ElementKind::Freehand(d) => {
            p.freehand_style = Some(d.brush);
            p.pressure_mode = Some(d.pressure_mode);
        }
        ElementKind::Text(t) => {
            p.font_family = Some(t.font.family);
            p.font_weight = Some(t.font.weight);
            p.font_size = Some(t.font.size);
            p.text_align = Some(t.font.align);
            p.line_height = Some(t.font.line_height);
            if has(Prop::VerticalAlign) {
                p.vertical_align = Some(t.font.vertical_align);
            }
        }
        ElementKind::Math(m) => p.font_size = Some(m.font_size),
        _ => {}
    }
    p
}

// ---------------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------------

/// The style of each element in `ids`, in order. Text properties of a shape or arrow that has a label are read from
/// the label when `include_bound_text` is set.
pub fn read<R: SceneRead>(
    r: &R,
    ids: &[ElementId],
    include_bound_text: bool,
) -> Result<Vec<(Kind, StyleProps)>, StyleError> {
    let labels = if include_bound_text { labels_of(r, ids) } else { Default::default() };
    ids.iter()
        .map(|id| {
            let e = live(r, id)?;
            let mut p = props_of(e);
            if let Some(l) = labels.get(id).and_then(|l| r.get(l)).filter(|l| !l.deleted) {
                let lp = props_of(l);
                p.font_family = lp.font_family;
                p.font_weight = lp.font_weight;
                p.font_size = lp.font_size;
                p.text_align = lp.text_align;
                p.vertical_align = lp.vertical_align;
                p.line_height = lp.line_height;
            }
            Ok((e.tag(), p))
        })
        .collect()
}

fn live<'a, R: SceneRead>(r: &'a R, id: &ElementId) -> Result<&'a Element, StyleError> {
    r.get(id).filter(|e| !e.deleted).ok_or_else(|| StyleError::Target(TargetError::NotFound(id.clone())))
}

// ---------------------------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------------------------

/// What [`apply`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleReport {
    /// The properties each element supports among those asked for. A label reached through its container is listed
    /// under the label's id.
    pub applied: BTreeMap<ElementId, BTreeSet<Prop>>,
    /// The properties an element does not support.
    pub skipped: BTreeMap<ElementId, BTreeSet<Prop>>,
    /// The elements whose stored style actually changed.
    pub changed: Vec<ElementId>,
}

/// Applies `props` to `targets` in the caller's transaction.
///
/// Checks first and writes after: a missing or locked element, an out-of-range number, or no target that supports
/// any property leaves the transaction untouched. An element that already has the requested values is not written, so
/// a repeat of the same call changes nothing.
pub fn apply<T: SceneTx>(
    tx: &mut T,
    targets: &[ElementId],
    props: &StyleProps,
    include_bound_text: bool,
) -> Result<StyleReport, StyleError> {
    props.validate()?;
    let wanted = props.set_props();
    let wants_text = include_bound_text && wanted.iter().any(|p| p.is_text());
    let labels = if wants_text { labels_of(tx, targets) } else { Default::default() };

    let mut report = StyleReport::default();
    for id in targets {
        let e = live(tx, id)?;
        if e.locked {
            return Err(StyleError::Target(TargetError::Locked(vec![id.clone()])));
        }
        for &p in &wanted {
            if applies(e, p) {
                report.applied.entry(id.clone()).or_default().insert(p);
                continue;
            }
            let routed = p.is_text() && e.tag().is_label_container() && wants_text;
            let label = labels.get(id).and_then(|l| tx.get(l)).filter(|l| !l.deleted && applies(l, p));
            match (routed, label) {
                (true, Some(l)) => {
                    if l.locked {
                        return Err(StyleError::Target(TargetError::Locked(vec![l.id.clone()])));
                    }
                    report.applied.entry(l.id.clone()).or_default().insert(p);
                }
                _ => {
                    report.skipped.entry(id.clone()).or_default().insert(p);
                }
            }
        }
    }
    if report.applied.is_empty() {
        return Err(StyleError::NotApplicable { props: wanted });
    }
    let parts: Vec<(&ElementId, StyleProps)> =
        report.applied.iter().map(|(id, only)| (id, props.restricted(only))).collect();
    // Every element is checked before the first write.
    for (id, part) in &parts {
        if let Some(old) = tx.get(id) {
            check_visible(old, part)?;
        }
    }
    for (id, part) in parts {
        let Some(old) = tx.get(id) else { continue };
        let mut new = old.clone();
        part.write(&mut new);
        if new != *old {
            tx.update(id, |e| part.write(e))?;
            report.changed.push(id.clone());
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------------------------
// Defaults for new elements
// ---------------------------------------------------------------------------------------------

/// The style new elements get when nothing says otherwise.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StyleDefaults {
    /// Shape style (the seed is ignored: each element gets its own).
    pub style: Style,
    /// Font of new text.
    pub text: TextStyle,
    /// Brush of new freehand strokes.
    pub brush: Brush,
    /// Pressure mode of new freehand strokes.
    pub pressure_mode: PressureMode,
}

impl StyleDefaults {
    /// Sets every property in `props`. Returns whether anything changed. The caller has validated `props`.
    pub fn apply(&mut self, props: &StyleProps) -> bool {
        let before = self.clone();
        if let Some(c) = props.stroke_color {
            self.style.stroke_color = c.0;
        }
        if let Some(c) = props.fill_color {
            self.style.fill_color = c.map(|c| c.0);
        }
        if let Some(v) = props.fill_pattern {
            self.style.fill_pattern = v;
        }
        if let Some(v) = props.stroke_width {
            self.style.stroke_width = v;
        }
        if let Some(v) = props.stroke_style {
            self.style.stroke_style = v;
        }
        if let Some(v) = props.sloppiness {
            self.style.sloppiness = v;
        }
        if let Some(v) = props.corner_radius {
            self.style.corner_radius = v;
        }
        if let Some(v) = props.opacity {
            self.style.opacity = v;
        }
        if let Some(v) = props.freehand_style {
            self.brush = v;
        }
        if let Some(v) = props.pressure_mode {
            self.pressure_mode = v;
        }
        if let Some(v) = props.font_family {
            self.text.family = v;
        }
        if let Some(v) = props.font_weight {
            self.text.weight = v;
        }
        if let Some(v) = props.font_size {
            self.text.size = v;
        }
        if let Some(v) = props.text_align {
            self.text.align = v;
        }
        if let Some(v) = props.vertical_align {
            self.text.vertical_align = v;
        }
        if let Some(v) = props.line_height {
            self.text.line_height = v;
        }
        *self != before
    }

    /// Every property with its current value.
    pub fn to_props(&self) -> StyleProps {
        StyleProps {
            stroke_color: Some(ColorArg(self.style.stroke_color)),
            fill_color: Some(self.style.fill_color.map(ColorArg)),
            fill_pattern: Some(self.style.fill_pattern),
            stroke_width: Some(self.style.stroke_width),
            stroke_style: Some(self.style.stroke_style),
            sloppiness: Some(self.style.sloppiness),
            corner_radius: Some(self.style.corner_radius),
            opacity: Some(self.style.opacity),
            freehand_style: Some(self.brush),
            pressure_mode: Some(self.pressure_mode),
            font_family: Some(self.text.family),
            font_weight: Some(self.text.weight),
            font_size: Some(self.text.size),
            text_align: Some(self.text.align),
            vertical_align: Some(self.text.vertical_align),
            line_height: Some(self.text.line_height),
        }
    }

    /// Defaults from a stored bundle (the `style.defaults` setting). Absent properties keep the initial value, and
    /// a bundle with a bad number is read as far as it is valid.
    pub fn from_props(props: &StyleProps) -> Self {
        let mut d = Self::default();
        let mut p = props.clone();
        if p.validate().is_err() {
            p = StyleProps {
                stroke_width: p
                    .stroke_width
                    .filter(|v| range(Prop::StrokeWidth, *v, 0.0, MAX_STROKE_WIDTH, true).is_ok()),
                corner_radius: p
                    .corner_radius
                    .filter(|v| range(Prop::CornerRadius, *v, 0.0, MAX_CORNER_RADIUS, false).is_ok()),
                opacity: p.opacity.filter(|v| range(Prop::Opacity, *v, 0.0, 1.0, false).is_ok()),
                font_size: p.font_size.filter(|v| range(Prop::FontSize, *v, 0.0, MAX_FONT_SIZE, true).is_ok()),
                line_height: p
                    .line_height
                    .filter(|v| range(Prop::LineHeight, *v, MIN_LINE_HEIGHT, MAX_LINE_HEIGHT, false).is_ok()),
                ..p
            };
        }
        // New elements always have a stroke (see [`StyleError::DefaultStroke`]).
        p.stroke_color = p.stroke_color.filter(|c| c.0.a > 0);
        d.apply(&p);
        d
    }
}

// ---------------------------------------------------------------------------------------------
// Themes
// ---------------------------------------------------------------------------------------------

/// A named style bundle and an optional page colour.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Theme {
    /// The name `style.apply_theme` takes.
    pub name: &'static str,
    /// The properties it sets.
    pub props: StyleProps,
    /// The page colour, applied only when asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<ColorArg>,
}

fn pal(name: &str) -> ColorArg {
    ColorArg(palette::lookup(name).unwrap_or(Color::BLACK))
}

fn hex(r: u8, g: u8, b: u8) -> ColorArg {
    ColorArg(Color::rgb(r, g, b))
}

/// The built-in themes, in listing order.
pub fn themes() -> Vec<Theme> {
    let d = StyleDefaults::default();
    let base = |stroke: ColorArg, fill: Option<ColorArg>, width: f64, sloppiness: Sloppiness| StyleProps {
        stroke_color: Some(stroke),
        fill_color: Some(fill),
        fill_pattern: Some(FillPattern::Solid),
        stroke_width: Some(width),
        sloppiness: Some(sloppiness),
        ..StyleProps::default()
    };
    let default_props = StyleProps {
        stroke_color: Some(ColorArg(d.style.stroke_color)),
        fill_color: Some(d.style.fill_color.map(ColorArg)),
        fill_pattern: Some(d.style.fill_pattern),
        stroke_width: Some(d.style.stroke_width),
        stroke_style: Some(d.style.stroke_style),
        sloppiness: Some(d.style.sloppiness),
        corner_radius: Some(d.style.corner_radius),
        opacity: Some(d.style.opacity),
        font_family: Some(d.text.family),
        font_weight: Some(d.text.weight),
        font_size: Some(d.text.size),
        text_align: Some(d.text.align),
        ..StyleProps::default()
    };
    vec![
        Theme { name: "default", props: default_props, background: Some(hex(0xff, 0xff, 0xff)) },
        Theme {
            name: "monochrome",
            props: base(hex(0, 0, 0), None, 2.0, Sloppiness::Architect),
            background: Some(hex(0xff, 0xff, 0xff)),
        },
        Theme {
            name: "pastel",
            props: base(pal("slate-700"), Some(pal("blue-100")), 2.0, Sloppiness::Artist),
            background: Some(hex(0xfa, 0xfa, 0xfa)),
        },
        Theme {
            name: "blueprint",
            props: StyleProps { fill_pattern: None, ..base(hex(0xff, 0xff, 0xff), None, 2.0, Sloppiness::Architect) },
            background: Some(pal("blue-900")),
        },
        Theme {
            name: "high_contrast",
            props: base(hex(0, 0, 0), Some(pal("yellow-200")), 4.0, Sloppiness::Architect),
            background: Some(hex(0xff, 0xff, 0xff)),
        },
    ]
}

/// The theme called `name`. Case, `-` and spaces do not matter (`High-Contrast` finds `high_contrast`).
pub fn theme(name: &str) -> Option<Theme> {
    let norm = |s: &str| s.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    let want = norm(name);
    themes().into_iter().find(|t| t.name == want)
}

/// The theme names, comma separated, for messages.
pub fn theme_names() -> String {
    themes().iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drafftink_geom::kurbo::Point;
    use drafftink_scene::{LinearKind, MemoryStore, SceneStore, TextData};

    fn id(s: &str) -> ElementId {
        ElementId::new(s).unwrap()
    }

    fn put(store: &mut MemoryStore, s: &str, kind: ElementKind) -> ElementId {
        let mut tx = store.begin();
        let z = tx.next_top_index().unwrap();
        tx.insert(Element::new(id(s), kind, z).with_bounds(0.0, 0.0, 50.0, 30.0)).unwrap();
        tx.commit().unwrap();
        id(s)
    }

    fn put_line(store: &mut MemoryStore, s: &str, closed: bool) -> ElementId {
        let mut tx = store.begin();
        let z = tx.next_top_index().unwrap();
        let mut e =
            Element::linear(id(s), LinearKind::Line, z, &[Point::ZERO, Point::new(5.0, 5.0), Point::new(0.0, 5.0)])
                .unwrap();
        if let ElementKind::Line(d) = &mut e.kind {
            d.closed = closed;
        }
        tx.insert(e).unwrap();
        tx.commit().unwrap();
        id(s)
    }

    fn props(v: Value) -> StyleProps {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn applicability_follows_the_table() {
        let mut st = MemoryStore::new();
        let rect = put(&mut st, "r", ElementKind::Rectangle);
        let ell = put(&mut st, "e", ElementKind::Ellipse);
        let txt = put(&mut st, "t", ElementKind::Text(TextData::new("hi")));
        let line = put_line(&mut st, "l", false);
        let cl = put_line(&mut st, "c", true);
        let free = {
            let mut tx = st.begin();
            let z = tx.next_top_index().unwrap();
            let kind = ElementKind::Freehand(drafftink_scene::FreehandData::new(vec![Point::ZERO]));
            tx.insert(Element::new(id("f"), kind, z)).unwrap();
            tx.commit().unwrap();
            id("f")
        };
        let img = put(
            &mut st,
            "i",
            ElementKind::Image(drafftink_scene::ImageData {
                file_id: None,
                natural_width: 1,
                natural_height: 1,
                crop: None,
            }),
        );
        let math = put(&mut st, "m", ElementKind::Math(drafftink_scene::MathData::new("x")));
        let g = |i: &ElementId, p: Prop| applies(st.get(i).unwrap(), p);
        assert!(g(&rect, Prop::CornerRadius) && !g(&ell, Prop::CornerRadius));
        assert!(
            g(&rect, Prop::FillColor)
                && g(&cl, Prop::FillColor)
                && !g(&line, Prop::FillColor)
                && g(&free, Prop::FillColor)
        );
        assert!(!g(&txt, Prop::StrokeWidth) && !g(&math, Prop::StrokeWidth) && g(&free, Prop::StrokeWidth));
        assert!(!g(&txt, Prop::Sloppiness) && !g(&free, Prop::Sloppiness) && g(&line, Prop::Sloppiness));
        assert!(g(&line, Prop::StrokeStyle) && g(&rect, Prop::StrokeStyle) && !g(&txt, Prop::StrokeStyle));
        assert!(g(&img, Prop::StrokeColor) && g(&img, Prop::StrokeWidth) && g(&img, Prop::StrokeStyle));
        assert!(g(&img, Prop::Opacity) && !g(&img, Prop::Sloppiness) && !g(&img, Prop::FillColor));
        assert!(g(&txt, Prop::FontFamily) && !g(&math, Prop::FontFamily) && g(&math, Prop::FontSize));
        assert!(!g(&txt, Prop::VerticalAlign), "free text has no container to align in");
        assert!(g(&free, Prop::FreehandStyle) && !g(&rect, Prop::PressureMode));
    }

    #[test]
    fn what_shows_follows_the_kind() {
        let style =
            |stroke: Color, fill: Option<Color>| Style { stroke_color: stroke, fill_color: fill, ..Style::default() };
        let blue = Some(Color::rgb(0, 0, 255));
        for kind in Kind::ALL {
            assert!(style_shows(kind, false, &style(Color::BLACK, None)), "{kind:?} with a stroke");
        }
        for kind in [Kind::Rectangle, Kind::Diamond, Kind::Ellipse] {
            assert!(style_shows(kind, false, &style(Color::TRANSPARENT, blue)), "{kind:?}: a fill is enough");
            assert!(!style_shows(kind, false, &style(Color::TRANSPARENT, None)));
            assert!(
                !style_shows(kind, false, &style(Color::TRANSPARENT, Some(Color::TRANSPARENT))),
                "a clear fill is none"
            );
        }
        assert!(style_shows(Kind::Line, true, &style(Color::TRANSPARENT, blue)), "closed lines are shapes");
        for kind in [Kind::Line, Kind::Arrow, Kind::Freehand, Kind::Text, Kind::Math, Kind::Frame] {
            assert!(!style_shows(kind, false, &style(Color::TRANSPARENT, blue)), "{kind:?} is drawn in its stroke");
        }
        assert!(style_shows(Kind::Image, false, &style(Color::TRANSPARENT, None)));
    }

    #[test]
    fn validation_names_the_prop_and_the_range() {
        assert_eq!(StyleProps::default().validate(), Err(StyleError::Empty));
        for (v, prop) in [
            (json!({"stroke_width": 0}), Prop::StrokeWidth),
            (json!({"stroke_width": 1e9}), Prop::StrokeWidth),
            (json!({"corner_radius": -1}), Prop::CornerRadius),
            (json!({"opacity": 1.5}), Prop::Opacity),
            (json!({"font_size": 0}), Prop::FontSize),
            (json!({"line_height": 0.5}), Prop::LineHeight),
            (json!({"line_height": 3.5}), Prop::LineHeight),
        ] {
            match props(v.clone()).validate() {
                Err(StyleError::OutOfRange { prop: p, .. }) => assert_eq!(p, prop, "{v}"),
                other => panic!("{v}: {other:?}"),
            }
        }
        assert!(
            props(json!({"opacity": 0, "corner_radius": 0, "line_height": 1, "stroke_width": 0.5})).validate().is_ok()
        );
    }

    #[test]
    fn colours_parse_names_and_hex_and_null_is_no_fill() {
        let p = props(json!({"stroke_color": "blue-500", "fill_color": null}));
        assert_eq!(p.stroke_color, Some(ColorArg(Color::rgb(59, 130, 246))));
        assert_eq!(p.fill_color, Some(None));
        assert_eq!(props(json!({})).fill_color, None);
        assert_eq!(props(json!({"fill_color": "#fff"})).fill_color, Some(Some(ColorArg(Color::WHITE))));
        let e = serde_json::from_value::<StyleProps>(json!({"stroke_color": "mauve-500"})).unwrap_err().to_string();
        assert!(e.contains("not a colour") && e.contains("blue-500"), "{e}");
        assert_eq!(p.value(Prop::FillColor), Some(Value::Null));
        // The bundle survives a round trip, with the removed fill kept.
        assert_eq!(serde_json::from_value::<StyleProps>(serde_json::to_value(&p).unwrap()).unwrap(), p);
    }

    #[test]
    fn apply_writes_supported_props_and_reports_the_rest() {
        let mut st = MemoryStore::new();
        let r = put(&mut st, "r", ElementKind::Rectangle);
        let t = put(&mut st, "t", ElementKind::Text(TextData::new("hi")));
        let mut tx = st.begin();
        let rep =
            apply(&mut tx, &[r.clone(), t.clone()], &props(json!({"corner_radius": 8, "opacity": 0.5})), true).unwrap();
        tx.commit().unwrap();
        assert_eq!(rep.applied[&r], BTreeSet::from([Prop::CornerRadius, Prop::Opacity]));
        assert_eq!(rep.applied[&t], BTreeSet::from([Prop::Opacity]));
        assert_eq!(rep.skipped[&t], BTreeSet::from([Prop::CornerRadius]));
        assert_eq!(st.get(&r).unwrap().style.corner_radius, 8.0);
        assert_eq!(st.get(&t).unwrap().style.opacity, 0.5);
        assert_eq!(st.get(&t).unwrap().style.corner_radius, 0.0);
    }

    #[test]
    fn apply_checks_before_it_writes() {
        let mut st = MemoryStore::new();
        let a = put(&mut st, "a", ElementKind::Rectangle);
        let b = put(&mut st, "b", ElementKind::Rectangle);
        {
            let mut tx = st.begin();
            tx.update(&b, |e| e.locked = true).unwrap();
            tx.commit().unwrap();
        }
        let mut tx = st.begin();
        let err = apply(&mut tx, &[a.clone(), b.clone()], &props(json!({"opacity": 0.2})), true).unwrap_err();
        assert_eq!(err, StyleError::Target(TargetError::Locked(vec![b])));
        assert_eq!(tx.get(&a).unwrap().style.opacity, 1.0);
        let err = apply(&mut tx, &[id("nope")], &props(json!({"opacity": 0.2})), true).unwrap_err();
        assert_eq!(err, StyleError::Target(TargetError::NotFound(id("nope"))));
        let err = apply(&mut tx, &[a], &props(json!({"font_size": 30})), true).unwrap_err();
        assert_eq!(err, StyleError::NotApplicable { props: vec![Prop::FontSize] });
    }

    /// An image with the style a new one gets: no outline.
    fn put_image(store: &mut MemoryStore, s: &str) -> ElementId {
        let kind = ElementKind::Image(drafftink_scene::ImageData {
            file_id: None,
            natural_width: 1,
            natural_height: 1,
            crop: None,
        });
        let image = put(store, s, kind);
        let mut tx = store.begin();
        tx.update(&image, |e| e.style = Style::for_image(7)).unwrap();
        tx.commit().unwrap();
        image
    }

    #[test]
    fn an_image_can_be_given_an_outline_and_lose_it_again() {
        let mut st = MemoryStore::new();
        let img = put_image(&mut st, "i");
        assert_eq!(st.get(&img).unwrap().style.stroke_color, Color::TRANSPARENT, "a new image has no outline");
        let mut tx = st.begin();
        let red = props(json!({"stroke_color": "red-500", "stroke_width": 4, "stroke_style": "dashed"}));
        let rep = apply(&mut tx, std::slice::from_ref(&img), &red, true).unwrap();
        assert!(rep.skipped.is_empty(), "{rep:?}");
        let style = &tx.get(&img).unwrap().style;
        assert_eq!(
            (style.stroke_color, style.stroke_width, style.stroke_style),
            (Color::rgb(239, 68, 68), 4.0, StrokeStyle::Dashed)
        );
        // "No stroke" is allowed: a picture shows without an outline.
        assert!(none_refusal(&tx, std::slice::from_ref(&img), Prop::StrokeColor).is_none());
        apply(&mut tx, std::slice::from_ref(&img), &props(json!({"stroke_color": "transparent"})), true).unwrap();
        assert_eq!(tx.get(&img).unwrap().style.stroke_color, Color::TRANSPARENT);
        assert!(is_visible(tx.get(&img).unwrap()));
    }

    #[test]
    fn a_width_or_dash_alone_outlines_an_image_that_has_none() {
        for p in [json!({"stroke_width": 4}), json!({"stroke_style": "dotted"})] {
            let mut st = MemoryStore::new();
            let img = put_image(&mut st, "i");
            let mut tx = st.begin();
            apply(&mut tx, std::slice::from_ref(&img), &props(p.clone()), true).unwrap();
            assert_eq!(tx.get(&img).unwrap().style.stroke_color, Style::default().stroke_color, "{p}");
        }
        // An image that has an outline keeps its colour when only the width changes.
        let mut st = MemoryStore::new();
        let img = put_image(&mut st, "i");
        let mut tx = st.begin();
        apply(&mut tx, std::slice::from_ref(&img), &props(json!({"stroke_color": "red-500"})), true).unwrap();
        apply(&mut tx, std::slice::from_ref(&img), &props(json!({"stroke_width": 8})), true).unwrap();
        assert_eq!(tx.get(&img).unwrap().style.stroke_color, Color::rgb(239, 68, 68));
    }

    #[test]
    fn a_repeat_changes_nothing() {
        let mut st = MemoryStore::new();
        let a = put(&mut st, "a", ElementKind::Rectangle);
        let p = props(json!({"stroke_width": 4}));
        let mut tx = st.begin();
        assert_eq!(apply(&mut tx, std::slice::from_ref(&a), &p, true).unwrap().changed, vec![a.clone()]);
        assert!(apply(&mut tx, &[a], &p, true).unwrap().changed.is_empty());
    }

    #[test]
    fn text_props_reach_the_label_of_a_container() {
        let mut st = MemoryStore::new();
        let r = put(&mut st, "r", ElementKind::Rectangle);
        let mut td = TextData::new("label");
        td.container_id = Some(r.clone());
        let l = put(&mut st, "l", ElementKind::Text(td));
        let bare = put(&mut st, "b", ElementKind::Rectangle);
        let p = props(json!({"font_size": 36, "vertical_align": "bottom", "stroke_color": "red-500"}));
        let mut tx = st.begin();
        let rep = apply(&mut tx, &[r.clone(), bare.clone()], &p, true).unwrap();
        let ElementKind::Text(t) = &tx.get(&l).unwrap().kind else { panic!() };
        assert_eq!((t.font.size, t.font.vertical_align), (36.0, VerticalAlign::Bottom));
        assert_eq!(rep.applied[&l], BTreeSet::from([Prop::FontSize, Prop::VerticalAlign]));
        assert_eq!(rep.applied[&r], BTreeSet::from([Prop::StrokeColor]));
        assert!(!rep.skipped.contains_key(&r));
        assert_eq!(rep.skipped[&bare], BTreeSet::from([Prop::FontSize, Prop::VerticalAlign]));
        // The label's own colour is a stroke property of the label, so it is not touched.
        assert_ne!(tx.get(&l).unwrap().style.stroke_color, Color::rgb(239, 68, 68));
        // Off: the container skips them.
        let rep =
            apply(&mut tx, std::slice::from_ref(&r), &props(json!({"font_size": 12, "opacity": 0.5})), false).unwrap();
        assert_eq!(rep.skipped[&r], BTreeSet::from([Prop::FontSize]));
        let read = read(&tx, &[r], true).unwrap();
        assert_eq!(read[0].1.font_size, Some(36.0));
    }

    #[test]
    fn defaults_apply_and_round_trip() {
        let mut d = StyleDefaults::default();
        assert!(!d.apply(&props(json!({"stroke_width": 2}))));
        assert!(d.apply(&props(json!({"fill_color": "yellow-200", "font_size": 28, "freehand_style": "calligraphy"}))));
        assert_eq!(d.text.size, 28.0);
        assert_eq!(d.brush, Brush::Calligraphy);
        let stored = serde_json::to_value(d.to_props()).unwrap();
        assert_eq!(StyleDefaults::from_props(&serde_json::from_value(stored).unwrap()), d);
        let bad = props(json!({"stroke_width": -3, "opacity": 0.5}));
        let r = StyleDefaults::from_props(&bad);
        assert_eq!((r.style.stroke_width, r.style.opacity), (2.0, 0.5));
    }

    #[test]
    fn built_in_themes_are_the_catalogued_bundles() {
        let names: Vec<_> = themes().iter().map(|t| t.name).collect();
        assert_eq!(names, ["default", "monochrome", "pastel", "blueprint", "high_contrast"]);
        let t = theme("High-Contrast").unwrap();
        assert_eq!(t.props.stroke_width, Some(4.0));
        assert_eq!(t.props.fill_color, Some(Some(pal("yellow-200"))));
        assert!(theme("neon").is_none());
        let bp = theme("blueprint").unwrap();
        assert_eq!(bp.props.fill_pattern, None);
        assert_eq!(bp.background, Some(pal("blue-900")));
        let d = theme("default").unwrap();
        assert_eq!(d.props.stroke_color, Some(pal("indigo-500")));
        assert_eq!(d.props.font_size, Some(20.0));
    }
}
