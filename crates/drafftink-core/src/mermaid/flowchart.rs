//! Flowchart (`graph` / `flowchart`) parser, layered layout, and shape builder.
//!
//! Parses node declarations (with their shape brackets), `subgraph` clusters,
//! and edges (with style, optional arrowhead, and optional label), then lays the
//! graph out hierarchically: each subgraph is laid out on its own with a
//! longest-path layered algorithm, then treated as a single node in its parent's
//! layout. Cycles are handled by ignoring back edges when ranking, so cyclic
//! graphs (like a request/response pair) still lay out sensibly. Every node,
//! cluster box, and edge becomes a native, editable shape.

use super::{
    BOX_FILL, BOX_PADDING_X, BOX_PADDING_Y, LABEL_FONT_SIZE, MIN_BOX_WIDTH, base_style,
    centered_text, line_shape, measure_text, normalize_label, rect_shape,
};
use crate::shapes::{Arrow, Ellipse, Line, PathStyle, SerializableColor, Shape, StrokeStyle, Text};
use kurbo::{Point, Rect};

// -----------------------------------------------------------------------------
// Layout constants.
// -----------------------------------------------------------------------------

/// Baseline node height (pixels) before label-driven growth.
const NODE_HEIGHT: f64 = 46.0;

/// Gap (pixels) between successive layers along the flow (primary) axis.
const RANK_GAP: f64 = 84.0;

/// Gap (pixels) between sibling nodes within a layer (cross axis).
const CROSS_GAP: f64 = 44.0;

/// Corner radius (pixels) for rectangular nodes (kept sharp).
const RECT_CORNER_RADIUS: f64 = 0.0;

/// Enlargement factor for diamond nodes so the label fits inside the rhombus.
const DIAMOND_SCALE: f64 = 1.5;

/// Font size (pixels) for edge labels (slightly smaller than node labels).
const EDGE_LABEL_FONT_SIZE: f64 = 14.0;

/// Padding (pixels) between a subgraph's content and its cluster box.
const CLUSTER_PADDING: f64 = 22.0;

/// Height (pixels) of the title band at the top of a subgraph cluster box.
const CLUSTER_LABEL_BAND: f64 = 32.0;

/// Corner radius (pixels) for subgraph cluster boxes.
const CLUSTER_CORNER_RADIUS: f64 = 8.0;

/// Light slate fill for subgraph cluster boxes (so nested nodes stay legible).
const CLUSTER_FILL: SerializableColor = SerializableColor {
    r: 241,
    g: 245,
    b: 249,
    a: 255,
};

/// Characters that make up a link connector run.
const LINK_CORE: [char; 5] = ['-', '.', '=', '<', '>'];

/// Characters that open or close a node shape bracket.
const BRACKET_CHARS: [char; 6] = ['[', '(', '{', ']', ')', '}'];

// -----------------------------------------------------------------------------
// Parsed model.
// -----------------------------------------------------------------------------

/// The visual shape requested for a node via its bracket syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeShape {
    Rectangle,
    Rounded,
    Stadium,
    Subroutine,
    Circle,
    Diamond,
    Hexagon,
}

/// A flowchart node. A node may also be a `subgraph` cluster, in which case it
/// contains other nodes and is drawn as a labeled container box.
struct Node {
    /// The node's Mermaid identifier (used for de-duplication and lookups).
    id: String,
    label: String,
    shape: NodeShape,
    /// Whether an explicit shape/label has been seen (so later bare references
    /// do not overwrite a richer definition).
    explicit: bool,
    /// The subgraph this node belongs to, or `None` for a top-level node.
    parent: Option<usize>,
    /// Whether this node is a subgraph cluster.
    is_cluster: bool,
    /// Flow direction for a cluster's own contents.
    direction: Direction,
}

/// Stroke rendering for an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkStyle {
    Solid,
    Dotted,
    Thick,
}

/// A parsed link connector between two nodes.
struct Link {
    style: LinkStyle,
    arrow: bool,
    label: Option<String>,
}

/// A directed edge between node indices with its connector styling.
struct Edge {
    from: usize,
    to: usize,
    link: Link,
}

/// Flow direction from the header (`LR`, `RL`, `TB`/`TD`, `BT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    LeftRight,
    RightLeft,
    TopBottom,
    BottomTop,
}

impl Direction {
    /// Whether this direction flows along the vertical axis.
    fn is_vertical(self) -> bool {
        matches!(self, Direction::TopBottom | Direction::BottomTop)
    }
}

/// The fully parsed flowchart.
struct Flowchart {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    direction: Direction,
}

// -----------------------------------------------------------------------------
// Public entry.
// -----------------------------------------------------------------------------

/// Parse `body` and build the shapes for a flowchart. Returns an empty vector if
/// there is nothing drawable.
pub(super) fn build(body: &str) -> Vec<Shape> {
    let chart = parse(body);
    if chart.nodes.is_empty() {
        return Vec::new();
    }
    layout(&chart)
}

// -----------------------------------------------------------------------------
// Parsing.
// -----------------------------------------------------------------------------

/// Keywords that introduce lines carrying no drawable geometry (styling, etc.).
const SKIP_KEYWORDS: &[&str] = &["style", "classDef", "class", "click", "linkStyle"];

/// Parse the flowchart body into nodes, subgraph clusters, edges, and a
/// top-level flow direction.
fn parse(body: &str) -> Flowchart {
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut direction = Direction::TopBottom;
    // The stack of open subgraph clusters; the last entry is the current scope.
    let mut cluster_stack: Vec<usize> = Vec::new();

    let mut lines = body.lines();
    if let Some(header) = lines.next() {
        direction = parse_direction(header);
    }

    for raw in lines {
        for statement in strip_inline_comment(raw).split(';') {
            let line = statement.trim();
            if line.is_empty() {
                continue;
            }
            parse_statement(line, &mut nodes, &mut edges, &mut cluster_stack, direction);
        }
    }

    Flowchart {
        nodes,
        edges,
        direction,
    }
}

/// Dispatch a single statement to the appropriate parser, maintaining the
/// current subgraph scope.
fn parse_statement(
    line: &str,
    nodes: &mut Vec<Node>,
    edges: &mut Vec<Edge>,
    cluster_stack: &mut Vec<usize>,
    root_direction: Direction,
) {
    let current = cluster_stack.last().copied();

    if let Some(rest) = keyword(line, "subgraph") {
        let idx = open_subgraph(rest, nodes, current, root_direction);
        cluster_stack.push(idx);
    } else if line == "end" {
        cluster_stack.pop();
    } else if let Some(rest) = keyword(line, "direction") {
        if let Some(&c) = cluster_stack.last() {
            nodes[c].direction = parse_direction(rest);
        }
    } else if !is_skippable(line) {
        parse_chain(line, nodes, edges, current);
    }
}

