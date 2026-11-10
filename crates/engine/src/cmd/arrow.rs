//! `arrow`: bindings, heads, routing and point editing of lines and arrows (docs/commands.md §2.12).
//!
//! Every command validates its arguments, then delegates to `drafftink-edit`: `binding` (the solver and
//! the drag acquisition), `elbow` (the router and the pinned segments), `linear` (points, heads, path
//! type) and `arrow` (bind an end to a named target, reroute, snapping of a dragged point). A rule lives
//! in those modules, never here.
//!
//! The stored model: a binding is on the arrow only (`start_binding`, `end_binding`). A command that moves
//! an end of a bound arrow re-solves the arrow before it returns. Shapes that move or vanish are handled
//! by the commands that move or delete them (`edit::transform`, `edit::lifecycle` re-solve the arrows
//! bound to them); the engine's invariant pass is the safety net.
//!
//! Beyond the nine commands of the catalog this family has `arrow.swap_heads`, `arrow.move_segment`
//! and `arrow.release_segment`, which expose `edit::linear::swap_heads` and the elbow pin functions.

use drafftink_edit::arrow::{self as arrow_ops, ArrowError, BindRequest, SnapFlags};
use drafftink_edit::binding::{self, BindOpts, BindingError, EndChange};
use drafftink_edit::elbow::{self, ElbowEditError, ElbowError, RouteOpts};
use drafftink_edit::linear::{self, LinearError, world_points};
use drafftink_geom::binding::FIXED_POINT_BOUND;
use drafftink_geom::guides::SmartGuideKind;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_scene::{BindMode, Element, ElementId, ElementKind, Head, PathType, SceneRead, StoreError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed,
    Undo,
};
use crate::state::{BASE_ZOOM, LinearEditor};
use crate::store::DocTx;

/// A point in world units: `[x, y]`.
pub type Pt = [f64; 2];

// ---------------------------------------------------------------------------------------------
// Argument types
// ---------------------------------------------------------------------------------------------

/// Which end of an arrow a command acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndWord {
    /// The first point.
    Start,
    /// The last point.
    End,
}

impl From<EndWord> for binding::End {
    fn from(e: EndWord) -> Self {
        match e {
            EndWord::Start => binding::End::Start,
            EndWord::End => binding::End::End,
        }
    }
}

/// One end or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndsWord {
    /// The first point.
    Start,
    /// The last point.
    End,
    /// Both ends.
    Both,
}

impl EndsWord {
    fn ends(self) -> &'static [binding::End] {
        match self {
            Self::Start => &[binding::End::Start],
            Self::End => &[binding::End::End],
            Self::Both => &[binding::End::Start, binding::End::End],
        }
    }
}

/// A point of a line or arrow: its number from 0, or `"start"` / `"end"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PointIndex {
    /// The number of the point, from 0.
    Number(usize),
    /// The first or the last point.
    Word(EndWord),
}

/// How a dragged end finds a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindChoice {
    /// Bind to a target under the pointer when the setting `arrow.binding` is on.
    #[default]
    Auto,
    /// Ignore targets (Ctrl held).
    Off,
    /// Bind inside the target under the pointer (Alt held).
    Inside,
}

/// The words `snap` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapWord {
    /// Follow the settings `snap.grid`, `snap.smart_guides` and `snap.angle`.
    Auto,
    /// No snapping.
    None,
}

/// Snap switches; a missing key follows its setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SnapSwitches {
    /// Snap to the grid.
    pub grid: Option<bool>,
    /// Snap to the edges and centres of other elements.
    pub smart: Option<bool>,
    /// Lock the direction to a multiple of the angle step (`snap.angle_step`, 15 degrees).
    pub angle: Option<bool>,
}

/// `auto`, `none`, or switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Snap {
    /// `"auto"` or `"none"`.
    Word(SnapWord),
    /// `{grid?, smart?, angle?}`.
    Switches(SnapSwitches),
}

impl Default for Snap {
    fn default() -> Self {
        Snap::Word(SnapWord::Auto)
    }
}

fn some_value<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Arguments of commands that act on arrows and lines: the targets default to the selection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct IdsArgs {
    /// The arrows. Default: the selection.
    pub ids: Option<Vec<Ref>>,
}

/// Arguments of `arrow.set_type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTypeArgs {
    /// The arrows and lines. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// `sharp` straight segments, `round` a smooth curve through the points, `elbow` an orthogonal route
    /// around the bound shapes.
    #[serde(rename = "type")]
    #[schemars(rename = "type")]
    pub path: PathType,
}

/// Arguments of `arrow.set_head`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetHeadArgs {
    /// The arrows. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// Which end gets the head. Default `end`.
    #[serde(default = "end_end")]
    pub end: EndsWord,
    /// The head, for example `arrow` or `triangle`. `null` (or `"none"`) removes it. Required.
    #[serde(deserialize_with = "head_or_null")]
    #[schemars(required)]
    pub head: Option<Head>,
}

fn end_end() -> EndsWord {
    EndsWord::End
}

fn head_or_null<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Head>, D::Error> {
    Option::<Head>::deserialize(d)
}

/// Arguments of `arrow.bind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BindArgs {
    /// The arrow or line.
    pub arrow: Ref,
    /// Which end to bind.
    pub end: EndWord,
    /// The element to attach to: a rectangle, diamond, ellipse, text, image or frame.
    pub target: Ref,
    /// `orbit` keeps the end on the target's outline plus a gap; `inside` keeps it inside the target.
    #[serde(default = "orbit")]
    pub mode: BindMode,
    /// Where on the target, as ratios `[rx, ry]` of its unrotated box (0 to 1 is inside the box). Default:
    /// the outline point facing the arrow.
    #[serde(default)]
    pub fixed_point: Option<Pt>,
}

