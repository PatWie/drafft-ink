//! What crosses the boundary to JS, as plain serialisable values.
//!
//! Coordinates are `[x, y]` pairs. Names are `camelCase`. Types that the canvas crate already serialises (a
//! `Handle`, an `Intent`, an `InputEvent`) are passed through with their own layout, which `docs/canvas-api.md`
//! describes.

use drafftink_canvas::{
    Button, Camera, ElementEvent, EventKind, Handle, Hit, HitPart, Modifiers, Node, Overlay, PointerKind,
    SelectionFrame, Viewport,
};
use drafftink_geom::kurbo::{BezPath, Point, Rect, Vec2};
use drafftink_geom::{OrientedBox, Primitive};
use serde::Serialize;

use crate::error::ApiError;

/// `[x, y]`.
pub type Xy = [f64; 2];

fn xy(p: Point) -> Xy {
    [p.x, p.y]
}

fn vec_xy(v: Vec2) -> Xy {
    [v.x, v.y]
}

/// The event names JS uses, in the order of [`EventKind`].
const EVENT_NAMES: [(&str, EventKind); 13] = [
    ("pointerdown", EventKind::PointerDown),
    ("click", EventKind::Click),
    ("dblclick", EventKind::DoubleClick),
    ("longpress", EventKind::LongPress),
    ("contextmenu", EventKind::ContextMenu),
    ("dragstart", EventKind::DragStart),
    ("dragmove", EventKind::DragMove),
    ("dragend", EventKind::DragEnd),
    ("pointerenter", EventKind::PointerEnter),
    ("pointerleave", EventKind::PointerLeave),
    ("transformstart", EventKind::TransformStart),
    ("transform", EventKind::Transform),
    ("transformend", EventKind::TransformEnd),
];

/// The event kind a JS name stands for.
pub fn parse_event_kind(name: &str) -> Result<EventKind, ApiError> {
    EVENT_NAMES.iter().find(|(n, _)| *n == name).map(|(_, k)| *k).ok_or_else(|| {
        let known: Vec<&str> = EVENT_NAMES.iter().map(|(n, _)| *n).collect();
        ApiError::invalid(format!("{name:?} is not an event; use one of {}", known.join(", ")))
    })
}

/// The JS name of an event kind.
pub fn event_name(kind: EventKind) -> &'static str {
    EVENT_NAMES.iter().find(|(_, k)| *k == kind).map_or("unknown", |(n, _)| n)
}

fn pointer_name(k: PointerKind) -> &'static str {
    match k {
        PointerKind::Mouse => "mouse",
        PointerKind::Pen => "pen",
        PointerKind::Touch => "touch",
    }
}

fn button_name(b: Button) -> &'static str {
    match b {
        Button::None => "none",
        Button::Primary => "primary",
        Button::Secondary => "secondary",
        Button::Middle => "middle",
        Button::Other => "other",
    }
}

/// The node a listener runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NodeDto {
    /// An element.
    Element {
        /// Its id.
        id: String,
    },
    /// A group.
    Group {
        /// Its id.
        id: String,
    },
    /// The stage.
    Stage,
}

/// The event a listener receives.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDto {
    /// `click`, `dragmove`, ...
    pub r#type: &'static str,
    /// The element the event is about; `null` for empty space and handles.
    pub target: Option<String>,
    /// The node whose listener runs now.
    pub current_target: NodeDto,
    /// Pointer position in logical pixels on the canvas.
    pub screen: Xy,
    /// Pointer position in world units.
    pub world: Xy,
    /// Drag movement since the press, in logical pixels.
    pub delta_screen: Xy,
    /// Drag movement since the press, in world units.
    pub delta_world: Xy,
    /// Keyboard modifiers.
    pub modifiers: Modifiers,
    /// `mouse`, `pen` or `touch`.
    pub pointer: &'static str,
    /// `primary`, `secondary`, `middle`, `other` or `none`.
    pub button: &'static str,
    /// The transformer handle of a `transform*` event.
    pub handle: Option<Handle>,
    /// A drag that was aborted (Escape, a second finger).
    pub cancelled: bool,
}

