//! `element` (part 1): delete, duplicate, move, nudge, resize, set_bounds, rotate, flip, lock, unlock_all
//! and erase (docs/commands.md §2.4).
//!
//! `element.create`, `connect`, `relabel` and `set_name` are in `element_create.rs`, and `element.query`
//! and `element.at` in `element_query.rs`; all three files register commands of the family `element`.
//!
//! Every command validates its arguments and delegates to `drafftink-edit` (`transform`, `lifecycle`,
//! `arrange`, `frame`). The transform rule of §2.4 (companions follow, bindings, followers,
//! locked elements refuse the whole call) is implemented there, never here. The selection is part of the
//! observed editor state: after a delete or a lock it is pruned, after a duplicate it becomes the copies, and
//! undo restores it with the step.

use std::collections::BTreeSet;

use drafftink_edit::arrange::{self, FlipAxis};
use drafftink_edit::arrow::SnapFlags;
use drafftink_edit::frame::{self, FrameError};
use drafftink_edit::group::units::{Reverse, primaries, with_companions};
use drafftink_edit::lifecycle::{self, ChildPolicy, DuplicateOpts, LifecycleError};
use drafftink_edit::transform::{self, CommonBounds, NotRotatableReason, ResizeSpec, RotateMode, TransformError, geometry};
use drafftink_geom::guides::{SmartGuide, SmartGuideKind, detect_smart_guides, detect_smart_guides_for_point};
use drafftink_geom::kurbo::{Point, Rect, Vec2};
use drafftink_geom::select::{BoxHandle, Corner, Edge};
use drafftink_geom::snap::{GRID_SIZE, MAX_SNAP_CANDIDATES, MULTI_MOVE_SNAP_RADIUS, SMART_GUIDE_THRESHOLD, snap_to_grid};
use drafftink_scene::{ElementId, ElementKind, IdSource, Kind, SceneRead, GroupId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::element_common::{id_strings, prune_selection, select_exactly};
use super::view::RectArg;
use crate::cmd::element_common::target_refusal;
use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::snap::Snap;
use crate::spec::{Batch, Binding, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed, Undo};

// ---- shared pieces -------------------------------------------------------------------------------------------

/// Document commands of the family: undoable, change the document and the selection, refuse in view-only
/// mode and during a gesture, commit a text edit first, redraw the scene.
const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    needs_selection: Selection::Required,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: true,
    doc_write: crate::spec::DocWrite::Edit,
};
const PALETTE: Facts = Facts { palette: true, ..DOC };
const GESTURE: Facts = Facts { undo: Undo::Gesture, ..DOC };

/// Wraps the id source of the session as a sized `IdSource` for the edit crate.
struct Ids<'a>(&'a mut dyn IdSource);

impl IdSource for Ids<'_> {
    fn element_id(&mut self) -> ElementId {
        self.0.element_id()
    }
    fn group_id(&mut self) -> GroupId {
        self.0.group_id()
    }
    fn seed(&mut self) -> u32 {
        self.0.seed()
    }
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
    /// Marker coordinates along the guide.
    pub snap_points: Vec<f64>,
    /// The empty stretch along the guide between the dragged thing and the box it lines up with, as `[from, to]`.
    /// The overlay shows its length. Absent when they overlap along the guide.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<[f64; 2]>,
}

pub(super) fn guide_out(g: &SmartGuide) -> GuideOut {
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
        snap_points: g.snap_points.clone(),
        gap: g.gap.map(|(from, to)| [from, to]),
    }
}

fn rect_out(r: Rect) -> RectArg {
    RectArg { x: r.x0, y: r.y0, w: r.width(), h: r.height() }
}

/// Maps a transform error to the refusal the catalog names for it.
fn transform_refusal(cmd: &str, e: TransformError) -> Refusal {
    match e {
        TransformError::Target(t) => target_refusal(cmd, t),
        TransformError::NotRotatable { ids, reason } => {
            let text = match reason {
                NotRotatableReason::Frame => "Frames cannot be rotated.",
                NotRotatableReason::ElbowArrow => {
                    "Elbow arrows cannot be rotated. Change the arrow type first with `arrow.set_type` (sharp or round)."
                }
            };
            Refusal::custom("not_rotatable", cmd, text).with_details(serde_json::json!({ "ids": id_strings(&ids) }))
        }
        TransformError::RotateToNeedsOne => Refusal::custom(
            "bad_args",
            cmd,
            "mode \"to\" sets an absolute angle and needs exactly one element; use mode \"by\" to rotate several.",
        ),
        e @ (TransformError::NoElements
        | TransformError::RotateHandle
        | TransformError::BadBounds
        | TransformError::NotPositive) => Refusal::bad_args(cmd, e.to_string()),
        TransformError::NotFinite => {
            Refusal::bad_args(cmd, "the result is not a valid number; use smaller values")
        }
        TransformError::Invalid { id, error } => {
            Refusal::bad_args(cmd, format!("element `{id}` would become invalid: {error}. Use smaller values"))
        }
        TransformError::Store(e) => Refusal::internal(cmd, e.to_string()),
        TransformError::Binding(e) => Refusal::internal(cmd, e.to_string()),
    }
}

