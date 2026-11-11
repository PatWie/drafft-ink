//! `diagram`: build diagrams from structure (docs/commands.md §2.19).
//!
//! `diagram.create` takes symbolic node ids and returns the real ones, so an agent builds a whole flowchart in one
//! call (nodes with labels, bound arrows with labels, optional group and frame) and undoes it in one step.
//! `diagram.import_mermaid` maps the parsed Mermaid graph (`drafftink_io::mermaid`) into the same
//! `DiagramSpec` (see `diagram_mermaid.rs`), because `io` and `edit` are both L4 and cannot depend on each other.
//! `diagram.extend` adds one connected node next to a shape (the Excalidraw flowchart helper).

use std::collections::BTreeMap;

use drafftink_edit::flowchart::{self, ExtendOpts, FlowchartError};
use drafftink_edit::layout::{
    Algorithm, DiagramResult, CreateOpts, Direction, DiagramError, DiagramSpec, EdgeSpec, GroupSpec, LayoutOpts, NodeSpec,
    StyleSpec, below_content_origin,
};
use drafftink_edit::transform;
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_scene::limits;
use drafftink_scene::ElementId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::text::support::{RectOut, refuse, select_only, with_doc};
use crate::cmd::element_common::target_refusal;
use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed, Undo,
};

#[path = "diagram_mermaid.rs"]
mod mermaid;

/// Most nodes and edges of one diagram (the limits of `edit`).
use drafftink_edit::layout::{MAX_EDGES, MAX_NODES};

// ---------------------------------------------------------------------------------------------
// Shared: validation, placement and the result
// ---------------------------------------------------------------------------------------------

/// The words `at` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AtWord {
    /// The middle of the viewport.
    ViewportCenter,
    /// Directly below everything that is in the document now.
    BelowContent,
}

/// Where the diagram goes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum DiagramAt {
    /// `"viewport_center"` or `"below_content"`.
    Word(AtWord),
    /// The top-left corner `[x, y]` in world units.
    Point([f64; 2]),
}

impl Default for DiagramAt {
    fn default() -> Self {
        DiagramAt::Word(AtWord::ViewportCenter)
    }
}

fn num(cmd: &str, key: &str, v: Option<f64>) -> Result<(), Refusal> {
    if let Some(v) = v {
        args::world(cmd, key, v)?;
    }
    Ok(())
}

fn check_style(cmd: &str, key: &str, st: &Option<StyleSpec>) -> Result<(), Refusal> {
    let Some(st) = st else { return Ok(()) };
    if let Some(v) = st.stroke_width {
        args::in_range(cmd, &format!("{key}.stroke_width"), v, 0.0, limits::MAX_STROKE_WIDTH)?;
    }
    if let Some(v) = st.opacity {
        args::in_range(cmd, &format!("{key}.opacity"), v, 0.0, 1.0)?;
    }
    if let Some(v) = st.corner_radius {
        args::in_range(cmd, &format!("{key}.corner_radius"), v, 0.0, limits::MAX_CORNER_RADIUS)?;
    }
    if let Some(v) = st.font_size {
        args::positive(cmd, &format!("{key}.font_size"), v, limits::MAX_FONT_SIZE)?;
    }
    Ok(())
}

/// Checks everything the spec carries that the layout would otherwise meet as a hostile number.
fn check_spec(cmd: &str, spec: &DiagramSpec) -> Result<(), Refusal> {
    if spec.nodes.len() > MAX_NODES {
        return Err(Refusal::limit_exceeded(cmd, spec.nodes.len(), "nodes", MAX_NODES));
    }
    if spec.edges.len() > MAX_EDGES {
        return Err(Refusal::limit_exceeded(cmd, spec.edges.len(), "edges", MAX_EDGES));
    }
    let l = &spec.layout;
    args::in_range(cmd, "layout.node_gap", l.node_gap, 0.0, args::MAX_WORLD)?;
    args::in_range(cmd, "layout.layer_gap", l.layer_gap, 0.0, args::MAX_WORLD)?;
    if let Some(c) = l.columns
        && !(1..=10_000).contains(&c)
    {
        return Err(Refusal::bad_arg(cmd, "layout.columns", "must be a whole number from 1 to 10000"));
    }
    for (i, n) in spec.nodes.iter().enumerate() {
        let k = format!("nodes[{i}]");
        num(cmd, &format!("{k}.width"), n.width)?;
        num(cmd, &format!("{k}.height"), n.height)?;
        num(cmd, &format!("{k}.x"), n.x)?;
        num(cmd, &format!("{k}.y"), n.y)?;
        if let Some([w, h]) = n.size {
            num(cmd, &format!("{k}.size"), Some(w))?;
            num(cmd, &format!("{k}.size"), Some(h))?;
        }
        check_style(cmd, &format!("{k}.style"), &n.style)?;
        if let Some(l) = &n.label
            && l.chars().count() > limits::MAX_TEXT_CHARS
        {
            return Err(Refusal::bad_arg(cmd, &format!("{k}.label"), "is too long"));
        }
    }
    for (i, e) in spec.edges.iter().enumerate() {
        check_style(cmd, &format!("edges[{i}].style"), &e.style)?;
        if let Some(l) = &e.label
            && l.chars().count() > limits::MAX_TEXT_CHARS
        {
            return Err(Refusal::bad_arg(cmd, &format!("edges[{i}].label"), "is too long"));
        }
    }
    Ok(())
}

