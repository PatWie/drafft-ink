//! The tool column on the left. Every button is `tool.set` with a tool name; the active tool is read from
//! the model.

use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::json;

use crate::icons::Icon;
use crate::state::UiState;
use crate::theme::MARGIN;
use crate::widgets::{Ctl, Cx, panel_frame};
use drafftink_engine::state::Tool;

/// The tools, in the order of the old toolbar, with the additions (lasso, diamond, frame).
pub const TOOLS: &[(Tool, Icon)] = &[
    (Tool::Select, Icon::Select),
    (Tool::Lasso, Icon::Lasso),
    (Tool::Hand, Icon::Hand),
    (Tool::Rectangle, Icon::Rectangle),
    (Tool::Diamond, Icon::Diamond),
    (Tool::Ellipse, Icon::Ellipse),
    (Tool::Arrow, Icon::Arrow),
    (Tool::Line, Icon::Line),
    (Tool::Freehand, Icon::Freehand),
    (Tool::Highlighter, Icon::Highlighter),
    (Tool::Text, Icon::Text),
    (Tool::Math, Icon::Math),
    (Tool::Frame, Icon::Frame),
    (Tool::Eraser, Icon::Eraser),
    (Tool::Laser, Icon::Laser),
];

/// Draws the tool column.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, _st: &mut UiState) {
    Area::new(Id::unique("toolbar")).anchor(Align2::LEFT_CENTER, vec2(MARGIN, 0.0)).order(Order::Middle).show(
        ctx,
        |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
                for &(tool, icon) in TOOLS {
                    let selected = cx.model.tool == tool;
                    let mut ctl = Ctl::new("tool.set").args(json!({ "tool": tool.name() })).selected(selected);
                    if tool == Tool::Lasso {
                        // Not a key binding of `tool.set`, so the command has no title for it.
                        ctl = ctl.label("Lasso tool");
                    }
                    cx.tool_button(ui, &ctl, icon);
                }
                ui.add_space(4.0);
                let lock = Ctl::new("tool.lock").selected(cx.model.tool_lock);
                cx.icon_button(ui, &lock, Icon::Lock, crate::theme::TOOL_BUTTON);
            });
        },
    );
}