fn orbit() -> BindMode {
    BindMode::Orbit
}

/// Arguments of `arrow.unbind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnbindArgs {
    /// The arrow or line.
    pub arrow: Ref,
    /// Which end to free. Default `both`.
    #[serde(default = "both")]
    pub end: EndsWord,
}

fn both() -> EndsWord {
    EndsWord::Both
}

/// Arguments of `arrow.move_point`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MovePointArgs {
    /// The arrow or line.
    pub arrow: Ref,
    /// The point: a number from 0, or `"start"` / `"end"`. With `insert`, the segment number (segment `n`
    /// joins point `n - 1` and point `n`, counted from 1).
    pub index: PointIndex,
    /// Where the point goes, in world units.
    pub to: Pt,
    /// Insert a point at the midpoint of the segment `index` first, then move it (dragging a midpoint
    /// handle).
    #[serde(default)]
    pub insert: bool,
    /// How an end finds a target: `auto`, `off` (Ctrl) or `inside` (Alt).
    #[serde(default)]
    pub bind: BindChoice,
    /// Snapping of the point.
    #[serde(default)]
    pub snap: Snap,
}

/// Arguments of `arrow.add_point`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddPointArgs {
    /// The arrow or line.
    pub arrow: Ref,
    /// The segment, counted from 1: segment `n` joins point `n - 1` and point `n`.
    pub segment: usize,
    /// Where the new point goes. Default: the middle of the segment.
    #[serde(default)]
    pub at: Option<Pt>,
}

/// Arguments of `arrow.delete_point`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct DeletePointArgs {
    /// The arrow or line. Default: the one in the point editor.
    pub arrow: Option<Ref>,
    /// The point to remove, from 0. Default: the points selected in the point editor.
    pub index: Option<usize>,
}

/// Arguments of `arrow.edit_points`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct EditPointsArgs {
    /// The arrow or line. Default: the single selected one.
    pub arrow: Option<Ref>,
    /// Turn the point editor on or off. Omitted: toggle, except that `select_point` alone turns it on.
    pub on: Option<bool>,
    /// Select the point with this number, or clear the selected point with `null`. Absent: unchanged.
    #[serde(deserialize_with = "some_value")]
    #[schemars(with = "Option<usize>")]
    pub select_point: Option<Option<usize>>,
}

/// Arguments of `arrow.move_segment` and `arrow.release_segment`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SegmentArgs {
    /// The elbow arrow.
    pub arrow: Ref,
    /// The segment, counted from 1: segment `n` joins point `n - 1` and point `n`.
    pub segment: usize,
    /// Where the segment goes, in world units (`arrow.move_segment` only). The segment is moved across its
    /// axis to pass through this point and stays pinned there.
    #[serde(default)]
    pub to: Option<Pt>,
}

// ---------------------------------------------------------------------------------------------
// Outputs
// ---------------------------------------------------------------------------------------------

/// The arrows a command acted on.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct IdsOut {
    /// Ids of the arrows and lines that were targeted.
    pub ids: Vec<String>,
}

/// Output of `arrow.bind`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct BindOut {
    /// The binding that was stored.
    pub binding: BoundTo,
}

/// A stored binding.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct BoundTo {
    /// The target.
    pub target: String,
    /// `inside` or `orbit`.
    pub mode: BindMode,
    /// Where on the target, as ratios of its unrotated box.
    pub fixed_point: Pt,
}

/// Output of `arrow.unbind`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct UnbindOut {
    /// The ends that were bound and are free now.
    pub freed: Vec<EndWord>,
}

/// A guide line for the overlay.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct GuideOut {
    /// `vertical`, `horizontal`, `equal_spacing_h` or `equal_spacing_v`.
    pub kind: &'static str,
    /// x of a vertical guide, y of a horizontal one.
    pub position: f64,
    /// Start along the guide.
    pub start: f64,
    /// End along the guide.
    pub end: f64,
}

/// Output of `arrow.move_point`, also the frame output of its gesture.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct MovePointOut {
    /// The number of the point that moved (the new point with `insert`).
    pub index: usize,
    /// Where the point is now, in world units.
    pub point: Pt,
    /// The target the end is bound to now, for the overlay to highlight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_highlight: Option<String>,
    /// Smart guides to draw.
    pub guides: Vec<GuideOut>,
}

/// Output of `arrow.add_point`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AddPointOut {
    /// The number of the new point.
    pub index: usize,
}

/// Output of `arrow.delete_point`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DeletePointOut {
    /// How many points were removed.
    pub removed: usize,
}

/// Output of `arrow.edit_points`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct EditPointsOut {
    /// False when the point editor was already as requested.
    pub changed: bool,
    /// The arrow in the point editor, `null` when it is off.
    pub arrow: Option<String>,
    /// The selected points.
    pub selected_points: Vec<usize>,
}

/// Output of `arrow.reroute`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RerouteOut {
    /// Ids of the targeted lines and arrows.
    pub ids: Vec<String>,
    /// Ids whose geometry changed.
    pub rerouted: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Inside a document edit: either a refusal or a store failure.
enum Fail {
    Refused(Refusal),
    Store(StoreError),
}

impl From<StoreError> for Fail {
    fn from(e: StoreError) -> Self {
        Fail::Store(e)
    }
}

impl From<Refusal> for Fail {
    fn from(r: Refusal) -> Self {
        Fail::Refused(r)
    }
}

