//! The panel on the right: the properties of the selection, or of the tool being drawn with. Which
//! sections show comes from `style.get`, which says per property whether it applies to the selected
//! elements, and from the kinds in the selection. Every control is a command.

use drafftink_engine::state::Tool;
use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::{Value, json};

use crate::action::{Scrub, ScrubPhase, UiAction, Verb};
use crate::icons::Icon;
use crate::model::StyleView;
use crate::state::PanelKind;
use crate::state::UiState;
use crate::theme::{MARGIN, TextSize, UiFontChoice, space};
use crate::widgets::{Ctl, Cx, PANEL_INSET_Y, panel_frame, scroll_when_taller, section_label, separator};

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
const FONT_FAMILIES: [(&str, &str); 5] = [
    ("GelPen", "gelpen"),
    ("GelPen Serif", "gelpen_serif"),
    ("Vanilla", "vanilla_extract"),
    ("Noto Sans", "noto_sans"),
    ("Plex Sans", "plex_sans"),
];
const FONT_WEIGHTS: [(&str, &str); 3] = [("Light", "light"), ("Regular", "regular"), ("Heavy", "heavy")];
const VERTICAL_ALIGNS: [(&str, &str); 3] = [("Top", "top"), ("Middle", "middle"), ("Bottom", "bottom")];
/// Line heights offered (the property takes 1.0 to 3.0).
pub const LINE_HEIGHTS: [f64; 4] = [1.0, 1.25, 1.5, 2.0];
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

/// The least height the panel scrolls in, on a window too short for it.
const MIN_SCROLL_HEIGHT: f32 = 160.0;

/// True for the tools that draw an element.
pub fn is_drawing_tool(t: Tool) -> bool {
    !matches!(t, Tool::Select | Tool::Lasso | Tool::Hand | Tool::Eraser | Tool::Laser)
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
            matches!(prop, "font_family" | "font_weight" | "font_size" | "text_align" | "line_height" | "opacity")
        }
        Tool::Select | Tool::Lasso | Tool::Hand | Tool::Eraser | Tool::Laser => false,
    }
}

fn shows(cx: &Cx<'_>, prop: &str) -> bool {
    let m = cx.model;
    if m.selection_count > 0 { m.style.applies(prop) } else { tool_shows(m.tool, prop) }
}

/// Draws the panel.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let selected = cx.model.selection_count;
    if selected == 0 && !is_drawing_tool(cx.model.tool) {
        return;
    }
    let screen = ctx.content_rect();
    let collapsed = st.panel_collapsed(PanelKind::Properties, screen.width());
    let compact = cx.layout.compact;
    let side = cx.layout.small_side();
    let width = cx.layout.panel_width();
    // Centred on the window as before, and never taller than the space under the strip: the sections scroll
    // instead. The title, the count and the collapse button stay outside the scrolling part.
    let header =
        TextSize::Heading.points() + 2.0 * space::SM + if compact { cx.layout.tool_side() + space::SM } else { 0.0 };
    let max_height = (st.chrome.centred_panel_height(screen) - 2.0 * PANEL_INSET_Y - header).max(MIN_SCROLL_HEIGHT);
    let area = Area::new(Id::unique("right_panel"))
        .anchor(Align2::RIGHT_CENTER, vec2(-MARGIN, 0.0))
        .order(Order::Middle)
        .show(ctx, |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(space::SM, space::SM);
                if compact {
                    let label = if collapsed { "Show the properties" } else { "Hide the properties" };
                    if cx.local_button(ui, Icon::Settings, cx.layout.tool_side(), label, !collapsed).clicked() {
                        st.set_panel_collapsed(PanelKind::Properties, !collapsed);
                    }
                }
                if collapsed {
                    return;
                }
                ui.set_width(width - 20.0);
                let title = if selected > 0 { "Properties" } else { "New elements" };
                ui.label(egui::RichText::new(title).font(TextSize::Heading.hand()).color(cx.pal.text));
                if selected > 1 {
                    ui.label(
                        egui::RichText::new(format!("{selected} selected"))
                            .size(TextSize::Caption.points())
                            .color(cx.pal.muted),
                    );
                }
                scroll_when_taller(ui, "props_scroll", max_height, |ui| {
                    ui.spacing_mut().item_spacing = vec2(space::SM, space::SM);
                    sections(ui, cx, side);
                });
            });
        });
    st.chrome.props = Some(area.response.rect);
}

