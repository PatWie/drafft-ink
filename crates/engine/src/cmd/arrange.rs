//! `arrange`: z-order, align and distribute (docs/commands.md §2.9).
//!
//! Groups, frames with their children and containers with their bound text are one unit. The rules are in
//! `drafftink_edit::zorder` and `drafftink_edit::arrange`; a command resolves its targets and maps the typed
//! errors to catalog refusals. The old app had these as buttons only (and no distribute); every one is a
//! command with a shortcut now, and the old inverted names of the centre alignments are fixed (`center_x`
//! makes the x-centres equal).

use drafftink_edit::arrange::Arrangement;
use drafftink_edit::arrange::{self, AlignMode, ArrangeError, Axis, DistributeOpts};
use drafftink_edit::transform::TransformError;
use drafftink_edit::zorder::{self, ZOp};
use drafftink_scene::ElementId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cmd::element_common::target_refusal;
use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed,
    Undo,
};

use super::group::common::{edit, editing_group, selected_ids, zorder_refusal};

// ---- errors ----

fn transform_refusal(cmd: &str, e: TransformError) -> Refusal {
    match e {
        TransformError::Target(t) => target_refusal(cmd, t),
        TransformError::NotFinite => Refusal::bad_args(cmd, "the result would not be a valid number, so use smaller values"),
        e @ (TransformError::NoElements
        | TransformError::RotateHandle
        | TransformError::BadBounds
        | TransformError::NotPositive
        | TransformError::RotateToNeedsOne) => Refusal::bad_args(cmd, e.to_string()),
        other => Refusal::internal(cmd, other.to_string()),
    }
}

fn arrange_refusal(cmd: &str, e: ArrangeError) -> Refusal {
    match e {
        ArrangeError::Target(t) => target_refusal(cmd, t),
        ArrangeError::TooFew { what: Arrangement::Align, have, .. } => Refusal::custom(
            "too_few",
            cmd,
            format!(
                "Align needs at least 2 elements (a group counts as one), and {have} were given. Select more, or pass `to` to align to a reference element."
            ),
        ),
        ArrangeError::TooFew { have, .. } => Refusal::custom(
            "too_few",
            cmd,
            format!(
                "Distribute needs at least 3 elements (a group counts as one), and {have} were given. Select more, or pass `ids`."
            ),
        ),
        ArrangeError::BadGap => Refusal::bad_arg(cmd, "gap", "must be a finite number"),
        ArrangeError::Transform(t) => transform_refusal(cmd, t),
    }
}

// ---- arrange.z_order ----

/// A z-order step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// To the top.
    Front,
    /// To the bottom.
    Back,
    /// One unit up.
    Forward,
    /// One unit down.
    Backward,
}

impl Op {
    fn domain(self) -> ZOp {
        match self {
            Op::Front => ZOp::Front,
            Op::Back => ZOp::Back,
            Op::Forward => ZOp::Forward,
            Op::Backward => ZOp::Backward,
        }
    }
}

/// Arguments of `arrange.z_order`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZOrderArgs {
    /// `front` and `back` go to the top or bottom; `forward` and `backward` step over the neighbouring unit.
    pub op: Op,
    /// Elements to move. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
}

/// Result of `arrange.z_order`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ZOrderOut {
    /// Elements that got a new place in the order (empty when they were at the end already).
    pub moved: Vec<ElementId>,
    /// True when all order keys had to be rewritten.
    pub rebalanced: bool,
}

fn z_order(s: &mut Session, a: ZOrderArgs) -> Result<ZOrderOut, Refusal> {
    let cmd = s.cmd();
    let ids = s.targets(a.ids.as_deref())?;
    let editing = editing_group(s);
    let report = edit(s, |tx, _| zorder::z_order(tx, &ids, a.op.domain(), editing.as_ref()))
        .map_err(|e| e.refusal(cmd, |z| zorder_refusal(cmd, z)))?;
    Ok(ZOrderOut { moved: report.moved, rebalanced: report.rebalanced })
}

// ---- arrange.align ----

/// Which edge or centre to align to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    /// Left edges on one vertical line.
    Left,
    /// Right edges on one vertical line.
    Right,
    /// Top edges on one horizontal line.
    Top,
    /// Bottom edges on one horizontal line.
    Bottom,
    /// The x-centres equal (elements share a vertical centre line).
    CenterX,
    /// The y-centres equal (elements share a horizontal centre line).
    CenterY,
}

impl Edge {
    fn domain(self) -> AlignMode {
        match self {
            Edge::Left => AlignMode::Left,
            Edge::Right => AlignMode::Right,
            Edge::Top => AlignMode::Top,
            Edge::Bottom => AlignMode::Bottom,
            Edge::CenterX => AlignMode::CenterX,
            Edge::CenterY => AlignMode::CenterY,
        }
    }
}

/// Arguments of `arrange.align`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlignArgs {
    /// The edge or centre line.
    pub edge: Edge,
    /// Elements to align (a group counts as one). Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// Align to this element instead of the bounds of the selection; it does not move.
    #[serde(default)]
    pub to: Option<Ref>,
}

/// Result of `arrange.align` and `arrange.distribute`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ArrangeOut {
    /// Elements that moved (bound text and frame children included), in z-order.
    pub moved: Vec<ElementId>,
    /// Arrow ends that were unbound because their target did not move too: `[arrow id, "start" or "end"]`.
    pub unbound: Vec<(ElementId, String)>,
}

fn out(report: drafftink_edit::transform::TransformReport) -> ArrangeOut {
    ArrangeOut {
        moved: report.changed,
        unbound: report.unbound.into_iter().map(|(id, end)| (id, format!("{end:?}").to_lowercase())).collect(),
    }
}

