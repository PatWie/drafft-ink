//! The select tool: click, marquee, move, resize, rotate, line points and double-click
//! (`docs/commands.md` section 7.3; `docs/exceptions.md` rows E2 to E5 and E9).
//!
//! * A **press** selects through `selection.select_at`, so that the drag that may follow moves the right thing.
//! * A **drag** is one of: a marquee (class B, one `selection.select_region` at the end), or a typed gesture
//!   (class A) of `element.move`, `element.resize`, `element.rotate`, `arrow.move_point` or
//!   `arrow.move_segment`, which stays alive between events as a [`Frame`].
//! * A **double-click** enters a group, starts a text edit, or edits the points of a line.
//!
//! The tool never changes the document or the selection by any route other than a command or the gesture
//! protocol. The text and math tools hand it presses and drags that start on an element or a handle.

mod click;
mod frame;

use drafftink_canvas::{Button, Handle, Target};
use drafftink_geom::aabb;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_geom::lasso::Lasso;
use drafftink_scene::{ElementId, PathType};

use self::frame::{Frame, Pivot};
use crate::cmd::element::Handle as BoxHandle;
use crate::cmd::selection::{RegionCombine, SelectRegionArgs};
use crate::cmd::selection_lasso::SelectLassoArgs;
use crate::cmd::view::RectArg;
use crate::gesture::GestureId;
use crate::interact::ctx::Ctx;
use crate::interact::events::{Ev, Key};
use crate::interact::frame_out;
use crate::interact::overlay::OverlayParts;
use crate::ring::Origin;
use crate::session::Ref;
use crate::state::Tool;

/// A marquee smaller than this on either side (screen pixels) selects nothing (the old minimum-size rule).
const MIN_MARQUEE_PX: f64 = 2.0;

/// A running gesture of the session that this tool began.
#[derive(Debug)]
struct Running {
    id: GestureId,
    frame: Frame,
    /// A frame was refused and the refusal shown; later refusals of the same drag stay quiet.
    warned: bool,
}

/// What the tool does with the drag that is running.
#[derive(Debug, Default)]
enum Drag {
    #[default]
    Idle,
    /// A marquee from this world point.
    Marquee(Point),
    /// A lasso; its path is `OverlayParts::lasso`.
    Lasso,
    /// A gesture frame stream.
    Gesture(Running),
    /// The drag was refused, or its gesture vanished: nothing more happens until it ends.
    Dead,
}

/// State of the select tool between pointer events.
#[derive(Debug, Default)]
pub(crate) struct SelectTool {
    drag: Drag,
    /// Shift was held on a press over a selected element: the toggle waits for the release, so that a drag
    /// keeps the selection.
    toggle_on_click: bool,
    /// The lasso tool was pressed on an element that is not selected: a drag starts a lasso there, and a click
    /// selects the element on release.
    lasso_on_click: bool,
}

impl SelectTool {
    pub(crate) fn press(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.lasso_on_click = lasso_starts_on_press(cx, ev);
        self.toggle_on_click = !self.lasso_on_click && click::press(cx, ev);
    }

    pub(crate) fn click(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        if std::mem::take(&mut self.lasso_on_click) {
            click::select_pressed(cx, ev);
            return;
        }
        let deferred = std::mem::take(&mut self.toggle_on_click);
        click::click(cx, ev, deferred);
    }

    pub(crate) fn double_click(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.toggle_on_click = false;
        self.lasso_on_click = false;
        click::double_click(cx, ev);
    }

    pub(crate) fn drag_start(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.toggle_on_click = false;
        let lasso_from_element = std::mem::take(&mut self.lasso_on_click);
        self.drag = Drag::Idle;
        match ev.target {
            Target::Empty if cx.tool() == Tool::Lasso || cx.mods.alt => self.begin_lasso(cx, ev),
            Target::Element { .. } if lasso_from_element => self.begin_lasso(cx, ev),
            Target::Empty => {
                self.drag = Drag::Marquee(ev.origin.world);
                cx.parts.marquee = Some(marquee_rect(ev.origin.world, ev.at.world));
                cx.overlay_changed();
            }
            Target::Element { id, .. } => {
                click::ensure_selected(cx, ev, id);
                self.begin(cx, Some(Frame::move_(ev, cx.mods)), None);
            }
            Target::Handle(handle) => {
                let (frame, active) = frame_for_handle(cx, ev, handle);
                self.begin(cx, frame, Some(active));
            }
            Target::None | Target::LinkIcon { .. } => self.drag = Drag::Dead,
        }
    }

