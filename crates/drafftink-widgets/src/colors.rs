//! Color palette and color picker components.
//!
//! Includes the full Tailwind CSS color palette and components for color selection.

use egui::{
    vec2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::{sizing, theme};

/// A Tailwind color with all shade variants (25-950).
#[derive(Clone, Copy)]
pub struct TailwindColor {
    /// Color name (e.g., "Red", "Blue")
    pub name: &'static str,
    /// Shades from 25 to 950 (12 total)
    pub shades: [Color32; 12],
}

impl TailwindColor {
    /// Create a new TailwindColor from RGB tuples.
    pub const fn new(name: &'static str, shades: [(u8, u8, u8); 12]) -> Self {
        Self {
            name,
            shades: [
                Color32::from_rgb(shades[0].0, shades[0].1, shades[0].2),
                Color32::from_rgb(shades[1].0, shades[1].1, shades[1].2),
                Color32::from_rgb(shades[2].0, shades[2].1, shades[2].2),
                Color32::from_rgb(shades[3].0, shades[3].1, shades[3].2),
                Color32::from_rgb(shades[4].0, shades[4].1, shades[4].2),
                Color32::from_rgb(shades[5].0, shades[5].1, shades[5].2),
                Color32::from_rgb(shades[6].0, shades[6].1, shades[6].2),
                Color32::from_rgb(shades[7].0, shades[7].1, shades[7].2),
                Color32::from_rgb(shades[8].0, shades[8].1, shades[8].2),
                Color32::from_rgb(shades[9].0, shades[9].1, shades[9].2),
                Color32::from_rgb(shades[10].0, shades[10].1, shades[10].2),
                Color32::from_rgb(shades[11].0, shades[11].1, shades[11].2),
            ],
        }
    }

    /// Get the 500-level color (primary shade, index 6)
    pub const fn primary(&self) -> Color32 {
        self.shades[6]
    }

    /// Get shade by index (0=25, 1=50, ..., 6=500, ..., 11=950)
    pub const fn shade(&self, index: usize) -> Color32 {
        self.shades[index]
    }
}

/// Shade labels for display
pub const SHADE_LABELS: [&str; 12] = [
    "25", "50", "100", "200", "300", "400", "500", "600", "700", "800", "900", "950",
];

/// The complete Tailwind CSS color palette.
pub struct TailwindPalette;

impl TailwindPalette {
    /// Get all colors in the palette
    pub fn all() -> &'static [TailwindColor] {
        TAILWIND_COLORS
    }

    /// Get color by name
    pub fn by_name(name: &str) -> Option<&'static TailwindColor> {
        TAILWIND_COLORS.iter().find(|c| c.name == name)
    }

    /// Get quick selection colors (commonly used subset)
    /// Returns indices into the TAILWIND_COLORS array
    pub fn quick_colors() -> &'static [usize] {
        // Blue, Amber, Lime, Indigo, Purple, Rose, Slate
        &[10, 2, 4, 11, 13, 16, 17]
    }
}