fn lifecycle_refusal(cmd: &str, e: LifecycleError) -> Refusal {
    match e {
        LifecycleError::Target(t) => target_refusal(cmd, t),
        LifecycleError::NoTarget => Refusal::no_target(cmd),
        LifecycleError::NotFinite => Refusal::bad_args(cmd, "the offset must be a finite number"),
        other => Refusal::internal(cmd, other.to_string()),
    }
}

fn frame_refusal(cmd: &str, e: FrameError) -> Refusal {
    Refusal::internal(cmd, e.to_string())
}

/// Runs the transform edit, mapping the error.
fn edit_t<R>(
    s: &mut Session,
    f: impl FnOnce(&mut crate::store::DocTx<'_, '_>) -> Result<R, TransformError>,
) -> Result<R, Refusal> {
    let cmd = s.cmd();
    match s.tab_mut().doc.edit::<R, TransformError>(f) {
        Ok((r, _)) => Ok(r),
        Err(e) => Err(transform_refusal(cmd, e)),
    }
}

// ---- element.delete ------------------------------------------------------------------------------------------

/// What happens to the children of a deleted frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameChildren {
    /// Delete them with the frame.
    #[default]
    Delete,
    /// Keep them and clear their frame.
    Release,
}

/// Arguments of `element.delete`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct DeleteArgs {
    /// Elements to delete. Default: the selection.
    pub ids: Option<Vec<Ref>>,
    /// What to do with the children of a deleted frame: `delete` (default) or `release`.
    pub frame_children: FrameChildren,
}

/// Result of `element.delete`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DeleteOut {
    /// Frame children that were kept and released.
    pub released: Vec<String>,
    /// Arrows that lost a target and became unbound at that end.
    pub arrows: Vec<String>,
}

fn delete(s: &mut Session, a: DeleteArgs) -> Result<DeleteOut, Refusal> {
    let cmd = s.cmd();
    let ids = s.targets(a.ids.as_deref())?;
    let policy = match a.frame_children {
        FrameChildren::Delete => ChildPolicy::Delete,
        FrameChildren::Release => ChildPolicy::Release,
    };
    let report = match s.tab_mut().doc.edit::<_, LifecycleError>(|tx| lifecycle::delete(tx, &ids, policy)) {
        Ok((r, _)) => r,
        Err(e) => return Err(lifecycle_refusal(cmd, e)),
    };
    prune_selection(s);
    Ok(DeleteOut { released: id_strings(&report.released), arrows: id_strings(&report.arrows) })
}

// ---- element.duplicate ---------------------------------------------------------------------------------------

/// Arguments of `element.duplicate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct DuplicateArgs {
    /// Elements to copy. Default: the selection.
    pub ids: Option<Vec<Ref>>,
    /// Offset of the copies, x.
    pub dx: f64,
    /// Offset of the copies, y.
    pub dy: f64,
    /// Select the copies afterwards.
    pub select: bool,
}

impl Default for DuplicateArgs {
    fn default() -> Self {
        Self { ids: None, dx: 20.0, dy: 20.0, select: true }
    }
}

/// Result of `element.duplicate`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DuplicateOut {
    /// The copies of the given elements, in z-order.
    pub ids: Vec<String>,
}

fn duplicate_elements(
    s: &mut Session,
    cmd: &str,
    ids: &[ElementId],
    offset: Vec2,
) -> Result<lifecycle::DuplicateReport, Refusal> {
    let editing = s.tab().observed.editing_group.clone();
    let opts = DuplicateOpts { offset, editing };
    let idx = s.active_index();
    let Some(tab) = s.tabs.get_mut(idx) else { return Err(Refusal::internal(cmd, "no active tab")) };
    let mut src = Ids(&mut *s.ports.ids);
    match tab.doc.edit::<_, LifecycleError>(|tx| lifecycle::duplicate(tx, ids, &opts, &mut src)) {
        Ok((r, _)) => Ok(r),
        Err(e) => Err(lifecycle_refusal(cmd, e)),
    }
}

fn duplicate(s: &mut Session, a: DuplicateArgs) -> Result<DuplicateOut, Refusal> {
    let cmd = s.cmd();
    let (dx, dy) = (args::world(cmd, "dx", a.dx)?, args::world(cmd, "dy", a.dy)?);
    let ids = s.targets(a.ids.as_deref())?;
    let report = duplicate_elements(s, cmd, &ids, Vec2::new(dx, dy))?;
    if a.select {
        select_exactly(s, &report.selection, true);
    }
    Ok(DuplicateOut { ids: id_strings(&report.selection) })
}

// ---- element.move --------------------------------------------------------------------------------------------

/// Whether a dragged element lands in a frame under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameRule {
    /// On the committed run, dropped elements join the frame under the pointer and dragged-out elements leave.
    #[default]
    Auto,
    /// Frame membership is left as it is.
    Keep,
}

/// Arguments of `element.move`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct MoveArgs {
    /// Elements to move. Default: the selection.
    pub ids: Option<Vec<Ref>>,
    /// Total displacement since the start, x (world units).
    pub dx: f64,
    /// Total displacement since the start, y (world units).
    pub dy: f64,
    /// Move copies and leave the originals (Alt-drag). The copies become the selection.
    pub duplicate: bool,
    /// Snapping: `auto` (follow the settings), `none`, or `{grid?, smart?}`.
    pub snap: Snap,
    /// `auto` (default) updates frame membership on the committed run, `keep` leaves it.
    pub frame: FrameRule,
    /// World position of the pointer; frame hit-testing uses it. Default: the centre of the moved bounds.
    pub pointer: Option<[f64; 2]>,
}

