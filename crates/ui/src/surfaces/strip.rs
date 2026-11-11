//! The top strip: one bar across the whole window, flush with its top edge, in the panel colour, two control
//! rows tall (one on a phone). The height depends on the layout only, never on what is selected.
//!
//! ```text
//! | menu rooms | stroke  ... ... | width ...  | font ...   |        | More v |
//! | title      | fill    ... ... | style ...  | weight ... |        |        |
//!   left        colour group       contextual groups, each in two rows   overflow
//! ```
//!
//! * The left cluster is `menu::buttons`, the colour group is `topbar::style_rows`, and the contextual groups
//!   are `ribbon::Group`s. Thin dividers stand between them. A group is two rows of controls; its rows line up
//!   with the rows of every other group.
//! * What does not fit goes behind the "More" button at the right end (`ribbon::fit` plans it from widths measured
//!   in a hidden pass, so the plan is right in the same frame). When even the colour group does not fit, it
//!   collapses into one style button that opens a popover with the same controls; a phone (one row, narrow) puts
//!   every contextual group behind "More" as well. As a last resort the strip scrolls sideways.
//! * With nothing selected and a tool that draws nothing there are no contextual groups: the strip holds the
//!   colour group, which sets the defaults for new elements. Hiding it would make the strip change with the tool,
//!   and the colours are the control people reach for first.
//! * The strip is flush with the window edge, with a hairline and a soft shadow under it, because it is a
//!   toolbar edge: a floating card with a gap would leave a strip of canvas above it that you can click and
//!   draw on.
//! * The rectangle of the strip, and of the menu button, go to `UiState::chrome` for the surfaces below.

use egui::{
    Align, Align2, Area, Context, CornerRadius, Id, Layout, Order, Rect, Role, ScrollArea, Sense, Shadow, Stroke, Ui,
    UiBuilder, WidgetInfo, pos2, vec2,
};
use serde_json::Value;

use crate::chrome::GAP;
use crate::colors;
use crate::state::UiState;
use crate::surfaces::color::{PopoverKey, forget_anchor};
use crate::surfaces::ribbon::{self, DIVIDER_WIDTH, Group, MORE_BUTTON_WIDTH, Widths};
use crate::surfaces::{menu, topbar};
use crate::theme::{MARGIN, Palette, space};
use crate::widgets::{Cx, Look, icon_color, paint_button_bg, paint_swatch, panel_frame, vertical_separator};

/// Draws the strip, the style popover when the strip is too narrow for the colour group, and the menus of the
/// groups.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let screen = ctx.content_rect();
    let rect = Rect::from_min_size(screen.min, vec2(screen.width(), cx.layout.strip_height()));
    st.chrome.strip = Some(rect);
    let drawn = Area::new(Id::unique("top_strip"))
        .fixed_pos(rect.min)
        .order(Order::Middle)
        .show(ctx, |ui| {
            ui.allocate_rect(rect, Sense::hover());
            paint_background(ui, rect, &cx.pal);
            bands(ui, rect, cx, st)
        })
        .inner;
    if drawn.colour_inline {
        st.style_open = false;
    } else {
        // The wheels are not in the strip now; a colour popover must not open at the place they were.
        forget_anchor(ctx, PopoverKey::Stroke);
        forget_anchor(ctx, PopoverKey::Fill);
        style_popover(ctx, cx, st);
    }
    ribbon::show_menus(ctx, cx, st, &drawn.overflow);
}

/// What `bands` decided, for the surfaces that depend on it.
struct Drawn {
    /// The colour group is in the strip (else the style button is).
    colour_inline: bool,
    /// The groups that went behind the "More" button.
    overflow: Vec<Group>,
}

/// Lays out the left cluster, the colour group, the contextual groups and the "More" button.
fn bands(ui: &mut Ui, rect: Rect, cx: &mut Cx<'_>, st: &mut UiState) -> Drawn {
    let inner = rect.shrink2(vec2(MARGIN, space::MD));
    let button = cx.layout.small_side();
    let groups = ribbon::visible_groups(cx);

    let left_w = measure(ui, inner, |ui| menu::buttons(ui, cx, st));
    let colour_w = measure(ui, inner, |ui| topbar::style_rows(ui, cx, st));
    let widths: Vec<f32> = groups.iter().map(|g| measure(ui, inner, |ui| g.draw(ui, cx, st))).collect();
    let plan = ribbon::fit(
        Widths {
            colour: colour_w,
            colour_button: button,
            groups: &widths,
            more_button: MORE_BUTTON_WIDTH,
            extras: ribbon::has_extras(cx),
        },
        DIVIDER_WIDTH,
        inner.width() - left_w,
        cx.layout.narrow,
    );

    let left = Rect::from_min_size(inner.min, vec2(left_w + 1.0, inner.height()));
    place(ui, "strip_left", left, Layout::top_down(Align::Min), |ui| menu::buttons(ui, cx, st));

    let band = inner.height();
    let content = Rect::from_min_max(pos2(inner.left() + left_w, inner.top()), inner.max);
    let (inline, overflow) = groups.split_at(plan.shown);
    let items = |ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState| {
        ui.spacing_mut().item_spacing.x = 0.0;
        divider(ui, cx, band);
        if plan.colour_inline {
            topbar::style_rows(ui, cx, st);
        } else {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space((band - button) / 2.0);
                style_button(ui, cx, st);
            });
        }
        for group in inline {
            divider(ui, cx, band);
            group.draw(ui, cx, st);
        }
    };
    if plan.scroll {
        place(ui, "strip_content", content, Layout::left_to_right(Align::Min), |ui| {
            ScrollArea::horizontal().id_salt("strip_scroll").auto_shrink([false, true]).show(ui, |ui| {
                items(ui, cx, st);
                if plan.more {
                    divider(ui, cx, band);
                    ribbon::more_button(ui, cx, st);
                }
            });
        });
    } else {
        place(ui, "strip_content", content, Layout::left_to_right(Align::Min), |ui| items(ui, cx, st));
        if plan.more {
            let right = Rect::from_min_max(pos2(inner.right() - MORE_BUTTON_WIDTH - 1.0, inner.top()), inner.max);
            place(ui, "strip_more", right, Layout::right_to_left(Align::Center), |ui| ribbon::more_button(ui, cx, st));
        }
    }
    Drawn { colour_inline: plan.colour_inline, overflow: overflow.to_vec() }
}

/// A divider between two parts of the strip: space, a hairline as tall as the two rows less a little, space.
fn divider(ui: &mut Ui, cx: &Cx<'_>, band: f32) {
    ui.add_space(space::MD);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(space::XS);
        vertical_separator(ui, &cx.pal, band - 2.0 * space::XS);
    });
    ui.add_space(space::MD);
}

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
        let mut info = WidgetInfo::labeled(Role::Button, true, "Stroke and fill");
        info.selected = Some(st.style_open);
        info
    });
    if resp.on_hover_text("Stroke and fill").clicked() {
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
