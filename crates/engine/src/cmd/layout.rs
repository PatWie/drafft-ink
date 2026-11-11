//! `layout`: tidy up existing shapes with an automatic layout (docs/commands.md §2.19).
//!
//! The shapes are the nodes, the arrows bound between them are the edges. The algorithms and the transform rules
//! (bound text, elbow routes re-routed once, locked refusal) are in `drafftink_edit::layout`.

use drafftink_edit::layout::{
    Algorithm, ApplyError, ApplyOpts, ApplyResult, Direction, LayoutAnchor, LayoutOpts, layout_apply,
};
use drafftink_edit::query::{Query, QueryCtx, QueryError, QueryOpts, run_query};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{ElementId, Kind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::text::support::{RectOut, with_doc};
use crate::cmd::element_common::target_refusal;
use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo};

/// The algorithms `layout.apply` runs (`none` would keep everything where it is).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayoutAlgorithm {
    /// Layers of nodes with few crossings; handles cycles. The default.
    #[default]
    Layered,
    /// A forest: every node has at most one incoming edge and there is no cycle.
    Tree,
    /// Rows and columns in node order.
    Grid,
    /// Force-directed placement.
    Force,
}

/// Where the laid-out set ends up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// The top-left corner of the set's bounds stays where it was. The default.
    #[default]
    Keep,
    /// The top-left corner of the result goes to `at`. Without `at` this is the same as `keep`.
    TopLeft,
    /// The centre of the set's bounds stays where it was.
    Center,
}

/// Arguments of `layout.apply`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ApplyArgs {
    /// The algorithm.
    pub algorithm: LayoutAlgorithm,
    /// The shapes to lay out. Arrows between them are re-routed. Default: the selection. Give `ids` or `filter`.
    pub ids: Option<Vec<Ref>>,
    /// Select the shapes with a query instead of `ids` (see `element.query`).
    pub filter: Option<Query>,
    /// Where layers grow (layered and tree): `down`, `right`, `up` or `left`.
    pub direction: Direction,
    /// Space between neighbours in a layer.
    pub node_gap: f64,
    /// Space between layers (between rows for grid).
    pub layer_gap: f64,
    /// Columns of the grid layout. Absent: about the square root of the node count.
    pub columns: Option<u32>,
    /// Where the result sits.
    pub anchor: Anchor,
    /// With `anchor` = `top_left`: the world point `[x, y]` the top-left corner goes to.
    pub at: Option<[f64; 2]>,
    /// Re-route the bound arrows after the move (elbow routes once, after all moves).
    pub reroute: bool,
}

impl Default for ApplyArgs {
    fn default() -> Self {
        let d = LayoutOpts::default();
        Self {
            algorithm: LayoutAlgorithm::Layered,
            ids: None,
            filter: None,
            direction: d.direction,
            node_gap: d.node_gap,
            layer_gap: d.layer_gap,
            columns: None,
            anchor: Anchor::Keep,
            at: None,
            reroute: true,
        }
    }
}

/// Output of `layout.apply`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ApplyOut {
    /// The shapes that moved.
    pub moved: Vec<String>,
    /// The arrows that were re-routed.
    pub rerouted: Vec<String>,
    /// The bounds of the shapes after the layout.
    pub bounds: RectOut,
}

fn is_node(e: &drafftink_scene::Element) -> bool {
    !e.deleted && !e.tag().is_linear() && e.tag() != Kind::Frame && e.container_id().is_none()
}

/// `too_few` for the default arguments: fewer than two shapes selected.
fn default_targets(s: &Session) -> Result<(), Refusal> {
    let have = s.selection().iter().filter(|i| s.element(i).is_some_and(is_node)).count();
    if have < 2 { Err(Refusal::too_few("layout.apply", 2, "elements", have)) } else { Ok(()) }
}

