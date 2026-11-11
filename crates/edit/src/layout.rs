//! Auto-layout for `diagram.create` and `layout.apply`.
//!
//! * [`compute_layout`] is the pure part: sizes and edges in, top-left positions out. Algorithms: `layered`
//!   (Sugiyama style, handles cycles), `tree` (a forest), `grid`, `force`, and `none` (use the given positions).
//!   It is deterministic and never returns overlapping boxes (except for `none`, which keeps what it is given).
//! * [`layout_apply`] applies a layout to elements of a scene: moves the nodes, reroutes the arrows between them
//!   and keeps labels with their shapes.
//! * [`diagram_create`] builds a diagram from a [`DiagramSpec`].
//!
//! Distances are world units.

mod diagram;
mod force;
mod layered;
mod route;
mod tree;

use std::collections::{BTreeMap, BTreeSet};

use drafftink_geom::OrientedBox;
use kurbo::{Point, Rect, Size};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use drafftink_scene::{ElementId, Kind, PathType, SceneRead, SceneTx, StoreError};

use crate::binding::update_bound_arrows;
use crate::text_bind::{GrowAnchor, TextBindError, TextMeasure, refresh_labels};

pub use diagram::{
    Arrowheads, CreateOpts, DiagramError, DiagramResult, DiagramSpec, EdgeSpec, GroupSpec, MAX_EDGES, MAX_NODES,
    NodeShape, NodeSpec, StyleSpec, below_content_origin, diagram_create,
};
pub use route::{
    Heads, NodeBox, Route, RouteParams, Side, arrow_element, pick_sides, reroute_element, route_edge, route_self_loop,
};

/// The direction layers grow in (layered and tree layouts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Top to bottom (Mermaid `TB` or `TD`).
    #[default]
    #[serde(alias = "tb", alias = "td", alias = "TB", alias = "TD")]
    Down,
    /// Left to right (Mermaid `LR`).
    #[serde(alias = "lr", alias = "LR")]
    Right,
    /// Bottom to top (Mermaid `BT`).
    #[serde(alias = "bt", alias = "BT")]
    Up,
    /// Right to left (Mermaid `RL`).
    #[serde(alias = "rl", alias = "RL")]
    Left,
}

/// Which layout to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    /// Layers of nodes with few crossings. Handles cycles. The default.
    #[default]
    #[serde(alias = "flowchart")]
    Layered,
    /// A forest: every node has at most one incoming edge and there is no cycle.
    Tree,
    /// Rows and columns in node order.
    Grid,
    /// Force-directed placement.
    Force,
    /// Keep the positions that are given.
    None,
}

fn default_node_gap() -> f64 {
    40.0
}
fn default_layer_gap() -> f64 {
    80.0
}

/// Layout options (`LayoutOpts` in the command catalog).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct LayoutOpts {
    /// The algorithm.
    pub algorithm: Algorithm,
    /// Where layers grow (layered and tree).
    pub direction: Direction,
    /// Space between neighbours in a layer (the other axis for grid).
    #[schemars(range(min = 0.0))]
    pub node_gap: f64,
    /// Space between layers (between rows for grid).
    #[schemars(range(min = 0.0))]
    pub layer_gap: f64,
    /// Columns of the grid layout. Absent: about the square root of the node count.
    pub columns: Option<u32>,
}

impl Default for LayoutOpts {
    fn default() -> Self {
        Self {
            algorithm: Algorithm::Layered,
            direction: Direction::Down,
            node_gap: default_node_gap(),
            layer_gap: default_layer_gap(),
            columns: None,
        }
    }
}

/// The input of a layout: node sizes and directed edges between node indexes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutGraph {
    /// Size of every node.
    pub sizes: Vec<Size>,
    /// Edges as `(from, to)` indexes into `sizes`. Duplicates and self loops are allowed.
    pub edges: Vec<(usize, usize)>,
    /// Given top-left positions, used by [`Algorithm::None`]. May be empty for the other algorithms.
    pub fixed: Vec<Option<Point>>,
}

/// The output of a layout.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutResult {
    /// Top-left of every node. The top-left of the bounding box is `(0, 0)`, except for [`Algorithm::None`].
    pub positions: Vec<Point>,
    /// For every input edge, the points between its ends that keep it clear of the other nodes (layered layout:
    /// one per layer an edge skips). Empty for most edges.
    pub waypoints: Vec<Vec<Point>>,
    /// The flow direction, when the algorithm has one.
    pub flow: Option<Direction>,
}

