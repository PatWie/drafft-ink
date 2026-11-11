//! Helpers of the palette, shortcuts and context-menu tests: a harness that installs the GelPen fonts
//! and the theme before the first frame, as `UiShell::frame` does, because these surfaces draw their
//! headings in the hand font.
#![allow(dead_code)]

use drafftink_ui::{draw, theme};
use egui_kittest::Harness;

use crate::common::Fixture;

/// A harness like `common::harness`, with fonts and theme installed and a chosen egui theme. A missing
/// glyph is an error, so a surface that draws a character the bundled fonts lack fails its test.
pub fn themed(fixture: Fixture, dark: bool) -> Harness<'static, Fixture> {
    build(fixture, dark, false)
}

/// Like [`themed`], but a missing glyph draws a box instead of failing. For the context menu, whose rows
/// get their shortcut text from `Cx::menu_item`, which prints chord labels with arrows (`Ctrl+Alt+←`) in
/// a font that has no arrow glyph.
pub fn themed_lenient(fixture: Fixture, dark: bool) -> Harness<'static, Fixture> {
    build(fixture, dark, true)
}

/// Like [`themed_lenient`] in a window of a given size (a menu with all its blocks open is taller than 800).
pub fn themed_lenient_sized(fixture: Fixture, size: egui::Vec2) -> Harness<'static, Fixture> {
    build_sized(fixture, false, true, size)
}

fn build(fixture: Fixture, dark: bool, allow_missing_glyphs: bool) -> Harness<'static, Fixture> {
    build_sized(fixture, dark, allow_missing_glyphs, egui::vec2(1280.0, 800.0))
}

fn build_sized(
    fixture: Fixture,
    dark: bool,
    allow_missing_glyphs: bool,
    size: egui::Vec2,
) -> Harness<'static, Fixture> {
    let builder = Harness::builder();
    let builder = if allow_missing_glyphs { builder.allow_missing_glyphs() } else { builder };
    builder
        .with_size(size)
        .with_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light })
        .with_context_setup(|ctx| {
            theme::install_fonts(ctx);
            theme::apply(ctx, None);
        })
        .build_ui_state(
            |ui, f: &mut Fixture| {
                let acts = draw(ui, &f.model, &mut f.state);
                f.actions.extend(acts);
            },
            fixture,
        )
}
