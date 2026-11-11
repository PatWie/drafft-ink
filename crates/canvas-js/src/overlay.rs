//! From the canvas crate's description of the overlay to the render crate's overlay pass.
//!
//! The canvas decides what to show (selection, transformer, hover, marquee, highlights, remote cursors); the render
//! crate decides how it looks. The render crate sits below the canvas and cannot name its `Overlay`, so this glue
//! is the one place that knows both (`docs/canvas-api.md`, "Overlay").

use drafftink_canvas::{Handle, Overlay, SelectionFrame};
use drafftink_geom::select::HandleSet;
use drafftink_render::{BindingHighlight, PointHandle, PointKind, RemoteCursor, SelectionBox};

use crate::dto::primitive_path;

/// The handle set a box offers, from the handles the canvas lists as visible. Edge handles are drawn all or
/// none: a box whose sides are all long enough shows four, anything smaller shows none.
fn handle_set(handles: &[drafftink_canvas::HandlePos]) -> HandleSet {
    let count = |f: fn(&Handle) -> bool| handles.iter().filter(|h| f(&h.handle)).count();
    HandleSet {
        corners: count(|h| matches!(h, Handle::TopLeft | Handle::TopRight | Handle::BottomLeft | Handle::BottomRight))
            > 0,
        edges: count(|h| matches!(h, Handle::Top | Handle::Right | Handle::Bottom | Handle::Left)) == 4,
        rotate: count(|h| matches!(h, Handle::Rotate)) > 0,
    }
}

/// The render crate's overlay for `overlay`.
pub(crate) fn to_render(overlay: &Overlay) -> drafftink_render::Overlay {
    let mut out = drafftink_render::Overlay::default();
    let none = HandleSet { corners: false, edges: false, rotate: false };
    let single_box =
        overlay.transformer.as_ref().is_some_and(|t| matches!(t.frame, SelectionFrame::Box { multi: false, .. }));
    // One selected element: its box carries the handles. Several: each element gets a plain box and the frame
    // around all of them carries the handles.
    if !single_box {
        out.selection.extend(overlay.selection.iter().map(|b| SelectionBox {
            obox: b.obox,
            handles: none,
            dashed: true,
        }));
    }
    if let Some(t) = &overlay.transformer {
        match &t.frame {
            SelectionFrame::Box { obox, .. } => {
                out.selection.push(SelectionBox { obox: *obox, handles: handle_set(&t.handles), dashed: true });
            }
            SelectionFrame::Linear { points, closed, .. } => {
                let last = points.len().saturating_sub(1);
                out.point_handles.extend(t.handles.iter().map(|h| {
                    let kind = match &h.handle {
                        Handle::Midpoint { .. } => PointKind::Midpoint,
                        Handle::Point { index, .. } if !closed && (*index == 0 || *index == last) => PointKind::End,
                        _ => PointKind::Vertex,
                    };
                    PointHandle { at: h.world, kind }
                }));
            }
            SelectionFrame::None => {}
        }
    }
    if let Some(h) = &overlay.hover {
        out.hover.push(match &h.shape {
            drafftink_geom::Primitive::Rect(r) => r.frame,
            drafftink_geom::Primitive::Ellipse(e) => e.frame,
            drafftink_geom::Primitive::Diamond(d) => d.frame,
            other => drafftink_geom::OrientedBox::from_rect(drafftink_geom::Outline::bounds(other)),
        });
    }
    out.marquee = overlay.marquee.map(|m| m.rect);
    out.guides = overlay.guides.clone();
    out.binding = overlay
        .bind_candidates
        .iter()
        .map(|c| BindingHighlight { outline: primitive_path(&c.shape), at: None })
        .collect();
    out.cursors = overlay.remote_cursors.iter().map(|c| RemoteCursor { at: c.pos, color: c.color.into() }).collect();
    out
}
