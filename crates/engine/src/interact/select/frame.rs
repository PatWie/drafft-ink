//! The typed arguments of the gesture frames the select tool streams (E2 to E5), kept between events and
//! rewritten in place, so a pointer move builds no JSON and allocates nothing.

use std::any::Any;

use drafftink_canvas::Modifiers;
use drafftink_geom::kurbo::Point;

use crate::cmd::arrow::{BindChoice, MovePointArgs, PointIndex, SegmentArgs, Snap as ArrowSnap};
use crate::cmd::element::{Handle, MoveArgs, ResizeArgs, RotateArgs, RotateModeArg, Snap, SnapWord};
use crate::interact::events::Ev;

/// The angle step of a rotation with Shift held (degrees).
const ROTATE_STEP_DEGREES: f64 = 15.0;

/// The turn about a centre that a rotate frame measures.
#[derive(Debug, Clone, Copy)]
pub(super) struct Pivot {
    /// What the selection turns about (world).
    pub(super) center: Point,
    /// The angle of the press position about the centre, in radians.
    pub(super) start: f64,
}

impl Pivot {
    /// A pivot for a press at `origin`; `None` when the press lies on the centre (no angle).
    pub(super) fn new(center: Point, origin: Point) -> Option<Pivot> {
        let d = origin - center;
        (d.hypot() > f64::EPSILON).then(|| Pivot { center, start: d.y.atan2(d.x) })
    }

    /// Degrees turned clockwise (y points down) since the press, in `[-180, 180)`.
    fn turn_to(&self, at: Point) -> f64 {
        let d = at - self.center;
        let turn = (d.y.atan2(d.x) - self.start).to_degrees();
        (turn + 180.0).rem_euclid(360.0) - 180.0
    }
}

/// One live gesture's current arguments.
#[derive(Debug, Clone)]
pub(super) enum Frame {
    /// `element.move`.
    Move(MoveArgs),
    /// `element.resize`.
    Resize(ResizeArgs),
    /// `element.rotate`, with the pivot the angle is measured about.
    Rotate(RotateArgs, Pivot),
    /// `arrow.move_point`, also with `insert` for a midpoint handle.
    Point(MovePointArgs),
    /// `arrow.move_segment`.
    Segment(SegmentArgs),
}

