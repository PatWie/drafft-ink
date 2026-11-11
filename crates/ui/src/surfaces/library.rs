//! The library drawer (`ui.toggle_library`): the shapes the user keeps, in a panel that opens from the bottom of
//! the window.
//!
//! * **Place.** It is as wide as the window less the margins and sits above the bottom bar and the tab bar, so
//!   nothing overlaps. It reads their rectangles, and those of the tool column and the properties panel, from
//!   [`ChromeRects`](crate::chrome::ChromeRects) (they are drawn before it) and records its own there. Where the
//!   tool column or the properties panel reach down to its height, it is narrower by their width, so that their
//!   buttons stay free; the toasts, the convert popup and the stats panel sit above it. Its height is 30 % of
//!   the window until the person drags the top edge; the height they chose is kept in
//!   [`UiState::library_height`].
//! * **Sections.** The items are grouped in sections, the person's own first ("My items"), then one per imported
//!   library, named after its file. A header shows the name and the number of items; a click on it folds the
//!   section (the tiles of a folded section are not drawn, so none of its pictures is asked for). The buttons at
//!   its right rename an imported section and remove one; a right click opens the same actions as a menu. Removing
//!   a section asks first (`library.remove_section` cannot be undone). The search keeps the matching items of each
//!   section and hides the sections that have none; while it is in use, folded sections show their matches.
//! * **Tiles.** One tile per item: its picture, its name under it. The picture is the answer of
//!   `library.thumbnail` (see [`crate::thumbs`]); until it arrives the tile shows an outline with the item's
//!   proportions. A click inserts the item in the middle of the view (`library.insert`); a drag onto the canvas
//!   inserts it where the pointer is released. Rename and remove are buttons in the corner of a tile that appear
//!   under the pointer (always on a touch layout); a right click opens the same actions as a menu
//!   ([`crate::menu_kit`]): Insert, Rename…, Remove. The tooltip shows a larger picture and the element count.
//! * **Header.** Title and count, a search field (it filters by name here, in the UI), the name for the next Add,
//!   Add (`library.add`), Import… ([`HostRequest::PickLibraryImport`], which ends in `library.import`), Export
//!   (`library.export`, an `.excalidrawlib` download) and Close.

use drafftink_engine::Origin;
use egui::{
    Align, Align2, Area, Color32, Context, CursorIcon, Id, LayerId, Layout as Flow, Modal, Order, Painter, Rect, Sense,
    Stroke, StrokeKind, TextureHandle, TextureId, TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

use crate::action::{HostRequest, UiAction};
use crate::chrome::{ChromeRects, GAP};
use crate::icons::Icon;
use crate::menu_kit::{self, Menu, Popup, Row};
use crate::model::{LibraryItem, LibrarySection, Model, ViewView};
use crate::state::{LibraryMenu, LibraryTarget, RenameTarget, UiState};
use crate::surfaces::dialogs::{Close, plain_button, show_modal};
use crate::theme::{self, Control, MARGIN, Palette, TextSize, space};
use crate::thumbs::{Thumb, ThumbImage, ThumbSize};
use crate::widgets::{Ctl, Cx, Layout, Look, paint_button_bg, panel_frame};

/// The file `library.export` writes.
pub const EXPORT_FILE: &str = "drafftink.excalidrawlib";

/// The share of the window height the drawer has until the person drags its edge.
const DEFAULT_SHARE: f32 = 0.3;
/// The most of the window height the drawer may have.
const MAX_SHARE: f32 = 0.7;
/// The least height of the drawer: the header, the header of a section and one row of tiles.
const MIN_HEIGHT: f32 = 224.0;
/// The same on a touch layout, where the header takes two rows and the targets are larger.
const MIN_HEIGHT_COMPACT: f32 = 360.0;
/// Room left above the drawer for the top bars.
const TOP_ROOM: f32 = 72.0;
/// Height of the strip on the top edge that resizes the drawer.
const GRIP: f32 = 10.0;
/// Width of the Add button, which the fields of the touch layout leave room for.
const ADD_WIDTH: f32 = 80.0;
/// Height of the name under a tile's picture.
const LABEL_HEIGHT: f32 = 20.0;
/// Space around a tile's picture inside its box.
const PICTURE_PADDING: f32 = 6.0;
/// The bounds of the largest picture of the tooltip, in points.
const TOOLTIP_BOX: egui::Vec2 = vec2(240.0, 200.0);

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

/// `library.insert` for a click: the engine puts the item in the middle of the view.
pub(crate) fn insert_args(item: &str) -> Value {
    json!({ "item": item })
}

/// `library.insert` for a drop at a screen point.
pub(crate) fn drop_args(item: &str, view: &ViewView, screen: egui::Pos2) -> Value {
    json!({ "item": item, "at": view.screen_to_world(screen) })
}

/// `library.remove` of one item.
pub(crate) fn remove_args(item: &str) -> Value {
    json!({ "items": [item] })
}

/// `library.remove_section` of one section.
pub(crate) fn remove_section_args(section: &str) -> Value {
    json!({ "section": section })
}

/// `library.add`: the selection under the typed name (no name: the engine names it).
pub(crate) fn add_args(name: &str) -> Value {
    let name = name.trim();
    if name.is_empty() { json!({}) } else { json!({ "name": name }) }
}

/// `library.export` of the whole library as a download.
pub(crate) fn export_args() -> Value {
    json!({ "to": { "download": EXPORT_FILE } })
}

/// The name an item is shown and described by.
fn display_name(item: &LibraryItem) -> &str {
    if item.name.is_empty() { "Untitled" } else { &item.name }
}

/// True when the item's name has the typed text in it, whatever the case. No text matches every item.
pub(crate) fn matches_filter(item: &LibraryItem, filter: &str) -> bool {
    let filter = filter.trim();
    filter.is_empty() || item.name.to_lowercase().contains(&filter.to_lowercase())
}

/// A section with the items the search leaves in it.
pub(crate) struct Group<'a> {
    pub section: &'a LibrarySection,
    pub items: Vec<&'a LibraryItem>,
}