/// Result of `element.move`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct MoveOut {
    /// The displacement after snapping, x.
    pub dx: f64,
    /// The displacement after snapping, y.
    pub dy: f64,
    /// Smart guides to draw.
    pub guides: Vec<GuideOut>,
    /// The frame under the pointer, when the moved elements would join it.
    pub frame_highlight: Option<String>,
    /// The copies, when `duplicate` is true.
    pub ids: Vec<String>,
}

/// Boxes of the other elements that guides and spacing look at: the visible ones near `zone`, at most
/// `MAX_SNAP_CANDIDATES`, plus the vertices of lines and arrows as zero-size boxes (the old app's rule).
fn snap_candidates(s: &Session, exclude: &BTreeSet<ElementId>, zone: Option<Rect>) -> Vec<Rect> {
    let (vx, vy, vw, vh) = s.view().visible();
    let viewport = Rect::new(vx, vy, vx + vw, vy + vh);
    let touches = |a: &Rect, b: &Rect| a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1;
    // Only elements that touch the viewport and the zone count, so the spatial index narrows the scene first.
    let area = zone.map_or(viewport, |z| z.intersect(viewport).inflate(1.0, 1.0));
    let index = s.tab().doc.scene_index();
    let mut out = Vec::new();
    for e in index.in_rect(s.store(), &area).into_iter().filter(|e| !exclude.contains(&e.id)) {
        let b = geometry::world_bounds(e).inflate(1.0, 1.0);
        if !touches(&viewport, &b) || zone.as_ref().is_some_and(|z| !touches(z, &b)) {
            continue;
        }
        if out.len() >= MAX_SNAP_CANDIDATES {
            break;
        }
        out.push(geometry::world_bounds(e));
        if matches!(e.kind, ElementKind::Line(_) | ElementKind::Arrow(_))
            && let Some(pts) = geometry::world_points(e)
        {
            out.extend(pts.into_iter().map(|p| Rect::new(p.x, p.y, p.x, p.y)));
        }
    }
    out
}

/// The old movement threshold: a drag under this many units does not snap (it prevents a snap on a click).
const SNAP_MOVE_THRESHOLD: f64 = 2.0;

/// Adjusts `(dx, dy)` by smart guides and then by the grid. The reference is the common bounds of what moves,
/// so the result does not depend on any iteration order (the old code used `HashMap::values().next()`).
fn snap_move(s: &Session, moved: &[ElementId], dx: f64, dy: f64, on: SnapFlags) -> (f64, f64, Vec<SmartGuide>) {
    let mut guides = Vec::new();
    if (!on.grid && !on.smart) || dx.hypot(dy) <= SNAP_MOVE_THRESHOLD {
        return (dx, dy, guides);
    }
    let Some(reference) = transform::common_bounds(s.store(), moved) else { return (dx, dy, guides) };
    let (mut dx, mut dy) = (dx, dy);
    if on.smart {
        let target = reference + Vec2::new(dx, dy);
        let rel = Reverse::of(s.store());
        let exclude: BTreeSet<ElementId> = with_companions(s.store(), &rel, moved, true).into_iter().collect();
        let zone = target.inflate(MULTI_MOVE_SNAP_RADIUS, MULTI_MOVE_SNAP_RADIUS);
        let others = snap_candidates(s, &exclude, Some(zone));
        let r = detect_smart_guides(target, &others, SMART_GUIDE_THRESHOLD);
        if r.snapped_x || r.snapped_y {
            dx += r.point.x - target.x0;
            dy += r.point.y - target.y0;
            guides = r.guides;
        }
    }
    if on.grid {
        let p = snap_to_grid(Point::new(reference.x0 + dx, reference.y0 + dy), GRID_SIZE).point;
        dx = p.x - reference.x0;
        dy = p.y - reference.y0;
    }
    (dx, dy, guides)
}

fn move_(s: &mut Session, a: MoveArgs) -> Result<MoveOut, Refusal> {
    let cmd = s.cmd();
    let (dx, dy) = (args::world(cmd, "dx", a.dx)?, args::world(cmd, "dy", a.dy)?);
    let pointer = match a.pointer {
        Some(p) => Some(Point::new(args::world(cmd, "pointer[0]", p[0])?, args::world(cmd, "pointer[1]", p[1])?)),
        None => None,
    };
    let ids = s.targets(a.ids.as_deref())?;
    let on = a.snap.resolve(s);
    let (dx, dy, guides) = snap_move(s, &ids, dx, dy, on);
    let mut moved = ids.clone();
    let mut copies: Vec<ElementId> = Vec::new();
    if a.duplicate {
        let report = duplicate_elements(s, cmd, &ids, Vec2::ZERO)?;
        copies = report.selection;
        moved = copies.clone();
    }
    let delta = Vec2::new(dx, dy);
    edit_t(s, |tx| transform::move_by(tx, &moved, delta))?;
    let live = s.live();
    if a.frame == FrameRule::Auto && !live {
        let cmd = s.cmd();
        s.tab_mut()
            .doc
            .edit::<_, FrameError>(|tx| frame::update_membership_after_move(tx, &moved, pointer))
            .map_err(|e| frame_refusal(cmd, e))?;
    }
    if a.duplicate {
        select_exactly(s, &copies, true);
    }
    let frame_highlight = if a.frame == FrameRule::Auto { highlighted_frame(s, &moved, pointer) } else { None };
    Ok(MoveOut {
        dx,
        dy,
        guides: guides.iter().map(guide_out).collect(),
        frame_highlight: frame_highlight.map(|f| f.as_str().to_owned()),
        ids: id_strings(&copies),
    })
}