// Extended Tailwind colors with 25 shade (OKLCH perceptual interpolation)
pub const TAILWIND_COLORS: &[TailwindColor] = &[
    TailwindColor::new("Red", [
        (253, 252, 252), (254, 242, 242), (254, 226, 226), (254, 202, 202), (252, 165, 165),
        (248, 113, 113), (239, 68, 68), (220, 38, 38), (185, 28, 28), (153, 27, 27), (127, 29, 29), (69, 10, 10),
    ]),
    TailwindColor::new("Orange", [
        (253, 252, 250), (255, 247, 237), (255, 237, 213), (254, 215, 170), (253, 186, 116),
        (251, 146, 60), (249, 115, 22), (234, 88, 12), (194, 65, 12), (154, 52, 18), (124, 45, 18), (67, 20, 7),
    ]),
    TailwindColor::new("Amber", [
        (254, 254, 254), (255, 251, 235), (254, 243, 199), (253, 230, 138), (252, 211, 77),
        (251, 191, 36), (245, 158, 11), (217, 119, 6), (180, 83, 9), (146, 64, 14), (120, 53, 15), (69, 26, 3),
    ]),
    TailwindColor::new("Yellow", [
        (253, 253, 253), (254, 252, 232), (254, 249, 195), (254, 240, 138), (253, 224, 71),
        (250, 204, 21), (234, 179, 8), (202, 138, 4), (161, 98, 7), (133, 77, 14), (113, 63, 18), (66, 32, 6),
    ]),
    TailwindColor::new("Lime", [
        (253, 255, 248), (247, 254, 231), (236, 252, 203), (217, 249, 157), (190, 242, 100),
        (163, 230, 53), (132, 204, 22), (101, 163, 13), (77, 124, 15), (63, 98, 18), (54, 83, 20), (26, 46, 5),
    ]),
    TailwindColor::new("Green", [
        (252, 252, 252), (240, 253, 244), (220, 252, 231), (187, 247, 208), (134, 239, 172),
        (74, 222, 128), (34, 197, 94), (22, 163, 74), (21, 128, 61), (22, 101, 52), (20, 83, 45), (5, 46, 22),
    ]),
    TailwindColor::new("Emerald", [
        (254, 254, 254), (236, 253, 245), (209, 250, 229), (167, 243, 208), (110, 231, 183),
        (52, 211, 153), (16, 185, 129), (5, 150, 105), (4, 120, 87), (6, 95, 70), (6, 78, 59), (2, 44, 34),
    ]),
    TailwindColor::new("Teal", [
        (254, 254, 254), (240, 253, 250), (204, 251, 241), (153, 246, 228), (94, 234, 212),
        (45, 212, 191), (20, 184, 166), (13, 148, 136), (15, 118, 110), (17, 94, 89), (19, 78, 74), (4, 47, 46),
    ]),
    TailwindColor::new("Cyan", [
        (254, 254, 254), (236, 254, 255), (207, 250, 254), (165, 243, 252), (103, 232, 249),
        (34, 211, 238), (6, 182, 212), (8, 145, 178), (14, 116, 144), (21, 94, 117), (22, 78, 99), (8, 51, 68),
    ]),
    TailwindColor::new("Sky", [
        (249, 253, 255), (240, 249, 255), (224, 242, 254), (186, 230, 253), (125, 211, 252),
        (56, 189, 248), (14, 165, 233), (2, 132, 199), (3, 105, 161), (7, 89, 133), (12, 74, 110), (8, 47, 73),
    ]),
    TailwindColor::new("Blue", [
        (253, 254, 255), (239, 246, 255), (219, 234, 254), (191, 219, 254), (147, 197, 253),
        (96, 165, 250), (59, 130, 246), (37, 99, 235), (29, 78, 216), (30, 64, 175), (30, 58, 138), (23, 37, 84),
    ]),
    TailwindColor::new("Indigo", [
        (246, 248, 254), (238, 242, 255), (224, 231, 255), (199, 210, 254), (165, 180, 252),
        (129, 140, 248), (99, 102, 241), (79, 70, 229), (67, 56, 202), (55, 48, 163), (49, 46, 129), (30, 27, 75),
    ]),
    TailwindColor::new("Violet", [
        (250, 249, 255), (245, 243, 255), (237, 233, 254), (221, 214, 254), (196, 181, 253),
        (167, 139, 250), (139, 92, 246), (124, 58, 237), (109, 40, 217), (91, 33, 182), (76, 29, 149), (46, 16, 101),
    ]),
    TailwindColor::new("Purple", [
        (254, 254, 255), (250, 245, 255), (243, 232, 255), (233, 213, 255), (216, 180, 254),
        (192, 132, 252), (168, 85, 247), (147, 51, 234), (126, 34, 206), (107, 33, 168), (88, 28, 135), (59, 7, 100),
    ]),
    TailwindColor::new("Fuchsia", [
        (254, 251, 255), (253, 244, 255), (250, 232, 255), (245, 208, 254), (240, 171, 252),
        (232, 121, 249), (217, 70, 239), (192, 38, 211), (162, 28, 175), (134, 25, 143), (112, 26, 117), (74, 4, 78),
    ]),
    TailwindColor::new("Pink", [
        (253, 248, 251), (253, 242, 248), (252, 231, 243), (251, 207, 232), (249, 168, 212),
        (244, 114, 182), (236, 72, 153), (219, 39, 119), (190, 24, 93), (157, 23, 77), (131, 24, 67), (80, 7, 36),
    ]),
    TailwindColor::new("Rose", [
        (254, 249, 250), (255, 241, 242), (255, 228, 230), (254, 205, 211), (253, 164, 175),
        (251, 113, 133), (244, 63, 94), (225, 29, 72), (190, 18, 60), (159, 18, 57), (136, 19, 55), (76, 5, 25),
    ]),
    TailwindColor::new("Slate", [
        (250, 251, 252), (248, 250, 252), (241, 245, 249), (226, 232, 240), (203, 213, 225),
        (148, 163, 184), (100, 116, 139), (71, 85, 105), (51, 65, 85), (30, 41, 59), (15, 23, 42), (2, 6, 23),
    ]),
    TailwindColor::new("Gray", [
        (251, 252, 253), (249, 250, 251), (243, 244, 246), (229, 231, 235), (209, 213, 219),
        (156, 163, 175), (107, 114, 128), (75, 85, 99), (55, 65, 81), (31, 41, 55), (17, 24, 39), (3, 7, 18),
    ]),
    TailwindColor::new("Zinc", [
        (250, 250, 250), (250, 250, 250), (244, 244, 245), (228, 228, 231), (212, 212, 216),
        (161, 161, 170), (113, 113, 122), (82, 82, 91), (63, 63, 70), (39, 39, 42), (24, 24, 27), (9, 9, 11),
    ]),
    TailwindColor::new("Neutral", [
        (250, 250, 250), (250, 250, 250), (245, 245, 245), (229, 229, 229), (212, 212, 212),
        (163, 163, 163), (115, 115, 115), (82, 82, 82), (64, 64, 64), (38, 38, 38), (23, 23, 23), (10, 10, 10),
    ]),
    TailwindColor::new("Stone", [
        (250, 250, 249), (250, 250, 249), (245, 245, 244), (231, 229, 228), (214, 211, 209),
        (168, 162, 158), (120, 113, 108), (87, 83, 78), (68, 64, 60), (41, 37, 36), (28, 25, 23), (12, 10, 9),
    ]),
];