/// The sections that have items matching `filter`, in the model's order, each with those items. A section with
/// none is left out, whatever it holds.
pub(crate) fn groups<'a>(model: &'a Model, filter: &str) -> Vec<Group<'a>> {
    let mut groups: Vec<Group<'a>> =
        model.library_sections.iter().map(|section| Group { section, items: Vec::new() }).collect();
    let at: HashMap<&str, usize> = model.library_sections.iter().enumerate().map(|(n, s)| (s.id.as_str(), n)).collect();
    for item in model.library.iter().filter(|i| matches_filter(i, filter)) {
        if let Some(group) = at.get(item.section.as_str()).and_then(|&n| groups.get_mut(n)) {
            group.items.push(item);
        }
    }
    groups.retain(|g| !g.items.is_empty());
    groups
}

// ---- the place of the drawer ----

/// The least and the most height the drawer may have in a window of this size, whose room above the bars ends at
/// `bottom`.
pub(crate) fn height_limits(screen: egui::Rect, bottom: f32, compact: bool) -> (f32, f32) {
    let least = if compact { MIN_HEIGHT_COMPACT } else { MIN_HEIGHT };
    let room = bottom - screen.top() - TOP_ROOM;
    (least, (screen.height() * MAX_SHARE).min(room).max(least))
}

/// The height of the drawer: what the person dragged it to, else 30 % of the window, kept within limits.
pub(crate) fn drawer_height(screen: egui::Rect, bottom: f32, compact: bool, chosen: Option<f32>) -> f32 {
    let (least, most) = height_limits(screen, bottom, compact);
    chosen.unwrap_or(screen.height() * DEFAULT_SHARE).clamp(least, most)
}

/// `rect` narrowed so that it does not cover the side panels that stand in its way (the tool column and the
/// properties panel both reach down the sides of the window, so a full-width drawer would cover their lowest
/// buttons). On a narrow window the panels are collapsed to a button each and the drawer would be left with no
/// width, so it covers them.
fn clear_of_side_panels(chrome: &ChromeRects, layout: &Layout, mut rect: Rect) -> Rect {
    if layout.narrow {
        return rect;
    }
    for (panel, side) in [(chrome.tools, Side::Left), (chrome.props, Side::Right)] {
        let Some(panel) = panel else { continue };
        if panel.bottom() <= rect.top() || panel.top() >= rect.bottom() {
            continue;
        }
        match side {
            Side::Left => rect.min.x = rect.min.x.max(panel.right() + GAP),
            Side::Right => rect.max.x = rect.max.x.min(panel.left() - GAP),
        }
    }
    rect
}

/// What a tile measures.
struct Tile {
    /// Side of the square that holds the picture.
    picture: f32,
    width: f32,
    height: f32,
}

impl Tile {
    fn of(layout: &Layout) -> Self {
        let picture = if layout.compact { 112.0 } else { 104.0 };
        Self { picture, width: picture + 8.0, height: picture + 8.0 + LABEL_HEIGHT }
    }
}

