//! Lines and arrows (E6, E5): a drag makes a two-point line; clicks make a multi-point draft that ends with a
//! double-click, Enter or Esc. While an end is placed, the shape it would bind to is highlighted.

use std::mem;

use drafftink_canvas::Modifiers;
use drafftink_edit::binding::{BindOpts, BindingCandidate, End, bound_end_position, find_hovered, hover_candidate};
use drafftink_edit::create;
use drafftink_geom::guides::SmartGuide;
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Element, ElementId, ElementKind, Kind, PathType};
use serde_json::json;

use super::snap::{self, Snaps};
use super::{NewArrowPath, preview};
use crate::interact::ctx::Ctx;
use crate::interact::events::Ev;
use crate::state::Tool;

/// A click this close (screen pixels) to the last placed point is not a new point. It is the second click of
/// a double-click, or a click on the spot.
const SAME_SPOT_PX: f64 = 6.0;

/// The shape under a placed point and what binding it would do.
#[derive(Debug, Clone, PartialEq)]
struct Hover {
    id: ElementId,
    /// The orbit candidate (none for a point inside the shape, which binds inside exactly there).
    candidate: Option<BindingCandidate>,
}

/// A point placed by the pointer after snapping.
#[derive(Debug)]
struct Located {
    point: Point,
    hover: Option<Hover>,
    guides: Vec<SmartGuide>,
}

/// The line or arrow being drawn.
#[derive(Debug, Default)]
pub(super) struct Linear {
    /// The points placed so far.
    points: Vec<Point>,
    /// Clicks started the draft, so it goes on after the pointer is released.
    multi: bool,
    /// A pointer drag is running.
    dragging: bool,
    /// Where the pointer is, snapped. The preview ends here. Kept only while there are placed points.
    tip: Option<Point>,
    /// The candidate at the first point, for the preview.
    start: Option<BindingCandidate>,
    /// The candidate at the tip, for the preview.
    end: Option<BindingCandidate>,
    /// The shape highlighted at the tip.
    target: Option<ElementId>,
    guides: Vec<SmartGuide>,
    /// The modifiers at the last placement. The key that ends a draft carries none, so these decide the binding.
    mods: Modifiers,
}

/// The kind of element the active tool makes.
fn kind_of(cx: &Ctx<'_>) -> Kind {
    if cx.tool() == Tool::Arrow { Kind::Arrow } else { Kind::Line }
}

/// The path type of a new arrow. Lines are always straight.
fn path_of(cx: &Ctx<'_>) -> PathType {
    match cx.tool() {
        Tool::Arrow => cx.s.ext.get::<NewArrowPath>().map(|p| p.0).unwrap_or_default(),
        _ => PathType::Sharp,
    }
}

/// Ctrl turns binding off, Alt binds inside, and the setting `arrow.binding` can turn it off for good. This is
/// what `element.create` does for `bind` auto, off and inside.
fn bind_opts(cx: &Ctx<'_>, mods: Modifiers) -> BindOpts {
    BindOpts {
        zoom: cx.world.displayed_zoom(),
        force_inside: mods.alt,
        disabled: mods.command() || !cx.setting("arrow.binding", true),
        midpoint_snap: cx.setting("arrow.midpoint_snap", false),
        angle_locked: false,
    }
}

/// The `bind` argument of `element.create`.
fn bind_policy(mods: Modifiers) -> &'static str {
    if mods.command() {
        "off"
    } else if mods.alt {
        "inside"
    } else {
        "auto"
    }
}

/// The shape that a point binds to, and the orbit candidate when there is one.
fn hover(cx: &Ctx<'_>, p: Point, elbow: bool, opts: &BindOpts) -> Option<Hover> {
    if opts.disabled {
        return None;
    }
    let store = cx.s.store();
    let found = find_hovered(store, p, opts.zoom, &[])?;
    Some(Hover { id: found.id, candidate: hover_candidate(store, p, elbow, opts) })
}

