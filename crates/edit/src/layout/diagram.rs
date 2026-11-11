//! `diagram.create`: a diagram from nodes and edges with symbolic ids.
//!
//! The spec is plain data (serde and JSON Schema). The Mermaid importer maps into the same spec, so both paths share
//! the layout, the sizing and the rules below.
//!
//! One call creates, in this order: every node (each followed by its label), then every edge as an arrow bound at
//! both ends (each followed by its label), then the optional frame. Each arrow and label sits directly above what
//! it belongs to. The caller commits the transaction, so a diagram is one undo step.

use std::collections::{BTreeMap, BTreeSet};

use drafftink_geom::kurbo::{Point, Rect, Size, Vec2};

use crate::binding::BindingError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use drafftink_scene::{
    Color, Element, ElementId, ElementKind, FillPattern, FontFamily, FrameData, GroupId, Head, IdSource, IndexError,
    Kind, PathType, SceneRead, SceneTx, Sloppiness, StoreError, StrokeStyle, Style, TextData, TextStyle, ZIndex,
};

use super::cluster::{CLUSTER_TITLE_BAND, ClusterIn, ClusteredInput, MAX_CLUSTER_DEPTH, layout_clustered};
use super::route::{Heads, NodeBox, RouteParams, arrow_element, route_edge, route_self_loop};
use super::{Algorithm, Direction, LayoutError, LayoutGraph, LayoutOpts, LayoutResult, compute_layout, scene_bounds};
use crate::binding::update_bound_arrows;
use crate::ids::unused_element_id;
use crate::text_bind::{
    TextBindError, TextMeasure, container_dim_for_text, create_label, inner_size, measure_lines, measure_text,
};

/// Most nodes in one diagram.
pub const MAX_NODES: usize = 2000;
/// Most edges in one diagram.
pub const MAX_EDGES: usize = 5000;
/// Smallest automatic node size.
const MIN_NODE: Size = Size::new(80.0, 48.0);
/// A node label wraps at this width when the node has no width of its own.
const AUTO_LABEL_WIDTH: f64 = 240.0;
const FRAME_PADDING: f64 = 32.0;
const MAX_SYMBOLIC_LEN: usize = 64;
/// Most clusters in one diagram.
pub const MAX_CLUSTERS: usize = 1000;
/// Fill of a cluster box: light slate, so that the nodes inside stay legible.
const CLUSTER_FILL: Color = Color::rgb(0xf1, 0xf5, 0xf9);
/// Outline of a cluster box.
const CLUSTER_STROKE: Color = Color::rgb(0x94, 0xa3, 0xb8);
const CLUSTER_CORNER_RADIUS: f64 = 8.0;

// ---------------------------------------------------------------------------------------------
// The spec
// ---------------------------------------------------------------------------------------------

/// The shape of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeShape {
    /// Rectangle (the default).
    #[default]
    Rectangle,
    /// Ellipse.
    Ellipse,
    /// Diamond.
    Diamond,
    /// A free text element that arrows can bind to. Needs a label.
    Text,
}

/// The few style properties a node or an edge can set. Everything left out keeps the default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct StyleSpec {
    /// Outline colour (text colour for a text node).
    pub stroke_color: Option<Color>,
    /// Outline width.
    pub stroke_width: Option<f64>,
    /// Dash pattern.
    pub stroke_style: Option<StrokeStyle>,
    /// Fill colour. `"transparent"` means no fill.
    pub fill_color: Option<Color>,
    /// Fill pattern.
    pub fill_pattern: Option<FillPattern>,
    /// How hand-drawn the outline is.
    pub sloppiness: Option<Sloppiness>,
    /// Opacity, 0 to 1.
    pub opacity: Option<f64>,
    /// Corner radius.
    pub corner_radius: Option<f64>,
    /// Font family of the label.
    pub font_family: Option<FontFamily>,
    /// Font size of the label.
    pub font_size: Option<f64>,
}

impl StyleSpec {
    fn apply(&self, s: &mut Style) {
        if let Some(c) = self.stroke_color {
            s.stroke_color = c;
        }
        if let Some(v) = self.stroke_width {
            s.stroke_width = v;
        }
        if let Some(v) = self.stroke_style {
            s.stroke_style = v;
        }
        if let Some(c) = self.fill_color {
            s.fill_color = if c.a == 0 { None } else { Some(c) };
        }
        if let Some(v) = self.fill_pattern {
            s.fill_pattern = v;
        }
        if let Some(v) = self.sloppiness {
            s.sloppiness = v;
        }
        if let Some(v) = self.opacity {
            s.opacity = v;
        }
        if let Some(v) = self.corner_radius {
            s.corner_radius = v;
        }
    }

    fn font(&self) -> TextStyle {
        let mut f = TextStyle::default();
        if let Some(v) = self.font_family {
            f.family = v;
        }
        if let Some(v) = self.font_size {
            f.size = v;
        }
        f
    }
}

/// One node of the diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSpec {
    /// The symbolic id: a name the edges and groups use, unique in the diagram. The result maps it to the real id.
    pub id: String,
    /// The text inside the node. Absent: an empty shape.
    #[serde(default)]
    pub label: Option<String>,
    /// The shape.
    #[serde(default)]
    pub shape: NodeShape,
    /// Style.
    #[serde(default)]
    pub style: Option<StyleSpec>,
    /// Width. Absent: measured from the label, at least 80.
    #[serde(default)]
    pub width: Option<f64>,
    /// Height. Absent: measured from the label, at least 48.
    #[serde(default)]
    pub height: Option<f64>,
    /// Width and height together. `width` and `height` win when both are given.
    #[serde(default)]
    pub size: Option<[f64; 2]>,
    /// The element `name`, so that later calls can say `@name`.
    #[serde(default)]
    pub name: Option<String>,
    /// Position for the `none` layout.
    #[serde(default)]
    pub x: Option<f64>,
    /// Position for the `none` layout.
    #[serde(default)]
    pub y: Option<f64>,
}