// ---- textures ----

/// The pictures of the items as egui textures, made when a picture arrives.
#[derive(Default)]
pub(crate) struct TextureCache(HashMap<String, Textures>);

#[derive(Default)]
struct Textures {
    tile: Option<Uploaded>,
    preview: Option<Uploaded>,
}

struct Uploaded {
    image: Arc<ThumbImage>,
    texture: TextureHandle,
}

impl std::fmt::Debug for TextureCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextureCache").field("items", &self.0.len()).finish()
    }
}

impl TextureCache {
    /// The texture of a picture, uploaded the first time and again when the picture is another one.
    fn texture(&mut self, ctx: &Context, item: &str, size: ThumbSize, image: &Arc<ThumbImage>) -> Option<TextureId> {
        if !self.0.contains_key(item) {
            self.0.insert(item.to_owned(), Textures::default());
        }
        let slots = self.0.get_mut(item)?;
        let slot = match size {
            ThumbSize::Tile => &mut slots.tile,
            ThumbSize::Preview => &mut slots.preview,
        };
        let fresh = slot.as_ref().is_some_and(|up| Arc::ptr_eq(&up.image, image));
        if !fresh {
            let color = egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba);
            let name = format!("library:{item}:{size:?}");
            let texture = ctx.load_texture(name, color, TextureOptions::LINEAR);
            *slot = Some(Uploaded { image: Arc::clone(image), texture });
        }
        slot.as_ref().map(|up| up.texture.id())
    }

    /// Lets go of the textures of items that are not on screen.
    fn keep_only(&mut self, shown: &std::collections::HashSet<String>, hovered: Option<&str>) {
        self.0.retain(|id, _| shown.contains(id) || hovered == Some(id.as_str()));
    }
}

// ---- the drawer ----

/// Draws the drawer when it is open.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if !st.library_open {
        st.library_menu = None;
        st.library_remove = None;
        st.library_known = None;
        return;
    }
    st.library_visible.clear();
    st.library_hover = None;
    let screen = ctx.content_rect();
    let bottom = (st.chrome.bottom_floor(screen) - GAP).min(screen.bottom() - MARGIN);
    let height = drawer_height(screen, bottom, cx.layout.compact, st.library_height);
    let rect = Rect::from_min_max(pos2(screen.left() + MARGIN, bottom - height), pos2(screen.right() - MARGIN, bottom));
    let rect = clear_of_side_panels(&st.chrome, &cx.layout, rect);
    // Recorded before the tiles are drawn: a drag that ends over the drawer is not a drop on the canvas.
    st.chrome.library = Some(rect);
    let frame = panel_frame(&cx.pal);
    let inner = rect.size() - frame.total_margin().sum();
    let mut drawn = rect;
    Area::new(Id::unique("library_panel")).fixed_pos(rect.min).order(Order::Middle).show(ctx, |ui| {
        let out = frame.show(ui, |ui| {
            ui.set_width(inner.x);
            ui.set_height(inner.y);
            ui.spacing_mut().item_spacing = vec2(space::MD, space::MD);
            header(ui, cx, st);
            body(ui, cx, st);
        });
        drawn = out.response.rect;
        grip(ui, cx, st, drawn, bottom, screen);
    });
    st.library_textures.keep_only(&st.library_visible, st.library_hover.as_deref());
    st.chrome.library = Some(drawn);
    library_menu(ctx, cx, st);
    remove_section_dialog(ctx, cx, st);
}

/// The strip on the top edge: dragging it sets the height.
fn grip(ui: &mut Ui, cx: &Cx<'_>, st: &mut UiState, drawer: Rect, bottom: f32, screen: Rect) {
    let strip = Rect::from_min_max(
        pos2(drawer.left() + 24.0, drawer.top() - GRIP / 2.0),
        pos2(drawer.right() - 24.0, drawer.top() + GRIP / 2.0),
    );
    let resp = ui.interact(strip, Id::unique("library_grip"), Sense::drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Splitter, true, "Resize the library"));
    let resp = resp.on_hover_cursor(CursorIcon::ResizeVertical);
    if resp.dragged()
        && let Some(pointer) = resp.interact_pointer_pos()
    {
        let (least, most) = height_limits(screen, bottom, cx.layout.compact);
        st.library_height = Some((bottom - pointer.y).clamp(least, most));
    }
    let colour = if resp.dragged() || resp.hovered() { cx.pal.accent } else { cx.pal.separator };
    let pill = Rect::from_center_size(pos2(drawer.center().x, drawer.top() + 4.0), vec2(40.0, 4.0));
    ui.painter().rect_filled(pill, 2.0, colour);
}

