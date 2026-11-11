//! Controls. Every control that does something is bound to a command id and its arguments: it asks
//! the model whether the command can run, greys out with the refusal text as its tooltip when it
//! cannot, and on a click pushes an action. Nothing here touches the session.

use drafftink_engine::Origin;
use egui::{
    Align2, Color32, CornerRadius, Frame, Margin, Painter, Rect, Response, Role, Sense, Shadow, Stroke, StrokeKind, Ui,
    Vec2, WidgetInfo, pos2, vec2,
};
use serde_json::{Value, json};

use crate::action::{HostRequest, UiAction, Verb};
use crate::icons::{self, Icon};
use crate::model::Model;
use crate::theme::{self, Control, Palette, TextSize, space};

/// How a control looks right now.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    /// The pointer is over it.
    pub hovered: bool,
    /// The primary button is down on it.
    pub pressed: bool,
    /// The command can run.
    pub enabled: bool,
    /// The control shows the current state (the active tool, a switched-on toggle).
    pub selected: bool,
}

/// The command a control runs, and how it is described.
#[derive(Debug, Clone)]
pub struct Ctl<'a> {
    /// Command id.
    pub id: &'a str,
    /// Arguments.
    pub args: Value,
    /// The accessible name and the first line of the tooltip. Empty: the title from the registry.
    pub label: String,
    /// Shows the current state.
    pub selected: bool,
    /// Greys the control for a reason the registry does not know (a property that does not apply to the
    /// selected elements). The registry's refusal text wins when both apply.
    pub blocked: Option<String>,
}

impl<'a> Ctl<'a> {
    /// A control for a command with no arguments.
    pub fn new(id: &'a str) -> Self {
        Self { id, args: json!({}), label: String::new(), selected: false, blocked: None }
    }

    /// With arguments.
    #[must_use]
    pub fn args(mut self, args: Value) -> Self {
        self.args = args;
        self
    }

    /// With a name.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Greys the control with this reason when `reason` is `Some`.
    #[must_use]
    pub fn blocked(mut self, reason: Option<String>) -> Self {
        self.blocked = reason;
        self
    }

    /// Marks the control as showing the current state.
    #[must_use]
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

/// How the surfaces are laid out: sizes of the targets and where panels sit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The touch layout: larger targets, collapsible panels.
    pub compact: bool,
    /// The window is narrow, so the touch layout also moves the bars and starts panels collapsed.
    pub narrow: bool,
    /// The window size in points.
    pub screen: Vec2,
}

impl Default for Layout {
    fn default() -> Self {
        Self { compact: false, narrow: false, screen: vec2(1280.0, 800.0) }
    }
}

impl Layout {
    /// The height (and, for an icon button, the side) of a control in this layout.
    pub fn control(&self, control: Control) -> f32 {
        control.points(self.compact)
    }

    /// Side of a tool button.
    pub fn tool_side(&self) -> f32 {
        self.control(Control::Tool)
    }

    /// Side of a small icon button.
    pub fn small_side(&self) -> f32 {
        self.control(Control::Small)
    }

    /// Height of a menu row.
    pub fn row_height(&self) -> f32 {
        self.control(Control::Small)
    }

    /// Height of a small toggle.
    pub fn toggle_height(&self) -> f32 {
        self.control(Control::Toggle)
    }

    /// Height of a dialog button.
    pub fn button_height(&self) -> f32 {
        self.control(Control::Button)
    }

    /// Height of the top strip: one row of small controls with padding above and below.
    pub fn strip_height(&self) -> f32 {
        self.control(Control::Small) + 2.0 * space::MD
    }

    /// Width of the right panel.
    pub fn panel_width(&self) -> f32 {
        if !self.compact {
            theme::RIGHT_PANEL_WIDTH
        } else if self.narrow {
            (self.screen.x * 0.6).clamp(180.0, 260.0)
        } else {
            260.0
        }
    }
}

/// What the surfaces draw with: the model to read, the palette, and the list that collects actions.
pub struct Cx<'a> {
    /// The model.
    pub model: &'a Model,
    /// Colours.
    pub pal: Palette,
    /// Which surface the actions come from.
    pub origin: Origin,
    /// Sizes and positions for this window.
    pub layout: Layout,
    out: &'a mut Vec<UiAction>,
}

