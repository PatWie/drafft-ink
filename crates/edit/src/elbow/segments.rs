//! Pinned ("fixed") elbow segments: moving a segment, releasing a pin, keeping pins while the ends
//! move, and cleaning up after edits.
//!
//! The model is the one Excalidraw uses (MIT): a pin `i` is the segment from `points[i-1]` to
//! `points[i]`. The arrow keeps its number of segments while its ends move; each pin replaces
//! exactly one segment of the old route. Where a pinned first or last segment would leave a shape
//! the wrong way, a hidden stub of [`BASE_PADDING`] is inserted (`start_special` / `end_special`).
//!
//! All coordinates are world coordinates. The functions are pure; the glue that reads and writes
//! elements is in [`crate::elbow`].

use drafftink_geom::binding::Heading;
use kurbo::Point;

use super::route::{
    BASE_PADDING, DEDUP_THRESHOLD, ElbowEnd, ElbowError, RouteOpts, heading_of_end, is_orthogonal, route,
};

/// Segments must be axis-aligned to within this to count as valid.
const STRICT: f64 = 1e-6;

/// A pinned segment, in world coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    /// Index of the segment's end point in the route (1 to len-1).
    pub index: usize,
    /// Where the segment starts.
    pub start: Point,
    /// Where the segment ends.
    pub end: Point,
}

/// An elbow route with its pins.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ElbowPath {
    /// The corners, first and last are the arrow's ends.
    pub points: Vec<Point>,
    /// Pinned segments, sorted by index.
    pub pins: Vec<Pin>,
    /// The first leg is a hidden stub that leaves the start shape.
    pub start_special: bool,
    /// The last leg is a hidden stub that enters the end shape.
    pub end_special: bool,
}

impl ElbowPath {
    /// A route with no pins.
    pub fn free(points: Vec<Point>) -> Self {
        Self { points, pins: Vec::new(), start_special: false, end_special: false }
    }

    /// Checks the invariants: points are finite and orthogonal, pins are sorted, in range and lie on their segments.
    pub fn is_valid(&self) -> bool {
        if self.points.len() < 2 || self.points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return false;
        }
        if !is_orthogonal(&self.points, STRICT) {
            return false;
        }
        let mut prev = 0;
        for p in &self.pins {
            if p.index < 1 || p.index >= self.points.len() || p.index <= prev {
                return false;
            }
            prev = p.index;
        }
        true
    }
}

fn horizontal(p: Point, origin: Point) -> bool {
    Heading::from_points(p, origin).is_horizontal()
}

fn set_axis(p: &mut Point, horizontal_axis_is_y: bool, v: f64) {
    if horizontal_axis_is_y {
        p.y = v;
    } else {
        p.x = v;
    }
}

fn axis(p: Point, y: bool) -> f64 {
    if y { p.y } else { p.x }
}

fn pin_for(pins: &[Pin], index: usize) -> Option<usize> {
    pins.iter().position(|p| p.index == index)
}

