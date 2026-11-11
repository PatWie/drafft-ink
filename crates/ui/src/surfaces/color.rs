//! The colour popover (stroke, fill, page background) and the font family popover.
//!
//! The colour grid is the old one: 22 Tailwind families by shades 50 to 950, 16 px round cells, a quick
//! column. A pick is a plain command: `style.set` for stroke and fill, `doc.set_background` for the
//! page. "None" on the fill is `fill_color: null`, never a transparent colour. The popover closes on a
//! pick, on Esc (the keymap) and on a click outside it.

use drafftink_scene::Color;
use drafftink_scene::palette::{FAMILIES, SHADES};
use egui::{Align2, Area, Color32, Context, FontFamily, FontId, Id, Order, Pos2, Rect, RichText, ScrollArea, Ui, vec2};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::model::Model;
use crate::state::{ColorTarget, UiState};
use crate::surfaces::dialogs::{enter_pressed, text_field};
use crate::theme::{self, MARGIN, RIGHT_PANEL_WIDTH};
use crate::widgets::{Ctl, Cx, Look, paint_button_bg, panel_frame, section_label};

/// Side of a colour cell.
const CELL: f32 = 16.0;
/// Space under the top bar for a popover that has no remembered anchor.
const BELOW_TOP_BAR: f32 = 64.0;
/// Space above the bottom bar for a popover that has no remembered anchor.
const ABOVE_BOTTOM_BAR: f32 = 64.0;
/// Gap between a control and the popover it opened.
const GAP: f32 = 8.0;
/// Height the title, the hex field and the frame take around the grid.
const GRID_CHROME: f32 = 130.0;
/// Width of a row of the font picker.
const FONT_ROW_WIDTH: f32 = 210.0;

/// The controls that open a popover. The surface that draws the control may tell the popover where it is
/// with [`remember_anchor`]; without that the popover uses a default place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopoverKey {
    /// The stroke colour wheel.
    Stroke,
    /// The fill colour wheel.
    Fill,
    /// The background button.
    Background,
    /// The font family button.
    Font,
}

impl PopoverKey {
    fn of(target: ColorTarget) -> Self {
        match target {
            ColorTarget::Stroke => Self::Stroke,
            ColorTarget::Fill => Self::Fill,
            ColorTarget::Background => Self::Background,
        }
    }
}

/// Tells the popover for `key` where its button is, in screen points. Call it every frame the button is
/// drawn. The popover opens below the stroke and fill buttons, above the background button and left of the
/// font button, and is moved back onto the screen when it would not fit.
pub fn remember_anchor(ctx: &Context, key: PopoverKey, rect: Rect) {
    ctx.data_mut(|d| d.insert_temp(Id::unique(("popover_anchor", key)), rect));
}

fn anchor_of(ctx: &Context, key: PopoverKey) -> Option<Rect> {
    ctx.data(|d| d.get_temp::<Rect>(Id::unique(("popover_anchor", key))))
}

/// The area of a popover next to its button, or at a default place.
fn popover_area(ctx: &Context, id: Id, key: PopoverKey) -> Area {
    let area = Area::new(id).order(Order::Foreground);
    match (anchor_of(ctx, key), key) {
        (Some(r), PopoverKey::Stroke | PopoverKey::Fill) => {
            area.pivot(Align2::LEFT_TOP).fixed_pos(r.left_bottom() + vec2(0.0, GAP))
        }
        (Some(r), PopoverKey::Background) => area.pivot(Align2::LEFT_BOTTOM).fixed_pos(r.left_top() - vec2(0.0, GAP)),
        (Some(r), PopoverKey::Font) => area.pivot(Align2::RIGHT_TOP).fixed_pos(r.left_top() - vec2(GAP, 0.0)),
        (None, PopoverKey::Stroke | PopoverKey::Fill) => area.anchor(Align2::CENTER_TOP, vec2(0.0, BELOW_TOP_BAR)),
        (None, PopoverKey::Background) => area.anchor(Align2::LEFT_BOTTOM, vec2(MARGIN, -ABOVE_BOTTOM_BAR)),
        (None, PopoverKey::Font) => {
            area.anchor(Align2::RIGHT_TOP, vec2(-(MARGIN + RIGHT_PANEL_WIDTH + GAP), BELOW_TOP_BAR + MARGIN))
        }
    }
}