impl<'a> Cx<'a> {
    /// A drawing context over an action list.
    pub fn new(model: &'a Model, pal: Palette, out: &'a mut Vec<UiAction>) -> Self {
        Self { model, pal, origin: Origin::Ui, layout: Layout::default(), out }
    }

    /// The same context with a layout.
    #[must_use]
    pub fn with_layout(mut self, layout: Layout) -> Self {
        self.layout = layout;
        self
    }

    /// Adds an action.
    pub fn emit(&mut self, action: UiAction) {
        self.out.push(action);
    }

    /// Adds a command call with the current origin.
    pub fn run(&mut self, id: &str, args: Value) {
        let args = crate::surfaces::export::complete_args(self.model, id, args);
        self.out.push(UiAction::run_from(self.origin, id, args));
    }

    /// Adds a UI verb.
    pub fn verb(&mut self, v: Verb) {
        self.out.push(UiAction::Verb(v));
    }

    /// Runs `f` with another origin.
    pub fn with_origin<R>(&mut self, origin: Origin, f: impl FnOnce(&mut Self) -> R) -> R {
        let before = std::mem::replace(&mut self.origin, origin);
        let r = f(self);
        self.origin = before;
        r
    }

    /// The refusal text of a command, `None` when it can run.
    pub fn refusal(&self, id: &str) -> Option<String> {
        self.model.refusal_text(id)
    }

    /// The core of every command control: allocate, paint, describe, gate, and push the call on a click.
    pub fn command_widget(
        &mut self,
        ui: &mut Ui,
        ctl: &Ctl<'_>,
        size: Vec2,
        paint: impl FnOnce(&Painter, Rect, Look),
    ) -> Response {
        // While a text edit runs, a style choice goes to the text editor, which applies it to the selected range
        // or to the element and hands the result to `text.edit_end` (docs/exceptions.md E16). `style.set`
        // itself would refuse with `text_editing`, so the control must not grey out for that.
        let editor_takes_it =
            self.model.text_editing && self.model.command(ctl.id).is_some_and(|c| c.host.text_editor_takes_style);
        let reason =
            if editor_takes_it { ctl.blocked.clone() } else { self.refusal(ctl.id).or_else(|| ctl.blocked.clone()) };
        let enabled = reason.is_none();
        let label = if ctl.label.is_empty() { self.model.label(ctl.id, &ctl.args) } else { ctl.label.clone() };
        let resp = ui
            .add_enabled_ui(enabled, |ui| {
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                let look = Look {
                    hovered: resp.hovered() || resp.has_focus(),
                    pressed: resp.is_pointer_button_down_on(),
                    enabled,
                    selected: ctl.selected,
                };
                if ui.is_rect_visible(rect) {
                    paint(ui.painter(), rect, look);
                }
                resp.widget_info(|| {
                    let mut info = WidgetInfo::labeled(Role::Button, enabled, label.as_str());
                    info.selected = Some(ctl.selected);
                    info
                });
                resp
            })
            .inner;
        let resp = match reason {
            Some(r) => resp.on_disabled_hover_text(r),
            None => {
                let tip = if ctl.label.is_empty() {
                    self.model.tooltip(ctl.id, &ctl.args)
                } else {
                    match self.model.shortcut_label(ctl.id, &ctl.args) {
                        Some(k) => format!("{} ({k})", ctl.label),
                        None => ctl.label.clone(),
                    }
                };
                resp.on_hover_text(tip)
            }
        };
        if resp.clicked() {
            if editor_takes_it {
                // `style.set` carries its properties under `props`; `text.format` has bold and italic as they are.
                let request = match ctl.args.get("props") {
                    Some(props) => HostRequest::TextEditStyle(props.clone()),
                    None => HostRequest::TextEditFormat(ctl.args.clone()),
                };
                self.emit(UiAction::Host(request));
            } else {
                self.run(ctl.id, ctl.args.clone());
            }
        }
        resp
    }

    /// A square icon button; the selected one is filled with the accent colour (tools).
    pub fn icon_button(&mut self, ui: &mut Ui, ctl: &Ctl<'_>, icon: Icon, side: f32) -> Response {
        let pal = self.pal;
        self.command_widget(ui, ctl, vec2(side, side), move |p, rect, look| {
            paint_button_bg(p, rect, look, &pal, true);
            let color = icon_color(look, &pal);
            icons::paint(p, rect.shrink(side * 0.2), icon, color);
        })
    }

