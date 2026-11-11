//! What the recognizers say: typed [`Intent`]s about what the user did, in terms of the scene.
//!
//! The canvas knows nothing about tools, selection rules or commands. An intent is a fact ("a drag started on this
//! handle", "the pointer now hovers this element"); the editor above decides what it means and which command or
//! gesture to run. All types serialise, so a JS host sees the same shapes.

use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_geom::select::{BoxHandle, Corner, Edge};
use drafftink_scene::{ElementId, GroupId};
use serde::{Deserialize, Serialize};

use crate::input::{Button, Modifiers, PointerId, PointerKind, Tilt};

/// A handle of the transformer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Handle {
    /// Top-left corner of the selection box.
    TopLeft,
    /// Middle of the top edge.
    Top,
    /// Top-right corner.
    TopRight,
    /// Middle of the right edge.
    Right,
    /// Bottom-right corner.
    BottomRight,
    /// Middle of the bottom edge.
    Bottom,
    /// Bottom-left corner.
    BottomLeft,
    /// Middle of the left edge.
    Left,
    /// The rotation handle above the top edge.
    Rotate,
    /// A vertex of the selected line or arrow.
    Point {
        /// The line or arrow.
        element: ElementId,
        /// Index into its points.
        index: usize,
    },
    /// The middle of the segment that joins point `segment` and the next one (the last segment of a closed line
    /// joins the last point and the first). Dragging it inserts a point.
    Midpoint {
        /// The line or arrow.
        element: ElementId,
        /// Index of the segment's first point.
        segment: usize,
    },
}

impl Handle {
    /// The box handle in the geometry crate's form, for the eight resize handles and the rotation handle.
    pub fn to_box(&self) -> Option<BoxHandle> {
        Some(match self {
            Handle::TopLeft => BoxHandle::Corner(Corner::TopLeft),
            Handle::TopRight => BoxHandle::Corner(Corner::TopRight),
            Handle::BottomRight => BoxHandle::Corner(Corner::BottomRight),
            Handle::BottomLeft => BoxHandle::Corner(Corner::BottomLeft),
            Handle::Top => BoxHandle::Edge(Edge::Top),
            Handle::Right => BoxHandle::Edge(Edge::Right),
            Handle::Bottom => BoxHandle::Edge(Edge::Bottom),
            Handle::Left => BoxHandle::Edge(Edge::Left),
            Handle::Rotate => BoxHandle::Rotate,
            Handle::Point { .. } | Handle::Midpoint { .. } => return None,
        })
    }

    /// The handle for a box handle of the geometry crate.
    pub fn from_box(h: BoxHandle) -> Handle {
        match h {
            BoxHandle::Corner(Corner::TopLeft) => Handle::TopLeft,
            BoxHandle::Corner(Corner::TopRight) => Handle::TopRight,
            BoxHandle::Corner(Corner::BottomRight) => Handle::BottomRight,
            BoxHandle::Corner(Corner::BottomLeft) => Handle::BottomLeft,
            BoxHandle::Edge(Edge::Top) => Handle::Top,
            BoxHandle::Edge(Edge::Right) => Handle::Right,
            BoxHandle::Edge(Edge::Bottom) => Handle::Bottom,
            BoxHandle::Edge(Edge::Left) => Handle::Left,
            BoxHandle::Rotate => Handle::Rotate,
        }
    }
}

/// What is under a pointer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    /// The pointer is not over the canvas (it left, or it is a touch that has lifted).
    None,
    /// The pointer is over the canvas and nothing is under it.
    Empty,
    /// An element: the topmost hit.
    Element {
        /// The hit element.
        id: ElementId,
        /// The group a click selects as a unit: the outermost group of the element, or the next group inside the
        /// group the user has entered ([`crate::Canvas::set_entered_group`]). `None` means the element itself.
        unit_group: Option<GroupId>,
    },
    /// A handle of the transformer. Handles win over elements.
    Handle(Handle),
    /// The link icon of an element that has a link and is not selected. It sits beside the top-right corner of the
    /// element, so it can lie outside the element's body; it wins over the element under it. A click on it opens
    /// the link (`element.open_link`); a host that has no use for it treats it as [`Target::Empty`] (see
    /// [`Target::without_link_icon`]).
    LinkIcon {
        /// The element that holds the link.
        id: ElementId,
    },
}

impl Target {
    /// The element id when the target is an element. A link icon is not the element.
    pub fn element(&self) -> Option<&ElementId> {
        match self {
            Target::Element { id, .. } => Some(id),
            _ => None,
        }
    }

    /// The element whose link icon this is.
    pub fn link_icon(&self) -> Option<&ElementId> {
        match self {
            Target::LinkIcon { id } => Some(id),
            _ => None,
        }
    }

    /// This target, except that a link icon counts as empty space: for the parts of a host that give the icon no
    /// meaning (a press, a drag or a context menu on it behave as they did before the icon was a target).
    pub fn without_link_icon(&self) -> &Target {
        match self {
            Target::LinkIcon { .. } => &Target::Empty,
            other => other,
        }
    }
}