impl From<&ElementEvent> for EventDto {
    fn from(e: &ElementEvent) -> Self {
        Self {
            r#type: event_name(e.kind),
            target: e.target.as_ref().map(|id| id.as_str().to_owned()),
            current_target: match &e.current {
                Node::Element(id) => NodeDto::Element { id: id.as_str().to_owned() },
                Node::Group(id) => NodeDto::Group { id: id.as_str().to_owned() },
                Node::Stage => NodeDto::Stage,
            },
            screen: xy(e.pos.screen),
            world: xy(e.pos.world),
            delta_screen: vec_xy(e.delta_screen),
            delta_world: vec_xy(e.delta_world),
            modifiers: e.modifiers,
            pointer: pointer_name(e.pointer),
            button: button_name(e.button),
            handle: e.handle.clone(),
            cancelled: e.cancelled,
        }
    }
}

/// A hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitDto {
    /// The element.
    pub id: String,
    /// `stroke`, `fill` or `body`.
    pub part: &'static str,
}

impl From<&Hit> for HitDto {
    fn from(h: &Hit) -> Self {
        let part = match h.part {
            HitPart::Stroke => "stroke",
            HitPart::Fill => "fill",
            HitPart::Body => "body",
        };
        Self { id: h.id.as_str().to_owned(), part }
    }
}

/// The camera.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraDto {
    /// The world point at the centre of the canvas.
    pub scroll: Xy,
    /// Logical pixels per world unit.
    pub zoom: f64,
}

impl From<Camera> for CameraDto {
    fn from(c: Camera) -> Self {
        Self { scroll: xy(c.scroll), zoom: c.zoom }
    }
}

/// A rotated box as its four corners (top-left first, clockwise) in world units.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxDto {
    /// The element this box belongs to, if it is a per-element box.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Corners.
    pub corners: [Xy; 4],
    /// Rotation in radians.
    pub angle: f64,
}

impl BoxDto {
    fn of(id: Option<String>, b: &OrientedBox) -> Self {
        Self { id, corners: b.corners().map(xy), angle: b.angle }
    }
}

/// A visible handle.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandleDto {
    /// Which handle.
    pub handle: Handle,
    /// World position.
    pub at: Xy,
    /// Screen position in logical pixels.
    pub screen: Xy,
}

/// The transformer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "frame", rename_all = "camelCase")]
pub enum TransformerDto {
    /// A box around the selection.
    #[serde(rename_all = "camelCase")]
    Box {
        /// The box.
        r#box: BoxDto,
        /// More than one element is selected.
        multi: bool,
        /// The visible handles.
        handles: Vec<HandleDto>,
        /// The handle being dragged.
        active: Option<Handle>,
        /// Drawn handle size in logical pixels.
        handle_size_px: f64,
    },
    /// The points of one line or arrow.
    #[serde(rename_all = "camelCase")]
    Linear {
        /// The element.
        element: String,
        /// The vertices in world units.
        points: Vec<Xy>,
        /// The last point joins the first.
        closed: bool,
        /// The visible handles.
        handles: Vec<HandleDto>,
        /// The handle being dragged.
        active: Option<Handle>,
        /// Drawn handle size in logical pixels.
        handle_size_px: f64,
    },
}

/// An outline as SVG path data in world units.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineDto {
    /// The element.
    pub id: String,
    /// SVG path data.
    pub path: String,
}

/// A remote user's pointer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorDto {
    /// The user.
    pub user: String,
    /// Their name.
    pub name: Option<String>,
    /// World position.
    pub at: Xy,
}

