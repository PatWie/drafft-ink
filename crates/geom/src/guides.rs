//! Smart guides: alignment and equal-spacing candidates for a dragged box, as pure functions over
//! rects.
//!
//! Ported from the old app's `snap.rs` with the same constants, the same tie-breaking and the same
//! guide order. The horizontal and vertical equal-spacing passes were two copies of one routine;
//! here one routine runs on transposed rects. Non-finite rects are ignored, and a NaN threshold
//! snaps nothing.
//!
//! A zero-area rect in `others` is a point, such as a line vertex: it can align the dragged box
//! but does not take part in equal spacing.

use kurbo::{Point, Rect};

use crate::aabb;
use crate::snap::EQUAL_SPACING_SNAP_RADIUS;

/// Equal spacing is skipped when more than this many boxes with area are passed in.
pub const MAX_SPACING_CANDIDATES: usize = 50;

/// A guide line to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct SmartGuide {
    /// What the guide aligns.
    pub kind: SmartGuideKind,
    /// x for a vertical guide, y for a horizontal one; the cross coordinate for spacing guides.
    pub position: f64,
    /// Start of the guide along its direction.
    pub start: f64,
    /// End of the guide along its direction.
    pub end: f64,
    /// Marker coordinates along the guide (y values for vertical, x values for horizontal).
    pub snap_points: Vec<f64>,
    /// For an alignment guide: the empty stretch along the guide between the dragged thing and the box it lines up
    /// with, as `(from, to)` in the guide's coordinates. A badge shows its length. `None` when the two overlap along
    /// the guide, and for spacing guides (which show their own distance).
    pub gap: Option<(f64, f64)>,
}

/// The kind of a [`SmartGuide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartGuideKind {
    /// Same x.
    Vertical,
    /// Same y.
    Horizontal,
    /// Equal gaps left and right.
    EqualSpacingH,
    /// Equal gaps above and below.
    EqualSpacingV,
}

/// Result of smart guide detection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmartGuideResult {
    /// For a box: the snapped top-left corner. For a point: the snapped point.
    pub point: Point,
    /// The guides to draw, in drawing order.
    pub guides: Vec<SmartGuide>,
    /// True when the x coordinate moved to a guide.
    pub snapped_x: bool,
    /// True when the y coordinate moved to a guide.
    pub snapped_y: bool,
}

/// `(snapped top-left coordinate, guide coordinate, the rect that caused it)` on one axis.
type AxisBest = Option<(f64, f64, Rect)>;

fn transpose(r: Rect) -> Rect {
    Rect::new(r.y0, r.x0, r.y1, r.x1)
}

fn mid(a: f64, b: f64) -> f64 {
    (a + b) / 2.0
}

/// A guide counts as touching a box line when it is this close, in world units.
const MARKER_TOLERANCE: f64 = 0.1;

/// The three vertical lines of a box: left edge, right edge, centre.
fn x_lines(r: &Rect) -> [f64; 3] {
    [r.x0, r.x1, mid(r.x0, r.x1)]
}

/// The three horizontal lines of a box: top edge, bottom edge, centre.
fn y_lines(r: &Rect) -> [f64; 3] {
    [r.y0, r.y1, mid(r.y0, r.y1)]
}

/// Add the marker coordinates of every box with a line on the guide at `position`.
///
/// `along` gives the box lines that run parallel to the guide (compared with `position`);
/// `across` gives the coordinates to mark on the guide.
fn extend_markers(
    markers: &mut Vec<f64>,
    others: &[Rect],
    position: f64,
    along: fn(&Rect) -> [f64; 3],
    across: fn(&Rect) -> [f64; 3],
) {
    for other in others {
        for line in along(other) {
            if (line - position).abs() < MARKER_TOLERANCE {
                markers.extend(across(other));
            }
        }
    }
}

