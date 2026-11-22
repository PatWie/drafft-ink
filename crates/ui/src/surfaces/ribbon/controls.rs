//! The controls of each ribbon group. Every one is a command (`style.set` and the others of the catalog) or a
//! UI verb; the choices that only the UI knows (which menu is open) live in `UiState`.

use drafftink_engine::state::Tool;
use drafftink_scene::{Head, MarkerColor};
use egui::{Color32, FontId, Painter, Rect, Response, Role, Sense, Stroke, Ui, WidgetInfo, pos2, vec2};
use serde_json::{Value, json};

use super::heads::{End, HeadChoice, head_sample};
use super::paint;
use super::pictograms::{self, Align, LineStyle, Pattern};
use super::{Group, Row, shows};
use crate::action::Verb;
use crate::icons::{self, Icon};
use crate::model::{MarkerState, StyleProp};
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
/// The largest radius the corner slider reaches. A shape never rounds more than half its shorter side, and the
/// command takes more (`style.set` allows up to the scene's limit), which the slider shows at its end.
const CORNER_SLIDER_MAX: f32 = 100.0;
/// The width of the corner radius slider in its menu.
const CORNER_SLIDER_WIDTH: f32 = 140.0;
/// The width reserved for the radius next to its slider ("mixed").
const RADIUS_TEXT_WIDTH: f32 = 40.0;
/// Line heights offered (the property takes 1.0 to 3.0).
pub(super) const LINE_HEIGHTS: [f64; 4] = [1.0, 1.25, 1.5, 2.0];
/// Width of the font button, wide enough for the longest family name.
const FONT_BUTTON_WIDTH: f32 = 124.0;
/// Width of the "More" button.
pub(crate) const MORE_BUTTON_WIDTH: f32 = 64.0;
/// Width of a head button: the side, a picture of the head and the chevron.
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
pub(super) const ARROW_TYPES: [(&str, &str); 4] =
    [("Sharp", "sharp"), ("Round", "round"), ("Elbow", "elbow"), ("Auto route", "auto_route")];
const BRUSHES: [(&str, &str); 2] = [("Normal", "normal"), ("Calligraphy", "calligraphy")];
const TIPS: [(&str, &str); 2] = [("Round", "round"), ("Chisel", "chisel")];
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
            Row::StrokeStyle => {
                stroke_style(ui, cx);
                highlight_in(Row::StrokeStyle, ui, cx, st);
            }
            Row::FillPattern => fill_pattern(ui, cx),
            Row::Corners => corners(ui, cx, st),
            Row::ArrowPath => arrow_path(ui, cx),
            Row::ArrowHeads => arrow_heads(ui, cx, st),
            Row::Brush => brush(ui, cx),
            Row::Tip => tip(ui, cx),
            Row::Marker => marker(ui, cx, st),
            Row::FontFamily => font_family(ui, cx, st),
            Row::FontWeight => font_weight(ui, cx),
            Row::TextSize => text_size(ui, cx),
            Row::Emphasis => {
                emphasis(ui, cx);
                highlight_in(Row::Emphasis, ui, cx, st);
            }
            Row::TextAlign => text_align(ui, cx),
            Row::VerticalAlign => vertical_align(ui, cx),
            Row::Opacity => {
                opacity(ui, cx);
                highlight_in(Row::Opacity, ui, cx, st);
            }
            Row::Align => align(ui, cx),
            Row::Layer => layer(ui, cx),
            Row::Flip => flip(ui, cx),
            Row::Compose => compose(ui, cx),
            Row::LinkDelete => link_and_delete(ui, cx),
            Row::ImageEdit => image_edit(ui, cx),
            Row::ImageReset => image_reset(ui, cx),
            Row::BrushShape => paint::size_and_hardness(ui, cx),
            Row::BrushOpacityDone => paint::opacity_reset_done(ui, cx),
        }
    }
}

// ---- building blocks ----

/// A small text button of a segmented choice, filled with the accent when it is on.
pub(crate) fn chip(cx: &mut Cx<'_>, ui: &mut Ui, ctl: &Ctl<'_>, text: &str) -> Response {
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
    /// A picture of an arrowhead between the text and the chevron, and the tooltip that names the head.
    head: Option<HeadView>,
}

/// The arrowhead a [`Dropdown`] shows.
struct HeadView {
    head: Head,
    end: End,
    /// What the tooltip says: the button's name and the head's.
    tooltip: String,
}