impl NodeSpec {
    /// A node with a label and defaults for everything else.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: Some(label.into()),
            shape: NodeShape::Rectangle,
            style: None,
            width: None,
            height: None,
            size: None,
            name: None,
            x: None,
            y: None,
        }
    }
}

/// Which ends of an edge have an arrowhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Arrowheads {
    /// No heads.
    None,
    /// A head at the target (the default).
    #[default]
    #[serde(alias = "forward")]
    End,
    /// A head at the source.
    #[serde(alias = "backward")]
    Start,
    /// Heads at both ends.
    Both,
}

/// One edge: an arrow from one node to another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EdgeSpec {
    /// Symbolic id of the source node.
    pub from: String,
    /// Symbolic id of the target node. May equal `from` (a self loop).
    pub to: String,
    /// Text on the arrow.
    #[serde(default)]
    pub label: Option<String>,
    /// Straight (`sharp`), curved (`round`) or `elbow`. Default `sharp`.
    #[serde(default, alias = "path")]
    pub arrow_type: Option<PathType>,
    /// Which ends have a head. Default: at the target.
    #[serde(default)]
    pub arrowheads: Option<Arrowheads>,
    /// Marker at the source. Overrides `arrowheads`.
    #[serde(default)]
    pub start_head: Option<Head>,
    /// Marker at the target. Overrides `arrowheads`.
    #[serde(default)]
    pub end_head: Option<Head>,
    /// Style of the arrow.
    #[serde(default)]
    pub style: Option<StyleSpec>,
    /// The element `name`.
    #[serde(default)]
    pub name: Option<String>,
}

impl EdgeSpec {
    /// An unlabelled edge with defaults.
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            label: None,
            arrow_type: None,
            arrowheads: None,
            start_head: None,
            end_head: None,
            style: None,
            name: None,
        }
    }
}

/// A set of nodes that is grouped. Groups may be nested (one contains the other) but not partly overlap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupSpec {
    /// A symbolic id for the result, optional.
    #[serde(default)]
    pub id: Option<String>,
    /// Symbolic ids of the member nodes (at least 2).
    pub members: Vec<String>,
}

/// A titled box around some nodes (a Mermaid subgraph). The box is a rectangle with a title above the nodes. The
/// layout puts the members inside it, and edges may start or end at it.
///
/// A cluster is drawn behind its members: a rounded rectangle with a light fill, and a text element with the
/// title centred in a band at the top. These are plain elements, not a frame: a frame clips what it holds and a
/// shape can be in one frame only, so frames cannot nest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClusterSpec {
    /// A symbolic id, unique among the nodes and the clusters. Edges may use it as an end.
    pub id: String,
    /// The title. Empty: no title text is made.
    #[serde(default)]
    pub title: String,
    /// The id of the cluster around this one.
    #[serde(default)]
    pub parent: Option<String>,
    /// The ids of the nodes directly inside. A node is in one cluster at most; nodes of nested clusters are not
    /// listed here.
    #[serde(default)]
    pub members: Vec<String>,
    /// The flow inside this cluster. Absent: the one around it.
    #[serde(default)]
    pub direction: Option<Direction>,
    /// Put the box, the title and everything inside in one group. Groups of nested clusters nest.
    #[serde(default)]
    pub group: bool,
}

/// A whole diagram.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagramSpec {
    /// The nodes.
    pub nodes: Vec<NodeSpec>,
    /// The edges.
    #[serde(default)]
    pub edges: Vec<EdgeSpec>,
    /// Groups of nodes. Arrows between two members of a group join it. When there are clusters, these groups sit
    /// outside every cluster group, and a cluster box joins the groups that hold all of its nodes.
    #[serde(default)]
    pub groups: Vec<GroupSpec>,
    /// Titled boxes around nodes.
    #[serde(default)]
    pub clusters: Vec<ClusterSpec>,
    /// How to place the nodes.
    #[serde(default)]
    pub layout: LayoutOpts,
}

/// Where and how the diagram goes into the scene.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CreateOpts {
    /// Top-left of the diagram. Ignored by the `none` layout, which uses the given positions.
    pub origin: Point,
    /// Wrap the diagram in a frame with this name.
    pub frame: Option<String>,
}

