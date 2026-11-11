//! The tool column on the left. Every button is `tool.set` with a tool name; the active tool is read from
//! the model. Below the tools: the sticky note (`element.create_sticky`), the tool lock and the library
//! panel. The Lasso button is a toggle: clicking it while it is the tool goes back to Select. In the touch
//! layout the column has larger buttons, scrolls, and collapses to one button.

use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::json;

use crate::action::Verb;
use crate::icons::Icon;
use crate::state::{PanelKind, UiState};
use crate::theme::{MARGIN, space};
use crate::widgets::{Ctl, Cx, PANEL_INSET_Y, panel_frame, scroll_when_taller};
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

/// The arguments of the sticky note button: a note in the middle of the view, open for typing.
pub(crate) fn sticky_args() -> serde_json::Value {
    json!({ "edit": true })
}

/// The control of a tool button. The active Lasso button returns to Select (a toggle).
fn tool_ctl(tool: Tool, active: Tool) -> Ctl<'static> {
    let selected = active == tool;
    if tool == Tool::Lasso && selected {
        return Ctl::new("tool.set").args(json!({ "tool": Tool::Select.name() })).label("Lasso tool").selected(true);
    }
    let ctl = Ctl::new("tool.set").args(json!({ "tool": tool.name() })).selected(selected);
    if tool == Tool::Lasso {
        // Not a key binding of `tool.set`, so the command has no title for it.
        return ctl.label("Lasso tool");
    }
    ctl
}

/// The least height the tool column scrolls in, on a window too short for it.
const MIN_SCROLL_HEIGHT: f32 = 120.0;

/// Draws the tool column.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let screen = ctx.content_rect();
    let collapsed = st.panel_collapsed(PanelKind::Tools, screen.width());
    let compact = cx.layout.compact;
    let side = cx.layout.tool_side();
    // Centred on the window as before, and never taller than the space under the strip: it scrolls instead.
    let header = if compact { side + space::XS } else { 0.0 };
    // On a touch screen the buttons are tall and the window is often short (an iPad in landscape): the column then
    // keeps clear of the bar at the bottom left too. The desktop column keeps its size.
    let room = if compact {
        st.chrome.centred_panel_height_above_bottom_bar(screen)
    } else {
        st.chrome.centred_panel_height(screen)
    };
    let max_height = (room - 2.0 * PANEL_INSET_Y - header).max(MIN_SCROLL_HEIGHT);
    let area = Area::new(Id::unique("toolbar"))
        .anchor(Align2::LEFT_CENTER, vec2(MARGIN, 0.0))
        .order(Order::Middle)
        .show(ctx, |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, space::XS);
                if compact {
                    let label = if collapsed { "Show the tools" } else { "Hide the tools" };
                    if cx.local_button(ui, Icon::Panel, side, label, !collapsed).clicked() {
                        st.set_panel_collapsed(PanelKind::Tools, !collapsed);
                    }
                }
                if collapsed {
                    return;
                }
                scroll_when_taller(ui, "tools_scroll", max_height, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, space::XS);
                    for &(tool, icon) in TOOLS {
                        cx.tool_button(ui, &tool_ctl(tool, cx.model.tool), icon);
                    }
                    ui.add_space(space::SM);
                    let sticky = Ctl::new("element.create_sticky").args(sticky_args()).label("Sticky note");
                    cx.icon_button(ui, &sticky, Icon::Sticky, side);
                    let lock = Ctl::new("tool.lock").selected(cx.model.tool_lock);
                    cx.icon_button(ui, &lock, Icon::Lock, side);
                    cx.verb_button(ui, Verb::ToggleLibrary, Icon::Library, side, "Library", st.library_open);
                });
            });
        });
    st.chrome.tools = Some(area.response.rect);
}
