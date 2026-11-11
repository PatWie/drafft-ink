//! The controls of each ribbon group. Every one is a command (`style.set` and the others of the catalog) or a
//! UI verb; the choices that only the UI knows (which menu is open) live in `UiState`.

use drafftink_engine::state::Tool;
use egui::{Color32, FontId, Painter, Rect, Response, Role, Sense, Stroke, Ui, WidgetInfo, pos2, vec2};
use serde_json::{Value, json};

use super::pictograms::{self, Align, LineStyle, Pattern};
use super::{Group, Row};
use crate::action::Verb;
use crate::icons::{self, Icon};
use crate::model::StyleProp;
use crate::scrub::{self, ScrubControl, SliderSpec};
use crate::state::{OpenRibbonMenu, RibbonMenu, UiState};
use crate::surfaces::color::{PopoverKey, remember_anchor_in};
use crate::surfaces::topbar::not_applicable;
use crate::theme::{TextSize, UiFontChoice, space};
use crate::widgets::{Ctl, Cx, Look, icon_color, paint_button_bg, strip_row};

/// Sizes of the text presets.
pub(super) const FONT_SIZES: [(&str, f64); 4] = [("S", 16.0), ("M", 20.0), ("L", 28.0), ("XL", 36.0)];
/// Corner radius of "round".
pub(super) const ROUND_CORNER: f64 = 32.0;
/// Line heights offered (the property takes 1.0 to 3.0).
pub(super) const LINE_HEIGHTS: [f64; 4] = [1.0, 1.25, 1.5, 2.0];
/// Width of the font button, wide enough for the longest family name.
const FONT_BUTTON_WIDTH: f32 = 124.0;
/// Width of the "More" button.
pub(crate) const MORE_BUTTON_WIDTH: f32 = 64.0;
/// Width of a head button, wide enough for "Start: Triangle".
const HEAD_BUTTON_WIDTH: f32 = 116.0;
/// The stroke widths: name and value.
const STROKE_WIDTHS: [(&str, f64); 4] = [("Thin", 1.0), ("Normal", 2.0), ("Bold", 4.0), ("Extra bold", 8.0)];
/// The thickness each stroke width is drawn with on its button.
const STROKE_WIDTH_SAMPLES: [f32; 4] = [1.0, 2.5, 4.5, 7.0];
/// The width the opacity slider has.
const SLIDER_WIDTH: f32 = 84.0;
/// The width reserved for the percentage next to the slider ("100%").
const PERCENT_WIDTH: f32 = 38.0;
/// The gap between the buttons of one segmented choice.
const SEGMENT_GAP: f32 = space::XXS;

pub(super) const FONT_WEIGHTS: [(&str, &str, &str); 3] =
    [("Light", "Light", "light"), ("Regular", "Reg", "regular"), ("Heavy", "Heavy", "heavy")];
pub(super) const TEXT_ALIGNS: [(&str, &str, Align); 3] =
    [("Left", "left", Align::Left), ("Center", "center", Align::Center), ("Right", "right", Align::Right)];
pub(super) const ARROW_TYPES: [(&str, &str); 3] = [("Sharp", "sharp"), ("Round", "round"), ("Elbow", "elbow")];
pub(super) const HEADS: [(&str, &str); 6] = [
    ("None", "none"),
    ("Arrow", "arrow"),
    ("Bar", "bar"),
    ("Dot", "circle"),
    ("Triangle", "triangle"),
    ("Diamond", "diamond"),
];
const BRUSHES: [(&str, &str); 2] = [("Normal", "normal"), ("Calligraphy", "calligraphy")];
const STROKE_STYLES: [(LineStyle, &str, &str); 3] = [
    (LineStyle::Solid, "solid", "Solid"),
    (LineStyle::Dashed, "dashed", "Dashed"),
    (LineStyle::Dotted, "dotted", "Dotted"),
];

