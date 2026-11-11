//! The panel on the right: the properties of the selection, or of the tool being drawn with. Which
//! sections show comes from `style.get`, which says per property whether it applies to the selected
//! elements, and from the kinds in the selection. Every control is a command.

use drafftink_engine::state::Tool;
use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::{Value, json};

use crate::action::{Scrub, ScrubPhase, UiAction};
use crate::icons::Icon;
use crate::model::StyleView;
use crate::state::UiState;
use crate::theme::{self, MARGIN, RIGHT_PANEL_WIDTH};
use crate::widgets::{Ctl, Cx, panel_frame, section_label, separator};

/// Sizes of the text presets.
pub const FONT_SIZES: [(&str, f64); 4] = [("S", 16.0), ("M", 20.0), ("L", 28.0), ("XL", 36.0)];
/// Corner radius of "round".
pub const ROUND_CORNER: f64 = 32.0;

const SLOPPINESS: [(&str, &str); 4] =
    [("Architect", "architect"), ("Artist", "artist"), ("Cartoonist", "cartoonist"), ("Drunk", "drunk")];
const FILL_PATTERNS: [(&str, &str); 7] = [
    ("Solid", "solid"),
    ("Hatch", "hachure"),
    ("Zigzag", "zigzag"),
    ("Cross", "cross_hatch"),
    ("Dots", "dots"),
    ("Dashed", "dashed"),
    ("Zigzag line", "zigzag_line"),
];
const STROKE_STYLES: [(&str, &str); 3] = [("Solid", "solid"), ("Dashed", "dashed"), ("Dotted", "dotted")];
const FONT_FAMILIES: [(&str, &str); 4] = [
    ("GelPen", "gelpen"),
    ("GelPen Serif", "gelpen_serif"),
    ("Vanilla", "vanilla_extract"),
    ("Noto Sans", "noto_sans"),
];
const FONT_WEIGHTS: [(&str, &str); 3] = [("Light", "light"), ("Regular", "regular"), ("Heavy", "heavy")];
const TEXT_ALIGNS: [(&str, &str); 3] = [("Left", "left"), ("Center", "center"), ("Right", "right")];
const ARROW_TYPES: [(&str, &str); 3] = [("Sharp", "sharp"), ("Round", "round"), ("Elbow", "elbow")];
const HEADS: [(&str, &str); 6] = [
    ("None", "none"),
    ("Arrow", "arrow"),
    ("Bar", "bar"),
    ("Dot", "circle"),
    ("Triangle", "triangle"),
    ("Diamond", "diamond"),
];
const BRUSHES: [(&str, &str); 2] = [("Normal", "normal"), ("Calligraphy", "calligraphy")];
const PRESSURE: [(&str, &str); 3] = [("Uniform", "uniform"), ("Simulated", "simulated"), ("Pen", "device")];

/// True for the tools that draw an element.
pub fn is_drawing_tool(t: Tool) -> bool {
    !matches!(t, Tool::Select | Tool::Hand | Tool::Eraser | Tool::Laser)
}

/// Whether a property section is relevant to the tool being drawn with (used only when nothing is
/// selected). This is presentation: the engine skips and reports properties that do not apply.
fn tool_shows(tool: Tool, prop: &str) -> bool {
    match tool {
        Tool::Rectangle | Tool::Diamond | Tool::Frame => !matches!(
            prop,
            "font_family" | "font_weight" | "font_size" | "text_align" | "freehand_style" | "pressure_mode"
        ),
        Tool::Ellipse => !matches!(
            prop,
            "font_family"
                | "font_weight"
                | "font_size"
                | "text_align"
                | "freehand_style"
                | "pressure_mode"
                | "corner_radius"
        ),
        Tool::Line | Tool::Arrow => matches!(prop, "stroke_style" | "sloppiness" | "corner_radius" | "opacity"),
        Tool::Freehand | Tool::Highlighter => {
            matches!(prop, "freehand_style" | "pressure_mode" | "opacity" | "fill_pattern")
        }
        Tool::Text | Tool::Math => {
            matches!(prop, "font_family" | "font_weight" | "font_size" | "text_align" | "opacity")
        }
        Tool::Select | Tool::Hand | Tool::Eraser | Tool::Laser => false,
    }
}

fn shows(cx: &Cx<'_>, prop: &str) -> bool {
    let m = cx.model;
    if m.selection_count > 0 { m.style.applies(prop) } else { tool_shows(m.tool, prop) }
}

/// Draws the panel.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, _st: &mut UiState) {
    let selected = cx.model.selection_count;
    if selected == 0 && !is_drawing_tool(cx.model.tool) {
        return;
    }
    Area::new(Id::unique("right_panel")).anchor(Align2::RIGHT_CENTER, vec2(-MARGIN, 0.0)).order(Order::Middle).show(
        ctx,
        |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.set_width(RIGHT_PANEL_WIDTH - 20.0);
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                let title = if selected > 0 { "Properties" } else { "New elements" };
                ui.label(egui::RichText::new(title).font(theme::hand(16.0)).color(cx.pal.text));
                if selected > 1 {
                    ui.label(egui::RichText::new(format!("{selected} selected")).size(11.0).color(cx.pal.muted));
                }
                text_sections(ui, cx);
                shape_sections(ui, cx);
                arrow_sections(ui, cx);
                freehand_sections(ui, cx);
                if selected > 0 {
                    opacity_section(ui, cx);
                    layer_section(ui, cx);
                    if selected > 1 {
                        align_section(ui, cx);
                    }
                    action_section(ui, cx);
                } else if shows(cx, "opacity") {
                    opacity_section(ui, cx);
                }
            });
        },
    );
}

