//! `selection.select_lasso`: select what a free-hand loop surrounds or crosses (docs/commands.md §2.5).
//!
//! The lasso is the free-form marquee. It takes the arguments of `selection.select_region` with a path instead of a
//! rectangle, and follows the same rules for groups, frames, locked elements, bound text and the modes replace
//! and add. The rule that decides whether a path selects a shape is `drafftink_geom::lasso`, and the selection
//! rule is `drafftink_edit::group::selection::select_lasso`; this file validates the arguments and shapes the
//! output.

use drafftink_edit::group::selection::{RegionMode, RegionOpts, select_lasso};
use drafftink_geom::kurbo::Point;
use drafftink_geom::lasso::{Lasso, LassoError};
use drafftink_geom::select::MarqueeMode;
use drafftink_scene::limits::MAX_POINTS;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::element_common::edit_selection;
use super::selection::{Contain, RegionCombine, SEL, SelectionOut, finish};
use super::view::Pt;
use crate::args;
use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::{CommandSpec, Typed};

/// The simplification of a path given without `simplify`, in world units (about the width of a thin stroke).
pub const DEFAULT_SIMPLIFY: f64 = 1.0;

fn default_simplify() -> f64 {
    DEFAULT_SIMPLIFY
}

/// Arguments of `selection.select_lasso`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectLassoArgs {
    /// The path of the lasso in world units, at least 3 points that do not lie on one line. The last point is
    /// joined to the first.
    pub points: Vec<Pt>,
    /// Replace the selection, or add to it.
    #[serde(default)]
    pub mode: RegionCombine,
    /// `overlap` selects what the path crosses, surrounds or (for a shape with a fill) lies inside; `contain`
    /// only what lies completely inside the path. Absent: the setting `input.box_select`.
    #[serde(default)]
    pub contain: Option<Contain>,
    /// Also select locked elements.
    #[serde(default)]
    pub include_locked: bool,
    /// Points of the path closer than this to the line between their neighbours are dropped, in world units (0
    /// keeps every point). A pointer path has one point per event, so the pointer tool sends 4 pixels.
    #[serde(default = "default_simplify")]
    pub simplify: f64,
}

fn lasso_of(cmd: &str, a: &SelectLassoArgs) -> Result<Lasso, Refusal> {
    if a.points.len() > MAX_POINTS {
        return Err(Refusal::limit_exceeded(cmd, a.points.len(), "points", MAX_POINTS));
    }
    let points = a
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            Ok(Point::new(
                args::world(cmd, &format!("points[{i}][0]"), p[0])?,
                args::world(cmd, &format!("points[{i}][1]"), p[1])?,
            ))
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    if !a.simplify.is_finite() || a.simplify < 0.0 {
        return Err(Refusal::bad_arg(cmd, "simplify", "must be a number of 0 or more"));
    }
    Lasso::new(&points, a.simplify).map_err(|e| match e {
        LassoError::NotFinite => Refusal::bad_arg(cmd, "points", e.to_string()),
        LassoError::TooFew { .. } | LassoError::NoArea => Refusal::bad_arg(
            cmd,
            "points",
            format!("{e}. Draw a loop around what you want to select, or give 3 or more points that make a closed shape"),
        ),
    })
}

fn select_lasso_cmd(s: &mut Session, a: SelectLassoArgs) -> Result<SelectionOut, Refusal> {
    let lasso = lasso_of(s.cmd(), &a)?;
    let contain = a.contain.unwrap_or_else(|| match s.settings.get("input.box_select").and_then(Value::as_str) {
        Some("contain") => Contain::Contain,
        _ => Contain::Overlap,
    });
    let opts = RegionOpts {
        mode: match a.mode {
            RegionCombine::Replace => RegionMode::Replace,
            RegionCombine::Add => RegionMode::Add,
        },
        contain: match contain {
            Contain::Overlap => MarqueeMode::Touch,
            Contain::Contain => MarqueeMode::Contain,
        },
        include_locked: a.include_locked,
    };
    let next = select_lasso(s.store(), &edit_selection(s), &lasso, opts);
    let before = s.tab().observed.clone();
    Ok(finish(s, &before, &next, &[]))
}

/// The lasso command of the `selection` family.
pub const COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "selection.select_lasso",
    "selection",
    "Select with a lasso",
    "Selects the elements that a free-hand loop (world points) surrounds or crosses, as a marquee does for a rectangle.",
    &Typed::new(select_lasso_cmd),
    SEL,
)
.example(r#"{"points":[[0,0],[300,20],[200,240]],"contain":"contain"}"#)];

#[cfg(test)]
#[path = "selection_lasso_tests.rs"]
mod tests;