/// True when the primary button went down this frame outside `inside`, on the canvas. A press on another
/// control (another colour wheel, a tool) is that control's business: it toggles or switches the popover.
fn pressed_outside(ctx: &Context, inside: Rect) -> bool {
    let pos: Option<Pos2> = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None });
    let Some(pos) = pos else {
        return false;
    };
    if inside.contains(pos) {
        return false;
    }
    ctx.layer_id_at(pos).is_none_or(|layer| layer.order == Order::Background)
}

// ---- what a pick does ----

/// What one cell picks.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pick {
    /// No colour: no fill, a clear stroke, a clear page.
    Clear,
    /// A palette name such as `blue-500`, or a `#rrggbb[aa]` colour.
    Color(String),
}

/// The command a pick runs, as the cell's control sends it.
pub(crate) fn pick_command(target: ColorTarget, pick: &Pick) -> (&'static str, Value) {
    let text = match pick {
        Pick::Clear => "transparent",
        Pick::Color(c) => c.as_str(),
    };
    match target {
        ColorTarget::Stroke => ("style.set", json!({ "props": { "stroke_color": text } })),
        ColorTarget::Fill => {
            let value = if *pick == Pick::Clear { Value::Null } else { Value::from(text) };
            ("style.set", json!({ "props": { "fill_color": value } }))
        }
        ColorTarget::Background => ("doc.set_background", json!({ "color": text })),
    }
}

/// The call a pick makes.
#[cfg(test)]
pub(crate) fn pick_call(target: ColorTarget, pick: &Pick) -> crate::action::Call {
    let (id, args) = pick_command(target, pick);
    crate::action::Call::new(drafftink_engine::Origin::Ui, id, args)
}

/// The colour a pick gives the target: `None` for no fill.
fn pick_value(target: ColorTarget, pick: &Pick) -> Option<Color> {
    match pick {
        Pick::Clear if target == ColorTarget::Fill => None,
        Pick::Clear => Some(Color::TRANSPARENT),
        Pick::Color(c) => drafftink_scene::palette::lookup(c).or_else(|| Color::parse(c).ok()),
    }
}

/// The colour the target has now. The outer `None` means the model does not know (mixed, not applicable);
/// the inner `None` is "no fill". Red, green, blue and alpha all count, so None is not Black.
fn current(model: &Model, target: ColorTarget) -> Option<Option<Color>> {
    match target {
        ColorTarget::Stroke => model.style.str_value("stroke_color").and_then(|s| Color::parse(s).ok()).map(Some),
        ColorTarget::Fill => match model.style.prop("fill_color")?.value.as_ref()? {
            Value::Null => Some(None),
            Value::String(s) if s != "mixed" => Color::parse(s).ok().map(Some),
            _ => None,
        },
        ColorTarget::Background => Color::parse(&model.background).ok().map(Some),
    }
}

/// The text a hex field accepts: `#rgb`, `#rrggbb` or `#rrggbbaa`, with or without the `#`.
pub(crate) fn parse_hex(text: &str) -> Result<Color, String> {
    let t = text.trim();
    let bad = || format!("`{t}` is not a colour. Use #rgb, #rrggbb or #rrggbbaa.");
    let hex = t.strip_prefix('#').unwrap_or(t);
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    Color::parse(&format!("#{hex}")).map_err(|_| bad())
}

// ---- the grid ----

/// One cell of the grid or of the quick column.
struct Choice {
    label: String,
    pick: Pick,
    shown: Option<Color32>,
}

