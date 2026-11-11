//! The colour popover (stroke, fill, page background) and the font family popover.
//!
//! The colour grid is the old one: 22 Tailwind families by shades 50 to 950, 16 px round cells, a quick
//! column. A pick is a plain command: `style.set` for stroke and fill, `doc.set_background` for the
//! page. "None" on the fill is `fill_color: null`, never a transparent colour. The popover closes on a
//! pick, on Esc (the keymap) and on a click outside it.

use drafftink_scene::Color;
use drafftink_scene::palette::{FAMILIES, SHADES};
use egui::{
    Align2, Area, Color32, Context, FontFamily, FontId, Id, Order, Pos2, Rect, RichText, ScrollArea, Ui, pos2, vec2,
};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::chrome::ChromeRects;
use crate::colors;
use crate::model::Model;
use crate::state::{ColorTarget, UiState};
use crate::surfaces::dialogs::{enter_pressed, text_field};
use crate::surfaces::topbar::clear_blocked;
use crate::theme::{MARGIN, TextSize, UiFontChoice};
use crate::widgets::{Ctl, Cx, Look, paint_button_bg, panel_frame, section_label};

/// Side of a colour cell.
const CELL: f32 = 16.0;
/// Gap between a control and the popover it opened.
const GAP: f32 = crate::chrome::GAP;
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
/// drawn. The popover opens below the stroke and fill buttons, above the background button and under the
/// font button, and is moved back onto the screen when it would not fit.
pub fn remember_anchor(ctx: &Context, key: PopoverKey, rect: Rect) {
    ctx.data_mut(|d| d.insert_temp(Id::unique(("popover_anchor", key)), rect));
}

/// [`remember_anchor`] for a control drawn in `ui`: nothing is remembered in a hidden sizing pass, which lays the
/// controls out away from where they will be drawn.
pub(crate) fn remember_anchor_in(ui: &Ui, key: PopoverKey, rect: Rect) {
    if !ui.is_sizing_pass() {
        remember_anchor(ui.ctx(), key, rect);
    }
}

/// Forgets the place of a button that is not drawn this frame (the stroke and fill wheels while the top strip shows
/// the style button instead), so that a popover opened by a shortcut does not appear at the old place.
pub(crate) fn forget_anchor(ctx: &Context, key: PopoverKey) {
    ctx.data_mut(|d| d.insert_temp(Id::unique(("popover_anchor", key)), Rect::NOTHING));
}

fn anchor_of(ctx: &Context, key: PopoverKey) -> Option<Rect> {
    ctx.data(|d| d.get_temp::<Rect>(Id::unique(("popover_anchor", key)))).filter(|r| r.is_positive())
}

