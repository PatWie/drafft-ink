//! The custom view of the colour popover: a square for saturation and value, a hue bar and, where the colour has an
//! alpha, an alpha bar, with the hex field kept in step.
//!
//! * **What is edited** is the colour as the canvas shows it ([`Hsv`] and an alpha). A pick stores the colour whose
//!   display it is (`colors::stored`), as the hex field and the eyedropper do, so in the dark theme the square shows
//!   what the drawing will look like and the document holds its inverse under the filter.
//! * **A drag is one gesture** (docs/exceptions.md E13). The stroke and the fill go through `scrub::drive`: `begin`
//!   on the first change, `update` for each further one, `end` on release, which is one undo step and a live preview on
//!   the canvas. The square and the bars of one colour share the gesture, so a drag may start on the square and end over
//!   a bar. The page colour has no gesture command, so its drag changes only the picker's own preview and one
//!   `doc.set_background` runs on release.
//! * **A key** (arrows on the focused square or bar) is a plain command, as an arrow key on a slider is.

use drafftink_scene::Color;
use egui::epaint::Mesh;
use egui::{Color32, CornerRadius, Key, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo, vec2};
use serde_json::Value;

use super::{HexDraft, Pick, cell, hex_row, pick_command, quick_column};
use crate::colors;
use crate::hsv::Hsv;
use crate::scrub::{self, ScrubControl};
use crate::state::ColorTarget;
use crate::theme::{Palette, TextSize, radius};
use crate::widgets::{Cx, section_label};

/// Width of the square and the bars, in points.
const WIDTH: f32 = 260.0;
/// Height of the square.
const SQUARE_HEIGHT: f32 = 150.0;
/// Height of the square on the touch layout.
const SQUARE_HEIGHT_TOUCH: f32 = 180.0;
/// Height of a bar.
const BAR_HEIGHT: f32 = 16.0;
/// Height of a bar on the touch layout.
const BAR_HEIGHT_TOUCH: f32 = 28.0;
/// Radius of the handle on the square and the bars.
const THUMB: f32 = 8.0;
/// Radius of the handle on the touch layout.
const THUMB_TOUCH: f32 = 12.0;
/// Space between the parts.
const GAP: f32 = 8.0;
/// What an arrow key changes: one hundredth of the range; with Shift, one tenth.
const KEY_STEP: f32 = 0.01;
/// The side of the preview chip next to the hex field.
const CHIP: f32 = 28.0;

/// The colour the picker is editing, and what it remembers between frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Working {
    hsv: Hsv,
    alpha: u8,
    /// The stored colour the picker last wrote or read: the document holds it, so a different one is a change from
    /// outside (a swatch, an undo, another selection) that the picker follows.
    known: Option<Option<Color>>,
    /// `known` has been set once.
    primed: bool,
    /// A widget of the picker was held in the last frame.
    held: bool,
    /// A drag changed the colour and has not been applied yet (the page colour only).
    unsaved: bool,
}

impl Default for Working {
    fn default() -> Self {
        Self { hsv: Hsv::default(), alpha: 255, known: None, primed: false, held: false, unsaved: false }
    }
}

impl Working {
    /// The colour as the canvas shows it.
    fn shown(&self) -> Color {
        self.hsv.to_color(self.alpha)
    }

    /// The colour to store so that the canvas shows [`Self::shown`].
    fn stored(&self, dark: bool) -> Color {
        colors::stored(self.shown(), dark)
    }

    /// Takes a colour of the document. A clear colour (no stroke) or no colour (no fill) leaves the hue where it is and
    /// starts from an opaque colour, so that the first drag does not make an invisible one.
    fn follow(&mut self, now: Option<Option<Color>>, dark: bool) {
        if self.primed && self.known == now {
            return;
        }
        (self.known, self.primed) = (now, true);
        match now {
            Some(Some(c)) if c.a > 0 => {
                self.hsv = Hsv::from_color(colors::shown(c, dark));
                self.alpha = c.a;
            }
            Some(_) => self.alpha = 255,
            None => {}
        }
    }