/// The frame that the moved elements would join: the frame under the pointer (or the centre of the moved
/// bounds), excluding the moved frames themselves.
fn highlighted_frame(s: &Session, moved: &[ElementId], pointer: Option<Point>) -> Option<ElementId> {
    let prim = primaries(s.store(), moved).ok()?;
    let exclude: BTreeSet<ElementId> =
        prim.iter().filter(|i| s.element(i).is_some_and(|e| e.tag() == Kind::Frame)).cloned().collect();
    if exclude.len() == prim.len() {
        return None;
    }
    let at = pointer.or_else(|| transform::common_bounds(s.store(), &prim).map(|b| b.center()))?;
    frame::frame_at(s.store(), at, &exclude)
}

// ---- element.nudge -------------------------------------------------------------------------------------------

/// A direction of the arrow keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    /// Towards smaller x.
    Left,
    /// Towards larger x.
    Right,
    /// Towards smaller y.
    Up,
    /// Towards larger y.
    Down,
}

/// Arguments of `element.nudge`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NudgeArgs {
    /// The direction.
    pub dir: Dir,
    /// Elements to move. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// Distance in world units. Default: the setting `input.nudge_step` (20).
    #[serde(default)]
    pub step: Option<f64>,
    /// Move by 1 unit (Shift+arrow).
    #[serde(default)]
    pub fine: bool,
}

/// Result of `element.nudge`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct NudgeOut {
    /// The displacement applied, x.
    pub dx: f64,
    /// The displacement applied, y.
    pub dy: f64,
}

fn nudge(s: &mut Session, a: NudgeArgs) -> Result<NudgeOut, Refusal> {
    let cmd = s.cmd();
    let step = match (a.step, a.fine) {
        (Some(v), _) => args::positive(cmd, "step", v, args::MAX_WORLD)?,
        (None, true) => 1.0,
        (None, false) => args::positive(cmd, "input.nudge_step", s.settings.f64_or("input.nudge_step", 20.0), args::MAX_WORLD)
            .unwrap_or(20.0),
    };
    let ids = s.targets(a.ids.as_deref())?;
    let (dx, dy) = match a.dir {
        Dir::Left => (-step, 0.0),
        Dir::Right => (step, 0.0),
        Dir::Up => (0.0, -step),
        Dir::Down => (0.0, step),
    };
    edit_t(s, |tx| transform::nudge(tx, &ids, dx, dy))?;
    Ok(NudgeOut { dx, dy })
}

// ---- element.resize ------------------------------------------------------------------------------------------

/// A handle of the selection box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Handle {
    /// Top-left corner.
    Nw,
    /// Top edge.
    N,
    /// Top-right corner.
    Ne,
    /// Right edge.
    E,
    /// Bottom-right corner.
    #[default]
    Se,
    /// Bottom edge.
    S,
    /// Bottom-left corner.
    Sw,
    /// Left edge.
    W,
}

impl Handle {
    fn to_box(self) -> BoxHandle {
        match self {
            Handle::Nw => BoxHandle::Corner(Corner::TopLeft),
            Handle::Ne => BoxHandle::Corner(Corner::TopRight),
            Handle::Se => BoxHandle::Corner(Corner::BottomRight),
            Handle::Sw => BoxHandle::Corner(Corner::BottomLeft),
            Handle::N => BoxHandle::Edge(Edge::Top),
            Handle::E => BoxHandle::Edge(Edge::Right),
            Handle::S => BoxHandle::Edge(Edge::Bottom),
            Handle::W => BoxHandle::Edge(Edge::Left),
        }
    }

    /// Which axes the handle moves: `(x, y)`.
    fn axes(self) -> (bool, bool) {
        match self {
            Handle::Nw | Handle::Ne | Handle::Se | Handle::Sw => (true, true),
            Handle::N | Handle::S => (false, true),
            Handle::E | Handle::W => (true, false),
        }
    }

    /// The position of the handle on an axis-aligned box.
    fn point(self, b: Rect) -> Point {
        match self {
            Handle::Nw => Point::new(b.x0, b.y0),
            Handle::N => Point::new(b.center().x, b.y0),
            Handle::Ne => Point::new(b.x1, b.y0),
            Handle::E => Point::new(b.x1, b.center().y),
            Handle::Se => Point::new(b.x1, b.y1),
            Handle::S => Point::new(b.center().x, b.y1),
            Handle::Sw => Point::new(b.x0, b.y1),
            Handle::W => Point::new(b.x0, b.center().y),
        }
    }
}

/// Arguments of `element.resize`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResizeArgs {
    /// Elements to resize. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// The handle that is dragged. Default `se`.
    #[serde(default)]
    pub handle: Handle,
    /// Total displacement of the handle since the start, x (world axes).
    pub dx: f64,
    /// Total displacement of the handle since the start, y (world axes).
    pub dy: f64,
    /// Keep the width to height ratio. Forced for text, math and images, and for rotated or grouped elements.
    #[serde(default)]
    pub keep_aspect: bool,
    /// Scale about the centre instead of the opposite handle (Alt).
    #[serde(default)]
    pub from_center: bool,
    /// Snapping of the dragged handle: `auto`, `none` or `{grid?, smart?}`.
    #[serde(default)]
    pub snap: Snap,
}

