//! Button components: icon buttons, toggle buttons, multi-state toggles.

use egui::{
    Align2, Color32, CornerRadius, CursorIcon, Image, ImageSource, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, Vec2, vec2,
};

use crate::{sizing, theme};

/// Style configuration for icon buttons.
#[derive(Clone)]
pub struct IconButtonStyle {
    /// Button size
    pub size: Vec2,
    /// Icon size (should be smaller than button size)
    pub icon_size: Vec2,
    /// Corner radius
    pub corner_radius: u8,
    /// Background color when not selected
    pub bg_color: Color32,
    /// Background color when hovered
    pub hover_color: Color32,
    /// Background color when selected/active
    pub selected_color: Color32,
    /// Icon tint when not selected (None = no tint)
    pub icon_tint: Option<Color32>,
    /// Icon tint when selected
    pub selected_icon_tint: Option<Color32>,
    /// Whether to use solid fill style (like toolbar tools)
    pub solid_selected: bool,
}

impl Default for IconButtonStyle {
    fn default() -> Self {
        Self {
            size: vec2(sizing::MEDIUM, sizing::MEDIUM),
            icon_size: vec2(18.0, 18.0),
            corner_radius: sizing::CORNER_RADIUS,
            bg_color: Color32::TRANSPARENT,
            hover_color: theme::HOVER_BG,
            selected_color: theme::ACCENT,
            icon_tint: Some(Color32::from_gray(80)),
            selected_icon_tint: Some(Color32::WHITE),
            solid_selected: true,
        }
    }
}

impl IconButtonStyle {
    /// Create a small icon button style (24x24 button, 16x16 icon) for bottom toolbar
    pub fn small() -> Self {
        Self {
            size: vec2(24.0, 24.0),
            icon_size: vec2(16.0, 16.0),
            corner_radius: sizing::CORNER_RADIUS,
            bg_color: Color32::TRANSPARENT,
            hover_color: Color32::TRANSPARENT,
            selected_color: Color32::TRANSPARENT,
            icon_tint: Some(Color32::from_gray(100)),
            selected_icon_tint: Some(theme::ACCENT),
            solid_selected: false,
        }
    }

    /// Create a toolbar tool button style (32x32, solid blue when selected)
    pub fn tool() -> Self {
        Self {
            size: vec2(32.0, 32.0),
            icon_size: vec2(18.0, 18.0),
            corner_radius: 6,
            bg_color: Color32::TRANSPARENT,
            hover_color: Color32::from_gray(235),
            selected_color: theme::ACCENT,
            icon_tint: Some(Color32::from_gray(80)),
            selected_icon_tint: Some(Color32::WHITE),
            solid_selected: true,
        }
    }

    /// Create a large button style (36x36)
    pub fn large() -> Self {
        Self {
            size: vec2(sizing::LARGE, sizing::LARGE),
            icon_size: vec2(24.0, 24.0),
            ..Default::default()
        }
    }
}

/// An icon button that displays an image/SVG.
pub struct IconButton<'a> {
    icon: ImageSource<'a>,
    tooltip: &'a str,
    shortcut: Option<&'a str>,
    selected: bool,
    style: IconButtonStyle,
}

impl<'a> IconButton<'a> {
    /// Create a new icon button.
    pub fn new(icon: ImageSource<'a>, tooltip: &'a str) -> Self {
        Self {
            icon,
            tooltip,
            shortcut: None,
            selected: false,
            style: IconButtonStyle::default(),
        }
    }

    /// Set whether the button is selected/active.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Set the button style.
    pub fn style(mut self, style: IconButtonStyle) -> Self {
        self.style = style;
        self
    }

    /// Use small style for bottom toolbar.
    pub fn small(mut self) -> Self {
        self.style = IconButtonStyle::small();
        self
    }

    /// Use tool style for left toolbar.
    pub fn tool(mut self) -> Self {
        self.style = IconButtonStyle::tool();
        self
    }

    /// Set keyboard shortcut (shown in hover tooltip).
    pub fn shortcut(mut self, shortcut: &'a str) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Show the button and return true if clicked.
    pub fn show(self, ui: &mut Ui) -> bool {
        let (rect, response) = ui.allocate_exact_size(self.style.size, Sense::click());

        if ui.is_rect_visible(rect) {
            let bg_color = if self.selected && self.style.solid_selected {
                self.style.selected_color
            } else if response.hovered() {
                self.style.hover_color
            } else {
                self.style.bg_color
            };

            // Draw background
            ui.painter()
                .rect_filled(rect, CornerRadius::same(self.style.corner_radius), bg_color);

            // Determine icon tint
            let icon_tint = if self.selected {
                self.style.selected_icon_tint
            } else if response.hovered() {
                Some(Color32::from_gray(40))
            } else {
                self.style.icon_tint
            };

            // Draw icon centered
            let icon_rect = Rect::from_center_size(rect.center(), self.style.icon_size);
            let mut image = Image::new(self.icon).fit_to_exact_size(self.style.icon_size);
            if let Some(tint) = icon_tint {
                image = image.tint(tint);
            }
            image.paint_at(ui, icon_rect);
        }

        let clicked = response.clicked();
        // Show tooltip with optional shortcut
        if let Some(shortcut) = self.shortcut {
            response.clone().on_hover_ui(|ui| {
                ui.horizontal(|ui| {
                    ui.label(self.tooltip);
                    ui.label(
                        egui::RichText::new(format!("({})", shortcut))
                            .color(Color32::from_gray(128))
                            .small(),
                    );
                });
            });
        } else {
            response.clone().on_hover_text(self.tooltip);
        }
        response.on_hover_cursor(CursorIcon::PointingHand);
        clicked
    }
}

/// A toggle button with text label.
/// Uses solid blue background when selected (Excalidraw style).
pub struct ToggleButton<'a> {
    label: &'a str,
    selected: bool,
    min_width: Option<f32>,
    height: f32,
    font_size: f32,
}