impl Linear {
    pub(super) fn is_active(&self) -> bool {
        !self.points.is_empty() || self.dragging
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    // ---- placing a point ----------------------------------------------------------------------------------

    /// Where a point at `raw` ends up. A binding candidate wins over snapping, as in `arrow.move_point`; a
    /// snapped point that falls into a shape's reach binds like the command will.
    fn locate(&self, cx: &Ctx<'_>, raw: Point) -> Located {
        let elbow = path_of(cx) == PathType::Elbow;
        let opts = bind_opts(cx, cx.mods);
        if let Some(h) = hover(cx, raw, elbow, &opts) {
            return Located { point: raw, hover: Some(h), guides: Vec::new() };
        }
        let snapped = snap::point(cx, raw, self.points.last().copied(), Snaps::line(cx));
        let hover = if snapped.point == raw { None } else { hover(cx, snapped.point, elbow, &opts) };
        Located { point: snapped.point, hover, guides: snapped.guides }
    }

    /// Where the first point at `raw` ends up: the grid only, as in the old app.
    fn locate_start(cx: &Ctx<'_>, raw: Point) -> Located {
        let elbow = path_of(cx) == PathType::Elbow;
        let opts = bind_opts(cx, cx.mods);
        if let Some(h) = hover(cx, raw, elbow, &opts) {
            return Located { point: raw, hover: Some(h), guides: Vec::new() };
        }
        let point = snap::start(cx, raw);
        let hover = if point == raw { None } else { hover(cx, point, elbow, &opts) };
        Located { point, hover, guides: Vec::new() }
    }

    /// Takes a located point as the pointer position. Returns true when something visible changed.
    fn track(&mut self, cx: &Ctx<'_>, loc: Located) -> bool {
        self.mods = cx.mods;
        let placed = !self.points.is_empty();
        let tip = placed.then_some(loc.point);
        let target = loc.hover.as_ref().map(|h| h.id.clone());
        let end = loc.hover.and_then(|h| h.candidate);
        let changed =
            self.tip != tip || self.target != target || self.guides != loc.guides || (placed && self.end != end);
        self.tip = tip;
        self.target = target;
        self.end = end;
        self.guides = loc.guides;
        changed
    }

    /// Shows the preview, the highlight and the guides.
    fn show(&self, cx: &mut Ctx<'_>, shown: &mut bool) {
        let element = self.preview(cx);
        preview::set(cx, element);
        preview::set_highlight(cx, self.target.as_ref());
        preview::set_guides(cx, self.guides.clone());
        *shown = true;
    }

    /// The preview: the placed points and the pointer position, with bound ends where the binding puts them.
    fn preview(&self, cx: &Ctx<'_>) -> Option<Element> {
        let mut pts = self.points.clone();
        if let Some(tip) = self.tip
            && pts.last() != Some(&tip)
        {
            pts.push(tip);
        }
        if path_of(cx) == PathType::Elbow {
            pts = ends_of(&pts);
        }
        if pts.len() < 2 {
            return None;
        }
        let kind = kind_of(cx);
        let drawn = preview::linear(cx, kind, &pts)?;
        let (start, end) = (self.start.as_ref(), self.end.as_ref());
        if start.is_none() && end.is_none() {
            return Some(drawn);
        }
        // Ask the binding solver where the ends sit, on a copy that carries the bindings.
        let mut bound = drawn;
        if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut bound.kind {
            d.start_binding = start.map(BindingCandidate::binding);
            d.end_binding = end.map(BindingCandidate::binding);
        }
        let store = cx.s.store();
        let first = bound_end_position(store, &bound, End::Start);
        let last = bound_end_position(store, &bound, End::End);
        if let (Some(slot), Some(p)) = (pts.first_mut(), first) {
            *slot = p;
        }
        if let (Some(slot), Some(p)) = (pts.last_mut(), last) {
            *slot = p;
        }
        preview::linear(cx, kind, &pts)
    }

    // ---- pointer ------------------------------------------------------------------------------------------

    /// Places the first point of a new line.
    fn begin(&mut self, cx: &Ctx<'_>, raw: Point) -> Located {
        let loc = Self::locate_start(cx, raw);
        self.start = loc.hover.as_ref().and_then(|h| h.candidate.clone());
        self.points.push(loc.point);
        loc
    }

    /// A click adds a point to the draft (or starts one).
    pub(super) fn click(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>, shown: &mut bool) {
        let raw = ev.at.world;
        let loc = if self.points.is_empty() {
            self.begin(cx, raw)
        } else {
            let loc = self.locate(cx, raw);
            if self.points.last().is_some_and(|last| last.distance(loc.point) * cx.zoom() < SAME_SPOT_PX) {
                return;
            }
            self.points.push(loc.point);
            loc
        };
        self.multi = true;
        self.track(cx, loc);
        self.show(cx, shown);
    }

    /// The pointer moved with no button: the rubber band follows, and the candidate shows.
    pub(super) fn hover(&mut self, cx: &mut Ctx<'_>, world: Point, shown: &mut bool) {
        let loc = self.locate(cx, world);
        if self.track(cx, loc) {
            self.show(cx, shown);
        }
    }

    pub(super) fn drag_start(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.dragging = true;
        if self.points.is_empty() {
            self.begin(cx, ev.origin.world);
            self.multi = false;
        }
    }

    pub(super) fn drag_update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>, shown: &mut bool) {
        if !self.dragging {
            return;
        }
        let loc = self.locate(cx, ev.at.world);
        if self.track(cx, loc) {
            self.show(cx, shown);
        }
    }

