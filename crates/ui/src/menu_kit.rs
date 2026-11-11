//! The menu kit: one row painter, one width and one flyout for every menu of the UI (the context menu, the
//! hamburger menu, the tab menu).
//!
//! A menu is written as a list of calls on [`Menu`]: [`Menu::command`], [`Menu::verb`], [`Menu::host`],
//! [`Menu::local`], [`Menu::submenu`], [`Menu::separator`] and [`Menu::heading`]. A row has a fixed height from
//! the layout (taller on touch screens), a 16 px icon slot (which also holds a check mark), the label, the
//! shortcut dimmed on the right, and a chevron when it opens a flyout. A row that cannot run is greyed and its
//! tooltip is the refusal text.
//!
//! Flyouts open to the right of their row, flip to the left at the screen edge and stay on screen. Leaving the
//! row for another row of the menu closes the flyout only after [`CLOSE_DELAY_S`], so a pointer that crosses a
//! neighbouring row on its way to the flyout does not lose it.
//!
//! The keyboard works through egui's focus: Up and Down move between rows, Right (or Enter) opens a flyout and
//! moves into it, Left closes it, Enter runs the row and Esc closes the menu (egui clears the focus first, then
//! the keymap sees the key).

use egui::{
    Align2, Area, Context, FontId, Id, Key, Order, Painter, Pos2, Rect, Response, Role, Sense, Stroke, Ui, UiBuilder,
    Vec2, WidgetInfo, pos2, vec2,
};

use crate::action::{HostRequest, UiAction, Verb};
use crate::icons::{self, Icon};
use crate::theme::{self, MARGIN, Palette};
use crate::widgets::{Ctl, Cx, panel_frame};

/// Width of the rows of every menu, inside the frame.
pub(crate) const MENU_WIDTH: f32 = 232.0;

/// Seconds the pointer may rest on another row of the menu before an open flyout closes.
const CLOSE_DELAY_S: f64 = 0.25;
/// What the frame of a menu adds around its rows: the inner margin (10 and 8) and the border.
const FRAME_PAD: Vec2 = vec2(11.0, 9.0);
/// Distance of a row's content from the left and right edge of the row.
pub(crate) const ROW_PAD: f32 = 8.0;
/// Side of the icon slot.
const ICON_SLOT: f32 = 16.0;
/// Space between the icon slot and the label.
const ICON_GAP: f32 = 8.0;
/// How far a flyout reaches over the frame of its parent, so the pointer never crosses a gap.
const FLYOUT_OVERLAP: f32 = 3.0;
/// The least distance of a flyout from the window edge.
const SCREEN_GAP: f32 = MARGIN;
/// Height of a section heading.
const HEADING_HEIGHT: f32 = 20.0;
/// Space above a heading that is not the first.
const HEADING_GAP: f32 = 6.0;
/// Height of a separator, which is a hairline with room around it.
const SEPARATOR_HEIGHT: f32 = 9.0;

/// Which rows were chosen. The menu closes on any of them; the hamburger menu treats the two kinds apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Picked {
    /// A command was called (the call is in the action list).
    pub command: bool,
    /// A UI verb, a request for the app or a purely local action ran.
    pub local: bool,
}

impl Picked {
    /// True when any row was chosen.
    pub(crate) fn any(self) -> bool {
        self.command || self.local
    }

    fn merge(&mut self, other: Picked) {
        self.command |= other.command;
        self.local |= other.local;
    }
}

/// How one row looks, besides what it does.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Row<'a> {
    icon: Option<Icon>,
    label: Option<&'a str>,
    check: Option<bool>,
    dialog: bool,
}

impl<'a> Row<'a> {
    /// A row with the label of its command and no icon.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// With an icon in the slot at the left.
    #[must_use]
    pub(crate) fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// With this label instead of the title from the registry (for rows whose call has arguments that no title
    /// names, and for rows that are not commands).
    #[must_use]
    pub(crate) fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }

    /// A toggle: the slot shows a check mark when `on`.
    #[must_use]
    pub(crate) fn checked(mut self, on: bool) -> Self {
        self.check = Some(on);
        self
    }

    /// The row opens a dialog: its label ends in an ellipsis.
    #[must_use]
    pub(crate) fn dialog(mut self) -> Self {
        self.dialog = true;
        self
    }

    fn text(&self, fallback: &str) -> String {
        let base = self.label.unwrap_or(fallback);
        if self.dialog && !base.ends_with('…') { format!("{base}…") } else { base.to_owned() }
    }
}

