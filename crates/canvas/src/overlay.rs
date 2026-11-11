//! The overlay description: everything drawn on top of the scene that is not an element.
//!
//! The canvas decides *what* to show and *where* (selection boxes, handles, hover outline, marquee, snapping
//! guides, binding highlights, remote cursors and selections); the render crate's overlay pass decides *how* to
//! paint it. An [`Overlay`] is plain data in world space. Sizes that must stay constant on screen (handle size, the
//! rotation offset) are already applied to the handle positions, and `zoom` is included for the line widths and
//! dash lengths the painter derives from it.
//!
//! Redrawing only the overlay is cheap, and a host can skip even that: [`crate::Canvas::take_dirty`] says whether
//! anything in the overlay changed since the last call.

use drafftink_geom::guides::SmartGuide;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_geom::{OrientedBox, Primitive};
use drafftink_scene::{Color, ElementId};

use crate::intent::Handle;
use crate::transformer::{HandlePos, SelectionFrame};
use crate::viewport::Viewport;

/// The box around one selected element.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionBox {
    /// The element.
    pub id: ElementId,
    /// Its rotated box.
    pub obox: OrientedBox,
}

/// The transformer frame with its handles.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformerOverlay {
    /// What the handles wrap.
    pub frame: SelectionFrame,
    /// The visible handles at the current zoom, in hit priority order.
    pub handles: Vec<HandlePos>,
    /// Drawn size of a handle in logical pixels.
    pub handle_size_px: f64,
    /// The handle being dragged, if any.
    pub active: Option<Handle>,
}

/// The outline of the hovered element.
#[derive(Debug, Clone, PartialEq)]
pub struct HoverOutline {
    /// The element.
    pub id: ElementId,
    /// Its outline in world space; the painter strokes it.
    pub shape: Primitive,
}

/// An element an arrow end would bind to if released now.
#[derive(Debug, Clone, PartialEq)]
pub struct BindCandidate {
    /// The candidate target.
    pub target: ElementId,
    /// Its outline in world space.
    pub shape: Primitive,
}

/// The marquee being dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarqueeOverlay {
    /// The rectangle in world space (normalised).
    pub rect: Rect,
}

/// The lasso being drawn: the path of the pointer so far. The painter closes it.
#[derive(Debug, Clone, PartialEq)]
pub struct LassoOverlay {
    /// The path in world space, in drawing order.
    pub points: Vec<Point>,
}

/// What the host knows about one collaborator.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteUser {
    /// A stable id for the collaborator.
    pub id: String,
    /// Name shown next to the cursor.
    pub name: Option<String>,
    /// Their colour.
    pub color: Color,
    /// Cursor position in world space, when they have one.
    pub cursor: Option<Point>,
    /// The elements they have selected.
    pub selection: Vec<ElementId>,
}

/// A collaborator's cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteCursor {
    /// The collaborator.
    pub user: String,
    /// Name to show.
    pub name: Option<String>,
    /// Their colour.
    pub color: Color,
    /// Position in world space.
    pub pos: Point,
}

/// A collaborator's selection, as boxes.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteSelection {
    /// The collaborator.
    pub user: String,
    /// Their colour.
    pub color: Color,
    /// One box per selected element, up to the configured cap.
    pub boxes: Vec<OrientedBox>,
    /// The selection had more elements than `boxes` holds.
    pub truncated: bool,
}

/// The overlay state the host sets: things only it knows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OverlayInputs {
    /// Smart guides to draw (from snapping during a drag).
    pub guides: Vec<SmartGuide>,
    /// Elements to highlight as binding candidates.
    pub bind_candidates: Vec<ElementId>,
    /// Collaborators.
    pub remote: Vec<RemoteUser>,
}

/// Everything to draw above the scene.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overlay {
    /// Zoom of the viewport the overlay was built for.
    pub zoom: f64,
    /// A box per selected element (omitted for large selections).
    pub selection: Vec<SelectionBox>,
    /// The frame and handles of the selection.
    pub transformer: Option<TransformerOverlay>,
    /// The hovered element's outline.
    pub hover: Option<HoverOutline>,
    /// The marquee being dragged.
    pub marquee: Option<MarqueeOverlay>,
    /// The lasso being drawn.
    pub lasso: Option<LassoOverlay>,
    /// Smart guides.
    pub guides: Vec<SmartGuide>,
    /// Binding highlights.
    pub bind_candidates: Vec<BindCandidate>,
    /// Remote cursors.
    pub remote_cursors: Vec<RemoteCursor>,
    /// Remote selections.
    pub remote_selections: Vec<RemoteSelection>,
}

impl Overlay {
    /// True when there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.selection.is_empty()
            && self.transformer.is_none()
            && self.hover.is_none()
            && self.marquee.is_none()
            && self.lasso.is_none()
            && self.guides.is_empty()
            && self.bind_candidates.is_empty()
            && self.remote_cursors.is_empty()
            && self.remote_selections.is_empty()
    }
}

/// The painter side of the overlay. The render crate's overlay pass implements it (render sits below the canvas and
/// cannot name [`Overlay`], so the app glues the two: `impl OverlaySink for OverlayPass`).
pub trait OverlaySink {
    /// Draws the overlay for a viewport, in an overlay layer above the scene.
    fn draw_overlay(&mut self, overlay: &Overlay, viewport: &Viewport);
}