    /// Takes the colour a pick stored (the hex field), which is in the document by now.
    fn adopt(&mut self, stored: Color, dark: bool) {
        self.follow(Some(Some(stored)), dark);
    }
}

/// How the colour of a target is applied while it is dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Live {
    /// A gesture: a live preview, one undo step.
    Gesture(ScrubControl),
    /// One command when the drag ends.
    OnRelease,
}

impl Live {
    fn of(target: ColorTarget) -> Self {
        match target {
            ColorTarget::Stroke => Self::Gesture(ScrubControl::StrokeColor),
            ColorTarget::Fill => Self::Gesture(ScrubControl::FillColor),
            // The brush has no style gesture: a pick is `image.paint_brush`, run once when the drag ends.
            ColorTarget::Background | ColorTarget::Brush | ColorTarget::Marker => Self::OnRelease,
        }
    }
}

/// True when the colour of the target has an alpha: the stroke and the fill do (`#rrggbbaa`, which the hex field also
/// takes); the colour of the page is a solid one, and "None" is its clear choice. The brush has its own opacity.
pub(super) fn has_alpha(target: ColorTarget) -> bool {
    matches!(target, ColorTarget::Stroke | ColorTarget::Fill)
}

/// What the custom view tells the popover.
#[derive(Debug, Default)]
pub(super) struct Out {
    /// The colour (as stored) to show as the last picked one: a drag has ended, or a key or the hex field changed it.
    pub remember: Option<Pick>,
    /// The popover closes: "None" was chosen.
    pub close: bool,
}

/// The arguments of the command that applies the colour.
fn args_of(target: ColorTarget, working: &Working, dark: bool) -> Value {
    pick_command(target, &Pick::Color(working.stored(dark).to_hex())).1
}