/// What the painter needs of a row.
struct Face<'a> {
    label: &'a str,
    icon: Option<Icon>,
    check: Option<bool>,
    shortcut: Option<&'a str>,
    submenu: bool,
}

fn paint_face(p: &Painter, rect: Rect, pal: &Palette, face: &Face<'_>, hot: bool, enabled: bool) {
    if hot && enabled {
        p.rect_filled(rect, theme::BUTTON_RADIUS, pal.hover);
    }
    let ink = if enabled { pal.text } else { pal.muted.gamma_multiply(0.6) };
    let dim = if enabled { pal.muted } else { pal.muted.gamma_multiply(0.5) };
    let slot =
        Rect::from_center_size(pos2(rect.left() + ROW_PAD + ICON_SLOT / 2.0, rect.center().y), Vec2::splat(ICON_SLOT));
    let slot_icon = match face.check {
        Some(true) => Some(Icon::Check),
        Some(false) => None,
        None => face.icon,
    };
    if let Some(icon) = slot_icon {
        icons::paint(p, slot.shrink(1.0), icon, if enabled { pal.text } else { ink });
    }
    let mut right = rect.right() - ROW_PAD;
    if face.submenu {
        icons::paint(
            p,
            Rect::from_center_size(pos2(right - 5.0, rect.center().y), Vec2::splat(12.0)),
            Icon::ChevronRight,
            dim,
        );
        right -= 14.0;
    }
    if let Some(key) = face.shortcut {
        let galley = p.layout_no_wrap(key.to_owned(), FontId::proportional(11.0), dim);
        let at = pos2(right - galley.size().x, rect.center().y - galley.size().y / 2.0);
        right = at.x - ICON_GAP;
        p.galley(at, galley, dim);
    }
    let left = rect.left() + ROW_PAD + ICON_SLOT + ICON_GAP;
    let mut text_rect = rect;
    text_rect.min.x = left;
    text_rect.max.x = right.max(left);
    p.with_clip_rect(text_rect).text(
        pos2(left, rect.center().y),
        Align2::LEFT_CENTER,
        face.label,
        FontId::proportional(13.0),
        ink,
    );
}

/// The size of a row in `ui`.
fn row_size(ui: &Ui, cx: &Cx<'_>) -> Vec2 {
    vec2(ui.available_width().max(MENU_WIDTH.min(160.0)), cx.layout.row_height())
}

/// A row that runs a command, drawn with the kit's painter. Greyed with the refusal text while the command
/// cannot run; the call is made with the origin of `cx`.
pub(crate) fn command_row(cx: &mut Cx<'_>, ui: &mut Ui, ctl: &Ctl<'_>, row: &Row<'_>) -> Response {
    let fallback = if ctl.label.is_empty() { cx.model.label(ctl.id, &ctl.args) } else { ctl.label.clone() };
    let shown = row.text(&fallback);
    let shortcut = cx.model.shortcut_label(ctl.id, &ctl.args);
    // The accessible name is the control's own when it has one (a long path shown by its file name).
    let name = if ctl.label.is_empty() { shown.clone() } else { ctl.label.clone() };
    let ctl = Ctl { label: name, selected: row.check.unwrap_or(ctl.selected), ..ctl.clone() };
    let pal = cx.pal;
    let (icon, check) = (row.icon, row.check);
    let size = row_size(ui, cx);
    cx.command_widget_quiet(ui, &ctl, size, move |p, rect, look| {
        let face = Face { label: &shown, icon, check, shortcut: shortcut.as_deref(), submenu: false };
        paint_face(p, rect, &pal, &face, look.hovered, look.enabled);
    })
}