/// A width and a height.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
pub struct SizeOut {
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// Result of `element.resize` and `element.set_bounds`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ResizeOut {
    /// The common bounds after the change.
    pub bounds: Option<RectArg>,
    /// Smart guides to draw.
    pub guides: Vec<GuideOut>,
    /// The new size, for the badge next to the pointer.
    pub size_badge: Option<SizeOut>,
}

/// Adjusts the displacement of a handle so that the dragged edges land on guides or the grid. Only for boxes
/// without rotation: a rotated box has no axis-aligned edge to snap.
fn snap_resize(s: &Session, ids: &[ElementId], a: &ResizeArgs, on: SnapFlags) -> (f64, f64, Vec<SmartGuide>) {
    let none = (a.dx, a.dy, Vec::new());
    if !on.grid && !on.smart {
        return none;
    }
    let Ok(prim) = primaries(s.store(), ids) else { return none };
    if prim.iter().any(|i| s.element(i).is_none_or(|e| e.transform.angle != 0.0)) {
        return none;
    }
    let Some(b) = transform::common_bounds(s.store(), &prim) else { return none };
    let from = a.handle.point(b);
    let mut target = Point::new(from.x + a.dx, from.y + a.dy);
    let (ax, ay) = a.handle.axes();
    let mut guides = Vec::new();
    let (mut snapped_x, mut snapped_y) = (false, false);
    if on.smart {
        let rel = Reverse::of(s.store());
        let exclude: BTreeSet<ElementId> = with_companions(s.store(), &rel, &prim, true).into_iter().collect();
        let zone = Rect::from_center_size(target, (MULTI_MOVE_SNAP_RADIUS * 2.0, MULTI_MOVE_SNAP_RADIUS * 2.0));
        let others = snap_candidates(s, &exclude, Some(zone));
        let r = detect_smart_guides_for_point(target, &others, SMART_GUIDE_THRESHOLD);
        if ax && r.snapped_x {
            target.x = r.point.x;
            snapped_x = true;
        }
        if ay && r.snapped_y {
            target.y = r.point.y;
            snapped_y = true;
        }
        if snapped_x || snapped_y {
            guides = r.guides;
        }
    }
    if on.grid {
        let g = snap_to_grid(target, GRID_SIZE).point;
        if ax && !snapped_x {
            target.x = g.x;
        }
        if ay && !snapped_y {
            target.y = g.y;
        }
    }
    (if ax { target.x - from.x } else { a.dx }, if ay { target.y - from.y } else { a.dy }, guides)
}

fn resize(s: &mut Session, a: ResizeArgs) -> Result<ResizeOut, Refusal> {
    let cmd = s.cmd();
    let (dx, dy) = (args::world(cmd, "dx", a.dx)?, args::world(cmd, "dy", a.dy)?);
    let ids = s.targets(a.ids.as_deref())?;
    let on = a.snap.resolve(s);
    let a = ResizeArgs { dx, dy, ..a };
    let (dx, dy, guides) = snap_resize(s, &ids, &a, on);
    let spec = ResizeSpec {
        handle: a.handle.to_box(),
        delta: Vec2::new(dx, dy),
        keep_aspect: a.keep_aspect,
        from_center: a.from_center,
    };
    let report = edit_t(s, |tx| {
        let r = transform::resize(tx, &ids, spec)?;
        transform::tidy(tx, &r.changed)?;
        Ok(r)
    })?;
    if !s.live() {
        // Frame membership is recomputed once, on the committed run.
        let frames: Vec<ElementId> = primaries(s.store(), &ids)
            .unwrap_or_default()
            .into_iter()
            .filter(|i| s.element(i).is_some_and(|e| e.tag() == Kind::Frame))
            .collect();
        for f in frames {
            s.tab_mut()
                .doc
                .edit::<_, FrameError>(|tx| frame::recompute_after_resize(tx, &f).map(|_| ()))
                .map_err(|e| frame_refusal(cmd, e))?;
        }
    }
    Ok(ResizeOut {
        bounds: report.bounds.map(rect_out),
        guides: guides.iter().map(guide_out).collect(),
        size_badge: report.bounds.map(|b| SizeOut { w: b.width(), h: b.height() }),
    })
}

// ---- element.set_bounds --------------------------------------------------------------------------------------

/// What `x` and `y` of `element.set_bounds` refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// The top-left corner.
    #[default]
    TopLeft,
    /// The centre.
    Center,
}

/// Arguments of `element.set_bounds`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetBoundsArgs {
    /// Elements. Default: the selection. One element gets exactly this box.
    pub ids: Option<Vec<Ref>>,
    /// Position x. Absent keeps it.
    pub x: Option<f64>,
    /// Position y. Absent keeps it.
    pub y: Option<f64>,
    /// Width, above 0. Absent keeps it.
    pub w: Option<f64>,
    /// Height, above 0. Absent keeps it.
    pub h: Option<f64>,
    /// Whether `x` and `y` name the top-left corner (default) or the centre.
    pub anchor: Anchor,
    /// When only one of `w` and `h` is given, the other follows the current ratio.
    pub keep_aspect: bool,
}