/// Draws the custom view. `now` is the colour of the target in the document (see `current`).
pub(super) fn show(
    ui: &mut Ui,
    cx: &mut Cx<'_>,
    target: ColorTarget,
    now: Option<Option<Color>>,
    working: &mut Working,
    hex: &mut HexDraft,
) -> Out {
    let pal = cx.pal;
    // The colours of a picture are not mapped for the dark theme (`ColorTarget::is_themed`).
    let dark = pal.dark && target.is_themed();
    let touch = cx.layout.compact;
    let live = Live::of(target);
    let pointer_down = ui.input(|i| i.pointer.any_down());
    // A change from outside is followed, but never while a drag is going on (the model lags the drag by a frame) or
    // waits to be applied.
    if !pointer_down && !working.unsaved {
        working.follow(now, dark);
    }
    let blocked = match live {
        Live::Gesture(control) => scrub::refusal(cx, control),
        Live::OnRelease => cx.refusal(pick_command(target, &Pick::Clear).0),
    };
    let width = WIDTH.min(ui.available_width().max(120.0));
    let (square_h, bar_h, thumb) =
        if touch { (SQUARE_HEIGHT_TOUCH, BAR_HEIGHT_TOUCH, THUMB_TOUCH) } else { (SQUARE_HEIGHT, BAR_HEIGHT, THUMB) };
    let mut out = Out::default();
    let mut held = false;
    let mut changed_by_key = false;

    ui.spacing_mut().item_spacing = vec2(0.0, GAP);
    ui.add_enabled_ui(blocked.is_none(), |ui| {
        let before = *working;
        let mut resps: Vec<Response> = Vec::with_capacity(3);

        // The square: saturation across, value up.
        let (rect, mut resp) = ui.allocate_exact_size(vec2(width, square_h), Sense::click_and_drag());
        if let Some(p) = pointer_in(&resp, rect) {
            working.hsv = working.hsv.with_sv(fraction(p.x, rect.x_range()), 1.0 - fraction(p.y, rect.y_range()));
        }
        let ds = key_axis(ui, &resp, Key::ArrowLeft, Key::ArrowRight);
        let dv = key_axis(ui, &resp, Key::ArrowDown, Key::ArrowUp);
        if ds != 0.0 || dv != 0.0 {
            working.hsv = working.hsv.with_sv(working.hsv.saturation() + ds, working.hsv.value() + dv);
        }
        let square_moved = working.hsv != before.hsv;
        if square_moved {
            resp.mark_changed();
        }
        paint_square(ui, rect, working.hsv, &pal, thumb);
        describe(&resp, "Saturation and value", None, blocked.is_none());
        resps.push(resp);

        // The hue bar.
        let (rect, mut resp) = ui.allocate_exact_size(vec2(width, bar_h), Sense::click_and_drag());
        if let Some(p) = pointer_in(&resp, rect) {
            working.hsv = working.hsv.with_hue(fraction(p.x, rect.x_range()) * HUE_MAX);
        }
        let dh = key_axis(ui, &resp, Key::ArrowLeft, Key::ArrowRight);
        if dh != 0.0 {
            working.hsv = working.hsv.with_hue(working.hsv.hue() + dh * 100.0);
        }
        if working.hsv.hue() != before.hsv.hue() {
            resp.mark_changed();
        }
        paint_hue_bar(ui, rect, working.hsv, &pal, thumb);
        describe(&resp, "Hue", Some(f64::from(working.hsv.hue())), blocked.is_none());
        resps.push(resp);

        // The alpha bar.
        if has_alpha(target) {
            let (rect, mut resp) = ui.allocate_exact_size(vec2(width, bar_h), Sense::click_and_drag());
            if let Some(p) = pointer_in(&resp, rect) {
                working.alpha = to_byte(fraction(p.x, rect.x_range()));
            }
            let da = key_axis(ui, &resp, Key::ArrowLeft, Key::ArrowRight);
            if da != 0.0 {
                working.alpha = to_byte(f32::from(working.alpha) / 255.0 + da);
            }
            if working.alpha != before.alpha {
                resp.mark_changed();
            }
            paint_alpha_bar(ui, rect, working.hsv, working.alpha, &pal, thumb);
            describe(&resp, "Alpha", Some(f64::from(working.alpha) * 100.0 / 255.0), blocked.is_none());
            resps.push(resp);
        }

        // What the widgets did.
        held = resps.iter().any(|r| r.is_pointer_button_down_on() || r.dragged());
        changed_by_key = !held && resps.iter().any(Response::changed);
        let args = args_of(target, working, dark);
        for resp in &resps {
            match live {
                Live::Gesture(control) => scrub::drive(ui, cx, control, resp, || args.clone()),
                Live::OnRelease => {
                    if resp.changed() && held {
                        working.unsaved = true;
                    } else if resp.changed() {
                        let (id, call_args) = pick_command(target, &Pick::Color(working.stored(dark).to_hex()));
                        cx.run(id, call_args);
                    }
                }
            }
        }
    });
    if blocked.is_some() {
        // The widgets above are greyed with the reason as their tooltip, which the greyed square cannot show itself.
        if let Some(reason) = &blocked {
            ui.label(egui::RichText::new(reason).size(TextSize::Small.points()).color(pal.muted));
        }
    }
    if live == Live::OnRelease && working.unsaved && !pointer_down {
        let (id, args) = pick_command(target, &Pick::Color(working.stored(dark).to_hex()));
        cx.run(id, args);
        working.unsaved = false;
        changed_by_key = true;
    }
    // A drag that has ended, or a change by key, is a pick worth remembering.
    let ended = working.held && !held;
    working.held = held;
    if ended || changed_by_key {
        out.remember = Some(Pick::Color(working.stored(dark).to_hex()));
    }
    // The stored colour is what the document will hold, so the picker knows it without waiting for the model.
    if held || changed_by_key || ended {
        (working.known, working.primed) = (Some(Some(working.stored(dark))), true);
    }

    // The preview and the hex field show what is on the canvas, and follow the picker unless the field is being typed
    // in; then the choice of no colour.
    ui.add_space(2.0);
    section_label(ui, &pal, "Hex");
    // What the canvas will draw: in the dark theme a colour the filter cannot show is drawn as the nearest one it can.
    let on_canvas = colors::shown(working.stored(dark), dark);
    if held || changed_by_key {
        hex.error = None;
    }
    // A value that was refused stays in the field, with its error, until the picker moves or the field is edited.
    if !hex.editing && hex.error.is_none() {
        hex.text = on_canvas.to_hex();
    }
    ui.horizontal(|ui| {
        paint_chip(ui, on_canvas, &pal);
        ui.spacing_mut().item_spacing.x = GAP;
        let reason = blocked.clone();
        if let Some(Pick::Color(stored)) = hex_row(ui, cx, target, hex, reason) {
            if let Ok(color) = Color::parse(&stored) {
                working.adopt(color, dark);
            }
            out.remember = Some(Pick::Color(stored));
        }
    });
    // A brush has a colour; "none" would paint nothing (erasing is the other mode).
    if target.is_themed()
        && let Some(none) = quick_column(dark).first()
        && cell(ui, cx, target, now, none).is_some()
    {
        out.close = true;
    }
    out
}