/// If `line` begins with `word` on a word boundary, return the trimmed
/// remainder.
fn keyword<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(word)?;
    if rest.is_empty() {
        Some(rest)
    } else if rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// Register (or upgrade) a subgraph cluster from its header remainder, which is
/// a node token such as `STACK["The stack"]` or a bare id `REAL`.
fn open_subgraph(
    rest: &str,
    nodes: &mut Vec<Node>,
    parent: Option<usize>,
    root_direction: Direction,
) -> usize {
    let chars: Vec<char> = rest.chars().collect();
    let parsed = parse_node(&chars, 0)
        .map(|(node, _)| node)
        .unwrap_or(ParsedNode {
            id: rest.trim().to_string(),
            label: None,
            shape: None,
        });

    let idx = intern_node(nodes, parsed, parent);
    nodes[idx].is_cluster = true;
    // A cluster's declared parent is the scope it is nested within.
    nodes[idx].parent = parent;
    // Default to the top-level direction; a following `direction` line (which is
    // parsed after this one) overrides it.
    nodes[idx].direction = root_direction;
    idx
}

/// Determine the flow direction declared on a header or `direction` line.
fn parse_direction(text: &str) -> Direction {
    let upper = text.to_ascii_uppercase();
    for (token, dir) in [
        ("LR", Direction::LeftRight),
        ("RL", Direction::RightLeft),
        ("TB", Direction::TopBottom),
        ("TD", Direction::TopBottom),
        ("BT", Direction::BottomTop),
    ] {
        if upper.split_whitespace().any(|w| w == token) {
            return dir;
        }
    }
    Direction::TopBottom
}