fn set_bounds(s: &mut Session, a: SetBoundsArgs) -> Result<ResizeOut, Refusal> {
    let cmd = s.cmd();
    let opt = |key: &str, v: Option<f64>| v.map(|v| args::world(cmd, key, v)).transpose();
    let (x, y) = (opt("x", a.x)?, opt("y", a.y)?);
    let positive = |key: &str, v: Option<f64>| v.map(|v| args::positive(cmd, key, v, args::MAX_WORLD)).transpose();
    let (w, h) = (positive("w", a.w)?, positive("h", a.h)?);
    let ids = s.targets(a.ids.as_deref())?;
    let spec = CommonBounds { x, y, w, h, center: a.anchor == Anchor::Center, keep_aspect: a.keep_aspect };
    let report = edit_t(s, |tx| {
        let r = transform::set_common_bounds(tx, &ids, spec)?;
        transform::tidy(tx, &r.changed)?;
        Ok(r)
    })?;
    Ok(ResizeOut {
        bounds: report.bounds.map(rect_out),
        guides: Vec::new(),
        size_badge: report.bounds.map(|b| SizeOut { w: b.width(), h: b.height() }),
    })
}

// ---- element.rotate ------------------------------------------------------------------------------------------

/// Relative or absolute rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RotateModeArg {
    /// Turn by `angle`.
    #[default]
    By,
    /// Set the angle (exactly one element).
    To,
}

/// Arguments of `element.rotate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RotateArgs {
    /// Elements to rotate. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// Degrees, clockwise.
    pub angle: f64,
    /// `by` (default) turns by the angle, `to` sets the angle of one element.
    #[serde(default)]
    pub mode: RotateModeArg,
    /// Round the turn (or the final angle) to a multiple of this many degrees. The UI passes 15 with Shift.
    #[serde(default)]
    pub snap_degrees: Option<f64>,
}

/// The angle badge next to the rotate handle.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AngleBadge {
    /// Where the rotation is about.
    pub center: [f64; 2],
    /// The angle shown, in degrees.
    pub degrees: f64,
    /// The angle was rounded to `snap_degrees`.
    pub snapped: bool,
}

/// Result of `element.rotate`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RotateOut {
    /// One element: its angle now, in degrees from 0 to 360. Several: the turn that was applied.
    pub angle: f64,
    /// For the overlay.
    pub angle_badge: Option<AngleBadge>,
}

fn rotate(s: &mut Session, a: RotateArgs) -> Result<RotateOut, Refusal> {
    let cmd = s.cmd();
    let angle = args::in_range(cmd, "angle", a.angle, -1.0e6, 1.0e6)?;
    let step = a.snap_degrees.map(|d| args::positive(cmd, "snap_degrees", d, 360.0)).transpose()?;
    let ids = s.targets(a.ids.as_deref())?;
    let mode = match a.mode {
        RotateModeArg::By => RotateMode::By,
        RotateModeArg::To => RotateMode::To,
    };
    let before: Vec<f64> = ids.iter().filter_map(|i| s.element(i)).map(|e| e.transform.angle).collect();
    edit_t(s, |tx| transform::rotate(tx, &ids, angle.to_radians(), mode, step.map(f64::to_radians)))?;
    let prim = primaries(s.store(), &ids).unwrap_or_default();
    let shown = match prim.as_slice() {
        [only] => s.element(only).map_or(0.0, |e| e.transform.angle.to_degrees().rem_euclid(360.0)),
        _ => {
            let after: Vec<f64> = ids.iter().filter_map(|i| s.element(i)).map(|e| e.transform.angle).collect();
            match (before.first(), after.first()) {
                (Some(b), Some(n)) => (n - b).to_degrees(),
                _ => angle,
            }
        }
    };
    let badge = transform::common_bounds(s.store(), &prim)
        .map(|b| AngleBadge { center: [b.center().x, b.center().y], degrees: shown, snapped: step.is_some() });
    Ok(RotateOut { angle: shown, angle_badge: badge })
}

// ---- element.flip --------------------------------------------------------------------------------------------

/// The axis of a flip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AxisArg {
    /// Mirror left to right.
    Horizontal,
    /// Mirror top to bottom.
    Vertical,
}

/// Arguments of `element.flip`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FlipArgs {
    /// Elements to mirror. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// `horizontal` or `vertical`.
    pub axis: AxisArg,
}

/// Result of `element.flip`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct FlipOut {
    /// The common bounds after the flip.
    pub bounds: Option<RectArg>,
}

fn flip(s: &mut Session, a: FlipArgs) -> Result<FlipOut, Refusal> {
    let ids = s.targets(a.ids.as_deref())?;
    let axis = match a.axis {
        AxisArg::Horizontal => FlipAxis::Horizontal,
        AxisArg::Vertical => FlipAxis::Vertical,
    };
    let report = edit_t(s, |tx| arrange::flip(tx, &ids, axis))?;
    Ok(FlipOut { bounds: report.bounds.map(rect_out) })
}

// ---- element.lock / element.unlock_all -----------------------------------------------------------------------

/// Arguments of `element.lock`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct LockArgs {
    /// Elements. Default: the selection.
    pub ids: Option<Vec<Ref>>,
    /// `true` locks, `false` unlocks. Absent toggles: unlock when all are locked, else lock.
    pub locked: Option<bool>,
}

