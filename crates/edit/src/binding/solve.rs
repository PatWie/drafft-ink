//! Where does a bound arrow end sit now? The live "simple" strategy of Excalidraw (MIT), in our model.
//!
//! * `Inside`: the end is exactly at the binding's fixed point (a ratio of the target's unrotated box,
//!   rotated with the target).
//! * `Orbit`: the fixed point is a focus the arrow aims at; the end is where the segment from the focus
//!   to the other end crosses the target outline pushed out by the gap.
//!
//! Degenerate cases (the arrow would invert, or is shorter than [`BASE_ARROW_MIN_LENGTH`]) fall back to the
//! focus. Dangling bindings (target missing, deleted or not bindable) leave their end where it is.

use drafftink_geom::binding::BASE_ARROW_MIN_LENGTH;
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_scene::{BindMode, Binding, Element, Head, LinearData, SceneRead};

use super::End;
use super::target::{Resolved, resolve};

/// A bound end with its target resolved.
pub(crate) struct Side<'a> {
    pub binding: &'a Binding,
    pub target: Resolved,
}

impl Side<'_> {
    pub(crate) fn focus(&self) -> Point {
        self.target.target.global_fixed_point(Vec2::new(self.binding.fixed_point[0], self.binding.fixed_point[1]))
    }
}

pub(crate) fn side<'a, R: SceneRead>(scene: &R, binding: &'a Option<Binding>) -> Option<Side<'a>> {
    let b = binding.as_ref()?;
    Some(Side { binding: b, target: resolve(scene, &b.target)? })
}

fn finite(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// The position of one bound end, in world space.
///
/// `world` are the arrow's points in world space. `dragging` is for a live drag: the "other shape covers
/// the outline point" shortcut is off then.
pub(crate) fn bound_point(
    world: &[Point],
    end: End,
    this: &Side<'_>,
    other: Option<&Side<'_>>,
    start_head: Head,
    end_head: Head,
    dragging: bool,
) -> Point {
    let focus = this.focus();
    if this.binding.mode == BindMode::Inside {
        return focus;
    }
    let n = world.len();
    let other_focus = other.map(Side::focus);
    let other_arrow_point = match end {
        End::Start => world.get(1),
        End::End => n.checked_sub(2).and_then(|i| world.get(i)),
    }
    .copied()
    .unwrap_or(focus);
    let other_ref = if n == 2 { other_focus.unwrap_or(other_arrow_point) } else { other_arrow_point };

    let outline = this.target.target.orbit_point(focus, other_ref, this.target.gap);
    let this_has_head = match end {
        End::Start => start_head != Head::None,
        End::End => end_head != Head::None,
    };
    let any_head = start_head != Head::None || end_head != Head::None;
    let resolved_target = if !any_head || this_has_head { focus } else { outline.unwrap_or(focus) };

    // 1. The outline point lies inside the other shape: the arrow would invert.
    if let (Some(o), Some(op)) = (other, outline)
        && !dragging
        && o.target.area < this.target.area * 2.0
        && o.target.target.within_reach(op, o.target.gap)
    {
        return resolved_target;
    }

    let other_outline = other.and_then(|o| {
        let mut hits = o.target.target.intersect_segment(focus, other_ref, o.target.gap);
        hits.sort_by(|a, b| (*a - focus).hypot2().total_cmp(&(*b - focus).hypot2()));
        hits.into_iter().next()
    });
    let other_target_point = match other {
        Some(_) => other_outline.or(other_focus).unwrap_or(other_arrow_point),
        None => other_arrow_point,
    };
    let own = outline.unwrap_or(focus);
    let too_short = (other_target_point - own).hypot() <= BASE_ARROW_MIN_LENGTH;

    // 2. The other end is free: a short arrow keeps the focus.
    if other.is_none() {
        return if too_short { focus } else { own };
    }
    // 3. Both ends are bound and the arrow is short.
    if too_short {
        return resolved_target;
    }
    // 4. The normal case: on the outline.
    own
}

/// The world points of a simple (non-elbow) arrow with its bound ends re-solved, or `None` if no end is
/// bound to a live target.
pub(crate) fn simple_points<R: SceneRead>(
    scene: &R,
    d: &LinearData,
    world: &[Point],
    dragging: bool,
) -> Option<Vec<Point>> {
    let n = world.len();
    if n < 2 {
        return None;
    }
    let s = side(scene, &d.start_binding);
    let e = side(scene, &d.end_binding);
    if s.is_none() && e.is_none() {
        return None;
    }
    let mut out = world.to_vec();
    if let Some(side_s) = &s {
        let p = bound_point(world, End::Start, side_s, e.as_ref(), d.start_head, d.end_head, dragging);
        if finite(p) {
            out[0] = p;
        }
    }
    if let Some(side_e) = &e {
        let p = bound_point(world, End::End, side_e, s.as_ref(), d.start_head, d.end_head, dragging);
        if finite(p) {
            out[n - 1] = p;
        }
    }
    // An arrow whose two ends are on the same shape moves its inner points with the shape.
    if let (Some(a), Some(b)) = (&s, &e)
        && a.target.id == b.target.id
        && n > 2
    {
        let delta = out[0] - world[0];
        for p in out.iter_mut().take(n - 1).skip(1) {
            *p += delta;
        }
    }
    Some(out)
}

/// Where the given end of `arrow` sits now according to its binding, or `None` if that end is not bound to a
/// live shape. For tests and for UI that needs to know the attach point without changing anything.
pub fn bound_end_position<R: SceneRead>(scene: &R, arrow: &Element, end: End) -> Option<Point> {
    let d = arrow.linear_data()?;
    let world = crate::linear::world_points(arrow)?;
    if d.path == drafftink_scene::PathType::Elbow {
        let (s, e) = crate::elbow::ends_for(scene, d, &world)?;
        let this = match end {
            End::Start => s,
            End::End => e,
        };
        return this.bound.is_some().then_some(this.point);
    }
    let pts = simple_points(scene, d, &world, false)?;
    let b = match end {
        End::Start => &d.start_binding,
        End::End => &d.end_binding,
    };
    side(scene, b)?;
    match end {
        End::Start => pts.first().copied(),
        End::End => pts.last().copied(),
    }
}
