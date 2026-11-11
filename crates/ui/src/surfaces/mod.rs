//! The surfaces. Each one is a function `show(&Context, &mut Cx, &mut UiState)` that draws from the model
//! and pushes actions.

use egui::Context;

use crate::state::UiState;
use crate::widgets::Cx;

pub mod about;
pub mod bottom;
pub mod collab;
pub mod color;
pub mod context_menu;
pub mod convert;
pub mod dialogs;
pub mod equation;
pub mod export;
pub mod library;
pub mod link;
pub mod menu;
pub mod naming;
pub mod palette;
pub mod preferences;
pub mod props;
pub mod shortcuts;
pub mod tabs;
pub mod toasts;
pub mod toolbar;
pub mod topbar;

/// Draws everything. Zen mode hides the chrome and keeps dialogs and toasts.
pub fn show_all(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if !st.zen {
        toolbar::show(ctx, cx, st);
        topbar::show(ctx, cx, st);
        props::show(ctx, cx, st);
        menu::show(ctx, cx, st);
        bottom::show(ctx, cx, st);
        tabs::show(ctx, cx, st);
        collab::show_presence(ctx, cx, st);
        library::show(ctx, cx, st);
    }
    color::show(ctx, cx, st);
    dialogs::show(ctx, cx, st);
    collab::show(ctx, cx, st);
    equation::show(ctx, cx, st);
    link::show(ctx, cx, st);
    naming::show(ctx, cx, st);
    export::show(ctx, cx, st);
    preferences::show(ctx, cx, st);
    shortcuts::show(ctx, cx, st);
    about::show(ctx, cx, st);
    convert::show(ctx, cx, st);
    context_menu::show(ctx, cx, st);
    palette::show(ctx, cx, st);
    toasts::show(ctx, cx, st);
}