/// What was created.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DiagramResult {
    /// Symbolic node id to real element id.
    pub nodes: BTreeMap<String, ElementId>,
    /// The arrows, in the order of the spec's edges.
    pub edges: Vec<ElementId>,
    /// Symbolic group id (for groups that have one) to the real group id. A cluster that is grouped appears under
    /// its own id.
    pub groups: BTreeMap<String, GroupId>,
    /// Symbolic cluster id to the element id of its box.
    pub clusters: BTreeMap<String, ElementId>,
    /// Every group created, in spec order.
    pub group_ids: Vec<GroupId>,
    /// The frame, when one was asked for.
    pub frame: Option<ElementId>,
    /// Bounds of the diagram as `[x, y, w, h]`.
    pub bounds: [f64; 4],
    /// Every element created (nodes, labels, arrows, frame), in z-order from the back.
    pub created: Vec<ElementId>,
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// Why a diagram was refused. Nothing is created in that case (the caller drops the transaction).
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum DiagramError {
    /// No nodes.
    #[error("Give at least one node.")]
    NoNodes,
    /// A node id is used twice.
    #[error("Node id `{0}` appears twice. Node ids must be unique.")]
    DuplicateNode(String),
    /// A node id is empty, too long or has control characters.
    #[error("Node id `{0}` is not usable. An id is 1 to 64 characters without control characters.")]
    BadNodeId(String),
    /// An edge names a node that is not in the spec.
    #[error("Edge {index} refers to node `{id}`, which is not in `nodes`. Known nodes: {known}.")]
    UnknownNode {
        /// Position of the edge in `edges`, 0-based.
        index: usize,
        /// The unknown id.
        id: String,
        /// The known ids, comma separated (the first 20).
        known: String,
    },
    /// A group names a node that is not in the spec.
    #[error("Group {index} refers to node `{id}`, which is not in `nodes`. Known nodes: {known}.")]
    UnknownGroupMember {
        /// Position of the group, 0-based.
        index: usize,
        /// The unknown id.
        id: String,
        /// The known ids.
        known: String,
    },
    /// A cluster is not usable.
    #[error("Cluster `{id}` is not usable: {reason}")]
    BadCluster {
        /// The cluster's symbolic id.
        id: String,
        /// What is wrong, with the fix.
        reason: String,
    },
    /// A group is not usable.
    #[error("Group {index} is not usable: {reason}")]
    BadGroup {
        /// Position of the group, 0-based.
        index: usize,
        /// What is wrong, with the fix.
        reason: String,
    },
    /// Two groups overlap without one containing the other, or two groups have the same members.
    #[error("Groups {a} and {b} overlap without one containing the other. Nest them or make them disjoint.")]
    GroupsOverlap {
        /// First group.
        a: usize,
        /// Second group.
        b: usize,
    },
    /// The tree layout needs a forest.
    #[error(
        "The tree layout needs every node to have at most one incoming edge and no cycles, and node `{0}` breaks that. Use algorithm \"layered\", which handles cycles."
    )]
    TreeNotForest(String),
    /// Too many nodes or edges.
    #[error("Too many {what}: a diagram has at most {max}. Split it into several diagrams.")]
    LimitExceeded {
        /// Nodes or edges.
        what: &'static str,
        /// The limit.
        max: usize,
    },
    /// A text node needs text.
    #[error("Node `{0}` is a text node and needs a label.")]
    EmptyTextNode(String),
    /// A size is not a positive finite number.
    #[error("Node `{0}` has a width or height that is not a positive number.")]
    BadSize(String),
    /// A `none` layout node has no position.
    #[error("Node `{0}` has no position. Give x and y for every node when the algorithm is none.")]
    MissingPosition(String),
    /// A name is not allowed.
    #[error(
        "The name `{0}` is not usable. A name is 1 to 64 characters, does not start with @ or $, and has no control characters."
    )]
    BadName(String),
    /// A name is used twice.
    #[error(
        "The name `{name}` is already used by element {other}. Names must be unique so that @{name} is unambiguous. Pick another name."
    )]
    NameTaken {
        /// The name.
        name: String,
        /// The element that has it (for a clash inside the spec, the symbolic node id).
        other: String,
    },
    /// The layout refused.
    #[error(transparent)]
    Layout(LayoutError),
    /// A label could not be made.
    #[error(transparent)]
    Text(#[from] TextBindError),
    /// The scene refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// No z-order key could be made.
    #[error(transparent)]
    Index(#[from] IndexError),
    /// The geometry came out non-finite.
    #[error("The diagram has coordinates that are not finite numbers. Check sizes and positions.")]
    NotFinite,
    /// Re-solving the arrows failed.
    #[error("re-solving the bound arrows failed: {0}")]
    Binding(#[from] BindingError),
}

// ---------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------

fn known_list(spec: &DiagramSpec) -> String {
    let v: Vec<&str> = spec.nodes.iter().take(20).map(|n| n.id.as_str()).collect();
    let more = spec.nodes.len().saturating_sub(20);
    let mut s = v.join(", ");
    if more > 0 {
        s.push_str(&format!(", and {more} more"));
    }
    s
}

fn check_name(name: &str) -> bool {
    let n = name.chars().count();
    (1..=64).contains(&n) && !name.starts_with('@') && !name.starts_with('$') && !name.chars().any(char::is_control)
}

/// The groups of a spec, checked to nest.
struct GroupSets {
    /// The member set of every group.
    members: Vec<BTreeSet<usize>>,
    /// The groups each node is in, smallest first.
    per_node: Vec<Vec<usize>>,
}

fn resolve_groups(spec: &DiagramSpec, index: &BTreeMap<&str, usize>) -> Result<GroupSets, DiagramError> {
    let mut members: Vec<BTreeSet<usize>> = Vec::new();
    for (gi, g) in spec.groups.iter().enumerate() {
        let mut set = BTreeSet::new();
        for m in &g.members {
            let i = index.get(m.as_str()).copied().ok_or_else(|| DiagramError::UnknownGroupMember {
                index: gi,
                id: m.clone(),
                known: known_list(spec),
            })?;
            set.insert(i);
        }
        if set.len() < 2 {
            return Err(DiagramError::BadGroup {
                index: gi,
                reason: "a group needs at least 2 different member nodes. Add members or drop the group.".into(),
            });
        }
        members.push(set);
    }
    for a in 0..members.len() {
        for b in a + 1..members.len() {
            let inter = members[a].intersection(&members[b]).count();
            if inter == 0 {
                continue;
            }
            let nested = inter == members[a].len() || inter == members[b].len();
            if !nested || members[a] == members[b] {
                return Err(DiagramError::GroupsOverlap { a, b });
            }
        }
    }
    // Per node: the groups it is in, smallest first.
    let mut per_node: Vec<Vec<usize>> = vec![Vec::new(); spec.nodes.len()];
    for (gi, set) in members.iter().enumerate() {
        for &n in set {
            per_node[n].push(gi);
        }
    }
    for list in &mut per_node {
        list.sort_by_key(|&g| (members[g].len(), g));
    }
    Ok(GroupSets { members, per_node })
}

/// The clusters of a spec, after the checks. Cluster `c` is item `nodes + c`.
#[derive(Debug, Default)]
struct ClusterPlan {
    /// The innermost cluster of every node.
    node_cluster: Vec<Option<usize>>,
    /// The cluster around every cluster.
    parent: Vec<Option<usize>>,
    /// How deeply every cluster is nested (0: at the top level).
    depth: Vec<usize>,
    /// Every node in the cluster, those of nested clusters included.
    nodes_in: Vec<BTreeSet<usize>>,
}

impl ClusterPlan {
    /// The clusters from `start` outwards.
    fn chain(&self, start: Option<usize>) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(start, |&c| self.parent.get(c).copied().flatten())
    }
}

fn bad_cluster(id: &str, reason: impl Into<String>) -> DiagramError {
    DiagramError::BadCluster { id: id.to_owned(), reason: reason.into() }
}

/// Checks the ids, parents and members of the clusters.
fn resolve_clusters(spec: &DiagramSpec, node_index: &BTreeMap<&str, usize>) -> Result<ClusterPlan, DiagramError> {
    if spec.clusters.len() > MAX_CLUSTERS {
        return Err(DiagramError::LimitExceeded { what: "clusters", max: MAX_CLUSTERS });
    }
    let mut cluster_index: BTreeMap<&str, usize> = BTreeMap::new();
    for (c, cl) in spec.clusters.iter().enumerate() {
        let len = cl.id.chars().count();
        if len == 0 || len > MAX_SYMBOLIC_LEN || cl.id.chars().any(char::is_control) {
            return Err(DiagramError::BadNodeId(cl.id.clone()));
        }
        if node_index.contains_key(cl.id.as_str()) || cluster_index.insert(cl.id.as_str(), c).is_some() {
            return Err(DiagramError::DuplicateNode(cl.id.clone()));
        }
    }
    let mut plan = ClusterPlan {
        node_cluster: vec![None; spec.nodes.len()],
        parent: Vec::with_capacity(spec.clusters.len()),
        depth: Vec::with_capacity(spec.clusters.len()),
        nodes_in: vec![BTreeSet::new(); spec.clusters.len()],
    };
    for cl in &spec.clusters {
        let parent = match &cl.parent {
            None => None,
            Some(p) => Some(cluster_index.get(p.as_str()).copied().ok_or_else(|| {
                let known: Vec<&str> = spec.clusters.iter().take(20).map(|c| c.id.as_str()).collect();
                bad_cluster(&cl.id, format!("its parent `{p}` is not a cluster. Known clusters: {}.", known.join(", ")))
            })?),
        };
        plan.parent.push(parent);
    }
    for (c, cl) in spec.clusters.iter().enumerate() {
        let depth = plan.chain(plan.parent[c]).take(MAX_CLUSTER_DEPTH + 1).count();
        if depth > MAX_CLUSTER_DEPTH {
            return Err(bad_cluster(
                &cl.id,
                format!("clusters are nested more than {MAX_CLUSTER_DEPTH} deep, or around each other. Flatten them."),
            ));
        }
        plan.depth.push(depth);
        for m in &cl.members {
            let n = node_index.get(m.as_str()).copied().ok_or_else(|| DiagramError::UnknownGroupMember {
                index: c,
                id: m.clone(),
                known: known_list(spec),
            })?;
            if plan.node_cluster[n].replace(c).is_some() {
                return Err(bad_cluster(
                    &cl.id,
                    format!("node `{m}` is a member of two clusters. A node belongs to one."),
                ));
            }
        }
    }
    for (n, cluster) in plan.node_cluster.clone().into_iter().enumerate() {
        for c in plan.chain(cluster).collect::<Vec<_>>() {
            plan.nodes_in[c].insert(n);
        }
    }
    Ok(plan)
}

// ---------------------------------------------------------------------------------------------
// Sizing
// ---------------------------------------------------------------------------------------------

fn positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// The size of a node that holds `label`.
fn node_size<M: TextMeasure + ?Sized>(
    m: &M,
    kind: Kind,
    font: &TextStyle,
    label: Option<&str>,
    width: Option<f64>,
    height: Option<f64>,
) -> Size {
    let Some(label) = label.filter(|l| !l.trim().is_empty()) else {
        return Size::new(width.unwrap_or(MIN_NODE.width), height.unwrap_or(MIN_NODE.height));
    };
    if kind == Kind::Text {
        let s = measure_text(m, label, font, width);
        return Size::new(width.unwrap_or(s.width), height.unwrap_or(s.height).max(s.height));
    }
    // Width: given, or what the label needs when wrapped at the automatic width.
    let natural = {
        let lines = m.wrap(label, font, AUTO_LABEL_WIDTH);
        measure_lines(m, &lines, font)
    };
    let mut w = width.unwrap_or_else(|| container_dim_for_text(natural.width, kind).max(MIN_NODE.width));
    // Height: whatever the text needs at that width, and at least the given or the minimum height.
    for _ in 0..2 {
        let inner = inner_size(kind, Size::new(w, 0.0)).map_or(0.0, |s| s.width);
        let lines = m.wrap(label, font, inner);
        let text = measure_lines(m, &lines, font);
        if text.width > inner {
            w = container_dim_for_text(text.width, kind).max(w);
        } else {
            let h = height.unwrap_or(MIN_NODE.height).max(container_dim_for_text(text.height, kind));
            return Size::new(w, h);
        }
    }
    let inner = inner_size(kind, Size::new(w, 0.0)).map_or(0.0, |s| s.width);
    let text = measure_lines(m, &m.wrap(label, font, inner), font);
    Size::new(w, height.unwrap_or(MIN_NODE.height).max(container_dim_for_text(text.height, kind)))
}

/// The origin that puts a diagram `gap` below everything in the scene (the origin for `at: below_content`).
pub fn below_content_origin<R: SceneRead>(r: &R, gap: f64) -> Point {
    match scene_bounds(r) {
        Some(b) => Point::new(b.x0, b.y1 + gap),
        None => Point::ZERO,
    }
}

// ---------------------------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------------------------

fn heads_of(e: &EdgeSpec) -> Heads {
    let base = match e.arrowheads.unwrap_or_default() {
        Arrowheads::None => Heads { start: Head::None, end: Head::None },
        Arrowheads::End => Heads { start: Head::None, end: Head::Arrow },
        Arrowheads::Start => Heads { start: Head::Arrow, end: Head::None },
        Arrowheads::Both => Heads { start: Head::Arrow, end: Head::Arrow },
    };
    Heads { start: e.start_head.unwrap_or(base.start), end: e.end_head.unwrap_or(base.end) }
}

/// What the checks of a spec found out. Items are numbered nodes first, then clusters.
struct Checked {
    /// The items each edge joins.
    edge_ends: Vec<(usize, usize)>,
    /// The groups each item is in, innermost first. An index is into the group ids made for the spec: the groups
    /// of the spec, then the groups of the clusters that ask for one.
    item_groups: Vec<Vec<usize>>,
    /// The index of the group of every cluster that has one.
    cluster_group: Vec<Option<usize>>,
    clusters: ClusterPlan,
}

impl Checked {
    /// How many groups the diagram makes.
    fn group_count(&self, spec: &DiagramSpec) -> usize {
        spec.groups.len() + self.cluster_group.iter().flatten().count()
    }
}

/// Checks counts, ids, edges, groups and names (unique inside the spec and against the scene).
fn check_spec<R: SceneRead>(scene: &R, spec: &DiagramSpec) -> Result<Checked, DiagramError> {
    if spec.nodes.is_empty() {
        return Err(DiagramError::NoNodes);
    }
    if spec.nodes.len() > MAX_NODES {
        return Err(DiagramError::LimitExceeded { what: "nodes", max: MAX_NODES });
    }
    if spec.edges.len() > MAX_EDGES {
        return Err(DiagramError::LimitExceeded { what: "edges", max: MAX_EDGES });
    }
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, n) in spec.nodes.iter().enumerate() {
        let len = n.id.chars().count();
        if len == 0 || len > MAX_SYMBOLIC_LEN || n.id.chars().any(char::is_control) {
            return Err(DiagramError::BadNodeId(n.id.clone()));
        }
        if index.insert(n.id.as_str(), i).is_some() {
            return Err(DiagramError::DuplicateNode(n.id.clone()));
        }
    }
    let clusters = resolve_clusters(spec, &index)?;
    let n = spec.nodes.len();
    let cluster_index: BTreeMap<&str, usize> =
        spec.clusters.iter().enumerate().map(|(c, cl)| (cl.id.as_str(), n + c)).collect();
    let mut edge_ends: Vec<(usize, usize)> = Vec::with_capacity(spec.edges.len());
    for (i, e) in spec.edges.iter().enumerate() {
        let find = |id: &str| {
            index.get(id).or_else(|| cluster_index.get(id)).copied().ok_or_else(|| DiagramError::UnknownNode {
                index: i,
                id: id.to_string(),
                known: known_list(spec),
            })
        };
        edge_ends.push((find(&e.from)?, find(&e.to)?));
    }
    let GroupSets { members: group_members, per_node: explicit } = resolve_groups(spec, &index)?;
    // The group of a cluster comes after the groups of the spec.
    let mut next_group = spec.groups.len();
    let cluster_group: Vec<Option<usize>> = spec
        .clusters
        .iter()
        .map(|cl| {
            cl.group.then(|| {
                next_group += 1;
                next_group - 1
            })
        })
        .collect();
    // A node is in the groups of its clusters (innermost first), then in the groups of the spec.
    let mut item_groups: Vec<Vec<usize>> = Vec::with_capacity(n + spec.clusters.len());
    for (node, spec_groups) in explicit.iter().enumerate() {
        let own = clusters.chain(clusters.node_cluster[node]).filter_map(|c| cluster_group[c]);
        item_groups.push(own.chain(spec_groups.iter().copied()).collect());
    }
    // A cluster box is in its own group and those around it, then in the groups of the spec that hold every node
    // of the cluster.
    for c in 0..spec.clusters.len() {
        let own = clusters.chain(Some(c)).filter_map(|c| cluster_group[c]);
        let mut outer: Vec<usize> = (0..group_members.len())
            .filter(|&g| !clusters.nodes_in[c].is_empty() && clusters.nodes_in[c].is_subset(&group_members[g]))
            .collect();
        outer.sort_by_key(|&g| (group_members[g].len(), g));
        item_groups.push(own.chain(outer).collect());
    }

    let mut taken: BTreeMap<String, String> =
        scene.iter().filter_map(|e| e.name.clone().map(|n| (n, e.id.to_string()))).collect();
    let names = spec
        .nodes
        .iter()
        .map(|n| (n.name.as_ref(), n.id.as_str()))
        .chain(spec.edges.iter().map(|e| (e.name.as_ref(), "an edge")));
    for (name, owner) in names {
        let Some(name) = name else { continue };
        if !check_name(name) {
            return Err(DiagramError::BadName(name.clone()));
        }
        if let Some(other) = taken.insert(name.clone(), owner.to_string()) {
            return Err(DiagramError::NameTaken { name: name.clone(), other });
        }
    }
    Ok(Checked { edge_ends, item_groups, cluster_group, clusters })
}