/// Style for color swatches.
#[derive(Clone)]
pub struct ColorSwatchStyle {
    /// Size of the swatch
    pub size: Vec2,
    /// Whether to show as circle (true) or rounded rect (false)
    pub circular: bool,
    /// Selection indicator style
    pub selection_style: SelectionStyle,
}

/// How to indicate selection on a color swatch.
#[derive(Clone, Copy, PartialEq)]
pub enum SelectionStyle {
    /// No selection indicator
    None,
    /// Inner offset ring (like Excalidraw)
    InnerRing,
    /// Outer border
    OuterBorder,
}

impl Default for ColorSwatchStyle {
    fn default() -> Self {
        Self {
            size: vec2(sizing::SMALL, sizing::SMALL),
            circular: true,
            selection_style: SelectionStyle::InnerRing,
        }
    }
}

impl ColorSwatchStyle {
    /// Small circular swatch (default)
    pub fn small() -> Self {
        Self::default()
    }

    /// Grid swatch (smaller, for color grids)
    pub fn grid() -> Self {
        Self {
            size: vec2(16.0, 16.0),
            circular: true,
            selection_style: SelectionStyle::InnerRing,
        }
    }

    /// Large swatch
    pub fn large() -> Self {
        Self {
            size: vec2(28.0, 28.0),
            circular: true,
            selection_style: SelectionStyle::InnerRing,
        }
    }
}

/// A clickable color swatch.
pub struct ColorSwatch<'a> {
    color: Color32,
    tooltip: &'a str,
    selected: bool,
    style: ColorSwatchStyle,
}

impl<'a> ColorSwatch<'a> {
    /// Create a new color swatch.
    pub fn new(color: Color32, tooltip: &'a str) -> Self {
        Self {
            color,
            tooltip,
            selected: false,
            style: ColorSwatchStyle::default(),
        }
    }

    /// Set whether this swatch is selected.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Set the style.
    pub fn style(mut self, style: ColorSwatchStyle) -> Self {
        self.style = style;
        self
    }

    /// Use grid style.
    pub fn grid(mut self) -> Self {
        self.style = ColorSwatchStyle::grid();
        self
    }