/// Maps a diagram error to the refusal of the catalog. The messages of `edit` are the catalog texts.
fn refuse_diagram(cmd: &str, e: DiagramError) -> Refusal {
    let msg = e.to_string();
    match e {
        DiagramError::DuplicateNode(id) => Refusal::custom("duplicate_node", cmd, msg).with_details(serde_json::json!({ "node": id })),
        DiagramError::UnknownNode { id, .. } => Refusal::custom("unknown_node", cmd, msg).with_details(serde_json::json!({ "node": id })),
        DiagramError::UnknownGroupMember { id, .. } => Refusal::custom("unknown_node", cmd, msg).with_details(serde_json::json!({ "node": id })),
        DiagramError::TreeNotForest(id) => Refusal::custom("tree_not_forest", cmd, msg).with_details(serde_json::json!({ "node": id })),
        DiagramError::LimitExceeded { what, max } => {
            Refusal::custom("limit_exceeded", cmd, format!("`{cmd}` was given too many {what}, and the limit is {max}. Split the diagram into several calls."))
        }
        DiagramError::NameTaken { .. } => Refusal::custom("name_taken", cmd, msg),
        DiagramError::BadName(_) => Refusal::custom("bad_name", cmd, msg),
        DiagramError::Text(t) => refuse(cmd, t),
        DiagramError::Store(_) | DiagramError::Index(_) | DiagramError::Binding(_) => Refusal::internal(cmd, msg),
        _ => Refusal::custom("bad_args", cmd, msg),
    }
}

/// The result of building a diagram.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DiagramOut {
    /// Symbolic node id to the real element id. In a batch, later steps use `$<step>.<node>`.
    pub nodes: BTreeMap<String, String>,
    /// The arrows, in the order of `edges`.
    pub edges: Vec<String>,
    /// The groups made, in order.
    pub groups: Vec<String>,
    /// Mermaid subgraphs: symbolic cluster id to the element id of its box. Edges may start or end at a box.
    pub clusters: BTreeMap<String, String>,
    /// The frame, when one was asked for.
    pub frame: Option<String>,
    /// Bounds of the whole diagram.
    pub bounds: RectOut,
}