/// The size and the font of every node.
fn plan_nodes<M: TextMeasure + ?Sized>(m: &M, spec: &DiagramSpec) -> Result<(Vec<Size>, Vec<TextStyle>), DiagramError> {
    let mut sizes = Vec::with_capacity(spec.nodes.len());
    let mut fonts = Vec::with_capacity(spec.nodes.len());
    for n in &spec.nodes {
        let kind = match n.shape {
            NodeShape::Rectangle => Kind::Rectangle,
            NodeShape::Ellipse => Kind::Ellipse,
            NodeShape::Diamond => Kind::Diamond,
            NodeShape::Text => Kind::Text,
        };
        if kind == Kind::Text && n.label.as_deref().is_none_or(|l| l.trim().is_empty()) {
            return Err(DiagramError::EmptyTextNode(n.id.clone()));
        }
        let width = n.width.or(n.size.map(|s| s[0]));
        let height = n.height.or(n.size.map(|s| s[1]));
        if width.is_some_and(|v| !positive(v)) || height.is_some_and(|v| !positive(v)) {
            return Err(DiagramError::BadSize(n.id.clone()));
        }
        let font = n.style.as_ref().map(StyleSpec::font).unwrap_or_default();
        if !positive(font.size) {
            return Err(DiagramError::BadSize(n.id.clone()));
        }
        sizes.push(node_size(m, kind, &font, n.label.as_deref(), width, height));
        fonts.push(font);
    }
    Ok((sizes, fonts))
}