    /// The big tool button.
    pub fn tool_button(&mut self, ui: &mut Ui, ctl: &Ctl<'_>, icon: Icon) -> Response {
        self.icon_button(ui, ctl, icon, self.layout.tool_side())
    }

    /// A small text toggle for a property value (`Architect`, `Dashed`): filled with the accent when selected.
    pub fn toggle_button(&mut self, ui: &mut Ui, ctl: &Ctl<'_>, text: &str) -> Response {
        self.toggle_button_font(ui, ctl, text, TextSize::Caption.font())
    }

    /// Like [`Cx::toggle_button`] with a font, for the font family choices that show their own face.
    pub fn toggle_button_font(&mut self, ui: &mut Ui, ctl: &Ctl<'_>, text: &str, font: egui::FontId) -> Response {
        let pal = self.pal;
        let galley = ui.painter().layout_no_wrap(text.to_owned(), font, pal.text);
        let size = vec2((galley.size().x + 16.0).max(34.0), self.layout.toggle_height());
        let ctl = Ctl { label: if ctl.label.is_empty() { text.to_owned() } else { ctl.label.clone() }, ..ctl.clone() };
        self.command_widget(ui, &ctl, size, move |p, rect, look| {
            paint_button_bg(p, rect, look, &pal, false);
            p.galley(
                pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
                galley,
                text_color(look, &pal),
            );
        })
    }

    /// A small text toggle that runs no command: it changes a choice inside a dialog (the export format).
    /// `reason` greys it with that tooltip.
    pub fn choice(&mut self, ui: &mut Ui, text: &str, selected: bool, reason: Option<&str>) -> Response {
        let pal = self.pal;
        let galley = ui.painter().layout_no_wrap(text.to_owned(), TextSize::Caption.font(), pal.text);
        let size = vec2((galley.size().x + 16.0).max(34.0), self.layout.toggle_height());
        let enabled = reason.is_none();
        let resp = ui
            .add_enabled_ui(enabled, |ui| {
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                let look = Look {
                    hovered: resp.hovered() || resp.has_focus(),
                    pressed: resp.is_pointer_button_down_on(),
                    enabled,
                    selected,
                };
                if ui.is_rect_visible(rect) {
                    paint_button_bg(ui.painter(), rect, look, &pal, false);
                    ui.painter().galley(
                        pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
                        galley,
                        text_color(look, &pal),
                    );
                }
                resp.widget_info(|| {
                    let mut info = WidgetInfo::labeled(Role::Button, enabled, text);
                    info.selected = Some(selected);
                    info
                });
                resp
            })
            .inner;
        match reason {
            Some(r) => resp.on_disabled_hover_text(r),
            None => resp,
        }
    }