/// Builds a diagram at `at`, selects the nodes and arrows, and returns the real ids.
fn build(
    s: &mut Session,
    built: mermaid::Built,
    at: DiagramAt,
    frame: Option<String>,
    select: bool,
) -> Result<DiagramOut, Refusal> {
    let cmd = s.cmd();
    check_spec(cmd, &built.spec)?;
    let at = match at {
        DiagramAt::Point([x, y]) => DiagramAt::Point([args::world(cmd, "at[0]", x)?, args::world(cmd, "at[1]", y)?]),
        w => w,
    };
    if let Some(f) = &frame
        && f.chars().count() > limits::MAX_NAME_CHARS
    {
        return Err(Refusal::bad_arg(cmd, "frame", "is too long for a frame name"));
    }
    let centre = s.view().scroll;
    let keep_positions = built.spec.layout.algorithm == Algorithm::None && built.seq.is_none();
    let (res, extra, group_ids): (DiagramResult, Vec<ElementId>, Vec<String>) = with_doc(s, |tx, m, _math, ids| {
        let origin = match at {
            DiagramAt::Point([x, y]) => Point::new(x, y),
            DiagramAt::Word(AtWord::BelowContent) => below_content_origin(&*tx, 80.0),
            DiagramAt::Word(AtWord::ViewportCenter) => Point::ZERO,
        };
        let mut spec = built.spec.clone();
        // A sequence diagram has fixed positions, so the shift is applied to them and to the lifelines.
        let mut shift = Vec2::ZERO;
        if let Some(seq) = &built.seq {
            shift = match at {
                DiagramAt::Word(AtWord::ViewportCenter) => centre - seq.bounds.center(),
                _ => origin - seq.bounds.origin(),
            };
            for n in &mut spec.nodes {
                n.x = n.x.map(|x| x + shift.x);
                n.y = n.y.map(|y| y + shift.y);
            }
        }
        let opts = CreateOpts { origin, frame: frame.clone() };
        let mut res = drafftink_edit::layout::diagram_create(tx, m, ids, &spec, &opts).map_err(BuildError::Diagram)?;
        let mut extra: Vec<ElementId> = Vec::new();
        if let Some(seq) = &built.seq {
            extra = mermaid::sequence_extras(tx, m, ids, seq, shift)?;
            let b = seq.bounds.with_origin(seq.bounds.origin() + shift);
            res.bounds = [b.x0, b.y0, b.width(), b.height()];
        } else if at == DiagramAt::Word(AtWord::ViewportCenter) && !keep_positions {
            let [x, y, w, h] = res.bounds;
            let d = Vec2::new(centre.x - (x + w / 2.0), centre.y - (y + h / 2.0));
            if d.x.is_finite() && d.y.is_finite() && (d.x != 0.0 || d.y != 0.0) {
                transform::move_by(tx, &res.created, d).map_err(|e| BuildError::Move(e.to_string()))?;
                res.bounds = [x + d.x, y + d.y, w, h];
            }
        }
        let mut groups: Vec<String> = res.group_ids.iter().map(|g| g.to_string()).collect();
        if built.group_all {
            let mut members: Vec<ElementId> = res.nodes.values().cloned().collect();
            members.extend(extra.iter().cloned());
            let g = drafftink_edit::group::group(tx, &members, None, ids.group_id())
                .map_err(|e| BuildError::Move(e.to_string()))?;
            groups.push(g.group.to_string());
        }
        Ok((res, extra, groups))
    })
    .map_err(|e: BuildError| match e {
        BuildError::Diagram(d) => refuse_diagram(cmd, d),
        BuildError::Move(m) => Refusal::internal(cmd, m),
        BuildError::Store(e) => Refusal::internal(cmd, e.to_string()),
    })?;
    if select {
        let mut sel: Vec<ElementId> = res.nodes.values().cloned().collect();
        sel.extend(res.clusters.values().cloned());
        sel.extend(res.edges.iter().cloned());
        sel.extend(extra.iter().cloned());
        select_only(s, &sel);
    }
    let [x, y, w, h] = res.bounds;
    Ok(DiagramOut {
        nodes: res.nodes.iter().map(|(k, v)| (k.clone(), v.as_str().to_owned())).collect(),
        edges: res.edges.iter().chain(extra.iter()).map(|e| e.as_str().to_owned()).collect(),
        groups: group_ids,
        clusters: res.clusters.iter().map(|(k, v)| (k.clone(), v.as_str().to_owned())).collect(),
        frame: res.frame.map(|f| f.as_str().to_owned()),
        bounds: RectOut { x: x.round(), y: y.round(), w: w.round(), h: h.round() },
    })
}

/// Why building a diagram failed.
pub(super) enum BuildError {
    Diagram(DiagramError),
    Move(String),
    Store(drafftink_scene::StoreError),
}

impl From<drafftink_scene::StoreError> for BuildError {
    fn from(e: drafftink_scene::StoreError) -> Self {
        BuildError::Store(e)
    }
}

// ---------------------------------------------------------------------------------------------
// diagram.create
// ---------------------------------------------------------------------------------------------

/// Arguments of `diagram.create`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateArgs {
    /// The nodes. `id` is a name you choose; edges refer to it, and the result maps it to the real element id.
    pub nodes: Vec<NodeSpec>,
    /// The edges: one arrow per edge, bound to both nodes, with an optional label.
    #[serde(default)]
    pub edges: Vec<EdgeSpec>,
    /// How to place the nodes. Default: layered, top to bottom, gaps 40 and 80.
    #[serde(default)]
    pub layout: LayoutOpts,
    /// Where the diagram goes: a top-left point, `"viewport_center"` (default) or `"below_content"`. Ignored by the
    /// `none` layout, which uses the `x` and `y` of the nodes.
    #[serde(default)]
    pub at: DiagramAt,
    /// Put all nodes (and the arrows between them) in one group.
    #[serde(default)]
    pub group: bool,
    /// Wrap the diagram in a frame with this name.
    #[serde(default)]
    pub frame: Option<String>,
    /// Select the nodes and arrows afterwards.
    #[serde(default = "yes")]
    pub select: bool,
}