fn apply(s: &mut Session, a: ApplyArgs) -> Result<ApplyOut, Refusal> {
    let cmd = s.cmd();
    args::in_range(cmd, "node_gap", a.node_gap, 0.0, args::MAX_WORLD)?;
    args::in_range(cmd, "layer_gap", a.layer_gap, 0.0, args::MAX_WORLD)?;
    if let Some(c) = a.columns
        && !(1..=10_000).contains(&c)
    {
        return Err(Refusal::bad_arg(cmd, "columns", "must be a whole number from 1 to 10000"));
    }
    let anchor = match (a.anchor, a.at) {
        (Anchor::Keep, None) => LayoutAnchor::Keep,
        (Anchor::Center, None) => LayoutAnchor::Center,
        (Anchor::TopLeft, None) => LayoutAnchor::Keep,
        (Anchor::TopLeft, Some([x, y])) => {
            LayoutAnchor::At(Point::new(args::world(cmd, "at[0]", x)?, args::world(cmd, "at[1]", y)?))
        }
        (_, Some(_)) => {
            return Err(Refusal::bad_arg(
                cmd,
                "at",
                "only works with anchor \"top_left\". Add anchor=\"top_left\", or drop `at`",
            ));
        }
    };
    let ids: Vec<ElementId> = match (&a.ids, &a.filter) {
        (Some(_), Some(_)) => {
            return Err(Refusal::bad_args(cmd, "give `ids` or `filter`, not both"));
        }
        (None, Some(q)) => {
            let ctx = QueryCtx { selected: Some(s.selection()) };
            let opts = QueryOpts { limit: drafftink_edit::query::MAX_LIMIT, ..QueryOpts::default() };
            let r = run_query(s.store(), q, &ctx, &opts).map_err(|e| match e {
                QueryError::Target(_) | QueryError::UnknownName(_) => Refusal::not_found(cmd, &e.to_string()),
                _ => Refusal::bad_args(cmd, e.to_string()),
            })?;
            if r.truncated {
                return Err(Refusal::limit_exceeded(cmd, r.total, "elements", drafftink_edit::query::MAX_LIMIT));
            }
            if r.ids.is_empty() {
                return Err(Refusal::no_target(cmd));
            }
            r.ids
        }
        (ids, None) => s.targets(ids.as_deref())?,
    };
    let algorithm = match a.algorithm {
        LayoutAlgorithm::Layered => Algorithm::Layered,
        LayoutAlgorithm::Tree => Algorithm::Tree,
        LayoutAlgorithm::Grid => Algorithm::Grid,
        LayoutAlgorithm::Force => Algorithm::Force,
    };
    let opts = ApplyOpts {
        layout: LayoutOpts {
            algorithm,
            direction: a.direction,
            node_gap: a.node_gap,
            layer_gap: a.layer_gap,
            columns: a.columns,
        },
        anchor,
        reroute: a.reroute,
    };
    let r: ApplyResult = with_doc(s, |tx, m, _math, _ids| Ok::<_, ApplyFailure>(layout_apply(tx, m, &ids, &opts)?))
        .map_err(|ApplyFailure(e)| refuse_apply(cmd, e))?;
    Ok(ApplyOut {
        moved: r.moved.iter().map(|i| i.as_str().to_owned()).collect(),
        rerouted: r.rerouted.iter().map(|i| i.as_str().to_owned()).collect(),
        bounds: r.bounds.into(),
    })
}

struct ApplyFailure(ApplyError);

impl From<ApplyError> for ApplyFailure {
    fn from(e: ApplyError) -> Self {
        ApplyFailure(e)
    }
}

impl From<drafftink_scene::StoreError> for ApplyFailure {
    fn from(e: drafftink_scene::StoreError) -> Self {
        ApplyFailure(ApplyError::Store(e))
    }
}

fn refuse_apply(cmd: &str, e: ApplyError) -> Refusal {
    let msg = e.to_string();
    match e {
        ApplyError::TooFew(n) => Refusal::too_few(cmd, 2, "elements", n),
        ApplyError::Target(t) => target_refusal(cmd, t),
        ApplyError::LimitExceeded { what, max } => Refusal::custom(
            "limit_exceeded",
            cmd,
            format!("`{cmd}` was given too many {what}, and the limit is {max}. Lay out a smaller part with `ids`."),
        ),
        ApplyError::TreeNotForest(id) => {
            Refusal::custom("tree_not_forest", cmd, msg).with_details(json!({ "node": id.as_str() }))
        }
        ApplyError::Layout(_) => Refusal::bad_args(cmd, msg),
        ApplyError::Store(_) | ApplyError::Text(_) | ApplyError::Binding(_) => Refusal::internal(cmd, msg),
    }
}

/// The layout family.
pub const COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "layout.apply",
    "layout",
    "Auto-layout",
    "Arranges the selected shapes with a layout algorithm and re-routes the arrows bound between them.",
    &Typed::new(apply),
    Facts {
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
    },
)
.default_targets(default_targets)
.refusals(&["tree_not_forest", "limit_exceeded"])
.example(r#"{"algorithm":"layered","direction":"right","node_gap":40,"layer_gap":100}"#)];

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
