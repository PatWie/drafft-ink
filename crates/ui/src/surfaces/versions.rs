//! The versions panel (`ui.toggle_versions`): the named versions of the document, newest first.
//!
//! * **Place.** A card at the top right under the strip, like the debug panel. On a compact layout it takes the
//!   width of the window less the margins and its rows are as tall as a touch target; it never reaches the bottom
//!   bars and scrolls when the list is long.
//! * **Header.** The title with the count, "Name this version…" and Close. The button opens the name dialog
//!   (`RenameTarget::NewVersion`, which ends in `version.save`); it is greyed with the refusal of `version.save` as
//!   its tooltip (view-only, a room that is still loading). Its label is the title of the command.
//! * **Rows.** Name, then how long ago and by whom, from the model (`version.list`). A click, a right click or Enter
//!   opens the row menu ([`crate::menu_kit`]): Open version as copy (`version.open`), Restore version… (asks
//!   first, then `version.restore`), Rename version… (the rename dialog, `version.rename`) and Delete version…
//!   (asks first, then `version.delete`). Every label is the title of its command in the registry.
//! * **Asking.** Restore and Delete ask in a dialog, because the first replaces the scene (one undo step takes it
//!   back) and the second cannot be taken back. The dialog says so, and its button is the command.

use drafftink_engine::Origin;
use egui::{
    Align, Align2, Area, Context, Id, Layout as Flow, Modal, Order, Role, ScrollArea, Sense, Ui, WidgetInfo, pos2, vec2,
};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::age;
use crate::chrome::GAP;
use crate::icons::{self, Icon};
use crate::menu_kit::{self, Popup, Row};
use crate::model::{Model, VersionRow};
use crate::state::{RenameTarget, UiState, VersionAsk, VersionMenu};
use crate::surfaces::dialogs::{Close, plain_button, show_modal};
use crate::theme::{MARGIN, SMALL_BUTTON, TextSize, space};
use crate::widgets::{Ctl, Cx, paint_button_bg, panel_frame};

/// Width of the card on a layout with a mouse.
const WIDTH: f32 = 320.0;
/// Tallest the list grows before it scrolls, when the window leaves room.
const LIST_HEIGHT: f32 = 420.0;
/// Height of a row on a layout with a mouse: two lines of text.
const ROW_HEIGHT: f32 = 46.0;
/// Space between two rows.
const ROW_GAP: f32 = 2.0;
/// What the title, the button and their spacing take above the list.
const HEADER_HEIGHT: f32 = 96.0;
/// Space inside a row, left and right.
const ROW_PAD: f32 = space::LG;

/// `version.open` of a version.
pub(crate) fn open_args(id: &str) -> Value {
    json!({ "id": id, "mode": "new_tab" })
}

/// `version.restore` or `version.delete` of a version.
pub(crate) fn id_args(id: &str) -> Value {
    json!({ "id": id })
}

/// The second line of a row: how long ago, and by whom when that is known.
pub(crate) fn detail(now_ms: u64, v: &VersionRow) -> String {
    let ago = age::ago(now_ms, v.created_ms);
    match &v.author {
        Some(author) => format!("{ago} · {author}"),
        None => ago,
    }
}

/// The tooltip of a row: the exact time and the size of the scene.
pub(crate) fn tooltip(v: &VersionRow) -> String {
    let n = v.elements;
    let elements = if n == 1 { "1 element".to_owned() } else { format!("{n} elements") };
    format!("{}\n{elements}", age::exact(v.created_ms))
}

/// The title of a command, with an ellipsis when its row opens a question first.
fn asking(model: &Model, id: &str) -> String {
    format!("{}…", model.title(id))
}

/// Draws the panel, its row menu and its question while they are open.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if st.versions_open {
        panel(ctx, cx, st);
    }
    row_menu(ctx, cx, st);
    ask(ctx, cx, st);
}