/// Everything drawn above the scene, for hosts that draw it themselves.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayDto {
    /// Zoom the overlay was built for.
    pub zoom: f64,
    /// A box per selected element (not for large selections).
    pub selection: Vec<BoxDto>,
    /// The frame and handles of the selection.
    pub transformer: Option<TransformerDto>,
    /// The outline of the hovered element.
    pub hover: Option<OutlineDto>,
    /// The rubber band in world units: `[x, y, w, h]`.
    pub marquee: Option<[f64; 4]>,
    /// Elements highlighted as targets.
    pub highlights: Vec<OutlineDto>,
    /// Other users' pointers.
    pub cursors: Vec<CursorDto>,
}

/// SVG path data of a primitive outline.
pub(crate) fn primitive_path(p: &Primitive) -> BezPath {
    use drafftink_geom::kurbo::Shape as _;
    let polygon = |pts: &[Point], closed: bool| {
        let mut path = BezPath::new();
        for (i, q) in pts.iter().enumerate() {
            if i == 0 {
                path.move_to(*q);
            } else {
                path.line_to(*q);
            }
        }
        if closed {
            path.close_path();
        }
        path
    };
    match p {
        Primitive::Rect(r) => polygon(&r.frame.corners(), true),
        Primitive::Diamond(d) => polygon(&d.vertices(), true),
        Primitive::Ellipse(e) => {
            let b = &e.frame;
            let radii = Vec2::new(b.size.width / 2.0 + e.offset, b.size.height / 2.0 + e.offset);
            drafftink_geom::kurbo::Ellipse::new(b.center, radii, b.angle).to_path(0.1)
        }
        Primitive::Polyline(l) => polygon(&l.points, l.closed),
        Primitive::Path(path) => path.path.clone(),
    }
}

impl OverlayDto {
    /// Describes `overlay` for a host. `vp` converts handle positions to screen pixels.
    pub fn new(overlay: &Overlay, vp: &Viewport) -> Self {
        let handles = |t: &drafftink_canvas::TransformerOverlay| -> Vec<HandleDto> {
            t.handles
                .iter()
                .map(|h| HandleDto {
                    handle: h.handle.clone(),
                    at: xy(h.world),
                    screen: xy(vp.world_to_screen(h.world)),
                })
                .collect()
        };
        let transformer = overlay.transformer.as_ref().and_then(|t| match &t.frame {
            SelectionFrame::None => None,
            SelectionFrame::Box { obox, multi, .. } => Some(TransformerDto::Box {
                r#box: BoxDto::of(None, obox),
                multi: *multi,
                handles: handles(t),
                active: t.active.clone(),
                handle_size_px: t.handle_size_px,
            }),
            SelectionFrame::Linear { element, points, closed } => Some(TransformerDto::Linear {
                element: element.as_str().to_owned(),
                points: points.iter().copied().map(xy).collect(),
                closed: *closed,
                handles: handles(t),
                active: t.active.clone(),
                handle_size_px: t.handle_size_px,
            }),
        });
        let outline = |id: &drafftink_scene::ElementId, shape: &Primitive| OutlineDto {
            id: id.as_str().to_owned(),
            path: primitive_path(shape).to_svg(),
        };
        Self {
            zoom: overlay.zoom,
            selection: overlay.selection.iter().map(|b| BoxDto::of(Some(b.id.as_str().to_owned()), &b.obox)).collect(),
            transformer,
            hover: overlay.hover.as_ref().map(|h| outline(&h.id, &h.shape)),
            marquee: overlay.marquee.map(|m| rect_xywh(m.rect)),
            highlights: overlay.bind_candidates.iter().map(|c| outline(&c.target, &c.shape)).collect(),
            cursors: overlay
                .remote_cursors
                .iter()
                .map(|c| CursorDto { user: c.user.clone(), name: c.name.clone(), at: xy(c.pos) })
                .collect(),
        }
    }
}

/// `[x, y, w, h]` of a rectangle.
pub(crate) fn rect_xywh(r: Rect) -> [f64; 4] {
    [r.x0, r.y0, r.width(), r.height()]
}
