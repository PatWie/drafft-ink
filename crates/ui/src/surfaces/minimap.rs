//! The minimap: a small card at the bottom right with the whole drawing in outline and the part of it the window
//! shows as a rectangle. A press or drag on it moves the view (`view.set`), the wheel zooms (`view.zoom_at`).
//!
//! It is off by default (the setting `view.minimap`, flipped by the verb `ui.toggle_minimap`) and then draws, reads
//! and builds nothing. What it shows is the model's [`Snapshot`](crate::minimap::Snapshot), a reduction of the
//! document made once per change; a frame only draws its marks (at most a few thousand rectangles) and one outline.
//! The maths is in [`crate::minimap`].

use egui::{
    Align2, Area, Color32, Context, CornerRadius, CursorIcon, Id, Margin, Mesh, Order, Painter, Pos2, Rect, Response,
    Role, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, pos2, vec2,
};
use serde_json::json;

use crate::colors;
use crate::minimap::{Grab, Look, Mapping, Scene, visible, zoom_factor};
use crate::model::ViewView;
use crate::state::UiState;
use crate::theme::{MARGIN, Palette, radius, space};
use crate::widgets::{Cx, Layout, panel_frame};

/// The space between the card's edge and the drawing, so that a mark at the extent's edge is not on the border.
pub const INSET: f32 = space::MD;
/// Padding between the panel's border and the card.
const PAD: f32 = space::MD;
/// A mark is at least this many points wide and high, so that a small element is still a dot.
const MIN_MARK: f32 = 1.5;
/// An outline is drawn only when its shorter side is at least this many points; a smaller one is a dot.
const MIN_OUTLINE: f32 = 3.0;
/// The camera's rectangle is at least this many points wide and high.
const MIN_VIEW: f32 = 6.0;
/// How much of the accent colour tints the inside of the camera's rectangle.
const VIEW_TINT: f32 = 0.16;

/// The size of the card: larger on a tablet, where a finger aims at it, smaller on a phone.
fn card_size(layout: &Layout) -> Vec2 {
    if layout.narrow {
        vec2(132.0, 88.0)
    } else if layout.compact {
        vec2(208.0, 140.0)
    } else {
        vec2(188.0, 124.0)
    }
}

/// Draws the card while the minimap is on. Nothing else runs while it is off.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if !cx.model.minimap.on {
        st.minimap_grab = None;
        return;
    }
    let screen = ctx.content_rect();
    let size = card_size(&cx.layout);
    let outer = size + Vec2::splat(2.0 * PAD);
    let lift = st.chrome.lift_bottom_right(screen, outer);
    let area = Area::new(Id::unique("minimap"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-MARGIN, -lift))
        .order(Order::Middle)
        .show(ctx, |ui| {
            panel_frame(&cx.pal).inner_margin(Margin::same(PAD as i8)).show(ui, |ui| card(ui, cx, st, size));
        });
    st.chrome.minimap = Some(area.response.rect);
}

/// The card: input first, so that the rectangle is drawn where the pointer put it, then the picture.
fn card(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, size: Vec2) {
    let model = cx.model;
    let snapshot = &model.minimap;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::labeled(Role::Canvas, true, "Minimap"));
    let mapping = st
        .minimap_grab
        .map(|grab| grab.mapping())
        .or_else(|| Mapping::fit(snapshot.scene.extent(&snapshot.camera), rect.shrink(INSET)));
    if let Some(mapping) = mapping {
        steer(ui.ctx(), &response, cx, st, (mapping, rect), &snapshot.camera);
    }
    zoom(ui.ctx(), &response, cx);
    if response.hovered() {
        ui.ctx().set_cursor_icon(if st.minimap_grab.is_some() { CursorIcon::Grabbing } else { CursorIcon::Grab });
    }

    // While the pointer drags, the rectangle is drawn at the target: the camera follows a frame later.
    let mut camera = snapshot.camera;
    if let Some(grab) = &st.minimap_grab {
        camera.scroll = grab.target();
    }
    let pal = cx.pal;
    let painter = ui.painter_at(rect);
    let page = colors::displayed(&model.background, pal.dark).unwrap_or(pal.button);
    let corner = CornerRadius::same(radius::SM);
    painter.rect_filled(rect, corner, page);
    if let Some(mapping) = mapping {
        paint_scene(&painter, &mapping, &snapshot.scene, pal.dark);
        paint_camera(&painter, &mapping, &camera, &pal);
    }
    painter.rect_stroke(rect, corner, Stroke::new(1.0, pal.border), StrokeKind::Inside);
}