/// A row that is no command: it allocates, paints, describes and gates itself. `reason` greys it.
fn plain_row(cx: &Cx<'_>, ui: &mut Ui, face: &Face<'_>, reason: Option<String>, highlighted: bool) -> Response {
    let pal = cx.pal;
    let enabled = reason.is_none();
    let size = row_size(ui, cx);
    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            if ui.is_rect_visible(rect) {
                let hot = resp.hovered() || resp.has_focus() || highlighted;
                paint_face(ui.painter(), rect, &pal, face, hot, enabled);
            }
            resp.widget_info(|| {
                let mut info = WidgetInfo::labeled(Role::Button, enabled, face.label);
                info.selected = face.check;
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

/// The flyout of a menu that is open, and since when the pointer rests on another row.
#[derive(Debug, Clone, Copy, Default)]
struct Flyouts {
    /// The row whose flyout is open.
    open: Option<Id>,
    /// When the pointer started resting on another row of the menu.
    leaving_since: Option<f64>,
    /// The flyout was opened with the keyboard: its first row takes the focus.
    focus_first: bool,
}

/// Where a menu sits in the tree of flyouts.
#[derive(Debug, Clone, Copy)]
struct Level {
    /// Names the menu's state.
    id: Id,
    /// The row of the parent menu that opened this one, and that menu's level.
    parent: Option<(Id, Id)>,
}

fn flyouts_key(level: Id) -> Id {
    level.with("flyouts")
}

/// A row the keyboard can reach.
#[derive(Debug, Clone, Copy)]
struct Stop {
    id: Id,
    enabled: bool,
    submenu: bool,
}

/// The rows of one menu (or flyout), drawn into a [`Ui`].
pub(crate) struct Menu<'u, 'c, 'a> {
    ui: &'u mut Ui,
    cx: &'c mut Cx<'a>,
    level: Level,
    /// The frame around the rows, used to place flyouts.
    frame: Rect,
    picked: Picked,
    stops: Vec<Stop>,
    /// The row the pointer is on.
    pointer_row: Option<Id>,
    /// Wanted before the next row: a separator, and a heading.
    pending_separator: bool,
    pending_heading: Option<String>,
    drawn_any: bool,
}

impl<'u, 'c, 'a> Menu<'u, 'c, 'a> {
    fn new(ui: &'u mut Ui, cx: &'c mut Cx<'a>, level: Level, frame: Rect) -> Self {
        ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
        Self {
            ui,
            cx,
            level,
            frame,
            picked: Picked::default(),
            stops: Vec::new(),
            pointer_row: None,
            pending_separator: false,
            pending_heading: None,
            drawn_any: false,
        }
    }

    /// Draws a menu whose frame the caller made (the hamburger menu): scrolls when the rows do not fit the window.
    /// `label` names the menu for screen readers and tests.
    pub(crate) fn run_in(
        ui: &mut Ui,
        cx: &mut Cx<'_>,
        key: Id,
        label: &str,
        add: impl FnOnce(&mut Menu<'_, '_, '_>),
    ) -> Picked {
        let level = Level { id: key, parent: None };
        let frame = ui.min_rect().expand2(FRAME_PAD);
        let max_height = (cx.layout.screen.y - 2.0 * SCREEN_GAP - 2.0 * FRAME_PAD.y - 60.0).max(160.0);
        let builder = UiBuilder::new().accessibility_role(Role::Menu).accessibility_label(label);
        ui.scope_builder(builder, |ui| {
            // An area measures its content with a default height of 400 points; the menu is as tall as the window
            // allows.
            ui.set_max_height(max_height);
            let mut picked = Picked::default();
            egui::ScrollArea::vertical()
                .id_salt(key.with("rows"))
                .max_height(max_height)
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    let mut menu = Menu::new(ui, cx, level, frame);
                    add(&mut menu);
                    picked = menu.finish();
                });
            picked
        })
        .inner
    }

    /// The refusal text of a command, `None` when it can run.
    pub(crate) fn refusal(&self, id: &str) -> Option<String> {
        self.cx.refusal(id)
    }

    /// Asks for a separator before the next row. Several in a row, and one before the first row, draw nothing.
    pub(crate) fn separator(&mut self) {
        if self.drawn_any {
            self.pending_separator = true;
        }
    }

    /// Asks for a heading before the next row. A heading with no row after it draws nothing.
    pub(crate) fn heading(&mut self, text: &str) {
        self.pending_heading = Some(text.to_owned());
    }

    /// Draws the separator and heading asked for since the last row. A heading starts its group by itself, so it
    /// comes with a gap instead of a line.
    fn flush(&mut self) {
        let pal = self.cx.pal;
        let heading = self.pending_heading.take();
        let width = self.ui.available_width();
        if let Some(text) = heading {
            let gap = if self.drawn_any { HEADING_GAP } else { 0.0 };
            let (rect, resp) = self.ui.allocate_exact_size(vec2(width, gap + HEADING_HEIGHT), Sense::hover());
            resp.widget_info(|| WidgetInfo::labeled(Role::Heading, true, text.as_str()));
            self.ui.painter().text(
                pos2(rect.left() + ROW_PAD + 2.0, rect.bottom() - HEADING_HEIGHT / 2.0),
                Align2::LEFT_CENTER,
                text,
                theme::hand(13.0),
                pal.muted,
            );
        } else if self.pending_separator {
            let (rect, _) = self.ui.allocate_exact_size(vec2(width, SEPARATOR_HEIGHT), Sense::hover());
            self.ui.painter().line_segment([rect.left_center(), rect.right_center()], Stroke::new(1.0, pal.separator));
        }
        self.pending_separator = false;
        self.drawn_any = true;
    }

    /// Notes a drawn row for the pointer and the keyboard.
    fn track(&mut self, resp: &Response, submenu: bool) {
        let over = self.ui.ctx().rect_contains_pointer(self.ui.layer_id(), resp.rect);
        if over {
            self.pointer_row = Some(resp.id);
        }
        if resp.has_focus() {
            // The row takes the arrow keys itself, so egui does not move the focus to whatever lies beside it.
            self.ui.ctx().memory_mut(|m| {
                m.set_focus_lock_filter(
                    resp.id,
                    egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() },
                );
            });
        }
        self.stops.push(Stop { id: resp.id, enabled: resp.enabled(), submenu });
    }

    /// A row that runs a command.
    pub(crate) fn command(&mut self, ctl: Ctl<'_>, row: Row<'_>) -> Response {
        self.flush();
        let resp = command_row(self.cx, self.ui, &ctl, &row);
        self.track(&resp, false);
        if resp.clicked() {
            self.picked.command = true;
        }
        resp
    }

    /// A row that applies a UI verb. `reason` greys it with that text, normally the refusal of the command the verb
    /// leads to. The label is the row's own.
    pub(crate) fn verb(&mut self, verb: Verb, label: &str, row: Row<'_>, reason: Option<String>) -> Response {
        self.flush();
        let shortcut = crate::keymap::verb_key_label(verb, self.cx.model.mac);
        let text = row.text(label);
        let face =
            Face { label: &text, icon: row.icon, check: row.check, shortcut: shortcut.as_deref(), submenu: false };
        let resp = plain_row(self.cx, self.ui, &face, reason, false);
        self.track(&resp, false);
        if resp.clicked() {
            self.cx.verb(verb);
            self.picked.local = true;
        }
        resp
    }

    /// A row that asks the app for something (a file picker).
    pub(crate) fn host(&mut self, request: HostRequest, label: &str, row: Row<'_>) -> Response {
        self.flush();
        let text = row.text(label);
        let face = Face { label: &text, icon: row.icon, check: row.check, shortcut: None, submenu: false };
        let resp = plain_row(self.cx, self.ui, &face, None, false);
        self.track(&resp, false);
        if resp.clicked() {
            self.cx.emit(UiAction::Host(request));
            self.picked.local = true;
        }
        resp
    }

    /// A row that only changes what the UI shows (it opens a dialog by itself). The caller reads `clicked()`.
    pub(crate) fn local(&mut self, label: &str, row: Row<'_>) -> Response {
        self.flush();
        let text = row.text(label);
        let face = Face { label: &text, icon: row.icon, check: row.check, shortcut: None, submenu: false };
        let resp = plain_row(self.cx, self.ui, &face, None, false);
        self.track(&resp, false);
        if resp.clicked() {
            self.picked.local = true;
        }
        resp
    }

    /// A control that is not a row (a line of toggles), drawn by `add` in the menu's `Ui`.
    pub(crate) fn custom(&mut self, add: impl FnOnce(&mut Ui, &mut Cx<'a>)) {
        self.flush();
        add(self.ui, self.cx);
    }

    /// A row with a chevron whose flyout holds the rows `add` draws.
    pub(crate) fn submenu(&mut self, label: &str, row: Row<'_>, add: impl FnOnce(&mut Menu<'_, '_, 'a>)) {
        self.flush();
        let ctx = self.ui.ctx().clone();
        let key = flyouts_key(self.level.id);
        let mut state: Flyouts = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
        let text = row.text(label);
        let row_id = self.ui.next_auto_id();
        let is_open = state.open == Some(row_id);
        let face = Face { label: &text, icon: row.icon, check: None, shortcut: None, submenu: true };
        let resp = plain_row(self.cx, self.ui, &face, None, is_open);
        self.track(&resp, true);
        let over = ctx.rect_contains_pointer(self.ui.layer_id(), resp.rect);
        let by_key = resp.has_focus() && ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space));
        if (over || resp.clicked()) && (state.open.is_none() || is_open) {
            state.open = Some(resp.id);
            state.leaving_since = None;
        }
        if by_key {
            state.focus_first = true;
        }
        let show_flyout = state.open == Some(resp.id);
        let focus_first = show_flyout && std::mem::take(&mut state.focus_first);
        // Stored before the flyout is drawn: a key in the flyout (Left) closes it by changing this state.
        ctx.data_mut(|d| d.insert_temp(key, state));
        if show_flyout {
            let level = Level { id: self.level.id.with(("flyout", resp.id)), parent: Some((resp.id, self.level.id)) };
            let picked = self.flyout(&ctx, level, resp.rect, &text, focus_first, add);
            self.picked.merge(picked);
        }
    }

    /// Draws the flyout of the row at `row`.
    fn flyout(
        &mut self,
        ctx: &Context,
        level: Level,
        row: Rect,
        label: &str,
        focus_first: bool,
        add: impl FnOnce(&mut Menu<'_, '_, 'a>),
    ) -> Picked {
        let screen = ctx.content_rect();
        let width = MENU_WIDTH + 2.0 * FRAME_PAD.x;
        let area_id = level.id;
        let height = ctx.memory(|m| m.area_rect(area_id)).map_or(self.cx.layout.row_height() * 6.0, |r| r.height());
        let at = flyout_origin(screen, self.frame, row, vec2(width, height));
        let pal = self.cx.pal;
        let max_height = (screen.height() - 2.0 * SCREEN_GAP - 2.0 * FRAME_PAD.y).max(120.0);
        let mut picked = Picked::default();
        let layer = Area::new(area_id)
            .order(Order::Foreground)
            .fixed_pos(at)
            .constrain(true)
            .show(ctx, |ui| {
                let builder = UiBuilder::new().accessibility_role(Role::Menu).accessibility_label(label);
                ui.scope_builder(builder, |ui| {
                    panel_frame(&pal).show(ui, |ui| {
                        ui.set_width(MENU_WIDTH);
                        ui.set_max_height(max_height);
                        let frame = ui.min_rect().expand2(FRAME_PAD);
                        egui::ScrollArea::vertical()
                            .id_salt("rows")
                            .max_height(max_height)
                            .auto_shrink([true, true])
                            .show(ui, |ui| {
                                let mut menu = Menu::new(ui, &mut *self.cx, level, frame);
                                if focus_first {
                                    menu.focus_first_on_open();
                                }
                                add(&mut menu);
                                picked = menu.finish();
                            });
                    });
                });
            })
            .response
            .layer_id;
        ctx.move_to_top(layer);
        picked
    }

    /// The first enabled row of a flyout opened with the keyboard takes the focus (when the rows are known).
    fn focus_first_on_open(&mut self) {
        let key = self.level.id.with("focus_first");
        self.ui.ctx().data_mut(|d| d.insert_temp(key, true));
    }

    /// Closes the open flyout when the pointer left it for good, handles the keys and returns what was chosen.
    fn finish(self) -> Picked {
        let ctx = self.ui.ctx().clone();
        let key = flyouts_key(self.level.id);
        let mut state: Flyouts = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
        self.close_when_left(&ctx, &mut state);
        ctx.data_mut(|d| d.insert_temp(key, state));
        self.keys(&ctx);
        self.picked
    }

    fn close_when_left(&self, ctx: &Context, state: &mut Flyouts) {
        let Some(open) = state.open else {
            return;
        };
        let flyout = ctx.memory(|m| m.area_rect(self.level.id.with(("flyout", open))));
        let pointer = ctx.pointer_hover_pos();
        let in_flyout = matches!((pointer, flyout), (Some(p), Some(r)) if r.contains(p));
        let on_other_row = self.pointer_row.is_some_and(|row| row != open);
        if in_flyout || !on_other_row {
            state.leaving_since = None;
            return;
        }
        let now = ctx.input(|i| i.time);
        let since = *state.leaving_since.get_or_insert(now);
        let left = CLOSE_DELAY_S - (now - since);
        if left <= 0.0 {
            self.forget_flyout(ctx, open, state);
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
    }

    fn forget_flyout(&self, ctx: &Context, open: Id, state: &mut Flyouts) {
        state.open = None;
        state.leaving_since = None;
        let child = self.level.id.with(("flyout", open));
        ctx.data_mut(|d| {
            d.remove::<Flyouts>(flyouts_key(child));
            d.remove::<bool>(child.with("focus_first"));
        });
    }

    /// Up, Down, Left and Right on the focused row, and the focus of the first row.
    fn keys(&self, ctx: &Context) {
        let focus = |id: Id| {
            // egui moves the focus by itself on an arrow key; this menu has just done it.
            ctx.memory_mut(|m| {
                m.request_focus(id);
                m.move_focus(egui::FocusDirection::None);
            });
        };
        let (up, down, right, left) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::ArrowLeft),
            )
        });
        let focused = ctx.memory(|m| m.focused());
        let first_open = self.level.id.with("focus_first");
        // The first pass of a new flyout only measures it; the focus waits for the pass that shows it.
        let wanted_first =
            !self.ui.is_sizing_pass() && ctx.data_mut(|d| d.remove_temp::<bool>(first_open)).unwrap_or(false);
        let enabled: Vec<&Stop> = self.stops.iter().filter(|s| s.enabled).collect();
        if wanted_first {
            if let Some(first) = enabled.first() {
                focus(first.id);
            }
            return;
        }
        let here = focused.and_then(|f| enabled.iter().position(|s| s.id == f));
        match here {
            Some(at) => {
                let len = enabled.len();
                let target = if down {
                    enabled.get((at + 1) % len)
                } else if up {
                    enabled.get((at + len - 1) % len)
                } else {
                    None
                };
                if let Some(target) = target {
                    focus(target.id);
                }
                if right && let Some(stop) = enabled.get(at).filter(|s| s.submenu) {
                    self.open_with_keys(ctx, stop.id);
                }
                if left && let Some((row, parent)) = self.level.parent {
                    // Back to the row that opened this flyout, and the flyout closes.
                    let parent_key = flyouts_key(parent);
                    ctx.data_mut(|d| {
                        let mut state: Flyouts = d.get_temp(parent_key).unwrap_or_default();
                        state.open = None;
                        d.insert_temp(parent_key, state);
                    });
                    focus(row);
                }
            }
            None if focused.is_none() && self.level.parent.is_none() && (down || up) => {
                let pick = if down { enabled.first() } else { enabled.last() };
                if let Some(stop) = pick {
                    focus(stop.id);
                }
            }
            None => {}
        }
        // Moving to a row of this menu with the keys closes a flyout opened from another row.
        if let (Some(f), key) = (focused, flyouts_key(self.level.id))
            && self.stops.iter().any(|s| s.id == f)
        {
            let mut state: Flyouts = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
            if state.open.is_some_and(|open| open != f) && (up || down) {
                self.forget_flyout(ctx, state.open.unwrap_or(f), &mut state);
                ctx.data_mut(|d| d.insert_temp(key, state));
            }
        }
    }

    fn open_with_keys(&self, ctx: &Context, row: Id) {
        let key = flyouts_key(self.level.id);
        ctx.data_mut(|d| {
            let mut state: Flyouts = d.get_temp(key).unwrap_or_default();
            state.open = Some(row);
            state.leaving_since = None;
            state.focus_first = true;
            d.insert_temp(key, state);
        });
        ctx.request_repaint();
    }
}

