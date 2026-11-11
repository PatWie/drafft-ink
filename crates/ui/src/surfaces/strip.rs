//! The top strip: one bar across the whole window, flush with its top edge, in the panel colour.
//!
//! ```text
//! | menu  rooms  title |         | stroke | fill | width |         | right slot |
//!   left group             centre group (centred on the window)      right slot
//! ```
//!
//! * The left group is `menu::buttons`, the centre group is `topbar::style_row`, and the right slot is empty
//!   for now ([`right_slot`]).
//! * The strip is flush with the window edge, with a hairline and a soft shadow under it, because it is a
//!   toolbar edge: a floating card with a gap would leave a strip of canvas above it that you can click and
//!   draw on, and the full-width fill is what makes the chrome read as one frame around the canvas.
//! * Each group is measured in a hidden sizing pass first, so that the centre group is centred on the window
//!   in the same frame (no frame of lag). When the three groups do not fit side by side (a phone, a narrow
//!   window), the centre group collapses into one style button that opens a popover with the same controls.
//! * The rectangle of the strip, and of the menu button, go to `UiState::chrome` for the surfaces below.

use egui::{
    Align, Align2, Area, Context, CornerRadius, Id, Layout, Order, Rect, Role, Sense, Shadow, Stroke, Ui, UiBuilder,
    WidgetInfo, pos2, vec2,
};
use serde_json::Value;

use crate::chrome::GAP;
use crate::colors;
use crate::state::UiState;
use crate::surfaces::color::{PopoverKey, forget_anchor};
use crate::surfaces::{menu, topbar};
use crate::theme::{MARGIN, Palette, space};
use crate::widgets::{Cx, Look, icon_color, paint_button_bg, paint_swatch, panel_frame};

/// Draws the strip, and the style popover when the strip is too narrow for the style controls.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let screen = ctx.content_rect();
    let rect = Rect::from_min_size(screen.min, vec2(screen.width(), cx.layout.strip_height()));
    st.chrome.strip = Some(rect);
    let collapsed = Area::new(Id::unique("top_strip"))
        .fixed_pos(rect.min)
        .order(Order::Middle)
        .show(ctx, |ui| {
            ui.allocate_rect(rect, Sense::hover());
            paint_background(ui, rect, &cx.pal);
            groups(ui, rect, cx, st)
        })
        .inner;
    if collapsed {
        // The wheels are not in the strip now; a colour popover must not open at the place they were.
        forget_anchor(ctx, PopoverKey::Stroke);
        forget_anchor(ctx, PopoverKey::Fill);
        style_popover(ctx, cx, st);
    } else {
        st.style_open = false;
    }
}

/// Lays out the three groups inside the strip. Returns true when the style controls collapsed into a button.
fn groups(ui: &mut Ui, rect: Rect, cx: &mut Cx<'_>, st: &mut UiState) -> bool {
    let inner = rect.shrink2(vec2(MARGIN, space::MD));
    let button = cx.layout.small_side();
    let left_w = measure(ui, inner, |ui| menu::buttons(ui, cx, st));
    let right_w = measure(ui, inner, |ui| right_slot(ui, cx, st));
    let centre_w = measure(ui, inner, |ui| topbar::style_row(ui, cx, st));

    // What the right end holds, and the x where the room for the middle ends.
    let right_gap = if right_w > 0.0 { space::XL } else { 0.0 };
    let room_end = inner.right() - right_w - right_gap;
    let fits = left_w + space::XL + centre_w <= room_end - inner.left();
    let cluster_w = if fits { right_w } else { right_w + button + if right_w > 0.0 { space::SM } else { 0.0 } };

    let left = Rect::from_min_max(inner.min, pos2(inner.left() + left_w + 1.0, inner.bottom()));
    place(ui, "strip_left", left, Layout::left_to_right(Align::Center), |ui| menu::buttons(ui, cx, st));

    let right = Rect::from_min_max(pos2(inner.right() - cluster_w - 1.0, inner.top()), inner.max);
    place(ui, "strip_right", right, Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = space::SM;
        right_slot(ui, cx, st);
        if !fits {
            style_button(ui, cx, st);
        }
    });

    if fits {
        // Centred on the window; pushed aside only as far as the neighbours need.
        let lowest = inner.left() + left_w + space::XL;
        let highest = room_end - centre_w;
        let x = (rect.center().x - centre_w / 2.0).max(lowest).min(highest);
        let centre = Rect::from_min_size(pos2(x, inner.top()), vec2(centre_w + 1.0, inner.height()));
        place(ui, "strip_centre", centre, Layout::left_to_right(Align::Center), |ui| topbar::style_row(ui, cx, st));
    }
    !fits
}