/// A button with a text and a chevron that opens something. The caller reacts to the click.
/// `reason` greys it with that tooltip.
fn dropdown(cx: &Cx<'_>, ui: &mut Ui, spec: Dropdown<'_>) -> Response {
    let Dropdown { label, text, font, width, open, reason, head } = spec;
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
                let text_right = at.x + galley.size().x;
                ui.painter().with_clip_rect(clip).galley(at, galley, ink);
                if let Some(view) = &head {
                    let room = Rect::from_min_max(
                        pos2(text_right + space::SM, rect.top() + space::SM),
                        pos2(chevron.left() - space::XS, rect.bottom() - space::SM),
                    );
                    head_sample(ui.painter(), room, view.head, view.end, ink);
                }
            }
            resp.widget_info(|| {
                let mut info = WidgetInfo::labeled(Role::Button, enabled, label);
                info.selected = Some(open);
                info
            });
            resp
        })
        .inner;
    match (reason, head) {
        (Some(r), _) => resp.on_disabled_hover_text(r),
        (None, Some(view)) => resp.on_hover_text(view.tooltip),
        (None, None) => resp.on_hover_text(label),
    }
}

/// A [`dropdown`] that opens a ribbon menu (`menus.rs`) under itself.
struct MenuButton<'a> {
    kind: RibbonMenu,
    /// The accessible name.
    label: &'a str,
    text: &'a str,
    width: f32,
    head: Option<HeadView>,
}

fn menu_button(cx: &Cx<'_>, ui: &mut Ui, st: &mut UiState, button: MenuButton<'_>) -> Response {
    let MenuButton { kind, label, text, width, head } = button;
    let open = st.ribbon_menu.is_some_and(|m| m.kind == kind);
    let spec = Dropdown { label, text, font: TextSize::Small.font(), width, open, reason: None, head };
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

fn corners(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    for (name, value) in CORNERS {
        let ctl = corner_ctl(cx, name, value);
        let rounded = value > 0.0;
        picture(cx, ui, &ctl, move |p, rect, ink| pictograms::corner(p, rect, rounded, ink));
    }
    corner_radius_button(ui, cx, st);
}

/// The small button after the presets that opens the radius slider: the sliders icon, its name and the radius in
/// the tooltip, selected while the menu is open or the radius is none of the presets.
fn corner_radius_button(ui: &mut Ui, cx: &Cx<'_>, st: &mut UiState) {
    let open = st.ribbon_menu.is_some_and(|m| m.kind == RibbonMenu::CornerRadius);
    let side = cx.layout.small_side();
    let pal = cx.pal;
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    let look = Look {
        hovered: resp.hovered() || resp.has_focus(),
        pressed: resp.is_pointer_button_down_on(),
        enabled: true,
        selected: open,
    };
    if ui.is_rect_visible(rect) {
        paint_button_bg(ui.painter(), rect, look, &pal, true);
        pictograms::radius_slider(ui.painter(), rect.shrink(side * 0.22), icon_color(look, &pal));
    }
    let tip = format!("Corner radius: {}", radius_text(cx));
    resp.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::Button, true, "Corner radius");
        info.selected = Some(open);
        info
    });
    let resp = resp.on_hover_text(tip);
    if !ui.is_sizing_pass() {
        if resp.clicked() {
            st.close_overlays();
            st.ribbon_menu = Some(OpenRibbonMenu { kind: RibbonMenu::CornerRadius, anchor: resp.rect });
        } else if open {
            st.ribbon_menu = Some(OpenRibbonMenu { kind: RibbonMenu::CornerRadius, anchor: resp.rect });
        }
    }
}

/// The radius as the button and the slider's value show it: a whole number, or `mixed`.
fn radius_text(cx: &Cx<'_>) -> String {
    if cx.model.style.prop("corner_radius").is_some_and(StyleProp::is_mixed) {
        "mixed".to_owned()
    } else {
        format!("{}", cx.model.style.num_value("corner_radius").unwrap_or(0.0).round())
    }
}