impl<'a> ToggleButton<'a> {
    /// Create a new toggle button.
    pub fn new(label: &'a str, selected: bool) -> Self {
        Self {
            label,
            selected,
            min_width: None,
            height: 24.0,
            font_size: 11.0,
        }
    }

    /// Set minimum width.
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = Some(width);
        self
    }

    /// Set the button height.
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    /// Set the font size.
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    /// Show the button and return true if clicked.
    pub fn show(self, ui: &mut Ui) -> bool {
        // Calculate text size for proper button width
        let font_id = egui::FontId::proportional(self.font_size);
        let galley = ui.painter().layout_no_wrap(
            self.label.to_string(),
            font_id.clone(),
            Color32::PLACEHOLDER, // Color doesn't matter for sizing
        );
        let text_width = galley.size().x;
        let width = self
            .min_width
            .unwrap_or(text_width + 16.0)
            .max(text_width + 16.0);
        let size = vec2(width, self.height);

        let (rect, response) = ui.allocate_exact_size(size, Sense::click());

        if ui.is_rect_visible(rect) {
            let bg_color = if self.selected {
                theme::ACCENT
            } else if response.hovered() {
                Color32::from_gray(235)
            } else {
                Color32::from_gray(245)
            };

            let text_color = if self.selected {
                Color32::WHITE
            } else {
                Color32::from_gray(80)
            };

            ui.painter()
                .rect_filled(rect, CornerRadius::same(sizing::CORNER_RADIUS), bg_color);

            // Draw text centered
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                self.label,
                font_id,
                text_color,
            );
        }

        let clicked = response.clicked();
        response.on_hover_cursor(CursorIcon::PointingHand);
        clicked
    }
}

/// State for a multi-toggle option.
pub struct MultiToggleState<'a, T: Clone + PartialEq> {
    /// The value this state represents
    pub value: T,
    /// Icon to display (SVG or image)
    pub icon: ImageSource<'a>,
    /// Tooltip text
    pub tooltip: &'a str,
}

impl<'a, T: Clone + PartialEq> MultiToggleState<'a, T> {
    /// Create a new toggle state.
    pub fn new(value: T, icon: ImageSource<'a>, tooltip: &'a str) -> Self {
        Self {
            value,
            icon,
            tooltip,
        }
    }
}

/// A multi-state toggle button that cycles through states.
/// Each state has an icon and tooltip.
pub struct MultiToggle<'a, T: Clone + PartialEq> {
    states: &'a [MultiToggleState<'a, T>],
    current: &'a T,
    style: IconButtonStyle,
}

impl<'a, T: Clone + PartialEq> MultiToggle<'a, T> {
    /// Create a new multi-toggle with the given states.
    pub fn new(states: &'a [MultiToggleState<'a, T>], current: &'a T) -> Self {
        Self {
            states,
            current,
            style: IconButtonStyle::default(),
        }
    }

    /// Set the button style.
    pub fn style(mut self, style: IconButtonStyle) -> Self {
        self.style = style;
        self
    }

    /// Use small style.
    pub fn small(mut self) -> Self {
        self.style = IconButtonStyle::small();
        self
    }

    /// Show the toggle and return the new value if clicked.
    pub fn show(self, ui: &mut Ui) -> Option<T> {
        // Find current state index
        let current_idx = self
            .states
            .iter()
            .position(|s| &s.value == self.current)
            .unwrap_or(0);

        let state = &self.states[current_idx];
        let (rect, response) = ui.allocate_exact_size(self.style.size, Sense::click());

        if ui.is_rect_visible(rect) {
            let bg_color = if response.hovered() {
                self.style.hover_color
            } else {
                self.style.bg_color
            };

            ui.painter()
                .rect_filled(rect, CornerRadius::same(self.style.corner_radius), bg_color);

            // Draw icon centered
            let icon_rect = Rect::from_center_size(rect.center(), self.style.icon_size);
            Image::new(state.icon.clone()).paint_at(ui, icon_rect);
        }

        let clicked = response.clicked();
        response
            .on_hover_text(state.tooltip)
            .on_hover_cursor(CursorIcon::PointingHand);

        if clicked {
            // Cycle to next state
            let next_idx = (current_idx + 1) % self.states.len();
            Some(self.states[next_idx].value.clone())
        } else {
            None
        }
    }
}