/// The top of the hue bar, a hair under a full turn so that the end of the bar is not the start.
const HUE_MAX: f32 = Hsv::HUE_RANGE - 0.01;

/// The position of the pointer in a widget it is dragging or has just pressed, kept inside the rectangle.
fn pointer_in(resp: &Response, rect: Rect) -> Option<Pos2> {
    if !(resp.dragged() || resp.is_pointer_button_down_on()) {
        return None;
    }
    resp.interact_pointer_pos().map(|p| p.clamp(rect.min, rect.max))
}

/// Where `x` lies in a range, from 0 to 1.
fn fraction(x: f32, range: egui::Rangef) -> f32 {
    if range.span() > 0.0 { ((x - range.min) / range.span()).clamp(0.0, 1.0) } else { 0.0 }
}

/// A fraction of the 8-bit range as a byte.
fn to_byte(fraction: f32) -> u8 {
    // The input is clamped into 0..=1, so the product is in 0..=255 and the cast cannot truncate.
    (fraction.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// What the arrow keys of a focused widget ask for along one axis: a step of one hundredth of the range (one tenth with
/// Shift) towards `more`, or away from it towards `less`. Nothing when the widget has no focus. The keys are consumed,
/// so they do not nudge the selection.
fn key_axis(ui: &Ui, resp: &Response, less: Key, more: Key) -> f32 {
    if !resp.has_focus() {
        return 0.0;
    }
    ui.input_mut(|i| {
        let step = if i.modifiers.shift { 10.0 * KEY_STEP } else { KEY_STEP };
        let mut pressed = |key: Key| {
            let plain = i.consume_key(egui::Modifiers::NONE, key);
            i.consume_key(egui::Modifiers::SHIFT, key) || plain
        };
        let (down, up) = (pressed(less), pressed(more));
        step * (f32::from(u8::from(up)) - f32::from(u8::from(down)))
    })
}

/// Names the widget for the accessibility tree and the tests.
fn describe(resp: &Response, label: &'static str, value: Option<f64>, enabled: bool) {
    resp.widget_info(|| match value {
        Some(v) => WidgetInfo::slider(enabled, v, label),
        None => WidgetInfo::labeled(egui::Role::Slider, enabled, label),
    });
}

// ---- painting ----

/// A mesh of one quad with a colour at each corner: top left, top right, bottom right, bottom left.
fn quad(rect: Rect, colors: [Color32; 4]) -> Mesh {
    let mut mesh = Mesh::default();
    let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    for (pos, color) in corners.into_iter().zip(colors) {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    mesh
}

fn border(ui: &Ui, rect: Rect, pal: &Palette) {
    ui.painter().rect_stroke(rect, CornerRadius::same(radius::SM), Stroke::new(1.0, pal.border), StrokeKind::Inside);
}

/// The handle: a ring that shows on any colour, filled with the colour under it.
fn paint_thumb(ui: &Ui, center: Pos2, thumb: f32, fill: Color32) {
    let p = ui.painter();
    p.circle_filled(center, thumb, fill);
    p.circle_stroke(center, thumb, Stroke::new(2.0, Color32::WHITE));
    p.circle_stroke(center, thumb + 1.0, Stroke::new(1.0, Color32::from_black_alpha(110)));
}

fn paint_square(ui: &Ui, rect: Rect, hsv: Hsv, pal: &Palette, thumb: f32) {
    // White to the hue along the top, and down to black: a corner colour each, which the mesh blends.
    let hue = colors::to_egui(hsv.pure_hue());
    ui.painter().add(Shape::mesh(quad(rect, [Color32::WHITE, hue, Color32::BLACK, Color32::BLACK])));
    border(ui, rect, pal);
    let at = Pos2::new(rect.left() + hsv.saturation() * rect.width(), rect.top() + (1.0 - hsv.value()) * rect.height());
    paint_thumb(ui, at, thumb, colors::to_egui(hsv.to_color(255)));
}

fn paint_hue_bar(ui: &Ui, rect: Rect, hsv: Hsv, pal: &Palette, thumb: f32) {
    let stops = 6;
    for i in 0..stops {
        let (a, b) = (i as f32 / stops as f32, (i + 1) as f32 / stops as f32);
        let hue_at = |f: f32| colors::to_egui(Hsv::new(f * Hsv::HUE_RANGE, 1.0, 1.0).to_color(255));
        let piece =
            Rect::from_x_y_ranges(rect.left() + a * rect.width()..=rect.left() + b * rect.width(), rect.y_range());
        ui.painter().add(Shape::mesh(quad(piece, [hue_at(a), hue_at(b), hue_at(b), hue_at(a)])));
    }
    border(ui, rect, pal);
    let at = Pos2::new(rect.left() + hsv.hue() / Hsv::HUE_RANGE * rect.width(), rect.center().y);
    paint_thumb(ui, at, thumb, colors::to_egui(hsv.pure_hue()));
}

/// The grey squares under a colour that is not opaque.
fn paint_checker(ui: &Ui, rect: Rect, pal: &Palette) {
    let [light, dark] = pal.checker;
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, light);
    let cell = 6.0;
    let (cols, rows) = ((rect.width() / cell).ceil() as i32, (rect.height() / cell).ceil() as i32);
    for row in 0..rows {
        for col in (row % 2..cols).step_by(2) {
            let min = rect.min + vec2(col as f32 * cell, row as f32 * cell);
            p.rect_filled(Rect::from_min_size(min, vec2(cell, cell)), 0.0, dark);
        }
    }
}

fn paint_alpha_bar(ui: &Ui, rect: Rect, hsv: Hsv, alpha: u8, pal: &Palette, thumb: f32) {
    paint_checker(ui, rect, pal);
    let opaque = colors::to_egui(hsv.to_color(255));
    let clear = Color32::from_rgba_unmultiplied(opaque.r(), opaque.g(), opaque.b(), 0);
    ui.painter().add(Shape::mesh(quad(rect, [clear, opaque, opaque, clear])));
    border(ui, rect, pal);
    let at = Pos2::new(rect.left() + f32::from(alpha) / 255.0 * rect.width(), rect.center().y);
    paint_thumb(ui, at, thumb, colors::to_egui(hsv.to_color(alpha)));
}

/// The colour as it will show, on the grey squares when it is not opaque.
fn paint_chip(ui: &mut Ui, color: Color, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(CHIP, CHIP), Sense::hover());
    if color.a < 255 {
        paint_checker(ui, rect, pal);
    }
    ui.painter().rect_filled(rect, CornerRadius::same(radius::SM), colors::to_egui(color));
    border(ui, rect, pal);
}