/// Strip a trailing Mermaid `%% ...` comment.
fn strip_inline_comment(line: &str) -> &str {
    match line.find("%%") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// Whether a line should be ignored (styling directives, etc.).
fn is_skippable(line: &str) -> bool {
    SKIP_KEYWORDS.iter().any(|kw| keyword(line, kw).is_some())
}

/// Parse one statement into a chain of nodes joined by links, recording edges.
/// Newly created nodes are assigned to the current subgraph scope.
fn parse_chain(line: &str, nodes: &mut Vec<Node>, edges: &mut Vec<Edge>, parent: Option<usize>) {
    let chars: Vec<char> = line.chars().collect();
    let mut cursor = skip_ws(&chars, 0);

    let Some((first, next)) = parse_node(&chars, cursor) else {
        return;
    };
    let mut prev = intern_node(nodes, first, parent);
    cursor = next;

    loop {
        cursor = skip_ws(&chars, cursor);
        let Some((link, after_link)) = parse_link(&chars, cursor) else {
            break;
        };
        cursor = skip_ws(&chars, after_link);
        let Some((node, after_node)) = parse_node(&chars, cursor) else {
            break;
        };
        let current = intern_node(nodes, node, parent);
        cursor = after_node;
        edges.push(Edge {
            from: prev,
            to: current,
            link,
        });
        prev = current;
    }
}

/// A parsed node token: identifier plus optional explicit label/shape.
struct ParsedNode {
    id: String,
    label: Option<String>,
    shape: Option<NodeShape>,
}

/// Find or create a node by id. An explicit label/shape upgrades a node that
/// was previously only referenced. A brand-new node is assigned to `parent`.
/// Returns the node's index.
fn intern_node(nodes: &mut Vec<Node>, parsed: ParsedNode, parent: Option<usize>) -> usize {
    if let Some(idx) = nodes.iter().position(|n| n.id == parsed.id) {
        if let (Some(shape), false) = (parsed.shape, nodes[idx].explicit) {
            nodes[idx].shape = shape;
            nodes[idx].label = parsed.label.unwrap_or_else(|| parsed.id.clone());
            nodes[idx].explicit = true;
        }
        return idx;
    }
    nodes.push(Node {
        id: parsed.id.clone(),
        label: parsed.label.unwrap_or(parsed.id),
        shape: parsed.shape.unwrap_or(NodeShape::Rectangle),
        explicit: parsed.shape.is_some(),
        parent,
        is_cluster: false,
        direction: Direction::TopBottom,
    });
    nodes.len() - 1
}

/// Skip whitespace starting at `start`, returning the next non-space index.
fn skip_ws(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// Whether `c` may appear in a node identifier.
fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Parse a node token at `start`. Returns the parsed node and the index just
/// past it, or `None` if no identifier is present.
fn parse_node(chars: &[char], start: usize) -> Option<(ParsedNode, usize)> {
    let mut i = start;
    while i < chars.len() && is_id_char(chars[i]) {
        i += 1;
    }
    if i == start {
        return None;
    }
    let id: String = chars[start..i].iter().collect();

    if let Some((open, close, shape)) = shape_delimiters(chars, i) {
        let content_start = i + open.len();
        if let Some(close_at) = find_subsequence(chars, content_start, &close) {
            let label: String = chars[content_start..close_at].iter().collect();
            let end = close_at + close.len();
            return Some((
                ParsedNode {
                    id,
                    label: Some(normalize_label(&label)),
                    shape: Some(shape),
                },
                end,
            ));
        }
    }

    Some((
        ParsedNode {
            id,
            label: None,
            shape: None,
        },
        i,
    ))
}

/// Identify a shape bracket opening at `i`, returning `(open, close, shape)`.
/// Two-character delimiters are tested before single-character ones.
fn shape_delimiters(chars: &[char], i: usize) -> Option<(Vec<char>, Vec<char>, NodeShape)> {
    let two = |a: char, b: char| chars.get(i) == Some(&a) && chars.get(i + 1) == Some(&b);
    if two('(', '(') {
        return Some((vec!['(', '('], vec![')', ')'], NodeShape::Circle));
    }
    if two('(', '[') {
        return Some((vec!['(', '['], vec![']', ')'], NodeShape::Stadium));
    }
    if two('[', '[') {
        return Some((vec!['[', '['], vec![']', ']'], NodeShape::Subroutine));
    }
    if two('{', '{') {
        return Some((vec!['{', '{'], vec!['}', '}'], NodeShape::Hexagon));
    }
    match chars.get(i) {
        Some('(') => Some((vec!['('], vec![')'], NodeShape::Rounded)),
        Some('[') => Some((vec!['['], vec![']'], NodeShape::Rectangle)),
        Some('{') => Some((vec!['{'], vec!['}'], NodeShape::Diamond)),
        _ => None,
    }
}

/// Find the next occurrence of `needle` within `chars` at or after `from`.
fn find_subsequence(chars: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > chars.len() {
        return None;
    }
    (from..=chars.len() - needle.len()).find(|&i| chars[i..i + needle.len()] == *needle)
}

/// Parse a link connector at `start`. Returns the link and the index just past
/// it, or `None` if no connector is present.
///
/// Handles plain connectors (`-->`, `---`, `-.->`, `==>`), pipe labels
/// (`-->|text|`), and embedded labels (`-- text -->`, `-- "quoted" -->`).
fn parse_link(chars: &[char], start: usize) -> Option<(Link, usize)> {
    if start >= chars.len() || !LINK_CORE.contains(&chars[start]) {
        return None;
    }

    let mut i = start;
    while i < chars.len() && LINK_CORE.contains(&chars[i]) {
        i += 1;
    }
    let run1: String = chars[start..i].iter().collect();
    let mut arrow = run1.contains('>') || run1.contains('<');
    i = consume_cross_arrowhead(chars, i, &mut arrow);
    let style = run_style(&run1);
    let after_run1 = i;

    // Pipe-delimited label: `-->|text|`.
    let probe = skip_ws(chars, i);
    if chars.get(probe) == Some(&'|') {
        if let Some((label, end)) = read_pipe_label(chars, probe) {
            return Some((
                Link {
                    style,
                    arrow,
                    label: Some(label),
                },
                end,
            ));
        }
    }

    // Embedded label: `-- text -->`. Only when the first run had no arrowhead,
    // and only if another connector run follows the candidate label.
    if !arrow && is_label_start(chars.get(probe)) {
        if let Some((link, end)) = try_embedded_label(chars, probe, style) {
            return Some((link, end));
        }
    }

    Some((
        Link {
            style,
            arrow,
            label: None,
        },
        after_run1,
    ))
}

/// Consume a trailing `x`/`o` arrowhead (as in `--x`, `--o`) when it is at a
/// token boundary, marking `arrow` accordingly. Returns the new index.
fn consume_cross_arrowhead(chars: &[char], i: usize, arrow: &mut bool) -> usize {
    let Some(&c) = chars.get(i) else { return i };
    if (c == 'x' || c == 'o') && chars.get(i + 1).is_none_or(|n| n.is_whitespace()) {
        *arrow = true;
        return i + 1;
    }
    i
}

/// Whether an embedded (unquoted or quoted) label could start at `c`.
fn is_label_start(c: Option<&char>) -> bool {
    matches!(c, Some('"')) || c.is_some_and(|c| c.is_alphanumeric())
}

/// Read a `|...|` label starting at the opening pipe. Returns the label and the
/// index past the closing pipe.
fn read_pipe_label(chars: &[char], open: usize) -> Option<(String, usize)> {
    let start = open + 1;
    let close = find_subsequence(chars, start, &['|'])?;
    let label: String = chars[start..close].iter().collect();
    Some((normalize_label(&label), close + 1))
}

/// Try to interpret text at `label_start` as an embedded link label that is
/// followed by a closing connector run. Returns the completed link on success.
///
/// The label reader stops at connector characters and at node-shape brackets,
/// so a plain link into a shaped node (e.g. `A --- B{{x}}`) is not mistaken for
/// a labeled link.
fn try_embedded_label(
    chars: &[char],
    label_start: usize,
    style_so_far: LinkStyle,
) -> Option<(Link, usize)> {
    let (label, after_label) = read_embedded_label(chars, label_start);
    if label.is_empty() {
        return None;
    }

    let run2_start = skip_ws(chars, after_label);
    if run2_start >= chars.len() || !LINK_CORE.contains(&chars[run2_start]) {
        return None; // No closing run: the "label" was actually the next node.
    }
    let mut j = run2_start;
    while j < chars.len() && LINK_CORE.contains(&chars[j]) {
        j += 1;
    }
    let run2: String = chars[run2_start..j].iter().collect();
    let mut arrow = run2.contains('>') || run2.contains('<');
    j = consume_cross_arrowhead(chars, j, &mut arrow);

    let style = merge_style(style_so_far, &run2);
    Some((
        Link {
            style,
            arrow,
            label: Some(label),
        },
        j,
    ))
}

/// Read an embedded label (quoted or bare) at `start`, returning the trimmed
/// label text and the index just past it. A bare label stops at a connector
/// character or a node-shape bracket.
fn read_embedded_label(chars: &[char], start: usize) -> (String, usize) {
    if chars.get(start) == Some(&'"') {
        let content_start = start + 1;
        if let Some(close) = find_subsequence(chars, content_start, &['"']) {
            let label: String = chars[content_start..close].iter().collect();
            return (normalize_label(&label), close + 1);
        }
    }
    let mut i = start;
    while i < chars.len()
        && !LINK_CORE.contains(&chars[i])
        && chars[i] != '|'
        && !BRACKET_CHARS.contains(&chars[i])
    {
        i += 1;
    }
    let label: String = chars[start..i].iter().collect();
    (normalize_label(label.trim()), i)
}

/// Determine a link style from a single connector run.
fn run_style(run: &str) -> LinkStyle {
    if run.contains('.') {
        LinkStyle::Dotted
    } else if run.contains('=') {
        LinkStyle::Thick
    } else {
        LinkStyle::Solid
    }
}

/// Merge a running style with a second connector run's style, preferring the
/// more specific (dotted/thick) style if either run declares it.
fn merge_style(current: LinkStyle, run2: &str) -> LinkStyle {
    match run_style(run2) {
        LinkStyle::Solid => current,
        other => other,
    }
}

// -----------------------------------------------------------------------------
// Layout.
// -----------------------------------------------------------------------------

/// Geometry computed for a node during layout.
struct Placed {
    center: Point,
    half_w: f64,
    half_h: f64,
    shape: NodeShape,
    is_cluster: bool,
}

/// Full layout state, computed hierarchically from the innermost clusters out.
struct LayoutState {
    /// Absolute center of each node.
    abs: Vec<Point>,
    /// Box size of each node (clusters include padding and title band).
    size: Vec<(f64, f64)>,
    /// Center of each node's box relative to its parent's content frame.
    rel: Vec<Point>,
    /// For a cluster, the center of its box within its own children's frame.
    content_center: Vec<Point>,
}

/// Turn a parsed flowchart into positioned shapes: cluster boxes first (behind),
/// then edges, then leaf nodes and labels on top.
fn layout(chart: &Flowchart) -> Vec<Shape> {
    let children = child_lists(chart);
    let mut state = LayoutState {
        abs: vec![Point::ZERO; chart.nodes.len()],
        size: vec![(0.0, 0.0); chart.nodes.len()],
        rel: vec![Point::ZERO; chart.nodes.len()],
        content_center: vec![Point::ZERO; chart.nodes.len()],
    };

    // Bottom-up: size and relatively place every scope, root last.
    measure_scope(chart, None, &children, &mut state);
    // Top-down: resolve absolute centers. Root children live in absolute space.
    for &root_child in &children.root {
        state.abs[root_child] = state.rel[root_child];
        assign_absolute(root_child, chart, &children, &mut state);
    }

    emit_shapes(chart, &state)
}

/// Direct-child lists for every scope (each cluster plus the root).
struct ChildLists {
    per_node: Vec<Vec<usize>>,
    root: Vec<usize>,
}

/// Build direct-child lists keyed by parent cluster (and a root list).
fn child_lists(chart: &Flowchart) -> ChildLists {
    let mut per_node = vec![Vec::new(); chart.nodes.len()];
    let mut root = Vec::new();
    for (i, node) in chart.nodes.iter().enumerate() {
        match node.parent {
            Some(p) => per_node[p].push(i),
            None => root.push(i),
        }
    }
    ChildLists { per_node, root }
}

/// Size and relatively place the direct children of `scope` (a cluster index,
/// or `None` for the root). Recurses into nested clusters first.
fn measure_scope(
    chart: &Flowchart,
    scope: Option<usize>,
    children: &ChildLists,
    state: &mut LayoutState,
) {
    let direct: &[usize] = match scope {
        Some(c) => &children.per_node[c],
        None => &children.root,
    };

    // Ensure each child's own size is known (recursing into clusters).
    for &ch in direct {
        if chart.nodes[ch].is_cluster {
            measure_scope(chart, Some(ch), children, state);
        } else {
            state.size[ch] = node_size(&chart.nodes[ch].label, chart.nodes[ch].shape);
        }
    }

    if direct.is_empty() {
        if let Some(c) = scope {
            state.size[c] = (MIN_BOX_WIDTH, NODE_HEIGHT + CLUSTER_LABEL_BAND);
            state.content_center[c] = Point::ZERO;
        }
        return;
    }

    // Place the direct children with a layered layout in this scope's direction.
    let direction = scope.map_or(chart.direction, |c| chart.nodes[c].direction);
    let local_edges = internal_edges(chart, scope);
    let local_sizes: Vec<(f64, f64)> = direct.iter().map(|&i| state.size[i]).collect();
    let centers = place_layer(direct.len(), &local_edges, &local_sizes, direction);
    for (k, &ch) in direct.iter().enumerate() {
        state.rel[ch] = centers[k];
    }

    finalize_scope(chart, scope, direct, state);
}

/// Compute a scope's bounding box and, for a cluster, its box size and the
/// center of that box within the children's coordinate frame.
fn finalize_scope(
    chart: &Flowchart,
    scope: Option<usize>,
    direct: &[usize],
    state: &mut LayoutState,
) {
    let mut bbox = Rect::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &ch in direct {
        let c = state.rel[ch];
        let (w, h) = state.size[ch];
        bbox = bbox.union(Rect::new(
            c.x - w / 2.0,
            c.y - h / 2.0,
            c.x + w / 2.0,
            c.y + h / 2.0,
        ));
    }

    let Some(cluster) = scope else {
        return; // Root needs no box; children already live in this frame.
    };

    // Grow the box to hold both the content and the title label.
    let (label_w, _) = measure_text(&chart.nodes[cluster].label, LABEL_FONT_SIZE);
    let content_cx = (bbox.x0 + bbox.x1) / 2.0;
    let half_w = (bbox.width() / 2.0 + CLUSTER_PADDING).max(label_w / 2.0 + CLUSTER_PADDING);
    let x0 = content_cx - half_w;
    let x1 = content_cx + half_w;
    let y0 = bbox.y0 - CLUSTER_PADDING - CLUSTER_LABEL_BAND;
    let y1 = bbox.y1 + CLUSTER_PADDING;

    state.size[cluster] = (x1 - x0, y1 - y0);
    state.content_center[cluster] = Point::new((x0 + x1) / 2.0, (y0 + y1) / 2.0);
}

/// Resolve absolute centers for a cluster's descendants once the cluster's own
/// absolute center is known.
fn assign_absolute(
    cluster: usize,
    chart: &Flowchart,
    children: &ChildLists,
    state: &mut LayoutState,
) {
    if !chart.nodes[cluster].is_cluster {
        return;
    }
    let base = state.abs[cluster];
    let offset = state.content_center[cluster];
    for &ch in &children.per_node[cluster] {
        state.abs[ch] = Point::new(
            base.x + state.rel[ch].x - offset.x,
            base.y + state.rel[ch].y - offset.y,
        );
        assign_absolute(ch, chart, children, state);
    }
}

/// Collect edges that connect two distinct direct children of `scope`, mapping
/// each endpoint up to the direct child that contains it. These drive the
/// layered ranking within the scope.
fn internal_edges(chart: &Flowchart, scope: Option<usize>) -> Vec<(usize, usize)> {
    // Direct children and a lookup from node -> its local index in `scope`.
    let direct: Vec<usize> = chart
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent == scope)
        .map(|(i, _)| i)
        .collect();
    let local_of = |node: usize| direct.iter().position(|&d| d == node);

    let mut edges = Vec::new();
    for edge in &chart.edges {
        let (Some(from), Some(to)) = (
            ancestor_in_scope(chart, edge.from, scope),
            ancestor_in_scope(chart, edge.to, scope),
        ) else {
            continue;
        };
        if from == to {
            continue;
        }
        if let (Some(a), Some(b)) = (local_of(from), local_of(to)) {
            edges.push((a, b));
        }
    }
    edges
}

/// Climb from `node` to the ancestor that is a direct child of `scope`, or
/// `None` if `node` is not inside `scope`.
fn ancestor_in_scope(chart: &Flowchart, node: usize, scope: Option<usize>) -> Option<usize> {
    let mut cur = node;
    loop {
        if chart.nodes[cur].parent == scope {
            return Some(cur);
        }
        cur = chart.nodes[cur].parent?;
    }
}

/// Lay out `count` nodes into layers using longest-path ranking, returning each
/// node's center relative to the layer group's centroid (centered at the
/// origin on both axes).
fn place_layer(
    count: usize,
    edges: &[(usize, usize)],
    sizes: &[(f64, f64)],
    direction: Direction,
) -> Vec<Point> {
    let vertical = direction.is_vertical();
    let primary_size = |i: usize| if vertical { sizes[i].1 } else { sizes[i].0 };
    let cross_size = |i: usize| if vertical { sizes[i].0 } else { sizes[i].1 };

    let ranks = longest_path_ranks(count, edges);
    let max_rank = ranks.iter().copied().max().unwrap_or(0);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    for (i, &r) in ranks.iter().enumerate() {
        members[r].push(i);
    }

    // Primary center of each layer, then shift so the whole run is centered.
    let mut rank_center = vec![0.0; max_rank + 1];
    let mut cursor = 0.0;
    for (r, layer) in members.iter().enumerate() {
        let extent = layer
            .iter()
            .map(|&i| primary_size(i))
            .fold(0.0_f64, f64::max);
        rank_center[r] = cursor + extent / 2.0;
        cursor += extent + RANK_GAP;
    }
    let primary_total = (cursor - RANK_GAP).max(0.0);

    let mut centers = vec![Point::ZERO; count];
    for (r, layer) in members.iter().enumerate() {
        let total_cross: f64 = layer.iter().map(|&i| cross_size(i)).sum::<f64>()
            + CROSS_GAP * layer.len().saturating_sub(1) as f64;
        let mut cross_cursor = -total_cross / 2.0;
        let primary = orient_primary(direction, rank_center[r] - primary_total / 2.0);
        for &i in layer {
            let cross_center = cross_cursor + cross_size(i) / 2.0;
            cross_cursor += cross_size(i) + CROSS_GAP;
            centers[i] = if vertical {
                Point::new(cross_center, primary)
            } else {
                Point::new(primary, cross_center)
            };
        }
    }
    centers
}

/// Mirror the primary coordinate for reversed directions (RL/BT).
fn orient_primary(direction: Direction, primary: f64) -> f64 {
    match direction {
        Direction::LeftRight | Direction::TopBottom => primary,
        Direction::RightLeft | Direction::BottomTop => -primary,
    }
}

/// Assign a layer index to each node using longest-path ranking on the acyclic
/// subgraph obtained by dropping back edges and self loops.
fn longest_path_ranks(count: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let back = back_edges(count, edges);

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut indegree = vec![0usize; count];
    for (idx, &(from, to)) in edges.iter().enumerate() {
        if back[idx] || from == to {
            continue;
        }
        adj[from].push(to);
        indegree[to] += 1;
    }

    let mut rank = vec![0usize; count];
    let mut queue: Vec<usize> = (0..count).filter(|&v| indegree[v] == 0).collect();
    let mut head = 0;
    while head < queue.len() {
        let u = queue[head];
        head += 1;
        for &v in &adj[u] {
            rank[v] = rank[v].max(rank[u] + 1);
            indegree[v] -= 1;
            if indegree[v] == 0 {
                queue.push(v);
            }
        }
    }
    rank
}

/// Detect back edges (edges to an ancestor on the DFS stack) with an iterative
/// DFS so deeply nested input cannot overflow the stack.
fn back_edges(count: usize, edges: &[(usize, usize)]) -> Vec<bool> {
    const WHITE: u8 = 0;
    const GRAY: u8 = 1;
    const BLACK: u8 = 2;

    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); count];
    for (idx, &(from, to)) in edges.iter().enumerate() {
        adj[from].push((idx, to));
    }

    let mut color = vec![WHITE; count];
    let mut is_back = vec![false; edges.len()];
    for start in 0..count {
        if color[start] != WHITE {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        color[start] = GRAY;
        while let Some(&(u, child)) = stack.last() {
            if child < adj[u].len() {
                stack.last_mut().unwrap().1 += 1;
                let (edge_idx, v) = adj[u][child];
                match color[v] {
                    GRAY => is_back[edge_idx] = true,
                    WHITE => {
                        color[v] = GRAY;
                        stack.push((v, 0));
                    }
                    _ => {}
                }
            } else {
                color[u] = BLACK;
                stack.pop();
            }
        }
    }
    is_back
}