fn panel(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let pal = cx.pal;
    let screen = ctx.content_rect();
    let top = st.chrome.below_strip(screen) + GAP;
    let floor = st.chrome.bottom_floor(screen) - GAP;
    let width = WIDTH.min((screen.width() - 2.0 * MARGIN).max(160.0));
    let room = (floor - top).max(120.0);
    Area::new(Id::unique("versions_panel")).anchor(Align2::RIGHT_TOP, vec2(-MARGIN, top)).order(Order::Middle).show(
        ctx,
        |ui| {
            panel_frame(&pal).show(ui, |ui| {
                ui.set_width(width);
                header(ui, cx, st);
                ui.add_space(space::MD);
                let rows = &cx.model.versions;
                if rows.is_empty() {
                    ui.label(TextSize::Small.rich("No versions yet. Name one to keep this state.").color(pal.muted));
                    return;
                }
                // The height of the list is known before it is drawn, so the card has its size in the first frame.
                let row_height = row_height(cx);
                let content = rows.len() as f32 * (row_height + ROW_GAP);
                let list = content.min((room - HEADER_HEIGHT).clamp(row_height, LIST_HEIGHT));
                ScrollArea::vertical().max_height(list).min_scrolled_height(list).auto_shrink([false, false]).show(
                    ui,
                    |ui| {
                        ui.spacing_mut().item_spacing.y = ROW_GAP;
                        for v in rows {
                            row(ui, cx, st, v);
                        }
                    },
                );
            });
        },
    );
}

/// The title with the count, the button that names a version, and Close.
fn header(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    let pal = cx.pal;
    ui.horizontal(|ui| {
        let count = cx.model.versions.len();
        let title = if count == 0 { "Versions".to_owned() } else { format!("Versions ({count})") };
        ui.label(egui::RichText::new(title).font(TextSize::Title.hand()).color(pal.text));
        ui.with_layout(Flow::right_to_left(Align::Center), |ui| {
            cx.verb_button(ui, Verb::ToggleVersions, Icon::Close, SMALL_BUTTON, "Close the versions", false);
        });
    });
    ui.add_space(space::SM);
    let reason = cx.refusal("version.save");
    let label = asking(cx.model, "version.save");
    let button = ui.add_enabled_ui(reason.is_none(), |ui| plain_button(ui, &pal, &label, true)).inner;
    let button = match reason {
        Some(r) => button.on_disabled_hover_text(r),
        None => button,
    };
    if button.clicked() {
        st.open_rename(RenameTarget::NewVersion);
    }
}

/// The height of a row: two lines of text, and at least a touch target on a layout that has them.
fn row_height(cx: &Cx<'_>) -> f32 {
    ROW_HEIGHT.max(cx.layout.row_height() + 18.0)
}

/// One version: its name above, and how long ago and by whom below. The whole row is the button that opens the menu.
fn row(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, v: &VersionRow) {
    let pal = cx.pal;
    let height = row_height(cx);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    if ui.is_rect_visible(rect) {
        let look = crate::widgets::Look {
            hovered: resp.hovered() || resp.has_focus(),
            pressed: resp.is_pointer_button_down_on(),
            enabled: true,
            selected: st.version_menu.as_ref().is_some_and(|m| m.id == v.id),
        };
        paint_button_bg(ui.painter(), rect, look, &pal, true);
        let text_width = rect.width() - 2.0 * ROW_PAD - SMALL_BUTTON;
        let name = truncated(ui, &v.name, TextSize::Body, pal.text, text_width);
        let sub = truncated(ui, &detail(cx.model.now_ms, v), TextSize::Small, pal.muted, text_width);
        let x = rect.left() + ROW_PAD;
        let block = name.size().y + sub.size().y;
        let y = rect.center().y - block / 2.0;
        ui.painter().galley(pos2(x, y), name.clone(), pal.text);
        ui.painter().galley(pos2(x, y + name.size().y), sub, pal.muted);
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(pos2(rect.right() - ROW_PAD - 8.0, rect.center().y), vec2(16.0, 16.0)),
            Icon::ChevronDown,
            pal.muted,
        );
    }
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("Version {}", v.name)));
    let resp = resp.on_hover_text(tooltip(v));
    if resp.clicked() || resp.secondary_clicked() {
        let at = resp.interact_pointer_pos().unwrap_or_else(|| rect.left_bottom());
        st.version_menu = Some(VersionMenu { id: v.id.clone(), screen: at });
    }
}

