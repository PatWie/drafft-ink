//! Geometry of the arrows the layout code creates: which sides they leave and enter, where they bind, and the
//! points they run through. Shared by `diagram_create`, `layout_apply` and the flowchart helpers.
//!
//! The binding module re-solves arrows later (`binding::update_bound_arrows`); this code only has to give each
//! new arrow a sensible first shape and valid bindings, so a document is correct even before that runs.

use drafftink_geom::OrientedBox;
use drafftink_geom::aabb::segment_intersects_rect;
use drafftink_geom::binding::{BindKind, BindTarget, binding_gap};
use drafftink_geom::kurbo::{Point, Rect, Vec2};

use drafftink_scene::{BindMode, Binding, Element, ElementId, ElementKind, Head, Kind, LinearData, PathType, ZIndex};

use super::Direction;

/// How far a self-loop leaves the shape.
const LOOP_STUB: f64 = 28.0;
/// Distance between parallel edges that run through the same layers.
const LANE_SPREAD: f64 = 60.0;

/// A shape an arrow can attach to, as geometry.
#[derive(Debug, Clone)]
pub struct NodeBox {
    /// The element.
    pub id: ElementId,
    /// Outline for binding.
    pub target: BindTarget,
    /// Stroke width, for the binding gap.
    pub stroke_width: f64,
}

impl NodeBox {
    /// The geometry of a bindable element. `None` for a kind arrows cannot attach to.
    pub fn of(e: &Element) -> Option<Self> {
        let kind = match e.tag() {
            Kind::Ellipse => BindKind::Ellipse,
            Kind::Diamond => BindKind::Diamond,
            k if k.is_bindable() => BindKind::Rect,
            _ => return None,
        };
        let t = &e.transform;
        let frame = OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle);
        let target = BindTarget::new(kind, frame).with_corner_radius(e.style.corner_radius);
        Some(Self { id: e.id.clone(), target, stroke_width: e.style.stroke_width })
    }

    /// The unrotated box.
    pub fn rect(&self) -> Rect {
        self.target.frame.unrotated_rect()
    }

    fn gap(&self) -> f64 {
        binding_gap(self.stroke_width)
    }
}

/// A side of a shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Top.
    Top,
    /// Right.
    Right,
    /// Bottom.
    Bottom,
    /// Left.
    Left,
}

impl Side {
    fn vertical(self) -> bool {
        matches!(self, Side::Top | Side::Bottom)
    }

    fn direction_vec(self) -> Vec2 {
        match self {
            Side::Top => Vec2::new(0.0, -1.0),
            Side::Bottom => Vec2::new(0.0, 1.0),
            Side::Left => Vec2::new(-1.0, 0.0),
            Side::Right => Vec2::new(1.0, 0.0),
        }
    }

    /// The fixed point ratio of a point on this side, `along` is the position along the side (0.5 is the middle).
    fn ratio(self, along: f64) -> [f64; 2] {
        match self {
            Side::Top => [along, 0.0],
            Side::Bottom => [along, 1.0],
            Side::Left => [0.0, along],
            Side::Right => [1.0, along],
        }
    }
}

/// The side a straight connection from `from` to `to` leaves, and the side it enters. With a `flow` hint the
/// flow axis wins whenever the two boxes are apart along it.
pub fn pick_sides(from: Rect, to: Rect, flow: Option<Direction>) -> (Side, Side) {
    let gap_x = (from.x0.max(to.x0) - from.x1.min(to.x1)).max(0.0);
    let gap_y = (from.y0.max(to.y0) - from.y1.min(to.y1)).max(0.0);
    let (cf, ct) = (from.center(), to.center());
    let vertical = match flow {
        Some(Direction::Down | Direction::Up) if gap_y > 0.0 => true,
        Some(Direction::Left | Direction::Right) if gap_x > 0.0 => false,
        _ => {
            if gap_y != gap_x {
                gap_y > gap_x
            } else {
                (ct.y - cf.y).abs() >= (ct.x - cf.x).abs()
            }
        }
    };
    if vertical {
        if ct.y >= cf.y { (Side::Bottom, Side::Top) } else { (Side::Top, Side::Bottom) }
    } else if ct.x >= cf.x {
        (Side::Right, Side::Left)
    } else {
        (Side::Left, Side::Right)
    }
}

/// How to route one edge.
#[derive(Debug, Clone)]
pub struct RouteParams<'a> {
    /// Straight, curved or elbow.
    pub path: PathType,
    /// Points the layout wants the edge to pass through, from the start to the end.
    pub waypoints: &'a [Point],
    /// The flow direction of the layout, if there is one.
    pub flow: Option<Direction>,
    /// `(index, count)` among the edges between the same two shapes. Several edges leave from different places
    /// along the same side.
    pub lane: (usize, usize),
    /// Boxes of the other shapes. A straight edge with waypoints uses them only when the direct line would cross
    /// one of these.
    pub obstacles: &'a [Rect],
}

/// The result: world points and both bindings.
#[derive(Debug, Clone)]
pub struct Route {
    /// World points, first at the start and last at the end.
    pub points: Vec<Point>,
    /// Binding of the first point.
    pub start: Binding,
    /// Binding of the last point.
    pub end: Binding,
}

fn lane_offset(lane: (usize, usize)) -> f64 {
    let (i, n) = lane;
    if n <= 1 {
        return 0.0;
    }
    let step = (0.5 / n as f64).min(0.2);
    (i as f64 - (n as f64 - 1.0) / 2.0) * step
}

