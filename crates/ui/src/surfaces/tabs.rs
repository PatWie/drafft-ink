//! The tab bar at the bottom centre. Switching, closing and adding are `tab.switch`, `tab.close` and
//! `tab.new`. A double click on a tab renames it (the rename dialog, `tab.rename`). A right click opens the
//! tab's menu: rename, duplicate (`tab.duplicate`), move left and right (`tab.move`), close.

use drafftink_engine::Origin;
use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::json;

use crate::action::Verb;
use crate::chrome::{GAP, lift_to};
use crate::icons::Icon;
use crate::menu_kit::{self, Menu, Popup, Row};
use crate::model::TabView;
use crate::state::{RenameTarget, TabMenu, UiState};
use crate::theme::{self, MARGIN, space};
use crate::widgets::{Ctl, Cx, panel_frame};

/// Draws the bar and the menu of a tab.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let max_width = if cx.layout.narrow {
        (ctx.content_rect().width() - 2.0 * MARGIN).max(220.0)
    } else {
        (ctx.content_rect().width() * 0.45).max(220.0)
    };
    // The tabs sit at the bottom centre, beside the bottom bar. Where the bar reaches past the tabs' left edge (a
    // narrow window, or many tabs) they sit above it instead.
    let screen = ctx.content_rect();
    let id = Id::unique("tab_bar");
    let width = ctx.memory(|m| m.area_rect(id)).map_or(0.0, |r| r.width());
    let beside = st.chrome.bottom_bar.is_none_or(|bar| bar.right() + GAP + width / 2.0 <= screen.center().x);
    let lift = if beside { MARGIN } else { lift_to(screen, st.chrome.above_bottom_bar(screen), GAP) };
    let area = Area::new(id).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -lift)).order(Order::Middle).show(ctx, |ui| {
        panel_frame(&cx.pal).show(ui, |ui| {
            ui.set_max_width(max_width);
            egui::ScrollArea::horizontal().id_salt("tab_scroll").auto_shrink([true, true]).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(space::SM, 0.0);
                    let tabs = cx.model.tabs.clone();
                    let many = tabs.len() > 1;
                    for tab in &tabs {
                        theme::fallback::note_text(ui.ctx(), &tab.name);
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
    st.chrome.tabs = Some(area.response.rect);
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
    let spec = Popup { key: Id::unique("tab_menu"), label: "Tab menu", pivot: Align2::LEFT_BOTTOM, at: open.screen };
    let result =
        cx.with_origin(Origin::ContextMenu, |cx| menu_kit::popup(ctx, cx, &spec, |m| rows(m, st, &tab, count)));
    if result.picked.any() {
        st.tab_menu = None;
    } else if result.outside {
        cx.verb(Verb::CloseOverlays);
    }
}

fn rows(m: &mut Menu<'_, '_, '_>, st: &mut UiState, tab: &TabView, count: usize) {
    if m.local("Rename tab", Row::new().icon(Icon::Text).dialog()).clicked() {
        st.open_rename(RenameTarget::Tab(tab.id.clone()), tab.name.clone());
    }
    m.command(Ctl::new("tab.duplicate").args(json!({ "tab": tab.id })), Row::new().icon(Icon::Duplicate));
    m.separator();
    // The rows are as available as the command is, with the engine's text when it is not. At either end the target
    // position is the tab's own, which `tab.move` accepts and leaves as it is.
    let left = Ctl::new("tab.move").args(move_args(&tab.id, tab.index.saturating_sub(1)));
    m.command(left, Row::new().label("Move left"));
    let right = Ctl::new("tab.move").args(move_args(&tab.id, (tab.index + 1).min(count.saturating_sub(1))));
    m.command(right, Row::new().label("Move right"));
    m.separator();
    m.command(Ctl::new("tab.close").args(json!({ "tab": tab.id })), Row::new().icon(Icon::Close));
}
