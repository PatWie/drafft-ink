//! Orthogonal routing between two arrow ends: obstacle boxes, the routing grid and A*.
//!
//! This follows the approach of Excalidraw's elbow arrows (MIT), rewritten for our model:
//! every shape the arrow is bound to is padded into a box, the two boxes are made to touch,
//! the ends leave the boxes through "dongle" points, and a bend-penalising A* runs on the grid
//! formed by the box edges. A cheaper one- or two-corner route is tried first.
//!
//! Everything here is pure and works in world coordinates. Inputs must be finite; anything else
//! is refused with [`ElbowError::NotFinite`].

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use drafftink_geom::binding::{
    BASE_BINDING_GAP, BindTarget, ELBOW_BASE_PADDING, Heading, grow_rect, max_binding_distance, offset_from_heading,
};
use drafftink_geom::kurbo::{Point, Rect, Vec2};
use drafftink_geom::{Outline, point_is_finite};
use drafftink_scene::{ElementId, Head};

use crate::linear::head_size;

/// `[left, top, right, bottom]`.
pub type Bounds = [f64; 4];

/// Distance an elbow arrow keeps from the shapes it connects.
pub const BASE_PADDING: f64 = ELBOW_BASE_PADDING;
/// Radius of the rounded corners that are drawn.
pub const CORNER_RADIUS: f64 = 16.0;
/// Segments shorter than this are removed or ignored.
pub const DEDUP_THRESHOLD: f64 = 1.0;

/// Why a route could not be made.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ElbowError {
    /// A coordinate is NaN or infinite, or too large to route.
    #[error("an elbow arrow needs finite coordinates; check the arrow's points and the shapes it is bound to")]
    NotFinite,
    /// The arrow has fewer than two points.
    #[error("an elbow arrow needs at least 2 points, and this one has {0}")]
    TooFewPoints(usize),
    /// A segment index is outside the arrow.
    #[error("segment {index} does not exist; this arrow has segments 1 to {last}")]
    NoSuchSegment {
        /// The index asked for.
        index: usize,
        /// The last valid index.
        last: usize,
    },
    /// The first and last segments belong to the end stubs and cannot be pinned.
    #[error("the first and last segment of an elbow arrow cannot be pinned; move the arrow's end instead")]
    EndSegment,
    /// The segment is not horizontal or vertical.
    #[error("segment {0} is not horizontal or vertical, so it cannot be moved as an elbow segment")]
    NotOrthogonal(usize),
}

/// A shape an elbow end is attached to.
#[derive(Debug, Clone, PartialEq)]
pub struct ElbowTarget {
    /// The element id (two ends on the same shape are routed differently).
    pub id: ElementId,
    /// Its outline.
    pub target: BindTarget,
    /// The binding gap of the shape (`5 + stroke / 2`).
    pub gap: f64,
}

impl ElbowTarget {
    fn aabb(&self, offsets: [f64; 4]) -> Bounds {
        bounds_of(grow_rect(self.target.frame.aabb(), offsets))
    }
}

/// One end of an elbow arrow.
#[derive(Debug, Clone, PartialEq)]
pub struct ElbowEnd {
    /// Where the arrow end is, in world space. For a bound end this is on the shape's outline plus gap.
    pub point: Point,
    /// Where the pointer was before the end was snapped. It decides whether the end counts as close to its shape.
    /// Use `point` when there is no pointer.
    pub orig: Point,
    /// The shape the end is bound to.
    pub bound: Option<ElbowTarget>,
    /// The marker at this end. A marker widens the padding.
    pub head: Head,
}

impl ElbowEnd {
    /// A free end at `point`.
    pub fn free(point: Point, head: Head) -> Self {
        Self { point, orig: point, bound: None, head }
    }

    /// An end bound to `target`, at `point`.
    pub fn bound(point: Point, target: ElbowTarget, head: Head) -> Self {
        Self { point, orig: point, bound: Some(target), head }
    }
}

/// Options of the router.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteOpts {
    /// Zoom, which sets how far from a shape an end still counts as attached.
    pub zoom: f64,
}

impl Default for RouteOpts {
    fn default() -> Self {
        Self { zoom: 1.0 }
    }
}

pub(crate) fn bounds_of(r: Rect) -> Bounds {
    [r.x0, r.y0, r.x1, r.y1]
}