    /// A button with a text label and an icon-less look, used for dialog actions that run commands.
    pub fn text_button(&mut self, ui: &mut Ui, ctl: &Ctl<'_>, text: &str, primary: bool) -> Response {
        let pal = self.pal;
        let font = TextSize::Body.font();
        let galley = ui.painter().layout_no_wrap(text.to_owned(), font, pal.text);
        let size = vec2((galley.size().x + 28.0).max(80.0), self.layout.button_height());
        let ctl = Ctl { label: if ctl.label.is_empty() { text.to_owned() } else { ctl.label.clone() }, ..ctl.clone() };
        self.command_widget(ui, &ctl, size, move |p, rect, look| {
            let fill = if primary {
                if look.hovered { pal.accent.gamma_multiply(0.9) } else { pal.accent }
            } else if look.hovered {
                pal.hover
            } else {
                pal.button
            };
            p.rect_filled(rect, theme::BUTTON_RADIUS, if look.enabled { fill } else { fill.gamma_multiply(0.5) });
            let c = if primary { pal.on_accent } else { pal.text };
            let c = if look.enabled { c } else { c.gamma_multiply(0.5) };
            p.galley(pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0), galley, c);
        })
    }

    /// A colour swatch that runs a command.
    pub fn swatch(&mut self, ui: &mut Ui, ctl: &Ctl<'_>, color: Option<Color32>, diameter: f32) -> Response {
        let pal = self.pal;
        self.command_widget(ui, ctl, vec2(diameter + 4.0, diameter + 4.0), move |p, rect, look| {
            paint_swatch(p, rect.center(), diameter / 2.0, color, look.selected, look.hovered, &pal, look.enabled);
        })
    }

    /// A row of a menu: label left, shortcut right.
    pub fn menu_item(&mut self, ui: &mut Ui, ctl: &Ctl<'_>, text: &str) -> Response {
        let pal = self.pal;
        let shortcut = self.model.shortcut_label(ctl.id, &ctl.args);
        let width = ui.available_width().max(160.0);
        let ctl = Ctl { label: if ctl.label.is_empty() { text.to_owned() } else { ctl.label.clone() }, ..ctl.clone() };
        let text = text.to_owned();
        self.command_widget(ui, &ctl, vec2(width, self.layout.row_height()), move |p, rect, look| {
            if look.hovered && look.enabled {
                p.rect_filled(rect, theme::BUTTON_RADIUS, pal.hover);
            }
            let c = if look.enabled { pal.text } else { pal.muted.gamma_multiply(0.6) };
            p.text(pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, &text, TextSize::Body.font(), c);
            if let Some(k) = shortcut {
                p.text(
                    pos2(rect.right() - 10.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    k,
                    TextSize::Caption.font(),
                    pal.muted,
                );
            }
        })
    }

    /// A menu row that applies a UI verb. The key is the verb's own key from the keymap, if it has one.
    pub fn verb_item(&mut self, ui: &mut Ui, verb: Verb, text: &str) -> Response {
        self.verb_item_gated(ui, verb, text, None)
    }

    /// Like [`Cx::verb_item`], greyed with `reason` as its tooltip when `reason` is `Some`. The reason is the text
    /// of the refusal of the command the verb leads to; a greyed row does nothing.
    pub fn verb_item_gated(&mut self, ui: &mut Ui, verb: Verb, text: &str, reason: Option<String>) -> Response {
        let pal = self.pal;
        let enabled = reason.is_none();
        let key = crate::keymap::verb_key_label(verb, self.model.mac);
        let row_height = self.layout.row_height();
        let resp = ui
            .add_enabled_ui(enabled, |ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width().max(160.0), row_height), Sense::click());
                if ui.is_rect_visible(rect) {
                    if resp.hovered() && enabled {
                        ui.painter().rect_filled(rect, theme::BUTTON_RADIUS, pal.hover);
                    }
                    let color = if enabled { pal.text } else { pal.muted.gamma_multiply(0.6) };
                    ui.painter().text(
                        pos2(rect.left() + 10.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        text,
                        TextSize::Body.font(),
                        color,
                    );
                    if let Some(k) = key {
                        ui.painter().text(
                            pos2(rect.right() - 10.0, rect.center().y),
                            Align2::RIGHT_CENTER,
                            k,
                            TextSize::Caption.font(),
                            pal.muted,
                        );
                    }
                }
                resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, text));
                resp
            })
            .inner;
        match reason {
            Some(r) => resp.on_disabled_hover_text(r),
            None => {
                if resp.clicked() {
                    self.verb(verb);
                }
                resp
            }
        }
    }

    /// A menu row that only changes what the UI shows (opens a block of rows, opens a dialog). It is not a
    /// command and is never greyed.
    pub fn plain_row(&mut self, ui: &mut Ui, text: &str) -> Response {
        let pal = self.pal;
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width().max(160.0), self.layout.row_height()), Sense::click());
        if ui.is_rect_visible(rect) {
            if resp.hovered() || resp.has_focus() {
                ui.painter().rect_filled(rect, theme::BUTTON_RADIUS, pal.hover);
            }
            ui.painter().text(
                pos2(rect.left() + 10.0, rect.center().y),
                Align2::LEFT_CENTER,
                text,
                TextSize::Body.font(),
                pal.text,
            );
        }
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, text));
        resp
    }

    /// An icon button that only changes what the UI shows and is not a verb of the catalog (collapsing a panel
    /// of the touch layout). The caller reads `clicked()` and changes the UI state.
    pub fn local_button(&mut self, ui: &mut Ui, icon: Icon, side: f32, label: &str, selected: bool) -> Response {
        let pal = self.pal;
        let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
        let look = Look {
            hovered: resp.hovered() || resp.has_focus(),
            pressed: resp.is_pointer_button_down_on(),
            enabled: true,
            selected,
        };
        if ui.is_rect_visible(rect) {
            paint_button_bg(ui.painter(), rect, look, &pal, true);
            icons::paint(ui.painter(), rect.shrink(side * 0.2), icon, icon_color(look, &pal));
        }
        resp.widget_info(|| {
            let mut info = WidgetInfo::labeled(Role::Button, true, label);
            info.selected = Some(selected);
            info
        });
        resp.on_hover_text(label)
    }

    /// A control that opens something in the UI (no command, never greyed).
    pub fn verb_button(
        &mut self,
        ui: &mut Ui,
        verb: Verb,
        icon: Icon,
        side: f32,
        tip: &str,
        selected: bool,
    ) -> Response {
        self.verb_button_gated(ui, verb, icon, side, tip, selected, None)
    }

    /// Like [`Cx::verb_button`], greyed with `reason` as its tooltip when `reason` is `Some`: the text of the
    /// refusal of the command the control leads to. A greyed control does nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn verb_button_gated(
        &mut self,
        ui: &mut Ui,
        verb: Verb,
        icon: Icon,
        side: f32,
        tip: &str,
        selected: bool,
        reason: Option<String>,
    ) -> Response {
        let pal = self.pal;
        let enabled = reason.is_none();
        let resp = ui
            .add_enabled_ui(enabled, |ui| {
                let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
                let look = Look {
                    hovered: resp.hovered() || resp.has_focus(),
                    pressed: resp.is_pointer_button_down_on(),
                    enabled,
                    selected,
                };
                if ui.is_rect_visible(rect) {
                    paint_button_bg(ui.painter(), rect, look, &pal, true);
                    icons::paint(ui.painter(), rect.shrink(side * 0.2), icon, icon_color(look, &pal));
                }
                resp.widget_info(|| {
                    let mut info = WidgetInfo::labeled(Role::Button, enabled, verb.title());
                    info.selected = Some(selected);
                    info
                });
                resp
            })
            .inner;
        match reason {
            Some(r) => resp.on_disabled_hover_text(r),
            None => {
                let resp = resp.on_hover_text(tip);
                if resp.clicked() {
                    self.verb(verb);
                }
                resp
            }
        }
    }
}