fn sections(ui: &mut egui::Ui, cx: &mut Cx<'_>, side: f32) {
    let selected = cx.model.selection_count;
    text_sections(ui, cx);
    shape_sections(ui, cx);
    arrow_sections(ui, cx);
    freehand_sections(ui, cx);
    if selected > 0 {
        opacity_section(ui, cx);
        layer_section(ui, cx, side);
        align_section(ui, cx, side);
        link_section(ui, cx, side);
        action_section(ui, cx, side);
    } else if shows(cx, "opacity") {
        opacity_section(ui, cx);
    }
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
        crate::surfaces::color::remember_anchor_in(ui, crate::surfaces::color::PopoverKey::Font, label.rect);
        let current = cx.model.style.str_value("font_family").map(str::to_owned);
        ui.horizontal_wrapped(|ui| {
            for (label, value) in FONT_FAMILIES {
                let ctl = Ctl::new("style.set")
                    .args(json!({ "props": { "font_family": value } }))
                    .label(label)
                    .selected(current.as_deref() == Some(value));
                let font = match UiFontChoice::from_setting(value) {
                    Some(choice) => choice.preview(TextSize::Body.points()),
                    None => TextSize::Body.font(),
                };
                cx.toggle_button_font(ui, &ctl, label, font);
            }
        });
    }
    choices(ui, cx, "Weight", "font_weight", &FONT_WEIGHTS);
    emphasis_section(ui, cx);
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
    choices(ui, cx, "Vertical align", "vertical_align", &VERTICAL_ALIGNS);
    line_height_section(ui, cx);
    auto_resize_section(ui, cx);
}

/// Bold and italic for the selected text, or for the part of it the editor has selected: `text.format`. A button
/// that is on turns the face off.
fn emphasis_section(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    let Some(faces) = cx.model.faces() else { return };
    section_label(ui, &cx.pal, "Emphasis");
    ui.horizontal(|ui| {
        // The labels differ from the stroke width "Bold" on the same panel.
        for (label, shown, on, arg) in
            [("Bold text", "Bold", faces.bold, "bold"), ("Italic text", "Italic", faces.italic, "italic")]
        {
            let ctl =
                Ctl::new("text.format").args(json!({ arg: on != Some(true) })).label(label).selected(on == Some(true));
            cx.toggle_button(ui, &ctl, shown);
        }
    });
}

/// The line height presets: `style.set` `line_height`.
fn line_height_section(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    if !shows(cx, "line_height") {
        return;
    }
    section_label(ui, &cx.pal, "Line height");
    let current = cx.model.style.num_value("line_height");
    ui.horizontal_wrapped(|ui| {
        for height in LINE_HEIGHTS {
            let ctl = Ctl::new("style.set")
                .args(json!({ "props": { "line_height": height } }))
                .label(format!("Line height {height}"))
                .selected(current.is_some_and(|c| (c - height).abs() < 0.01));
            cx.toggle_button(ui, &ctl, &format!("{height}"));
        }
    });
}

/// Whether the text box follows its content: `text.set_autoresize`.
fn auto_resize_section(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    if cx.model.selection_count == 0 || !cx.model.style.has_kind("text") {
        return;
    }
    section_label(ui, &cx.pal, "Box");
    ui.horizontal_wrapped(|ui| {
        for (label, on) in [("Fit to text", true), ("Fixed width", false)] {
            let ctl = Ctl::new("text.set_autoresize").args(json!({ "on": on })).label(label);
            cx.toggle_button(ui, &ctl, label);
        }
    });
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

/// The path type and heads of new arrows, for the arrow tool with nothing selected: `tool.set_arrow`, with the
/// current choice read from the settings the command writes.
fn new_arrow_sections(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    let word = |cx: &Cx<'_>, key: &str, default: &str| {
        cx.model.settings.get(key).and_then(Value::as_str).unwrap_or(default).to_owned()
    };
    let path = word(cx, "arrow.type", "sharp");
    section_label(ui, &cx.pal, "Path");
    ui.horizontal_wrapped(|ui| {
        for (label, value) in ARROW_TYPES {
            let ctl = Ctl::new("tool.set_arrow")
                .args(json!({ "arrow_type": value }))
                .label(format!("New arrows: {label}"))
                .selected(path == value);
            cx.toggle_button(ui, &ctl, label);
        }
    });
    for (title, arg, key, default) in
        [("Start", "start_head", "arrow.start_head", "none"), ("End", "end_head", "arrow.end_head", "arrow")]
    {
        let current = word(cx, key, default);
        section_label(ui, &cx.pal, &format!("{title} head"));
        ui.horizontal_wrapped(|ui| {
            for (label, value) in HEADS {
                let ctl = Ctl::new("tool.set_arrow")
                    .args(json!({ arg: value }))
                    .label(format!("New arrows, {title} head: {label}"))
                    .selected(current == value);
                cx.toggle_button(ui, &ctl, label);
            }
        });
    }
}