    pub(crate) fn drag_update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let gone = matches!(&self.drag, Drag::Gesture(run) if cx.s.running_gesture() != Some(run.id));
        if gone {
            self.vanished(cx);
            return;
        }
        match &mut self.drag {
            Drag::Idle | Drag::Dead => {}
            Drag::Marquee(origin) => {
                cx.parts.marquee = Some(marquee_rect(*origin, ev.at.world));
                cx.overlay_changed();
            }
            Drag::Lasso => extend_lasso(cx, ev.at.world),
            Drag::Gesture(run) => {
                run.frame.follow(ev, cx.mods, cx.bind_choice(cx.mods));
                match cx.s.gesture_update(run.id, run.frame.args()) {
                    Ok(out) => {
                        frame_out::apply(cx.parts, &out);
                    }
                    Err(refusal) => {
                        if !std::mem::replace(&mut run.warned, true) {
                            cx.out.refusals.push(refusal);
                        }
                    }
                }
                cx.overlay_changed();
            }
        }
    }

    pub(crate) fn drag_end(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>, cancelled: bool) {
        match std::mem::take(&mut self.drag) {
            Drag::Idle | Drag::Dead => {}
            Drag::Marquee(origin) => {
                if !cancelled {
                    select_region(cx, ev, origin);
                }
            }
            Drag::Lasso => {
                if !cancelled {
                    extend_lasso(cx, ev.at.world);
                    select_lasso(cx);
                }
            }
            Drag::Gesture(mut run) => {
                if cx.s.running_gesture() == Some(run.id) {
                    if cancelled {
                        if let Err(refusal) = cx.s.gesture_cancel(Some(run.id)) {
                            cx.out.refusals.push(refusal);
                        }
                    } else {
                        run.frame.follow(ev, cx.mods, cx.bind_choice(cx.mods));
                        if let Err(refusal) = cx.s.gesture_end_typed(run.id, Some(run.frame.args())) {
                            cx.out.refusals.push(refusal);
                        }
                    }
                }
            }
        }
        clear_parts(cx.parts);
        cx.overlay_changed();
    }

    /// Drops whatever is running without a trace (gesture cancelled, marquee dropped).
    pub(crate) fn cancel(&mut self, cx: &mut Ctx<'_>) {
        self.toggle_on_click = false;
        self.lasso_on_click = false;
        if let Drag::Gesture(run) = std::mem::take(&mut self.drag)
            && cx.s.running_gesture() == Some(run.id)
            && let Err(refusal) = cx.s.gesture_cancel(Some(run.id))
        {
            cx.out.refusals.push(refusal);
        }
        self.drag = Drag::Idle;
        clear_parts(cx.parts);
    }

    /// The tool uses no key of its own: Escape reaches a drag as a cancelled drag end.
    pub(crate) fn key(&mut self, _cx: &mut Ctx<'_>, _key: Key) -> bool {
        false
    }

    /// True while a drag of the select tool is running.
    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.drag, Drag::Idle)
    }

    /// The gesture ended without us (`tool.cancel`, a script): forget it quietly.
    fn vanished(&mut self, cx: &mut Ctx<'_>) {
        self.drag = Drag::Dead;
        clear_parts(cx.parts);
        cx.overlay_changed();
    }

    /// Starts a lasso at the origin of the drag (the drag has moved to `ev.at` already).
    fn begin_lasso(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.drag = Drag::Lasso;
        cx.parts.lasso = vec![ev.origin.world];
        extend_lasso(cx, ev.at.world);
    }

    /// Begins the gesture for `frame` (a refusal is shown once and kills the drag).
    fn begin(&mut self, cx: &mut Ctx<'_>, frame: Option<Frame>, active: Option<Handle>) {
        let Some(frame) = frame else {
            self.drag = Drag::Dead;
            return;
        };
        match cx.s.gesture_begin_typed(Origin::Ui, frame.command(), frame.args()) {
            Ok((id, out)) => {
                frame_out::apply(cx.parts, &out);
                cx.parts.active_handle = active;
                self.drag = Drag::Gesture(Running { id, frame, warned: false });
                cx.overlay_changed();
            }
            Err(refusal) => {
                cx.out.refusals.push(refusal);
                self.drag = Drag::Dead;
            }
        }
    }
}

/// The world rectangle between two corners, in any order.
fn marquee_rect(a: Point, b: Point) -> Rect {
    Rect::from_points(a, b)
}

/// The one `selection.select_region` of a finished marquee.
fn select_region(cx: &mut Ctx<'_>, ev: &Ev<'_>, origin: Point) {
    if ev.delta_screen.x.abs() < MIN_MARQUEE_PX || ev.delta_screen.y.abs() < MIN_MARQUEE_PX {
        return;
    }
    let rect = marquee_rect(origin, ev.at.world);
    let args = SelectRegionArgs {
        rect: RectArg { x: rect.x0, y: rect.y0, w: rect.width(), h: rect.height() },
        mode: if cx.mods.shift { RegionCombine::Add } else { RegionCombine::Replace },
        contain: None,
        include_locked: false,
    };
    cx.exec_typed("selection.select_region", &args);
}