/// Where a flyout of `size` sits beside its `row`: on the right of the parent `frame` with its first row level
/// with `row`; on the left when the right has no room; always inside `screen`.
pub(crate) fn flyout_origin(screen: Rect, frame: Rect, row: Rect, size: Vec2) -> Pos2 {
    let right = frame.right() - FLYOUT_OVERLAP;
    let left = frame.left() + FLYOUT_OVERLAP - size.x;
    let x =
        if right + size.x <= screen.right() - SCREEN_GAP || left < screen.left() + SCREEN_GAP { right } else { left };
    let x = x.min(screen.right() - SCREEN_GAP - size.x).max(screen.left() + SCREEN_GAP);
    let y = (row.top() - FRAME_PAD.y).min(screen.bottom() - SCREEN_GAP - size.y).max(screen.top() + SCREEN_GAP);
    pos2(x, y)
}

/// Where and how a popup menu is drawn.
pub(crate) struct Popup<'a> {
    /// Names the menu's ids. A menu opened again gets a new key, so no flyout of the last one stays open.
    pub key: Id,
    /// Names the menu for screen readers and tests.
    pub label: &'a str,
    /// The corner of the menu that sits at `at`.
    pub pivot: Align2,
    /// Where it opens, in screen points.
    pub at: Pos2,
}

