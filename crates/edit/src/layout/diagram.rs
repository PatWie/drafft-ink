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

use super::route::{Heads, NodeBox, RouteParams, arrow_element, route_edge, route_self_loop};
use super::{Algorithm, LayoutError, LayoutGraph, LayoutOpts, LayoutResult, compute_layout, scene_bounds};
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

/// A whole diagram.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagramSpec {
    /// The nodes.
    pub nodes: Vec<NodeSpec>,
    /// The edges.
    #[serde(default)]
    pub edges: Vec<EdgeSpec>,
    /// Groups of nodes. Arrows between two members of a group join it.
    #[serde(default)]
    pub groups: Vec<GroupSpec>,
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
    /// Symbolic group id (for groups that have one) to the real group id.
    pub groups: BTreeMap<String, GroupId>,
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

/// Group membership per node (innermost first), checked to nest.
fn resolve_groups(spec: &DiagramSpec, index: &BTreeMap<&str, usize>) -> Result<Vec<Vec<usize>>, DiagramError> {
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
    Ok(per_node)
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

/// What the checks of a spec found out: the nodes each edge joins and the groups each node is in.
struct Checked {
    edge_ends: Vec<(usize, usize)>,
    node_groups: Vec<Vec<usize>>,
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
    let mut edge_ends: Vec<(usize, usize)> = Vec::with_capacity(spec.edges.len());
    for (i, e) in spec.edges.iter().enumerate() {
        let find = |id: &str| {
            index.get(id).copied().ok_or_else(|| DiagramError::UnknownNode {
                index: i,
                id: id.to_string(),
                known: known_list(spec),
            })
        };
        edge_ends.push((find(&e.from)?, find(&e.to)?));
    }
    let node_groups = resolve_groups(spec, &index)?;

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
    Ok(Checked { edge_ends, node_groups })
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

/// Places the nodes. Returns the layout and the shift that moves it to the requested origin.
fn place_nodes(
    spec: &DiagramSpec,
    sizes: &[Size],
    edge_ends: &[(usize, usize)],
    requested_origin: Point,
) -> Result<(LayoutResult, Vec2), DiagramError> {
    let fixed: Vec<Option<Point>> = spec.nodes.iter().map(|n| n.x.zip(n.y).map(|(x, y)| Point::new(x, y))).collect();
    if spec.layout.algorithm == Algorithm::None
        && let Some(i) = fixed.iter().position(Option::is_none)
    {
        return Err(DiagramError::MissingPosition(spec.nodes[i].id.clone()));
    }
    let graph = LayoutGraph { sizes: sizes.to_vec(), edges: edge_ends.to_vec(), fixed };
    let layout = compute_layout(&graph, &spec.layout).map_err(|e| match e {
        LayoutError::NotForest { node } => DiagramError::TreeNotForest(spec.nodes[node].id.clone()),
        LayoutError::BadSize { node } => DiagramError::BadSize(spec.nodes[node].id.clone()),
        LayoutError::MissingPosition { node } => DiagramError::MissingPosition(spec.nodes[node].id.clone()),
        other => DiagramError::Layout(other),
    })?;
    let origin = if spec.layout.algorithm == Algorithm::None { Point::ZERO } else { requested_origin };
    if !origin.x.is_finite() || !origin.y.is_finite() {
        return Err(DiagramError::NotFinite);
    }
    Ok((layout, origin.to_vec2()))
}

/// What the creation steps share.
struct Build<'a, M: ?Sized> {
    m: &'a M,
    spec: &'a DiagramSpec,
    node_groups: &'a [Vec<usize>],
    group_ids: &'a [GroupId],
    frame_id: Option<&'a ElementId>,
}

/// The elements made for the nodes.
struct NodeSet {
    ids: Vec<ElementId>,
    boxes: Vec<NodeBox>,
    rects: Vec<Rect>,
    created: Vec<ElementId>,
}

impl<M: TextMeasure + ?Sized> Build<'_, M> {
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
            e.group_ids.extend(self.node_groups[i].iter().map(|&g| self.group_ids[g].clone()));
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
        edge_ends: &[(usize, usize)],
        nodes: &mut NodeSet,
    ) -> Result<Vec<ElementId>, DiagramError> {
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
            let route = if a == b {
                route_self_loop(&nodes.boxes[a])
            } else {
                let waypoints: Vec<Point> = layout.waypoints[k].iter().map(|p| *p + shift).collect();
                let obstacles: Vec<Rect> = if waypoints.is_empty() {
                    Vec::new()
                } else {
                    nodes.rects.iter().enumerate().filter(|(i, _)| *i != a && *i != b).map(|(_, r)| *r).collect()
                };
                let p = RouteParams { path, waypoints: &waypoints, flow: layout.flow, lane, obstacles: &obstacles };
                route_edge(&nodes.boxes[a], &nodes.boxes[b], &p)
            };
            let id = unused_element_id(tx, ids);
            let mut arrow = arrow_element(id.clone(), tx.next_top_index()?, &route, path, heads_of(e))
                .ok_or(DiagramError::NotFinite)?;
            let mut style = Style::default();
            if let Some(c) = tx.get(&nodes.ids[a]).map(|n| n.style.stroke_color) {
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
                self.node_groups[a]
                    .iter()
                    .filter(|g| self.node_groups[b].contains(g))
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
    let Checked { edge_ends, node_groups } = check_spec(&*tx, spec)?;
    let (sizes, fonts) = plan_nodes(m, spec)?;
    let (layout, shift) = place_nodes(spec, &sizes, &edge_ends, opts.origin)?;

    let group_ids: Vec<GroupId> = spec.groups.iter().map(|_| ids.group_id()).collect();
    let groups_out: BTreeMap<String, GroupId> = spec
        .groups
        .iter()
        .zip(&group_ids)
        .filter_map(|(g, id)| g.id.as_ref().map(|sym| (sym.clone(), id.clone())))
        .collect();

    let frame_id = opts.frame.as_ref().map(|_| unused_element_id(tx, ids));
    let build = Build { m, spec, node_groups: &node_groups, group_ids: &group_ids, frame_id: frame_id.as_ref() };
    let mut nodes = build.create_nodes(tx, ids, &layout, shift, &sizes, &fonts)?;
    let edge_out = build.create_edges(tx, ids, &layout, shift, &edge_ends, &mut nodes)?;

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

    let touched: Vec<ElementId> = nodes.ids.iter().chain(&edge_out).cloned().collect();
    update_bound_arrows(tx, &touched)?;

    let node_map = spec.nodes.iter().zip(&nodes.ids).map(|(n, id)| (n.id.clone(), id.clone())).collect();
    Ok(DiagramResult {
        nodes: node_map,
        edges: edge_out,
        groups: groups_out,
        group_ids,
        frame: frame_id,
        bounds: [bounds.x0, bounds.y0, bounds.width(), bounds.height()],
        created: nodes.created,
    })
}
