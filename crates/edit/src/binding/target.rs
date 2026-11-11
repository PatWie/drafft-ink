//! What an arrow can attach to, and the shape of it.

use drafftink_geom::binding::{BindKind, BindTarget, binding_gap};
use drafftink_scene::{Element, ElementId, ElementKind, SceneRead};

/// A bind target read from the scene: its outline, binding gap and area.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The element id.
    pub id: ElementId,
    /// Outline and frame.
    pub target: BindTarget,
    /// Gap between an orbit end and the outline: `5 + stroke / 2`.
    pub gap: f64,
    /// Width times height of the box, used to compare shapes.
    pub area: f64,
}

/// The outline of an element an arrow can bind to, or `None` when it is not bindable.
///
/// Rectangles, diamonds, ellipses, images, frames and free text can be targets. Text that is the
/// label of a container, arrows, lines, freehand strokes and formulas cannot. Deleted elements are
/// not filtered here; see [`resolve`].
pub fn bind_target(e: &Element) -> Option<BindTarget> {
    let kind = match &e.kind {
        ElementKind::Rectangle | ElementKind::Image(_) | ElementKind::Frame(_) => BindKind::Rect,
        ElementKind::Text(t) if t.container_id.is_none() => BindKind::Rect,
        ElementKind::Diamond => BindKind::Diamond,
        ElementKind::Ellipse => BindKind::Ellipse,
        _ => return None,
    };
    let t = &e.transform;
    let frame = t.obox();
    let radius = match &e.kind {
        ElementKind::Rectangle | ElementKind::Diamond => e.style.corner_radius,
        _ => 0.0,
    };
    let target = BindTarget::new(kind, frame).with_corner_radius(radius);
    target.is_valid().then_some(target)
}

/// True when `e` is alive and an arrow can attach to it.
pub fn is_bindable(e: &Element) -> bool {
    !e.deleted && bind_target(e).is_some()
}

/// The target `id` names, if it exists, is alive and is bindable. A binding to anything else is
/// dangling: the solver leaves such an end where it is.
pub fn resolve<R: SceneRead>(scene: &R, id: &ElementId) -> Option<Resolved> {
    resolve_element(scene.get(id)?)
}

pub(crate) fn resolve_element(e: &Element) -> Option<Resolved> {
    if e.deleted {
        return None;
    }
    let target = bind_target(e)?;
    Some(Resolved {
        id: e.id.clone(),
        gap: binding_gap(e.style.stroke_width),
        area: target.frame.size.width * target.frame.size.height,
        target,
    })
}