/// The right end of the strip. Empty for now.
///
/// To move the bottom-left bar here: call `bottom::bar(ui, cx, st)` from this function, and take
/// `bottom::show` out of `surfaces::show_all`. The slot is a right-to-left row, so the controls of a group
/// are added last to first, and the strip's width accounting (`groups`) already includes whatever it draws.
fn right_slot(_ui: &mut Ui, _cx: &mut Cx<'_>, _st: &mut UiState) {}

/// The fill of the strip, a soft shadow and the hairline along its lower edge.
fn paint_background(ui: &Ui, rect: Rect, pal: &Palette) {
    let painter = ui.painter();
    painter.add(Shadow { offset: [0, 1], blur: 6, spread: 0, color: pal.shadow }.as_shape(rect, CornerRadius::ZERO));
    painter.rect_filled(rect, CornerRadius::ZERO, pal.panel);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, pal.border_stroke());
}

/// How wide a group is: it is laid out once in a hidden sizing pass (nothing is drawn, nothing can be clicked).
fn measure(ui: &mut Ui, bounds: Rect, add: impl FnOnce(&mut Ui)) -> f32 {
    let builder = UiBuilder::new()
        .id_salt("strip_measure")
        .max_rect(bounds)
        .layout(Layout::left_to_right(Align::Center))
        .sizing_pass()
        .invisible();
    let mut child = ui.new_child(builder);
    add(&mut child);
    child.min_rect().width()
}

/// Runs `add` in a child of the strip that has exactly `rect` and `layout`.
fn place(ui: &mut Ui, salt: &'static str, rect: Rect, layout: Layout, add: impl FnOnce(&mut Ui)) {
    let mut child = ui.new_child(UiBuilder::new().id_salt(salt).max_rect(rect).layout(layout));
    add(&mut child);
}

// ---- the collapsed style controls ----

/// The button that stands for the stroke, fill and width controls: the stroke ring over the fill disc.
fn style_button(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    let pal = cx.pal;
    let side = cx.layout.small_side();
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    st.chrome.style_button = Some(rect);
    let look = Look {
        hovered: resp.hovered() || resp.has_focus(),
        pressed: resp.is_pointer_button_down_on(),
        enabled: true,
        selected: st.style_open,
    };
    if ui.is_rect_visible(rect) {
        paint_button_bg(ui.painter(), rect, look, &pal, true);
        let radius = side * 0.22;
        let offset = side * 0.14;
        let current = |prop: &str| -> Option<egui::Color32> {
            let name = cx.model.style.prop(prop)?.value.as_ref().and_then(Value::as_str)?;
            colors::displayed(name, pal.dark)
        };
        // On the accent fill of the selected look, the rings keep the contrast of the icon colour.
        let ring = if look.selected { None } else { current("stroke_color") }.unwrap_or_else(|| icon_color(look, &pal));
        paint_swatch(
            ui.painter(),
            rect.center() + vec2(offset, offset),
            radius,
            current("fill_color"),
            false,
            false,
            &pal,
            true,
        );
        ui.painter().circle_stroke(rect.center() - vec2(offset, offset), radius, Stroke::new(2.5, ring));
    }
    resp.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::Button, true, "Stroke, fill and width");
        info.selected = Some(st.style_open);
        info
    });
    if resp.on_hover_text("Stroke, fill and width").clicked() {
        st.style_open = !st.style_open;
    }
}

/// The popover under the strip, at the right edge: the style controls in rows. A press outside it closes it,
/// unless a colour popover (which opens from a control in it) is open.
fn style_popover(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if !st.style_open {
        return;
    }
    let screen = ctx.content_rect();
    let at = pos2(screen.right() - MARGIN, st.chrome.below_strip(screen) + GAP);
    let max_width = (screen.width() - 2.0 * MARGIN - 2.0 * space::XL).max(160.0);
    let area = Area::new(Id::unique("style_popover"))
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(at)
        .order(Order::Foreground)
        .show(ctx, |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.set_max_width(max_width);
                topbar::style_column(ui, cx, st);
            });
        });
    let button = st.chrome.style_button.unwrap_or(Rect::NOTHING);
    let outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !button.contains(p))
    });
    if outside && st.color_popover.is_none() {
        st.style_open = false;
    }
}