fn edit<R>(s: &mut Session, f: impl FnOnce(&mut DocTx<'_, '_>) -> Result<R, Fail>) -> Result<R, Refusal> {
    let cmd = s.cmd();
    match s.tab_mut().doc.edit::<R, Fail>(f) {
        Ok((r, _)) => Ok(r),
        Err(Fail::Refused(r)) => Err(r),
        Err(Fail::Store(e)) => Err(Refusal::internal(cmd, e.to_string())),
    }
}

const NOT_MOVABLE: &str =
    "Elbow arrows only allow moving their two ends. Move the whole arrow, or change its type with `arrow.set_type`.";

fn not_movable(cmd: &str) -> Refusal {
    Refusal::custom("not_movable", cmd, NOT_MOVABLE).suggesting("arrow.set_type", None)
}

fn not_elbow(cmd: &str, id: &str) -> Refusal {
    Refusal::wrong_kind(cmd, "sharp or round arrow", "elbow arrows")
        .with_details(serde_json::json!({ "ids": [id] }))
        .suggesting("arrow.set_type", Some(serde_json::json!({ "ids": [id], "type": "elbow" })))
}

fn pt(cmd: &str, key: &str, p: Pt) -> Result<Point, Refusal> {
    Ok(Point::new(
        args::world(cmd, &format!("{key}[0]"), p[0])?,
        args::world(cmd, &format!("{key}[1]"), p[1])?,
    ))
}

fn no_such_point(cmd: &str, key: &str, index: usize, len: usize) -> Refusal {
    Refusal::bad_arg(
        cmd,
        key,
        format!("point {index} does not exist; this element has {len} points (0 to {})", len.saturating_sub(1)),
    )
}

fn no_such_segment(cmd: &str, key: &str, segment: usize, len: usize) -> Refusal {
    Refusal::bad_arg(
        cmd,
        key,
        format!("segment {segment} does not exist; this element has segments 1 to {}", len.saturating_sub(1)),
    )
}

fn from_binding(cmd: &str, key: &str, e: BindingError) -> Fail {
    match e {
        BindingError::Missing(id) => Refusal::not_found(cmd, id.as_str()).into(),
        BindingError::NotLinear(id) => {
            Refusal::wrong_kind(cmd, "non-linear", "arrows and lines").with_details(serde_json::json!({ "ids": [id.as_str()] })).into()
        }
        BindingError::NotFinite => Refusal::bad_arg(cmd, key, "must be a finite number").into(),
        BindingError::Store(e) => Fail::Store(e),
        other @ (BindingError::TooFewPoints(_) | BindingError::Elbow { .. }) => {
            Refusal::internal(cmd, other.to_string()).into()
        }
    }
}

fn from_elbow(cmd: &str, key: &str, e: ElbowEditError) -> Fail {
    match e {
        ElbowEditError::Missing(id) => Refusal::not_found(cmd, id.as_str()).into(),
        ElbowEditError::NotElbow(id) => not_elbow(cmd, id.as_str()).into(),
        ElbowEditError::Store(e) => Fail::Store(e),
        ElbowEditError::Route(ElbowError::NoSuchSegment { index, last }) => {
            Refusal::bad_arg(cmd, key, format!("segment {index} does not exist; this arrow has segments 1 to {last}"))
                .into()
        }
        ElbowEditError::Route(ElbowError::NotOrthogonal(i)) => Refusal::bad_arg(
            cmd,
            key,
            format!("segment {i} is not horizontal or vertical, so it cannot be moved as an elbow segment"),
        )
        .into(),
        ElbowEditError::Route(ElbowError::EndSegment) => Refusal::bad_arg(
            cmd,
            key,
            "the first and last segment of an elbow arrow cannot be pinned. Move the arrow's end with `arrow.move_point` instead",
        )
        .suggesting("arrow.move_point", None)
        .into(),
        ElbowEditError::Route(other) => Refusal::internal(cmd, other.to_string()).into(),
    }
}

fn from_linear(cmd: &str, key: &str, e: LinearError) -> Fail {
    match e {
        LinearError::Missing(id) => Refusal::not_found(cmd, id.as_str()).into(),
        LinearError::NotLinear(id) => {
            Refusal::wrong_kind(cmd, "non-linear", "arrows and lines").with_details(serde_json::json!({ "ids": [id.as_str()] })).into()
        }
        LinearError::NoSuchPoint { index, len } => no_such_point(cmd, key, index, len).into(),
        LinearError::TooFewPoints { kind, min, have } => {
            let text = if min <= 2 {
                format!("A {kind} needs at least 2 points, so no point can be deleted from it.")
            } else {
                format!("A closed {kind} needs at least {min} points and this would leave {have}, so no point can be deleted.")
            };
            Refusal::custom("too_few", cmd, text).into()
        }
        LinearError::NotFinite => Refusal::bad_arg(cmd, key, "must be a finite number").into(),
        LinearError::ElbowInteriorPoint => not_movable(cmd).into(),
        LinearError::ClosedElbow => Refusal::bad_arg(
            cmd,
            key,
            "a closed line cannot be an elbow. Open the line first, or choose `sharp` or `round`",
        )
        .into(),
        LinearError::Elbow(e) => from_elbow(cmd, key, e),
        LinearError::Binding(e) => from_binding(cmd, key, e),
        LinearError::Store(e) => Fail::Store(e),
    }
}

