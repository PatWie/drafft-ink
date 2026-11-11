//! The menus that open from the ribbon: the properties that did not fit it ("More") and the heads of an arrow.
//! They are menu-kit popups, so they look and behave like every other menu: rows with check marks, a click
//! outside closes them, a greyed row says why.

use egui::{Align2, Context, Id, vec2};
use serde_json::{Value, json};

use super::controls::{CORNERS, HEADS, LINE_HEIGHTS, corner_ctl, has_path, new_arrows};
use super::shows;
use crate::chrome::GAP;
use crate::menu_kit::{Menu, Popup, Row, popup};
use crate::state::{OpenRibbonMenu, RibbonMenu, UiState};
use crate::widgets::{Ctl, Cx};

/// A property with a few named values, as a `style.set` choice.
struct Choice {
    heading: &'static str,
    prop: &'static str,
    options: &'static [(&'static str, &'static str)],
}

const CHOICES: [Choice; 3] = [
    Choice {
        heading: "Sloppiness",
        prop: "sloppiness",
        options: &[("Architect", "architect"), ("Artist", "artist"), ("Cartoonist", "cartoonist"), ("Drunk", "drunk")],
    },
    Choice {
        heading: "Pressure",
        prop: "pressure_mode",
        options: &[("Uniform", "uniform"), ("Simulated", "simulated"), ("Pen", "device")],
    },
    Choice {
        heading: "Vertical align",
        prop: "vertical_align",
        options: &[("Top", "top"), ("Middle", "middle"), ("Bottom", "bottom")],
    },
];

/// True when the "More" menu has something to offer: the groups show its button only then.
pub(super) fn has_more(cx: &Cx<'_>) -> bool {
    CHOICES.iter().any(|c| shows(cx, c.prop)) || shows(cx, "line_height") || box_available(cx) || corners_here(cx)
}

/// The corners are in the menu when the ribbon has a path group instead of a corners group.
fn corners_here(cx: &Cx<'_>) -> bool {
    shows(cx, "corner_radius") && has_path(cx)
}

/// The text box choices are for the selected text (`text.set_autoresize`).
fn box_available(cx: &Cx<'_>) -> bool {
    cx.model.selection_count > 0 && cx.model.style.has_kind("text") && cx.refusal("text.set_autoresize").is_none()
}

/// What the "More" menu offers now, and the current values.
struct MoreRows {
    /// The choices that apply, with the current value of each.
    choices: Vec<(&'static Choice, Option<String>)>,
    /// The line height presets, with the current height (`Some(None)`: offered, value unknown).
    line_height: Option<Option<f64>>,
    has_box: bool,
    /// The corner choices, with the control of each (their state is read from the model).
    corners: Vec<(&'static str, Ctl<'static>)>,
}

impl MoreRows {
    fn read(cx: &Cx<'_>) -> Self {
        Self {
            choices: CHOICES
                .iter()
                .filter(|c| shows(cx, c.prop))
                .map(|c| (c, cx.model.style.str_value(c.prop).map(str::to_owned)))
                .collect(),
            line_height: shows(cx, "line_height").then(|| cx.model.style.num_value("line_height")),
            has_box: box_available(cx),
            corners: if corners_here(cx) {
                CORNERS.iter().map(|&(name, value)| (name, corner_ctl(cx, name, value))).collect()
            } else {
                Vec::new()
            },
        }
    }

    fn draw(&self, menu: &mut Menu<'_, '_, '_>) {
        for (choice, current) in &self.choices {
            menu.heading(choice.heading);
            for (label, value) in choice.options {
                let ctl = Ctl::new("style.set").args(json!({ "props": { choice.prop: value } })).label(*label);
                menu.command(ctl, Row::new().label(label).checked(current.as_deref() == Some(*value)));
            }
        }
        if !self.corners.is_empty() {
            menu.heading("Corners");
            for (name, ctl) in &self.corners {
                menu.command(ctl.clone(), Row::new().label(name).checked(ctl.selected));
            }
        }
        if let Some(current) = self.line_height {
            menu.heading("Line height");
            for height in LINE_HEIGHTS {
                let ctl = Ctl::new("style.set")
                    .args(json!({ "props": { "line_height": height } }))
                    .label(format!("Line height {height}"));
                let on = current.is_some_and(|c| (c - height).abs() < 0.01);
                menu.command(ctl, Row::new().label(&format!("{height}")).checked(on));
            }
        }
        if self.has_box {
            menu.heading("Box");
            for (label, on) in [("Fit to text", true), ("Fixed width", false)] {
                let ctl = Ctl::new("text.set_autoresize").args(json!({ "on": on })).label(label);
                menu.command(ctl, Row::new().label(label));
            }
        }
    }
}

/// The heads of one end of an arrow, and swapping the ends of arrows that exist.
struct HeadRows {
    side: &'static str,
    /// `start` or `end`, as `arrow.set_head` takes it.
    end: &'static str,
    /// The argument of `tool.set_arrow`.
    arg: &'static str,
    /// New arrows are being set up: the choice is read from, and written to, the settings.
    new: bool,
    current: String,
}

impl HeadRows {
    fn read(cx: &Cx<'_>, kind: RibbonMenu) -> Self {
        let (side, end, arg, setting) = match kind {
            RibbonMenu::StartHead => ("Start", "start", "start_head", "arrow.start_head"),
            RibbonMenu::EndHead | RibbonMenu::More => ("End", "end", "end_head", "arrow.end_head"),
        };
        let current = cx.model.settings.get(setting).and_then(Value::as_str).unwrap_or_default().to_owned();
        Self { side, end, arg, new: new_arrows(cx), current }
    }

    fn draw(&self, menu: &mut Menu<'_, '_, '_>) {
        menu.heading(&format!("{} head", self.side));
        for (label, value) in HEADS {
            let ctl = if self.new {
                Ctl::new("tool.set_arrow")
                    .args(json!({ self.arg: value }))
                    .label(format!("New arrows, {} head: {label}", self.side))
            } else {
                let head = if value == "none" { Value::Null } else { json!(value) };
                Ctl::new("arrow.set_head")
                    .args(json!({ "end": self.end, "head": head }))
                    .label(format!("{} head: {label}", self.side))
            };
            menu.command(ctl, Row::new().label(label).checked(self.new && self.current == value));
        }
        if !self.new {
            menu.separator();
            menu.command(Ctl::new("arrow.swap_heads").label("Swap heads"), Row::new().label("Swap heads"));
        }
    }
}

/// Draws the open menu, if any, and closes it after a pick or a click outside.
pub(super) fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(OpenRibbonMenu { kind, anchor }) = st.ribbon_menu else { return };
    let label = match kind {
        RibbonMenu::More => "More options menu",
        RibbonMenu::StartHead => "Start head menu",
        RibbonMenu::EndHead => "End head menu",
    };
    let spec = Popup {
        key: Id::unique(("ribbon_menu", kind)),
        label,
        pivot: Align2::LEFT_TOP,
        at: anchor.left_bottom() + vec2(0.0, GAP),
    };
    let result = match kind {
        RibbonMenu::More => {
            let rows = MoreRows::read(cx);
            popup(ctx, cx, &spec, |menu| rows.draw(menu))
        }
        RibbonMenu::StartHead | RibbonMenu::EndHead => {
            let rows = HeadRows::read(cx, kind);
            popup(ctx, cx, &spec, |menu| rows.draw(menu))
        }
    };
    if result.picked.any() || result.outside {
        st.ribbon_menu = None;
    }
}