/// Result of `element.lock`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct LockOut {
    /// The state the elements have now.
    pub locked: bool,
    /// Elements whose state changed: whole groups, bound text included.
    pub ids: Vec<String>,
    /// False when everything was already in that state.
    pub changed: bool,
}

fn set_lock(s: &mut Session, ids: &[ElementId], locked: Option<bool>) -> Result<lifecycle::LockReport, Refusal> {
    let cmd = s.cmd();
    match s.tab_mut().doc.edit::<_, LifecycleError>(|tx| lifecycle::lock(tx, ids, locked)) {
        Ok((r, _)) => Ok(r),
        Err(e) => Err(lifecycle_refusal(cmd, e)),
    }
}

fn lock(s: &mut Session, a: LockArgs) -> Result<LockOut, Refusal> {
    let ids = s.targets(a.ids.as_deref())?;
    let report = set_lock(s, &ids, a.locked)?;
    if report.locked {
        // Locked elements leave the selection.
        prune_selection(s);
    }
    Ok(LockOut { locked: report.locked, changed: !report.changed.is_empty(), ids: id_strings(&report.changed) })
}

/// Arguments of `element.unlock_all`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct UnlockAllArgs {
    /// Elements to unlock. Default: every locked element.
    pub ids: Option<Vec<Ref>>,
    /// Select the unlocked elements afterwards.
    pub select: bool,
}

impl Default for UnlockAllArgs {
    fn default() -> Self {
        Self { ids: None, select: true }
    }
}

/// Result of `element.unlock_all`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct UnlockOut {
    /// Elements that were unlocked.
    pub ids: Vec<String>,
}

fn any_locked(s: &Session) -> Result<(), Refusal> {
    if s.store().iter().any(|e| e.locked) {
        Ok(())
    } else {
        Err(Refusal::nothing_to_do(s.cmd(), "unlock", "no element is locked"))
    }
}

fn unlock_all(s: &mut Session, a: UnlockAllArgs) -> Result<UnlockOut, Refusal> {
    let ids: Vec<ElementId> = match a.ids.as_deref() {
        Some(refs) => s.targets(Some(refs))?,
        None => s.store().iter().filter(|e| e.locked).map(|e| e.id.clone()).collect(),
    };
    if ids.is_empty() {
        return Err(Refusal::nothing_to_do(s.cmd(), "unlock", "no element is locked"));
    }
    let report = set_lock(s, &ids, Some(false))?;
    if report.changed.is_empty() {
        return Err(Refusal::nothing_to_do(s.cmd(), "unlock", "none of the given elements is locked"));
    }
    if a.select && !report.changed.is_empty() {
        select_exactly(s, &report.changed, false);
    }
    Ok(UnlockOut { ids: id_strings(&report.changed) })
}

// ---- element.erase -------------------------------------------------------------------------------------------

/// The unit of the eraser radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RadiusUnits {
    /// Screen pixels, converted with the current zoom.
    #[default]
    Screen,
    /// World units.
    World,
}

/// Arguments of `element.erase`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EraseArgs {
    /// The path of the eraser in world coordinates, 1 point or more.
    pub path: Vec<[f64; 2]>,
    /// The radius of the eraser. Default 10.
    #[serde(default = "default_radius")]
    pub radius: f64,
    /// `screen` (default) or `world`.
    #[serde(default)]
    pub units: RadiusUnits,
}

fn default_radius() -> f64 {
    10.0
}

/// Longest eraser path.
const MAX_PATH_POINTS: usize = 10_000;

/// Result of `element.erase`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct EraseOut {
    /// The elements that were hit and deleted (with their bound text and frame children in `deleted`).
    pub hit: Vec<String>,
}

fn erase(s: &mut Session, a: EraseArgs) -> Result<EraseOut, Refusal> {
    let cmd = s.cmd();
    if a.path.is_empty() {
        return Err(Refusal::bad_arg(cmd, "path", "needs at least one point"));
    }
    if a.path.len() > MAX_PATH_POINTS {
        return Err(Refusal::limit_exceeded(cmd, a.path.len(), "path points", MAX_PATH_POINTS));
    }
    let mut path = Vec::with_capacity(a.path.len());
    for (i, p) in a.path.iter().enumerate() {
        path.push(Point::new(args::world(cmd, &format!("path[{i}][0]"), p[0])?, args::world(cmd, &format!("path[{i}][1]"), p[1])?));
    }
    let radius = args::positive(cmd, "radius", a.radius, 1.0e6)?;
    let radius = match a.units {
        RadiusUnits::World => radius,
        RadiusUnits::Screen => radius / s.view().zoom,
    };
    let hit = drafftink_edit::query::hit_by_path(s.store(), &path, radius);
    if hit.is_empty() {
        return Ok(EraseOut { hit: Vec::new() });
    }
    match s.tab_mut().doc.edit::<_, LifecycleError>(|tx| lifecycle::delete(tx, &hit, ChildPolicy::Delete)) {
        Ok(_) => {}
        Err(e) => return Err(lifecycle_refusal(cmd, e)),
    }
    prune_selection(s);
    Ok(EraseOut { hit: id_strings(&hit) })
}

// ---- registration --------------------------------------------------------------------------------------------

