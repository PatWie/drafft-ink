//! Turns the interaction layer's description of what to show (`drafftink_canvas::Overlay`) into the render
//! crate's overlay (`drafftink_render::Overlay`). Render sits below canvas and cannot name its types, so
//! the app glues the two (the doc of `drafftink_canvas::overlay` says so).

use drafftink_canvas::{Handle, Overlay as CanvasOverlay, RemoteParts, SelectionFrame};
use drafftink_engine::interact::{AngleRays as EngineRays, Badge, EraserRing as EngineRing, InteractionOverlay};
use drafftink_geom::guides::SmartGuide;
use drafftink_geom::kurbo::{BezPath, Point, Shape};
use drafftink_geom::select::HandleSet;
use drafftink_geom::{OrientedBox, Primitive};
use drafftink_render::{
    AngleRays, BindingHighlight, EraserRing, LaserTrail, LinkMark, Overlay, PointHandle, PointKind, RemoteCursor,
    RemoteSelection, RotationGuide, SelectionBox, TrailPoint,
};

/// The handles of a selection box that only outlines: none.
const OUTLINE_ONLY: HandleSet = HandleSet { corners: false, edges: false, rotate: false };

/// Everything the overlay pass should draw for `canvas` and the other people in the room (`remote`, from
/// `drafftink_canvas::remote_parts`; the interaction layer's overlay leaves them empty), with `extra_guides`
/// appended (a drawing snap).
pub(crate) fn to_render(canvas: &CanvasOverlay, remote: &RemoteParts, extra_guides: &[SmartGuide]) -> Overlay {
    let mut out = Overlay::default();
    match canvas.transformer.as_ref().map(|t| &t.frame) {
        Some(SelectionFrame::Box { obox, set, multi }) => {
            if *multi {
                out.selection.extend(canvas.selection.iter().map(|b| SelectionBox {
                    obox: b.obox,
                    handles: OUTLINE_ONLY,
                    dashed: true,
                }));
            }
            out.selection.push(SelectionBox { obox: *obox, handles: *set, dashed: true });
        }
        Some(SelectionFrame::Linear { points, .. }) => {
            if let Some(t) = &canvas.transformer {
                out.point_handles
                    .extend(t.handles.iter().filter_map(|h| point_handle(&h.handle, h.world, points.len())));
            }
        }
        Some(SelectionFrame::None) | None => {}
    }
    if out.selection.is_empty() && out.point_handles.is_empty() {
        out.selection.extend(canvas.selection.iter().map(|b| SelectionBox {
            obox: b.obox,
            handles: OUTLINE_ONLY,
            dashed: true,
        }));
    }
    out.hover.extend(canvas.hover.iter().filter_map(|h| outline_box(&h.shape)));
    out.binding
        .extend(canvas.bind_candidates.iter().map(|c| BindingHighlight { outline: outline_path(&c.shape), at: None }));
    out.marquee = canvas.marquee.as_ref().map(|m| m.rect);
    out.lasso = canvas.lasso.iter().flat_map(|l| l.points.iter().copied()).collect();
    out.link_icons.extend(canvas.link_icons.iter().map(|i| LinkMark { rect: i.rect, to_element: i.to_element }));
    out.invisible.extend(canvas.invisible.iter().map(|i| outline_path(&i.shape)));
    add_remote(&mut out, remote);
    out.guides.extend(canvas.guides.iter().cloned());
    out.guides.extend(extra_guides.iter().cloned());
    out
}

/// Adds what only the interaction layer knows to `out`: the polar rays of an angle lock, the crosshair at a snapped
/// point, the rotation guides (from the angle badge of a rotation) and the eraser's radius ring.
pub(crate) fn add_interaction(out: &mut Overlay, interaction: &InteractionOverlay) {
    out.snap_point = interaction.snap_point;
    out.angle_rays = interaction.angle_rays.map(|r: EngineRays| AngleRays {
        origin: r.origin,
        step_degrees: r.step_degrees,
        locked_degrees: r.locked_degrees,
    });
    out.rotation = match interaction.badge {
        Some(Badge::Angle { center, degrees, snapped }) => Some(RotationGuide { center, degrees, snapped }),
        Some(Badge::Size { .. }) | None => None,
    };
    out.eraser = interaction.eraser.map(|e: EngineRing| EraserRing { at: e.at, radius_px: e.radius_px });
}

/// Other people's pointers, selection outlines and laser trails, in their colours.
fn add_remote(out: &mut Overlay, remote: &RemoteParts) {
    out.cursors.extend(remote.cursors.iter().map(|c| {
        let cursor = RemoteCursor::new(c.pos, c.color.into());
        match c.name.as_deref().filter(|n| !n.is_empty()) {
            Some(name) => cursor.named(name),
            None => cursor,
        }
    }));
    out.remote_selections
        .extend(remote.selections.iter().map(|s| RemoteSelection { color: s.color.into(), boxes: s.boxes.clone() }));
    out.trails.extend(remote.trails.iter().map(|t| LaserTrail {
        color: t.color.into(),
        points: t.points.iter().map(|p| TrailPoint { at: p.pos, age_ms: p.age_ms }).collect(),
    }));
}

fn point_handle(handle: &Handle, at: Point, count: usize) -> Option<PointHandle> {
    let kind = match handle {
        Handle::Point { index, .. } if *index == 0 || index + 1 == count => PointKind::End,
        Handle::Point { .. } => PointKind::Vertex,
        Handle::Midpoint { .. } => PointKind::Midpoint,
        _ => return None,
    };
    Some(PointHandle { at, kind })
}

/// The box to outline for a hovered shape. Lines and free paths have no box; their bounds stand in.
fn outline_box(shape: &Primitive) -> Option<OrientedBox> {
    use drafftink_geom::Outline;
    match shape {
        Primitive::Rect(r) => Some(r.frame),
        Primitive::Ellipse(e) => Some(e.frame),
        Primitive::Diamond(d) => Some(d.frame),
        Primitive::Polyline(_) | Primitive::Path(_) => Some(OrientedBox::from_rect(shape.bounds())),
    }
}

/// The outline of a binding target as a path in world space.
fn outline_path(shape: &Primitive) -> BezPath {
    match shape {
        Primitive::Rect(r) => polygon(&r.frame.corners(), true),
        Primitive::Diamond(d) => polygon(&d.vertices(), true),
        Primitive::Ellipse(e) => {
            let b = e.frame;
            let radii = (b.size.width / 2.0, b.size.height / 2.0);
            drafftink_geom::kurbo::Ellipse::new(b.center, radii, b.angle).to_path(0.1)
        }
        Primitive::Polyline(p) => polygon(&p.points, p.closed),
        Primitive::Path(p) => p.path.clone(),
    }
}

fn polygon(points: &[Point], closed: bool) -> BezPath {
    let mut path = BezPath::new();
    for (i, p) in points.iter().enumerate() {
        if i == 0 {
            path.move_to(*p);
        } else {
            path.line_to(*p);
        }
    }
    if closed {
        path.close_path();
    }
    path
}

#[cfg(test)]
mod tests;