/// One row of choices for a `style.set` property.
fn choices(ui: &mut egui::Ui, cx: &mut Cx<'_>, title: &str, prop: &str, options: &[(&str, &str)]) {
    if !shows(cx, prop) {
        return;
    }
    section_label(ui, &cx.pal, title);
    let current = cx.model.style.str_value(prop).map(str::to_owned);
    ui.horizontal_wrapped(|ui| {
        for (label, value) in options {
            let ctl = Ctl::new("style.set")
                .args(json!({ "props": { prop: value } }))
                .label(*label)
                .selected(current.as_deref() == Some(*value));
            cx.toggle_button(ui, &ctl, label);
        }
    });
}

fn text_sections(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    if shows(cx, "font_family") {
        let label = section_label(ui, &cx.pal, "Font");
        crate::surfaces::color::remember_anchor(ui.ctx(), crate::surfaces::color::PopoverKey::Font, label.rect);
        let current = cx.model.style.str_value("font_family").map(str::to_owned);
        ui.horizontal_wrapped(|ui| {
            for (label, value) in FONT_FAMILIES {
                let ctl = Ctl::new("style.set")
                    .args(json!({ "props": { "font_family": value } }))
                    .label(label)
                    .selected(current.as_deref() == Some(value));
                let font = match value {
                    "gelpen" => theme::hand(13.0),
                    "gelpen_serif" => theme::named(theme::PREVIEW_SERIF, 13.0),
                    "vanilla_extract" => theme::named(theme::PREVIEW_VANILLA, 13.0),
                    _ => egui::FontId::proportional(12.0),
                };
                cx.toggle_button_font(ui, &ctl, label, font);
            }
        });
    }
    choices(ui, cx, "Weight", "font_weight", &FONT_WEIGHTS);
    if shows(cx, "font_size") {
        section_label(ui, &cx.pal, "Size");
        let current = cx.model.style.num_value("font_size");
        ui.horizontal(|ui| {
            for (label, size) in FONT_SIZES {
                let ctl = Ctl::new("style.set")
                    .args(json!({ "props": { "font_size": size } }))
                    .label(format!("Text size {label} ({size} px)"))
                    .selected(current.is_some_and(|c| (c - size).abs() < 1.0));
                cx.toggle_button(ui, &ctl, label);
            }
        });
    }
    choices(ui, cx, "Align", "text_align", &TEXT_ALIGNS);
}

fn shape_sections(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    if shows(cx, "corner_radius") {
        section_label(ui, &cx.pal, "Corners");
        let r = cx.model.style.num_value("corner_radius");
        ui.horizontal(|ui| {
            for (label, value) in [("Sharp", 0.0), ("Round", ROUND_CORNER)] {
                let selected = r.is_some_and(|r| if value == 0.0 { r < 1.0 } else { r >= 1.0 });
                let ctl = Ctl::new("style.set")
                    .args(json!({ "props": { "corner_radius": value } }))
                    .label(label)
                    .selected(selected);
                cx.toggle_button(ui, &ctl, label);
            }
        });
    }
    choices(ui, cx, "Sloppiness", "sloppiness", &SLOPPINESS);
    let has_fill = !matches!(cx.model.style.prop("fill_color").and_then(|p| p.value.as_ref()), Some(Value::Null));
    if has_fill || cx.model.style.prop("fill_color").is_some_and(StyleProp::is_mixed) {
        choices(ui, cx, "Fill pattern", "fill_pattern", &FILL_PATTERNS);
    }
    choices(ui, cx, "Stroke", "stroke_style", &STROKE_STYLES);
}

use crate::model::StyleProp;

fn arrow_sections(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    let kinds: &StyleView = &cx.model.style;
    if cx.model.selection_count == 0 || !(kinds.has_kind("arrow") || kinds.has_kind("line")) {
        return;
    }
    section_label(ui, &cx.pal, "Path");
    ui.horizontal_wrapped(|ui| {
        for (label, value) in ARROW_TYPES {
            let ctl = Ctl::new("arrow.set_type").args(json!({ "type": value })).label(label);
            cx.toggle_button(ui, &ctl, label);
        }
    });
    if cx.model.style.has_kind("arrow") {
        for (title, end) in [("Start", "start"), ("End", "end")] {
            section_label(ui, &cx.pal, &format!("{title} head"));
            ui.horizontal_wrapped(|ui| {
                for (label, value) in HEADS {
                    let head = if value == "none" { Value::Null } else { json!(value) };
                    let ctl = Ctl::new("arrow.set_head")
                        .args(json!({ "end": end, "head": head }))
                        .label(format!("{title} head: {label}"));
                    cx.toggle_button(ui, &ctl, label);
                }
            });
        }
        let swap = Ctl::new("arrow.swap_heads").label("Swap heads");
        cx.text_button(ui, &swap, "Swap heads", false);
    }
}