fn yes() -> bool {
    true
}

fn create(s: &mut Session, a: CreateArgs) -> Result<DiagramOut, Refusal> {
    let cmd = s.cmd();
    if a.nodes.is_empty() {
        return Err(Refusal::custom("bad_args", cmd, "Give at least one node."));
    }
    let mut spec = DiagramSpec { nodes: a.nodes, edges: a.edges, groups: Vec::new(), clusters: Vec::new(), layout: a.layout };
    if a.group && spec.nodes.len() >= 2 {
        spec.groups.push(GroupSpec { id: None, members: spec.nodes.iter().map(|n| n.id.clone()).collect() });
    }
    build(s, mermaid::Built { spec, seq: None, group_all: false }, a.at, a.frame, a.select)
}

// ---------------------------------------------------------------------------------------------
// diagram.import_mermaid
// ---------------------------------------------------------------------------------------------

/// Where the Mermaid text comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MermaidSource {
    /// The word `"clipboard"`.
    Clipboard(ClipboardWord),
    /// The text itself.
    Text {
        /// Mermaid source, with or without a code fence.
        text: String,
    },
}

/// The only word `source` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardWord {
    /// Read the clipboard port.
    Clipboard,
}

/// Arguments of `diagram.import_mermaid`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportArgs {
    /// `"clipboard"`, or `{"text": "graph TD; A-->B"}`.
    pub source: MermaidSource,
    /// Where the diagram goes: a top-left point, or `"viewport_center"` (default).
    #[serde(default)]
    pub at: DiagramAt,
    /// Group the whole diagram, and every Mermaid subgraph with its box and title.
    #[serde(default = "yes")]
    pub group: bool,
    /// Select the result.
    #[serde(default = "yes")]
    pub select: bool,
}

fn import_mermaid(s: &mut Session, a: ImportArgs) -> Result<DiagramOut, Refusal> {
    let cmd = s.cmd();
    let text = match a.source {
        MermaidSource::Text { text } => text,
        MermaidSource::Clipboard(_) => {
            s.require_port(
                crate::ports::PortKind::Clipboard,
                "the clipboard",
                "Pass the diagram as `source: {\"text\": \"...\"}` instead.",
            )?;
            let payload = match s.ports.clipboard.as_ref() {
                Some(c) => c
                    .read(&[crate::ports::ClipKind::Text])
                    .map_err(|e| Refusal::io_failed(cmd, "read", "the clipboard", &e.to_string()))?,
                None => None,
            };
            match payload {
                Some(crate::ports::ClipPayload::Text(t)) if !t.trim().is_empty() => t,
                _ => {
                    return Err(Refusal::custom(
                        "clipboard_empty",
                        cmd,
                        "The clipboard holds no text. Copy the Mermaid text first, or pass it as `source: {\"text\": \"...\"}`.",
                    ));
                }
            }
        }
    };
    let (built, warnings) = mermaid::to_spec(cmd, &text, a.group, s)?;
    for w in warnings {
        s.warn(w);
    }
    build(s, built, a.at, None, a.select)
}

// ---------------------------------------------------------------------------------------------
// diagram.extend
// ---------------------------------------------------------------------------------------------

/// Arguments of `diagram.extend`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtendArgs {
    /// The rectangle, diamond or ellipse to grow from. Default: the single selected element.
    #[serde(default)]
    pub from: Option<Ref>,
    /// Which side the new node goes.
    pub direction: Direction,
    /// Text of the new node.
    #[serde(default)]
    pub label: Option<String>,
    /// Distance between the shapes, in world units.
    #[serde(default = "default_gap")]
    pub gap: f64,
}

fn default_gap() -> f64 {
    flowchart::DEFAULT_GAP
}

/// Output of `diagram.extend`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ExtendOut {
    /// The new node, now selected.
    pub node: String,
    /// The arrow from the parent to the new node.
    pub arrow: String,
    /// The node's label text element, when `label` was given.
    pub node_label: Option<String>,
    /// Where the node is.
    pub bounds: RectOut,
}