fn to_color32(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

fn capitalised(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|f| f.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The quick column: None, Black, White and seven colours at shade 500.
fn quick_column() -> Vec<Choice> {
    let mut out = vec![
        Choice { label: "None".to_owned(), pick: Pick::Clear, shown: None },
        Choice { label: "Black".to_owned(), pick: Pick::Color("#000000".to_owned()), shown: Some(Color32::BLACK) },
        Choice { label: "White".to_owned(), pick: Pick::Color("#ffffff".to_owned()), shown: Some(Color32::WHITE) },
    ];
    for name in ["blue", "red", "emerald", "amber", "purple", "slate"] {
        let full = format!("{name}-500");
        let shown = drafftink_scene::palette::lookup(&full).map(to_color32);
        out.push(Choice { label: capitalised(name), pick: Pick::Color(full), shown });
    }
    out
}

/// Draws one cell. Returns true when it was clicked (the command is already pushed by the control).
fn cell(ui: &mut Ui, cx: &mut Cx<'_>, target: ColorTarget, now: Option<Option<Color>>, c: &Choice) -> bool {
    let (id, args) = pick_command(target, &c.pick);
    let selected = now.is_some() && now == Some(pick_value(target, &c.pick));
    let ctl = Ctl::new(id).args(args).label(&c.label).selected(selected);
    cx.swatch(ui, &ctl, c.shown, CELL).clicked()
}

/// Draws the popover of a colour target. Returns nothing: picks and the outside click push actions.
fn color_popover(ctx: &Context, cx: &mut Cx<'_>, target: ColorTarget) {
    let pal = cx.pal;
    let key = PopoverKey::of(target);
    let now = current(cx.model, target);
    let quick = quick_column();
    let screen = ctx.content_rect();
    let max_w = (screen.width() - 2.0 * MARGIN - 24.0).max(120.0);
    let max_h = (screen.height() - 2.0 * MARGIN - GRID_CHROME).max(80.0);
    let hex_key = Id::unique("color_hex_draft");
    let mut hex: HexDraft = ctx.data_mut(|d| d.get_temp(hex_key)).unwrap_or_default();
    let mut picked = false;

    let out = popover_area(ctx, Id::unique("color_popover"), key).show(ctx, |ui| {
        panel_frame(&pal).show(ui, |ui| {
            let title = match target {
                ColorTarget::Stroke => "Stroke colour",
                ColorTarget::Fill => "Fill colour",
                ColorTarget::Background => "Background colour",
            };
            ui.label(RichText::new(title).font(theme::hand(14.0)).color(pal.muted));
            ui.add_space(4.0);
            ScrollArea::both().max_width(max_w).max_height(max_h).auto_shrink([true, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                // Shade 25 is too close to white to tell apart in a 16 px cell: the grid starts at 50.
                let shades = SHADES.iter().enumerate().skip(1);
                for (row, (col, shade)) in shades.enumerate() {
                    if row == 1 || row == 5 || row == 6 {
                        ui.add_space(4.0);
                    }
                    ui.horizontal(|ui| {
                        for fam in FAMILIES {
                            let (r, g, b) = fam.shades.get(col).copied().unwrap_or((0, 0, 0));
                            let c = Choice {
                                label: format!("{} {shade}", capitalised(fam.name)),
                                pick: Pick::Color(format!("{}-{shade}", fam.name)),
                                shown: Some(Color32::from_rgb(r, g, b)),
                            };
                            picked |= cell(ui, cx, target, now, &c);
                        }
                        if let Some(q) = quick.get(row) {
                            ui.add_space(8.0);
                            picked |= cell(ui, cx, target, now, q);
                        }
                    });
                }
            });
            ui.add_space(8.0);
            picked |= hex_row(ui, cx, target, &mut hex);
        });
    });

    if picked {
        hex = HexDraft::default();
        cx.verb(Verb::CloseOverlays);
    } else if pressed_outside(ctx, out.response.rect) {
        cx.verb(Verb::CloseOverlays);
    }
    ctx.data_mut(|d| d.insert_temp(hex_key, hex));
}

#[derive(Clone, Default)]
struct HexDraft {
    text: String,
    error: Option<String>,
}

/// The hex field. Enter applies a valid value; a bad one shows an error under the field.
fn hex_row(ui: &mut Ui, cx: &mut Cx<'_>, target: ColorTarget, draft: &mut HexDraft) -> bool {
    let pal = cx.pal;
    let (id, _) = pick_command(target, &Pick::Clear);
    let reason = cx.refusal(id);
    section_label(ui, &pal, "Hex");
    let resp = text_field(ui, &mut draft.text, "#rrggbb, then Enter", reason.clone(), Some(9), "Hex colour");
    let mut applied = false;
    if reason.is_none() && resp.lost_focus() && enter_pressed(ui) {
        match parse_hex(&draft.text) {
            Ok(color) => {
                let (id, args) = pick_command(target, &Pick::Color(color.to_hex()));
                cx.run(id, args);
                applied = true;
            }
            Err(message) => draft.error = Some(message),
        }
    }
    if let Some(err) = &draft.error {
        ui.label(RichText::new(err).size(12.0).color(pal.danger));
    }
    applied
}

// ---- fonts ----

/// The font families of the canvas: setting value, name, and the family that previews it.
fn font_choices() -> [(&'static str, &'static str, FontFamily); 4] {
    [
        ("gelpen", "GelPen", FontFamily::Name(theme::HAND_FONT.into())),
        ("gelpen_serif", "GelPen Serif", FontFamily::Name(theme::PREVIEW_SERIF.into())),
        ("vanilla_extract", "Vanilla Extract", FontFamily::Name(theme::PREVIEW_VANILLA.into())),
        ("noto_sans", "Noto Sans", FontFamily::Proportional),
    ]
}

/// The arguments of `style.set` for a font family.
pub(crate) fn font_args(family: &str) -> Value {
    json!({ "props": { "font_family": family } })
}

fn font_picker(ctx: &Context, cx: &mut Cx<'_>) {
    let pal = cx.pal;
    let now = cx.model.style.str_value("font_family").map(str::to_owned);
    let mut picked = false;
    let out = popover_area(ctx, Id::unique("font_popover"), PopoverKey::Font).show(ctx, |ui| {
        panel_frame(&pal).show(ui, |ui| {
            ui.label(RichText::new("Font").font(theme::hand(14.0)).color(pal.muted));
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
            for (value, name, family) in font_choices() {
                let ctl =
                    Ctl::new("style.set").args(font_args(value)).label(name).selected(now.as_deref() == Some(value));
                let galley_font = FontId::new(18.0, family);
                let resp = cx.command_widget(ui, &ctl, vec2(FONT_ROW_WIDTH, 34.0), |p, rect, look: Look| {
                    paint_button_bg(p, rect, look, &pal, false);
                    let color = if !look.enabled {
                        pal.muted.gamma_multiply(0.5)
                    } else if look.selected {
                        pal.on_accent
                    } else {
                        pal.text
                    };
                    let galley = p.layout_no_wrap(name.to_owned(), galley_font, color);
                    p.galley(egui::pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0), galley, color);
                    if look.selected {
                        paint_tick(p, rect.right_center() - vec2(18.0, 0.0), color);
                    }
                });
                picked |= resp.clicked();
            }
        });
    });
    if picked || pressed_outside(ctx, out.response.rect) {
        cx.verb(Verb::CloseOverlays);
    }
}

fn paint_tick(p: &egui::Painter, at: Pos2, color: Color32) {
    let stroke = egui::Stroke::new(2.0, color);
    p.line_segment([at + vec2(-5.0, 0.0), at + vec2(-1.5, 3.5)], stroke);
    p.line_segment([at + vec2(-1.5, 3.5), at + vec2(5.0, -4.0)], stroke);
}

/// Draws the colour popover and the font picker, whichever is open.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    match st.color_popover {
        Some(target) => color_popover(ctx, cx, target),
        None => {
            ctx.data_mut(|d| d.remove_temp::<HexDraft>(Id::unique("color_hex_draft")));
        }
    }
    if st.font_picker_open {
        font_picker(ctx, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_make_the_documented_commands() {
        let blue = Pick::Color("blue-500".to_owned());
        assert_eq!(
            pick_command(ColorTarget::Stroke, &blue),
            ("style.set", json!({"props": {"stroke_color": "blue-500"}}))
        );
        assert_eq!(pick_command(ColorTarget::Fill, &blue), ("style.set", json!({"props": {"fill_color": "blue-500"}})));
        assert_eq!(
            pick_command(ColorTarget::Fill, &Pick::Clear),
            ("style.set", json!({"props": {"fill_color": null}})),
            "no fill is null, never transparent"
        );
        assert_eq!(
            pick_command(ColorTarget::Stroke, &Pick::Clear),
            ("style.set", json!({"props": {"stroke_color": "transparent"}}))
        );
        assert_eq!(pick_command(ColorTarget::Background, &blue), ("doc.set_background", json!({"color": "blue-500"})));
        assert_eq!(pick_call(ColorTarget::Background, &Pick::Clear).args, json!({"color": "transparent"}));
    }

    #[test]
    fn the_background_call_routes_to_its_command() {
        use crate::action::{Routed, UiAction, route};
        let call = pick_call(ColorTarget::Background, &Pick::Color("blue-500".to_owned()));
        assert_eq!(call.id, "doc.set_background");
        assert_eq!(call.args, json!({"color": "blue-500"}));
        let routed = route(&UiAction::Command(call.clone()), &UiState::new());
        assert_eq!(routed, Routed::Command(call));
    }

    #[test]
    fn none_is_not_black() {
        let none = pick_value(ColorTarget::Stroke, &Pick::Clear);
        let black = pick_value(ColorTarget::Stroke, &Pick::Color("#000000".to_owned()));
        assert_ne!(none, black);
        assert_eq!(pick_value(ColorTarget::Fill, &Pick::Clear), None);
    }

    #[test]
    fn hex_accepts_three_forms_and_rejects_the_rest() {
        assert_eq!(parse_hex("#abc"), Ok(Color::rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_hex("  #3B82F6 "), Ok(Color::rgb(0x3b, 0x82, 0xf6)));
        assert_eq!(parse_hex("3b82f680"), Ok(Color::rgba(0x3b, 0x82, 0xf6, 0x80)));
        for bad in ["", "#", "#12", "#12345", "#gg0000", "blue", "transparent", "#é12345"] {
            assert!(parse_hex(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn quick_column_has_nine_cells() {
        let q = quick_column();
        let labels: Vec<_> = q.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["None", "Black", "White", "Blue", "Red", "Emerald", "Amber", "Purple", "Slate"]);
        assert!(q.iter().skip(3).all(|c| c.shown.is_some()));
    }

    #[test]
    fn the_grid_names_come_from_the_palette() {
        for fam in FAMILIES {
            for shade in SHADES.iter().skip(1) {
                assert!(drafftink_scene::palette::lookup(&format!("{}-{shade}", fam.name)).is_some());
            }
        }
        assert_eq!(capitalised("blue"), "Blue");
        assert_eq!(capitalised(""), "");
    }

    #[test]
    fn current_reads_the_model() {
        let mut m = Model::new(false);
        m.background = "#ffffff".to_owned();
        assert_eq!(current(&m, ColorTarget::Background), Some(Some(Color::WHITE)));
        assert_eq!(current(&m, ColorTarget::Stroke), None, "unknown without a style");
        m.style.props.insert(
            "fill_color".to_owned(),
            crate::model::StyleProp { applicable: true, value: Some(Value::Null), reason: None },
        );
        assert_eq!(current(&m, ColorTarget::Fill), Some(None));
        m.style.props.insert(
            "stroke_color".to_owned(),
            crate::model::StyleProp { applicable: true, value: Some(json!("#00000000")), reason: None },
        );
        assert_eq!(current(&m, ColorTarget::Stroke), Some(Some(Color::TRANSPARENT)));
    }

    #[test]
    fn font_choices_are_the_four_families() {
        let ids: Vec<_> = font_choices().iter().map(|c| c.0).collect();
        assert_eq!(ids, ["gelpen", "gelpen_serif", "vanilla_extract", "noto_sans"]);
        assert_eq!(font_args("noto_sans"), json!({"props": {"font_family": "noto_sans"}}));
    }
}