/// A lasso path point closer than this to the last one (screen pixels) is not kept.
const LASSO_STEP_PX: f64 = 2.0;

/// The path of a finished lasso is simplified to this tolerance (screen pixels) before it is tested.
const LASSO_SIMPLIFY_PX: f64 = 4.0;

/// The lasso tool does not select on a press over an element that is not selected: a drag from there is a lasso.
fn lasso_starts_on_press(cx: &Ctx<'_>, ev: &Ev<'_>) -> bool {
    cx.tool() == Tool::Lasso
        && ev.meta.button == Button::Primary
        && !ev.meta.modifiers.shift
        && ev.target.element().is_some_and(|id| !cx.s.selection().contains(id))
}

/// Adds a point to the lasso path and shows it.
fn extend_lasso(cx: &mut Ctx<'_>, at: Point) {
    let zoom = cx.zoom();
    let far_enough = cx.parts.lasso.last().is_none_or(|last| last.distance(at) * zoom >= LASSO_STEP_PX);
    if far_enough {
        cx.parts.lasso.push(at);
        cx.overlay_changed();
    }
}

/// The one `selection.select_lasso` of a finished lasso. A path with no area (a tiny scribble, a straight line)
/// selects nothing and says nothing, as a marquee under 2 pixels does.
fn select_lasso(cx: &mut Ctx<'_>) {
    let path = std::mem::take(&mut cx.parts.lasso);
    let zoom = cx.zoom();
    let Some(bounds) = aabb::from_points(path.iter().copied()) else { return };
    if bounds.width() * zoom < MIN_MARQUEE_PX
        || bounds.height() * zoom < MIN_MARQUEE_PX
        || Lasso::new(&path, 0.0).is_err()
    {
        return;
    }
    let args = SelectLassoArgs {
        points: path.iter().map(|p| [p.x, p.y]).collect(),
        mode: if cx.mods.shift { RegionCombine::Add } else { RegionCombine::Replace },
        contain: None,
        include_locked: false,
        simplify: LASSO_SIMPLIFY_PX / zoom,
    };
    cx.exec_typed("selection.select_lasso", &args);
}

/// The frame for a drag that starts on a handle, and the handle to show as active.
fn frame_for_handle(cx: &Ctx<'_>, ev: &Ev<'_>, handle: &Handle) -> (Option<Frame>, Handle) {
    let frame = match handle {
        Handle::TopLeft => Some(Frame::resize(BoxHandle::Nw)),
        Handle::Top => Some(Frame::resize(BoxHandle::N)),
        Handle::TopRight => Some(Frame::resize(BoxHandle::Ne)),
        Handle::Right => Some(Frame::resize(BoxHandle::E)),
        Handle::BottomRight => Some(Frame::resize(BoxHandle::Se)),
        Handle::Bottom => Some(Frame::resize(BoxHandle::S)),
        Handle::BottomLeft => Some(Frame::resize(BoxHandle::Sw)),
        Handle::Left => Some(Frame::resize(BoxHandle::W)),
        Handle::Rotate => {
            cx.world.frame().obox().and_then(|b| Pivot::new(b.center, ev.origin.world)).map(Frame::rotate)
        }
        Handle::Point { element, index } => Some(Frame::point(arrow_ref(element), *index, false, ev.at.world)),
        // Segment `s` of the canvas joins point `s` and `s + 1`; the command counts segments from 1.
        Handle::Midpoint { element, segment } => Some(if is_elbow(cx, element) {
            Frame::segment(arrow_ref(element), segment + 1, ev.at.world)
        } else {
            Frame::point(arrow_ref(element), segment + 1, true, ev.at.world)
        }),
    };
    (frame, handle.clone())
}

fn arrow_ref(id: &ElementId) -> Ref {
    Ref(id.as_str().to_owned())
}

/// True for an arrow or line routed with right angles.
pub(super) fn is_elbow(cx: &Ctx<'_>, id: &ElementId) -> bool {
    cx.s.element(id).and_then(|e| e.linear_data()).is_some_and(|d| d.path == PathType::Elbow)
}

/// Drops what this tool puts in the overlay: the marquee, guides, highlights, the badge and the active handle.
/// The drawing preview and the laser trail belong to others.
fn clear_parts(parts: &mut OverlayParts) {
    parts.marquee = None;
    parts.lasso.clear();
    parts.guides.clear();
    parts.highlight.clear();
    parts.badge = None;
    parts.active_handle = None;
    parts.angle_rays = None;
    parts.snap_point = None;
}

#[cfg(test)]
#[path = "../select_lasso_tests.rs"]
mod lasso_tests;
#[cfg(test)]
#[path = "../select_tests.rs"]
mod tests;