/// What a popup did.
pub(crate) struct PopupResult {
    /// The rows chosen.
    pub picked: Picked,
    /// A click landed outside the menu.
    pub outside: bool,
}

/// Draws a popup menu over the window with a backdrop that catches the click outside it.
pub(crate) fn popup(
    ctx: &Context,
    cx: &mut Cx<'_>,
    spec: &Popup<'_>,
    add: impl FnOnce(&mut Menu<'_, '_, '_>),
) -> PopupResult {
    let screen = ctx.content_rect();
    // The backdrop lies under the menu and over everything else.
    let backdrop = Area::new(spec.key.with("backdrop"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| ui.allocate_rect(screen, Sense::click()));
    let outside = backdrop.inner.clicked() || backdrop.inner.secondary_clicked();
    let width = MENU_WIDTH + 2.0 * FRAME_PAD.x;
    let pos = pos2(
        spec.at.x.clamp(screen.left() + SCREEN_GAP, (screen.right() - width - SCREEN_GAP).max(screen.left())),
        spec.at.y.max(screen.top() + SCREEN_GAP),
    );
    let pal = cx.pal;
    let mut picked = Picked::default();
    Area::new(spec.key).order(Order::Foreground).pivot(spec.pivot).fixed_pos(pos).constrain(true).show(ctx, |ui| {
        panel_frame(&pal).show(ui, |ui| {
            ui.set_width(MENU_WIDTH);
            picked = Menu::run_in(ui, cx, spec.key.with("rows"), spec.label, add);
        });
    });
    PopupResult { picked, outside }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1280.0, 800.0));

    fn parent(left: f32, right: f32) -> Rect {
        Rect::from_min_max(pos2(left, 100.0), pos2(right, 500.0))
    }

    #[test]
    fn a_flyout_opens_on_the_right_with_its_first_row_level_with_the_row() {
        let row = Rect::from_min_max(pos2(20.0, 200.0), pos2(250.0, 228.0));
        let at = flyout_origin(SCREEN, parent(10.0, 262.0), row, vec2(254.0, 150.0));
        assert_eq!(at.x, 262.0 - FLYOUT_OVERLAP, "it reaches a little over the frame of its parent");
        assert_eq!(at.y, 200.0 - FRAME_PAD.y);
    }

    #[test]
    fn a_flyout_flips_to_the_left_where_the_right_has_no_room() {
        let row = Rect::from_min_max(pos2(1000.0, 200.0), pos2(1250.0, 228.0));
        let size = vec2(254.0, 150.0);
        let at = flyout_origin(SCREEN, parent(990.0, 1262.0), row, size);
        assert_eq!(at.x, 990.0 + FLYOUT_OVERLAP - size.x);
        assert!(at.x + size.x <= SCREEN.right());
    }

    #[test]
    fn a_flyout_stays_on_screen_at_the_bottom_and_when_neither_side_has_room() {
        let size = vec2(254.0, 300.0);
        let row = Rect::from_min_max(pos2(20.0, 700.0), pos2(250.0, 728.0));
        let at = flyout_origin(SCREEN, parent(10.0, 262.0), row, size);
        assert_eq!(at.y, SCREEN.bottom() - SCREEN_GAP - size.y);
        // A window narrower than two menus: the flyout still lies inside it.
        let narrow = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 800.0));
        let at = flyout_origin(narrow, parent(10.0, 262.0), row, size);
        assert!(at.x >= narrow.left() && at.x + size.x <= narrow.right(), "{at:?}");
    }
}