impl Group {
    /// Draws the group in `ui`: its rows one under the other, each as tall as a small button.
    pub(crate) fn draw(self, ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
        let rows = self.rows(cx);
        let height = cx.layout.small_side();
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = space::SM;
            for row in rows {
                strip_row(ui, height, |ui| {
                    ui.spacing_mut().item_spacing = vec2(SEGMENT_GAP, 0.0);
                    row.draw(ui, cx, st);
                });
            }
        });
    }
}

impl Row {
    fn draw(self, ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
        match self {
            Row::Width => width(ui, cx),
            Row::StrokeStyle => stroke_style(ui, cx),
            Row::FillPattern => fill_pattern(ui, cx),
            Row::Corners => corners(ui, cx),
            Row::ArrowPath => arrow_path(ui, cx),
            Row::ArrowHeads => arrow_heads(ui, cx, st),
            Row::Brush => brush(ui, cx),
            Row::FontFamily => font_family(ui, cx, st),
            Row::FontWeight => font_weight(ui, cx),
            Row::TextSize => text_size(ui, cx),
            Row::Emphasis => emphasis(ui, cx),
            Row::TextAlign => text_align(ui, cx),
            Row::VerticalAlign => vertical_align(ui, cx),
            Row::Opacity => opacity(ui, cx),
            Row::Align => align(ui, cx),
            Row::Layer => layer(ui, cx),
            Row::Flip => flip(ui, cx),
            Row::Compose => compose(ui, cx),
            Row::LinkDelete => link_and_delete(ui, cx),
        }
    }
}

// ---- building blocks ----

/// A small text button of a segmented choice, filled with the accent when it is on.
fn chip(cx: &mut Cx<'_>, ui: &mut Ui, ctl: &Ctl<'_>, text: &str) -> Response {
    let pal = cx.pal;
    let galley = ui.painter().layout_no_wrap(text.to_owned(), TextSize::Small.font(), pal.text);
    let height = cx.layout.small_side();
    let size = vec2((galley.size().x + 2.0 * space::LG).max(height), height);
    let ctl = Ctl { label: if ctl.label.is_empty() { text.to_owned() } else { ctl.label.clone() }, ..ctl.clone() };
    cx.command_widget(ui, &ctl, size, move |p, rect, look| {
        paint_button_bg(p, rect, look, &pal, false);
        let at = pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0);
        p.galley(at, galley, icon_color(look, &pal));
    })
}

/// A square button that paints its own picture.
fn picture(cx: &mut Cx<'_>, ui: &mut Ui, ctl: &Ctl<'_>, draw: impl FnOnce(&Painter, Rect, Color32)) -> Response {
    let pal = cx.pal;
    let side = cx.layout.small_side();
    cx.command_widget(ui, ctl, vec2(side, side), move |p, rect, look: Look| {
        paint_button_bg(p, rect, look, &pal, true);
        draw(p, rect.shrink(side * 0.22), icon_color(look, &pal));
    })
}

/// What a [`dropdown`] shows.
struct Dropdown<'a> {
    /// The accessible name and the tooltip.
    label: &'a str,
    /// The text on the button.
    text: &'a str,
    font: FontId,
    width: f32,
    /// The thing it opens is open: the button is drawn selected.
    open: bool,
    /// Greys the button with this tooltip.
    reason: Option<String>,
}