/// The empty stretch between two ranges on a line, `(from, to)`. `None` when they touch or overlap.
fn gap_between(a: (f64, f64), b: (f64, f64)) -> Option<(f64, f64)> {
    let (a, b) = ((a.0.min(a.1), a.0.max(a.1)), (b.0.min(b.1), b.0.max(b.1)));
    if a.1 < b.0 {
        Some((a.1, b.0))
    } else if b.1 < a.0 {
        Some((b.1, a.0))
    } else {
        None
    }
}

fn guide(kind: SmartGuideKind, position: f64, start: f64, end: f64) -> SmartGuide {
    SmartGuide { kind, position, start, end, snap_points: Vec::new(), gap: None }
}

/// The closest alignment of the dragged box's left, right and centre with those of any other box, closer than `dist`
/// (which is lowered to the distance found): where the dragged box's left edge must go, the guide's x, and the box
/// aligned with.
fn closest_alignment(dragged: Rect, others: &[Rect], dist: &mut f64) -> AxisBest {
    let mut best: AxisBest = None;
    for other in others {
        for dx in x_lines(&dragged) {
            for ox in x_lines(other) {
                let d = (dx - ox).abs();
                if d < *dist {
                    *dist = d;
                    best = Some((dragged.x0 + (ox - dx), ox, *other));
                }
            }
        }
    }
    best
}

/// Detect smart guides for a dragged box against other boxes.
///
/// Compares the left, centre and right of the dragged box with those of every other box (and top,
/// middle, bottom likewise) and snaps to the closest one under `threshold`. Equal-spacing
/// positions compete with alignment: they win when they are closer.
pub fn detect_smart_guides(dragged: Rect, others: &[Rect], threshold: f64) -> SmartGuideResult {
    let mut result = SmartGuideResult { point: Point::new(dragged.x0, dragged.y0), ..Default::default() };
    if !aabb::is_finite(&dragged) || !threshold.is_finite() {
        return result;
    }
    let dragged = aabb::normalize(dragged);
    let others: Vec<Rect> = others.iter().filter(|r| aabb::is_finite(r)).map(|r| aabb::normalize(*r)).collect();

    let (cy, w) = (mid(dragged.y0, dragged.y1), dragged.x1 - dragged.x0);

    let mut dist_x = threshold;
    let mut dist_y = threshold;
    let mut best_dx: AxisBest = closest_alignment(dragged, &others, &mut dist_x);
    // The vertical pass is the horizontal pass on transposed rects.
    let others_t: Vec<Rect> = others.iter().copied().map(transpose).collect();
    let mut best_dy: AxisBest =
        closest_alignment(transpose(dragged), &others_t, &mut dist_y).map(|(a, b, r)| (a, b, transpose(r)));

    // Equal spacing looks at real boxes only, not points.
    let boxes: Vec<Rect> = others.iter().filter(|r| !r.is_zero_area()).copied().collect();
    if boxes.len() <= MAX_SPACING_CANDIDATES {
        let h_pool: Vec<Rect> = boxes.iter().filter(|b| b.y0 < dragged.y1 && b.y1 > dragged.y0).copied().collect();
        let v_pool: Vec<Rect> = boxes.iter().filter(|b| b.x0 < dragged.x1 && b.x1 > dragged.x0).copied().collect();

        equal_spacing(&h_pool, dragged, SmartGuideKind::EqualSpacingH, &mut dist_x, &mut best_dx, &mut result.guides);

        // The vertical pass is the horizontal pass on transposed rects.
        let t_pool: Vec<Rect> = v_pool.into_iter().map(transpose).collect();
        let mut t_best = best_dy.map(|(a, b, r)| (a, b, transpose(r)));
        equal_spacing(
            &t_pool,
            transpose(dragged),
            SmartGuideKind::EqualSpacingV,
            &mut dist_y,
            &mut t_best,
            &mut result.guides,
        );
        best_dy = t_best.map(|(a, b, r)| (a, b, transpose(r)));
    }

    if let Some((snap_x, guide_x, other)) = best_dx {
        result.point.x = snap_x;
        result.snapped_x = true;
        let lo = dragged.y0.min(dragged.y1).min(other.y0).min(other.y1);
        let hi = dragged.y0.max(dragged.y1).max(other.y0).max(other.y1);
        let mut snap_points = vec![dragged.y0, dragged.y1, cy];
        extend_markers(&mut snap_points, &others, guide_x, x_lines, y_lines);
        result.guides.push(SmartGuide {
            kind: SmartGuideKind::Vertical,
            position: guide_x,
            start: lo,
            end: hi,
            snap_points,
            gap: gap_between((dragged.y0, dragged.y1), (other.y0, other.y1)),
        });
    }

    if let Some((snap_y, guide_y, other)) = best_dy {
        result.point.y = snap_y;
        result.snapped_y = true;
        let x0 = if result.snapped_x { result.point.x } else { dragged.x0 };
        let x1 = x0 + w;
        let lo = x0.min(x1).min(other.x0).min(other.x1);
        let hi = x0.max(x1).max(other.x0).max(other.x1);
        let mut snap_points = vec![x0, x1, x0 + w / 2.0];
        extend_markers(&mut snap_points, &others, guide_y, y_lines, x_lines);
        result.guides.push(SmartGuide {
            kind: SmartGuideKind::Horizontal,
            position: guide_y,
            start: lo,
            end: hi,
            snap_points,
            gap: gap_between((x0, x1), (other.x0, other.x1)),
        });
    }
    result
}