fn bad(path: &ElbowPath) -> Result<(), ElbowError> {
    if path.points.len() < 2 {
        return Err(ElbowError::TooFewPoints(path.points.len()));
    }
    if path.points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return Err(ElbowError::NotFinite);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Moving a segment
// ---------------------------------------------------------------------------------------------

/// Pins segment `index` (or moves its pin) so that it passes through `pointer` across its axis. The
/// number of points stays the same, except that moving the first or last segment adds the stubs
/// that keep the end legs leaving and entering the shapes.
pub fn move_segment(
    path: &ElbowPath,
    start: &ElbowEnd,
    end: &ElbowEnd,
    index: usize,
    pointer: Point,
    opts: RouteOpts,
) -> Result<ElbowPath, ElbowError> {
    bad(path)?;
    if !pointer.x.is_finite() || !pointer.y.is_finite() {
        return Err(ElbowError::NotFinite);
    }
    let last = path.points.len() - 1;
    if index < 1 || index > last {
        return Err(ElbowError::NoSuchSegment { index, last });
    }
    let (a, b) = (path.points[index - 1], path.points[index]);
    if (a.x - b.x).abs() >= DEDUP_THRESHOLD && (a.y - b.y).abs() >= DEDUP_THRESHOLD {
        return Err(ElbowError::NotOrthogonal(index));
    }
    let is_h = horizontal(b, a);
    let pin = Pin {
        index,
        start: Point::new(if is_h { a.x } else { pointer.x }, if is_h { pointer.y } else { a.y }),
        end: Point::new(if is_h { b.x } else { pointer.x }, if is_h { pointer.y } else { b.y }),
    };
    let mut pins: Vec<Pin> = path.pins.iter().filter(|p| p.index != index).cloned().collect();
    pins.push(pin);
    pins.sort_by_key(|p| p.index);
    let sh = heading_of_end(start, end, opts);
    let eh = heading_of_end(end, start, opts);
    let moved = pin_for(&pins, index).ok_or(ElbowError::NoSuchSegment { index, last })?;
    let result = handle_segment_move(path, pins, moved, sh, eh, start.bound.is_some(), end.bound.is_some())?;
    // A segment dragged onto its neighbours leaves zero-length legs; merge them (and the pins on them) now.
    valid_or_reroute(renormalize(&result, start, end, opts)?, start, end, opts)
}

/// A result that is not strictly orthogonal (an edit met geometry it could not follow) drops its pins and is routed
/// again, so the stored route is always valid.
fn valid_or_reroute(
    path: ElbowPath,
    start: &ElbowEnd,
    end: &ElbowEnd,
    opts: RouteOpts,
) -> Result<ElbowPath, ElbowError> {
    if path.is_valid() { Ok(path) } else { Ok(ElbowPath::free(route(start, end, opts)?)) }
}

fn handle_segment_move(
    path: &ElbowPath,
    mut pins: Vec<Pin>,
    modified: usize,
    sh: Heading,
    eh: Heading,
    start_bound: bool,
    end_bound: bool,
) -> Result<ElbowPath, ElbowError> {
    let len = path.points.len();
    let first_pinned = path.pins.iter().any(|p| p.index == 1);
    let last_pinned = path.pins.iter().any(|p| p.index == len - 1);
    let seg_len = {
        let s = pins.get(modified).ok_or(ElbowError::EndSegment)?;
        (s.end - s.start).hypot()
    };
    let too_short = seg_len < BASE_PADDING + 5.0;
    let pad_along = |heading: Heading| -> f64 {
        let positive = matches!(heading, Heading::Right | Heading::Down);
        let amount = if too_short { seg_len / 2.0 } else { BASE_PADDING };
        if positive { amount } else { -amount }
    };
    if !first_pinned && pins[modified].index == 1 && start_bound {
        let pad = pad_along(sh);
        let s = &mut pins[modified];
        if sh.is_horizontal() {
            s.start.x += pad;
        } else {
            s.start.y += pad;
        }
    }
    if !last_pinned && pins[modified].index == len - 1 && end_bound {
        let pad = pad_along(eh);
        let s = &mut pins[modified];
        if eh.is_horizontal() {
            s.end.x += pad;
        } else {
            s.end.y += pad;
        }
    }

    let mut np = path.points.clone();
    let (start_idx, end_idx) = (pins[modified].index - 1, pins[modified].index);
    let (start, end) = (pins[modified].start, pins[modified].end);
    let seg_is_h = horizontal(end, start);
    let prev_h =
        (start_idx >= 1 && np[start_idx] != np[start_idx - 1]).then(|| horizontal(np[start_idx - 1], np[start_idx]));
    let next_h =
        (end_idx + 1 < len && np[end_idx] != np[end_idx + 1]).then(|| horizontal(np[end_idx + 1], np[end_idx]));
    if let Some(h) = prev_h {
        set_axis(&mut np[start_idx - 1], h, axis(start, h));
    }
    np[start_idx] = start;
    np[end_idx] = end;
    if let Some(h) = next_h {
        set_axis(&mut np[end_idx + 1], h, axis(end, h));
    }

    // Neighbouring pins follow, so they stay connected to the moved one.
    if let Some(pi) = pins.iter().position(|p| p.index == start_idx) {
        let h = horizontal(pins[pi].end, pins[pi].start);
        let v = axis(start, h);
        set_axis(&mut pins[pi].start, h, v);
        pins[pi].end = start;
    }
    if let Some(ni) = pins.iter().position(|p| p.index == end_idx + 1) {
        let h = horizontal(pins[ni].end, pins[ni].start);
        let v = axis(end, h);
        set_axis(&mut pins[ni].end, h, v);
        pins[ni].start = end;
    }

    let orig_start = *path.points.first().ok_or(ElbowError::TooFewPoints(0))?;
    let orig_end = *path.points.last().ok_or(ElbowError::TooFewPoints(0))?;
    if !first_pinned && start_idx == 0 {
        // The first leg was moved: keep a leg that leaves the start.
        np.insert(
            0,
            Point::new(if seg_is_h { start.x } else { orig_start.x }, if seg_is_h { orig_start.y } else { start.y }),
        );
        if start_bound {
            np.insert(0, orig_start);
        }
        let shift = if start_bound { 2 } else { 1 };
        for p in &mut pins {
            p.index += shift;
        }
    }
    if !last_pinned && end_idx == len - 1 {
        np.push(Point::new(if seg_is_h { end.x } else { orig_end.x }, if seg_is_h { orig_end.y } else { end.y }));
        if end_bound {
            np.push(orig_end);
        }
    }
    Ok(ElbowPath { points: np, pins, start_special: false, end_special: false })
}

// ---------------------------------------------------------------------------------------------
// Releasing a pin
// ---------------------------------------------------------------------------------------------

/// Unpins segment `index`. The part of the route between the neighbouring pins (or the ends) is routed
/// again; the rest is untouched. Releasing an index that is not pinned changes nothing.
pub fn release_segment(
    path: &ElbowPath,
    start: &ElbowEnd,
    end: &ElbowEnd,
    index: usize,
    opts: RouteOpts,
) -> Result<ElbowPath, ElbowError> {
    bad(path)?;
    let Some(di) = pin_for(&path.pins, index) else { return Ok(path.clone()) };
    let prev = di.checked_sub(1).and_then(|i| path.pins.get(i));
    let next = path.pins.get(di + 1);
    let new_pins: Vec<Pin> = path.pins.iter().filter(|p| p.index != index).cloned().collect();
    if new_pins.is_empty() {
        // The last pin is gone: the whole arrow is routed freely again.
        return Ok(ElbowPath::free(route(start, end, opts)?));
    }

    let s_end = match prev {
        Some(p) => ElbowEnd::free(p.end, drafftink_scene::Head::None),
        None => ElbowEnd { head: drafftink_scene::Head::None, ..start.clone() },
    };
    let e_end = match next {
        Some(p) => ElbowEnd::free(p.start, drafftink_scene::Head::None),
        None => ElbowEnd { head: drafftink_scene::Head::None, ..end.clone() },
    };
    let restored = route(&s_end, &e_end, opts)?;

    let mut np: Vec<Point> = Vec::new();
    if let Some(p) = prev {
        np.extend(path.points.iter().take(p.index));
    }
    np.extend(restored.iter().copied());
    if let Some(p) = next {
        np.extend(path.points.iter().skip(p.index));
    }
    let old_count = next.map_or(path.points.len(), |p| p.index) as isize - prev.map_or(0, |p| p.index) as isize - 1;
    let restored_segments = restored.len() as isize - 1;
    let mut pins: Vec<Pin> = new_pins
        .into_iter()
        .map(|mut p| {
            if p.index > index {
                p.index = (p.index as isize - old_count + restored_segments).max(1) as usize;
            }
            p
        })
        .collect();

    // Merge collinear points, split back-tracking ones, and move the pin indices with them.
    let mut events: Vec<(usize, isize)> = Vec::new();
    let mut simplified: Vec<Point> = Vec::with_capacity(np.len());
    for (i, p) in np.iter().enumerate() {
        if i > 0 && i + 1 < np.len() {
            let prev_h = Heading::from_points(*p, np[i - 1]);
            let next_h = Heading::from_points(np[i + 1], *p);
            if prev_h == next_h {
                events.push((i, -1));
                continue;
            } else if prev_h == next_h.flip() {
                events.push((i, 1));
                simplified.push(*p);
                simplified.push(*p);
                continue;
            }
        }
        simplified.push(*p);
    }
    for pin in &mut pins {
        let delta: isize = events.iter().filter(|(i, _)| pin.index > *i).map(|(_, d)| *d).sum();
        pin.index = (pin.index as isize + delta).max(1) as usize;
    }
    let result = refresh_pins(ElbowPath { points: simplified, pins, start_special: false, end_special: false });
    valid_or_reroute(renormalize(&result, start, end, opts)?, start, end, opts)
}

/// Sets every pin's start and end from the route points.
fn refresh_pins(mut path: ElbowPath) -> ElbowPath {
    let n = path.points.len();
    path.pins.retain(|p| p.index >= 1 && p.index < n);
    for p in &mut path.pins {
        p.start = path.points[p.index - 1];
        p.end = path.points[p.index];
    }
    path
}

// ---------------------------------------------------------------------------------------------
// Moving the ends while pins stay
// ---------------------------------------------------------------------------------------------

/// Re-routes `path` for new ends. Without pins the whole route is made again. With pins the route keeps
/// its shape: the pinned segments stay, and only the legs next to the ends change. If the pins cannot
/// be kept (the result would not be orthogonal), they are dropped and the route is made again.
pub fn update_ends(
    path: &ElbowPath,
    start: &ElbowEnd,
    end: &ElbowEnd,
    opts: RouteOpts,
) -> Result<ElbowPath, ElbowError> {
    if path.pins.is_empty() || path.points.len() < 4 {
        return Ok(ElbowPath::free(route(start, end, opts)?));
    }
    match keep_pins(path, start, end, opts) {
        Ok(Some(p)) if p.is_valid() => valid_or_reroute(renormalize(&p, start, end, opts)?, start, end, opts),
        _ => Ok(ElbowPath::free(route(start, end, opts)?)),
    }
}

fn keep_pins(
    path: &ElbowPath,
    start: &ElbowEnd,
    end: &ElbowEnd,
    opts: RouteOpts,
) -> Result<Option<ElbowPath>, ElbowError> {
    bad(path)?;
    let sh = heading_of_end(start, end, opts);
    let eh = heading_of_end(end, start, opts);
    let len = path.points.len();
    let mut start_special = path.start_special;
    let mut end_special = path.end_special;
    let mut g = path.points.clone();
    g[0] = start.point;
    g[len - 1] = end.point;
    let mut pins = path.pins.clone();

    let offset = 2 + usize::from(start_special);
    let end_offset = 2 + usize::from(end_special);
    if offset + end_offset > len {
        return Ok(None);
    }
    let mut np: Vec<Point> = g[offset..len - end_offset].to_vec();

    // The start side.
    {
        let (Some(second), Some(third)) =
            (g.get(if start_special { 2 } else { 1 }), g.get(if start_special { 3 } else { 2 }))
        else {
            return Ok(None);
        };
        let (second, third) = (*second, *third);
        let start_h = sh.is_horizontal();
        let second_h = Heading::from_vector(second - third).is_horizontal();
        if start.bound.is_some() && start_h == second_h {
            let pad = sh.vector() * BASE_PADDING;
            np.insert(
                0,
                Point::new(
                    if second_h { start.point.x + pad.x } else { third.x },
                    if second_h { third.y } else { start.point.y + pad.y },
                ),
            );
            np.insert(0, Point::new(start.point.x + pad.x, start.point.y + pad.y));
            if !start_special {
                start_special = true;
                for p in &mut pins {
                    if p.index > 1 {
                        p.index += 1;
                    }
                }
            }
        } else {
            np.insert(
                0,
                Point::new(
                    if second_h { start.point.x } else { second.x },
                    if second_h { second.y } else { start.point.y },
                ),
            );
            if start_special {
                start_special = false;
                for p in &mut pins {
                    if p.index > 1 {
                        p.index -= 1;
                    }
                }
            }
        }
        np.insert(0, start.point);
    }

    // The end side.
    {
        let (Some(second), Some(third)) = (
            g.get(len.wrapping_sub(if end_special { 3 } else { 2 })),
            g.get(len.wrapping_sub(if end_special { 4 } else { 3 })),
        ) else {
            return Ok(None);
        };
        let (second, third) = (*second, *third);
        let end_h = eh.is_horizontal();
        let second_h = horizontal(third, second);
        if end.bound.is_some() && end_h == second_h {
            let pad = eh.vector() * BASE_PADDING;
            np.push(Point::new(
                if second_h { end.point.x + pad.x } else { third.x },
                if second_h { third.y } else { end.point.y + pad.y },
            ));
            np.push(Point::new(end.point.x + pad.x, end.point.y + pad.y));
            end_special = true;
        } else {
            np.push(Point::new(
                if second_h { end.point.x } else { second.x },
                if second_h { second.y } else { end.point.y },
            ));
            end_special = false;
        }
        np.push(end.point);
    }

    let result = ElbowPath { points: np, pins, start_special, end_special };
    Ok(Some(refresh_pins(result)))
}

// ---------------------------------------------------------------------------------------------
// Cleaning up
// ---------------------------------------------------------------------------------------------

/// Merges collinear legs, removes legs shorter than one unit and drops the pins they touch. A pin on the
/// first or last leg is dropped too. With no pin left the route is made again from scratch.
pub fn renormalize(
    path: &ElbowPath,
    start: &ElbowEnd,
    end: &ElbowEnd,
    opts: RouteOpts,
) -> Result<ElbowPath, ElbowError> {
    bad(path)?;
    if path.pins.is_empty() {
        return Ok(path.clone());
    }
    let mut pins = path.pins.clone();
    let mut kept: Vec<Point> = Vec::with_capacity(path.points.len());
    for (i, &p) in path.points.iter().enumerate() {
        if i < 2 {
            kept.push(p);
            continue;
        }
        let cur = Heading::from_points(p, path.points[i - 1]);
        let prev = Heading::from_points(path.points[i - 1], path.points[i - 2]);
        if cur == prev {
            let prev_pin = pin_for(&pins, i - 1);
            if let Some(si) = pin_for(&pins, i) {
                pins[si].start = path.points[i - 2];
            }
            if let Some(pi) = prev_pin {
                pins.remove(pi);
            }
            kept.pop();
            for pin in &mut pins {
                if pin.index > i - 1 {
                    pin.index -= 1;
                }
            }
        }
        kept.push(p);
    }

    let mut next: Vec<Point> = Vec::with_capacity(kept.len());
    for (i, &p) in kept.iter().enumerate() {
        if i < 3 {
            next.push(p);
            continue;
        }
        if (kept[i - 2] - kept[i - 1]).hypot() < DEDUP_THRESHOLD {
            if let Some(pi) = pin_for(&pins, i - 1) {
                pins.remove(pi);
            }
            if let Some(pi) = pin_for(&pins, i - 2) {
                pins.remove(pi);
            }
            let keep = next.len().saturating_sub(2);
            next.truncate(keep);
            for pin in &mut pins {
                if pin.index > i - 2 {
                    pin.index = pin.index.saturating_sub(2).max(1);
                }
            }
            // Reconnect: slide the point before the removed leg along its own leg until it lines up with `p`. The
            // points `p` and the ends are never moved.
            let qi = next.len().wrapping_sub(1);
            if let Some(q) = next.last().copied() {
                let aligned = (q.x - p.x).abs() < 1e-9 || (q.y - p.y).abs() < 1e-9;
                if !aligned {
                    if qi >= 1 {
                        let before = next[qi - 1];
                        if (before.x - q.x).abs() < 1e-9 {
                            next[qi].y = p.y;
                        } else {
                            next[qi].x = p.x;
                        }
                    } else {
                        next.push(Point::new(p.x, q.y));
                    }
                }
            }
            next.push(p);
            continue;
        }
        next.push(p);
    }

    // Zero-length legs the loops above do not look at (at the very start or end).
    let mut j = 1;
    while j < next.len() {
        if (next[j] - next[j - 1]).hypot() < 1e-9 && next.len() > 2 {
            next.remove(j);
            pins.retain(|p| p.index != j);
            for pin in &mut pins {
                if pin.index > j {
                    pin.index -= 1;
                }
            }
        } else {
            j += 1;
        }
    }
    let n = next.len();
    pins.retain(|p| p.index != 1 && p.index + 1 != n && p.index < n);
    if pins.is_empty() {
        let routed = route(start, end, opts)?;
        return Ok(ElbowPath::free(routed));
    }
    Ok(refresh_pins(ElbowPath { points: next, pins, start_special: path.start_special, end_special: path.end_special }))
}
