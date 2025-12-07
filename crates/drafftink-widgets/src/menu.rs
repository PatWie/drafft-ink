//! Menu components: menu items, separators, panel frames.

use egui::{
    Color32, CornerRadius, CursorIcon, Frame, Margin, Pos2, Response, Sense, Stroke, Ui, vec2,
};

use crate::{sizing, theme};

/// Response from a menu item click.
pub struct MenuItemResponse {
    /// Whether the item was clicked
    pub clicked: bool,
    /// The underlying egui Response
    pub response: Response,
}

/// Show a menu item with label and optional shortcut.
pub fn menu_item(ui: &mut Ui, label: &str, shortcut: &str) -> bool {
    menu_item_inner(ui, label, shortcut, true)
}

/// Show a menu item that can be enabled/disabled.
pub fn menu_item_enabled(ui: &mut Ui, label: &str, shortcut: &str, enabled: bool) -> bool {
    // We handle disabled state manually in menu_item_inner
    menu_item_inner(ui, label, shortcut, enabled)
}

fn menu_item_inner(ui: &mut Ui, label: &str, shortcut: &str, enabled: bool) -> bool {
    let size = vec2(ui.available_width(), 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    if ui.is_rect_visible(rect) {
        let bg_color = if !enabled {
            Color32::TRANSPARENT
        } else if response.hovered() {
            theme::HOVER_BG
        } else {
            Color32::TRANSPARENT
        };

        ui.painter()
            .rect_filled(rect, CornerRadius::same(sizing::CORNER_RADIUS), bg_color);

        let text_color = if enabled {
            theme::TEXT
        } else {
            Color32::from_gray(180)
        };

        // Draw label
        ui.painter().text(
            Pos2::new(rect.left() + 12.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            text_color,
        );

        // Draw shortcut
        if !shortcut.is_empty() {
            let shortcut_color = if enabled {
                theme::TEXT_MUTED
            } else {
                Color32::from_gray(200)
            };
            ui.painter().text(
                Pos2::new(rect.right() - 12.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                shortcut,
                egui::FontId::proportional(12.0),
                shortcut_color,
            );
        }
    }

    let clicked = response.clicked();
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand);
    }
    enabled && clicked
}

/// Draw a menu separator line.
pub fn menu_separator(ui: &mut Ui) {
    ui.add_space(4.0);
    let rect = ui.available_rect_before_wrap();
    let y = rect.top();
    ui.painter().line_segment(
        [
            Pos2::new(rect.left() + 8.0, y),
            Pos2::new(rect.right() - 8.0, y),
        ],
        Stroke::new(1.0, Color32::from_gray(230)),
    );
    ui.add_space(4.0);
}

/// Create a standard panel frame with shadow.
pub fn panel_frame() -> Frame {
    Frame::new()
        .fill(theme::PANEL_BG)
        .corner_radius(CornerRadius::same(sizing::PANEL_RADIUS))
        .stroke(Stroke::new(1.0, theme::BORDER))
        .shadow(egui::epaint::Shadow {
            spread: 0,
            blur: 8,
            offset: [0, 2],
            color: Color32::from_black_alpha(15),
        })
        .inner_margin(Margin::same(8))
}

/// Create a toolbar panel frame (slightly different padding).
pub fn toolbar_frame() -> Frame {
    Frame::new()
        .fill(theme::PANEL_BG)
        .corner_radius(CornerRadius::same(sizing::PANEL_RADIUS))
        .stroke(Stroke::new(1.0, theme::BORDER))
        .shadow(egui::epaint::Shadow {
            spread: 0,
            blur: 6,
            offset: [0, 2],
            color: Color32::from_black_alpha(10),
        })
        .inner_margin(Margin::symmetric(12, 6))
}