const DELETE_KEYS: &[Binding] = &[Binding::new(&["Delete", "Backspace"], Scope::Canvas)];
const DUPLICATE_KEYS: &[Binding] = &[Binding::new(&["Mod+D"], Scope::Canvas)];
const NUDGE_KEYS: &[Binding] = &[
    Binding::new(&["Left"], Scope::Canvas).with_args(r#"{"dir":"left"}"#),
    Binding::new(&["Right"], Scope::Canvas).with_args(r#"{"dir":"right"}"#),
    Binding::new(&["Up"], Scope::Canvas).with_args(r#"{"dir":"up"}"#),
    Binding::new(&["Down"], Scope::Canvas).with_args(r#"{"dir":"down"}"#),
    Binding::new(&["Shift+Left"], Scope::Canvas).with_args(r#"{"dir":"left","fine":true}"#),
    Binding::new(&["Shift+Right"], Scope::Canvas).with_args(r#"{"dir":"right","fine":true}"#),
    Binding::new(&["Shift+Up"], Scope::Canvas).with_args(r#"{"dir":"up","fine":true}"#),
    Binding::new(&["Shift+Down"], Scope::Canvas).with_args(r#"{"dir":"down","fine":true}"#),
];
const FLIP_KEYS: &[Binding] = &[
    Binding::new(&["Shift+H"], Scope::Canvas).with_args(r#"{"axis":"horizontal"}"#).titled("Flip horizontal"),
    Binding::new(&["Shift+V"], Scope::Canvas).with_args(r#"{"axis":"vertical"}"#).titled("Flip vertical"),
];
const LOCK_KEYS: &[Binding] = &[Binding::new(&["Mod+Shift+L"], Scope::Canvas)];

/// The commands of this file.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "element.delete",
        "element",
        "Delete",
        "Deletes the elements with their bound text; arrows bound to them stay, unbound at that end.",
        &Typed::new(delete),
        PALETTE,
    )
    .shortcuts(DELETE_KEYS)
    .example(r#"{"ids":["e1"]}"#),
    CommandSpec::new(
        "element.duplicate",
        "element",
        "Duplicate",
        "Copies the elements with new ids, offset by (dx, dy), and selects the copies.",
        &Typed::new(duplicate),
        PALETTE,
    )
    .shortcuts(DUPLICATE_KEYS)
    .example(r#"{"ids":["e1"],"dx":20,"dy":20}"#),
    CommandSpec::new(
        "element.move",
        "element",
        "Move",
        "Moves the elements by a total offset, following the transform rule; a gesture command.",
        &Typed::new(move_),
        GESTURE,
    )
    .example(r#"{"ids":["e1"],"dx":40,"dy":-10}"#),
    CommandSpec::new(
        "element.nudge",
        "element",
        "Nudge",
        "Moves the elements one step in a direction; repeats within a second merge into one undo step.",
        &Typed::new(nudge),
        DOC,
    )
    .coalesce("element.nudge", 1000)
    .shortcuts(NUDGE_KEYS)
    .example(r#"{"dir":"left","ids":["e1"]}"#),
    CommandSpec::new(
        "element.resize",
        "element",
        "Resize",
        "Scales the common bounds of the elements by dragging one handle; a gesture command.",
        &Typed::new(resize),
        GESTURE,
    )
    .example(r#"{"ids":["e1"],"handle":"se","dx":30,"dy":20}"#),
    CommandSpec::new(
        "element.set_bounds",
        "element",
        "Set position and size",
        "Sets the position and the size of the common bounds of the elements; one element gets exactly that box.",
        &Typed::new(set_bounds),
        DOC,
    )
    .example(r#"{"ids":["e1"],"x":100,"y":50,"w":200,"h":80}"#),
    CommandSpec::new(
        "element.rotate",
        "element",
        "Rotate",
        "Rotates the elements about their common centre by an angle in degrees; a gesture command.",
        &Typed::new(rotate),
        GESTURE,
    )
    .refusals(&["not_rotatable"])
    .example(r#"{"ids":["e1"],"angle":45}"#),
    CommandSpec::new(
        "element.flip",
        "element",
        "Flip",
        "Mirrors the elements about their common centre, horizontally or vertically.",
        &Typed::new(flip),
        PALETTE,
    )
    .shortcuts(FLIP_KEYS)
    .example(r#"{"ids":["e1"],"axis":"horizontal"}"#),
    CommandSpec::new(
        "element.lock",
        "element",
        "Lock or unlock",
        "Locks or unlocks the elements, with their whole outermost group; omitting `locked` toggles.",
        &Typed::new(lock),
        PALETTE,
    )
    .shortcuts(LOCK_KEYS)
    .example(r#"{"ids":["e1"],"locked":true}"#),
    CommandSpec::new(
        "element.unlock_all",
        "element",
        "Unlock all",
        "Unlocks the given elements, or every locked element, and selects them.",
        &Typed::new(unlock_all),
        Facts { needs_selection: Selection::Soft, ..PALETTE },
    )
    .available(any_locked)
    .example(r#"{"select":true}"#),
    CommandSpec::new(
        "element.erase",
        "element",
        "Erase along path",
        "Deletes every unlocked element hit by an eraser path, as one undo step.",
        &Typed::new(erase),
        Facts { needs_selection: Selection::No, ..DOC },
    )
    .example(r#"{"path":[[0,0],[100,40]],"radius":10,"units":"world"}"#),
];

#[cfg(test)]
#[path = "element_tests.rs"]
mod tests;