/// The corner radius slider and its value: any radius, where the two buttons give the presets. A drag is one
/// gesture (docs/exceptions.md E13), so it is one undo step; with several shapes of different radii the value says
/// `mixed`.
pub(super) fn corner_slider(ui: &mut Ui, cx: &mut Cx<'_>) {
    let mut value = cx.model.style.num_value("corner_radius").unwrap_or(0.0) as f32;
    ui.spacing_mut().slider_width = CORNER_SLIDER_WIDTH;
    style_slider_rail(ui, cx);
    let spec =
        SliderSpec { control: ScrubControl::CornerRadius, label: "Corner radius", range: 0.0..=CORNER_SLIDER_MAX };
    scrub::slider(ui, cx, &spec, &mut value, |v| json!({ "props": { "corner_radius": v.round() } }));
    let text = radius_text(cx);
    ui.add_sized(
        [RADIUS_TEXT_WIDTH, cx.layout.toggle_height()],
        egui::Label::new(egui::RichText::new(text).size(TextSize::Caption.points()).color(cx.pal.muted)),
    );
}

/// The rail and the part before the handle of a slider must show on the card colour, in both themes.
pub(super) fn style_slider_rail(ui: &mut Ui, cx: &Cx<'_>) {
    let pal = cx.pal;
    let visuals = ui.visuals_mut();
    visuals.widgets.inactive.bg_fill = pal.border;
    visuals.widgets.hovered.bg_fill = pal.border;
    visuals.widgets.active.bg_fill = pal.border;
    visuals.selection.bg_fill = pal.accent;
    visuals.slider_trailing_fill = true;
}

/// The two corner choices: sharp (radius 0) and round.
pub(super) const CORNERS: [(&str, f64); 2] = [("Sharp", 0.0), ("Round", ROUND_CORNER)];

