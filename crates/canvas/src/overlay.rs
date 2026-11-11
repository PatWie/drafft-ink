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
use drafftink_scene::{Color, Element, ElementId, SceneRead};

use crate::intent::Handle;
use crate::shape::element_box;
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

/// The link icon of an element that has a link: a small square beside its top-right corner.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkIcon {
    /// The element that holds the link.
    pub element: ElementId,
    /// The upright square of the icon in world units. Its centre turns with the element
    /// (`drafftink_geom::link_icon`).
    pub rect: Rect,
    /// True for a link to another element in the document (`#id`), false for a web address: the icon differs.
    pub to_element: bool,
}

impl LinkIcon {
    /// The icon of `e` at `zoom`.
    pub(crate) fn of(e: &Element, zoom: f64) -> Self {
        Self {
            element: e.id.clone(),
            rect: crate::index::icon_rect_of(e, zoom),
            to_element: e.link.as_deref().is_some_and(|l| l.starts_with(ELEMENT_LINK_PREFIX)),
        }
    }
}

/// A link that starts with this character points at another element of the document (`#id`).
const ELEMENT_LINK_PREFIX: char = '#';

/// One point of a collaborator's pointer trail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailPoint {
    /// World position.
    pub pos: Point,
    /// Milliseconds since the point was recorded; the trail fades with it.
    pub age_ms: f64,
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
    /// The recent path of their pointer, oldest first (a laser trail). Empty: none.
    pub trail: Vec<TrailPoint>,
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

/// A collaborator's laser trail.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTrail {
    /// The collaborator.
    pub user: String,
    /// Their colour.
    pub color: Color,
    /// The path, oldest first.
    pub points: Vec<TrailPoint>,
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
    /// Remote laser trails.
    pub remote_trails: Vec<RemoteTrail>,
    /// Link icons of the elements in view that have a link and are not selected.
    pub link_icons: Vec<LinkIcon>,
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
            && self.remote_trails.is_empty()
            && self.link_icons.is_empty()
    }
}

/// The parts of the overlay that come from collaborators.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteParts {
    /// Pointers.
    pub cursors: Vec<RemoteCursor>,
    /// Selections as boxes.
    pub selections: Vec<RemoteSelection>,
    /// Laser trails.
    pub trails: Vec<RemoteTrail>,
}

/// Works out what to show for `users`: a cursor for each one that has a pointer, a box for each element they have
/// selected that still exists (at most `max_boxes` per user), and their trails. A host that builds its own
/// [`Overlay`] (the editor does) calls this with its collaborators; [`crate::Canvas::overlay`] calls it with the
/// ones set by [`crate::Canvas::set_remote_users`].
pub fn remote_parts<R: SceneRead>(scene: &R, users: &[RemoteUser], max_boxes: usize) -> RemoteParts {
    let mut out = RemoteParts::default();
    for u in users {
        if let Some(pos) = u.cursor {
            out.cursors.push(RemoteCursor { user: u.id.clone(), name: u.name.clone(), color: u.color, pos });
        }
        if !u.selection.is_empty() {
            let boxes: Vec<OrientedBox> = u
                .selection
                .iter()
                .filter_map(|id| scene.get(id))
                .filter(|e| !e.deleted)
                .take(max_boxes)
                .map(element_box)
                .collect();
            out.selections.push(RemoteSelection {
                user: u.id.clone(),
                color: u.color,
                truncated: u.selection.len() > max_boxes,
                boxes,
            });
        }
        if !u.trail.is_empty() {
            out.trails.push(RemoteTrail { user: u.id.clone(), color: u.color, points: u.trail.clone() });
        }
    }
    out
}

/// The painter side of the overlay. The render crate's overlay pass implements it (render sits below the canvas and
/// cannot name [`Overlay`], so the app glues the two: `impl OverlaySink for OverlayPass`).
pub trait OverlaySink {
    /// Draws the overlay for a viewport, in an overlay layer above the scene.
    fn draw_overlay(&mut self, overlay: &Overlay, viewport: &Viewport);
}
