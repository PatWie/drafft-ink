//! The tool column on the left: the 16 tools and nothing else. Every button is `tool.set` with a tool name; the
//! active tool is read from the model. The Lasso button is a toggle: clicking it while it is the tool goes back
//! to Select.
//!
//! The tool lock is on the buttons too. A click on a tool clears the lock (`tool.lock` off, when it is on), and a
//! double-click keeps the tool (`tool.lock` on): the first click of the pair runs `tool.set`, the second only the
//! lock. The locked tool wears a small padlock in the corner of its button. The Q key and the command `tool.lock`
//! switch the lock without a button. In the touch layout the column has larger buttons, scrolls, and collapses to
//! one button; a double-tap is a double-click.

use drafftink_scene::MarkerColor;
use egui::{Align2, Area, Color32, Context, Id, Order, Response, vec2};
use serde_json::json;

use crate::icons::Icon;
use crate::state::{PanelKind, UiState};
use crate::theme::{MARGIN, space};
use crate::widgets::{Ctl, Cx, PANEL_INSET_Y, ToolFace, panel_frame, scroll_when_taller};
use drafftink_engine::state::Tool;

/// The tools, in the order of the old toolbar, with the additions (lasso, diamond, sticky note, frame).
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
    (Tool::Sticky, Icon::Sticky),
    (Tool::Math, Icon::Math),
    (Tool::Frame, Icon::Frame),
    (Tool::Eraser, Icon::Eraser),
    (Tool::Laser, Icon::Laser),
];

/// The second line of the tooltip of every tool button.
const KEEP_ACTIVE_HINT: &str = "Double-click to keep it active";

/// The name of the tools that have no key binding of `tool.set`, so that the command has no title for them.
fn unbound_name(tool: Tool) -> Option<&'static str> {
    match tool {
        Tool::Lasso => Some("Lasso tool"),
        Tool::Highlighter => Some("Highlighter tool"),
        Tool::Sticky => Some("Sticky note tool"),
        _ => None,
    }
}

/// The control of a tool button. The active Lasso button returns to Select (a toggle).
fn tool_ctl(tool: Tool, active: Tool) -> Ctl<'static> {
    let selected = active == tool;
    let go = if tool == Tool::Lasso && selected { Tool::Select } else { tool };
    let ctl = Ctl::new("tool.set").args(json!({ "tool": go.name() })).selected(selected).note(KEEP_ACTIVE_HINT);
    match unbound_name(tool) {
        Some(name) => ctl.label(name),
        None => ctl,
    }
}

/// One tool button, and what its clicks do.
fn tool_button(cx: &mut Cx<'_>, ui: &mut egui::Ui, tool: Tool, icon: Icon) {
    let ctl = tool_ctl(tool, cx.model.tool);
    let locked = cx.model.tool_lock && cx.model.tool == tool;
    let marker = (tool == Tool::Highlighter)
        .then(|| highlighter_bar(cx.model.setting_bool(COLOURFUL), cx.model.tool_marker, cx.pal.dark))
        .flatten();
    let resp = cx.tool_button(ui, &ctl, ToolFace::new(icon).locked(locked).marker(marker));
    clicked(cx, tool, &ctl, &resp);
}

/// The calls of a click, or of the second click of a double-click, on the button of `tool`.
fn clicked(cx: &mut Cx<'_>, tool: Tool, ctl: &Ctl<'_>, resp: &Response) {
    if resp.double_clicked() || resp.triple_clicked() {
        // The first click of the pair ran `tool.set`; the model has it by now unless the command was refused or the
        // click was the toggle that left the Lasso. Then the tool is set again, so the lock never lands on another.
        if cx.model.tool != tool {
            cx.run(ctl.id, json!({ "tool": tool.name() }));
        }
        cx.run("tool.lock", json!({ "on": true }));
    } else if resp.clicked() {
        cx.run(ctl.id, ctl.args.clone());
        if cx.model.tool_lock {
            cx.run("tool.lock", json!({ "on": false }));
        }
    }
}

/// The setting that makes the bar under the highlighter show its colour.
pub(crate) const COLOURFUL: &str = "ui.colourful_toolbar";

/// The colour of the bar under the highlighter icon: the ink of the marker on this theme when the toolbar is
/// colourful and a marker is chosen; `None` (the neutral grey of the icon) when it is not, and for "none".
pub(crate) fn highlighter_bar(colourful: bool, marker: Option<MarkerColor>, dark: bool) -> Option<Color32> {
    marker.filter(|_| colourful).map(|m| crate::colors::marker_offer(m, dark).shown)
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
                        tool_button(cx, ui, tool, icon);
                    }
                });
            });
        });
    st.chrome.tools = Some(area.response.rect);
}