fn header(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    if cx.layout.compact {
        // Two rows: the title with the file buttons, then the fields and Add.
        ui.horizontal(|ui| {
            title(ui, cx, &st.library_filter);
            ui.with_layout(Flow::right_to_left(Align::Center), |ui| {
                close(ui, cx);
                export_button(ui, cx);
                import_button(ui, cx);
            });
        });
        ui.horizontal(|ui| {
            let spare = ui.available_width() - ADD_WIDTH - 2.0 * ui.spacing().item_spacing.x;
            let width = (spare / 2.0).max(60.0);
            search_field(ui, st, width);
            name_field(ui, cx, st, width);
            add_button(ui, cx, st);
        });
    } else {
        ui.horizontal(|ui| {
            title(ui, cx, &st.library_filter);
            search_field(ui, st, 180.0);
            // From the right edge: Close, Export, Import…, Add, the name; they read in the opposite order.
            ui.with_layout(Flow::right_to_left(Align::Center), |ui| {
                close(ui, cx);
                export_button(ui, cx);
                import_button(ui, cx);
                add_button(ui, cx, st);
                name_field(ui, cx, st, 170.0);
            });
        });
    }
}

fn title(ui: &mut Ui, cx: &mut Cx<'_>, filter: &str) {
    ui.label(egui::RichText::new("Library").font(TextSize::Heading.hand()).color(cx.pal.text));
    let all = cx.model.library.len();
    let shown = cx.model.library.iter().filter(|i| matches_filter(i, filter)).count();
    ui.label(TextSize::Caption.rich(count_text(shown, all)).color(cx.pal.muted));
}

/// "3 items", or "1 of 3 items" while the search hides some.
pub(crate) fn count_text(shown: usize, all: usize) -> String {
    let items = if all == 1 { "1 item".to_owned() } else { format!("{all} items") };
    if shown == all { items } else { format!("{shown} of {items}") }
}

fn close(ui: &mut Ui, cx: &mut Cx<'_>) {
    cx.verb_button(
        ui,
        crate::action::Verb::ToggleLibrary,
        Icon::Close,
        cx.layout.small_side() - 6.0,
        "Close the library",
        false,
    );
}

fn search_field(ui: &mut Ui, st: &mut UiState, width: f32) {
    ui.scope(|ui| {
        ui.set_width(width);
        crate::surfaces::dialogs::text_field(
            ui,
            &mut st.library_filter,
            "Search by name",
            None,
            Some(256),
            "Search the library",
        );
    });
}

fn name_field(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, width: f32) {
    let field = ui
        .scope(|ui| {
            ui.set_width(width);
            crate::surfaces::dialogs::text_field(
                ui,
                &mut st.library_name,
                "Name (optional)",
                None,
                Some(256),
                "Library item name",
            )
        })
        .inner;
    // Enter in the name field adds, like the button.
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && cx.model.is_available("library.add") {
        cx.run("library.add", add_args(&st.library_name));
    }
}

fn add_button(ui: &mut Ui, cx: &mut Cx<'_>, st: &UiState) {
    let add = Ctl::new("library.add").args(add_args(&st.library_name)).label("Add selection to library");
    cx.text_button(ui, &add, "Add", true);
}

fn import_button(ui: &mut Ui, cx: &mut Cx<'_>) {
    let button = egui::Button::new(TextSize::Body.rich("Import…").color(cx.pal.text))
        .fill(cx.pal.button)
        .corner_radius(theme::BUTTON_RADIUS)
        .min_size(vec2(80.0, cx.layout.button_height()));
    if ui.add(button).clicked() {
        cx.emit(UiAction::Host(HostRequest::PickLibraryImport));
    }
}

fn export_button(ui: &mut Ui, cx: &mut Cx<'_>) {
    // The file goes to the downloads: there is nothing more to do here.
    cx.text_button(ui, &Ctl::new("library.export").args(export_args()).label("Export library"), "Export", false);
}