/// Why a layout could not run.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LayoutError {
    /// The tree layout needs a forest.
    #[error(
        "the tree layout needs every node to have at most one incoming edge and no cycles, and node {node} breaks that. Use the layered algorithm, which handles cycles."
    )]
    NotForest {
        /// Index of the node.
        node: usize,
    },
    /// A node has no usable size.
    #[error("node {node} has a size that is not a positive finite number")]
    BadSize {
        /// Index of the node.
        node: usize,
    },
    /// An edge names a node that does not exist.
    #[error("edge {edge} refers to a node index outside the graph")]
    BadEdge {
        /// Index of the edge.
        edge: usize,
    },
    /// The `none` algorithm has no position for a node.
    #[error("node {node} has no position. Give x and y for every node when the algorithm is none.")]
    MissingPosition {
        /// Index of the node.
        node: usize,
    },
    /// A gap is negative or not a number.
    #[error("{0} must be a finite number of 0 or more")]
    BadGap(&'static str),
}

/// Runs the layout.
pub fn compute_layout(g: &LayoutGraph, o: &LayoutOpts) -> Result<LayoutResult, LayoutError> {
    if !o.node_gap.is_finite() || o.node_gap < 0.0 {
        return Err(LayoutError::BadGap("node_gap"));
    }
    if !o.layer_gap.is_finite() || o.layer_gap < 0.0 {
        return Err(LayoutError::BadGap("layer_gap"));
    }
    let n = g.sizes.len();
    for (i, s) in g.sizes.iter().enumerate() {
        let ok = s.width.is_finite() && s.height.is_finite() && s.width >= 0.0 && s.height >= 0.0;
        if !ok {
            return Err(LayoutError::BadSize { node: i });
        }
    }
    for (i, &(a, b)) in g.edges.iter().enumerate() {
        if a >= n || b >= n {
            return Err(LayoutError::BadEdge { edge: i });
        }
    }
    let mut result = match o.algorithm {
        Algorithm::Layered => layered::layered(g, o),
        Algorithm::Tree => tree::tree(g, o)?,
        Algorithm::Grid => tree::grid(g, o),
        Algorithm::Force => force::force(g, o),
        Algorithm::None => {
            let mut positions = Vec::with_capacity(n);
            for i in 0..n {
                match g.fixed.get(i).copied().flatten() {
                    Some(p) if p.x.is_finite() && p.y.is_finite() => positions.push(p),
                    _ => return Err(LayoutError::MissingPosition { node: i }),
                }
            }
            return Ok(LayoutResult { positions, waypoints: vec![Vec::new(); g.edges.len()], flow: None });
        }
    };
    // The top-left of the bounding box is the origin.
    let min_x = result.positions.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let min_y = result.positions.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    if min_x.is_finite() && min_y.is_finite() {
        let shift = kurbo::Vec2::new(-min_x, -min_y);
        result.positions.iter_mut().for_each(|p| *p += shift);
        result.waypoints.iter_mut().flatten().for_each(|p| *p += shift);
    }
    Ok(result)
}

/// True when any two of the boxes overlap by more than `tolerance` in both directions.
pub fn rects_overlap(rects: &[Rect], tolerance: f64) -> bool {
    let mut idx: Vec<usize> = (0..rects.len()).collect();
    idx.sort_by(|&a, &b| rects[a].x0.total_cmp(&rects[b].x0));
    for (k, &i) in idx.iter().enumerate() {
        for &j in &idx[k + 1..] {
            if rects[j].x0 >= rects[i].x1 - tolerance {
                break;
            }
            if rects[i].y0 < rects[j].y1 - tolerance && rects[j].y0 < rects[i].y1 - tolerance {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// layout.apply
// ---------------------------------------------------------------------------------------------

/// Limits of `layout.apply`.
pub const APPLY_MAX_NODES: usize = 2000;
/// See [`APPLY_MAX_NODES`].
pub const APPLY_MAX_EDGES: usize = 5000;

/// Where the laid-out set ends up.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LayoutAnchor {
    /// The top-left of the set's bounds stays where it was.
    #[default]
    Keep,
    /// The centre of the set's bounds stays where it was.
    Center,
    /// The top-left of the new bounds goes to this point.
    At(Point),
}

/// Options of [`layout_apply`].
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyOpts {
    /// The layout.
    pub layout: LayoutOpts,
    /// Where the result sits.
    pub anchor: LayoutAnchor,
    /// Reroute the arrows between the nodes.
    pub reroute: bool,
}

impl Default for ApplyOpts {
    fn default() -> Self {
        Self { layout: LayoutOpts::default(), anchor: LayoutAnchor::Keep, reroute: true }
    }
}

/// What [`layout_apply`] changed.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyResult {
    /// Nodes that were moved.
    pub moved: Vec<ElementId>,
    /// Arrows that were rerouted.
    pub rerouted: Vec<ElementId>,
    /// The bounds of the nodes after the layout.
    pub bounds: Rect,
}

/// Why `layout.apply` refused.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ApplyError {
    /// Fewer than two nodes.
    #[error(
        "Auto-layout needs at least 2 elements to place; {0} given. Select the shapes to arrange, or pass their ids."
    )]
    TooFew(usize),
    /// An id names nothing.
    #[error("element {0} does not exist. Look up ids with element.query or doc.outline.")]
    Missing(ElementId),
    /// A node is locked.
    #[error("{0} is locked, so it cannot be moved. Unlock it first, or leave it out of the layout.")]
    Locked(ElementId),
    /// Too many nodes or edges.
    #[error("Too many {what}: the layout handles at most {max}. Lay out a smaller part.")]
    LimitExceeded {
        /// Nodes or edges.
        what: &'static str,
        /// The limit.
        max: usize,
    },
    /// The tree layout cannot handle the graph.
    #[error(
        "the tree layout needs every node to have at most one incoming edge and no cycles, and node {0} breaks that. Use algorithm \"layered\", which handles cycles."
    )]
    TreeNotForest(ElementId),
    /// The layout itself refused.
    #[error(transparent)]
    Layout(LayoutError),
    /// A write failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Moving a label failed.
    #[error(transparent)]
    Text(#[from] TextBindError),
    /// Re-solving arrows failed.
    #[error("re-solving the bound arrows failed: {0}")]
    Binding(String),
}