/// A text button with optional shortcut hint.
pub struct TextButton<'a> {
    label: &'a str,
    shortcut: Option<&'a str>,
}

impl<'a> TextButton<'a> {
    /// Create a new text button.
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            shortcut: None,
        }
    }

    /// Add a shortcut hint.
    pub fn shortcut(mut self, shortcut: &'a str) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Show the button and return true if clicked.
    pub fn show(self, ui: &mut Ui) -> bool {
        let size = vec2(0.0, 24.0);
        let (rect, response) = ui.allocate_at_least(size, Sense::click());

        if ui.is_rect_visible(rect) {
            let bg_color = if response.hovered() {
                theme::HOVER_BG
            } else {
                Color32::TRANSPARENT
            };

            ui.painter()
                .rect_filled(rect, CornerRadius::same(sizing::CORNER_RADIUS), bg_color);

            // Draw label
            ui.painter().text(
                Pos2::new(rect.left() + 8.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                self.label,
                egui::FontId::proportional(12.0),
                theme::TEXT,
            );

            // Draw shortcut if present
            if let Some(shortcut) = self.shortcut {
                ui.painter().text(
                    Pos2::new(rect.right() - 8.0, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    shortcut,
                    egui::FontId::proportional(11.0),
                    theme::TEXT_MUTED,
                );
            }
        }

        let clicked = response.clicked();
        response.on_hover_cursor(CursorIcon::PointingHand);
        clicked
    }
}

/// A stroke width button showing a horizontal line.
pub struct StrokeWidthButton<'a> {
    width: f32,
    tooltip: &'a str,
    selected: bool,
}

impl<'a> StrokeWidthButton<'a> {
    /// Create a new stroke width button.
    pub fn new(width: f32, tooltip: &'a str, selected: bool) -> Self {
        Self {
            width,
            tooltip,
            selected,
        }
    }

    /// Show the button and return true if clicked.
    pub fn show(self, ui: &mut Ui) -> bool {
        let size = vec2(28.0, 20.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());

        if ui.is_rect_visible(rect) {
            let bg_color = if self.selected {
                theme::ACCENT
            } else if response.hovered() {
                Color32::from_gray(235)
            } else {
                Color32::from_gray(250)
            };

            let line_color = if self.selected {
                Color32::WHITE
            } else {
                Color32::from_gray(60)
            };

            // Background
            ui.painter()
                .rect_filled(rect, CornerRadius::same(sizing::CORNER_RADIUS), bg_color);

            // Border
            if !self.selected {
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(sizing::CORNER_RADIUS),
                    Stroke::new(1.0, Color32::from_gray(200)),
                    StrokeKind::Inside,
                );
            }

            // Line representing the width
            let line_y = rect.center().y;
            let line_start = Pos2::new(rect.left() + 6.0, line_y);
            let line_end = Pos2::new(rect.right() - 6.0, line_y);
            ui.painter().line_segment(
                [line_start, line_end],
                Stroke::new(self.width.min(4.0), line_color),
            );
        }

        let clicked = response.clicked();
        response
            .on_hover_text(self.tooltip)
            .on_hover_cursor(CursorIcon::PointingHand);
        clicked
    }
}

/// A font size button (S/M/L/XL style).
pub struct FontSizeButton<'a> {
    label: &'a str,
    size_px: f32,
    selected: bool,
}

impl<'a> FontSizeButton<'a> {
    /// Create a new font size button.
    pub fn new(label: &'a str, size_px: f32, selected: bool) -> Self {
        Self {
            label,
            size_px,
            selected,
        }
    }

    /// Show the button and return true if clicked.
    pub fn show(self, ui: &mut Ui) -> bool {
        // XL needs more width for the extra letter
        let width = if self.label == "XL" { 36.0 } else { 28.0 };
        let size = vec2(width, 24.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());

        if ui.is_rect_visible(rect) {
            let bg_color = if self.selected {
                theme::ACCENT
            } else if response.hovered() {
                Color32::from_gray(230)
            } else {
                Color32::from_gray(245)
            };

            let text_color = if self.selected {
                Color32::WHITE
            } else {
                Color32::from_gray(60)
            };

            ui.painter()
                .rect_filled(rect, CornerRadius::same(sizing::CORNER_RADIUS), bg_color);

            // Draw the label letter with size proportional to actual font size
            let display_size = match self.label {
                "S" => 10.0,
                "M" => 12.0,
                "L" => 14.0,
                "XL" => 14.0,
                _ => 12.0,
            };

            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                self.label,
                egui::FontId::proportional(display_size),
                text_color,
            );
        }

        let clicked = response.clicked();
        response
            .on_hover_text(format!("{} px", self.size_px as i32))
            .on_hover_cursor(CursorIcon::PointingHand);
        clicked
    }
}