fn anchor(b: &NodeBox, side: Side, along: f64) -> (Point, [f64; 2]) {
    let ratio = side.ratio(along);
    let focus = b.target.global_fixed_point(Vec2::new(ratio[0], ratio[1]));
    let fallback = focus + side.direction_vec() * b.gap();
    (b.target.nearest_on_offset_outline(focus, b.gap()).unwrap_or(fallback), ratio)
}

fn binding(b: &NodeBox, ratio: [f64; 2]) -> Binding {
    Binding { target: b.id.clone(), mode: BindMode::Orbit, fixed_point: ratio }
}

/// Drops repeated points and the middle point of three collinear axis-aligned points.
fn tidy(points: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if out.last().is_some_and(|q| (*q - p).hypot() < 1e-9) {
            continue;
        }
        while out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let same_x = (a.x - b.x).abs() < 1e-9 && (b.x - p.x).abs() < 1e-9;
            let same_y = (a.y - b.y).abs() < 1e-9 && (b.y - p.y).abs() < 1e-9;
            if same_x || same_y {
                out.pop();
            } else {
                break;
            }
        }
        out.push(p);
    }
    out
}

/// Orthogonal route through `through` (start, waypoints, end), moving along the axis of the first side.
fn orthogonal(through: &[Point], vertical: bool) -> Vec<Point> {
    let mut out = Vec::new();
    for (i, p) in through.iter().enumerate() {
        if i > 0 {
            let q = through[i - 1];
            if vertical {
                let ym = (q.y + p.y) / 2.0;
                out.push(Point::new(q.x, ym));
                out.push(Point::new(p.x, ym));
            } else {
                let xm = (q.x + p.x) / 2.0;
                out.push(Point::new(xm, q.y));
                out.push(Point::new(xm, p.y));
            }
        }
        out.push(*p);
    }
    tidy(out)
}

/// Routes an edge between two different shapes.
pub fn route_edge(from: &NodeBox, to: &NodeBox, p: &RouteParams<'_>) -> Route {
    let (sa, sb) = pick_sides(from.rect(), to.rect(), p.flow);
    let along = 0.5 + lane_offset(p.lane);
    let (s, rs) = anchor(from, sa, along);
    let (e, re) = anchor(to, sb, along);
    let mut through = Vec::with_capacity(p.waypoints.len() + 2);
    through.push(s);
    let direct_blocked = || p.obstacles.iter().any(|r| segment_intersects_rect(s, e, r));
    if p.path == PathType::Elbow || (!p.waypoints.is_empty() && direct_blocked()) {
        // Edges that share a lane group run side by side through the layers they skip.
        let cross = match p.flow {
            Some(Direction::Down | Direction::Up) => Vec2::new(1.0, 0.0),
            Some(Direction::Left | Direction::Right) => Vec2::new(0.0, 1.0),
            None => Vec2::ZERO,
        };
        let shift = cross * (lane_offset(p.lane) * LANE_SPREAD);
        through.extend(p.waypoints.iter().map(|w| *w + shift));
    }
    through.push(e);
    let points = if p.path == PathType::Elbow { orthogonal(&through, sa.vertical()) } else { through };
    Route { points, start: binding(from, rs), end: binding(to, re) }
}

/// Routes an edge from a shape back to itself: out of the right side and back in below.
pub fn route_self_loop(node: &NodeBox) -> Route {
    let (s, rs) = anchor(node, Side::Right, 0.3);
    let (e, re) = anchor(node, Side::Right, 0.7);
    let x = s.x.max(e.x) + LOOP_STUB;
    Route {
        points: vec![s, Point::new(x, s.y), Point::new(x, e.y), e],
        start: binding(node, rs),
        end: binding(node, re),
    }
}

/// How the heads of a new arrow look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heads {
    /// Marker at the start.
    pub start: Head,
    /// Marker at the end.
    pub end: Head,
}

impl Default for Heads {
    fn default() -> Self {
        Self { start: Head::None, end: Head::Arrow }
    }
}

/// A new arrow element along `route`. `None` when the route has non-finite numbers.
pub fn arrow_element(id: ElementId, index: ZIndex, route: &Route, path: PathType, heads: Heads) -> Option<Element> {
    let mut e = Element::linear(id, Kind::Arrow, index, &route.points)?;
    if let ElementKind::Arrow(d) = &mut e.kind {
        d.path = path;
        d.start_binding = Some(route.start.clone());
        d.end_binding = Some(route.end.clone());
        d.start_head = heads.start;
        d.end_head = heads.end;
    }
    Some(e)
}

/// Replaces the points and bindings of an existing arrow with `route`, keeping everything else. Pinned elbow
/// segments are dropped because they refer to the old points.
pub fn reroute_element(e: &mut Element, route: &Route) {
    e.transform.angle = 0.0;
    e.transform.flip_x = false;
    e.transform.flip_y = false;
    e.transform.x = 0.0;
    e.transform.y = 0.0;
    if let ElementKind::Arrow(LinearData {
        points,
        start_binding,
        end_binding,
        fixed_segments,
        start_is_special,
        end_is_special,
        ..
    }) = &mut e.kind
    {
        *points = route.points.clone();
        *start_binding = Some(route.start.clone());
        *end_binding = Some(route.end.clone());
        fixed_segments.clear();
        *start_is_special = false;
        *end_is_special = false;
    }
    e.refit_points();
}