/// Compute the `(width, height)` a node needs to hold its label in its shape.
fn node_size(label: &str, shape: NodeShape) -> (f64, f64) {
    let (tw, th) = measure_text(label, LABEL_FONT_SIZE);
    let base_w = (tw + 2.0 * BOX_PADDING_X).max(MIN_BOX_WIDTH);
    let base_h = (th + 2.0 * BOX_PADDING_Y).max(NODE_HEIGHT);
    match shape {
        NodeShape::Circle => {
            let d = base_w.max(base_h);
            (d, d)
        }
        NodeShape::Diamond => (base_w * DIAMOND_SCALE, base_h * DIAMOND_SCALE),
        NodeShape::Stadium | NodeShape::Hexagon => (base_w + base_h, base_h),
        NodeShape::Rectangle | NodeShape::Rounded | NodeShape::Subroutine => (base_w, base_h),
    }
}

// -----------------------------------------------------------------------------
// Shape building.
// -----------------------------------------------------------------------------

/// Emit all shapes in draw order: cluster boxes (outermost first), edges, then
/// leaf nodes and their labels.
fn emit_shapes(chart: &Flowchart, state: &LayoutState) -> Vec<Shape> {
    let placed: Vec<Placed> = (0..chart.nodes.len())
        .map(|i| Placed {
            center: state.abs[i],
            half_w: state.size[i].0 / 2.0,
            half_h: state.size[i].1 / 2.0,
            shape: chart.nodes[i].shape,
            is_cluster: chart.nodes[i].is_cluster,
        })
        .collect();

    let mut shapes: Vec<Shape> = Vec::new();

    // Cluster boxes, shallow (outer) first so nesting stacks correctly.
    let mut clusters: Vec<usize> = (0..chart.nodes.len())
        .filter(|&i| chart.nodes[i].is_cluster)
        .collect();
    clusters.sort_by_key(|&i| depth(chart, i));
    for c in clusters {
        build_cluster(&chart.nodes[c], &placed[c], &mut shapes);
    }

    for edge in &chart.edges {
        build_edge(edge, &placed, &mut shapes);
    }

    for (i, node) in chart.nodes.iter().enumerate() {
        if !node.is_cluster {
            build_node(node, &placed[i], &mut shapes);
        }
    }

    shapes
}