    /// Show the swatch and return (clicked, rect).
    pub fn show(self, ui: &mut Ui) -> (bool, Rect) {
        let (rect, response) = ui.allocate_exact_size(self.style.size, Sense::click());

        if ui.is_rect_visible(rect) {
            let center = rect.center();
            let radius = rect.width().min(rect.height()) / 2.0;

            if self.style.circular {
                // Circular swatch
                ui.painter().circle_filled(center, radius, self.color);

                if self.selected && self.style.selection_style == SelectionStyle::InnerRing {
                    // Inner offset ring
                    ui.painter().circle_stroke(
                        center,
                        radius - 3.0,
                        Stroke::new(2.0, Color32::from_gray(30)),
                    );
                } else if self.selected && self.style.selection_style == SelectionStyle::OuterBorder
                {
                    ui.painter()
                        .circle_stroke(center, radius, Stroke::new(2.0, theme::ACCENT));
                }
            } else {
                // Rounded rect swatch
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(sizing::CORNER_RADIUS),
                    self.color,
                );

                if self.selected {
                    ui.painter().rect_stroke(
                        rect,
                        CornerRadius::same(sizing::CORNER_RADIUS),
                        Stroke::new(2.0, Color32::from_gray(30)),
                        StrokeKind::Inside,
                    );
                }
            }
        }

        let clicked = response.clicked();
        response.on_hover_text(self.tooltip).on_hover_cursor(CursorIcon::PointingHand);
        (clicked, rect)
    }
}

/// A color swatch with hue wheel indicator (for "current color" buttons).
pub struct ColorSwatchWithWheel<'a> {
    color: Color32,
    tooltip: &'a str,
    size: Vec2,
}

impl<'a> ColorSwatchWithWheel<'a> {
    /// Create a new color swatch with hue wheel.
    pub fn new(color: Color32, tooltip: &'a str) -> Self {
        Self {
            color,
            tooltip,
            size: vec2(sizing::SMALL, sizing::SMALL),
        }
    }

    /// Set the size.
    pub fn size(mut self, size: Vec2) -> Self {
        self.size = size;
        self
    }

    /// Show the swatch and return (clicked, rect).
    pub fn show(self, ui: &mut Ui) -> (bool, Rect) {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::click());

        if ui.is_rect_visible(rect) {
            let center = rect.center();
            let outer_radius = rect.width().min(rect.height()) / 2.0;
            let ring_width = 3.0;
            let inner_radius = outer_radius - ring_width;

            // Draw hue wheel as segments
            let num_segments = 32;
            for i in 0..num_segments {
                let angle1 = (i as f32 / num_segments as f32) * std::f32::consts::TAU;
                let angle2 = ((i + 1) as f32 / num_segments as f32) * std::f32::consts::TAU;

                let hue = i as f32 / num_segments as f32;
                let hue_color = hue_to_rgb(hue);

                let p1 = Pos2::new(
                    center.x + outer_radius * angle1.cos(),
                    center.y + outer_radius * angle1.sin(),
                );
                let p2 = Pos2::new(
                    center.x + outer_radius * angle2.cos(),
                    center.y + outer_radius * angle2.sin(),
                );
                let p3 = Pos2::new(
                    center.x + inner_radius * angle2.cos(),
                    center.y + inner_radius * angle2.sin(),
                );
                let p4 = Pos2::new(
                    center.x + inner_radius * angle1.cos(),
                    center.y + inner_radius * angle1.sin(),
                );

                ui.painter().add(egui::Shape::convex_polygon(
                    vec![p1, p2, p3, p4],
                    hue_color,
                    Stroke::NONE,
                ));
            }

            // Black inner circle (offset)
            ui.painter()
                .circle_filled(center, inner_radius, Color32::from_gray(30));

            // Inner color fill
            ui.painter()
                .circle_filled(center, inner_radius - 2.0, self.color);
        }

        let clicked = response.clicked();
        response.on_hover_text(self.tooltip).on_hover_cursor(CursorIcon::PointingHand);
        (clicked, rect)
    }
}

/// A "no color" swatch (white with red diagonal).
pub struct NoColorSwatch<'a> {
    tooltip: &'a str,
    selected: bool,
    size: Vec2,
}

impl<'a> NoColorSwatch<'a> {
    /// Create a new "no color" swatch.
    pub fn new(tooltip: &'a str) -> Self {
        Self {
            tooltip,
            selected: false,
            size: vec2(sizing::SMALL, sizing::SMALL),
        }
    }

    /// Set whether this swatch is selected.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Use grid size (smaller, for color grids).
    pub fn grid(mut self) -> Self {
        self.size = vec2(16.0, 16.0);
        self
    }