/// The background of a button.
/// `flat` buttons (icons) have no fill at rest; the others (text toggles) rest on a light fill.
pub fn paint_button_bg(p: &Painter, rect: Rect, look: Look, pal: &Palette, flat: bool) {
    let r = CornerRadius::same(theme::BUTTON_RADIUS);
    if look.selected {
        p.rect_filled(rect, r, if look.enabled { pal.accent } else { pal.accent.gamma_multiply(0.4) });
    } else if look.enabled && (look.hovered || look.pressed) {
        p.rect_filled(rect, r, if look.pressed { pal.border } else { pal.hover });
    } else if !flat {
        p.rect_filled(rect, r, if look.enabled { pal.button } else { pal.button.gamma_multiply(0.6) });
    }
}

/// The colour of an icon.
pub fn icon_color(look: Look, pal: &Palette) -> Color32 {
    if !look.enabled {
        pal.muted.gamma_multiply(0.45)
    } else if look.selected {
        pal.on_accent
    } else {
        pal.text
    }
}

fn text_color(look: Look, pal: &Palette) -> Color32 {
    icon_color(look, pal)
}

/// A colour swatch: a filled circle, a ring when selected, the slashed circle for "no colour".
#[allow(clippy::too_many_arguments)]
pub fn paint_swatch(
    p: &Painter,
    center: egui::Pos2,
    radius: f32,
    color: Option<Color32>,
    selected: bool,
    hovered: bool,
    pal: &Palette,
    enabled: bool,
) {
    match color {
        Some(c) => {
            let c = if enabled { c } else { c.gamma_multiply(0.4) };
            p.circle_filled(center, radius, c);
            p.circle_stroke(center, radius, Stroke::new(1.0, pal.border));
        }
        None => {
            p.circle_filled(center, radius, if pal.dark { Color32::from_gray(20) } else { Color32::WHITE });
            p.circle_stroke(center, radius, Stroke::new(1.0, pal.border));
            let d = radius * 0.7;
            p.line_segment([center + vec2(-d, d), center + vec2(d, -d)], Stroke::new(1.5, pal.danger));
        }
    }
    if selected {
        p.circle_stroke(center, radius + 2.5, Stroke::new(2.0, pal.text));
    } else if hovered && enabled {
        p.circle_stroke(center, radius + 1.5, Stroke::new(1.5, pal.muted));
    }
}