/// Number of ancestor clusters above a node (its nesting depth).
fn depth(chart: &Flowchart, node: usize) -> usize {
    let mut d = 0;
    let mut cur = node;
    while let Some(p) = chart.nodes[cur].parent {
        d += 1;
        cur = p;
    }
    d
}

/// Emit a subgraph cluster box and its title label.
fn build_cluster(node: &Node, placed: &Placed, out: &mut Vec<Shape>) {
    let left = placed.center.x - placed.half_w;
    let top = placed.center.y - placed.half_h;
    out.push(Shape::Rectangle(rect_shape(
        left,
        top,
        placed.half_w * 2.0,
        placed.half_h * 2.0,
        CLUSTER_CORNER_RADIUS,
        Some(CLUSTER_FILL),
    )));
    // Title sits centered within the top band.
    let title_center = Point::new(placed.center.x, top + CLUSTER_LABEL_BAND / 2.0);
    out.push(Shape::Text(centered_text(title_center, &node.label)));
}

/// Emit the outline shape(s) and label for one leaf node.
fn build_node(node: &Node, placed: &Placed, out: &mut Vec<Shape>) {
    let c = placed.center;
    let (w, h) = (placed.half_w * 2.0, placed.half_h * 2.0);
    let left = c.x - placed.half_w;
    let top = c.y - placed.half_h;

    match node.shape {
        NodeShape::Circle => {
            let mut ellipse = Ellipse::circle(c, placed.half_w);
            ellipse.style = base_style(Some(BOX_FILL));
            out.push(Shape::Ellipse(ellipse));
        }
        NodeShape::Diamond => out.push(Shape::Line(polygon(diamond_points(c, placed)))),
        NodeShape::Hexagon => out.push(Shape::Line(polygon(hexagon_points(c, placed)))),
        NodeShape::Rounded | NodeShape::Stadium => {
            let radius = if node.shape == NodeShape::Stadium {
                placed.half_h
            } else {
                (placed.half_h / 2.0).min(16.0)
            };
            out.push(Shape::Rectangle(rect_shape(
                left,
                top,
                w,
                h,
                radius,
                Some(BOX_FILL),
            )));
        }
        NodeShape::Rectangle | NodeShape::Subroutine => {
            out.push(Shape::Rectangle(rect_shape(
                left,
                top,
                w,
                h,
                RECT_CORNER_RADIUS,
                Some(BOX_FILL),
            )));
            if node.shape == NodeShape::Subroutine {
                let inset = (w * 0.08).min(10.0);
                out.push(Shape::Line(line_shape(
                    Point::new(left + inset, top),
                    Point::new(left + inset, top + h),
                    StrokeStyle::Solid,
                )));
                out.push(Shape::Line(line_shape(
                    Point::new(left + w - inset, top),
                    Point::new(left + w - inset, top + h),
                    StrokeStyle::Solid,
                )));
            }
        }
    }

    out.push(Shape::Text(centered_text(c, &node.label)));
}