fn arrow_sections(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    if cx.model.selection_count == 0 && cx.model.tool == Tool::Arrow {
        new_arrow_sections(ui, cx);
        return;
    }
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
        ui.label(egui::RichText::new(text).size(TextSize::Caption.points()).color(cx.pal.muted));
        let args = json!({ "props": { "opacity": (f64::from(value) * 100.0).round() / 100.0 } });
        if resp.changed() {
            let phase = if cx.model.gesture_running { ScrubPhase::Update } else { ScrubPhase::Begin };
            cx.emit(UiAction::Scrub(Scrub { phase, command: "style.set", args }));
        } else if resp.drag_stopped() {
            cx.emit(UiAction::Scrub(Scrub { phase: ScrubPhase::End, command: "style.set", args }));
        }
    });
}

fn layer_section(ui: &mut egui::Ui, cx: &mut Cx<'_>, side: f32) {
    section_label(ui, &cx.pal, "Layer");
    ui.horizontal(|ui| {
        for (icon, op) in [
            (Icon::LayerBack, "back"),
            (Icon::LayerBackward, "backward"),
            (Icon::LayerForward, "forward"),
            (Icon::LayerFront, "front"),
        ] {
            let ctl = Ctl::new("arrange.z_order").args(json!({ "op": op }));
            cx.icon_button(ui, &ctl, icon, side);
        }
    });
    section_label(ui, &cx.pal, "Flip");
    ui.horizontal(|ui| {
        for (icon, axis) in [(Icon::FlipH, "horizontal"), (Icon::FlipV, "vertical")] {
            let ctl = Ctl::new("element.flip").args(json!({ "axis": axis }));
            cx.icon_button(ui, &ctl, icon, side);
        }
    });
}

fn align_section(ui: &mut egui::Ui, cx: &mut Cx<'_>, side: f32) {
    section_label(ui, &cx.pal, "Align");
    ui.horizontal_wrapped(|ui| {
        for (icon, edge) in [
            (Icon::AlignLeft, "left"),
            (Icon::AlignCenterX, "center_x"),
            (Icon::AlignRight, "right"),
            (Icon::AlignTop, "top"),
            (Icon::AlignCenterY, "center_y"),
            (Icon::AlignBottom, "bottom"),
        ] {
            let ctl = Ctl::new("arrange.align").args(json!({ "edge": edge }));
            cx.icon_button(ui, &ctl, icon, side);
        }
    });
    ui.horizontal(|ui| {
        for (icon, axis) in [(Icon::DistributeH, "horizontal"), (Icon::DistributeV, "vertical")] {
            let ctl = Ctl::new("arrange.distribute").args(json!({ "axis": axis }));
            cx.icon_button(ui, &ctl, icon, side);
        }
    });
}

/// The link of the selected element: the address, Open (`element.open_link`) and the link dialog. The controls
/// are always there and grey out with the engine's text while their command is refused.
fn link_section(ui: &mut egui::Ui, cx: &mut Cx<'_>, side: f32) {
    section_label(ui, &cx.pal, "Link");
    ui.horizontal_wrapped(|ui| {
        let reason = cx.refusal("element.set_link");
        let has_link = cx.model.selection_link.is_some();
        cx.verb_button_gated(ui, Verb::OpenLinkEditor, Icon::Link, side, "Edit the link", has_link, reason);
        if let Some(link) = cx.model.selection_link.clone() {
            let shown: String = link.chars().take(LINK_CHARS).collect();
            ui.label(egui::RichText::new(shown).size(TextSize::Caption.points()).color(cx.pal.muted));
        }
        let open = Ctl::new("element.open_link").label("Open the link");
        cx.text_button(ui, &open, "Open", false);
    });
}

/// The longest link shown in the panel, in characters.
const LINK_CHARS: usize = 24;

fn action_section(ui: &mut egui::Ui, cx: &mut Cx<'_>, side: f32) {
    separator(ui, &cx.pal);
    ui.horizontal_wrapped(|ui| {
        for (icon, id) in [
            (Icon::Duplicate, "element.duplicate"),
            (Icon::Group, "group.create"),
            (Icon::Ungroup, "group.ungroup"),
            (Icon::Lock, "element.lock"),
            (Icon::Delete, "element.delete"),
        ] {
            let ctl = Ctl::new(id);
            cx.icon_button(ui, &ctl, icon, side);
        }
    });
}
