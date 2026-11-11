//! Overlay state: what the render crate draws above the scene while the user interacts.
//!
//! [`OverlayParts`] is written by the tool machines (marquee rectangle, drawing preview, guides, ...).
//! [`Interaction::overlay`](super::Interaction::overlay) turns it, with the selection and the hover, into an
//! [`InteractionOverlay`]: a `drafftink_canvas::Overlay` (selection boxes, handles, hover outline, marquee, guides,
//! binding highlights) plus the parts that the canvas library has no field for (preview element, faded
//! elements, badges, laser trail).

use drafftink_canvas::shape::{element_box, shape_of};
use drafftink_canvas::{
    BindCandidate, Handle, HoverOutline, LassoOverlay, MarqueeOverlay, Overlay, SelectionBox, SelectionFrame,
    TransformerOverlay,
};
use drafftink_geom::guides::SmartGuide;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_scene::{Element, ElementId, SceneRead};

use super::world::World;
use crate::session::Session;

/// A label that follows the pointer: the angle while rotating, the size while resizing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Badge {
    /// Rotation about `center`, in degrees.
    Angle {
        /// The point the selection turns about (world).
        center: Point,
        /// The angle shown.
        degrees: f64,
    },
    /// The size of the resized selection.
    Size {
        /// Width in world units.
        w: f64,
        /// Height in world units.
        h: f64,
    },
}

/// One point of the laser trail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaserPoint {
    /// World position.
    pub at: Point,
    /// Time on the caller's clock (ms). The trail fades from this by the clock, not by frame count.
    pub t_ms: f64,
}

/// Overlay state owned by the tool machines. Everything is in world space.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OverlayParts {
    /// The marquee being dragged.
    pub marquee: Option<Rect>,
    /// The path of the lasso being drawn, in drawing order. Empty when there is none.
    pub lasso: Vec<Point>,
    /// Smart guides of the running move or resize, or of a drawing snap.
    pub guides: Vec<SmartGuide>,
    /// Elements to highlight: an arrow end would bind to them, or a drag would drop into a frame.
    pub highlight: Vec<ElementId>,
    /// The element being drawn, built from the points collected so far with the style defaults.
    pub preview: Option<Element>,
    /// Elements the eraser path touches: draw them faded. Nothing is deleted yet.
    pub faded: Vec<ElementId>,
    /// The angle or size label.
    pub badge: Option<Badge>,
    /// The handle being dragged.
    pub active_handle: Option<Handle>,
    /// The local laser trail.
    pub laser: Vec<LaserPoint>,
}

impl OverlayParts {
    /// Drops everything a gesture shows. Not the laser trail, which fades by the clock.
    pub fn clear_gesture(&mut self) {
        let laser = std::mem::take(&mut self.laser);
        *self = OverlayParts { laser, ..OverlayParts::default() };
    }

    /// True when a gesture shows something.
    pub fn has_gesture_state(&self) -> bool {
        self.marquee.is_some()
            || !self.lasso.is_empty()
            || !self.guides.is_empty()
            || !self.highlight.is_empty()
            || self.preview.is_some()
            || !self.faded.is_empty()
            || self.badge.is_some()
            || self.active_handle.is_some()
    }
}

/// Everything to draw above the scene for the current interaction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InteractionOverlay {
    /// Selection boxes, transformer handles, hover outline, marquee, guides and binding highlights, for the
    /// overlay pass of the render crate (`OverlaySink`).
    pub canvas: Overlay,
    /// The element being drawn. Draw it like any element.
    pub preview: Option<Element>,
    /// Elements to draw faded (the eraser's soft delete preview).
    pub faded: Vec<ElementId>,
    /// The angle or size badge.
    pub badge: Option<Badge>,
    /// The laser trail, oldest first. The shell drops points older than the fade time.
    pub laser: Vec<LaserPoint>,
}

/// Builds the overlay from the parts, the selection and the hover.
pub(super) fn build(world: &World, s: &Session, parts: &OverlayParts, hover: Option<&ElementId>) -> InteractionOverlay {
    let cfg = &world.config;
    let store = s.store();
    let selection = s.selection();
    let frame = SelectionFrame::of(store, selection.iter());
    let mut o = Overlay { zoom: world.viewport.zoom(), ..Overlay::default() };
    let linear = matches!(frame, SelectionFrame::Linear { .. });
    if !linear && selection.len() <= cfg.max_selection_boxes {
        o.selection = selection
            .iter()
            .filter_map(|id| store.get(id))
            .filter(|e| !e.deleted)
            .map(|e| SelectionBox { id: e.id.clone(), obox: element_box(e) })
            .collect();
    }
    if !frame.is_none() {
        o.transformer = Some(TransformerOverlay {
            handles: frame.handles(&world.viewport, &cfg.transformer),
            frame,
            handle_size_px: cfg.transformer.handle_size_px,
            active: parts.active_handle.clone(),
        });
    }
    if let Some(id) = hover
        && !selection.contains(id)
        && let Some(shape) = store.get(id).and_then(shape_of)
    {
        o.hover = Some(HoverOutline { id: id.clone(), shape });
    }
    o.marquee = parts.marquee.map(|rect| MarqueeOverlay { rect });
    o.lasso = (!parts.lasso.is_empty()).then(|| LassoOverlay { points: parts.lasso.clone() });
    o.guides = parts.guides.clone();
    o.bind_candidates = parts
        .highlight
        .iter()
        .filter_map(|id| store.get(id).filter(|e| !e.deleted))
        .filter_map(|e| shape_of(e).map(|shape| BindCandidate { target: e.id.clone(), shape }))
        .collect();
    InteractionOverlay {
        canvas: o,
        preview: parts.preview.clone(),
        faded: parts.faded.clone(),
        badge: parts.badge,
        laser: parts.laser.clone(),
    }
}