fn align(s: &mut Session, a: AlignArgs) -> Result<ArrangeOut, Refusal> {
    let cmd = s.cmd();
    let to = a.to.as_ref().map(|r| s.resolve_ref(r)).transpose()?;
    let ids = s.targets(a.ids.as_deref())?;
    let editing = editing_group(s);
    let report = edit(s, |tx, _| arrange::align(tx, &ids, a.edge.domain(), to.as_ref(), editing.as_ref()))
        .map_err(|e| e.refusal(cmd, |e| arrange_refusal(cmd, e)))?;
    Ok(out(report))
}

/// `too_few` for the selection, the text `execute` would give without `to`.
fn align_default_targets(s: &Session) -> Result<(), Refusal> {
    drafftink_edit::preflight::check_align(s.store(), &selected_ids(s), editing_group(s).as_ref())
        .map_err(|e| arrange_refusal("arrange.align", e))
}

// ---- arrange.distribute ----

/// The axis along which elements are spread out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DistAxis {
    /// Left to right.
    Horizontal,
    /// Top to bottom.
    Vertical,
}

/// Arguments of `arrange.distribute`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DistributeArgs {
    /// Spread along this axis.
    pub axis: DistAxis,
    /// Elements to distribute (a group counts as one). Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// Exact gap between neighbours, starting from the first element. Omitted: the two outer elements stay and the
    /// gaps between all become equal.
    #[serde(default)]
    pub gap: Option<f64>,
}

fn distribute(s: &mut Session, a: DistributeArgs) -> Result<ArrangeOut, Refusal> {
    let cmd = s.cmd();
    let gap = a.gap.map(|g| args::world(cmd, "gap", g)).transpose()?;
    let ids = s.targets(a.ids.as_deref())?;
    let editing = editing_group(s);
    let axis = match a.axis {
        DistAxis::Horizontal => Axis::X,
        DistAxis::Vertical => Axis::Y,
    };
    let report = edit(s, |tx, _| arrange::distribute(tx, &ids, axis, DistributeOpts { gap }, editing.as_ref()))
        .map_err(|e| e.refusal(cmd, |e| arrange_refusal(cmd, e)))?;
    Ok(out(report))
}

/// `too_few` for the selection.
fn distribute_default_targets(s: &Session) -> Result<(), Refusal> {
    drafftink_edit::preflight::check_distribute(s.store(), &selected_ids(s), editing_group(s).as_ref())
        .map_err(|e| arrange_refusal("arrange.distribute", e))
}

// ---- registration ----

const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC,
    needs_selection: Selection::Required,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: true,
    agent: true,
    doc_write: crate::spec::DocWrite::Edit,
};

const Z_KEYS: &[Binding] = &[
    Binding::new(&["Mod+]"], Scope::Canvas).with_args(r#"{"op":"forward"}"#).titled("Bring forward"),
    Binding::new(&["Mod+["], Scope::Canvas).with_args(r#"{"op":"backward"}"#).titled("Send backward"),
    Binding::new(&["Mod+Shift+]"], Scope::Canvas)
        .mac(&["Mod+Alt+]"])
        .with_args(r#"{"op":"front"}"#)
        .titled("Bring to front"),
    Binding::new(&["Mod+Shift+["], Scope::Canvas)
        .mac(&["Mod+Alt+["])
        .with_args(r#"{"op":"back"}"#)
        .titled("Send to back"),
];

const ALIGN_KEYS: &[Binding] = &[
    Binding::new(&["Mod+Shift+Left"], Scope::Canvas).with_args(r#"{"edge":"left"}"#).titled("Align left"),
    Binding::new(&["Mod+Shift+Right"], Scope::Canvas).with_args(r#"{"edge":"right"}"#).titled("Align right"),
    Binding::new(&["Mod+Shift+Up"], Scope::Canvas).with_args(r#"{"edge":"top"}"#).titled("Align top"),
    Binding::new(&["Mod+Shift+Down"], Scope::Canvas).with_args(r#"{"edge":"bottom"}"#).titled("Align bottom"),
];

const DISTRIBUTE_KEYS: &[Binding] = &[
    Binding::new(&["Alt+H"], Scope::Canvas)
        .with_args(r#"{"axis":"horizontal"}"#)
        .titled("Distribute horizontally"),
    Binding::new(&["Alt+V"], Scope::Canvas).with_args(r#"{"axis":"vertical"}"#).titled("Distribute vertically"),
];

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "arrange.z_order",
        "arrange",
        "Change layer order",
        "Moves the elements to the front or back, or one unit forward or backward; groups and frames count as one unit.",
        &Typed::new(z_order),
        DOC,
    )
    .shortcuts(Z_KEYS)
    .example(r#"{"op":"front","ids":["a"]}"#),
    CommandSpec::new(
        "arrange.align",
        "arrange",
        "Align",
        "Aligns the elements (a group counts as one) to an edge or a centre line of their bounds, or of the element `to`.",
        &Typed::new(align),
        DOC,
    )
    .default_targets(align_default_targets)
    .shortcuts(ALIGN_KEYS)
    .example(r#"{"edge":"left","ids":["a","b"]}"#),
    CommandSpec::new(
        "arrange.distribute",
        "arrange",
        "Distribute",
        "Spaces three or more elements (a group counts as one) evenly along an axis, or by an exact gap.",
        &Typed::new(distribute),
        DOC,
    )
    .default_targets(distribute_default_targets)
    .shortcuts(DISTRIBUTE_KEYS)
    .example(r#"{"axis":"horizontal","ids":["a","b","c"]}"#),
];

#[cfg(test)]
#[path = "arrange_tests.rs"]
mod tests;
