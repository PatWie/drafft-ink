//! The ribbon: the properties of what is selected, or of the tool being drawn with, in a card under the top
//! strip. It replaces the properties panel on the right.
//!
//! * **Where.** The strip keeps its row of stroke colour, fill colour and width in the middle (`topbar.rs`),
//!   which is what every element has, so those controls never move and the ribbon costs no height while it has
//!   nothing else to show. Everything else is a rounded card hanging under the strip, centred on the window and
//!   kept clear of the tool column. The card is in `ChromeRects::ribbon`; popovers, the library drawer and the
//!   toasts read it from there.
//! * **What.** Groups of controls in a fixed order ([`Group::ALL`]). A group is there when `style.get` says its
//!   property applies to the selection (or, with nothing selected, to the tool: [`tool_shows`]), or when the
//!   engine says its command can run (align needs two elements, distribute three). Layering, flip and the
//!   actions need a selection. With nothing selected and no drawing tool there is no card.
//! * **How it fits.** The groups are measured in a hidden pass and packed into rows as wide as the room between
//!   the tool column and the right edge (`layout.rs`); a window too narrow for the widest group, and the phone
//!   layout, get one row that scrolls sideways. Properties that do not fit a row go to the "More" menu and the
//!   head menus (`menus.rs`), which are menu-kit popups.
//!
//! Every control is a command; the scrub of the opacity slider is the one gesture (docs/exceptions.md E13).

mod controls;
mod layout;
mod menus;
mod pictograms;

use drafftink_engine::state::Tool;
use egui::{Align, Area, Context, Id, LayerId, Layout, Order, Pos2, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use self::layout::{Plan, Row};
use crate::chrome::GAP;
use crate::model::StyleProp;
use crate::state::UiState;
use crate::theme::{MARGIN, space};
use crate::widgets::{Cx, panel_frame, vertical_separator};

/// The gap between the strip and the card.
const CARD_GAP: f32 = space::MD;
/// The width of the divider between two groups of a row: space, the line, space.
const DIVIDER_WIDTH: f32 = 2.0 * space::MD + 1.0;
/// The gap between two rows of the card.
const ROW_GAP: f32 = space::SM;

/// Whether a property is relevant to the tool being drawn with (used only when nothing is selected). This is
/// presentation: the engine skips and reports the properties that do not apply.
fn tool_shows(tool: Tool, prop: &str) -> bool {
    const TEXT: [&str; 4] = ["font_family", "font_weight", "font_size", "text_align"];
    const FREEHAND: [&str; 2] = ["freehand_style", "pressure_mode"];
    match tool {
        Tool::Rectangle | Tool::Diamond | Tool::Frame => !TEXT.contains(&prop) && !FREEHAND.contains(&prop),
        Tool::Ellipse => !TEXT.contains(&prop) && !FREEHAND.contains(&prop) && prop != "corner_radius",
        Tool::Line | Tool::Arrow => matches!(prop, "stroke_style" | "sloppiness" | "corner_radius" | "opacity"),
        Tool::Freehand | Tool::Highlighter => {
            matches!(prop, "freehand_style" | "pressure_mode" | "opacity" | "fill_pattern")
        }
        Tool::Text | Tool::Math => TEXT.contains(&prop) || matches!(prop, "line_height" | "opacity"),
        Tool::Select | Tool::Lasso | Tool::Hand | Tool::Eraser | Tool::Laser => false,
    }
}

/// Whether a property has a control now: the engine's `style.get` for a selection, the tool for new elements.
fn shows(cx: &Cx<'_>, prop: &str) -> bool {
    let m = cx.model;
    if m.selection_count > 0 { m.style.applies(prop) } else { tool_shows(m.tool, prop) }
}

/// The groups of the ribbon, in the order they have from left to right and top to bottom. The order never
/// changes; groups come and go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Group {
    /// Solid, dashed, dotted.
    StrokeStyle,
    /// Solid, hatch, cross-hatch, dots and the others.
    FillPattern,
    /// Sharp or round corners.
    Corners,
    /// Straight, curved or elbow arrows and lines.
    ArrowPath,
    /// The start and end heads of an arrow.
    ArrowHeads,
    /// Normal or calligraphy.
    Brush,
    /// Font family and weight.
    FontFace,
    /// Text size, bold and italic.
    TextSize,
    /// Text alignment.
    TextAlign,
    /// The menu of everything that did not fit.
    More,
    /// The opacity slider.
    Opacity,
    /// Align and distribute.
    Align,
    /// Z-order and flip.
    Layer,
    /// Duplicate, group, lock, link, delete.
    Actions,
}

impl Group {
    const ALL: [Group; 14] = [
        Group::StrokeStyle,
        Group::FillPattern,
        Group::Corners,
        Group::ArrowPath,
        Group::ArrowHeads,
        Group::Brush,
        Group::FontFace,
        Group::TextSize,
        Group::TextAlign,
        Group::More,
        Group::Opacity,
        Group::Align,
        Group::Layer,
        Group::Actions,
    ];