    /// Show the swatch and return true if clicked.
    pub fn show(self, ui: &mut Ui) -> bool {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::click());

        if ui.is_rect_visible(rect) {
            let center = rect.center();
            let radius = rect.width().min(rect.height()) / 2.0;

            // White circle
            ui.painter().circle_filled(center, radius, Color32::WHITE);
            ui.painter()
                .circle_stroke(center, radius, Stroke::new(1.0, Color32::from_gray(200)));

            // Red diagonal line
            let offset = radius * 0.6;
            ui.painter().line_segment(
                [
                    Pos2::new(center.x - offset, center.y + offset),
                    Pos2::new(center.x + offset, center.y - offset),
                ],
                Stroke::new(2.0, Color32::from_rgb(239, 68, 68)),
            );

            if self.selected {
                ui.painter().circle_stroke(
                    center,
                    radius - 3.0,
                    Stroke::new(2.0, Color32::from_gray(30)),
                );
            }
        }

        let clicked = response.clicked();
        response.on_hover_text(self.tooltip).on_hover_cursor(CursorIcon::PointingHand);
        clicked
    }
}

/// A full color grid picker using Tailwind colors.
pub struct ColorGrid<'a> {
    current_color: Color32,
    title: &'a str,
    /// Which shades to show (indices into SHADE_LABELS)
    shade_indices: &'a [usize],
    position: ColorGridPosition,
}

/// Position of the color grid relative to anchor.
#[derive(Clone, Copy, Default)]
pub enum ColorGridPosition {
    /// Below the anchor (for top toolbars)
    #[default]
    Below,
    /// Above the anchor (for bottom toolbars)
    Above,
}

impl<'a> ColorGrid<'a> {
    /// Create a new color grid.
    pub fn new(current_color: Color32, title: &'a str) -> Self {
        Self {
            current_color,
            title,
            shade_indices: &[2, 3, 4, 5, 6, 7, 8, 9, 10], // 100-900 by default
            position: ColorGridPosition::Below,
        }
    }

    /// Set which shade indices to show.
    pub fn shades(mut self, indices: &'a [usize]) -> Self {
        self.shade_indices = indices;
        self
    }

    /// Position the grid above the anchor.
    pub fn above(mut self) -> Self {
        self.position = ColorGridPosition::Above;
        self
    }

    /// Position the grid below the anchor.
    pub fn below(mut self) -> Self {
        self.position = ColorGridPosition::Below;
        self
    }