fn body(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    let model = cx.model;
    if let Some(why) = &model.library_error {
        ui.label(TextSize::Caption.rich(why.as_str()).color(cx.pal.muted));
        return;
    }
    if model.library.is_empty() {
        ui.label(TextSize::Caption.rich("Nothing here yet. Select shapes and press Add.").color(cx.pal.muted));
        return;
    }
    let groups = groups(model, &st.library_filter);
    if groups.is_empty() {
        let text = format!("No item has “{}” in its name.", st.library_filter.trim());
        ui.label(TextSize::Caption.rich(text).color(cx.pal.muted));
        return;
    }
    let searching = !st.library_filter.trim().is_empty();
    let tile = Tile::of(&cx.layout);
    // A section that was not there when the drawer last drew is the result of an import: bring it into view.
    let newest =
        st.library_known.as_ref().and_then(|known| groups.iter().rev().find(|g| !known.contains(&g.section.id)));
    let newest = newest.map(|g| g.section.id.clone());
    st.library_known = Some(model.library_sections.iter().map(|s| s.id.clone()).collect());
    egui::ScrollArea::vertical().id_salt("library_tiles").auto_shrink([false, false]).show(ui, |ui| {
        for group in &groups {
            let open = searching || !st.library_collapsed.contains(&group.section.id);
            let header = section_header(ui, cx, st, group, open);
            if newest.as_deref() == Some(group.section.id.as_str()) {
                ui.scroll_to_rect(header, Some(Align::TOP));
                ui.ctx().request_repaint();
            }
            if open {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(space::LG, space::LG);
                    for item in &group.items {
                        show_tile(ui, cx, st, item, &tile);
                    }
                });
            }
            ui.add_space(space::SM);
        }
    });
}

// ---- the header of a section ----

/// Folds or unfolds a section.
fn toggle_section(st: &mut UiState, section: &str) {
    if !st.library_collapsed.remove(section) {
        st.library_collapsed.insert(section.to_owned());
    }
}

/// Paints one line of text at `at` (its left, vertically centred on `centre_y`), cut with an ellipsis at
/// `max_width`. Returns its width.
fn paint_line(painter: &Painter, text: &str, size: TextSize, colour: Color32, at: (f32, f32), max_width: f32) -> f32 {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(size.font(), colour));
    job.wrap = egui::text::TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    let width = galley.size().x;
    painter.galley(pos2(at.0, at.1 - galley.size().y / 2.0), galley, colour);
    width
}

/// The header of a section: the chevron, the name, how many items it holds, and at the right the buttons that
/// rename and remove it. The whole row folds the section; a right click opens the menu. Returns its rectangle.
fn section_header(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, group: &Group<'_>, open: bool) -> Rect {
    let pal = cx.pal;
    let section = group.section;
    let height = cx.layout.control(Control::Small);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let verb = if open { "Collapse" } else { "Expand" };
    let label = format!("{verb} {}", section.name);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.as_str()));
    // The buttons sit on the right; the text gets what they leave.
    let side = height - 4.0;
    let buttons = if section.mine { 1.0 } else { 2.0 };
    let tools = buttons * side + (buttons - 1.0) * 2.0 + space::SM;
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, theme::BUTTON_RADIUS, pal.hover);
        }
        let icon = if open { Icon::ChevronDown } else { Icon::ChevronRight };
        let arrow = Rect::from_center_size(pos2(rect.left() + space::LG + 8.0, rect.center().y), vec2(16.0, 16.0));
        crate::icons::paint(painter, arrow, icon, pal.muted);
        let text_left = arrow.right() + space::MD;
        let room = rect.right() - tools - text_left;
        let name = if section.name.is_empty() { "Untitled" } else { section.name.as_str() };
        let counts = count_text(group.items.len(), section.items);
        let count_width = TextSize::Caption.font().size * counts.chars().count() as f32 * 0.55;
        let name_width = paint_line(
            painter,
            name,
            TextSize::Label,
            pal.text,
            (text_left, rect.center().y),
            room - count_width - space::LG,
        );
        let count_at = text_left + name_width + space::LG;
        paint_line(
            painter,
            &counts,
            TextSize::Caption,
            pal.muted,
            (count_at, rect.center().y),
            rect.right() - tools - count_at,
        );
    }
    if resp.clicked() {
        toggle_section(st, &section.id);
    }
    if resp.secondary_clicked() {
        let at = resp.interact_pointer_pos().unwrap_or_else(|| resp.rect.center());
        st.close_overlays();
        st.library_menu = Some(LibraryMenu { target: LibraryTarget::Section(section.id.clone()), screen: at });
    }
    // Over the row and not in the layout: a child of the row's own rectangle allocates nothing in the scroll area.
    let area = Rect::from_min_max(
        pos2(rect.right() - tools, rect.top() + 2.0),
        pos2(rect.right() - space::SM, rect.bottom() - 2.0),
    );
    let mut tools_ui = ui.new_child(
        UiBuilder::new()
            .id_salt(("section_tools", &section.id))
            .max_rect(area)
            .layout(Flow::right_to_left(Align::Center)),
    );
    tools_ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
    if cx.local_button(&mut tools_ui, Icon::Delete, side, &format!("Remove section {}", section.name), false).clicked()
    {
        st.close_overlays();
        st.library_remove = Some(section.id.clone());
    }
    if !section.mine && rename_button(&mut tools_ui, cx, side, &format!("Rename section {}", section.name)) {
        st.open_rename(RenameTarget::LibrarySection(section.id.clone()), section.name.clone());
    }
    rect
}

