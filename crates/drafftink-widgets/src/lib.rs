//! Reusable egui widget components with Tailwind-inspired styling.
//!
//! This crate provides a collection of styled UI components for egui applications:
//!
//! - **Buttons**: Icon buttons, toggle buttons, multi-state toggles
//! - **Colors**: Tailwind color palette, color swatches, color picker grids
//! - **Menu**: Menu items, separators, panel frames
//! - **Layout**: Section labels, separators, spacing helpers

pub mod buttons;
pub mod colors;
pub mod common;
pub mod layout;
pub mod menu;

pub use buttons::{
    FontSizeButton, IconButton, IconButtonStyle, MultiToggle, MultiToggleState, StrokeWidthButton,
    TextButton, ToggleButton,
};
pub use colors::{
    ColorGrid, ColorGridPosition, ColorSwatch, ColorSwatchStyle, ColorSwatchWithWheel,
    NoColorSwatch, SHADE_LABELS, SelectionStyle, TAILWIND_COLORS, TailwindColor, TailwindPalette,
    colors_match, hue_to_rgb, parse_css_color,
};
pub use common::{default_btn, input_text, primary_btn, secondary_btn};
pub use layout::{section_label, separator, vertical_separator};
pub use menu::{menu_item, menu_item_enabled, menu_separator, panel_frame, toolbar_frame};

/// Standard sizing constants used across widgets.
pub mod sizing {
    /// Small button size (icons, color swatches)
    pub const SMALL: f32 = 20.0;
    /// Medium button size (toolbar buttons)
    pub const MEDIUM: f32 = 28.0;
    /// Large button size
    pub const LARGE: f32 = 36.0;
    /// Standard corner radius
    pub const CORNER_RADIUS: u8 = 4;
    /// Panel corner radius
    pub const PANEL_RADIUS: u8 = 8;
}

/// Standard colors used across widgets.
pub mod theme {
    use egui::Color32;

    /// Text color (dark gray)
    pub const TEXT: Color32 = Color32::from_rgb(60, 60, 60);
    /// Muted text color
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(120, 120, 120);
    /// Border color
    pub const BORDER: Color32 = Color32::from_rgb(220, 220, 220);
    /// Selection/active color (blue)
    pub const ACCENT: Color32 = Color32::from_rgb(59, 130, 246);
    /// Hover background
    pub const HOVER_BG: Color32 = Color32::from_rgb(245, 245, 245);
    /// Selected background
    pub const SELECTED_BG: Color32 = Color32::from_rgb(235, 245, 255);
    /// Panel background
    pub const PANEL_BG: Color32 = Color32::from_rgba_premultiplied(250, 250, 252, 250);
}