fn extend(s: &mut Session, a: ExtendArgs) -> Result<ExtendOut, Refusal> {
    let cmd = s.cmd();
    let gap = args::in_range(cmd, "gap", a.gap, 0.0, args::MAX_WORLD)?;
    if let Some(l) = &a.label
        && l.chars().count() > limits::MAX_TEXT_CHARS
    {
        return Err(Refusal::bad_arg(cmd, "label", "is too long"));
    }
    let from = match &a.from {
        Some(r) => s.resolve_ref(r)?,
        None => {
            let sel: Vec<ElementId> = s.selection().iter().filter(|i| s.element(i).is_some()).cloned().collect();
            match sel.as_slice() {
                [one] => one.clone(),
                other => return Err(Refusal::needs_single(cmd, "element", other.len())),
            }
        }
    };
    let opts = ExtendOpts { gap, label: a.label.clone(), ..ExtendOpts::default() };
    let dir = a.direction;
    let res = with_doc(s, |tx, m, _math, ids| {
        flowchart::create_connected_node(tx, m, ids, &from, dir, &opts)
    })
    .map_err(|e: FlowchartError| refuse_flow(cmd, e))?;
    let Some(x) = res else {
        return Err(Refusal::nothing_to_do(
            cmd,
            "add a node",
            &format!("there is no free spot next to `{from}` in that direction. Move other elements away, or choose another direction"),
        ));
    };
    select_only(s, std::slice::from_ref(&x.node));
    Ok(ExtendOut {
        node: x.node.as_str().to_owned(),
        arrow: x.arrow.as_str().to_owned(),
        node_label: x.node_label.map(|l| l.as_str().to_owned()),
        bounds: x.rect.into(),
    })
}

fn refuse_flow(cmd: &str, e: FlowchartError) -> Refusal {
    match e {
        FlowchartError::Target(t) => target_refusal(cmd, t),
        FlowchartError::WrongKind { kind, .. } => Refusal::wrong_kind(cmd, kind.name(), "rectangle, diamond and ellipse elements"),
        FlowchartError::BadGap | FlowchartError::NotFinite => Refusal::bad_args(cmd, e.to_string()),
        FlowchartError::Text(t) => refuse(cmd, t),
        other => Refusal::internal(cmd, other.to_string()),
    }
}

// ---------------------------------------------------------------------------------------------
// registration
// ---------------------------------------------------------------------------------------------

const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    needs_selection: Selection::No,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: true,
};

const EXTEND_KEYS: &[Binding] = &[
    Binding::new(&["Mod+Left"], Scope::Canvas).with_args(r#"{"direction":"left"}"#),
    Binding::new(&["Mod+Right"], Scope::Canvas).with_args(r#"{"direction":"right"}"#),
    Binding::new(&["Mod+Up"], Scope::Canvas).with_args(r#"{"direction":"up"}"#),
    Binding::new(&["Mod+Down"], Scope::Canvas).with_args(r#"{"direction":"down"}"#),
];

/// The diagram family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "diagram.create",
        "diagram",
        "Create diagram",
        "Builds a whole diagram from nodes and edges with symbolic ids: shapes with labels, bound arrows, one undo step.",
        &Typed::new(create),
        DOC,
    )
    .refusals(&["duplicate_node", "unknown_node", "tree_not_forest", "name_taken", "bad_name"])
    .example(
        r#"{"nodes":[{"id":"start","label":"Start","shape":"ellipse"},{"id":"check","label":"Valid?","shape":"diamond"},{"id":"done","label":"Done"}],"edges":[{"from":"start","to":"check"},{"from":"check","to":"done","label":"yes"}],"layout":{"direction":"down"}}"#,
    ),
    CommandSpec::new(
        "diagram.import_mermaid",
        "diagram",
        "Import Mermaid diagram",
        "Reads Mermaid flowchart or sequence diagram text and draws it with bound arrows, laid out by Mermaid's direction.",
        &Typed::new(import_mermaid),
        Facts { ports: PortSet::CLIPBOARD, palette: true, ..DOC },
    )
    .refusals(&["mermaid_unsupported", "clipboard_empty"])
    .example(r#"{"source":{"text":"graph LR; A[Start] --> B{Ok?}; B -->|yes| C[Done]"}}"#),
    CommandSpec::new(
        "diagram.extend",
        "diagram",
        "Add connected node",
        "Adds a node of the same kind and style next to a shape, joined to it by a bound arrow, and selects it.",
        &Typed::new(extend),
        Facts { palette: true, ..DOC },
    )
    .shortcuts(EXTEND_KEYS)
    .example(r#"{"from":"@start","direction":"down","label":"Next"}"#),
];

#[cfg(test)]
#[path = "diagram_tests.rs"]
mod tests;