    /// Show the color grid at the given anchor rect.
    /// Returns the selected color if one was clicked.
    pub fn show(self, ctx: &egui::Context, anchor_rect: Rect) -> Option<Color32> {
        let mut selected = None;

        // Calculate position
        let grid_height = 220.0;
        let pos = match self.position {
            ColorGridPosition::Below => {
                Pos2::new(anchor_rect.left() - 100.0, anchor_rect.bottom() + 8.0)
            }
            ColorGridPosition::Above => {
                Pos2::new(anchor_rect.left() - 50.0, anchor_rect.top() - grid_height - 8.0)
            }
        };

        egui::Area::new(egui::Id::new("color_grid"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                crate::menu::panel_frame().show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = vec2(0.0, 4.0);

                        // Header
                        ui.label(
                            egui::RichText::new(self.title)
                                .size(12.0)
                                .color(theme::TEXT_MUTED),
                        );

                        ui.add_space(4.0);

                        // Color grid with quick access column on right
                        // Quick access colors: None, Black, White, Blue, Red, Emerald, Amber, Purple, Slate
                        // Using Tailwind 500-level colors for the named colors
                        #[derive(Clone, Copy)]
                        enum QuickColor {
                            None,
                            Black,
                            White,
                            Tailwind(usize), // Index into TAILWIND_COLORS, uses shade 5 (500-level)
                        }
                        let quick_colors: [QuickColor; 9] = [
                            QuickColor::None,
                            QuickColor::Black,
                            QuickColor::White,
                            QuickColor::Tailwind(10), // Blue
                            QuickColor::Tailwind(0),  // Red
                            QuickColor::Tailwind(6),  // Emerald
                            QuickColor::Tailwind(2),  // Amber
                            QuickColor::Tailwind(13), // Purple
                            QuickColor::Tailwind(17), // Slate
                        ];
                        let quick_tooltips = ["None", "Black", "White", "Blue", "Red", "Emerald", "Amber", "Purple", "Slate"];

                        for (row_idx, &shade_idx) in self.shade_indices.iter().enumerate() {
                            // Add padding above 500-level row
                            if shade_idx == 6 {
                                ui.add_space(4.0);
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                                
                                // Main Tailwind color grid
                                for tw_color in TAILWIND_COLORS.iter() {
                                    let color = tw_color.shades[shade_idx];
                                    let is_selected = colors_match(self.current_color, color);
                                    let tooltip =
                                        format!("{} {}", tw_color.name, SHADE_LABELS[shade_idx]);
                                    let (clicked, _) = ColorSwatch::new(color, &tooltip)
                                        .selected(is_selected)
                                        .grid()
                                        .show(ui);
                                    if clicked {
                                        selected = Some(color);
                                    }
                                }
                                
                                // Quick access column (only for first 9 rows)
                                if row_idx < quick_colors.len() {
                                    ui.add_space(4.0);
                                    
                                    // Quick access color
                                    match quick_colors[row_idx] {
                                        QuickColor::None => {
                                            // "No color" swatch
                                            let is_selected = self.current_color.a() == 0;
                                            if NoColorSwatch::new(quick_tooltips[row_idx])
                                                .selected(is_selected)
                                                .grid()
                                                .show(ui)
                                            {
                                                selected = Some(Color32::TRANSPARENT);
                                            }
                                        }
                                        QuickColor::Black => {
                                            let is_selected = colors_match(self.current_color, Color32::BLACK);
                                            let (clicked, _) = ColorSwatch::new(Color32::BLACK, quick_tooltips[row_idx])
                                                .selected(is_selected)
                                                .grid()
                                                .show(ui);
                                            if clicked {
                                                selected = Some(Color32::BLACK);
                                            }
                                        }
                                        QuickColor::White => {
                                            // White circle with gray border
                                            let size = vec2(16.0, 16.0);
                                            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                                            if ui.is_rect_visible(rect) {
                                                let center = rect.center();
                                                let radius = 8.0;
                                                ui.painter().circle_filled(center, radius, Color32::WHITE);
                                                ui.painter().circle_stroke(center, radius, Stroke::new(1.0, Color32::from_gray(180)));
                                                let is_selected = colors_match(self.current_color, Color32::WHITE);
                                                if is_selected {
                                                    ui.painter().circle_stroke(center, radius - 3.0, Stroke::new(2.0, Color32::from_gray(30)));
                                                }
                                            }
                                            let clicked = response.clicked();
                                            response.on_hover_text(quick_tooltips[row_idx]).on_hover_cursor(CursorIcon::PointingHand);
                                            if clicked {
                                                selected = Some(Color32::WHITE);
                                            }
                                        }
                                        QuickColor::Tailwind(idx) => {
                                            let color = TAILWIND_COLORS[idx].shades[6]; // 500-level
                                            let is_selected = colors_match(self.current_color, color);
                                            let (clicked, _) = ColorSwatch::new(color, quick_tooltips[row_idx])
                                                .selected(is_selected)
                                                .grid()
                                                .show(ui);
                                            if clicked {
                                                selected = Some(color);
                                            }
                                        }
                                    }
                                }
                            });
                            // Add padding below 500-level row
                            if shade_idx == 6 {
                                ui.add_space(4.0);
                            }
                        }
                    });
                });
            });

        selected
    }
}

/// Check if two colors match (for selection highlighting).
pub fn colors_match(a: Color32, b: Color32) -> bool {
    a.r() == b.r() && a.g() == b.g() && a.b() == b.b()
}

/// Convert hue (0.0-1.0) to RGB color.
pub fn hue_to_rgb(hue: f32) -> Color32 {
    let h = hue * 6.0;
    let c = 1.0_f32;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());

    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// Parse a CSS color string (e.g., "#6366f1") to Color32.
pub fn parse_css_color(color: &str) -> Color32 {
    if color.starts_with('#') && color.len() == 7 {
        let r = u8::from_str_radix(&color[1..3], 16).unwrap_or(128);
        let g = u8::from_str_radix(&color[3..5], 16).unwrap_or(128);
        let b = u8::from_str_radix(&color[5..7], 16).unwrap_or(128);
        Color32::from_rgb(r, g, b)
    } else {
        Color32::from_rgb(128, 128, 128)
    }
}