    pub(super) fn drag_end(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>, cancelled: bool, shown: &mut bool) {
        if !mem::take(&mut self.dragging) {
            return;
        }
        if cancelled {
            if self.multi {
                // The draft stays; only the dragged end is let go.
                self.tip = self.points.last().copied();
                self.show(cx, shown);
            } else {
                self.reset();
                take_back(cx, shown);
            }
            return;
        }
        let loc = self.locate(cx, ev.at.world);
        let end = loc.point;
        self.track(cx, loc);
        if self.multi {
            if self.points.last().is_some_and(|last| last.distance(end) * cx.zoom() >= SAME_SPOT_PX) {
                self.points.push(end);
            }
            self.show(cx, shown);
        } else {
            let mut points = mem::take(&mut self.points);
            points.push(end);
            self.commit(cx, points, shown);
        }
    }

    /// Ends the draft with the points placed so far (the tip is only where the pointer is).
    pub(super) fn finish(&mut self, cx: &mut Ctx<'_>, shown: &mut bool) {
        let points = mem::take(&mut self.points);
        self.commit(cx, points, shown);
    }

    /// Enter or Escape. Returns true when a draft was running.
    pub(super) fn end_draft(&mut self, cx: &mut Ctx<'_>, shown: &mut bool) -> bool {
        if self.points.is_empty() || self.dragging {
            return false;
        }
        self.finish(cx, shown);
        true
    }

    /// Runs the one `element.create`, if the points make a line the command accepts.
    fn commit(&mut self, cx: &mut Ctx<'_>, mut points: Vec<Point>, shown: &mut bool) {
        let mods = self.mods;
        let (kind, path) = (kind_of(cx), path_of(cx));
        self.reset();
        take_back(cx, shown);
        points.dedup_by(|a, b| a.distance(*b) < 1e-9);
        if path == PathType::Elbow {
            points = ends_of(&points);
        }
        if points.len() < 2 || !has_extent(kind, &points) {
            return;
        }
        let pts: Vec<[f64; 2]> = points.iter().map(|p| [p.x, p.y]).collect();
        let mut args = json!({ "kind": kind.name(), "points": pts, "bind": bind_policy(mods), "from_tool": true });
        if kind == Kind::Arrow {
            args["arrow_type"] = json!(path);
        }
        cx.exec("element.create", args);
    }
}

/// Removes the preview, highlight and guides.
fn take_back(cx: &mut Ctx<'_>, shown: &mut bool) {
    if mem::take(shown) {
        preview::clear(cx);
    }
}

/// An elbow arrow keeps its two ends; the engine routes between them.
fn ends_of(points: &[Point]) -> Vec<Point> {
    match (points.first(), points.last()) {
        (Some(a), Some(b)) if points.len() > 1 => vec![*a, *b],
        _ => points.to_vec(),
    }
}

/// True when the points reach 1 unit in one direction, the rule of `element.create` for a line.
fn has_extent(kind: Kind, points: &[Point]) -> bool {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in points {
        (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
    }
    create::check_extent(kind, x1 - x0, y1 - y0).is_ok()
}