impl ApplyError {
    /// The refusal code of the command catalog.
    pub fn code(&self) -> &'static str {
        match self {
            ApplyError::TooFew(_) => "too_few",
            ApplyError::Missing(_) => "not_found",
            ApplyError::Locked(_) => "locked",
            ApplyError::LimitExceeded { .. } => "limit_exceeded",
            ApplyError::TreeNotForest(_) => "tree_not_forest",
            ApplyError::Layout(_) => "bad_args",
            ApplyError::Store(_) | ApplyError::Text(_) | ApplyError::Binding(_) => "internal",
        }
    }
}

fn is_node_kind(e: &drafftink_scene::Element) -> bool {
    !e.deleted && !e.tag().is_linear() && e.tag() != Kind::Frame && e.container_id().is_none()
}

/// Lays out `ids`: the non-arrow elements are nodes, the arrows bound at both ends to nodes in the set are edges.
/// Moves the nodes, reroutes the edges (once, after all moves) when `opts.reroute` is set, and keeps labels with
/// their shapes.
pub fn layout_apply<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &[ElementId],
    opts: &ApplyOpts,
) -> Result<ApplyResult, ApplyError> {
    // Nodes, in the order given, without repeats.
    let mut nodes: Vec<ElementId> = Vec::new();
    let mut seen: BTreeSet<ElementId> = BTreeSet::new();
    for id in ids {
        let e = tx.get(id).filter(|e| !e.deleted).ok_or_else(|| ApplyError::Missing(id.clone()))?;
        if is_node_kind(e) && seen.insert(id.clone()) {
            if e.locked {
                return Err(ApplyError::Locked(id.clone()));
            }
            nodes.push(id.clone());
        }
    }
    if nodes.len() < 2 {
        return Err(ApplyError::TooFew(nodes.len()));
    }
    if nodes.len() > APPLY_MAX_NODES {
        return Err(ApplyError::LimitExceeded { what: "nodes", max: APPLY_MAX_NODES });
    }
    let index: BTreeMap<&ElementId, usize> = nodes.iter().enumerate().map(|(i, id)| (id, i)).collect();

    // Edges: arrows whose two ends are bound to nodes of the set.
    struct EdgeInfo {
        arrow: ElementId,
        a: usize,
        b: usize,
    }
    let mut infos: Vec<EdgeInfo> = Vec::new();
    for e in tx.iter() {
        if e.tag() != Kind::Arrow {
            continue;
        }
        let [s, t] = e.binding_targets();
        if let (Some(s), Some(t)) = (s.and_then(|s| index.get(s)), t.and_then(|t| index.get(t))) {
            infos.push(EdgeInfo { arrow: e.id.clone(), a: *s, b: *t });
        }
    }
    if infos.len() > APPLY_MAX_EDGES {
        return Err(ApplyError::LimitExceeded { what: "edges", max: APPLY_MAX_EDGES });
    }

    // Sizes: the box around each rotated node.
    let mut aabbs: Vec<Rect> = Vec::with_capacity(nodes.len());
    for id in &nodes {
        let e = tx.get(id).ok_or_else(|| ApplyError::Missing(id.clone()))?;
        let t = &e.transform;
        aabbs.push(OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle).aabb());
    }
    let graph = LayoutGraph {
        sizes: aabbs.iter().map(|r| r.size()).collect(),
        edges: infos.iter().map(|i| (i.a, i.b)).collect(),
        fixed: aabbs.iter().map(|r| Some(r.origin())).collect(),
    };
    let result = compute_layout(&graph, &opts.layout).map_err(|e| match e {
        LayoutError::NotForest { node } => ApplyError::TreeNotForest(nodes[node].clone()),
        other => ApplyError::Layout(other),
    })?;

    // Where the set goes.
    let old = union(&aabbs);
    let new_rects: Vec<Rect> =
        result.positions.iter().zip(&graph.sizes).map(|(p, s)| Rect::from_origin_size(*p, *s)).collect();
    let new = union(&new_rects);
    let shift = match opts.anchor {
        LayoutAnchor::Keep => old.origin() - new.origin(),
        LayoutAnchor::Center => old.center() - new.center(),
        LayoutAnchor::At(p) => p - new.origin(),
    };
    let shift = if opts.layout.algorithm == Algorithm::None { kurbo::Vec2::ZERO } else { shift };

    // Move the nodes. The element keeps its offset from its rotated box.
    let mut moved = Vec::new();
    for (i, id) in nodes.iter().enumerate() {
        let target = new_rects[i].origin() + shift;
        let d = target - aabbs[i].origin();
        if d.hypot() > 0.0 {
            tx.update(id, |e| {
                e.transform.x += d.x;
                e.transform.y += d.y;
            })?;
            moved.push(id.clone());
        }
    }

    // Reroute the edges once, after all moves.
    let mut rerouted = Vec::new();
    if opts.reroute {
        let boxes: Vec<Option<NodeBox>> = nodes.iter().map(|id| tx.get(id).and_then(NodeBox::of)).collect();
        let rects: Vec<Rect> = nodes.iter().map(|id| tx.get(id).map_or(Rect::ZERO, |e| e.rect())).collect();
        let mut lanes: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let mut totals: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        for i in &infos {
            *totals.entry((i.a.min(i.b), i.a.max(i.b))).or_default() += 1;
        }
        for (k, info) in infos.iter().enumerate() {
            let arrow = tx.get(&info.arrow).ok_or_else(|| ApplyError::Missing(info.arrow.clone()))?;
            if arrow.locked {
                continue;
            }
            let path = arrow.linear_data().map_or(PathType::Sharp, |d| d.path);
            let key = (info.a.min(info.b), info.a.max(info.b));
            let lane_i = lanes.entry(key).or_default();
            let lane = (*lane_i, totals.get(&key).copied().unwrap_or(1));
            *lane_i += 1;
            let (Some(Some(from)), Some(Some(to))) = (boxes.get(info.a), boxes.get(info.b)) else { continue };
            let route = if info.a == info.b {
                route_self_loop(from)
            } else {
                let waypoints: Vec<Point> = result.waypoints[k].iter().map(|p| *p + shift).collect();
                let obstacles: Vec<Rect> = if waypoints.is_empty() {
                    Vec::new()
                } else {
                    rects.iter().enumerate().filter(|(i, _)| *i != info.a && *i != info.b).map(|(_, r)| *r).collect()
                };
                let p = RouteParams { path, waypoints: &waypoints, flow: result.flow, lane, obstacles: &obstacles };
                route_edge(from, to, &p)
            };
            tx.update(&info.arrow, |e| reroute_element(e, &route))?;
            rerouted.push(info.arrow.clone());
        }
    }

    // Labels follow their shapes and arrows; other arrows bound to the moved shapes are re-solved.
    let mut touched: Vec<ElementId> = moved.clone();
    touched.extend(rerouted.iter().cloned());
    refresh_labels(tx, m, &touched, GrowAnchor::Top)?;
    update_bound_arrows(tx, &moved).map_err(|e| ApplyError::Binding(e.to_string()))?;
    let bounds =
        nodes.iter().filter_map(|id| tx.get(id)).map(|e| e.rect()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
    Ok(ApplyResult { moved, rerouted, bounds })
}

fn union(rects: &[Rect]) -> Rect {
    rects.iter().copied().reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO)
}

/// The bounds of every live element, or `None` for an empty scene.
pub fn scene_bounds<R: SceneRead>(r: &R) -> Option<Rect> {
    r.iter()
        .filter(|e| e.container_id().is_none())
        .map(|e| {
            let t = &e.transform;
            OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle).aabb()
        })
        .reduce(|a, b| a.union(b))
}