/// The small button with "Aa" on it. True when it was clicked.
fn rename_button(ui: &mut Ui, cx: &Cx<'_>, side: f32, label: &str) -> bool {
    let (rect, rename) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    let look =
        Look { hovered: rename.hovered(), pressed: rename.is_pointer_button_down_on(), enabled: true, selected: false };
    paint_button_bg(ui.painter(), rect, look, &cx.pal, false);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Aa", TextSize::Caption.font(), cx.pal.text);
    rename.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    rename.on_hover_text("Rename").clicked()
}

/// Asks before a section is removed: its items go with it, and the library has no undo.
fn remove_section_dialog(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(id) = st.library_remove.clone() else { return };
    let Some(section) = cx.model.library_sections.iter().find(|s| s.id == id) else {
        st.library_remove = None;
        return;
    };
    let pal = cx.pal;
    let items = if section.items == 1 { "1 item".to_owned() } else { format!("{} items", section.items) };
    let message = format!("Remove “{}” and its {items} from the library? This cannot be undone.", section.name);
    let title = "Remove section";
    show_modal(ctx, cx, Modal::new(Id::unique("library_remove_dialog")), title, 380.0, &Close::Overlays, |ui, cx| {
        ui.label(TextSize::Body.rich(message).color(pal.text));
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if plain_button(ui, &pal, "Cancel", false).clicked() {
                cx.verb(crate::action::Verb::CloseOverlays);
            }
            let remove = Ctl::new("library.remove_section").args(remove_section_args(&id)).label("Remove the section");
            if cx.text_button(ui, &remove, "Remove", true).clicked() {
                cx.verb(crate::action::Verb::CloseOverlays);
            }
        });
    });
}

// ---- one tile ----

/// What a tile paints for an item: its picture when it has one, else an outline with the item's proportions.
enum Picture<'a> {
    Drawn(&'a Arc<ThumbImage>),
    Outline,
}

fn picture_of<'a>(model: &'a Model, item: &LibraryItem, size: ThumbSize, dark: bool) -> Picture<'a> {
    match model.thumbnails.picture(item, size, dark) {
        Some(Thumb::Ready(image)) => Picture::Drawn(image),
        Some(Thumb::Pending | Thumb::Failed(_)) | None => Picture::Outline,
    }
}

/// The largest rectangle with the proportions `w` by `h` that fits in `inner`, centred in it.
fn fit(inner: Rect, w: f32, h: f32) -> Rect {
    let scale = (inner.width() / w.max(1.0)).min(inner.height() / h.max(1.0));
    Rect::from_center_size(inner.center(), vec2(w * scale, h * scale).max(vec2(4.0, 4.0)))
}

/// Where and how a picture is painted.
#[derive(Clone, Copy)]
struct Place {
    /// The box the picture is fitted into.
    inner: Rect,
    /// Which of the item's pictures it is, for the texture cache.
    size: ThumbSize,
    /// Multiplies the colours of the picture (white leaves them).
    tint: Color32,
}