/// Where the nodes and the cluster boxes go.
struct Placed {
    layout: LayoutResult,
    /// The box of every cluster.
    boxes: Vec<Rect>,
}

/// The size of the title text of every cluster. No title: no size.
fn title_sizes<M: TextMeasure + ?Sized>(m: &M, spec: &DiagramSpec) -> Vec<Size> {
    let font = TextStyle::default();
    spec.clusters
        .iter()
        .map(|c| if c.title.trim().is_empty() { Size::ZERO } else { measure_text(m, &c.title, &font, None) })
        .collect()
}

/// Places the nodes and the clusters. Returns the layout and the shift that moves it to the requested origin.
fn place_nodes(
    spec: &DiagramSpec,
    sizes: &[Size],
    titles: &[Size],
    checked: &Checked,
    requested_origin: Point,
) -> Result<(Placed, Vec2), DiagramError> {
    let node_id = |i: usize| spec.nodes.get(i).map_or_else(String::new, |n| n.id.clone());
    let node_error = |e: LayoutError| match e {
        LayoutError::NotForest { node } => DiagramError::TreeNotForest(node_id(node)),
        LayoutError::BadSize { node } => DiagramError::BadSize(node_id(node)),
        LayoutError::MissingPosition { node } => DiagramError::MissingPosition(node_id(node)),
        LayoutError::BadCluster { cluster } => DiagramError::BadCluster {
            id: spec.clusters.get(cluster).map_or_else(String::new, |c| c.id.clone()),
            reason: e.to_string(),
        },
        other => DiagramError::Layout(other),
    };
    let placed = if spec.clusters.is_empty() {
        let fixed: Vec<Option<Point>> =
            spec.nodes.iter().map(|n| n.x.zip(n.y).map(|(x, y)| Point::new(x, y))).collect();
        if spec.layout.algorithm == Algorithm::None
            && let Some(i) = fixed.iter().position(Option::is_none)
        {
            return Err(DiagramError::MissingPosition(spec.nodes[i].id.clone()));
        }
        let graph = LayoutGraph { sizes: sizes.to_vec(), edges: checked.edge_ends.clone(), fixed };
        Placed { layout: compute_layout(&graph, &spec.layout).map_err(node_error)?, boxes: Vec::new() }
    } else {
        if spec.layout.algorithm == Algorithm::None {
            return Err(bad_cluster(
                &spec.clusters[0].id,
                "boxes around nodes need an automatic layout. Use layered, tree, grid or force.",
            ));
        }
        let plan = &checked.clusters;
        let clusters: Vec<ClusterIn> = spec
            .clusters
            .iter()
            .zip(titles)
            .zip(&plan.parent)
            .map(|((c, t), &parent)| ClusterIn { parent, title: *t, direction: c.direction })
            .collect();
        let input = ClusteredInput {
            node_sizes: sizes,
            node_cluster: &plan.node_cluster,
            clusters: &clusters,
            edges: &checked.edge_ends,
        };
        let laid = layout_clustered(&input, &spec.layout).map_err(node_error)?;
        Placed {
            layout: LayoutResult { positions: laid.nodes, waypoints: laid.waypoints, flow: laid.flow },
            boxes: laid.boxes,
        }
    };
    let origin = if spec.layout.algorithm == Algorithm::None { Point::ZERO } else { requested_origin };
    if !origin.x.is_finite() || !origin.y.is_finite() {
        return Err(DiagramError::NotFinite);
    }
    Ok((placed, origin.to_vec2()))
}