/// The vertical padding inside a panel frame.
pub const PANEL_INSET_Y: f32 = 8.0;

/// The frame of a floating panel: the old app's soft white card, in the current theme.
pub fn panel_frame(pal: &Palette) -> Frame {
    Frame::new()
        .fill(pal.panel)
        .stroke(pal.border_stroke())
        .corner_radius(CornerRadius::same(theme::PANEL_RADIUS))
        .inner_margin(Margin::symmetric(10, PANEL_INSET_Y as i8))
        .shadow(Shadow { offset: [0, 2], blur: 8, spread: 0, color: pal.shadow })
}

/// The frame of a dialog.
pub fn dialog_frame(pal: &Palette) -> Frame {
    Frame::new()
        .fill(pal.panel)
        .stroke(pal.border_stroke())
        .corner_radius(CornerRadius::same(theme::DIALOG_RADIUS))
        .inner_margin(Margin::same(20))
        .shadow(Shadow { offset: [0, 6], blur: 24, spread: 0, color: pal.shadow })
}

/// A small caption above a group of controls, in the hand-written font.
pub fn section_label(ui: &mut Ui, pal: &Palette, text: &str) -> Response {
    ui.label(egui::RichText::new(text).font(TextSize::Body.hand()).color(pal.muted))
}

/// A thin vertical line between groups in a row.
pub fn vertical_separator(ui: &mut Ui, pal: &Palette, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
    ui.painter().line_segment([rect.center_top(), rect.center_bottom()], Stroke::new(1.0, pal.separator));
}

/// A thin horizontal line.
pub fn separator(ui: &mut Ui, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment([rect.left_center(), rect.right_center()], Stroke::new(1.0, pal.separator));
}

/// A rectangle outline used by the tab bar and the property rows.
pub fn outline(p: &Painter, rect: Rect, pal: &Palette) {
    p.rect_stroke(rect, theme::BUTTON_RADIUS, Stroke::new(1.0, pal.border), StrokeKind::Inside);
}

/// Which way a [`scroll_if`] area scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    /// Up and down, limited in height.
    Vertical,
    /// Left and right, limited in width by the caller.
    Horizontal,
}

/// Runs `add` in a column that scrolls when it is taller than `max_height`, and plainly when it fits. The height
/// is measured in a hidden sizing pass first, so the answer is right in the first frame (a scroll area alone
/// sizes itself from the frame before).
pub fn scroll_when_taller(ui: &mut Ui, id: &'static str, max_height: f32, mut add: impl FnMut(&mut Ui)) {
    let bounds = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), f32::INFINITY));
    let mut probe =
        ui.new_child(egui::UiBuilder::new().id_salt("scroll_probe").max_rect(bounds).sizing_pass().invisible());
    add(&mut probe);
    if probe.min_rect().height() <= max_height {
        add(ui);
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(max_height)
        .min_scrolled_height(max_height)
        .auto_shrink([true, false])
        .show(ui, add);
}

/// Runs `add` inside a scroll area when `on` (a touch or narrow layout that may not have room), else
/// directly: a panel that fits is drawn plainly.
pub fn scroll_if(ui: &mut Ui, on: bool, dir: ScrollDir, id: &'static str, max_height: f32, add: impl FnOnce(&mut Ui)) {
    if !on {
        add(ui);
        return;
    }
    let area = match dir {
        ScrollDir::Vertical => egui::ScrollArea::vertical().max_height(max_height),
        ScrollDir::Horizontal => egui::ScrollArea::horizontal(),
    };
    area.id_salt(id).auto_shrink([true, true]).show(ui, add);
}