/// The four rhombus vertices of a diamond node (top, right, bottom, left).
fn diamond_points(c: Point, placed: &Placed) -> Vec<Point> {
    vec![
        Point::new(c.x, c.y - placed.half_h),
        Point::new(c.x + placed.half_w, c.y),
        Point::new(c.x, c.y + placed.half_h),
        Point::new(c.x - placed.half_w, c.y),
    ]
}

/// The six vertices of a horizontal hexagon node.
fn hexagon_points(c: Point, placed: &Placed) -> Vec<Point> {
    let inset = (placed.half_h).min(placed.half_w);
    vec![
        Point::new(c.x - placed.half_w + inset, c.y - placed.half_h),
        Point::new(c.x + placed.half_w - inset, c.y - placed.half_h),
        Point::new(c.x + placed.half_w, c.y),
        Point::new(c.x + placed.half_w - inset, c.y + placed.half_h),
        Point::new(c.x - placed.half_w + inset, c.y + placed.half_h),
        Point::new(c.x - placed.half_w, c.y),
    ]
}

/// Build a closed, white-filled polygon line from vertices.
fn polygon(points: Vec<Point>) -> Line {
    let mut line = Line::from_points(points, PathStyle::Direct);
    line.closed = true;
    line.style = base_style(Some(BOX_FILL));
    line
}

/// Emit the connector (arrow or line) and optional label for one edge.
fn build_edge(edge: &Edge, placed: &[Placed], out: &mut Vec<Shape>) {
    if edge.from == edge.to {
        build_self_edge(&placed[edge.from], &edge.link, out);
        return;
    }

    let source = &placed[edge.from];
    let target = &placed[edge.to];
    let start = border_point(source, target.center);
    let end = border_point(target, source.center);

    let stroke = stroke_for(edge.link.style);
    if edge.link.arrow {
        let mut arrow = Arrow::from_points(vec![start, end], PathStyle::Direct);
        arrow.stroke_style = stroke;
        arrow.style = base_style(None);
        out.push(Shape::Arrow(arrow));
    } else {
        out.push(Shape::Line(line_shape(start, end, stroke)));
    }

    if let Some(label) = &edge.link.label {
        build_edge_label(label, start, end, out);
    }
}

/// Emit a small loop connector for a self-referential edge.
fn build_self_edge(node: &Placed, link: &Link, out: &mut Vec<Shape>) {
    let c = node.center;
    let loop_w = node.half_w * 0.8;
    let top = c.y - node.half_h;
    let points = vec![
        Point::new(c.x + node.half_w * 0.4, top),
        Point::new(c.x + node.half_w * 0.4 + loop_w, top - node.half_h),
        Point::new(c.x + node.half_w * 0.4 + loop_w, top - node.half_h * 0.2),
        Point::new(c.x + node.half_w * 0.6, top),
    ];
    let stroke = stroke_for(link.style);
    if link.arrow {
        let mut arrow = Arrow::from_points(points, PathStyle::Direct);
        arrow.stroke_style = stroke;
        arrow.style = base_style(None);
        out.push(Shape::Arrow(arrow));
    } else {
        let mut line = Line::from_points(points, PathStyle::Direct);
        line.stroke_style = stroke;
        line.style = base_style(None);
        out.push(Shape::Line(line));
    }
    if let Some(label) = &link.label {
        let center = Point::new(c.x + node.half_w * 0.4 + loop_w, top - node.half_h);
        out.push(edge_label_text(center, label));
    }
}

/// Emit an edge label with an opaque background so it stays legible over lines.
fn build_edge_label(label: &str, start: Point, end: Point, out: &mut Vec<Shape>) {
    let mid = Point::new((start.x + end.x) / 2.0, (start.y + end.y) / 2.0);
    let content = normalize_label(label);
    let (w, h) = measure_text(&content, EDGE_LABEL_FONT_SIZE);
    let pad = 4.0;
    let mut bg = rect_shape(
        mid.x - w / 2.0 - pad,
        mid.y - h / 2.0 - pad,
        w + 2.0 * pad,
        h + 2.0 * pad,
        2.0,
        Some(BOX_FILL),
    );
    bg.style.stroke_color = SerializableColor::transparent();
    out.push(Shape::Rectangle(bg));
    out.push(edge_label_text(mid, &content));
}

/// Build a small centered text shape for an edge label.
fn edge_label_text(center: Point, label: &str) -> Shape {
    let content = normalize_label(label);
    let (w, h) = measure_text(&content, EDGE_LABEL_FONT_SIZE);
    let position = Point::new(center.x - w / 2.0, center.y - h / 2.0);
    let mut text = Text::new(position, content).with_font_size(EDGE_LABEL_FONT_SIZE);
    text.style = base_style(None);
    Shape::Text(text)
}

/// Map a link style to the corresponding stroke style.
fn stroke_for(style: LinkStyle) -> StrokeStyle {
    match style {
        LinkStyle::Solid | LinkStyle::Thick => StrokeStyle::Solid,
        LinkStyle::Dotted => StrokeStyle::Dotted,
    }
}