/// The line closest to `coord` among `lines` (a position and the extent of the box it belongs to) and closer than
/// `dist`, which is lowered to the distance found. The first of equally close lines wins.
fn closest_line(coord: f64, lines: impl Iterator<Item = (f64, (f64, f64))>, dist: &mut f64) -> Option<(f64, f64, f64)> {
    let mut best = None;
    for (pos, (lo, hi)) in lines {
        let d = (coord - pos).abs();
        if d < *dist {
            *dist = d;
            best = Some((pos, lo, hi));
        }
    }
    best
}

/// Detect smart guides for a single point, such as a line or arrow endpoint, against boxes.
/// There is no equal spacing for points.
pub fn detect_smart_guides_for_point(point: Point, others: &[Rect], threshold: f64) -> SmartGuideResult {
    let mut result = SmartGuideResult { point, ..Default::default() };
    if !crate::point_is_finite(point) || !threshold.is_finite() {
        return result;
    }
    let others: Vec<Rect> = others.iter().filter(|r| aabb::is_finite(r)).map(|r| aabb::normalize(*r)).collect();
    let across = |r: &Rect| (r.y0, r.y1);
    let mut dist_x = threshold;
    let mut dist_y = threshold;
    let best_dx = closest_line(point.x, others.iter().flat_map(|o| x_lines(o).map(|ox| (ox, across(o)))), &mut dist_x);
    let best_dy =
        closest_line(point.y, others.iter().flat_map(|o| y_lines(o).map(|oy| (oy, (o.x0, o.x1)))), &mut dist_y);

    if let Some((gx, y0, y1)) = best_dx {
        result.point.x = gx;
        result.snapped_x = true;
        let mut snap_points = vec![point.y];
        extend_markers(&mut snap_points, &others, gx, x_lines, y_lines);
        result.guides.push(SmartGuide {
            kind: SmartGuideKind::Vertical,
            position: gx,
            start: point.y.min(y0).min(y1),
            end: point.y.max(y0).max(y1),
            snap_points,
            gap: gap_between((point.y, point.y), (y0, y1)),
        });
    }
    if let Some((gy, x0, x1)) = best_dy {
        result.point.y = gy;
        result.snapped_y = true;
        let sx = if result.snapped_x { result.point.x } else { point.x };
        let mut snap_points = vec![sx];
        extend_markers(&mut snap_points, &others, gy, y_lines, x_lines);
        result.guides.push(SmartGuide {
            kind: SmartGuideKind::Horizontal,
            position: gy,
            start: sx.min(x0).min(x1),
            end: sx.max(x0).max(x1),
            snap_points,
            gap: gap_between((sx, sx), (x0, x1)),
        });
    }
    result
}