/// A button with a text and a chevron that opens something. The caller reacts to the click.
/// `reason` greys it with that tooltip.
fn dropdown(cx: &Cx<'_>, ui: &mut Ui, spec: Dropdown<'_>) -> Response {
    let Dropdown { label, text, font, width, open, reason } = spec;
    let pal = cx.pal;
    let enabled = reason.is_none();
    let height = cx.layout.small_side();
    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
            let look = Look {
                hovered: resp.hovered() || resp.has_focus(),
                pressed: resp.is_pointer_button_down_on(),
                enabled,
                selected: open,
            };
            if ui.is_rect_visible(rect) {
                paint_button_bg(ui.painter(), rect, look, &pal, false);
                let ink = icon_color(look, &pal);
                let chevron =
                    Rect::from_center_size(pos2(rect.right() - space::LG - 3.0, rect.center().y), vec2(12.0, 12.0));
                icons::paint(ui.painter(), chevron, Icon::ChevronDown, ink);
                let galley = ui.painter().layout_no_wrap(text.to_owned(), font, ink);
                let clip = Rect::from_min_max(rect.min, pos2(chevron.left() - space::XS, rect.max.y));
                let at = pos2(rect.left() + space::LG, rect.center().y - galley.size().y / 2.0);
                ui.painter().with_clip_rect(clip).galley(at, galley, ink);
            }
            resp.widget_info(|| {
                let mut info = WidgetInfo::labeled(Role::Button, enabled, label);
                info.selected = Some(open);
                info
            });
            resp
        })
        .inner;
    match reason {
        Some(r) => resp.on_disabled_hover_text(r),
        None => resp.on_hover_text(label),
    }
}

/// A [`dropdown`] that opens a ribbon menu (`menus.rs`) under itself.
fn menu_button(
    cx: &Cx<'_>,
    ui: &mut Ui,
    st: &mut UiState,
    kind: RibbonMenu,
    label: &str,
    text: &str,
    width: f32,
) -> Response {
    let open = st.ribbon_menu.is_some_and(|m| m.kind == kind);
    let spec = Dropdown { label, text, font: TextSize::Small.font(), width, open, reason: None };
    let resp = dropdown(cx, ui, spec);
    if !ui.is_sizing_pass() {
        if resp.clicked() {
            st.close_overlays();
            st.ribbon_menu = Some(OpenRibbonMenu { kind, anchor: resp.rect });
        } else if open {
            // The ribbon may have moved since the menu opened.
            st.ribbon_menu = Some(OpenRibbonMenu { kind, anchor: resp.rect });
        }
    }
    resp
}

fn style_ctl(prop: &str, value: impl Into<Value>) -> Ctl<'static> {
    Ctl::new("style.set").args(json!({ "props": { prop: value.into() } }))
}

// ---- stroke and fill ----

fn width(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.num_value("stroke_width");
    let blocked = not_applicable(cx, "stroke_width");
    for (&(name, w), thickness) in STROKE_WIDTHS.iter().zip(STROKE_WIDTH_SAMPLES) {
        let selected = current.is_some_and(|c| (c - w).abs() < 0.01);
        let ctl = style_ctl("stroke_width", w).label(name).selected(selected).blocked(blocked.clone());
        picture(cx, ui, &ctl, move |p, rect, ink| {
            let y = rect.center().y;
            p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(thickness, ink));
        });
    }
}

fn stroke_style(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.str_value("stroke_style").map(str::to_owned);
    for (style, value, name) in STROKE_STYLES {
        let ctl = style_ctl("stroke_style", value).label(name).selected(current.as_deref() == Some(value));
        picture(cx, ui, &ctl, move |p, rect, ink| pictograms::line_sample(p, rect, style, ink));
    }
}

fn fill_pattern(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.str_value("fill_pattern").map(str::to_owned);
    for (pattern, value, name) in Pattern::ALL {
        let ctl = style_ctl("fill_pattern", value)
            .label(format!("Fill pattern: {name}"))
            .selected(current.as_deref() == Some(value));
        picture(cx, ui, &ctl, move |p, rect, ink| pictograms::pattern_tile(p, rect, pattern, ink));
    }
}

fn corners(ui: &mut Ui, cx: &mut Cx<'_>) {
    for (name, value) in CORNERS {
        let ctl = corner_ctl(cx, name, value);
        let rounded = value > 0.0;
        picture(cx, ui, &ctl, move |p, rect, ink| pictograms::corner(p, rect, rounded, ink));
    }
}

/// The two corner choices: sharp (radius 0) and round.
pub(super) const CORNERS: [(&str, f64); 2] = [("Sharp", 0.0), ("Round", ROUND_CORNER)];

