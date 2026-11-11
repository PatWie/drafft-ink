//! The options of a stage, as the JS developer writes them (`Stage.create(canvas, { ... })`).
//!
//! Option bags use the JS spelling (`camelCase`). Shapes use the document's own layout (`snake_case`), see
//! [`crate::json`].

use drafftink_render::{GridSpec, GridStyle, Theme};
use serde::Deserialize;
use serde_json::Value;

use crate::json::Obj;

/// Light or dark rendering of the stored colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeOption {
    /// Colours as stored.
    #[default]
    Light,
    /// Colours mapped for a dark canvas.
    Dark,
}

impl From<ThemeOption> for Theme {
    fn from(t: ThemeOption) -> Self {
        match t {
            ThemeOption::Light => Theme::Light,
            ThemeOption::Dark => Theme::Dark,
        }
    }
}

/// The look of the background grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GridKind {
    /// No grid.
    #[default]
    None,
    /// Horizontal and vertical lines.
    Lines,
    /// Horizontal lines only.
    Rules,
    /// A small plus at each crossing.
    Crosses,
    /// A dot at each crossing.
    Dots,
}

/// The background grid.
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct GridOptions {
    /// The look. `none` draws no grid.
    pub kind: GridKind,
    /// Base spacing in world units (a power-of-two multiple is drawn when zoomed out).
    pub spacing: Option<f64>,
}

impl From<GridOptions> for GridSpec {
    fn from(g: GridOptions) -> Self {
        let style = match g.kind {
            GridKind::None => GridStyle::None,
            GridKind::Lines => GridStyle::Lines,
            GridKind::Rules => GridStyle::HorizontalLines,
            GridKind::Crosses => GridStyle::Crosses,
            GridKind::Dots => GridStyle::Dots,
        };
        let mut spec = GridSpec::new(style);
        if let Some(spacing) = g.spacing.filter(|s| s.is_finite() && *s > 0.0) {
            spec.spacing = spacing;
        }
        spec
    }
}

/// How a drag on empty space selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarqueeOption {
    /// No rubber band.
    Off,
    /// Elements that lie completely inside the rectangle.
    #[default]
    Contain,
    /// Every element the rectangle touches.
    Intersect,
}

/// The built-in behaviour of pointer input. Turn a part off to drive the stage with your own listeners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct InteractionOptions {
    /// A press selects the element (or its group); a press on empty space clears the selection.
    pub select: bool,
    /// A drag moves the element, or the whole selection when the element is part of it. A shape with
    /// `custom: { draggable: false }` stays where it is.
    pub drag: bool,
    /// A drag on a handle of the transformer resizes, rotates or reshapes the selection.
    pub transform: bool,
    /// A drag on empty space draws a rubber band and selects.
    pub marquee: MarqueeOption,
}

impl Default for InteractionOptions {
    fn default() -> Self {
        Self { select: true, drag: true, transform: true, marquee: MarqueeOption::Contain }
    }
}

/// Everything `Stage.create` accepts besides the canvas element.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct StageOptions {
    /// Logical width in CSS pixels. Defaults to the element's width.
    pub width: Option<f64>,
    /// Logical height in CSS pixels.
    pub height: Option<f64>,
    /// Device pixels per CSS pixel. Defaults to `window.devicePixelRatio`.
    pub device_pixel_ratio: Option<f64>,
    /// Canvas colour as `#rrggbb` or `#rrggbbaa`.
    pub background: Option<String>,
    /// Light or dark rendering.
    pub theme: ThemeOption,
    /// The background grid.
    pub grid: GridOptions,
    /// Built-in pointer behaviour.
    pub interaction: InteractionOptions,
    /// Style properties every added shape starts from, for example `{ sloppiness: "architect" }` for clean lines.
    pub default_style: Obj,
    /// A partial `CanvasConfig` merged over the defaults: hit tolerances, gesture thresholds, handle sizes.
    pub config: Option<Value>,
    /// A document in the native format (as `toJSON` returns it), loaded at start.
    pub document: Option<Value>,
}