/// A press on the card puts the camera there, and a drag from it moves the camera along (`view.set`, a command made
/// for many calls a second: it leaves no ring entry). The press is read from the raw pointer state so that a tap that
/// starts and ends inside one frame still moves the view.
fn steer(
    ctx: &Context,
    response: &Response,
    cx: &mut Cx<'_>,
    st: &mut UiState,
    (mapping, card): (Mapping, Rect),
    camera: &ViewView,
) {
    let (position, pressed, down) =
        ctx.input(|i| (i.pointer.interact_pos(), i.pointer.primary_pressed(), i.pointer.primary_down()));
    let Some(position) = position else {
        st.minimap_grab = None;
        return;
    };
    let before = if pressed && response.hovered() {
        st.minimap_grab = Some(Grab::begin(mapping, card, camera, position));
        camera.scroll
    } else if let Some(grab) = &st.minimap_grab {
        grab.target()
    } else {
        return;
    };
    let Some(grab) = st.minimap_grab.as_mut() else { return };
    let target = grab.aim(position);
    if target != before {
        cx.run("view.set", json!({ "scroll": target }));
    }
    if !down {
        st.minimap_grab = None;
    }
}

/// The wheel, or a pinch, zooms about the middle of the window (`view.zoom_at`).
fn zoom(ctx: &Context, response: &Response, cx: &mut Cx<'_>) {
    if !response.hovered() {
        return;
    }
    let (scroll, pinch) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
    if let Some(factor) = zoom_factor(scroll, pinch) {
        cx.run("view.zoom_at", json!({ "factor": factor }));
    }
}

/// Every mark, back to front. The rectangles go into one mesh (four vertices each, no outline tessellation), which is
/// what keeps a frame with a thousand marks cheap; a line ends the mesh so that the order of the elements holds.
fn paint_scene(painter: &Painter, mapping: &Mapping, scene: &Scene, dark: bool) {
    let clip = painter.clip_rect();
    let mut mesh = Mesh::default();
    for mark in scene.marks() {
        let at = mapping.rect_to_card(mark.bounds);
        if !clip.intersects(at) {
            continue;
        }
        let dot = at.size().min_elem() < MIN_OUTLINE;
        match mark.look {
            Look::Outline(tint) if !dot => add_outline(&mut mesh, at, tint.pick(dark)),
            Look::Line { tint, from, to } if !dot => {
                flush(painter, &mut mesh);
                painter.line_segment([mapping.to_card(from), mapping.to_card(to)], Stroke::new(1.0, tint.pick(dark)));
            }
            Look::Fill(tint) | Look::Outline(tint) | Look::Line { tint, .. } => {
                mesh.add_colored_rect(at_least(at, MIN_MARK), tint.pick(dark));
            }
        }
    }
    flush(painter, &mut mesh);
}

/// Hands the rectangles collected so far to the painter.
fn flush(painter: &Painter, mesh: &mut Mesh) {
    if !mesh.is_empty() {
        painter.add(Shape::mesh(std::mem::take(mesh)));
    }
}

/// A one point frame inside `rect`, as four thin rectangles.
fn add_outline(mesh: &mut Mesh, rect: Rect, color: Color32) {
    let edge = |min: Pos2, max: Pos2| Rect::from_min_max(min, max);
    let inner_top = rect.top() + 1.0;
    let inner_bottom = rect.bottom() - 1.0;
    mesh.add_colored_rect(edge(rect.left_top(), pos2(rect.right(), inner_top)), color);
    mesh.add_colored_rect(edge(pos2(rect.left(), inner_bottom), rect.right_bottom()), color);
    mesh.add_colored_rect(edge(pos2(rect.left(), inner_top), pos2(rect.left() + 1.0, inner_bottom)), color);
    mesh.add_colored_rect(edge(pos2(rect.right() - 1.0, inner_top), pos2(rect.right(), inner_bottom)), color);
}

/// The part of the world the window shows: a tinted rectangle with the accent colour as its edge.
fn paint_camera(painter: &Painter, mapping: &Mapping, camera: &ViewView, pal: &Palette) {
    let at = at_least(mapping.rect_to_card(visible(camera)), MIN_VIEW);
    painter.rect_filled(at, CornerRadius::same(radius::SM), pal.accent.gamma_multiply(VIEW_TINT));
    painter.rect_stroke(at, CornerRadius::same(radius::SM), Stroke::new(1.5, pal.accent), StrokeKind::Inside);
}

/// The rectangle, grown around its middle to at least `min` points on each side.
fn at_least(rect: Rect, min: f32) -> Rect {
    Rect::from_center_size(rect.center(), rect.size().max(Vec2::splat(min)))
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::*;

    #[test]
    fn a_tiny_mark_is_grown_to_a_dot_around_its_middle() {
        let tiny = Rect::from_min_size(pos2(10.0, 10.0), vec2(0.2, 0.2));
        let dot = at_least(tiny, 1.5);
        assert_eq!(dot.size(), vec2(1.5, 1.5));
        assert!((dot.center() - tiny.center()).length() < 1e-5);
        let big = Rect::from_min_size(pos2(0.0, 0.0), vec2(40.0, 20.0));
        assert_eq!(at_least(big, 1.5), big, "a mark that is large enough is as it was");
    }

    #[test]
    fn the_card_is_larger_for_a_finger_and_smaller_on_a_phone() {
        let desktop = card_size(&Layout::default());
        let tablet = card_size(&Layout { compact: true, ..Layout::default() });
        let phone = card_size(&Layout { compact: true, narrow: true, ..Layout::default() });
        assert!(tablet.x > desktop.x && phone.x < desktop.x);
    }
}
