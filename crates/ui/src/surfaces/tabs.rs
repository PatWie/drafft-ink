//! The tab bar at the bottom centre. Switching, closing and adding are `tab.switch`, `tab.close` and
//! `tab.new`. A double click on a tab renames it (the rename dialog, `tab.rename`). A right click opens the
//! tab's menu: rename, duplicate (`tab.duplicate`), move left and right (`tab.move`), close.

use drafftink_engine::Origin;
use egui::{Align2, Area, Context, Id, Order, Sense, pos2, vec2};
use serde_json::json;

use crate::action::Verb;
use crate::icons::Icon;
use crate::model::TabView;
use crate::state::{RenameTarget, TabMenu, UiState};
use crate::theme::MARGIN;
use crate::widgets::{Ctl, Cx, panel_frame, separator};

/// Width of the tab menu.
const MENU_WIDTH: f32 = 190.0;

/// Draws the bar and the menu of a tab.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let max_width = if cx.layout.narrow {
        (ctx.content_rect().width() - 2.0 * MARGIN).max(220.0)
    } else {
        (ctx.content_rect().width() * 0.45).max(220.0)
    };
    // On a narrow window the bottom bar takes the corner, so the tabs sit above it.
    let lift = if cx.layout.narrow { cx.layout.small_side() + MARGIN + 28.0 } else { 0.0 };
    Area::new(Id::unique("tab_bar"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -MARGIN - lift))
        .order(Order::Middle)
        .show(ctx, |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.set_max_width(max_width);
                egui::ScrollArea::horizontal().id_salt("tab_scroll").auto_shrink([true, true]).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
                        let tabs = cx.model.tabs.clone();
                        let many = tabs.len() > 1;
                        for tab in &tabs {
                            let name = if tab.dirty { format!("{} •", tab.name) } else { tab.name.clone() };
                            let ctl = Ctl::new("tab.switch")
                                .args(json!({ "tab": tab.index }))
                                .label(tab.name.clone())
                                .selected(tab.active);
                            let resp = cx.toggle_button(ui, &ctl, &name);
                            if resp.double_clicked() {
                                st.open_rename(RenameTarget::Tab(tab.id.clone()), tab.name.clone());
                            } else if resp.secondary_clicked() {
                                let screen = resp.interact_pointer_pos().unwrap_or_else(|| resp.rect.center_top());
                                st.close_overlays();
                                st.tab_menu = Some(TabMenu { tab: tab.id.clone(), screen });
                            }
                            if many {
                                let close = Ctl::new("tab.close")
                                    .args(json!({ "tab": tab.index }))
                                    .label(format!("Close {}", tab.name));
                                cx.icon_button(ui, &close, Icon::Close, cx.layout.small_side() - 8.0);
                            }
                        }
                        cx.icon_button(ui, &Ctl::new("tab.new"), Icon::Plus, cx.layout.small_side() - 4.0);
                    });
                });
            });
        });
    menu(ctx, cx, st);
}

/// The arguments of `tab.move`.
pub(crate) fn move_args(tab: &str, to_index: usize) -> serde_json::Value {
    json!({ "tab": tab, "to_index": to_index })
}

fn menu(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(open) = st.tab_menu.clone() else {
        return;
    };
    let Some(tab) = cx.model.tabs.iter().find(|t| t.id == open.tab).cloned() else {
        st.tab_menu = None;
        return;
    };
    let count = cx.model.tabs.len();
    let screen = ctx.content_rect();
    let backdrop = Area::new(Id::unique("tab_menu.backdrop"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| ui.allocate_rect(screen, Sense::click()));
    let outside = backdrop.inner.clicked() || backdrop.inner.secondary_clicked();
    let pos = pos2(
        open.screen.x.clamp(screen.left() + MARGIN, (screen.right() - MENU_WIDTH - MARGIN).max(screen.left())),
        open.screen.y,
    );
    let mut chosen = false;
    Area::new(Id::unique("tab_menu")).order(Order::Foreground).pivot(Align2::LEFT_BOTTOM).fixed_pos(pos).show(
        ctx,
        |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.set_width(MENU_WIDTH);
                ui.spacing_mut().item_spacing.y = 1.0;
                chosen = cx.with_origin(Origin::ContextMenu, |cx| rows(ui, cx, st, &tab, count));
            });
        },
    );
    if chosen {
        st.tab_menu = None;
    } else if outside {
        cx.verb(Verb::CloseOverlays);
    }
}

fn rows(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &mut UiState, tab: &TabView, count: usize) -> bool {
    let mut chosen = false;
    if cx.plain_row(ui, "Rename…").clicked() {
        st.open_rename(RenameTarget::Tab(tab.id.clone()), tab.name.clone());
        return false;
    }
    chosen |= cx.menu_item(ui, &Ctl::new("tab.duplicate").args(json!({ "tab": tab.id })), "Duplicate").clicked();
    separator(ui, &cx.pal);
    // The rows are as available as the command is, with the engine's text when it is not. At either end the target
    // position is the tab's own, which `tab.move` accepts and leaves as it is.
    let left = Ctl::new("tab.move").args(move_args(&tab.id, tab.index.saturating_sub(1)));
    chosen |= cx.menu_item(ui, &left, "Move left").clicked();
    let right = Ctl::new("tab.move").args(move_args(&tab.id, (tab.index + 1).min(count.saturating_sub(1))));
    chosen |= cx.menu_item(ui, &right, "Move right").clicked();
    separator(ui, &cx.pal);
    chosen |= cx.menu_item(ui, &Ctl::new("tab.close").args(json!({ "tab": tab.id })), "Close").clicked();
    chosen
}