/// The control of one corner choice, selected when the selection (or the defaults) has it.
pub(super) fn corner_ctl(cx: &Cx<'_>, name: &str, value: f64) -> Ctl<'static> {
    let selected =
        cx.model.style.num_value("corner_radius").is_some_and(|r| if value == 0.0 { r < 1.0 } else { r >= 1.0 });
    style_ctl("corner_radius", value).label(format!("Corners: {name}")).selected(selected)
}

// ---- arrows and lines ----

/// The word the settings hold for a property of new arrows.
fn arrow_setting(cx: &Cx<'_>, key: &str, default: &str) -> String {
    cx.model.settings.get(key).and_then(Value::as_str).unwrap_or(default).to_owned()
}

/// True for the arrow tool with nothing selected: the controls then set the defaults of new arrows
/// (`tool.set_arrow`).
pub(super) fn new_arrows(cx: &Cx<'_>) -> bool {
    cx.model.selection_count == 0 && cx.model.tool == Tool::Arrow
}

/// True when the ribbon has the path group: arrows and lines, selected or about to be drawn.
pub(super) fn has_path(cx: &Cx<'_>) -> bool {
    let model = cx.model;
    new_arrows(cx) || (model.selection_count > 0 && (model.style.has_kind("arrow") || model.style.has_kind("line")))
}

fn arrow_path(ui: &mut Ui, cx: &mut Cx<'_>) {
    if new_arrows(cx) {
        let path = arrow_setting(cx, "arrow.type", "sharp");
        for (name, value) in ARROW_TYPES {
            let ctl = Ctl::new("tool.set_arrow")
                .args(json!({ "arrow_type": value }))
                .label(format!("New arrows: {name}"))
                .selected(path == value);
            chip(cx, ui, &ctl, name);
        }
    } else {
        for (name, value) in ARROW_TYPES {
            let ctl = Ctl::new("arrow.set_type").args(json!({ "type": value })).label(format!("Path: {name}"));
            chip(cx, ui, &ctl, name);
        }
    }
}

/// The word shown on a head button: the head when it is known (new arrows, a selected arrow), else the side.
fn head_text(side: &str, known: Option<&str>) -> String {
    match known {
        Some("mixed") => format!("{side}: Mixed"),
        Some(v) => match HEADS.iter().find(|(_, value)| *value == v) {
            Some((name, _)) => format!("{side}: {name}"),
            None => format!("{side}: {}", v.replace('_', " ")),
        },
        None => side.to_owned(),
    }
}

fn arrow_heads(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    let new = new_arrows(cx);
    for (kind, side, key, default) in [
        (RibbonMenu::StartHead, "Start", "arrow.start_head", "none"),
        (RibbonMenu::EndHead, "End", "arrow.end_head", "arrow"),
    ] {
        let known = if new {
            Some(arrow_setting(cx, key, default))
        } else {
            cx.model
                .style
                .heads
                .as_ref()
                .map(|h| if kind == RibbonMenu::StartHead { h.start.clone() } else { h.end.clone() })
        };
        let text = head_text(side, known.as_deref());
        menu_button(cx, ui, st, kind, &format!("{side} head"), &text, HEAD_BUTTON_WIDTH);
    }
}

// ---- freehand ----

fn brush(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.str_value("freehand_style").map(str::to_owned);
    for (name, value) in BRUSHES {
        let ctl = style_ctl("freehand_style", value)
            .label(format!("Brush: {name}"))
            .selected(current.as_deref() == Some(value));
        chip(cx, ui, &ctl, name);
    }
}

// ---- text ----

fn font_family(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    let family = cx.model.style.prop("font_family");
    let choice = cx.model.style.str_value("font_family").and_then(UiFontChoice::from_setting);
    let mixed = family.is_some_and(StyleProp::is_mixed);
    let (text, font) = match choice {
        Some(c) => (c.title(), c.preview(TextSize::Body.points())),
        None => (if mixed { "Mixed" } else { "Font" }, TextSize::Body.font()),
    };
    let reason = not_applicable(cx, "font_family");
    let spec =
        Dropdown { label: "Pick the font", text, font, width: FONT_BUTTON_WIDTH, open: st.font_picker_open, reason };
    let resp = dropdown(cx, ui, spec);
    remember_anchor_in(ui, PopoverKey::Font, resp.rect);
    if resp.clicked() {
        cx.verb(Verb::OpenFontPicker);
    }
}

fn font_weight(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.str_value("font_weight").map(str::to_owned);
    for (name, shown, value) in FONT_WEIGHTS {
        let ctl = style_ctl("font_weight", value).label(name).selected(current.as_deref() == Some(value));
        chip(cx, ui, &ctl, shown);
    }
}

fn text_size(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.num_value("font_size");
    for (name, size) in FONT_SIZES {
        let ctl = style_ctl("font_size", size)
            .label(format!("Text size {name} ({size} px)"))
            .selected(current.is_some_and(|c| (c - size).abs() < 1.0));
        chip(cx, ui, &ctl, name);
    }
}

/// Bold and italic for the selected text, or for the part of it the editor has selected: `text.format`. A button
/// that is on turns the face off.
fn emphasis(ui: &mut Ui, cx: &mut Cx<'_>) {
    let Some(faces) = cx.model.faces() else { return };
    for (name, shown, on, arg) in [("Bold text", "B", faces.bold, "bold"), ("Italic text", "I", faces.italic, "italic")]
    {
        let ctl = Ctl::new("text.format").args(json!({ arg: on != Some(true) })).label(name).selected(on == Some(true));
        chip(cx, ui, &ctl, shown);
    }
}

fn text_align(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.str_value("text_align").map(str::to_owned);
    for (name, value, align) in TEXT_ALIGNS {
        let ctl = style_ctl("text_align", value)
            .label(format!("Text align: {name}"))
            .selected(current.as_deref() == Some(value));
        picture(cx, ui, &ctl, move |p, rect, ink| pictograms::text_lines(p, rect, align, ink));
    }
}

const VERTICAL_ALIGNS: [(&str, &str, pictograms::VAlign); 3] = [
    ("Top", "top", pictograms::VAlign::Top),
    ("Middle", "middle", pictograms::VAlign::Middle),
    ("Bottom", "bottom", pictograms::VAlign::Bottom),
];

/// Where text sits in its box (a label of a shape, a fixed-width text).
fn vertical_align(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.str_value("vertical_align").map(str::to_owned);
    for (name, value, align) in VERTICAL_ALIGNS {
        let ctl = style_ctl("vertical_align", value)
            .label(format!("Vertical align: {name}"))
            .selected(current.as_deref() == Some(value));
        picture(cx, ui, &ctl, move |p, rect, ink| pictograms::box_lines(p, rect, align, ink));
    }
}

// ---- more ----

/// The "More" button at the right end of the strip: it opens the popup of `menus.rs`.
pub(crate) fn more_button(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    menu_button(cx, ui, st, RibbonMenu::More, "More options", "More", MORE_BUTTON_WIDTH);
}

// ---- opacity ----

/// The opacity slider. A drag is one gesture (docs/exceptions.md E13): the first change begins it, each
/// change updates it, the release ends it, and Esc cancels it. A click is one plain `style.set`.
fn opacity(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.num_value("opacity").unwrap_or(1.0);
    let mixed = cx.model.style.prop("opacity").is_some_and(StyleProp::is_mixed);
    let mut value = current as f32;
    ui.spacing_mut().slider_width = SLIDER_WIDTH;
    // The rail and the part before the handle must show on the card colour, in both themes.
    let pal = cx.pal;
    let visuals = ui.visuals_mut();
    visuals.widgets.inactive.bg_fill = pal.border;
    visuals.widgets.hovered.bg_fill = pal.border;
    visuals.widgets.active.bg_fill = pal.border;
    visuals.selection.bg_fill = pal.accent;
    visuals.slider_trailing_fill = true;
    let spec = SliderSpec { control: ScrubControl::Opacity, label: "Opacity", range: 0.0..=1.0 };
    scrub::slider(
        ui,
        cx,
        &spec,
        &mut value,
        |v| json!({ "props": { "opacity": (f64::from(v) * 100.0).round() / 100.0 } }),
    );
    let text = if mixed { "mixed".to_owned() } else { format!("{}%", (value * 100.0).round()) };
    ui.add_sized(
        [PERCENT_WIDTH, cx.layout.toggle_height()],
        egui::Label::new(egui::RichText::new(text).size(TextSize::Caption.points()).color(cx.pal.muted)),
    );
}

// ---- arrange ----

fn align(ui: &mut Ui, cx: &mut Cx<'_>) {
    let side = cx.layout.small_side();
    for (icon, edge) in [
        (Icon::AlignLeft, "left"),
        (Icon::AlignCenterX, "center_x"),
        (Icon::AlignRight, "right"),
        (Icon::AlignTop, "top"),
        (Icon::AlignCenterY, "center_y"),
        (Icon::AlignBottom, "bottom"),
    ] {
        cx.icon_button(ui, &Ctl::new("arrange.align").args(json!({ "edge": edge })), icon, side);
    }
    // Distributing needs three elements: the engine says when it can.
    if cx.refusal("arrange.distribute").is_none() {
        ui.add_space(space::XS);
        for (icon, axis) in [(Icon::DistributeH, "horizontal"), (Icon::DistributeV, "vertical")] {
            cx.icon_button(ui, &Ctl::new("arrange.distribute").args(json!({ "axis": axis })), icon, side);
        }
    }
}

fn layer(ui: &mut Ui, cx: &mut Cx<'_>) {
    let side = cx.layout.small_side();
    for (icon, op) in [
        (Icon::LayerBack, "back"),
        (Icon::LayerBackward, "backward"),
        (Icon::LayerForward, "forward"),
        (Icon::LayerFront, "front"),
    ] {
        cx.icon_button(ui, &Ctl::new("arrange.z_order").args(json!({ "op": op })), icon, side);
    }
}

fn flip(ui: &mut Ui, cx: &mut Cx<'_>) {
    let side = cx.layout.small_side();
    for (icon, axis) in [(Icon::FlipH, "horizontal"), (Icon::FlipV, "vertical")] {
        cx.icon_button(ui, &Ctl::new("element.flip").args(json!({ "axis": axis })), icon, side);
    }
}

/// Duplicate, group, ungroup and lock.
fn compose(ui: &mut Ui, cx: &mut Cx<'_>) {
    let side = cx.layout.small_side();
    for (icon, id) in [
        (Icon::Duplicate, "element.duplicate"),
        (Icon::Group, "group.create"),
        (Icon::Ungroup, "group.ungroup"),
        (Icon::Lock, "element.lock"),
    ] {
        cx.icon_button(ui, &Ctl::new(id), icon, side);
    }
}

/// The link (`Edit the link` opens the dialog, `Open the link` follows it; both grey out with the engine's text
/// while their command is refused) and delete.
fn link_and_delete(ui: &mut Ui, cx: &mut Cx<'_>) {
    let side = cx.layout.small_side();
    let reason = cx.refusal("element.set_link");
    let link = cx.model.selection_link.clone();
    let tip = match &link {
        Some(link) => format!("Edit the link ({link})"),
        None => "Edit the link".to_owned(),
    };
    cx.verb_button_gated(ui, Verb::OpenLinkEditor, Icon::Link, side, &tip, link.is_some(), reason);
    cx.icon_button(ui, &Ctl::new("element.open_link").label("Open the link"), Icon::Open, side);
    cx.icon_button(ui, &Ctl::new("element.delete"), Icon::Delete, side);
}