fn point_bounds(p: Point) -> Bounds {
    [p.x - 2.0, p.y - 2.0, p.x + 2.0, p.y + 2.0]
}

fn inside(p: Point, b: &Bounds) -> bool {
    p.x > b[0] && p.x < b[2] && p.y > b[1] && p.y < b[3]
}

fn intersects(a: &Bounds, b: &Bounds) -> bool {
    a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}

fn common_bounds(list: &[Bounds]) -> Bounds {
    let mut out = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for b in list {
        out[0] = out[0].min(b[0]);
        out[1] = out[1].min(b[1]);
        out[2] = out[2].max(b[2]);
        out[3] = out[3].max(b[3]);
    }
    out
}

fn cross(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// True when every segment is horizontal or vertical within `tolerance`.
pub fn is_orthogonal(points: &[Point], tolerance: f64) -> bool {
    points.windows(2).all(|w| (w[1].x - w[0].x).abs() < tolerance || (w[1].y - w[0].y).abs() < tolerance)
}

fn finite(p: Point) -> bool {
    point_is_finite(p) && p.x.abs() <= 1.0e9 && p.y.abs() <= 1.0e9
}

pub(crate) fn check_end(e: &ElbowEnd) -> Result<(), ElbowError> {
    if !finite(e.point) || !finite(e.orig) {
        return Err(ElbowError::NotFinite);
    }
    if let Some(b) = &e.bound
        && (!b.target.is_valid() || !b.gap.is_finite())
    {
        return Err(ElbowError::NotFinite);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Headings
// ---------------------------------------------------------------------------------------------

/// The direction an end leaves in. A bound end leaves the shape on the side it sits at; a free end
/// points towards the other end.
pub(crate) fn end_heading(
    p: Point,
    other: Point,
    bound: Option<&ElbowTarget>,
    orig: Point,
    opts: RouteOpts,
) -> Heading {
    let toward_other = Heading::from_points(other, p);
    let Some(b) = bound else { return toward_other };
    let shape = b.target.shape(0.0);
    let d = shape.outline_distance(p);
    let aabb = if d.is_finite() { grow_rect(b.target.frame.aabb(), [d; 4]) } else { b.target.frame.aabb() };
    let reach = shape.outline_distance(orig);
    // Far from the shape, or exactly on it: use the side of the centre instead of the cones.
    if !reach.is_finite() || reach > max_binding_distance(opts.zoom) || reach == 0.0 {
        return Heading::from_vector(p - b.target.center());
    }
    b.target.heading_for_point(p, aabb)
}

// ---------------------------------------------------------------------------------------------
// Obstacle boxes
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Data {
    dynamic: [Bounds; 2],
    start_dongle: Point,
    end_dongle: Point,
    start: Point,
    end: Point,
    start_heading: Heading,
    end_heading: Heading,
    common: Bounds,
}

fn head_pad(h: Head) -> f64 {
    if h == Head::None { 2.0 } else { 6.0 }
}

fn data(s: &ElbowEnd, e: &ElbowEnd, opts: RouteOpts) -> Data {
    let (sp, ep) = (s.point, e.point);
    let start_heading = end_heading(sp, ep, s.bound.as_ref(), s.orig, opts);
    let end_heading_ = end_heading(ep, sp, e.bound.as_ref(), e.orig, opts);

    let (spb, epb) = (point_bounds(sp), point_bounds(ep));
    let start_el = match &s.bound {
        Some(b) => b.aabb(offset_from_heading(start_heading, b.gap * head_pad(s.head), 1.0)),
        None => spb,
    };
    let end_el = match &e.bound {
        Some(b) => b.aabb(offset_from_heading(end_heading_, b.gap * head_pad(e.head), 1.0)),
        None => epb,
    };
    let start_pad_box = match &s.bound {
        Some(b) => b.aabb(offset_from_heading(start_heading, BASE_PADDING, BASE_PADDING)),
        None => spb,
    };
    let end_pad_box = match &e.bound {
        Some(b) => b.aabb(offset_from_heading(end_heading_, BASE_PADDING, BASE_PADDING)),
        None => epb,
    };
    let overlap = inside(sp, &end_pad_box) || inside(ep, &start_pad_box);
    let common = if overlap { common_bounds(&[spb, epb]) } else { common_bounds(&[start_el, end_el]) };
    let none_bound = s.bound.is_none() && e.bound.is_none();

    let diff = |heading: Heading, head: Head| -> [f64; 4] {
        if overlap {
            offset_from_heading(heading, if none_bound { 0.0 } else { BASE_PADDING }, 0.0)
        } else {
            let head_part = if none_bound { 0.0 } else { BASE_PADDING - BASE_BINDING_GAP * head_pad(head) };
            offset_from_heading(heading, head_part, BASE_PADDING)
        }
    };
    let dynamic = dynamic_boxes(
        if overlap { spb } else { start_el },
        if overlap { epb } else { end_el },
        common,
        diff(start_heading, s.head),
        diff(end_heading_, e.head),
        overlap,
        s.bound.as_ref().map(|b| b.aabb([0.0; 4])),
        e.bound.as_ref().map(|b| b.aabb([0.0; 4])),
    );
    Data {
        start_dongle: dongle(&dynamic[0], start_heading, sp),
        end_dongle: dongle(&dynamic[1], end_heading_, ep),
        dynamic,
        start: sp,
        end: ep,
        start_heading,
        end_heading: end_heading_,
        common,
    }
}

fn dongle(b: &Bounds, h: Heading, p: Point) -> Point {
    match h {
        Heading::Up => Point::new(p.x, b[1]),
        Heading::Right => Point::new(b[2], p.y),
        Heading::Down => Point::new(p.x, b[3]),
        Heading::Left => Point::new(b[0], p.y),
    }
}

/// Two boxes that always touch: each grows towards the other until they meet, or towards the common
/// bounds where they overlap.
#[allow(clippy::too_many_arguments)]
fn dynamic_boxes(
    a: Bounds,
    b: Bounds,
    common: Bounds,
    start_diff: [f64; 4],
    end_diff: [f64; 4],
    disable_side_hack: bool,
    start_el: Option<Bounds>,
    end_el: Option<Bounds>,
) -> [Bounds; 2] {
    let s_el = start_el.unwrap_or(a);
    let e_el = end_el.unwrap_or(b);
    let [s_up, s_right, s_down, s_left] = start_diff;
    let [e_up, e_right, e_down, e_left] = end_diff;

    let first: Bounds = [
        if a[0] > b[2] {
            if a[1] > b[3] || a[3] < b[1] {
                ((s_el[0] + e_el[2]) / 2.0).min(a[0] - s_left)
            } else {
                (s_el[0] + e_el[2]) / 2.0
            }
        } else if a[0] > b[0] {
            a[0] - s_left
        } else {
            common[0] - s_left
        },
        if a[1] > b[3] {
            if a[0] > b[2] || a[2] < b[0] {
                ((s_el[1] + e_el[3]) / 2.0).min(a[1] - s_up)
            } else {
                (s_el[1] + e_el[3]) / 2.0
            }
        } else if a[1] > b[1] {
            a[1] - s_up
        } else {
            common[1] - s_up
        },
        if a[2] < b[0] {
            if a[1] > b[3] || a[3] < b[1] {
                ((s_el[2] + e_el[0]) / 2.0).max(a[2] + s_right)
            } else {
                (s_el[2] + e_el[0]) / 2.0
            }
        } else if a[2] < b[2] {
            a[2] + s_right
        } else {
            common[2] + s_right
        },
        if a[3] < b[1] {
            if a[0] > b[2] || a[2] < b[0] {
                ((s_el[3] + e_el[1]) / 2.0).max(a[3] + s_down)
            } else {
                (s_el[3] + e_el[1]) / 2.0
            }
        } else if a[3] < b[3] {
            a[3] + s_down
        } else {
            common[3] + s_down
        },
    ];
    let second: Bounds = [
        if b[0] > a[2] {
            if b[1] > a[3] || b[3] < a[1] {
                ((e_el[0] + s_el[2]) / 2.0).min(b[0] - e_left)
            } else {
                (e_el[0] + s_el[2]) / 2.0
            }
        } else if b[0] > a[0] {
            b[0] - e_left
        } else {
            common[0] - e_left
        },
        if b[1] > a[3] {
            if b[0] > a[2] || b[2] < a[0] {
                ((e_el[1] + s_el[3]) / 2.0).min(b[1] - e_up)
            } else {
                (e_el[1] + s_el[3]) / 2.0
            }
        } else if b[1] > a[1] {
            b[1] - e_up
        } else {
            common[1] - e_up
        },
        if b[2] < a[0] {
            if b[1] > a[3] || b[3] < a[1] {
                ((e_el[2] + s_el[0]) / 2.0).max(b[2] + e_right)
            } else {
                (e_el[2] + s_el[0]) / 2.0
            }
        } else if b[2] < a[2] {
            b[2] + e_right
        } else {
            common[2] + e_right
        },
        if b[3] < a[1] {
            if b[0] > a[2] || b[2] < a[0] {
                ((e_el[3] + s_el[1]) / 2.0).max(b[3] + e_down)
            } else {
                (e_el[3] + s_el[1]) / 2.0
            }
        } else if b[3] < a[3] {
            b[3] + e_down
        } else {
            common[3] + e_down
        },
    ];

    let c = common_bounds(&[first, second]);
    let overlap_both = (first[2] - first[0]) + (second[2] - second[0]) > (c[2] - c[0]) + 1e-11
        && (first[3] - first[1]) + (second[3] - second[1]) > (c[3] - c[1]) + 1e-11;
    if !disable_side_hack && overlap_both {
        let end_center = Vec2::new((second[0] + second[2]) / 2.0, (second[1] + second[3]) / 2.0);
        let v = |x: f64, y: f64| Vec2::new(x - end_center.x, y - end_center.y);
        if b[0] > a[2] && a[1] > b[3] {
            // The end box is to the upper right of the start box.
            let cx = first[2] + (second[0] - first[2]) / 2.0;
            let cy = second[3] + (first[1] - second[3]) / 2.0;
            if cross(v(a[2], a[1]), v(a[0], a[3])) > 0.0 {
                return [[first[0], first[1], cx, first[3]], [cx, second[1], second[2], second[3]]];
            }
            return [[first[0], cy, first[2], first[3]], [second[0], second[1], second[2], cy]];
        } else if a[2] < b[0] && a[3] < b[1] {
            let cx = first[2] + (second[0] - first[2]) / 2.0;
            let cy = first[3] + (second[1] - first[3]) / 2.0;
            if cross(v(a[0], a[1]), v(a[2], a[3])) > 0.0 {
                return [[first[0], first[1], first[2], cy], [second[0], cy, second[2], second[3]]];
            }
            return [[first[0], first[1], cx, first[3]], [cx, second[1], second[2], second[3]]];
        } else if a[0] > b[2] && a[3] < b[1] {
            let cx = second[2] + (first[0] - second[2]) / 2.0;
            let cy = first[3] + (second[1] - first[3]) / 2.0;
            if cross(v(a[2], a[1]), v(a[0], a[3])) > 0.0 {
                return [[cx, first[1], first[2], first[3]], [second[0], second[1], cx, second[3]]];
            }
            return [[first[0], first[1], first[2], cy], [second[0], cy, second[2], second[3]]];
        } else if a[0] > b[2] && a[1] > b[3] {
            let cx = second[2] + (first[0] - second[2]) / 2.0;
            let cy = second[3] + (first[1] - second[3]) / 2.0;
            if cross(v(a[0], a[1]), v(a[2], a[3])) > 0.0 {
                return [[cx, first[1], first[2], first[3]], [second[0], second[1], cx, second[3]]];
            }
            return [[first[0], cy, first[2], first[3]], [second[0], second[1], second[2], cy]];
        }
    }
    [first, second]
}

// ---------------------------------------------------------------------------------------------
// Grid and A*
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Node {
    pos: Point,
    g: f64,
    f: f64,
    closed: bool,
    visited: bool,
    parent: Option<usize>,
}

struct Grid {
    cols: usize,
    rows: usize,
    xs: Vec<f64>,
    ys: Vec<f64>,
    nodes: Vec<Node>,
}

fn sorted_unique(mut v: Vec<f64>) -> Vec<f64> {
    for x in &mut v {
        *x += 0.0; // turns -0.0 into 0.0
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    v.dedup();
    v
}

fn make_grid(aabbs: &[Bounds], start: Point, sh: Heading, end: Point, eh: Heading, common: &Bounds) -> Grid {
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    if sh.is_horizontal() {
        ys.push(start.y)
    } else {
        xs.push(start.x)
    }
    if eh.is_horizontal() {
        ys.push(end.y)
    } else {
        xs.push(end.x)
    }
    for b in aabbs {
        xs.extend([b[0], b[2]]);
        ys.extend([b[1], b[3]]);
    }
    xs.extend([common[0], common[2]]);
    ys.extend([common[1], common[3]]);
    let (xs, ys) = (sorted_unique(xs), sorted_unique(ys));
    let mut nodes = Vec::with_capacity(xs.len() * ys.len());
    for &y in &ys {
        for &x in &xs {
            nodes.push(Node { pos: Point::new(x, y), g: 0.0, f: 0.0, closed: false, visited: false, parent: None });
        }
    }
    Grid { cols: xs.len(), rows: ys.len(), xs, ys, nodes }
}

fn find_exact(list: &[f64], v: f64) -> Option<usize> {
    list.binary_search_by(|probe| probe.partial_cmp(&(v + 0.0)).unwrap_or(Ordering::Equal)).ok()
}

impl Grid {
    fn node_at(&self, p: Point) -> Option<usize> {
        let col = find_exact(&self.xs, p.x)?;
        let row = find_exact(&self.ys, p.y)?;
        Some(row * self.cols + col)
    }

    /// Up, right, down, left.
    fn neighbors(&self, i: usize) -> [Option<usize>; 4] {
        let (col, row) = (i % self.cols, i / self.cols);
        [
            (row > 0).then(|| (row - 1) * self.cols + col),
            (col + 1 < self.cols).then(|| row * self.cols + col + 1),
            (row + 1 < self.rows).then(|| (row + 1) * self.cols + col),
            (col > 0).then(|| row * self.cols + col - 1),
        ]
    }
}

fn manhattan(a: Point, b: Point) -> f64 {
    (a.x - b.x).abs() + (a.y - b.y).abs()
}

/// How many more bends the route is expected to need from `start` heading `sh` to reach `end`
/// entering with heading `eh`. A heuristic that keeps A* away from needless detours.
fn estimate_bends(start: Point, end: Point, sh: Heading, eh: Heading) -> f64 {
    use Heading::{Down, Left, Right, Up};
    let same_y = start.y == end.y;
    let same_x = start.x == end.x;
    let n = match (eh, sh) {
        (Right, Right) => {
            if start.x >= end.x {
                4
            } else if same_y {
                0
            } else {
                2
            }
        }
        (Right, Up) => {
            if start.y > end.y && start.x < end.x {
                1
            } else {
                3
            }
        }
        (Right, Down) => {
            if start.y < end.y && start.x < end.x {
                1
            } else {
                3
            }
        }
        (Right, Left) => {
            if same_y {
                4
            } else {
                2
            }
        }
        (Left, Right) => {
            if same_y {
                4
            } else {
                2
            }
        }
        (Left, Up) => {
            if start.y > end.y && start.x > end.x {
                1
            } else {
                3
            }
        }
        (Left, Down) => {
            if start.y < end.y && start.x > end.x {
                1
            } else {
                3
            }
        }
        (Left, Left) => {
            if start.x <= end.x {
                4
            } else if same_y {
                0
            } else {
                2
            }
        }
        (Up, Right) => {
            if start.y > end.y && start.x < end.x {
                1
            } else {
                3
            }
        }
        (Up, Up) => {
            if start.y >= end.y {
                4
            } else if same_x {
                0
            } else {
                2
            }
        }
        (Up, Down) => {
            if same_x {
                4
            } else {
                2
            }
        }
        (Up, Left) => {
            if start.y > end.y && start.x > end.x {
                1
            } else {
                3
            }
        }
        (Down, Right) => {
            if start.y < end.y && start.x < end.x {
                1
            } else {
                3
            }
        }
        (Down, Up) => {
            if same_x {
                4
            } else {
                2
            }
        }
        (Down, Down) => {
            if start.y <= end.y {
                4
            } else if same_x {
                0
            } else {
                2
            }
        }
        (Down, Left) => {
            if start.y < end.y && start.x > end.x {
                1
            } else {
                3
            }
        }
    };
    f64::from(n)
}

#[derive(PartialEq)]
struct Entry {
    f: f64,
    seq: u64,
    node: usize,
}

impl Eq for Entry {}

impl Ord for Entry {
    // BinaryHeap is a max-heap, so the order is reversed: smallest f first, then oldest first.
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.total_cmp(&self.f).then_with(|| other.seq.cmp(&self.seq))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A* over the grid. Bends cost the cube of the start-to-end distance, so fewer bends always win,
/// and ties go to the shorter route. The route may not turn back on itself, nor leave the start or
/// enter the end against its heading, and may not cross the middle of an obstacle box.
fn astar(
    grid: &mut Grid,
    start: usize,
    end: usize,
    sh: Heading,
    eh: Heading,
    obstacles: &[Bounds],
) -> Option<Vec<usize>> {
    let end_pos = grid.nodes.get(end)?.pos;
    let bend = manhattan(grid.nodes.get(start)?.pos, end_pos);
    let (bend2, bend3) = (bend * bend, bend * bend * bend);
    let mut open = BinaryHeap::new();
    let mut seq = 0u64;
    open.push(Entry { f: 0.0, seq, node: start });
    while let Some(Entry { node: current, .. }) = open.pop() {
        if grid.nodes.get(current)?.closed {
            continue;
        }
        if current == end {
            let mut path = vec![current];
            let mut at = current;
            while let Some(p) = grid.nodes.get(at)?.parent {
                path.push(p);
                at = p;
            }
            path.reverse();
            return Some(path);
        }
        grid.nodes.get_mut(current)?.closed = true;
        let (cur_pos, cur_g, cur_parent) = {
            let n = grid.nodes.get(current)?;
            (n.pos, n.g, n.parent)
        };
        let prev_dir = match cur_parent {
            Some(p) => Heading::from_vector(cur_pos - grid.nodes.get(p)?.pos),
            None => sh,
        };
        for (i, heading) in [Heading::Up, Heading::Right, Heading::Down, Heading::Left].into_iter().enumerate() {
            let Some(nb) = grid.neighbors(current).get(i).copied().flatten() else { continue };
            let (nb_pos, nb_closed, nb_visited, nb_g) = {
                let n = grid.nodes.get(nb)?;
                (n.pos, n.closed, n.visited, n.g)
            };
            if nb_closed {
                continue;
            }
            let mid = Point::new((nb_pos.x + cur_pos.x) / 2.0, (nb_pos.y + cur_pos.y) / 2.0);
            if obstacles.iter().any(|b| inside(mid, b)) {
                continue;
            }
            let reverse = heading == prev_dir.flip() || (nb == start && heading == sh) || (nb == end && heading == eh);
            if reverse {
                continue;
            }
            let g = cur_g + manhattan(nb_pos, cur_pos) + if prev_dir != heading { bend3 } else { 0.0 };
            if !nb_visited || g < nb_g {
                let h = manhattan(end_pos, nb_pos) + estimate_bends(nb_pos, end_pos, heading, eh) * bend2;
                let n = grid.nodes.get_mut(nb)?;
                n.visited = true;
                n.parent = Some(current);
                n.g = g;
                n.f = g + h;
                seq += 1;
                open.push(Entry { f: n.f, seq, node: nb });
            }
        }
    }
    None
}

fn grid_route(d: &Data, end_bound: bool, start_bound: bool) -> Option<Vec<Point>> {
    let mut grid = make_grid(&d.dynamic, d.start_dongle, d.start_heading, d.end_dongle, d.end_heading, &d.common);
    let start_dongle = grid.node_at(d.start_dongle)?;
    let end_dongle = grid.node_at(d.end_dongle)?;
    // The true end points are not to be stepped on.
    if end_bound && let Some(n) = grid.node_at(d.end).and_then(|i| grid.nodes.get_mut(i)) {
        n.closed = true;
    }
    if start_bound && let Some(n) = grid.node_at(d.start).and_then(|i| grid.nodes.get_mut(i)) {
        n.closed = true;
    }
    let sd = grid.nodes.get(start_dongle)?.pos;
    let ed = grid.nodes.get(end_dongle)?.pos;
    let dongle_overlap = inside(sd, &d.dynamic[1]) || inside(ed, &d.dynamic[0]);
    let obstacles: &[Bounds] = if dongle_overlap { &[] } else { &d.dynamic };
    let path = astar(&mut grid, start_dongle, end_dongle, d.start_heading, d.end_heading, obstacles)?;
    let mut points = vec![d.start];
    for i in path {
        points.push(grid.nodes.get(i)?.pos);
    }
    points.push(d.end);
    Some(points)
}

// ---------------------------------------------------------------------------------------------
// The one- and two-corner route
// ---------------------------------------------------------------------------------------------

fn crosses(a: Point, b: Point, bounds: &Bounds) -> bool {
    intersects(&[a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)], bounds)
}

fn dot(v: Vec2, h: Heading) -> f64 {
    let u = h.vector();
    v.x * u.x + v.y * u.y
}

/// A route with one corner (the ends leave at right angles) or two (they face each other), if the
/// legs are long enough to keep their rounded corners and arrowheads, and nothing is crossed.
fn simple_route(s: &ElbowEnd, e: &ElbowEnd, d: &Data) -> Option<Vec<Point>> {
    let one_corner = d.start_heading.is_horizontal() != d.end_heading.is_horizontal();
    let (sb, eb) = (s.bound.as_ref()?, e.bound.as_ref()?);
    if sb.id == eb.id || (!one_corner && d.start_heading != d.end_heading.flip()) {
        return None;
    }
    let ahead = dot(d.end - d.start, d.start_heading);
    let (first_leg, last_leg);
    if one_corner {
        first_leg = ahead;
        last_leg = dot(d.start - d.end, d.end_heading);
    } else {
        let mut first = ahead / 2.0;
        if (s.head == Head::None) != (e.head == Head::None) {
            let head_leg = (ahead / 2.0).max(BASE_PADDING.min(ahead - CORNER_RADIUS * 2.0));
            first = if s.head != Head::None { head_leg } else { ahead - head_leg };
        }
        first_leg = first;
        last_leg = ahead - first;
    }
    let min_leg = |h: Head| (CORNER_RADIUS / 2.0).max(if h == Head::None { 0.0 } else { head_size(h) });
    if first_leg <= min_leg(s.head) || last_leg <= min_leg(e.head) {
        return None;
    }
    let first_turn = d.start + d.start_heading.vector() * first_leg;
    let last_turn = d.end + d.end_heading.vector() * last_leg;
    let pad = |t: &ElbowTarget| t.aabb([t.gap; 4]);
    let (start_box, end_box) = (pad(sb), pad(eb));
    if crosses(d.start, first_turn, &end_box)
        || crosses(first_turn, last_turn, &start_box)
        || crosses(first_turn, last_turn, &end_box)
        || crosses(last_turn, d.end, &start_box)
    {
        return None;
    }
    Some(vec![d.start, first_turn, last_turn, d.end])
}

// ---------------------------------------------------------------------------------------------
// Cleaning up
// ---------------------------------------------------------------------------------------------

fn dedupe_exact(points: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    out
}

/// Drops interior points whose distance to the previous point is at most [`DEDUP_THRESHOLD`]. Only for 4+ points.
/// A point whose removal would leave a diagonal stays, so the route stays strictly orthogonal; the collinear
/// pass thins such points out afterwards.
pub(crate) fn remove_short_segments(points: Vec<Point>) -> Vec<Point> {
    if points.len() < 4 {
        return points;
    }
    let last = points.len() - 1;
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        let short = i > 0 && i < last && points.get(i - 1).is_some_and(|prev| (*p - *prev).hypot() <= DEDUP_THRESHOLD);
        if short {
            // Only drop it when the neighbours are still aligned without it.
            let (prev, next) = (points[i - 1], points[i + 1]);
            let aligned = (prev.x - next.x).abs() < 1e-9 || (prev.y - next.y).abs() < 1e-9;
            if aligned {
                continue;
            }
        }
        out.push(*p);
    }
    out
}

/// Keeps the first and last points and the points where the direction changes.
pub(crate) fn corner_points(points: Vec<Point>) -> Vec<Point> {
    if points.len() < 2 {
        return points;
    }
    let horizontal = |a: Point, b: Point| (a.y - b.y).abs() < (a.x - b.x).abs();
    let mut previous_horizontal = match (points.first(), points.get(1)) {
        (Some(a), Some(b)) => horizontal(*a, *b),
        _ => return points,
    };
    let last = points.len() - 1;
    let mut out = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        if i == 0 || i == last {
            out.push(*p);
            continue;
        }
        let Some(next) = points.get(i + 1) else { continue };
        let next_horizontal = horizontal(*p, *next);
        if previous_horizontal != next_horizontal {
            out.push(*p);
        }
        previous_horizontal = next_horizontal;
    }
    out
}

/// The worst case: a Z through the middle, used when the grid has no route.
fn fallback_route(d: &Data) -> Vec<Point> {
    if d.start_heading.is_horizontal() {
        let mx = (d.start.x + d.end.x) / 2.0;
        vec![d.start, Point::new(mx, d.start.y), Point::new(mx, d.end.y), d.end]
    } else {
        let my = (d.start.y + d.end.y) / 2.0;
        vec![d.start, Point::new(d.start.x, my), Point::new(d.end.x, my), d.end]
    }
}

/// Routes an elbow arrow from `start` to `end`: every segment is horizontal or vertical, the first
/// and last points are exactly the two ends, and consecutive segments always turn.
pub fn route(start: &ElbowEnd, end: &ElbowEnd, opts: RouteOpts) -> Result<Vec<Point>, ElbowError> {
    check_end(start)?;
    check_end(end)?;
    let d = data(start, end, opts);
    if !(finite(d.start_dongle) && finite(d.end_dongle)) {
        return Err(ElbowError::NotFinite);
    }
    let raw = simple_route(start, end, &d)
        .or_else(|| grid_route(&d, end.bound.is_some(), start.bound.is_some()))
        .unwrap_or_else(|| fallback_route(&d));
    let cleaned = corner_points(remove_short_segments(dedupe_exact(raw)));
    Ok(make_orthogonal(cleaned, start.point, end.point))
}

/// Guards the contract: exact ends, and orthogonal segments. A route that is not (a bound end whose
/// dongle sits off the end's own axis) gets a corner point inserted.
fn make_orthogonal(points: Vec<Point>, start: Point, end: Point) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len() + 2);
    for p in points {
        if let Some(prev) = out.last().copied()
            && (p.x - prev.x).abs() >= DEDUP_THRESHOLD
            && (p.y - prev.y).abs() >= DEDUP_THRESHOLD
        {
            out.push(Point::new(p.x, prev.y));
        }
        out.push(p);
    }
    if out.len() < 2 {
        out = vec![start, end];
        if (end.x - start.x).abs() >= DEDUP_THRESHOLD && (end.y - start.y).abs() >= DEDUP_THRESHOLD {
            out.insert(1, Point::new(end.x, start.y));
        }
    }
    if let Some(f) = out.first_mut() {
        *f = start;
    }
    if let Some(l) = out.last_mut() {
        *l = end;
    }
    out
}