impl Frame {
    /// The command the gesture runs.
    pub(super) fn command(&self) -> &'static str {
        match self {
            Frame::Move(_) => "element.move",
            Frame::Resize(_) => "element.resize",
            Frame::Rotate(..) => "element.rotate",
            Frame::Point(_) => "arrow.move_point",
            Frame::Segment(_) => "arrow.move_segment",
        }
    }

    /// The arguments as the typed gesture door wants them.
    pub(super) fn args(&self) -> &dyn Any {
        match self {
            Frame::Move(a) => a,
            Frame::Resize(a) => a,
            Frame::Rotate(a, _) => a,
            Frame::Point(a) => a,
            Frame::Segment(a) => a,
        }
    }

    /// Rewrites the arguments for the pointer now: the totals since the press, and the modifier keys now.
    pub(super) fn follow(&mut self, ev: &Ev<'_>, mods: Modifiers) {
        match self {
            Frame::Move(a) => {
                a.dx = ev.delta_world.x;
                a.dy = ev.delta_world.y;
                a.pointer = Some([ev.at.world.x, ev.at.world.y]);
                a.snap = element_snap(mods);
            }
            Frame::Resize(a) => {
                a.dx = ev.delta_world.x;
                a.dy = ev.delta_world.y;
                a.keep_aspect = mods.shift;
                a.from_center = mods.alt;
                a.snap = element_snap(mods);
            }
            Frame::Rotate(a, pivot) => {
                a.angle = pivot.turn_to(ev.at.world);
                a.snap_degrees = mods.shift.then_some(ROTATE_STEP_DEGREES);
            }
            Frame::Point(a) => {
                a.to = [ev.at.world.x, ev.at.world.y];
                a.bind = bind_choice(mods);
            }
            Frame::Segment(a) => a.to = Some([ev.at.world.x, ev.at.world.y]),
        }
    }

    /// `element.move` starting at the press.
    pub(super) fn move_(ev: &Ev<'_>, mods: Modifiers) -> Frame {
        let mut a = MoveArgs { duplicate: mods.alt, ..MoveArgs::default() };
        a.pointer = Some([ev.at.world.x, ev.at.world.y]);
        a.snap = element_snap(mods);
        Frame::Move(a)
    }

    /// `element.resize` for a handle of the box.
    pub(super) fn resize(handle: Handle) -> Frame {
        Frame::Resize(ResizeArgs {
            ids: None,
            handle,
            dx: 0.0,
            dy: 0.0,
            keep_aspect: false,
            from_center: false,
            snap: Snap::default(),
        })
    }

    /// `element.rotate` by the turn since the press.
    pub(super) fn rotate(pivot: Pivot) -> Frame {
        Frame::Rotate(RotateArgs { ids: None, angle: 0.0, mode: RotateModeArg::By, snap_degrees: None }, pivot)
    }

    /// `arrow.move_point` for point `index`, or for a point inserted into segment `index` (counted from 1).
    pub(super) fn point(arrow: crate::session::Ref, index: usize, insert: bool, to: Point) -> Frame {
        Frame::Point(MovePointArgs {
            arrow,
            index: PointIndex::Number(index),
            to: [to.x, to.y],
            insert,
            bind: BindChoice::Auto,
            snap: ArrowSnap::default(),
        })
    }

    /// `arrow.move_segment` for segment `segment` (counted from 1).
    pub(super) fn segment(arrow: crate::session::Ref, segment: usize, to: Point) -> Frame {
        Frame::Segment(SegmentArgs { arrow, segment, to: Some([to.x, to.y]) })
    }
}

/// Ctrl or Cmd switches snapping off while it is held.
fn element_snap(mods: Modifiers) -> Snap {
    if mods.command() { Snap::Word(SnapWord::None) } else { Snap::default() }
}

/// Ctrl or Cmd: do not bind. Alt: bind inside the shape.
fn bind_choice(mods: Modifiers) -> BindChoice {
    if mods.command() {
        BindChoice::Off
    } else if mods.alt {
        BindChoice::Inside
    } else {
        BindChoice::Auto
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quarter_turn_clockwise_is_ninety_degrees() {
        let pivot = Pivot::new(Point::new(0.0, 0.0), Point::new(0.0, -10.0));
        let turn = pivot.map(|p| p.turn_to(Point::new(10.0, 0.0)));
        assert!(turn.is_some_and(|t| (t - 90.0).abs() < 1e-9), "{turn:?}");
    }

    #[test]
    fn the_turn_wraps_into_the_half_circle() {
        let pivot = Pivot::new(Point::new(0.0, 0.0), Point::new(10.0, 0.0));
        let turn = pivot.map(|p| p.turn_to(Point::new(0.0, -10.0)));
        assert!(turn.is_some_and(|t| (t + 90.0).abs() < 1e-9), "{turn:?}");
    }

    #[test]
    fn a_press_on_the_centre_has_no_pivot() {
        assert!(Pivot::new(Point::new(3.0, 3.0), Point::new(3.0, 3.0)).is_none());
    }

    #[test]
    fn modifiers_choose_bind_and_snap() {
        let ctrl = Modifiers { ctrl: true, ..Modifiers::NONE };
        let alt = Modifiers { alt: true, ..Modifiers::NONE };
        assert_eq!(bind_choice(ctrl), BindChoice::Off);
        assert_eq!(bind_choice(alt), BindChoice::Inside);
        assert_eq!(bind_choice(Modifiers::NONE), BindChoice::Auto);
        assert_eq!(element_snap(ctrl), Snap::Word(SnapWord::None));
        assert_eq!(element_snap(Modifiers::NONE), Snap::default());
    }
}