/// The mouse cursor the host should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Cursor {
    /// The arrow.
    Default,
    /// Over an element: it can be grabbed and moved.
    Move,
    /// An open hand (hand tool).
    Grab,
    /// A pointing hand: over a link icon, which a click opens.
    Pointer,
    /// A closed hand (panning).
    Grabbing,
    /// The I-beam: over text, over a label that can be edited, and with the text tool.
    Text,
    /// A crosshair (drawing tools, line point handles).
    Crosshair,
    /// Resize up/down.
    ResizeNs,
    /// Resize left/right.
    ResizeEw,
    /// Resize along the diagonal from top-right to bottom-left.
    ResizeNesw,
    /// Resize along the diagonal from top-left to bottom-right.
    ResizeNwse,
    /// Rotate.
    Rotate,
}

/// A position in both spaces.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pos {
    /// Logical screen pixels.
    pub screen: Point,
    /// World units.
    pub world: Point,
}

/// Who did it and how, for every pointer-driven intent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// The pointer.
    pub pointer: PointerId,
    /// Its device kind.
    pub kind: PointerKind,
    /// The button involved (primary for touch and pen contact).
    pub button: Button,
    /// Modifiers at the time of the event.
    pub modifiers: Modifiers,
    /// Pen pressure `0..=1` (0.5 when unknown).
    pub pressure: f32,
    /// Pen tilt.
    pub tilt: Tilt,
    /// Time of the event on the caller's clock.
    pub time_ms: f64,
}

/// What produced a pan or zoom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViewSource {
    /// Wheel or trackpad scroll.
    Wheel,
    /// Two fingers (pinch and drag).
    Touch,
    /// Middle-button drag.
    MiddleButton,
    /// A primary drag while the pointer mode is [`crate::PointerMode::Hand`].
    HandTool,
}

/// A recognized user action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Intent {
    /// A pointer went down. Sent at once, before it is known whether this becomes a click or a drag. Editors select
    /// on press so that the drag that follows moves the right thing.
    Press {
        /// Where.
        at: Pos,
        /// What was under the pointer.
        target: Target,
        /// Pointer details.
        meta: Meta,
    },
    /// A press and release without a drag.
    Click {
        /// Where the pointer was released.
        at: Pos,
        /// What was under the pointer when it went down.
        target: Target,
        /// Pointer details.
        meta: Meta,
    },
    /// The second click of a double click (always preceded by its `Click`).
    DoubleClick {
        /// Where.
        at: Pos,
        /// The target of both clicks.
        target: Target,
        /// Pointer details.
        meta: Meta,
    },
    /// The pointer stayed down without moving past the long-press time (touch and pen; also mouse when enabled).
    /// The release that follows is not a click.
    LongPress {
        /// Where.
        at: Pos,
        /// What is under the pointer.
        target: Target,
        /// Pointer details.
        meta: Meta,
    },
    /// Secondary button released without a drag.
    ContextMenu {
        /// Where.
        at: Pos,
        /// What is under the pointer.
        target: Target,
        /// Pointer details.
        meta: Meta,
    },
    /// The pointer moved past the drag slop. `origin` is where it went down and `target` what was there; an
    /// `Empty` target starts a marquee.
    DragStart {
        /// Where the press happened.
        origin: Pos,
        /// Where the pointer is now.
        at: Pos,
        /// What was under the press.
        target: Target,
        /// Pointer details.
        meta: Meta,
    },
    /// The pointer moved during a drag. Deltas are totals since the press.
    DragUpdate {
        /// Where the press happened.
        origin: Pos,
        /// Where the pointer is now.
        at: Pos,
        /// Total movement in screen pixels.
        delta_screen: Vec2,
        /// Total movement in world units.
        delta_world: Vec2,
        /// What was under the press.
        target: Target,
        /// Pointer details.
        meta: Meta,
    },
    /// The drag ended: released, or cancelled (`cancelled`, by Escape, a second finger or a system cancel).
    DragEnd {
        /// Where the press happened.
        origin: Pos,
        /// Where the pointer was at the end.
        at: Pos,
        /// Total movement in screen pixels.
        delta_screen: Vec2,
        /// Total movement in world units.
        delta_world: Vec2,
        /// What was under the press.
        target: Target,
        /// True when the drag was aborted and should leave no trace.
        cancelled: bool,
        /// Pointer details.
        meta: Meta,
    },
    /// The hover target changed. Sent only on a change, never per move: an idle pointer costs no redraw.
    Hover {
        /// The previous target.
        from: Target,
        /// The new target (`Target::None` when the pointer left).
        to: Target,
        /// Where the pointer is.
        at: Pos,
        /// The cursor to show for `to`.
        cursor: Cursor,
    },
    /// Move the view by `delta` logical pixels (content follows the fingers or pointer). The canvas facade applies
    /// it to its viewport before returning it.
    Pan {
        /// Movement in screen pixels.
        delta: Vec2,
        /// What produced it.
        source: ViewSource,
    },
    /// Multiply the zoom by `factor` around the screen point `at`. The facade clamps the resulting zoom.
    Zoom {
        /// Fixed point in screen pixels.
        at: Point,
        /// Zoom multiplier.
        factor: f64,
        /// What produced it.
        source: ViewSource,
    },
    /// A pan or pinch gesture that went on over several events ended (fingers lifted, button released). A host
    /// that reports the camera to its session sends the final camera now.
    ViewEnd,
}