/// The heading an end leaves its shape in, as used by the router.
pub fn heading_of_end(this: &ElbowEnd, other: &ElbowEnd, opts: RouteOpts) -> Heading {
    end_heading(this.point, other.point, this.bound.as_ref(), this.orig, opts)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn free_route_is_a_z_through_the_middle() {
        let s = ElbowEnd::free(Point::new(-45.0, -100.1), Head::None);
        let e = ElbowEnd::free(Point::new(45.0, 99.9), Head::None);
        let r = route(&s, &e, RouteOpts::default()).unwrap();
        let rel: Vec<(f64, f64)> = r.iter().map(|p| (p.x - r[0].x, p.y - r[0].y)).collect();
        let want = [(0.0, 0.0), (0.0, 100.0), (90.0, 100.0), (90.0, 200.0)];
        assert_eq!(rel.len(), 4, "{rel:?}");
        for (a, b) in rel.iter().zip(want) {
            assert!((a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6, "{rel:?}");
        }
    }

    #[test]
    fn non_finite_is_refused() {
        let s = ElbowEnd::free(Point::new(f64::NAN, 0.0), Head::None);
        let e = ElbowEnd::free(Point::new(1.0, 1.0), Head::None);
        assert_eq!(route(&s, &e, RouteOpts::default()), Err(ElbowError::NotFinite));
    }

    #[test]
    fn aligned_ends_give_a_straight_line() {
        let s = ElbowEnd::free(Point::new(0.0, 0.0), Head::None);
        let e = ElbowEnd::free(Point::new(100.0, 0.0), Head::None);
        let r = route(&s, &e, RouteOpts::default()).unwrap();
        assert_eq!(r, vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)]);
    }
}