    /// Whether the group has anything to offer now.
    fn visible(self, cx: &Cx<'_>) -> bool {
        let model = cx.model;
        let selected = model.selection_count > 0;
        match self {
            Group::StrokeStyle => shows(cx, "stroke_style"),
            Group::FillPattern => shows(cx, "fill_pattern") && has_fill(cx),
            // Next to a path (sharp, round, elbow) two more "sharp" and "round" would only confuse: the corners of
            // arrows and lines are in the "More" menu.
            Group::Corners => shows(cx, "corner_radius") && !controls::has_path(cx),
            Group::ArrowPath => controls::has_path(cx),
            Group::ArrowHeads => controls::new_arrows(cx) || (selected && model.style.has_kind("arrow")),
            Group::Brush => shows(cx, "freehand_style"),
            Group::FontFace => shows(cx, "font_family") || shows(cx, "font_weight"),
            Group::TextSize => shows(cx, "font_size") || model.faces().is_some(),
            Group::TextAlign => shows(cx, "text_align"),
            Group::More => menus::has_more(cx),
            Group::Opacity => shows(cx, "opacity"),
            // Align needs two elements and distribute three; the engine knows, and the group follows its answer.
            Group::Align => selected && cx.refusal("arrange.align").is_none(),
            Group::Layer | Group::Actions => selected,
        }
    }
}

/// A fill pattern only matters while there is a fill (or the selection disagrees about it).
fn has_fill(cx: &Cx<'_>) -> bool {
    let fill = cx.model.style.prop("fill_color");
    !matches!(fill.and_then(|p| p.value.as_ref()), Some(serde_json::Value::Null))
        || fill.is_some_and(StyleProp::is_mixed)
}

/// Draws the ribbon.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let groups: Vec<Group> = Group::ALL.into_iter().filter(|g| g.visible(cx)).collect();
    if groups.is_empty() {
        st.ribbon_menu = None;
        return;
    }
    let screen = ctx.content_rect();
    let frame = panel_frame(&cx.pal);
    let padding = frame.total_margin().sum();
    // Centred on the window, and clear of the tool column on both sides so that the card stays centred.
    let reserve = match st.chrome.tools {
        Some(tools) if !cx.layout.narrow => tools.right() + GAP,
        _ => MARGIN,
    };
    let (left, right) = (screen.left() + reserve, screen.right() - reserve);
    let widths = measure(ctx, cx, st, &groups);
    let room = right - left - padding.x;
    let plan = layout::pack(&widths, DIVIDER_WIDTH, room, cx.layout.narrow);
    let inner_width = plan.width().min(room);
    let card_width = inner_width + padding.x;
    let x = (screen.center().x - card_width / 2.0).clamp(left, (right - card_width).max(left));
    let y = st.chrome.below_strip(screen) + CARD_GAP;
    let area = Area::new(Id::unique("ribbon")).order(Order::Middle).fixed_pos(pos2(x, y)).show(ctx, |ui| {
        frame.show(ui, |ui| draw_plan(ui, cx, st, &groups, &plan, inner_width));
    });
    st.chrome.ribbon = Some(area.response.rect);
    menus::show(ctx, cx, st);
}

/// How wide each group is: it is laid out once in a hidden sizing pass (nothing is drawn, nothing can be clicked).
fn measure(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState, groups: &[Group]) -> Vec<f32> {
    let id = Id::unique("ribbon_measure");
    let bounds = Rect::from_min_size(Pos2::ZERO, vec2(4000.0, 200.0));
    let mut root =
        Ui::new(ctx.clone(), id, UiBuilder::new().layer_id(LayerId::new(Order::Background, id)).max_rect(bounds));
    groups
        .iter()
        .map(|group| {
            let builder = UiBuilder::new()
                .id_salt(group)
                .max_rect(bounds)
                .layout(Layout::left_to_right(Align::Center))
                .sizing_pass()
                .invisible();
            let mut child = root.new_child(builder);
            group.draw(&mut child, cx, st);
            child.min_rect().width()
        })
        .collect()
}

fn draw_plan(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, groups: &[Group], plan: &Plan, width: f32) {
    ui.spacing_mut().item_spacing = vec2(0.0, ROW_GAP);
    match plan {
        Plan::Rows(rows) => {
            for row in rows {
                draw_row(ui, cx, st, groups, row);
            }
        }
        Plan::Scroll(row) => {
            // An area is as wide as the window by default; the scrolling row is as wide as the room.
            ui.set_width(width);
            egui::ScrollArea::horizontal().id_salt("ribbon_scroll").auto_shrink([false, true]).show(ui, |ui| {
                draw_row(ui, cx, st, groups, row);
            });
        }
    }
}

/// One row: its groups with a divider between two of them, centred on the height of a button.
fn draw_row(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, groups: &[Group], row: &Row) {
    let height = cx.layout.small_side();
    let (rect, _) = ui.allocate_exact_size(vec2(row.width, height), Sense::hover());
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().item_spacing.x = 0.0;
    for (n, &index) in row.groups.iter().enumerate() {
        if n > 0 {
            child.add_space(space::MD);
            vertical_separator(&mut child, &cx.pal, height - 2.0 * space::SM);
            child.add_space(space::MD);
        }
        child.scope(|ui| groups[index].draw(ui, cx, st));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_order_lists_every_group_once() {
        let mut seen = Group::ALL.to_vec();
        seen.sort_by_key(|g| *g as u8);
        seen.dedup();
        assert_eq!(seen.len(), Group::ALL.len());
    }

    #[test]
    fn a_drawing_tool_gets_the_properties_that_suit_it() {
        assert!(tool_shows(Tool::Rectangle, "corner_radius") && !tool_shows(Tool::Rectangle, "font_family"));
        assert!(!tool_shows(Tool::Ellipse, "corner_radius"));
        assert!(tool_shows(Tool::Text, "font_family") && tool_shows(Tool::Text, "line_height"));
        assert!(tool_shows(Tool::Freehand, "pressure_mode") && !tool_shows(Tool::Freehand, "stroke_style"));
        assert!(tool_shows(Tool::Arrow, "stroke_style") && !tool_shows(Tool::Arrow, "font_size"));
        assert!(!tool_shows(Tool::Select, "opacity") && !tool_shows(Tool::Hand, "stroke_style"));
    }
}