fn freehand_sections(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    choices(ui, cx, "Brush", "freehand_style", &BRUSHES);
    choices(ui, cx, "Pressure", "pressure_mode", &PRESSURE);
}

/// The opacity slider. A drag is one gesture (docs/exceptions.md E13): the first change begins it, each
/// change updates it, the release ends it, and Esc cancels it. A click is one plain `style.set`.
fn opacity_section(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    section_label(ui, &cx.pal, "Opacity");
    let current = cx.model.style.num_value("opacity").unwrap_or(1.0);
    let mixed = cx.model.style.prop("opacity").is_some_and(StyleProp::is_mixed);
    let blocked = cx.refusal("style.set");
    let mut value = current as f32;
    ui.horizontal(|ui| {
        let resp = ui
            .add_enabled(blocked.is_none(), egui::Slider::new(&mut value, 0.0..=1.0).show_value(false))
            .on_disabled_hover_text(blocked.clone().unwrap_or_default());
        resp.widget_info(|| egui::WidgetInfo::slider(blocked.is_none(), f64::from(value), "Opacity"));
        let text = if mixed { "mixed".to_owned() } else { format!("{}%", (value * 100.0).round()) };
        ui.label(egui::RichText::new(text).size(11.0).color(cx.pal.muted));
        let args = json!({ "props": { "opacity": (f64::from(value) * 100.0).round() / 100.0 } });
        if resp.changed() {
            let phase = if cx.model.gesture_running { ScrubPhase::Update } else { ScrubPhase::Begin };
            cx.emit(UiAction::Scrub(Scrub { phase, command: "style.set", args }));
        } else if resp.drag_stopped() {
            cx.emit(UiAction::Scrub(Scrub { phase: ScrubPhase::End, command: "style.set", args }));
        }
    });
}

fn layer_section(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    section_label(ui, &cx.pal, "Layer");
    ui.horizontal(|ui| {
        for (icon, op, label) in [
            (Icon::LayerBack, "back", "Send to back"),
            (Icon::LayerBackward, "backward", "Send backward"),
            (Icon::LayerForward, "forward", "Bring forward"),
            (Icon::LayerFront, "front", "Bring to front"),
        ] {
            let ctl = Ctl::new("arrange.z_order").args(json!({ "op": op })).label(label);
            cx.icon_button(ui, &ctl, icon, theme::SMALL_BUTTON);
        }
    });
    section_label(ui, &cx.pal, "Flip");
    ui.horizontal(|ui| {
        for (icon, axis, label) in
            [(Icon::FlipH, "horizontal", "Flip horizontally"), (Icon::FlipV, "vertical", "Flip vertically")]
        {
            let ctl = Ctl::new("element.flip").args(json!({ "axis": axis })).label(label);
            cx.icon_button(ui, &ctl, icon, theme::SMALL_BUTTON);
        }
    });
}

fn align_section(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    section_label(ui, &cx.pal, "Align");
    ui.horizontal_wrapped(|ui| {
        for (icon, edge, label) in [
            (Icon::AlignLeft, "left", "Align left"),
            (Icon::AlignCenterX, "center_x", "Align centres horizontally"),
            (Icon::AlignRight, "right", "Align right"),
            (Icon::AlignTop, "top", "Align top"),
            (Icon::AlignCenterY, "center_y", "Align centres vertically"),
            (Icon::AlignBottom, "bottom", "Align bottom"),
        ] {
            let ctl = Ctl::new("arrange.align").args(json!({ "edge": edge })).label(label);
            cx.icon_button(ui, &ctl, icon, theme::SMALL_BUTTON);
        }
    });
    ui.horizontal(|ui| {
        for (icon, axis, label) in [
            (Icon::DistributeH, "horizontal", "Distribute horizontally"),
            (Icon::DistributeV, "vertical", "Distribute vertically"),
        ] {
            let ctl = Ctl::new("arrange.distribute").args(json!({ "axis": axis })).label(label);
            cx.icon_button(ui, &ctl, icon, theme::SMALL_BUTTON);
        }
    });
}

fn action_section(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    separator(ui, &cx.pal);
    ui.horizontal_wrapped(|ui| {
        for (icon, id, label) in [
            (Icon::Duplicate, "element.duplicate", "Duplicate"),
            (Icon::Group, "group.create", "Group"),
            (Icon::Ungroup, "group.ungroup", "Ungroup"),
            (Icon::Lock, "element.lock", "Lock"),
            (Icon::Delete, "element.delete", "Delete"),
        ] {
            let ctl = Ctl::new(id).label(label);
            cx.icon_button(ui, &ctl, icon, theme::SMALL_BUTTON);
        }
    });
}