/// Compute the point where the segment from a node's center toward `toward`
/// exits the node's outline (bounding box, or radius for a circle, or rhombus
/// for a diamond). Clusters use their box border.
fn border_point(node: &Placed, toward: Point) -> Point {
    let dx = toward.x - node.center.x;
    let dy = toward.y - node.center.y;
    if dx == 0.0 && dy == 0.0 {
        return node.center;
    }

    let scale = match node.shape {
        NodeShape::Circle if !node.is_cluster => {
            let r = node.half_w;
            r / (dx * dx + dy * dy).sqrt()
        }
        NodeShape::Diamond if !node.is_cluster => {
            1.0 / (dx.abs() / node.half_w + dy.abs() / node.half_h)
        }
        _ => {
            let sx = if dx != 0.0 {
                node.half_w / dx.abs()
            } else {
                f64::INFINITY
            };
            let sy = if dy != 0.0 {
                node.half_h / dy.abs()
            } else {
                f64::INFINITY
            };
            sx.min(sy)
        }
    };
    Point::new(node.center.x + dx * scale, node.center.y + dy * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request/response cycle flowchart using a neutral pizza-shop scenario.
    const EXAMPLE: &str = "graph LR\n\
        C[Cashier] -- \"Order (make this)\" --> S[Kitchen]\n\
        S -- \"Report (done / burnt)\" --> C\n\
        S -- \"Event (low stock, timer, walk-in)\" --> C\n";

    /// A nested-subgraph diagram exercising clusters, hexagons, `<br/>` labels,
    /// bidirectional chains, and dotted/thick pipe-labeled links.
    const NESTED: &str = "flowchart TB\n\
        subgraph STACK[\"Prep line\"]\n\
            direction TB\n\
            A[mixer]\n\
            B[shaper]\n\
            A <--> B\n\
        end\n\
        STACK --- SWAP{{\"counter<br/>tickets\"}}\n\
        SWAP -.->|\"dine-in\"| REAL\n\
        SWAP ==>|\"takeout\"| SIM\n\
        subgraph REAL[\"Real kitchen\"]\n\
            direction TB\n\
            R1[oven]\n\
            R2[toppings]\n\
            R1 --- R2\n\
        end\n\
        subgraph SIM[\"Takeout kitchen\"]\n\
            direction TB\n\
            M1[prep bot]\n\
            M2[sim oven]\n\
            M1 --- M2\n\
        end\n";

    #[test]
    fn parses_direction() {
        assert_eq!(parse_direction("graph LR"), Direction::LeftRight);
        assert_eq!(parse_direction("flowchart TD"), Direction::TopBottom);
        assert_eq!(parse_direction("graph BT"), Direction::BottomTop);
        assert_eq!(parse_direction("graph"), Direction::TopBottom);
    }

    #[test]
    fn parses_nodes_with_labels_and_shapes() {
        let chart = parse(EXAMPLE);
        assert_eq!(chart.nodes.len(), 2);
        assert_eq!(chart.nodes[0].label, "Cashier");
        assert_eq!(chart.nodes[0].shape, NodeShape::Rectangle);
        assert_eq!(chart.nodes[1].label, "Kitchen");
    }

    #[test]
    fn parses_edges_with_quoted_labels() {
        let chart = parse(EXAMPLE);
        assert_eq!(chart.edges.len(), 3);
        assert_eq!(
            chart.edges[0].link.label.as_deref(),
            Some("Order (make this)")
        );
        assert!(chart.edges[0].link.arrow);
        assert_eq!(
            chart.edges[2].link.label.as_deref(),
            Some("Event (low stock, timer, walk-in)")
        );
    }

    #[test]
    fn cycle_does_not_hang_and_ranks_are_finite() {
        let chart = parse(EXAMPLE);
        let edges: Vec<(usize, usize)> = chart.edges.iter().map(|e| (e.from, e.to)).collect();
        let ranks = longest_path_ranks(chart.nodes.len(), &edges);
        assert_eq!(ranks.len(), 2);
        assert!(ranks.iter().all(|&r| r <= chart.nodes.len()));
    }

    #[test]
    fn back_edges_are_detected() {
        let chart = parse(EXAMPLE);
        let edges: Vec<(usize, usize)> = chart.edges.iter().map(|e| (e.from, e.to)).collect();
        let back = back_edges(chart.nodes.len(), &edges);
        assert!(!back[0]);
        assert!(back[1] || back[2]);
    }

    #[test]
    fn build_produces_two_nodes_and_three_connectors() {
        let shapes = build(EXAMPLE);
        let rects = shapes
            .iter()
            .filter(|s| matches!(s, Shape::Rectangle(_)))
            .count();
        let arrows = shapes
            .iter()
            .filter(|s| matches!(s, Shape::Arrow(_)))
            .count();
        // 2 node rectangles + 3 edge-label backgrounds.
        assert_eq!(rects, 5);
        assert_eq!(arrows, 3);
    }

    #[test]
    fn parses_various_node_shapes() {
        let src = "graph TD\nA[rect]\nB(round)\nC{diamond}\nD((circle))\nE([stadium])\n";
        let chart = parse(src);
        let shapes: Vec<NodeShape> = chart.nodes.iter().map(|n| n.shape).collect();
        assert_eq!(
            shapes,
            [
                NodeShape::Rectangle,
                NodeShape::Rounded,
                NodeShape::Diamond,
                NodeShape::Circle,
                NodeShape::Stadium,
            ]
        );
    }

    #[test]
    fn parses_dotted_and_thick_and_plain_links() {
        let src = "graph LR\nA-.->B\nB==>C\nC---D\n";
        let chart = parse(src);
        assert_eq!(chart.edges[0].link.style, LinkStyle::Dotted);
        assert!(chart.edges[0].link.arrow);
        assert_eq!(chart.edges[1].link.style, LinkStyle::Thick);
        assert_eq!(chart.edges[2].link.style, LinkStyle::Solid);
        assert!(!chart.edges[2].link.arrow, "--- has no arrowhead");
    }

    #[test]
    fn plain_link_does_not_swallow_target_node() {
        let chart = parse("graph LR\nA --- B\n");
        assert_eq!(chart.nodes.len(), 2);
        assert_eq!(chart.edges.len(), 1);
        assert!(chart.edges[0].link.label.is_none());
    }

    #[test]
    fn plain_link_into_shaped_node_is_not_a_label() {
        // Regression: `STACK --- SWAP{{...}}` must parse SWAP as a hexagon node,
        // not treat "SWAP{{...}}" as an edge label.
        let chart = parse("graph LR\nSTACK --- SWAP{{\"x<br/>y\"}}\n");
        assert_eq!(chart.nodes.len(), 2);
        assert_eq!(chart.edges.len(), 1);
        assert!(chart.edges[0].link.label.is_none());
        assert_eq!(chart.nodes[1].shape, NodeShape::Hexagon);
        assert_eq!(chart.nodes[1].label, "x\ny");
    }

    #[test]
    fn pipe_label_syntax_is_parsed() {
        let chart = parse("graph LR\nA -->|go| B\n");
        assert_eq!(chart.edges.len(), 1);
        assert_eq!(chart.edges[0].link.label.as_deref(), Some("go"));
        assert!(chart.edges[0].link.arrow);
    }

    #[test]
    fn subgraphs_capture_membership_and_are_clusters() {
        let chart = parse(NESTED);
        let idx = |id: &str| chart.nodes.iter().position(|n| n.id == id).unwrap();
        assert!(chart.nodes[idx("STACK")].is_cluster);
        assert!(chart.nodes[idx("REAL")].is_cluster);
        assert!(chart.nodes[idx("SIM")].is_cluster);
        assert!(!chart.nodes[idx("SWAP")].is_cluster);
        assert_eq!(chart.nodes[idx("SWAP")].shape, NodeShape::Hexagon);

        // A and B belong to STACK; R1/R2 to REAL; M1/M2 to SIM.
        assert_eq!(chart.nodes[idx("A")].parent, Some(idx("STACK")));
        assert_eq!(chart.nodes[idx("B")].parent, Some(idx("STACK")));
        assert_eq!(chart.nodes[idx("R1")].parent, Some(idx("REAL")));
        assert_eq!(chart.nodes[idx("M2")].parent, Some(idx("SIM")));
        // Top-level nodes have no parent.
        assert_eq!(chart.nodes[idx("STACK")].parent, None);
        assert_eq!(chart.nodes[idx("SWAP")].parent, None);
    }

    #[test]
    fn nested_layout_places_cluster_children_inside_the_cluster_box() {
        let chart = parse(NESTED);
        let children = child_lists(&chart);
        let mut state = LayoutState {
            abs: vec![Point::ZERO; chart.nodes.len()],
            size: vec![(0.0, 0.0); chart.nodes.len()],
            rel: vec![Point::ZERO; chart.nodes.len()],
            content_center: vec![Point::ZERO; chart.nodes.len()],
        };
        measure_scope(&chart, None, &children, &mut state);
        for &root_child in &children.root {
            state.abs[root_child] = state.rel[root_child];
            assign_absolute(root_child, &chart, &children, &mut state);
        }

        let idx = |id: &str| chart.nodes.iter().position(|n| n.id == id).unwrap();
        let stack = idx("STACK");
        let cluster_box = Rect::from_center_size(
            state.abs[stack],
            kurbo::Size::new(state.size[stack].0, state.size[stack].1),
        );
        // Every direct child of STACK must sit within STACK's box.
        for &ch in &children.per_node[stack] {
            let c = state.abs[ch];
            assert!(
                cluster_box.contains(c),
                "child center {c:?} escaped cluster box {cluster_box:?}"
            );
        }
    }

    #[test]
    fn nested_diagram_imports_and_draws_cluster_boxes() {
        let shapes = build(NESTED);
        // Three clusters -> at least three rectangles with the cluster fill.
        let cluster_rects = shapes
            .iter()
            .filter(|s| match s {
                Shape::Rectangle(r) => r.style.fill_color == Some(CLUSTER_FILL),
                _ => false,
            })
            .count();
        assert_eq!(cluster_rects, 3);
        // Connectors: STACK---SWAP (line), SWAP-.->REAL, SWAP==>SIM (arrows),
        // plus the internal A<-->B, R1---R2, M1---M2.
        let connectors = shapes
            .iter()
            .filter(|s| matches!(s, Shape::Arrow(_) | Shape::Line(_)))
            .count();
        assert!(
            connectors >= 5,
            "expected several connectors, got {connectors}"
        );
    }

    /// A full diagram mirroring the reported structure — nested subgraphs, a
    /// multi-hop bidirectional chain, a hexagon with a `<br/>` label, and
    /// dotted/thick pipe-labeled links — using a neutral pizza-shop scenario.
    const USER_DIAGRAM: &str = "flowchart TB\n\
        subgraph KITCHEN[\"The order pipeline — unchanged\"]\n\
            direction TB\n\
            A[counter / customer]\n\
            B[cashier bridge]\n\
            C[kitchen coordinator]\n\
            D[menu / stock / prep board]\n\
            A <--> B <--> C <--> D\n\
        end\n\
        KITCHEN --- MODE{{\"service boundary — same tickets either way<br/>▼ orders · recipes   ▲ status · timers · counts\"}}\n\
        MODE -.->|\"dine-in\"| REAL\n\
        MODE ==>|\"takeout (offline, no oven)\"| SIM\n\
        subgraph REAL[\"Real kitchen\"]\n\
            direction TB\n\
            R1[stone oven + peels]\n\
            R2[fresh basil · mozzarella · tomatoes]\n\
            R3[the dining room]\n\
            R1 --- R2 --- R3\n\
        end\n\
        subgraph SIM[\"takeout — drop-in replacement\"]\n\
            direction TB\n\
            M1[prep_bot + bake_sim / chill_sim]\n\
            M2[simulated oven · timer_feed · scales]\n\
            M3[recipe scene + packing world]\n\
            M1 --- M2 --- M3\n\
        end\n";

    #[test]
    fn user_diagram_parses_chains_clusters_and_hexagon() {
        let chart = parse(USER_DIAGRAM);
        let idx = |id: &str| chart.nodes.iter().position(|n| n.id == id).unwrap();

        // Four leaf nodes in KITCHEN joined by a 3-hop bidirectional chain.
        for id in ["A", "B", "C", "D"] {
            assert_eq!(chart.nodes[idx(id)].parent, Some(idx("KITCHEN")));
        }
        let kitchen_edges = chart
            .edges
            .iter()
            .filter(|e| chart.nodes[e.from].parent == Some(idx("KITCHEN")))
            .count();
        assert_eq!(kitchen_edges, 3, "A<-->B<-->C<-->D is three edges");

        // MODE is a hexagon with a two-line label from the <br/>.
        assert_eq!(chart.nodes[idx("MODE")].shape, NodeShape::Hexagon);
        assert!(chart.nodes[idx("MODE")].label.contains('\n'));

        // Pipe labels on the boundary links survive.
        let mode_real = chart
            .edges
            .iter()
            .find(|e| e.from == idx("MODE") && e.to == idx("REAL"))
            .expect("MODE -> REAL edge");
        assert_eq!(mode_real.link.label.as_deref(), Some("dine-in"));
        assert_eq!(mode_real.link.style, LinkStyle::Dotted);

        let mode_sim = chart
            .edges
            .iter()
            .find(|e| e.from == idx("MODE") && e.to == idx("SIM"))
            .expect("MODE -> SIM edge");
        assert_eq!(mode_sim.link.style, LinkStyle::Thick);
    }

    #[test]
    fn user_diagram_builds_without_panic_as_single_group() {
        // Exercises the full pipeline on the real input.
        let shapes = build(USER_DIAGRAM);
        assert!(!shapes.is_empty());
        let clusters = shapes
            .iter()
            .filter(|s| match s {
                Shape::Rectangle(r) => r.style.fill_color == Some(CLUSTER_FILL),
                _ => false,
            })
            .count();
        assert_eq!(clusters, 3, "KITCHEN, REAL, SIM cluster boxes");
    }
}
