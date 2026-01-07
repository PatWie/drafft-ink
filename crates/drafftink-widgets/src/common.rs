//! Common UI components: buttons and text inputs.

use egui::{Color32, CornerRadius, Stroke, TextEdit, Ui, Vec2};

/// Primary button (blue background, white text).
pub fn primary_btn(ui: &mut Ui, text: &str) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE))
            .fill(Color32::from_rgb(59, 130, 246))
            .min_size(Vec2::new(80.0, 32.0))
            .corner_radius(CornerRadius::same(6)),
    )
    .clicked()
}

/// Secondary button (gray background, gray text).
pub fn secondary_btn(ui: &mut Ui, text: &str) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::from_gray(100)))
            .fill(Color32::from_gray(240))
            .min_size(Vec2::new(80.0, 32.0))
            .corner_radius(CornerRadius::same(6)),
    )
    .clicked()
}

/// Default button (frameless close button).
pub fn default_btn(ui: &mut Ui, text: &str) -> bool {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(16.0)
                .color(Color32::from_gray(100)),
        )
        .frame(false),
    )
    .clicked()
}

/// Single-line text input with modern styling.
pub fn input_text(ui: &mut Ui, text: &mut String, width: f32, hint: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(220));
        ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(180));
        ui.visuals_mut().widgets.active.bg_stroke =
            Stroke::new(1.0, Color32::from_rgb(59, 130, 246));

        ui.add(
            TextEdit::singleline(text)
                .desired_width(width)
                .text_color(Color32::from_gray(30))
                .background_color(Color32::WHITE)
                .hint_text(hint)
                .frame(true),
        )
    })
    .inner
}
