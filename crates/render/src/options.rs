//! Options that change how every element looks or how much work a frame does.

use crate::theme::Theme;

/// Level of detail: below these on-screen sizes an element is replaced by something cheaper.
///
/// Sizes are in device pixels along the longer side of the element's paint bounds (text uses the font size).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodConfig {
    /// Smaller than this: not drawn at all.
    pub skip_below_px: f64,
    /// Smaller than this: a flat box instead of the element.
    pub box_below_px: f64,
    /// A text whose font size is below this is drawn as grey bars, one per line.
    pub greek_text_below_px: f64,
    /// Most boxes and bars one frame draws. The rest are skipped, so a fully zoomed-out 100k scene stays cheap.
    pub max_cheap_per_frame: usize,
}

impl Default for LodConfig {
    fn default() -> Self {
        Self { skip_below_px: 0.75, box_below_px: 3.0, greek_text_below_px: 3.5, max_cheap_per_frame: 20_000 }
    }
}

impl LodConfig {
    /// No level of detail: every element is drawn in full. For export.
    pub const OFF: LodConfig =
        LodConfig { skip_below_px: 0.0, box_below_px: 0.0, greek_text_below_px: 0.0, max_cheap_per_frame: usize::MAX };
}

/// How to draw. Part of every fragment's cache key where it matters (the theme), so changing it re-encodes.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RenderOptions {
    /// Light or dark palette.
    pub theme: Theme,
    /// Level of detail.
    pub lod: LodConfig,
}

impl RenderOptions {
    /// Options for export: the given theme and no level of detail.
    pub fn for_export(theme: Theme) -> Self {
        Self { theme, lod: LodConfig::OFF }
    }
}