fn from_arrow(cmd: &str, e: ArrowError) -> Fail {
    match e {
        ArrowError::Missing(id) => Refusal::not_found(cmd, id.as_str()).into(),
        ArrowError::NotLinear(id) => {
            Refusal::wrong_kind(cmd, "non-linear", "arrows and lines").with_details(serde_json::json!({ "ids": [id.as_str()] })).into()
        }
        ArrowError::NotBindable { id, kind, label } => not_bindable(cmd, id.as_str(), kind, label).into(),
        ArrowError::NotFinite => Refusal::bad_arg(cmd, "fixed_point", "must be two finite numbers").into(),
        ArrowError::Binding(e) => from_binding(cmd, "to", e),
        ArrowError::Elbow(e) => from_elbow(cmd, "to", e),
        ArrowError::Store(e) => Fail::Store(e),
    }
}

fn not_bindable(cmd: &str, id: &str, kind: &str, label: bool) -> Refusal {
    let text = if label {
        "Text that labels another element cannot be an arrow target. Bind to the labelled element instead.".to_owned()
    } else {
        format!(
            "{} elements cannot be arrow targets. Bindable kinds: rectangle, diamond, ellipse, text, image, frame.",
            capitalise(kind)
        )
    };
    Refusal::custom("not_bindable", cmd, text).with_details(serde_json::json!({ "ids": [id] }))
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// A live line or arrow that is not locked.
fn linear_target(s: &Session, cmd: &str, id: &ElementId) -> Result<Element, Refusal> {
    let e = s.element(id).ok_or_else(|| Refusal::not_found(cmd, id.as_str()))?;
    if !e.tag().is_linear() {
        return Err(Refusal::wrong_kind(cmd, e.tag().name(), "arrows and lines")
            .with_details(serde_json::json!({ "ids": [id.as_str()] })));
    }
    if e.locked {
        return Err(Refusal::locked(cmd, id.as_str()));
    }
    Ok(e.clone())
}

/// The targets of a multi-target command that the command applies to (`keep` decides), in the order given.
/// A locked applicable target refuses the whole command. `none` is the `not_applicable` refusal.
fn applicable(
    s: &Session,
    cmd: &str,
    ids: Option<&[Ref]>,
    keep: impl Fn(&Element) -> bool,
    none: impl FnOnce() -> Refusal,
) -> Result<Vec<ElementId>, Refusal> {
    let all = s.targets(ids)?;
    let mut out = Vec::new();
    for id in all {
        let Some(e) = s.element(&id) else { continue };
        if !keep(e) {
            continue;
        }
        if e.locked {
            return Err(Refusal::locked(cmd, id.as_str()));
        }
        out.push(id);
    }
    if out.is_empty() { Err(none()) } else { Ok(out) }
}

fn is_arrow(e: &Element) -> bool {
    matches!(e.kind, ElementKind::Arrow(_))
}

fn names(ids: &[ElementId]) -> Vec<String> {
    ids.iter().map(|i| i.as_str().to_owned()).collect()
}

/// The displayed zoom (1 is 100%), which the binding reach and the elbow router are written for.
fn zoom(s: &Session) -> f64 {
    s.view().zoom / BASE_ZOOM
}

// ---------------------------------------------------------------------------------------------
// arrow.set_type
// ---------------------------------------------------------------------------------------------

fn set_type(s: &mut Session, a: SetTypeArgs) -> Result<IdsOut, Refusal> {
    let cmd = s.cmd();
    let ids = applicable(
        s,
        cmd,
        a.ids.as_deref(),
        |e| e.tag().is_linear(),
        || Refusal::not_applicable(cmd, "an arrow type", "arrows and lines"),
    )?;
    let path = a.path;
    edit(s, |tx| {
        for id in &ids {
            linear::set_path(tx, id, path).map_err(|e| from_linear(cmd, "type", e))?;
        }
        Ok(())
    })?;
    Ok(IdsOut { ids: names(&ids) })
}

// ---------------------------------------------------------------------------------------------
// arrow.set_head, arrow.swap_heads
// ---------------------------------------------------------------------------------------------

fn set_head(s: &mut Session, a: SetHeadArgs) -> Result<IdsOut, Refusal> {
    let cmd = s.cmd();
    let ids = applicable(s, cmd, a.ids.as_deref(), is_arrow, || {
        Refusal::not_applicable(cmd, "arrowheads", "arrows (lines have no heads)")
    })?;
    let head = a.head.unwrap_or(Head::None);
    let ends = a.end.ends();
    edit(s, |tx| {
        for id in &ids {
            for end in ends {
                linear::set_head(tx, id, *end, head).map_err(|e| from_linear(cmd, "end", e))?;
            }
        }
        Ok(())
    })?;
    Ok(IdsOut { ids: names(&ids) })
}

fn swap_heads(s: &mut Session, a: IdsArgs) -> Result<IdsOut, Refusal> {
    let cmd = s.cmd();
    let ids = applicable(s, cmd, a.ids.as_deref(), is_arrow, || {
        Refusal::not_applicable(cmd, "arrowheads", "arrows (lines have no heads)")
    })?;
    edit(s, |tx| {
        for id in &ids {
            linear::swap_heads(tx, id).map_err(|e| from_linear(cmd, "ids", e))?;
        }
        Ok(())
    })?;
    Ok(IdsOut { ids: names(&ids) })
}

// ---------------------------------------------------------------------------------------------
// arrow.bind, arrow.unbind
// ---------------------------------------------------------------------------------------------

fn bind(s: &mut Session, a: BindArgs) -> Result<BindOut, Refusal> {
    let cmd = s.cmd();
    let arrow = s.resolve_ref(&a.arrow)?;
    let target = s.resolve_ref(&a.target)?;
    linear_target(s, cmd, &arrow)?;
    if let Some(t) = s.element(&target)
        && t.locked
    {
        return Err(Refusal::locked(cmd, target.as_str()));
    }
    if let Some(v) = a.fixed_point {
        for (i, x) in v.iter().enumerate() {
            if !(x.is_finite() && x.abs() <= FIXED_POINT_BOUND) {
                return Err(Refusal::bad_arg(
                    cmd,
                    &format!("fixed_point[{i}]"),
                    format!("must be a ratio of the target's box between -{FIXED_POINT_BOUND} and {FIXED_POINT_BOUND}"),
                ));
            }
        }
    }
    let end: binding::End = a.end.into();
    let midpoint_snap = s.settings.bool_or("arrow.midpoint_snap", false);
    let z = zoom(s);
    let mut note = None;
    if a.mode == BindMode::Inside
        && s.element(&arrow).and_then(Element::linear_data).is_some_and(|d| d.path == PathType::Elbow)
    {
        note = Some("Elbow arrows always bind in orbit mode, so `mode: inside` was ignored.");
    }
    let stored = edit(s, |tx| {
        let req = BindRequest { target: &target, mode: a.mode, fixed_point: a.fixed_point, zoom: z, midpoint_snap };
        arrow_ops::bind_end(tx, &arrow, end, &req).map_err(|e| from_arrow(cmd, e))
    })?;
    if let Some(n) = note {
        s.warn(n);
    }
    Ok(BindOut {
        binding: BoundTo { target: stored.target.as_str().to_owned(), mode: stored.mode, fixed_point: stored.fixed_point },
    })
}

fn unbind(s: &mut Session, a: UnbindArgs) -> Result<UnbindOut, Refusal> {
    let cmd = s.cmd();
    let id = s.resolve_ref(&a.arrow)?;
    let el = linear_target(s, cmd, &id)?;
    let (start, end) = el.linear_data().map_or((false, false), |d| (d.start_binding.is_some(), d.end_binding.is_some()));
    let wanted = a.end.ends();
    let bound: Vec<binding::End> = wanted
        .iter()
        .copied()
        .filter(|e| match e {
            binding::End::Start => start,
            binding::End::End => end,
        })
        .collect();
    if bound.is_empty() {
        let reason = if wanted.len() > 1 {
            format!("`{}` has no bound ends", id.as_str())
        } else {
            format!("`{}` is not bound at that end", id.as_str())
        };
        return Err(Refusal::nothing_to_do(cmd, "unbind", &reason));
    }
    edit(s, |tx| {
        for e in &bound {
            binding::unbind_end(tx, &id, *e).map_err(|err| from_binding(cmd, "end", err))?;
        }
        Ok(())
    })?;
    let freed = bound
        .iter()
        .map(|e| match e {
            binding::End::Start => EndWord::Start,
            binding::End::End => EndWord::End,
        })
        .collect();
    Ok(UnbindOut { freed })
}

// ---------------------------------------------------------------------------------------------
// arrow.move_point
// ---------------------------------------------------------------------------------------------

fn snap_flags(s: &Session, snap: &Snap) -> SnapFlags {
    let setting = SnapFlags {
        grid: s.settings.bool_or("snap.grid", false),
        smart: s.settings.bool_or("snap.smart_guides", false),
        angle: s.settings.bool_or("snap.angle", false),
    };
    match snap {
        Snap::Word(SnapWord::Auto) => setting,
        Snap::Word(SnapWord::None) => SnapFlags::default(),
        Snap::Switches(w) => SnapFlags {
            grid: w.grid.unwrap_or(setting.grid),
            smart: w.smart.unwrap_or(setting.smart),
            angle: w.angle.unwrap_or(setting.angle),
        },
    }
}

fn guide_out(g: &drafftink_geom::guides::SmartGuide) -> GuideOut {
    GuideOut {
        kind: match g.kind {
            SmartGuideKind::Vertical => "vertical",
            SmartGuideKind::Horizontal => "horizontal",
            SmartGuideKind::EqualSpacingH => "equal_spacing_h",
            SmartGuideKind::EqualSpacingV => "equal_spacing_v",
        },
        position: g.position,
        start: g.start,
        end: g.end,
    }
}

fn move_point(s: &mut Session, a: MovePointArgs) -> Result<MovePointOut, Refusal> {
    let cmd = s.cmd();
    let to = pt(cmd, "to", a.to)?;
    let id = s.resolve_ref(&a.arrow)?;
    let el = linear_target(s, cmd, &id)?;
    let world = world_points(&el).unwrap_or_default();
    let n = world.len();
    let elbow = el.linear_data().is_some_and(|d| d.path == PathType::Elbow);

    // Which point, and whether a point is inserted first.
    let index = if a.insert {
        let segment = match a.index {
            PointIndex::Number(i) => i,
            PointIndex::Word(_) => {
                return Err(Refusal::bad_arg(
                    cmd,
                    "index",
                    "with insert=true, index is a segment number, not \"start\" or \"end\"",
                ));
            }
        };
        if elbow {
            return Err(not_movable(cmd));
        }
        if segment < 1 || segment >= n {
            return Err(no_such_segment(cmd, "index", segment, n));
        }
        segment
    } else {
        let i = match a.index {
            PointIndex::Word(EndWord::Start) => 0,
            PointIndex::Word(EndWord::End) => n.saturating_sub(1),
            PointIndex::Number(i) => i,
        };
        if i >= n {
            return Err(no_such_point(cmd, "index", i, n));
        }
        if elbow && i != 0 && i + 1 != n {
            return Err(not_movable(cmd));
        }
        i
    };
    let is_end = !a.insert && (index == 0 || index + 1 == n);

    // Snapping: guides and angle use the neighbouring point as the origin.
    let flags = snap_flags(s, &a.snap);
    let step = {
        let v = s.settings.f64_or("snap.angle_step", 15.0);
        if v.is_finite() && v > 0.0 { v } else { 15.0 }
    };
    let reference = if a.insert || index > 0 { world.get(index.saturating_sub(1)) } else { world.get(1) }.copied();
    let snapped = if flags.grid || flags.smart || flags.angle {
        let mut others = arrow_ops::snap_candidates(s.store(), std::slice::from_ref(&id), to);
        for (i, p) in world.iter().enumerate() {
            if a.insert || i != index {
                others.push(Rect::new(p.x, p.y, p.x, p.y));
            }
        }
        arrow_ops::snap_dragged_point(to, reference, &others, flags, step)
    } else {
        arrow_ops::PointSnap { point: to, guides: Vec::new(), angle: None }
    };

    let opts = BindOpts {
        zoom: zoom(s),
        force_inside: a.bind == BindChoice::Inside,
        disabled: a.bind == BindChoice::Off || (a.bind == BindChoice::Auto && !s.settings.bool_or("arrow.binding", true)),
        midpoint_snap: s.settings.bool_or("arrow.midpoint_snap", false),
        angle_locked: flags.angle,
    };
    let insert_at = if a.insert {
        let (p, q) = (world.get(index - 1).copied(), world.get(index).copied());
        p.zip(q).map(|(p, q)| p.midpoint(q))
    } else {
        None
    };

    let (point, highlight) = edit(s, |tx| {
        if let Some(mid) = insert_at {
            linear::insert_midpoint(tx, &id, index, mid).map_err(|e| from_linear(cmd, "index", e))?;
        }
        let mut highlight = None;
        if is_end {
            let end = if index == 0 { binding::End::Start } else { binding::End::End };
            let now = tx.get(&id).cloned().ok_or_else(|| Refusal::not_found(cmd, id.as_str()))?;
            let plan = binding::plan_end_drag(tx, &now, end, to, &opts);
            let at = if matches!(plan.dragged, EndChange::Bind(_)) { to } else { snapped.point };
            highlight =
                binding::drag_end(tx, &id, end, at, &opts).map_err(|e| from_binding(cmd, "to", e))?.map(|c| c.target);
        } else {
            linear::move_points(tx, &id, &[(index, snapped.point)]).map_err(|e| from_linear(cmd, "to", e))?;
        }
        let point = tx.get(&id).and_then(world_points).and_then(|w| w.get(index).copied()).unwrap_or(to);
        Ok((point, highlight))
    })?;
    Ok(MovePointOut {
        index,
        point: [point.x, point.y],
        bind_highlight: highlight.map(|h| h.as_str().to_owned()),
        guides: snapped.guides.iter().map(guide_out).collect(),
    })
}

// ---------------------------------------------------------------------------------------------
// arrow.add_point, arrow.delete_point
// ---------------------------------------------------------------------------------------------

fn add_point(s: &mut Session, a: AddPointArgs) -> Result<AddPointOut, Refusal> {
    let cmd = s.cmd();
    let at = a.at.map(|p| pt(cmd, "at", p)).transpose()?;
    let id = s.resolve_ref(&a.arrow)?;
    let el = linear_target(s, cmd, &id)?;
    let world = world_points(&el).unwrap_or_default();
    if el.linear_data().is_some_and(|d| d.path == PathType::Elbow) {
        return Err(not_movable(cmd));
    }
    let seg = a.segment;
    if seg < 1 || seg >= world.len() {
        return Err(no_such_segment(cmd, "segment", seg, world.len()));
    }
    let mid = world.get(seg - 1).copied().zip(world.get(seg).copied()).map(|(p, q)| p.midpoint(q));
    let place = at.or(mid).ok_or_else(|| no_such_segment(cmd, "segment", seg, world.len()))?;
    let index = edit(s, |tx| linear::insert_midpoint(tx, &id, seg, place).map_err(|e| from_linear(cmd, "segment", e)))?;
    Ok(AddPointOut { index })
}

fn delete_point(s: &mut Session, a: DeletePointArgs) -> Result<DeletePointOut, Refusal> {
    let cmd = s.cmd();
    let editor = s.tab().observed.linear_editor.clone();
    let id = match (&a.arrow, &editor) {
        (Some(r), _) => s.resolve_ref(r)?,
        (None, Some(e)) => s.resolve_ref(&Ref(e.element.as_str().to_owned()))?,
        (None, None) => {
            return Err(Refusal::nothing_to_do(
                cmd,
                "delete",
                "the point editor is not open, so pass `arrow` and `index`, or open it with `arrow.edit_points`",
            ));
        }
    };
    let el = linear_target(s, cmd, &id)?;
    let indices: Vec<usize> = match (a.index, &editor) {
        (Some(i), _) => vec![i],
        (None, Some(e)) if e.element == id => e.selected_points.iter().copied().collect(),
        (None, _) => Vec::new(),
    };
    if indices.is_empty() {
        return Err(Refusal::nothing_to_do(
            cmd,
            "delete",
            "no point is selected in the point editor, so pass `index` or select a point with `arrow.edit_points`",
        ));
    }
    let len = world_points(&el).map_or(0, |w| w.len());
    if let Some(bad) = indices.iter().find(|i| **i >= len) {
        return Err(no_such_point(cmd, "index", *bad, len));
    }
    if el.linear_data().is_some_and(|d| d.path == PathType::Elbow) {
        return Err(not_movable(cmd));
    }
    let deleted = edit(s, |tx| {
        linear::delete_points(tx, &id, &indices).map_err(|e| match e {
            LinearError::TooFewPoints { kind, min, have } => {
                let text = if min <= 2 {
                    format!("A {kind} needs at least 2 points, so no point can be deleted from `{}`.", id.as_str())
                } else {
                    format!(
                        "A closed {kind} needs at least {min} points, and this would leave {have}, so no point can be deleted from `{}`.",
                        id.as_str()
                    )
                };
                Fail::Refused(Refusal::custom("too_few", cmd, text))
            }
            other => from_linear(cmd, "index", other),
        })
    })?;
    if let Some(ed) = s.tab_mut().observed.linear_editor.as_mut()
        && ed.element == id
    {
        ed.selected_points.clear();
    }
    Ok(DeletePointOut { removed: deleted.removed })
}

// ---------------------------------------------------------------------------------------------
// arrow.edit_points
// ---------------------------------------------------------------------------------------------

fn edit_points_available(s: &Session) -> Result<(), Refusal> {
    if s.tab().observed.linear_editor.is_some() {
        return Ok(());
    }
    let have = s.selection().len();
    if have == 1 { Ok(()) } else { Err(Refusal::needs_single("arrow.edit_points", "line or arrow", have)) }
}

fn edit_points(s: &mut Session, a: EditPointsArgs) -> Result<EditPointsOut, Refusal> {
    let cmd = s.cmd();
    let before = s.tab().observed.linear_editor.clone();
    let id = match &a.arrow {
        Some(r) => s.resolve_ref(r)?,
        None => {
            let live: Vec<ElementId> = s.selection().iter().filter(|i| s.element(i).is_some()).cloned().collect();
            match (live.as_slice(), &before) {
                ([one], _) => one.clone(),
                (_, Some(e)) => e.element.clone(),
                _ => return Err(Refusal::needs_single(cmd, "line or arrow", live.len())),
            }
        }
    };
    let el = s.element(&id).ok_or_else(|| Refusal::not_found(cmd, id.as_str()))?.clone();
    if !el.tag().is_linear() {
        return Err(Refusal::wrong_kind(cmd, el.tag().name(), "arrows and lines")
            .with_details(serde_json::json!({ "ids": [id.as_str()] })));
    }
    let len = world_points(&el).map_or(0, |w| w.len());
    let active_here = before.as_ref().is_some_and(|e| e.element == id);
    let on = a.on.unwrap_or(a.select_point.is_some() || !active_here);
    if let Some(Some(i)) = a.select_point {
        if !on {
            return Err(Refusal::bad_arg(cmd, "select_point", "the point editor is off, so there is no point to select. Pass on=true"));
        }
        if i >= len {
            return Err(no_such_point(cmd, "select_point", i, len));
        }
    }
    let obs = &mut s.tab_mut().observed;
    if on {
        let mut ed = match &before {
            Some(e) if e.element == id => e.clone(),
            _ => LinearEditor { element: id.clone(), selected_points: Default::default() },
        };
        match a.select_point {
            Some(Some(i)) => {
                ed.selected_points.clear();
                ed.selected_points.insert(i);
            }
            Some(None) => ed.selected_points.clear(),
            None => {}
        }
        obs.selection.clear();
        obs.selection.insert(id.clone());
        obs.selected_groups.clear();
        obs.linear_editor = Some(ed);
    } else {
        obs.linear_editor = None;
    }
    let after = s.tab().observed.linear_editor.clone();
    Ok(EditPointsOut {
        changed: after != before,
        arrow: after.as_ref().map(|e| e.element.as_str().to_owned()),
        selected_points: after.map(|e| e.selected_points.into_iter().collect()).unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------------------------
// arrow.reroute
// ---------------------------------------------------------------------------------------------

fn reroute(s: &mut Session, a: IdsArgs) -> Result<RerouteOut, Refusal> {
    let cmd = s.cmd();
    let ids = applicable(
        s,
        cmd,
        a.ids.as_deref(),
        |e| e.tag().is_linear(),
        || Refusal::not_applicable(cmd, "re-routing", "arrows and lines"),
    )?;
    let z = zoom(s);
    let changed = edit(s, |tx| arrow_ops::reroute(tx, &ids, z).map_err(|e| from_arrow(cmd, e)))?;
    Ok(RerouteOut { ids: names(&ids), rerouted: names(&changed) })
}

// ---------------------------------------------------------------------------------------------
// arrow.move_segment, arrow.release_segment
// ---------------------------------------------------------------------------------------------

fn elbow_target(s: &Session, cmd: &str, a: &SegmentArgs) -> Result<(ElementId, usize), Refusal> {
    let id = s.resolve_ref(&a.arrow)?;
    let el = linear_target(s, cmd, &id)?;
    if el.linear_data().is_none_or(|d| d.path != PathType::Elbow) {
        return Err(not_elbow(cmd, id.as_str()));
    }
    let len = world_points(&el).map_or(0, |w| w.len());
    if a.segment < 1 || a.segment >= len {
        return Err(no_such_segment(cmd, "segment", a.segment, len));
    }
    Ok((id, len))
}

fn move_segment(s: &mut Session, a: SegmentArgs) -> Result<IdsOut, Refusal> {
    let cmd = s.cmd();
    let Some(to) = a.to else {
        return Err(Refusal::bad_arg(cmd, "to", "is required: the point the segment should pass through"));
    };
    let to = pt(cmd, "to", to)?;
    let (id, _) = elbow_target(s, cmd, &a)?;
    let opts = RouteOpts { zoom: zoom(s) };
    edit(s, |tx| elbow::move_segment_of(tx, &id, a.segment, to, opts).map_err(|e| from_elbow(cmd, "segment", e)))?;
    Ok(IdsOut { ids: vec![id.as_str().to_owned()] })
}

fn release_segment(s: &mut Session, a: SegmentArgs) -> Result<IdsOut, Refusal> {
    let cmd = s.cmd();
    if a.to.is_some() {
        return Err(Refusal::bad_arg(cmd, "to", "is not used by `arrow.release_segment`; omit it"));
    }
    let (id, _) = elbow_target(s, cmd, &a)?;
    let pinned = s
        .element(&id)
        .and_then(Element::linear_data)
        .is_some_and(|d| d.fixed_segments.iter().any(|f| f.index as usize == a.segment));
    if !pinned {
        return Err(Refusal::nothing_to_do(
            cmd,
            "release",
            &format!("segment {} of `{}` is not pinned", a.segment, id.as_str()),
        ));
    }
    let opts = RouteOpts { zoom: zoom(s) };
    edit(s, |tx| elbow::release_segment_of(tx, &id, a.segment, opts).map_err(|e| from_elbow(cmd, "segment", e)))?;
    Ok(IdsOut { ids: vec![id.as_str().to_owned()] })
}

// ---------------------------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------------------------

/// Edits the document: undoable, refused in view-only mode, during a gesture, and commits a text edit first.
const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC,
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

const ON_SELECTION: Facts = Facts { needs_selection: Selection::Required, palette: true, ..DOC };
const GESTURE: Facts = Facts { undo: Undo::Gesture, ..DOC };

const EDIT_POINTS_KEYS: &[Binding] = &[Binding::new(&["Mod+Enter"], Scope::Canvas)];

/// The commands of the `arrow` family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "arrow.set_type",
        "arrow",
        "Set arrow type",
        "Sets how the points of arrows and lines are joined: sharp, round or elbow.",
        &Typed::new(set_type),
        ON_SELECTION,
    )
    .example(r#"{"ids":["a1"],"type":"elbow"}"#),
    CommandSpec::new(
        "arrow.set_head",
        "arrow",
        "Set arrowhead",
        "Sets or removes the arrowhead at the start, the end or both ends of arrows.",
        &Typed::new(set_head),
        ON_SELECTION,
    )
    .example(r#"{"ids":["a1"],"end":"both","head":"triangle"}"#),
    CommandSpec::new(
        "arrow.swap_heads",
        "arrow",
        "Swap arrowheads",
        "Swaps the arrowheads of the two ends of arrows.",
        &Typed::new(swap_heads),
        ON_SELECTION,
    ),
    CommandSpec::new(
        "arrow.bind",
        "arrow",
        "Bind arrow end",
        "Attaches one end of an arrow or line to a shape so that the end follows the shape.",
        &Typed::new(bind),
        DOC,
    )
    .refusals(&["not_bindable"])
    .example(r#"{"arrow":"a1","end":"end","target":"r2","mode":"orbit"}"#),
    CommandSpec::new(
        "arrow.unbind",
        "arrow",
        "Unbind arrow end",
        "Frees one or both ends of an arrow or line from the shapes they are attached to.",
        &Typed::new(unbind),
        DOC,
    )
    .example(r#"{"arrow":"a1","end":"both"}"#),
    CommandSpec::new(
        "arrow.move_point",
        "arrow",
        "Move arrow point",
        "Moves one point of an arrow or line, binding an end to the shape under it and snapping as asked.",
        &Typed::new(move_point),
        GESTURE,
    )
    .refusals(&["not_movable"])
    .example(r#"{"arrow":"a1","index":"end","to":[300,120],"bind":"auto"}"#),
    CommandSpec::new(
        "arrow.add_point",
        "arrow",
        "Add arrow point",
        "Inserts a vertex on one segment of an arrow or line, by default at its midpoint.",
        &Typed::new(add_point),
        DOC,
    )
    .refusals(&["not_movable"])
    .example(r#"{"arrow":"a1","segment":1}"#),
    CommandSpec::new(
        "arrow.delete_point",
        "arrow",
        "Delete arrow point",
        "Removes a point of an arrow or line, by default the point selected in the point editor.",
        &Typed::new(delete_point),
        DOC,
    )
    .refusals(&["not_movable"]),
    CommandSpec::new(
        "arrow.edit_points",
        "arrow",
        "Edit points",
        "Turns the point editor of a line or arrow on or off and selects a point in it.",
        &Typed::new(edit_points),
        Facts { undo: Undo::NoUndo, mutates: Mutates::OBSERVED, needs_selection: Selection::Soft, ..DOC },
    )
    .default_targets(edit_points_available)
    .shortcuts(EDIT_POINTS_KEYS),
    CommandSpec::new(
        "arrow.reroute",
        "arrow",
        "Re-route arrows",
        "Clears the pinned elbow segments, re-solves the bindings and routes arrows again.",
        &Typed::new(reroute),
        ON_SELECTION,
    ),
    CommandSpec::new(
        "arrow.move_segment",
        "arrow",
        "Move elbow segment",
        "Drags one segment of an elbow arrow across its axis and pins it there.",
        &Typed::new(move_segment),
        GESTURE,
    )
    .example(r#"{"arrow":"a1","segment":2,"to":[200,80]}"#),
    CommandSpec::new(
        "arrow.release_segment",
        "arrow",
        "Release elbow segment",
        "Unpins a segment of an elbow arrow and routes that part of the arrow again.",
        &Typed::new(release_segment),
        DOC,
    )
    .example(r#"{"arrow":"a1","segment":2}"#),
];

#[cfg(test)]
#[path = "arrow_tests.rs"]
mod tests;
