//! The tab bar at the bottom centre. Switching, closing and adding are `tab.switch`, `tab.close` and
//! `tab.new`; renaming and moving are in the palette.

use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::json;

use crate::icons::Icon;
use crate::state::UiState;
use crate::theme::MARGIN;
use crate::widgets::{Ctl, Cx, panel_frame};

/// Draws the bar. A single tab shows no close button.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, _st: &mut UiState) {
    let max_width = (ctx.content_rect().width() * 0.45).max(220.0);
    Area::new(Id::unique("tab_bar")).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -MARGIN)).order(Order::Middle).show(
        ctx,
        |ui| {
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
                            cx.toggle_button(ui, &ctl, &name);
                            if many {
                                let close = Ctl::new("tab.close")
                                    .args(json!({ "tab": tab.index }))
                                    .label(format!("Close {}", tab.name));
                                cx.icon_button(ui, &close, Icon::Close, 20.0);
                            }
                        }
                        cx.icon_button(ui, &Ctl::new("tab.new"), Icon::Plus, 24.0);
                    });
                });
            });
        },
    );
}