/// What the creation steps share.
struct Build<'a, M: ?Sized> {
    m: &'a M,
    spec: &'a DiagramSpec,
    checked: &'a Checked,
    group_ids: &'a [GroupId],
    frame_id: Option<&'a ElementId>,
}

/// The elements made for the clusters, indexed like the clusters of the spec.
struct ClusterSet {
    ids: Vec<ElementId>,
    boxes: Vec<NodeBox>,
}

/// The elements made for the nodes.
struct NodeSet {
    ids: Vec<ElementId>,
    boxes: Vec<NodeBox>,
    rects: Vec<Rect>,
    created: Vec<ElementId>,
}

impl<M: TextMeasure + ?Sized> Build<'_, M> {
    /// Makes the cluster boxes and their titles, outermost first, so that inner boxes lie on top of outer ones.
    /// Every id goes into `created` in the order of creation.
    fn create_clusters<T: SceneTx>(
        &self,
        tx: &mut T,
        ids: &mut dyn IdSource,
        placed: &Placed,
        titles: &[Size],
        shift: Vec2,
        created: &mut Vec<ElementId>,
    ) -> Result<ClusterSet, DiagramError> {
        let n = self.spec.nodes.len();
        let plan = &self.checked.clusters;
        let mut order: Vec<usize> = (0..self.spec.clusters.len()).collect();
        order.sort_by_key(|&c| (plan.depth[c], c));
        let mut made: Vec<Option<(ElementId, NodeBox)>> = vec![None; order.len()];
        for c in order {
            let rect = placed.boxes[c] + shift;
            let groups: Vec<GroupId> =
                self.checked.item_groups[n + c].iter().map(|&g| self.group_ids[g].clone()).collect();
            let id = unused_element_id(tx, ids);
            let style = Style {
                stroke_color: CLUSTER_STROKE,
                stroke_width: 1.5,
                fill_color: Some(CLUSTER_FILL),
                fill_pattern: FillPattern::Solid,
                corner_radius: CLUSTER_CORNER_RADIUS,
                seed: ids.seed(),
                ..Style::default()
            };
            let mut e = Element::new(id.clone(), ElementKind::Rectangle, tx.next_top_index()?)
                .with_bounds(rect.x0, rect.y0, rect.width(), rect.height())
                .with_style(style);
            e.frame_id = self.frame_id.cloned();
            e.group_ids.extend(groups.iter().cloned());
            let node_box = NodeBox::of(&e).ok_or(DiagramError::NotFinite)?;
            tx.insert(e)?;
            created.push(id.clone());
            let title = &self.spec.clusters[c].title;
            if !title.trim().is_empty() {
                let size = titles[c];
                let mut d = TextData::new(title.as_str());
                d.font.align = drafftink_scene::TextAlign::Center;
                let tid = unused_element_id(tx, ids);
                let x = rect.center().x - size.width / 2.0;
                let y = rect.y0 + CLUSTER_TITLE_BAND / 2.0 - size.height / 2.0;
                let mut t = Element::new(tid.clone(), ElementKind::Text(d), tx.next_top_index()?).with_bounds(
                    x,
                    y,
                    size.width,
                    size.height,
                );
                t.style.seed = ids.seed();
                t.frame_id = self.frame_id.cloned();
                t.group_ids.extend(groups);
                tx.insert(t)?;
                created.push(tid);
            }
            made[c] = Some((id, node_box));
        }
        let (ids, boxes): (Vec<_>, Vec<_>) =
            made.into_iter().collect::<Option<Vec<_>>>().ok_or(DiagramError::NotFinite)?.into_iter().unzip();
        Ok(ClusterSet { ids, boxes })
    }

    fn create_nodes<T: SceneTx>(
        &self,
        tx: &mut T,
        ids: &mut dyn IdSource,
        layout: &LayoutResult,
        shift: Vec2,
        sizes: &[Size],
        fonts: &[TextStyle],
    ) -> Result<NodeSet, DiagramError> {
        let n = self.spec.nodes.len();
        let mut set = NodeSet {
            ids: Vec::with_capacity(n),
            boxes: Vec::with_capacity(n),
            rects: Vec::with_capacity(n),
            created: Vec::new(),
        };
        for (i, node) in self.spec.nodes.iter().enumerate() {
            let pos = layout.positions[i] + shift;
            let size = sizes[i];
            let id = unused_element_id(tx, ids);
            let mut style = Style::default();
            if let Some(s) = &node.style {
                s.apply(&mut style);
            }
            style.seed = ids.seed();
            let label = node.label.as_deref().filter(|l| !l.trim().is_empty());
            let kind = match (node.shape, label) {
                (NodeShape::Rectangle, _) => ElementKind::Rectangle,
                (NodeShape::Ellipse, _) => ElementKind::Ellipse,
                (NodeShape::Diamond, _) => ElementKind::Diamond,
                (NodeShape::Text, text) => {
                    let mut d = TextData::new(text.unwrap_or_default());
                    d.font = fonts[i].clone();
                    d.font.align = drafftink_scene::TextAlign::Center;
                    d.auto_resize = node.width.is_none();
                    ElementKind::Text(d)
                }
            };
            let mut e = Element::new(id.clone(), kind, tx.next_top_index()?)
                .with_bounds(pos.x, pos.y, size.width, size.height)
                .with_style(style);
            e.name = node.name.clone();
            e.frame_id = self.frame_id.cloned();
            e.group_ids.extend(self.checked.item_groups[i].iter().map(|&g| self.group_ids[g].clone()));
            let nb = NodeBox::of(&e).ok_or(DiagramError::NotFinite)?;
            set.rects.push(e.rect());
            tx.insert(e)?;
            set.created.push(id.clone());
            if node.shape != NodeShape::Text
                && let Some(text) = label
            {
                let o = create_label(tx, self.m, ids, &id, text, Some(&fonts[i]))?;
                set.created.push(o.label);
                // The node may have grown if the measure and the real wrap disagree; keep the box current.
                if o.container_grew
                    && let Some(e) = tx.get(&id)
                {
                    set.rects[i] = e.rect();
                }
            }
            set.boxes.push(tx.get(&id).and_then(NodeBox::of).unwrap_or(nb));
            set.ids.push(id);
        }
        Ok(set)
    }

    /// Makes the arrows. Edges between the same two nodes share the space in lanes.
    fn create_edges<T: SceneTx>(
        &self,
        tx: &mut T,
        ids: &mut dyn IdSource,
        layout: &LayoutResult,
        shift: Vec2,
        nodes: &mut NodeSet,
        clusters: &ClusterSet,
    ) -> Result<Vec<ElementId>, DiagramError> {
        let edge_ends = &self.checked.edge_ends;
        let n = self.spec.nodes.len();
        // The box and the element of an end, which is a node or a cluster.
        let end_box = |i: usize| if i < n { nodes.boxes.get(i) } else { clusters.boxes.get(i - n) };
        let end_id = |i: usize| if i < n { nodes.ids.get(i) } else { clusters.ids.get(i - n) };
        // The nodes an end holds: itself, or everything in the cluster.
        let holds = |i: usize, node: usize| match i.checked_sub(n) {
            None => i == node,
            Some(c) => self.checked.clusters.nodes_in.get(c).is_some_and(|set| set.contains(&node)),
        };
        let mut lanes: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let mut totals: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        for &(a, b) in edge_ends {
            *totals.entry((a.min(b), a.max(b))).or_default() += 1;
        }
        let mut out = Vec::with_capacity(self.spec.edges.len());
        for (k, (e, &(a, b))) in self.spec.edges.iter().zip(edge_ends).enumerate() {
            let key = (a.min(b), a.max(b));
            let lane_slot = lanes.entry(key).or_default();
            let lane = (*lane_slot, totals.get(&key).copied().unwrap_or(1));
            *lane_slot += 1;
            let path = e.arrow_type.unwrap_or(PathType::Sharp);
            let (Some(box_a), Some(box_b)) = (end_box(a), end_box(b)) else { return Err(DiagramError::NotFinite) };
            let route = if a == b {
                route_self_loop(box_a)
            } else {
                let waypoints: Vec<Point> = layout.waypoints[k].iter().map(|p| *p + shift).collect();
                let obstacles: Vec<Rect> = if waypoints.is_empty() {
                    Vec::new()
                } else {
                    nodes
                        .rects
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !holds(a, *i) && !holds(b, *i))
                        .map(|(_, r)| *r)
                        .collect()
                };
                let p = RouteParams { path, waypoints: &waypoints, flow: layout.flow, lane, obstacles: &obstacles };
                route_edge(box_a, box_b, &p)
            };
            let id = unused_element_id(tx, ids);
            let mut arrow = arrow_element(id.clone(), tx.next_top_index()?, &route, path, heads_of(e))
                .ok_or(DiagramError::NotFinite)?;
            let mut style = Style::default();
            if let Some(c) = end_id(a).and_then(|id| tx.get(id)).map(|n| n.style.stroke_color) {
                style.stroke_color = c;
            }
            if let Some(s) = &e.style {
                s.apply(&mut style);
            }
            style.seed = ids.seed();
            arrow.style = style;
            arrow.name = e.name.clone();
            arrow.frame_id = self.frame_id.cloned();
            // The arrow joins the groups both ends share, innermost first.
            arrow.group_ids.extend(
                self.checked.item_groups[a]
                    .iter()
                    .filter(|g| self.checked.item_groups[b].contains(g))
                    .map(|&g| self.group_ids[g].clone()),
            );
            tx.insert(arrow)?;
            nodes.created.push(id.clone());
            if let Some(text) = e.label.as_deref().filter(|l| !l.trim().is_empty()) {
                let font = e.style.as_ref().map(StyleSpec::font).unwrap_or_default();
                let o = create_label(tx, self.m, ids, &id, text, Some(&font))?;
                nodes.created.push(o.label);
            }
            out.push(id);
        }
        Ok(out)
    }
}