/// Paints the picture of an item (or its outline) in `place`.
fn paint_picture(
    painter: &Painter,
    cache: &mut TextureCache,
    pal: &Palette,
    item: &LibraryItem,
    picture: &Picture<'_>,
    place: &Place,
) {
    let Place { inner, size, tint } = *place;
    match picture {
        Picture::Drawn(image) => {
            let [w, h] = image.size;
            let shape = fit(inner, w as f32, h as f32);
            if let Some(texture) = cache.texture(painter.ctx(), &item.id, size, image) {
                painter.image(texture, shape, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
            }
        }
        Picture::Outline => {
            let shape = fit(inner, item.width.max(1.0) as f32, item.height.max(1.0) as f32);
            painter.rect_stroke(shape, 2.0, Stroke::new(1.5, pal.muted), StrokeKind::Inside);
        }
    }
}

fn show_tile(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, item: &LibraryItem, tile: &Tile) {
    let pal = cx.pal;
    let model = cx.model;
    let (rect, resp) = ui.allocate_exact_size(vec2(tile.width, tile.height), Sense::click_and_drag());
    let enabled = model.is_available("library.insert");
    let name = display_name(item);
    let seen = ui.is_rect_visible(rect);
    let box_rect = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(tile.picture, tile.picture));
    if seen {
        st.library_visible.insert(item.id.clone());
        if ui.rect_contains_pointer(rect) && enabled {
            ui.painter().rect_filled(rect, theme::BUTTON_RADIUS, pal.hover);
        }
        ui.painter().rect_filled(box_rect, theme::BUTTON_RADIUS, pal.button);
        let picture = picture_of(model, item, ThumbSize::Tile, pal.dark);
        let inner = box_rect.shrink(PICTURE_PADDING);
        let place = Place { inner, size: ThumbSize::Tile, tint: Color32::WHITE };
        paint_picture(ui.painter(), &mut st.library_textures, &pal, item, &picture, &place);
        paint_name(ui, &pal, rect, box_rect, name);
    }
    let label = format!("Insert {name}");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label.as_str()));
    let resp = match cx.refusal("library.insert") {
        Some(why) => resp.on_hover_text(why),
        None => resp.on_hover_ui(|ui| tooltip(ui, model, st, item, pal)),
    };
    if enabled && resp.clicked() {
        cx.run("library.insert", insert_args(&item.id));
    }
    if resp.secondary_clicked() {
        let at = resp.interact_pointer_pos().unwrap_or_else(|| resp.rect.center());
        st.close_overlays();
        st.library_menu = Some(LibraryMenu { target: LibraryTarget::Item(item.id.clone()), screen: at });
    }
    if resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        paint_drag(ui.ctx(), st, model, item, &pal, tile.picture);
    }
    if enabled && resp.drag_stopped() {
        // The release point decides: over the drawer it is a cancelled drag.
        let drawer = st.chrome.library.unwrap_or(Rect::NOTHING);
        if let Some(at) = ui.ctx().pointer_latest_pos().filter(|p| !drawer.contains(*p)) {
            cx.run("library.insert", drop_args(&item.id, &model.view, at));
        }
    }
    if seen && !resp.dragged() && (cx.layout.compact || ui.rect_contains_pointer(rect)) {
        corner_buttons(ui, cx, st, item, box_rect);
    }
}

fn paint_name(ui: &Ui, pal: &Palette, tile: Rect, box_rect: Rect, name: &str) {
    let width = tile.width() - 8.0;
    let mut job = egui::text::LayoutJob::single_section(
        name.to_owned(),
        egui::TextFormat::simple(TextSize::Small.font(), pal.text),
    );
    job.wrap =
        egui::text::TextWrapping {
            max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…')
        };
    let galley = ui.painter().layout_job(job);
    let at = pos2(tile.center().x - galley.size().x / 2.0, box_rect.bottom() + 4.0);
    ui.painter().galley(at, galley, pal.text);
}

/// The picture follows the pointer while a tile is dragged.
fn paint_drag(ctx: &Context, st: &mut UiState, model: &Model, item: &LibraryItem, pal: &Palette, side: f32) {
    let Some(pointer) = ctx.pointer_latest_pos() else { return };
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("library_drag")));
    let inner = Rect::from_center_size(pointer, vec2(side, side));
    let picture = picture_of(model, item, ThumbSize::Tile, pal.dark);
    let place = Place { inner, size: ThumbSize::Tile, tint: Color32::from_white_alpha(170) };
    paint_picture(&painter, &mut st.library_textures, pal, item, &picture, &place);
}

/// Rename and remove, in the top right corner of a tile. They are drawn over the tile in a child `Ui` that is
/// not allocated in the parent: the tiles wrap, and a rectangle that the parent has to place would move the tiles
/// that follow it.
fn corner_buttons(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, item: &LibraryItem, box_rect: Rect) {
    let side = cx.layout.small_side() - 6.0;
    let name = display_name(item);
    let area = Rect::from_min_max(
        pos2(box_rect.right() - 2.0 * side - 8.0, box_rect.top() + 2.0),
        box_rect.right_top() + vec2(-2.0, 2.0 + side),
    );
    let mut corner = ui.new_child(
        UiBuilder::new().id_salt(("corner", &item.id)).max_rect(area).layout(Flow::right_to_left(Align::Min)),
    );
    corner.spacing_mut().item_spacing = vec2(2.0, 0.0);
    let remove = Ctl::new("library.remove").args(remove_args(&item.id)).label(format!("Remove {name}"));
    cx.icon_button(&mut corner, &remove, Icon::Delete, side);
    if rename_button(&mut corner, cx, side, &format!("Rename {name}")) {
        st.open_rename(RenameTarget::LibraryItem(item.id.clone()), item.name.clone());
    }
}