/// The area of a popover next to its button, or at a default place next to the chrome that is there.
fn popover_area(ctx: &Context, chrome: &ChromeRects, id: Id, key: PopoverKey) -> Area {
    let area = Area::new(id).order(Order::Foreground);
    let screen = ctx.content_rect();
    let under_strip = chrome.below_strip(screen) + GAP;
    match (anchor_of(ctx, key), key) {
        (Some(r), PopoverKey::Stroke | PopoverKey::Fill) => {
            area.pivot(Align2::LEFT_TOP).fixed_pos(r.left_bottom() + vec2(0.0, GAP))
        }
        (Some(r), PopoverKey::Background) => area.pivot(Align2::LEFT_BOTTOM).fixed_pos(r.left_top() - vec2(0.0, GAP)),
        (Some(r), PopoverKey::Font) => area.pivot(Align2::LEFT_TOP).fixed_pos(r.left_bottom() + vec2(0.0, GAP)),
        (None, PopoverKey::Stroke | PopoverKey::Fill) => {
            area.pivot(Align2::CENTER_TOP).fixed_pos(pos2(screen.center().x, under_strip))
        }
        (None, PopoverKey::Background) => area
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(pos2(screen.left() + MARGIN, chrome.above_bottom_left(screen) - GAP)),
        (None, PopoverKey::Font) => {
            let under_top = under_strip;
            area.pivot(Align2::CENTER_TOP).fixed_pos(pos2(screen.center().x, under_top))
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

fn capitalised(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|f| f.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// A cell for the stored colour `text` (a palette name): the swatch shows what the canvas draws for it on this
/// theme, and a pick stores `text` (`colors::offer`).
fn choice(label: String, text: &str, dark: bool) -> Choice {
    from_offer(label, text, colors::offer(text, dark))
}

/// A cell named by how it looks, Black or White: a pick stores what the canvas draws as that colour on this theme
/// (`colors::offer_as_seen`).
fn choice_as_seen(label: String, text: &str, dark: bool) -> Choice {
    from_offer(label, text, colors::offer_as_seen(text, dark))
}

fn from_offer(label: String, text: &str, offer: Option<colors::Offer>) -> Choice {
    match offer {
        Some(o) => Choice { label, pick: Pick::Color(o.stored), shown: Some(o.shown) },
        None => Choice { label, pick: Pick::Color(text.to_owned()), shown: None },
    }
}

/// The quick column: None, Black, White and six colours at shade 500.
fn quick_column(dark: bool) -> Vec<Choice> {
    let mut out = vec![
        Choice { label: "None".to_owned(), pick: Pick::Clear, shown: None },
        choice_as_seen("Black".to_owned(), "#000000", dark),
        choice_as_seen("White".to_owned(), "#ffffff", dark),
    ];
    for name in ["blue", "red", "emerald", "amber", "purple", "slate"] {
        out.push(choice(capitalised(name), &format!("{name}-500"), dark));
    }
    out
}

/// Draws one cell. Returns its pick when it was clicked (the command is already pushed by the control).
fn cell(ui: &mut Ui, cx: &mut Cx<'_>, target: ColorTarget, now: Option<Option<Color>>, c: &Choice) -> Option<Pick> {
    let (id, args) = pick_command(target, &c.pick);
    let selected = now.is_some() && now == Some(pick_value(target, &c.pick));
    // "None" greys with the engine's words when the element would show nothing.
    let blocked = if c.pick == Pick::Clear { clear_blocked(cx, target) } else { None };
    let ctl = Ctl::new(id).args(args).label(&c.label).selected(selected).blocked(blocked);
    cx.swatch(ui, &ctl, c.shown, CELL).clicked().then(|| c.pick.clone())
}

/// Draws the popover of a colour target. The pick and the outside click push actions; the pick is also returned.
fn color_popover(ctx: &Context, cx: &mut Cx<'_>, chrome: &ChromeRects, target: ColorTarget) -> Option<Pick> {
    let pal = cx.pal;
    let key = PopoverKey::of(target);
    let now = current(cx.model, target);
    let dark = pal.dark;
    let quick = quick_column(dark);
    let screen = ctx.content_rect();
    let max_w = (screen.width() - 2.0 * MARGIN - 24.0).max(120.0);
    let max_h = (screen.height() - 2.0 * MARGIN - GRID_CHROME).max(80.0);
    let hex_key = Id::unique("color_hex_draft");
    let mut hex: HexDraft = ctx.data_mut(|d| d.get_temp(hex_key)).unwrap_or_default();
    let mut picked: Option<Pick> = None;

    let out = popover_area(ctx, chrome, Id::unique("color_popover"), key).show(ctx, |ui| {
        panel_frame(&pal).show(ui, |ui| {
            let title = match target {
                ColorTarget::Stroke => "Stroke colour",
                ColorTarget::Fill => "Fill colour",
                ColorTarget::Background => "Background colour",
            };
            ui.label(RichText::new(title).font(TextSize::Label.hand()).color(pal.muted));
            ui.add_space(4.0);
            ScrollArea::both().max_width(max_w).max_height(max_h).auto_shrink([true, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                // Shade 25 is too close to white to tell apart in a 16 px cell: the grid starts at 50.
                let shades = SHADES.iter().enumerate().skip(1);
                for (row, (_, shade)) in shades.enumerate() {
                    if row == 1 || row == 5 || row == 6 {
                        ui.add_space(4.0);
                    }
                    ui.horizontal(|ui| {
                        for fam in FAMILIES {
                            let c = choice(
                                format!("{} {shade}", capitalised(fam.name)),
                                &format!("{}-{shade}", fam.name),
                                dark,
                            );
                            picked = cell(ui, cx, target, now, &c).or(picked.take());
                        }
                        if let Some(q) = quick.get(row) {
                            ui.add_space(8.0);
                            picked = cell(ui, cx, target, now, q).or(picked.take());
                        }
                    });
                }
            });
            ui.add_space(8.0);
            picked = hex_row(ui, cx, target, &mut hex).or(picked.take());
        });
    });

    if picked.is_some() {
        hex = HexDraft::default();
        cx.verb(Verb::CloseOverlays);
    } else if pressed_outside(ctx, out.response.rect) {
        cx.verb(Verb::CloseOverlays);
    }
    ctx.data_mut(|d| d.insert_temp(hex_key, hex));
    picked
}

#[derive(Clone, Default)]
struct HexDraft {
    text: String,
    error: Option<String>,
}

/// The hex field. Enter applies a valid value and returns it as a pick; a bad one shows an error under the field.
fn hex_row(ui: &mut Ui, cx: &mut Cx<'_>, target: ColorTarget, draft: &mut HexDraft) -> Option<Pick> {
    let pal = cx.pal;
    let (id, _) = pick_command(target, &Pick::Clear);
    let reason = cx.refusal(id);
    section_label(ui, &pal, "Hex");
    let resp = text_field(ui, &mut draft.text, "#rrggbb, then Enter", reason.clone(), Some(9), "Hex colour");
    let mut applied = None;
    if reason.is_none() && resp.lost_focus() && enter_pressed(ui) {
        match parse_hex(&draft.text) {
            Ok(color) => {
                // The typed colour is the one to see on the canvas; the document holds the colour that draws it.
                let pick = Pick::Color(colors::stored(color, pal.dark).to_hex());
                let (id, args) = pick_command(target, &pick);
                cx.run(id, args);
                applied = Some(pick);
            }
            Err(message) => draft.error = Some(message),
        }
    }
    if let Some(err) = &draft.error {
        ui.label(RichText::new(err).size(TextSize::Small.points()).color(pal.danger));
    }
    applied
}

// ---- fonts ----

/// The font families of the canvas: setting value, name, and the family that previews it.
fn font_choices() -> [(&'static str, &'static str, FontFamily); 5] {
    // A face this build lacks (Noto Sans on the web, Plex Sans before it is fetched) previews in the default font.
    UiFontChoice::ALL.map(|c| (c.setting(), c.title(), c.preview(TextSize::Title.points()).family))
}

/// The arguments of `style.set` for a font family.
pub(crate) fn font_args(family: &str) -> Value {
    json!({ "props": { "font_family": family } })
}

fn font_picker(ctx: &Context, cx: &mut Cx<'_>, chrome: &ChromeRects) {
    let pal = cx.pal;
    let now = cx.model.style.str_value("font_family").map(str::to_owned);
    let mut picked = false;
    let out = popover_area(ctx, chrome, Id::unique("font_popover"), PopoverKey::Font).show(ctx, |ui| {
        panel_frame(&pal).show(ui, |ui| {
            ui.label(RichText::new("Font").font(TextSize::Label.hand()).color(pal.muted));
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
            for (value, name, family) in font_choices() {
                let ctl =
                    Ctl::new("style.set").args(font_args(value)).label(name).selected(now.as_deref() == Some(value));
                let galley_font = FontId::new(TextSize::Title.points(), family);
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
        Some(target) => {
            if let Some(Pick::Color(color)) = color_popover(ctx, cx, &st.chrome, target) {
                st.last_picked.remember(target, color);
            }
        }
        None => {
            ctx.data_mut(|d| d.remove_temp::<HexDraft>(Id::unique("color_hex_draft")));
        }
    }
    if st.font_picker_open {
        font_picker(ctx, cx, &st.chrome);
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
        let q = quick_column(false);
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
            crate::model::StyleProp { applicable: true, value: Some(Value::Null), reason: None, clear_reason: None },
        );
        assert_eq!(current(&m, ColorTarget::Fill), Some(None));
        m.style.props.insert(
            "stroke_color".to_owned(),
            crate::model::StyleProp {
                applicable: true,
                value: Some(json!("transparent")),
                reason: None,
                clear_reason: None,
            },
        );
        assert_eq!(current(&m, ColorTarget::Stroke), Some(Some(Color::TRANSPARENT)));
    }

    #[test]
    fn font_choices_are_the_five_families() {
        let ids: Vec<_> = font_choices().iter().map(|c| c.0).collect();
        assert_eq!(ids, ["gelpen", "gelpen_serif", "vanilla_extract", "noto_sans", "plex_sans"]);
        assert_eq!(font_args("noto_sans"), json!({"props": {"font_family": "noto_sans"}}));
    }
}
