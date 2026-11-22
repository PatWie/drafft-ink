//! Overlay state: what the render crate draws above the scene while the user interacts.
//!
//! [`OverlayParts`] is written by the tool machines (marquee rectangle, drawing preview, guides, ...).
//! [`Interaction::overlay`](super::Interaction::overlay) turns it, with the selection and the hover, into an
//! [`InteractionOverlay`]: a `drafftink_canvas::Overlay` (selection boxes, handles, hover outline, marquee, guides,
//! binding highlights) plus the parts that the canvas library has no field for (preview element, faded
//! elements, badges, laser trail, angle rays, snap point, eraser ring).

use drafftink_canvas::shape::{selection_box, shape_of};
use drafftink_canvas::{
    BindCandidate, Handle, HoverOutline, LassoOverlay, MarqueeOverlay, Overlay, PointerKind, SelectionBox,
    SelectionFrame, TransformerOverlay,
};
use drafftink_geom::OrientedBox;
use drafftink_geom::guides::SmartGuide;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_scene::{Element, ElementId, MarkerColor, SceneRead};

pub use drafftink_canvas::LinkIcon;

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
        /// The angle is rounded to a step (Shift): the guides turn green.
        snapped: bool,
    },
    /// The size of the resized selection.
    Size {
        /// Width in world units.
        w: f64,
        /// Height in world units.
        h: f64,
    },
}

/// The polar rays of an angle lock: a line or arrow end that is locked to a multiple of `step_degrees` about
/// `origin`. The painter draws a faint ray at each multiple and the locked one strongly, with an arc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngleRays {
    /// The neighbouring point the end turns about (world).
    pub origin: Point,
    /// The step of the lock, in degrees.
    pub step_degrees: f64,
    /// The direction the end is locked to, in degrees from the +x axis, clockwise on the screen (y points down).
    pub locked_degrees: f64,
}

/// The ring that shows the radius of the eraser round the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EraserRing {
    /// The pointer (world).
    pub at: Point,
    /// The radius in screen pixels. It is a screen size, so it does not grow with the zoom.
    pub radius_px: f64,
}

/// One point of the laser trail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaserPoint {
    /// World position.
    pub at: Point,
    /// Time on the caller's clock (ms). The trail fades from this by the clock, not by frame count.
    pub t_ms: f64,
}

/// What the highlighter shows while it is dragged: the path it has made, and the characters it would highlight (the
/// elements it would highlight are in [`OverlayParts::highlight`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarkerPreview {
    /// The path of the pointer, in world space.
    pub trail: Vec<Point>,
    /// The crossed characters of the texts, as quadrilaterals in world space (a rotated text gives rotated ones).
    pub bands: Vec<[Point; 4]>,
    /// The colour the drag applies, `None` when it removes the highlight.
    pub color: Option<MarkerColor>,
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
    /// The polar rays of the angle lock of a line or arrow end.
    pub angle_rays: Option<AngleRays>,
    /// The point a dragged point snapped to (grid or guides), marked with a crosshair.
    pub snap_point: Option<Point>,
    /// The eraser's radius ring, at the pointer.
    pub eraser: Option<EraserRing>,
    /// The trail and the crossed characters of the highlighter.
    pub marker: Option<MarkerPreview>,
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
            || self.angle_rays.is_some()
            || self.snap_point.is_some()
            || self.eraser.is_some()
            || self.marker.is_some()
    }
}

/// What the crop editor shows: the whole picture dimmed (reduced opacity under a dark scrim), the draft over it, the window with its handles.
#[derive(Debug, Clone, PartialEq)]
pub struct CropView {
    /// The image being cropped. The scene leaves it out while the editor draws its own picture of it.
    pub element: ElementId,
    /// The image as the draft would leave it: draw it like any element.
    pub shown: Element,
    /// The whole picture, uncropped, to draw dimmed under `shown`.
    pub whole: Element,
    /// The crop window in world space (it turns with the image). It carries the eight handles.
    pub window: OrientedBox,
    /// The whole picture in world space, to outline.
    pub picture: OrientedBox,
    /// A drag runs (a handle or the picture): the shell lays the rule-of-thirds grid over the window.
    pub grid: bool,
    /// The pointer that was used last. A finger or a pen reaches handles from further away than a mouse, so the
    /// shell draws them larger (the size of what reacts).
    pub pointer: PointerKind,
}

/// What the paint editor shows: the image with the stroke being dragged already painted on it.
#[derive(Debug, Clone, PartialEq)]
pub struct PaintView {
    /// The image being painted on. The scene leaves it out while the editor draws its own picture of it.
    pub element: ElementId,
    /// The image as it is now with the draft stroke added to its `paint`: draw it like any element, on top of the
    /// veil. The stroke that is stored when the pointer is released is this same stroke.
    pub shown: Element,
    /// Everything else on the canvas is dimmed (a translucent theme veil) and cannot be picked while the editor is
    /// open.
    pub veil: bool,
}

/// Everything to draw above the scene for the current interaction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InteractionOverlay {
    /// Selection boxes, transformer handles, hover outline, marquee, guides, binding highlights and link icons, for the
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
    /// The polar rays of the angle lock.
    pub angle_rays: Option<AngleRays>,
    /// The point a dragged point snapped to.
    pub snap_point: Option<Point>,
    /// The eraser's radius ring.
    pub eraser: Option<EraserRing>,
    /// The highlighter's trail and the characters it would highlight.
    pub marker: Option<MarkerPreview>,
    /// The crop editor, while it is open. The overlay then has no selection box, handles or hover outline of its
    /// own: the window carries the handles.
    pub crop: Option<CropView>,
    /// The paint editor, while it is open. The overlay then has no selection box, handles or hover outline of its
    /// own. The brush ring is `eraser`.
    pub paint: Option<PaintView>,
}

/// Builds the overlay from the parts, the selection and the hover.
pub(super) fn build(
    world: &World,
    s: &Session,
    parts: &OverlayParts,
    hover: Option<&ElementId>,
    crop: Option<CropView>,
    paint: Option<(PaintView, Option<EraserRing>)>,
) -> InteractionOverlay {
    if crop.is_some() {
        let canvas = Overlay { zoom: world.viewport.zoom(), ..Overlay::default() };
        return InteractionOverlay { canvas, crop, ..InteractionOverlay::default() };
    }
    if let Some((view, ring)) = paint {
        let canvas = Overlay { zoom: world.viewport.zoom(), ..Overlay::default() };
        return InteractionOverlay { canvas, eraser: ring, paint: Some(view), ..InteractionOverlay::default() };
    }
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
            .map(|e| SelectionBox { id: e.id.clone(), obox: selection_box(e) })
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
    // Elements with a link, in view and not selected (a selected element shows its handles there instead).
    o.link_icons =
        s.tab()
            .doc
            .scene_index()
            .link_icons(store, &world.viewport.visible_rect(), world.viewport.zoom(), |e| selection.contains(&e.id));
    // Elements that draw nothing get a dotted outline while the document can be edited, unless they are selected.
    // A view-only document is for looking at, and an element left invisible on purpose should stay so there.
    if !s.view_only {
        o.invisible = s
            .tab()
            .doc
            .scene_index()
            .invisible_outlines(store, &world.viewport.visible_rect(), |e| selection.contains(&e.id));
    }
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
        angle_rays: parts.angle_rays,
        snap_point: parts.snap_point,
        eraser: parts.eraser,
        marker: parts.marker.clone(),
        crop: None,
        paint: None,
    }
}