/// The control of one corner choice, selected when the selection (or the defaults) has exactly that radius.
pub(super) fn corner_ctl(cx: &Cx<'_>, name: &str, value: f64) -> Ctl<'static> {
    let selected = cx.model.style.num_value("corner_radius").is_some_and(|r| (r - value).abs() < 0.5);
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

/// What a head button shows: the side, and the head when it is known (new arrows, a selected arrow). `Mixed`
/// arrows have no one head to draw.
fn head_view(side: &str, known: Option<&str>, end: End) -> (String, Option<HeadView>) {
    match known {
        Some("mixed") => (format!("{side}: Mixed"), None),
        Some(value) => match HeadChoice::named(value) {
            Some(choice) => {
                let tooltip = format!("{side} head: {}", choice.name);
                (side.to_owned(), Some(HeadView { head: choice.head, end, tooltip }))
            }
            None => (format!("{side}: {}", value.replace('_', " ")), None),
        },
        None => (side.to_owned(), None),
    }
}

fn arrow_heads(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    let new = new_arrows(cx);
    for (kind, side, key, default, end) in [
        (RibbonMenu::StartHead, "Start", "arrow.start_head", "none", End::Start),
        (RibbonMenu::EndHead, "End", "arrow.end_head", "arrow", End::End),
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
        let (text, head) = head_view(side, known.as_deref(), end);
        let label = format!("{side} head");
        menu_button(cx, ui, st, MenuButton { kind, label: &label, text: &text, width: HEAD_BUTTON_WIDTH, head });
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

fn tip(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.str_value("tip").map(str::to_owned);
    for (name, value) in TIPS {
        let ctl = style_ctl("tip", value).label(format!("Tip: {name}")).selected(current.as_deref() == Some(value));
        chip(cx, ui, &ctl, name);
    }
}

/// The colour of the highlighter tool: the split button of the highlight control, with the same colours and "none"
/// in its popover. A pick is `tool.set_marker`; "none" makes a drag take the highlight off.
fn marker(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    let state = cx.model.tool_marker.map_or(MarkerState::Off, MarkerState::On);
    highlight_split(ui, cx, st, state, Pick::Tool);
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
    let spec = Dropdown {
        label: "Pick the font",
        text,
        font,
        width: FONT_BUTTON_WIDTH,
        open: st.font_picker_open,
        reason,
        head: None,
    };
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
    menu_button(
        cx,
        ui,
        st,
        MenuButton {
            kind: RibbonMenu::More,
            label: "More options",
            text: "More",
            width: MORE_BUTTON_WIDTH,
            head: None,
        },
    );
}

// ---- opacity ----

/// The opacity slider. A drag is one gesture (docs/exceptions.md E13): the first change begins it, each
/// change updates it, the release ends it, and Esc cancels it. A click is one plain `style.set`.
fn opacity(ui: &mut Ui, cx: &mut Cx<'_>) {
    let current = cx.model.style.num_value("opacity").unwrap_or(1.0);
    let mixed = cx.model.style.prop("opacity").is_some_and(StyleProp::is_mixed);
    let mut value = current as f32;
    ui.spacing_mut().slider_width = SLIDER_WIDTH;
    style_slider_rail(ui, cx);
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

// ---- highlight ----

/// Width of the little arrow part of the highlight button.
const MARKER_ARROW_WIDTH: f32 = 14.0;

/// Whether the highlight button is on the strip now: something is selected that can be highlighted.
fn has_highlight(cx: &Cx<'_>) -> bool {
    cx.model.selection_count > 0 && !cx.model.paint_editing && cx.model.marker().is_some()
}

/// The row that holds the highlight button. It makes no row of its own: it goes at the end of the first of these
/// that the strip shows now. The stroke style row has room (three buttons) for everything with an outline; a text
/// has no outline, so the bold and italic row (also two buttons) is its place; a formula has neither, and the opacity
/// row is.
fn highlight_home(cx: &Cx<'_>) -> Option<Row> {
    if !has_highlight(cx) {
        None
    } else if shows(cx, "stroke_style") {
        Some(Row::StrokeStyle)
    } else if cx.model.faces().is_some() {
        Some(Row::Emphasis)
    } else {
        Some(Row::Opacity)
    }
}

/// Draws the highlight button when `row` is its home.
fn highlight_in(row: Row, ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    if highlight_home(cx) == Some(row) {
        ui.add_space(space::XS);
        highlight(ui, cx, st);
    }
}

/// The highlight button: a marker with a bar of the current colour under it, and a small arrow that opens the five
/// colours and "none". Clicking the marker puts the last colour on the selection, or takes the highlight off when it
/// has one. It is `style.set` with `highlight`, so it works for the characters selected in a text edit too.
fn highlight(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(state) = cx.model.marker().filter(|_| has_highlight(cx)) else { return };
    highlight_split(ui, cx, st, state, Pick::Style);
}

/// What a pick of the highlight button or of its colours runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pick {
    /// `style.set` with `highlight`: the selection.
    Style,
    /// `tool.set_marker`: the colour of the highlighter tool.
    Tool,
}

impl Pick {
    /// The command that puts `colour` (`null` is none) where this pick goes.
    pub(super) fn ctl(self, colour: Value) -> Ctl<'static> {
        match self {
            Pick::Style => Ctl::new("style.set").args(json!({ "props": { "highlight": colour } })),
            Pick::Tool => Ctl::new("tool.set_marker").args(json!({ "color": colour })),
        }
    }
}

fn highlight_split(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, state: MarkerState, pick: Pick) {
    if let MarkerState::On(shown) = state {
        st.last_marker = Some(shown);
    }
    let colour = st.last_marker.unwrap_or(MarkerColor::Yellow);
    let (value, tip) = match state {
        MarkerState::On(_) if pick == Pick::Tool => (Value::Null, "Highlighter: none".to_owned()),
        MarkerState::On(_) => (Value::Null, "Take the highlight off".to_owned()),
        MarkerState::Off | MarkerState::Mixed if pick == Pick::Tool => {
            (json!(colour.name()), format!("Highlighter: {}", colour.name()))
        }
        MarkerState::Off | MarkerState::Mixed => (json!(colour.name()), format!("Highlight ({})", colour.name())),
    };
    let ctl = pick.ctl(value).label(tip).selected(matches!(state, MarkerState::On(_)));
    let (pal, side) = (cx.pal, cx.layout.small_side());
    cx.command_widget(ui, &ctl, vec2(side, side), move |p, rect, look| {
        paint_button_bg(p, rect, look, &pal, true);
        let ink = icon_color(look, &pal);
        pictograms::marker_pen(p, rect.shrink2(vec2(side * 0.2, side * 0.12)), ink);
        // The bar under the pen: the marker colour, or the outline of an empty one.
        let bar = Rect::from_min_max(
            pos2(rect.left() + side * 0.2, rect.bottom() - side * 0.2),
            pos2(rect.right() - side * 0.2, rect.bottom() - side * 0.08),
        );
        match state {
            MarkerState::On(m) => pictograms::marker_swatch(p, bar, Some(m), pal.dark, ink),
            MarkerState::Mixed => {
                p.rect_stroke(bar, 2.0, Stroke::new(1.0, pal.muted), egui::StrokeKind::Inside);
            }
            MarkerState::Off => pictograms::marker_swatch(p, bar, Some(colour), pal.dark, ink),
        }
    });
    marker_arrow(ui, cx, st);
}

/// The small arrow beside the highlight button; it opens the colours.
fn marker_arrow(ui: &mut Ui, cx: &Cx<'_>, st: &mut UiState) {
    let open = st.ribbon_menu.is_some_and(|m| m.kind == RibbonMenu::Highlight);
    let (pal, height) = (cx.pal, cx.layout.small_side());
    let (rect, resp) = ui.allocate_exact_size(vec2(MARKER_ARROW_WIDTH, height), Sense::click());
    let look = Look {
        hovered: resp.hovered() || resp.has_focus(),
        pressed: resp.is_pointer_button_down_on(),
        enabled: true,
        selected: open,
    };
    if ui.is_rect_visible(rect) {
        paint_button_bg(ui.painter(), rect, look, &pal, true);
        let chevron = Rect::from_center_size(rect.center(), vec2(10.0, 10.0));
        icons::paint(ui.painter(), chevron, Icon::ChevronDown, icon_color(look, &pal));
    }
    resp.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::Button, true, "Highlight colours");
        info.selected = Some(open);
        info
    });
    let resp = resp.on_hover_text("Highlight colours");
    if !ui.is_sizing_pass() {
        if resp.clicked() {
            st.close_overlays();
            st.ribbon_menu = Some(OpenRibbonMenu { kind: RibbonMenu::Highlight, anchor: resp.rect });
        } else if open {
            st.ribbon_menu = Some(OpenRibbonMenu { kind: RibbonMenu::Highlight, anchor: resp.rect });
        }
    }
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

/// The crop editor can be opened, or is open (its chip then says Done).
pub(super) fn can_crop(cx: &Cx<'_>) -> bool {
    cx.model.crop_editing || cx.refusal("image.crop_begin").is_none()
}

/// The paint editor can be opened on the selection. It is not offered while the crop editor is open: the two never
/// run together, and the chip next to Done would only close the crop.
pub(super) fn can_paint(cx: &Cx<'_>) -> bool {
    !cx.model.crop_editing && cx.refusal("image.paint_begin").is_none()
}

/// The cropped picture can be shown whole again.
pub(super) fn can_reset_crop(cx: &Cx<'_>) -> bool {
    cx.refusal("image.reset_crop").is_none()
}

/// The paint can be taken off the picture (it is not offered while the crop editor is open).
pub(super) fn can_reset_paint(cx: &Cx<'_>) -> bool {
    !cx.model.crop_editing && cx.refusal("image.reset_paint").is_none()
}

/// Crop the selected image, or, while its crop editor is open, finish; and open the paint editor. The labels are
/// the registry's titles.
fn image_edit(ui: &mut Ui, cx: &mut Cx<'_>) {
    if can_crop(cx) {
        let (id, icon) =
            if cx.model.crop_editing { ("image.crop_end", Icon::Check) } else { ("image.crop_begin", Icon::Crop) };
        chip_with_icon(ui, cx, &Ctl::new(id), icon);
    }
    if can_paint(cx) {
        paint::edit_picture(ui, cx);
    }
}

/// Shows the whole picture of the cropped image again, and takes the paint off the painted one.
fn image_reset(ui: &mut Ui, cx: &mut Cx<'_>) {
    if can_reset_crop(cx) {
        chip_with_icon(ui, cx, &Ctl::new("image.reset_crop"), Icon::CropReset);
    }
    if can_reset_paint(cx) {
        paint::reset_paint(ui, cx);
    }
}

/// A small button with an icon and the registry's title of its command beside it.
pub(super) fn chip_with_icon(ui: &mut Ui, cx: &mut Cx<'_>, ctl: &Ctl<'_>, icon: Icon) -> Response {
    let pal = cx.pal;
    let title = cx.model.title(ctl.id).to_owned();
    let galley = ui.painter().layout_no_wrap(title, TextSize::Small.font(), pal.text);
    let height = cx.layout.small_side();
    let size = vec2(height + galley.size().x + 2.0 * space::MD, height);
    cx.command_widget(ui, ctl, size, move |p, rect, look| {
        paint_button_bg(p, rect, look, &pal, false);
        let color = icon_color(look, &pal);
        let glyph = Rect::from_min_size(rect.min, vec2(height, height)).shrink(height * 0.2);
        icons::paint(p, glyph, icon, color);
        let at = pos2(rect.left() + height + space::XS, rect.center().y - galley.size().y / 2.0);
        p.galley(at, galley, color);
    })
}