/// While an angle is locked, snap the point on the ray to the nearest crossing with the edge or
/// centre lines of other boxes. Only the single best crossing within `threshold` is used.
pub fn snap_ray_to_smart_guides(
    origin: Point,
    angle_degrees: f64,
    target: Point,
    others: &[Rect],
    threshold: f64,
) -> SmartGuideResult {
    let mut result = SmartGuideResult { point: target, ..Default::default() };
    if !crate::point_is_finite(origin)
        || !crate::point_is_finite(target)
        || !angle_degrees.is_finite()
        || !threshold.is_finite()
    {
        return result;
    }
    let (sin_a, cos_a) = angle_degrees.to_radians().sin_cos();
    let mut best: Option<(Point, SmartGuide)> = None;
    let mut best_dist = f64::MAX;

    for o in others.iter().filter(|r| aabb::is_finite(r)) {
        let (ocx, ocy) = (mid(o.x0, o.x1), mid(o.y0, o.y1));
        if cos_a.abs() > 0.001 {
            for ox in [o.x0, o.x1, ocx] {
                let t = (ox - origin.x) / cos_a;
                if t > 0.0 {
                    let iy = origin.y + t * sin_a;
                    let dist = (ox - target.x).hypot(iy - target.y);
                    if dist < threshold && dist < best_dist {
                        best_dist = dist;
                        best = Some((
                            Point::new(ox, iy),
                            SmartGuide {
                                kind: SmartGuideKind::Vertical,
                                position: ox,
                                start: iy.min(o.y0).min(o.y1),
                                end: iy.max(o.y0).max(o.y1),
                                snap_points: vec![iy, o.y0, o.y1, ocy],
                                gap: gap_between((iy, iy), (o.y0, o.y1)),
                            },
                        ));
                    }
                }
            }
        }
        if sin_a.abs() > 0.001 {
            for oy in [o.y0, o.y1, ocy] {
                let t = (oy - origin.y) / sin_a;
                if t > 0.0 {
                    let ix = origin.x + t * cos_a;
                    let dist = (ix - target.x).hypot(oy - target.y);
                    if dist < threshold && dist < best_dist {
                        best_dist = dist;
                        best = Some((
                            Point::new(ix, oy),
                            SmartGuide {
                                kind: SmartGuideKind::Horizontal,
                                position: oy,
                                start: ix.min(o.x0).min(o.x1),
                                end: ix.max(o.x0).max(o.x1),
                                snap_points: vec![ix, o.x0, o.x1, ocx],
                                gap: gap_between((ix, ix), (o.x0, o.x1)),
                            },
                        ));
                    }
                }
            }
        }
    }
    if let Some((p, g)) = best {
        result.point = p;
        result.snapped_x = true;
        result.snapped_y = true;
        result.guides.push(g);
    }
    result
}