/// One line of `text`, cut with an ellipsis at `width`.
fn truncated(ui: &Ui, text: &str, size: TextSize, color: egui::Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), size.font(), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(16.0));
    ui.painter().layout_job(job)
}

/// The menu of a row.
fn row_menu(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(open) = st.version_menu.clone() else { return };
    let Some(v) = cx.model.versions.iter().find(|v| v.id == open.id) else {
        st.version_menu = None;
        return;
    };
    let model = cx.model;
    let spec =
        Popup { key: Id::unique("version_menu"), label: "Version menu", pivot: Align2::LEFT_TOP, at: open.screen };
    let (restore, delete) = (asking(model, "version.restore"), asking(model, "version.delete"));
    let (restore_reason, delete_reason) = (cx.refusal("version.restore"), cx.refusal("version.delete"));
    let result = cx.with_origin(Origin::ContextMenu, |cx| {
        menu_kit::popup(ctx, cx, &spec, |m| {
            m.command(Ctl::new("version.open").args(open_args(&v.id)), Row::new().icon(Icon::Open));
            if m.local_gated(&restore, Row::new().icon(Icon::Undo), restore_reason).clicked() {
                st.version_ask = Some(VersionAsk::Restore(v.id.clone()));
            }
            if m.local(&asking(model, "version.rename"), Row::new().icon(Icon::Text)).clicked() {
                st.open_rename(RenameTarget::Version(v.id.clone()));
            }
            m.separator();
            if m.local_gated(&delete, Row::new().icon(Icon::Delete), delete_reason).clicked() {
                st.version_ask = Some(VersionAsk::Delete(v.id.clone()));
            }
        })
    });
    if result.picked.any() {
        st.version_menu = None;
    } else if result.outside {
        cx.verb(Verb::CloseOverlays);
    }
}

/// The question before a restore or a delete.
fn ask(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(what) = st.version_ask.clone() else { return };
    let id = match &what {
        VersionAsk::Restore(id) | VersionAsk::Delete(id) => id,
    };
    let Some(v) = cx.model.versions.iter().find(|v| &v.id == id) else {
        st.version_ask = None;
        return;
    };
    let pal = cx.pal;
    let when = age::exact(v.created_ms);
    let (command, title, message, button) = match what {
        VersionAsk::Restore(_) => (
            "version.restore",
            "Restore this version?",
            format!(
                "The scene goes back to “{}” as it was on {when}. Everything changed since then is replaced. \
                 Undo (Ctrl+Z) takes it back, and every named version stays.",
                v.name
            ),
            "Restore",
        ),
        VersionAsk::Delete(_) => (
            "version.delete",
            "Delete this version?",
            format!("“{}” from {when} is deleted for good. The scene is not changed. This cannot be undone.", v.name),
            "Delete",
        ),
    };
    let args = id_args(&v.id);
    show_modal(ctx, cx, Modal::new(Id::unique("version_ask")), title, 380.0, &Close::Overlays, |ui, cx| {
        ui.label(TextSize::Body.rich(message).color(pal.text));
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if plain_button(ui, &pal, "Cancel", false).clicked() {
                cx.verb(Verb::CloseOverlays);
            }
            let ctl = Ctl::new(command).args(args.clone()).label(cx.model.title(command));
            if cx.text_button(ui, &ctl, button, true).clicked() {
                cx.verb(Verb::CloseOverlays);
            }
        });
    });
}