/// The tooltip of a tile: the larger picture (the tile's own, enlarged, until it arrives), the name, the count.
fn tooltip(ui: &mut Ui, model: &Model, st: &mut UiState, item: &LibraryItem, pal: Palette) {
    // The tooltip is open, so the larger picture is worth drawing.
    st.library_hover = Some(item.id.clone());
    let (size, picture) = match picture_of(model, item, ThumbSize::Preview, pal.dark) {
        Picture::Outline => (ThumbSize::Tile, picture_of(model, item, ThumbSize::Tile, pal.dark)),
        drawn => (ThumbSize::Preview, drawn),
    };
    ui.set_max_width(TOOLTIP_BOX.x);
    let (rect, _) = ui.allocate_exact_size(TOOLTIP_BOX, Sense::hover());
    ui.painter().rect_filled(rect, theme::BUTTON_RADIUS, pal.button);
    let place = Place { inner: rect.shrink(PICTURE_PADDING), size, tint: Color32::WHITE };
    paint_picture(ui.painter(), &mut st.library_textures, &pal, item, &picture, &place);
    ui.label(TextSize::Body.rich(display_name(item)).color(pal.text));
    let elements =
        if item.element_count == 1 { "1 element".to_owned() } else { format!("{} elements", item.element_count) };
    ui.label(TextSize::Caption.rich(elements).color(pal.muted));
    ui.label(TextSize::Caption.rich("Click to insert. Drag onto the canvas to place it.").color(pal.muted));
}

// ---- the menu of a tile ----

/// Draws the right-click menu of a tile or a section header, when one is open: the same actions as the click and
/// the buttons.
fn library_menu(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(open) = st.library_menu.clone() else { return };
    let model = cx.model;
    let (label, key) = match &open.target {
        LibraryTarget::Item(_) => ("Library item menu", "library_menu"),
        LibraryTarget::Section(_) => ("Library section menu", "library_section_menu"),
    };
    let spec = Popup { key: Id::unique(key), label, pivot: Align2::LEFT_TOP, at: open.screen };
    let result = match &open.target {
        LibraryTarget::Item(id) => {
            let Some(item) = model.library.iter().find(|i| &i.id == id) else {
                st.library_menu = None;
                return;
            };
            cx.with_origin(Origin::ContextMenu, |cx| menu_kit::popup(ctx, cx, &spec, |m| tile_rows(m, st, item)))
        }
        LibraryTarget::Section(id) => {
            let Some(section) = model.library_sections.iter().find(|s| &s.id == id) else {
                st.library_menu = None;
                return;
            };
            cx.with_origin(Origin::ContextMenu, |cx| menu_kit::popup(ctx, cx, &spec, |m| section_rows(m, st, section)))
        }
    };
    if result.picked.any() {
        st.library_menu = None;
    } else if result.outside {
        cx.verb(crate::action::Verb::CloseOverlays);
    }
}

/// The rows of the menu of a section header.
fn section_rows(m: &mut Menu<'_, '_, '_>, st: &mut UiState, section: &LibrarySection) {
    let folded = st.library_collapsed.contains(&section.id);
    let (text, icon) = if folded { ("Expand", Icon::ChevronDown) } else { ("Collapse", Icon::ChevronRight) };
    if m.local(text, Row::new().icon(icon)).clicked() {
        toggle_section(st, &section.id);
    }
    if !section.mine && m.local("Rename", Row::new().icon(Icon::Text).dialog()).clicked() {
        st.open_rename(RenameTarget::LibrarySection(section.id.clone()), section.name.clone());
    }
    m.separator();
    if m.local("Remove section", Row::new().icon(Icon::Delete).dialog()).clicked() {
        st.close_overlays();
        st.library_remove = Some(section.id.clone());
    }
}

/// The rows of the menu of a tile.
fn tile_rows(m: &mut Menu<'_, '_, '_>, st: &mut UiState, item: &LibraryItem) {
    m.command(Ctl::new("library.insert").args(insert_args(&item.id)), Row::new().label("Insert").icon(Icon::Library));
    if m.local("Rename", Row::new().icon(Icon::Text).dialog()).clicked() {
        st.open_rename(RenameTarget::LibraryItem(item.id.clone()), item.name.clone());
    }
    m.separator();
    m.command(Ctl::new("library.remove").args(remove_args(&item.id)), Row::new().label("Remove").icon(Icon::Delete));
}