/// Horizontal equal spacing with chain walking. `candidates` overlap the dragged box on the cross
/// axis. `kind` names the guides it emits.
///
/// Three placements are tried for every pair of boxes with a positive gap up to
/// [`EQUAL_SPACING_SNAP_RADIUS`]: between the pair (gap split evenly), to the right of the pair
/// and to the left of it (both at the pair's gap). A placement wins when its distance to the
/// dragged left edge beats `best_dist`. Afterwards the chain is extended outward over boxes at the
/// same spacing (within 1 unit).
fn equal_spacing(
    candidates: &[Rect],
    dragged: Rect,
    kind: SmartGuideKind,
    best_dist: &mut f64,
    best: &mut AxisBest,
    guides: &mut Vec<SmartGuide>,
) {
    let mut sorted: Vec<Rect> = candidates.to_vec();
    sorted.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    let dragged_w = dragged.x1 - dragged.x0;
    let dragged_cy = mid(dragged.y0, dragged.y1);

    let mut spacing: Option<(f64, usize, usize)> = None;
    let replace = |guides: &mut Vec<SmartGuide>| guides.retain(|g| g.kind != kind);

    for i in 0..sorted.len() {
        for j in (i + 1)..sorted.len() {
            let (left, right) = (sorted[i], sorted[j]);
            if left.x1 >= right.x0 {
                continue;
            }
            let gap = right.x0 - left.x1;
            if gap > EQUAL_SPACING_SNAP_RADIUS {
                continue;
            }

            // Between the pair.
            if gap > dragged_w {
                let target = left.x1 + (gap - dragged_w) / 2.0;
                let dist = (dragged.x0 - target).abs();
                if dist < *best_dist {
                    *best_dist = dist;
                    *best = Some((target, target + dragged_w / 2.0, left));
                    spacing = Some(((gap - dragged_w) / 2.0, i, j));
                    replace(guides);
                    guides.push(guide(kind, dragged_cy, left.x1, target));
                    guides.push(guide(kind, dragged_cy, target + dragged_w, right.x0));
                }
            }

            // Right of the pair.
            let target_r = right.x1 + gap;
            let dist_r = (dragged.x0 - target_r).abs();
            if dist_r < *best_dist {
                *best_dist = dist_r;
                *best = Some((target_r, target_r + dragged_w / 2.0, right));
                spacing = Some((gap, i, j));
                replace(guides);
                guides.push(guide(kind, mid(left.y0, left.y1), left.x1, right.x0));
                guides.push(guide(kind, dragged_cy, right.x1, target_r));
            }

            // Left of the pair.
            let target_l = left.x0 - gap - dragged_w;
            let dist_l = (dragged.x0 - target_l).abs();
            if dist_l < *best_dist {
                *best_dist = dist_l;
                *best = Some((target_l, target_l + dragged_w / 2.0, left));
                spacing = Some((gap, i, j));
                replace(guides);
                guides.push(guide(kind, dragged_cy, target_l + dragged_w, left.x0));
                guides.push(guide(kind, mid(right.y0, right.y1), left.x1, right.x0));
            }
        }
    }

    if let Some((spacing, li, ri)) = spacing {
        let mut cur = sorted[li];
        for k in (0..li).rev() {
            let cand = sorted[k];
            if (cand.x1 - (cur.x0 - spacing)).abs() < 1.0 {
                let y = mid(cand.y0, cand.y1).min(mid(cur.y0, cur.y1));
                guides.push(guide(kind, y, cand.x1, cur.x0));
                cur = cand;
            }
        }
        cur = sorted[ri];
        for cand in &sorted[(ri + 1)..] {
            if (cand.x0 - (cur.x1 + spacing)).abs() < 1.0 {
                let y = mid(cand.y0, cand.y1).min(mid(cur.y0, cur.y1));
                guides.push(guide(kind, y, cur.x1, cand.x0));
                cur = *cand;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::SMART_GUIDE_THRESHOLD;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, x + w, y + h)
    }

    #[test]
    fn aligns_left_edges() {
        let res = detect_smart_guides(r(103.0, 300.0, 50.0, 50.0), &[r(100.0, 0.0, 80.0, 80.0)], SMART_GUIDE_THRESHOLD);
        assert!(res.snapped_x && !res.snapped_y);
        assert_eq!(res.point, Point::new(100.0, 300.0));
        assert_eq!(res.guides.len(), 1);
        let g = &res.guides[0];
        assert_eq!(g.kind, SmartGuideKind::Vertical);
        assert_eq!(g.position, 100.0);
        assert_eq!((g.start, g.end), (0.0, 350.0));
        assert!(g.snap_points.contains(&0.0) && g.snap_points.contains(&80.0) && g.snap_points.contains(&40.0));
    }

    #[test]
    fn centre_to_centre_and_right_to_left() {
        // dragged centre x = 40+25 = 65... other centre = 100+40 = 140 -> not near. Use matching centres.
        let res = detect_smart_guides(r(116.0, 500.0, 50.0, 10.0), &[r(100.0, 0.0, 80.0, 80.0)], 8.0);
        assert!(res.snapped_x);
        assert_eq!(res.point.x, 115.0, "centres 141 vs 140: shift by -1");
        // right edge of dragged to left edge of other: dragged.x1 = 98, other.x0 = 100
        let res = detect_smart_guides(r(48.0, 500.0, 50.0, 10.0), &[r(100.0, 0.0, 80.0, 80.0)], 8.0);
        assert_eq!(res.point.x, 50.0);
    }

    #[test]
    fn threshold_is_strict() {
        let wide = [r(100.0, 0.0, 100.0, 80.0)];
        let res = detect_smart_guides(r(108.0, 300.0, 50.0, 50.0), &wide, 8.0);
        assert!(!res.snapped_x, "distance equal to the threshold does not snap");
        let res = detect_smart_guides(r(107.9, 300.0, 50.0, 50.0), &wide, 8.0);
        assert!(res.snapped_x);
    }

    #[test]
    fn both_axes_and_the_closest_wins() {
        let others = [r(100.0, 100.0, 40.0, 40.0), r(104.0, 400.0, 40.0, 40.0)];
        let res = detect_smart_guides(r(101.0, 102.0, 40.0, 40.0), &others, 8.0);
        assert!(res.snapped_x && res.snapped_y);
        assert_eq!(res.point, Point::new(100.0, 100.0));
        assert_eq!(res.guides.len(), 2);
        assert_eq!(res.guides[0].kind, SmartGuideKind::Vertical);
        assert_eq!(res.guides[1].kind, SmartGuideKind::Horizontal);
    }

    #[test]
    fn zero_area_points_align_but_do_not_space() {
        let pt = Rect::new(200.0, 200.0, 200.0, 200.0);
        let res = detect_smart_guides(r(204.0, 500.0, 10.0, 10.0), &[pt], 8.0);
        assert!(res.snapped_x);
        assert_eq!(res.point.x, 200.0);
    }

    #[test]
    fn equal_spacing_between_two_boxes() {
        // Boxes at x 0..100 and 300..400 (gap 200). A 50-wide box dragged to the middle: x0 should be 175.
        let others = [r(0.0, 0.0, 100.0, 100.0), r(300.0, 0.0, 100.0, 100.0)];
        let res = detect_smart_guides(r(172.0, 10.0, 50.0, 50.0), &others, 8.0);
        assert!(res.snapped_x);
        assert_eq!(res.point.x, 175.0);
        let spacing: Vec<_> = res.guides.iter().filter(|g| g.kind == SmartGuideKind::EqualSpacingH).collect();
        assert_eq!(spacing.len(), 2);
        assert_eq!((spacing[0].start, spacing[0].end), (100.0, 175.0));
        assert_eq!((spacing[1].start, spacing[1].end), (225.0, 300.0));
        assert_eq!(spacing[0].position, 35.0, "guide sits at the dragged box's vertical centre");
    }

    #[test]
    fn equal_spacing_continues_a_row() {
        // Two boxes with gap 40; dragged box goes to the right at the same gap.
        let others = [r(0.0, 0.0, 100.0, 100.0), r(140.0, 0.0, 100.0, 100.0)];
        let res = detect_smart_guides(r(276.0, 10.0, 100.0, 50.0), &others, 8.0);
        assert!(res.snapped_x);
        assert_eq!(res.point.x, 280.0);
        assert!(
            res.guides.iter().any(|g| g.kind == SmartGuideKind::EqualSpacingH && g.start == 240.0 && g.end == 280.0)
        );
        // and to the left
        let res = detect_smart_guides(r(-143.0, 10.0, 100.0, 50.0), &others, 8.0);
        assert_eq!(res.point.x, -140.0);
    }

    #[test]
    fn equal_spacing_chain_walks_over_a_longer_row() {
        let others = [r(0.0, 0.0, 50.0, 50.0), r(90.0, 0.0, 50.0, 50.0), r(180.0, 0.0, 50.0, 50.0)];
        let res = detect_smart_guides(r(267.0, 0.0, 50.0, 50.0), &others, 8.0);
        assert_eq!(res.point.x, 270.0);
        let n = res.guides.iter().filter(|g| g.kind == SmartGuideKind::EqualSpacingH).count();
        assert!(n >= 3, "chain guides extend over the row: {n}");
    }

    #[test]
    fn vertical_spacing_mirrors_horizontal() {
        let others = [r(0.0, 0.0, 100.0, 100.0), r(0.0, 300.0, 100.0, 100.0)];
        let res = detect_smart_guides(r(10.0, 172.0, 50.0, 50.0), &others, 8.0);
        assert!(res.snapped_y);
        assert_eq!(res.point.y, 175.0);
        let v: Vec<_> = res.guides.iter().filter(|g| g.kind == SmartGuideKind::EqualSpacingV).collect();
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].start, v[0].end), (100.0, 175.0));
        assert_eq!(v[0].position, 35.0, "x centre of the dragged box");
    }

    #[test]
    fn spacing_needs_cross_axis_overlap() {
        // Boxes in a row far above the dragged box: no horizontal spacing.
        let others = [r(0.0, 0.0, 100.0, 100.0), r(300.0, 0.0, 100.0, 100.0)];
        let res = detect_smart_guides(r(120.0, 500.0, 50.0, 50.0), &others, 8.0);
        assert!(!res.snapped_x);
    }

    #[test]
    fn spacing_is_skipped_above_fifty_boxes() {
        let mut others: Vec<Rect> = (0..51).map(|i| r(i as f64 * 1000.0, 5000.0, 10.0, 10.0)).collect();
        others.push(r(0.0, 0.0, 100.0, 100.0));
        others.push(r(300.0, 0.0, 100.0, 100.0));
        let res = detect_smart_guides(r(172.0, 10.0, 50.0, 50.0), &others, 8.0);
        assert!(!res.guides.iter().any(|g| g.kind == SmartGuideKind::EqualSpacingH));
        let few = &others[51..];
        let res = detect_smart_guides(r(172.0, 10.0, 50.0, 50.0), few, 8.0);
        assert!(res.guides.iter().any(|g| g.kind == SmartGuideKind::EqualSpacingH));
    }

    #[test]
    fn gap_over_the_radius_is_ignored() {
        let others = [r(0.0, 0.0, 100.0, 100.0), r(100.0 + 801.0, 0.0, 100.0, 100.0)];
        let res = detect_smart_guides(r(500.0, 10.0, 50.0, 50.0), &others, 8.0);
        assert!(!res.guides.iter().any(|g| g.kind == SmartGuideKind::EqualSpacingH));
    }

    #[test]
    fn point_snaps_to_edges_and_centres() {
        let res = detect_smart_guides_for_point(Point::new(103.0, 205.0), &[r(100.0, 0.0, 80.0, 80.0)], 8.0);
        assert!(res.snapped_x && !res.snapped_y);
        assert_eq!(res.point, Point::new(100.0, 205.0));
        assert_eq!(res.guides[0].kind, SmartGuideKind::Vertical);
        assert_eq!((res.guides[0].start, res.guides[0].end), (0.0, 205.0));
        assert_eq!(res.guides[0].snap_points[0], 205.0);

        let res = detect_smart_guides_for_point(Point::new(500.0, 42.0), &[r(100.0, 0.0, 80.0, 80.0)], 8.0);
        assert!(res.snapped_y);
        assert_eq!(res.point, Point::new(500.0, 40.0), "snaps to the vertical centre");
        assert_eq!(res.guides[0].kind, SmartGuideKind::Horizontal);
    }

    #[test]
    fn ray_snaps_to_the_nearest_guide_crossing() {
        // 45 degree ray from the origin; box edge at x = 100 crosses at (100, 100).
        let res =
            snap_ray_to_smart_guides(Point::ZERO, 45.0, Point::new(97.0, 99.0), &[r(100.0, 500.0, 50.0, 50.0)], 8.0);
        assert!(res.snapped_x && res.snapped_y);
        assert!((res.point.x - 100.0).abs() < 1e-9 && (res.point.y - 100.0).abs() < 1e-9);
        assert_eq!(res.guides.len(), 1);
        assert_eq!(res.guides[0].kind, SmartGuideKind::Vertical);
        // too far: nothing
        let res =
            snap_ray_to_smart_guides(Point::ZERO, 45.0, Point::new(60.0, 60.0), &[r(100.0, 500.0, 50.0, 50.0)], 8.0);
        assert!(!res.snapped_x && res.point == Point::new(60.0, 60.0));
        // a guide behind the origin is not a candidate
        let res = snap_ray_to_smart_guides(
            Point::new(200.0, 200.0),
            45.0,
            Point::new(97.0, 99.0),
            &[r(100.0, 500.0, 50.0, 50.0)],
            8.0,
        );
        assert!(!res.snapped_x);
    }

    #[test]
    fn horizontal_ray_only_meets_horizontal_guides_never() {
        // sin = 0: only vertical guides can be crossed.
        let res = snap_ray_to_smart_guides(
            Point::new(0.0, 50.0),
            0.0,
            Point::new(97.0, 50.0),
            &[r(100.0, 0.0, 50.0, 100.0)],
            8.0,
        );
        assert_eq!(res.point, Point::new(100.0, 50.0));
    }

    #[test]
    fn nan_and_inf_do_nothing() {
        let o = [r(0.0, 0.0, 10.0, 10.0), Rect::new(f64::NAN, 0.0, 1.0, 1.0)];
        let res = detect_smart_guides(r(1.0, 1.0, 10.0, 10.0), &o, f64::NAN);
        assert!(!res.snapped_x && !res.snapped_y && res.guides.is_empty());
        let res = detect_smart_guides(Rect::new(f64::NAN, 0.0, 1.0, 1.0), &o, 8.0);
        assert!(res.guides.is_empty());
        let res = detect_smart_guides(r(1.0, 1.0, 10.0, 10.0), &o, 8.0);
        assert!(res.snapped_x, "the finite box still works");
        let res = detect_smart_guides_for_point(Point::new(f64::INFINITY, 0.0), &o, 8.0);
        assert!(res.guides.is_empty());
        let res = snap_ray_to_smart_guides(Point::ZERO, f64::NAN, Point::ZERO, &o, 8.0);
        assert!(res.guides.is_empty());
    }

    #[test]
    fn an_alignment_guide_carries_the_empty_stretch_between_the_two_boxes() {
        // Left edges line up at x = 0; the dragged box ends at y = 20, the other starts at y = 100.
        let res = detect_smart_guides(r(2.0, 0.0, 10.0, 20.0), &[r(0.0, 100.0, 30.0, 30.0)], 8.0);
        let g = res.guides.iter().find(|g| g.kind == SmartGuideKind::Vertical);
        assert_eq!(g.and_then(|g| g.gap), Some((20.0, 100.0)));
    }

    #[test]
    fn boxes_that_overlap_along_the_guide_have_no_gap() {
        let res = detect_smart_guides(r(2.0, 0.0, 10.0, 60.0), &[r(0.0, 40.0, 30.0, 30.0)], 8.0);
        let g = res.guides.iter().find(|g| g.kind == SmartGuideKind::Vertical);
        assert_eq!(g.map(|g| g.gap), Some(None));
    }

    #[test]
    fn a_dragged_point_has_a_gap_to_the_box_it_aligns_with() {
        let res = detect_smart_guides_for_point(Point::new(1.0, 10.0), &[r(0.0, 50.0, 20.0, 20.0)], 8.0);
        let g = res.guides.iter().find(|g| g.kind == SmartGuideKind::Vertical);
        assert_eq!(g.and_then(|g| g.gap), Some((10.0, 50.0)));
    }

    #[test]
    fn spacing_guides_have_no_gap_of_their_own() {
        let g = guide(SmartGuideKind::EqualSpacingH, 5.0, 0.0, 10.0);
        assert_eq!(g.gap, None);
    }
}