/// Creates the diagram described by `spec` in `tx`. See the module docs.
pub fn diagram_create<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &mut dyn IdSource,
    spec: &DiagramSpec,
    opts: &CreateOpts,
) -> Result<DiagramResult, DiagramError> {
    let checked = check_spec(&*tx, spec)?;
    let (sizes, fonts) = plan_nodes(m, spec)?;
    let titles = title_sizes(m, spec);
    let (placed, shift) = place_nodes(spec, &sizes, &titles, &checked, opts.origin)?;
    let layout = &placed.layout;

    let group_ids: Vec<GroupId> = (0..checked.group_count(spec)).map(|_| ids.group_id()).collect();
    let mut groups_out: BTreeMap<String, GroupId> = BTreeMap::new();
    for (g, id) in spec.groups.iter().zip(&group_ids) {
        if let Some(sym) = &g.id {
            groups_out.insert(sym.clone(), id.clone());
        }
    }
    for (c, slot) in spec.clusters.iter().zip(&checked.cluster_group) {
        if let Some(id) = slot.and_then(|g| group_ids.get(g)) {
            groups_out.insert(c.id.clone(), id.clone());
        }
    }

    let frame_id = opts.frame.as_ref().map(|_| unused_element_id(tx, ids));
    let build = Build { m, spec, checked: &checked, group_ids: &group_ids, frame_id: frame_id.as_ref() };
    let mut created = Vec::new();
    let cluster_set = build.create_clusters(tx, ids, &placed, &titles, shift, &mut created)?;
    let mut nodes = build.create_nodes(tx, ids, layout, shift, &sizes, &fonts)?;
    created.append(&mut nodes.created);
    nodes.created = created;
    let edge_out = build.create_edges(tx, ids, layout, shift, &mut nodes, &cluster_set)?;

    let mut bounds = nodes.rects.iter().copied().reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
    for id in &nodes.created {
        if let Some(e) = tx.get(id) {
            bounds = bounds.union(e.rect());
        }
    }
    if let (Some(name), Some(fid)) = (&opts.frame, &frame_id) {
        let r = bounds.inflate(FRAME_PADDING, FRAME_PADDING);
        let data = FrameData { name: Some(name.clone()), clip: true };
        let z: ZIndex = tx.next_top_index()?;
        let mut f =
            Element::new(fid.clone(), ElementKind::Frame(data), z).with_bounds(r.x0, r.y0, r.width(), r.height());
        f.style.seed = ids.seed();
        tx.insert(f)?;
        nodes.created.push(fid.clone());
        bounds = r;
    }

    let touched: Vec<ElementId> = nodes.ids.iter().chain(&cluster_set.ids).chain(&edge_out).cloned().collect();
    update_bound_arrows(tx, &touched)?;

    let node_map = spec.nodes.iter().zip(&nodes.ids).map(|(n, id)| (n.id.clone(), id.clone())).collect();
    Ok(DiagramResult {
        nodes: node_map,
        edges: edge_out,
        groups: groups_out,
        clusters: spec.clusters.iter().map(|c| c.id.clone()).zip(cluster_set.ids).collect(),
        group_ids,
        frame: frame_id,
        bounds: [bounds.x0, bounds.y0, bounds.width(), bounds.height()],
        created: nodes.created,
    })
}
