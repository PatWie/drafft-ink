//! Layout helpers: separators, section labels, spacing.

use egui::{Color32, Stroke, Ui};

use crate::theme;

/// Draw a horizontal separator line.
pub fn separator(ui: &mut Ui) {
    let rect = ui.available_rect_before_wrap();
    let y = rect.top() + 4.0;
    ui.painter().line_segment(
        [
            egui::Pos2::new(rect.left(), y),
            egui::Pos2::new(rect.right(), y),
        ],
        Stroke::new(1.0, Color32::from_gray(230)),
    );
    ui.add_space(8.0);
}

/// Draw a vertical separator line (small height).
pub fn vertical_separator(ui: &mut Ui) {
    let rect = ui.available_rect_before_wrap();
    let height = 14.0;
    let x = rect.left() + 1.0;
    let top = rect.center().y - height / 2.0;
    ui.painter().line_segment(
        [egui::Pos2::new(x, top), egui::Pos2::new(x, top + height)],
        Stroke::new(1.0, Color32::from_gray(210)),
    );
    ui.add_space(3.0);
}

/// Draw a section label (small, muted text).
pub fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(10.0)
            .color(theme::TEXT_MUTED),
    );
}
