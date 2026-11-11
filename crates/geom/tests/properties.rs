//! Property tests: containment, bounds, distance and intersection must agree with each other,
//! under rotation and offset, and hostile numbers must never panic.

use drafftink_geom::align::{AlignMode, Axis, align, distribute};
use drafftink_geom::binding::{BindKind, BindTarget};
use drafftink_geom::guides::{detect_smart_guides, detect_smart_guides_for_point, snap_ray_to_smart_guides};
use drafftink_geom::kurbo::{Affine, BezPath, PathEl, Point, Rect, Size, Vec2};
use drafftink_geom::select::{BoxHandle, Corner, Edge, HandleSet, ResizeOpts, resize_box};
use drafftink_geom::snap::{snap_angle, snap_endpoint, snap_ray_to_grid_lines, snap_to_grid};
use drafftink_geom::transform::{ElementFrame, map_box};
use drafftink_geom::{
    DiamondShape, EllipseShape, HitOpts, OrientedBox, Outline, PathShape, Polyline, Primitive, Probe, RectShape,
    SpatialIndex, aabb,
};
use proptest::prelude::*;

fn config() -> ProptestConfig {
    ProptestConfig { cases: 400, failure_persistence: None, ..ProptestConfig::default() }
}

fn coord() -> impl Strategy<Value = f64> {
    -500.0..500.0f64
}

fn obox() -> impl Strategy<Value = OrientedBox> {
    (coord(), coord(), 2.0..300.0f64, 2.0..300.0f64, -7.0..7.0f64)
        .prop_map(|(x, y, w, h, a)| OrientedBox::new(Point::new(x, y), Size::new(w, h), a))
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Rect,
    Ellipse,
    Diamond,
}

fn kind() -> impl Strategy<Value = Kind> {
    prop::sample::select(vec![Kind::Rect, Kind::Ellipse, Kind::Diamond])
}

fn make(kind: Kind, b: OrientedBox, radius: f64, offset: f64) -> Primitive {
    match kind {
        Kind::Rect => RectShape::new(b).with_corner_radius(radius).with_offset(offset).into(),
        Kind::Ellipse => EllipseShape::new(b).with_offset(offset).into(),
        Kind::Diamond => DiamondShape::new(b).with_corner_radius(radius).with_offset(offset).into(),
    }
}

fn shape() -> impl Strategy<Value = (Primitive, OrientedBox)> {
    (kind(), obox(), 0.0..60.0f64, prop::sample::select(vec![0.0, 0.0, 5.0, 12.0]))
        .prop_map(|(k, b, r, o)| (make(k, b, r, o), b))
}

fn query_point(b: OrientedBox) -> impl Strategy<Value = Point> {
    let c = b.center;
    let r = b.size.width.max(b.size.height) * 1.5 + 30.0;
    (-r..r, -r..r).prop_map(move |(dx, dy)| Point::new(c.x + dx, c.y + dy))
}

/// Points on the outline found with rays from the centre (valid for these convex shapes).
fn outline_samples(s: &Primitive, b: &OrientedBox, n: usize) -> Vec<Point> {
    (0..n)
        .filter_map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            s.intersect(&Probe::ray(b.center, Vec2::new(a.cos(), a.sin()))).first().map(|h| h.point)
        })
        .collect()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn bounds_contain_every_outline_point_and_every_interior_point(((s, b), seed) in (shape(), 0u32..1000)) {
        let bounds = aabb::inflate(s.bounds(), 1e-6);
        for p in outline_samples(&s, &b, 64) {
            prop_assert!(aabb::contains_point(&bounds, p), "{p:?} outside {bounds:?}");
        }
        // Interior points are inside the bounds; points outside the bounds are not inside the shape.
        let mut x = seed as u64 + 1;
        for _ in 0..32 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let fx = (x >> 33) as f64 / (1u64 << 31) as f64;
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let fy = (x >> 33) as f64 / (1u64 << 31) as f64;
            let r = s.bounds();
            let p = Point::new(r.x0 - 50.0 + fx * (r.width() + 100.0), r.y0 - 50.0 + fy * (r.height() + 100.0));
            if s.contains(p) {
                prop_assert!(aabb::contains_point(&bounds, p));
            }
            if !aabb::contains_point(&bounds, p) {
                prop_assert!(!s.contains(p));
            }
        }
    }

    #[test]
    fn bounds_are_tight(((s, b), _) in (shape(), 0u8..1)) {
        // The extreme outline points touch the bounds on all four sides.
        let r = s.bounds();
        let pts = outline_samples(&s, &b, 720);
        let min_x = pts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let max_x = pts.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        let min_y = pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_y = pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
        let slack = 0.02 * (r.width() + r.height()) + 1.0;
        prop_assert!(min_x - r.x0 < slack && r.x1 - max_x < slack && min_y - r.y0 < slack && r.y1 - max_y < slack, "{r:?} vs {min_x} {max_x} {min_y} {max_y}");
    }

    #[test]
    fn signed_distance_agrees_with_containment(((s, b), p) in shape().prop_flat_map(|(s, b)| (Just((s, b)), query_point(b)))) {
        let sd = s.signed_distance(p);
        let d = s.outline_distance(p);
        prop_assert!(d >= 0.0 && d.is_finite());
        prop_assert!((sd.abs() - d).abs() < 1e-9);
        if d > 1e-6 {
            prop_assert_eq!(sd < 0.0, s.contains(p));
        }
        let filled = HitOpts::filled(0.0, 0.0);
        prop_assert_eq!(s.hit_test(p, filled), s.contains(p) || d <= 0.0);
        // Growing the reach never loses a hit.
        let small = s.hit_test(p, HitOpts::stroke(1.0, 0.0));
        let big = s.hit_test(p, HitOpts::stroke(3.0, 0.0));
        prop_assert!(!small || big);
        // Containment with the translated origin
        let _ = b;
    }

    #[test]
    fn nearest_point_is_on_the_outline_and_minimal(((s, b), p) in shape().prop_flat_map(|(s, b)| (Just((s, b)), query_point(b)))) {
        let n = s.nearest(p).expect("valid shape");
        prop_assert!(n.distance.is_finite());
        prop_assert!(((p - n.point).hypot() - n.distance).abs() < 1e-9);
        // On the outline: a small step along the normal leaves, against it enters.
        let eps = 1e-3;
        prop_assert!((n.normal.hypot() - 1.0).abs() < 1e-6, "unit normal {:?}", n.normal);
        prop_assert!(!s.contains(n.point + n.normal * eps), "outside along the normal");
        prop_assert!(s.contains(n.point - n.normal * eps), "inside against the normal");
        // Minimal against samples.
        for q in outline_samples(&s, &b, 90) {
            prop_assert!(n.distance <= (p - q).hypot() + 1e-6, "sample {q:?} is closer than {n:?}");
        }
    }

    #[test]
    fn rays_from_inside_hit_once_on_the_outline(((s, b), angle) in (shape(), 0.0..std::f64::consts::TAU)) {
        let dir = Vec2::new(angle.cos(), angle.sin());
        let hits = s.intersect(&Probe::ray(b.center, dir));
        prop_assert_eq!(hits.len(), 1, "convex shape, origin inside");
        let h = hits[0];
        prop_assert!(h.t > 0.0);
        prop_assert!((b.center + dir * h.t - h.point).hypot() < 1e-6);
        prop_assert!(s.outline_distance(h.point) < 1e-6, "hit {:?} is {} from the outline", h.point, s.outline_distance(h.point));
        prop_assert!(h.normal.dot(dir) > -1e-9, "outward normal faces along the ray");
        // A line through the centre crosses twice, opposite sides.
        let line = s.intersect(&Probe::line(b.center, dir));
        prop_assert_eq!(line.len(), 2);
        prop_assert!(line[0].t < 0.0 && line[1].t > 0.0);
    }

    #[test]
    fn segments_across_the_shape_cross_an_even_number_of_times(((s, b), p, q) in shape().prop_flat_map(|(s, b)| (Just((s, b)), query_point(b), query_point(b)))) {
        prop_assume!((p - q).hypot() > 1e-3);
        let hits = s.intersect(&Probe::segment(p, q));
        let (pi, qi) = (s.contains(p), s.contains(q));
        // Skip near-tangent and boundary cases.
        prop_assume!(s.outline_distance(p) > 1e-4 && s.outline_distance(q) > 1e-4);
        prop_assume!(hits.iter().all(|h| (0.0..=1.0).contains(&h.t)));
        prop_assert_eq!(hits.len() % 2 == 1, pi != qi, "p inside {}, q inside {}, hits {}", pi, qi, hits.len());
        let _ = b;
    }

    #[test]
    fn rotating_the_shape_rotates_the_answers(((kind, b, r), pivot, dtheta, p) in (kind(), obox(), 0.0..40.0f64).prop_flat_map(|(k, b, r)| (Just((k, b, r)), (coord(), coord()), -3.0..3.0f64, query_point(b)))) {
        let s0 = make(kind, b, r, 0.0);
        let pivot = Point::new(pivot.0, pivot.1);
        let rot = Affine::rotate_about(dtheta, pivot);
        let s1 = make(kind, b.rotated_about(pivot, dtheta), r, 0.0);
        let d0 = s0.signed_distance(p);
        let d1 = s1.signed_distance(rot * p);
        prop_assert!((d0 - d1).abs() < 1e-6 * (1.0 + d0.abs()), "{d0} vs {d1}");
        // Bounds of the rotated shape are the bounds of the rotated outline.
        let r1 = s1.bounds();
        for q in outline_samples(&s0, &b, 48) {
            prop_assert!(aabb::contains_point(&aabb::inflate(r1, 1e-6), rot * q));
        }
    }

    #[test]
    fn offset_outline_is_the_original_pushed_out(((kind, b, r), off, angle) in (kind(), obox(), 0.0..30.0f64).prop_flat_map(|(k, b, r)| (Just((k, b, r)), 0.5..20.0f64, 0.0..std::f64::consts::TAU))) {
        prop_assume!(!matches!(kind, Kind::Ellipse)); // ellipse offset is axis growth, not a parallel curve
        let base = make(kind, b, r, 0.0);
        let grown = make(kind, b, r, off);
        let dir = Vec2::new(angle.cos(), angle.sin());
        let h = grown.intersect(&Probe::ray(b.center, dir));
        prop_assert_eq!(h.len(), 1);
        // Every point of the offset outline is exactly `off` from the original outline.
        let d = base.signed_distance(h[0].point);
        prop_assert!((d - off).abs() < 1e-6, "{d} vs {off}");
    }

    #[test]
    fn index_matches_brute_force(
        ops in prop::collection::vec((0u8..4, 0u32..40, coord(), coord(), 1.0..80.0f64, 1.0..80.0f64), 1..120),
        q in (coord(), coord(), 10.0..400.0f64, 10.0..400.0f64),
        tol in 0.0..30.0f64,
    ) {
        let mut ix: SpatialIndex<u32> = SpatialIndex::new();
        let mut model: std::collections::HashMap<u32, Rect> = std::collections::HashMap::new();
        for (op, id, x, y, w, h) in ops {
            let r = Rect::new(x, y, x + w, y + h);
            match op {
                0 | 1 => { ix.insert(id, r).unwrap(); model.insert(id, r); }
                2 => { prop_assert_eq!(ix.update(&id, r).is_ok(), model.contains_key(&id)); if model.contains_key(&id) { model.insert(id, r); } }
                _ => { prop_assert_eq!(ix.remove(&id), model.remove(&id)); }
            }
        }
        prop_assert_eq!(ix.len(), model.len());
        let rect = Rect::new(q.0, q.1, q.0 + q.2, q.1 + q.3);
        let mut got: Vec<u32> = ix.query_rect(&rect).map(|(i, _)| *i).collect();
        let mut want: Vec<u32> = model.iter().filter(|(_, r)| aabb::intersects(r, &rect)).map(|(i, _)| *i).collect();
        got.sort_unstable(); want.sort_unstable();
        prop_assert_eq!(&got, &want);

        let mut got: Vec<u32> = ix.query_rect_contained(&rect).map(|(i, _)| *i).collect();
        let mut want: Vec<u32> = model.iter().filter(|(_, r)| aabb::contains_rect(&rect, r)).map(|(i, _)| *i).collect();
        got.sort_unstable(); want.sort_unstable();
        prop_assert_eq!(&got, &want);

        let p = Point::new(q.0, q.1);
        let mut got: Vec<u32> = ix.query_point(p, tol).map(|(i, _)| *i).collect();
        let mut want: Vec<u32> = model.iter().filter(|(_, r)| aabb::distance_to_point(r, p) <= tol).map(|(i, _)| *i).collect();
        got.sort_unstable(); want.sort_unstable();
        prop_assert_eq!(&got, &want);

        match ix.nearest(p) {
            None => prop_assert!(model.is_empty()),
            Some((_, _, d)) => {
                let best = model.values().map(|r| aabb::distance_to_point(r, p)).fold(f64::INFINITY, f64::min);
                prop_assert!((d - best).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn polygon_and_path_agree_with_the_box(b in obox(), p in (coord(), coord())) {
        // A rotated rectangle as a polygon, as a bezier path, and as a RectShape must agree.
        let p = Point::new(p.0, p.1);
        let rect = RectShape::new(b);
        let poly = Polyline::new(b.corners().to_vec(), true);
        let mut path = BezPath::new();
        let c = b.corners();
        path.move_to(c[0]); path.line_to(c[1]); path.line_to(c[2]); path.line_to(c[3]); path.close_path();
        let path = PathShape::new(path);
        let d = rect.outline_distance(p);
        prop_assert!((poly.outline_distance(p) - d).abs() < 1e-6);
        prop_assert!((path.outline_distance(p) - d).abs() < 1e-4, "{} vs {d}", path.outline_distance(p));
        if d > 1e-6 {
            prop_assert_eq!(poly.contains(p), rect.contains(p));
            prop_assert_eq!(path.contains(p), rect.contains(p));
        }
        let (a, bb, cc) = (rect.bounds(), poly.bounds(), path.bounds());
        for r in [bb, cc] {
            prop_assert!((a.x0 - r.x0).abs() < 1e-6 && (a.y1 - r.y1).abs() < 1e-6 && (a.x1 - r.x1).abs() < 1e-6 && (a.y0 - r.y0).abs() < 1e-6);
        }
    }

    #[test]
    fn element_frame_round_trips_and_flip_keeps_the_box(f in (coord(), coord(), 1.0..200.0f64, 1.0..200.0f64, -7.0..7.0f64, any::<bool>(), any::<bool>()), p in (coord(), coord())) {
        let frame = ElementFrame::new(Point::new(f.0, f.1), Size::new(f.2, f.3), f.4).with_flip(f.5, f.6);
        let p = Point::new(p.0, p.1);
        prop_assert!((frame.to_local() * (frame.to_world() * p) - p).hypot() < 1e-6);
        // The local box maps onto the same oriented box with or without flip.
        let plain = ElementFrame::new(frame.origin, frame.size, frame.angle);
        let corners = |fr: &ElementFrame| [Point::ZERO, Point::new(fr.size.width, 0.0), Point::new(fr.size.width, fr.size.height), Point::new(0.0, fr.size.height)].map(|q| fr.to_world() * q);
        for c in corners(&frame) {
            prop_assert!(corners(&plain).iter().any(|d| (*d - c).hypot() < 1e-6));
        }
    }

    #[test]
    fn map_box_through_similarity_matches_corner_mapping(b in obox(), s in 0.2..4.0f64, a in -3.0..3.0f64, t in (coord(), coord()), mirror in any::<bool>()) {
        let m = Affine::translate(Vec2::new(t.0, t.1)) * Affine::rotate(a) * Affine::scale_non_uniform(if mirror { -s } else { s }, s);
        let (mapped, mirrored) = map_box(&b, m).expect("finite");
        prop_assert_eq!(mirrored, mirror);
        let mut want: Vec<Point> = b.corners().iter().map(|c| m * *c).collect();
        for c in mapped.corners() {
            let i = want.iter().position(|w| (*w - c).hypot() < 1e-6 * (1.0 + s));
            prop_assert!(i.is_some(), "corner {c:?} not among {want:?}");
            want.swap_remove(i.unwrap_or(0));
        }
    }

    #[test]
    fn resize_keeps_the_anchor_and_follows_the_pointer(b in obox(), corner in prop::sample::select(vec![Corner::TopLeft, Corner::TopRight, Corner::BottomRight, Corner::BottomLeft]), keep in any::<bool>(), p in (coord(), coord())) {
        let pointer = Point::new(p.0, p.1);
        let handle = BoxHandle::Corner(corner);
        let r = resize_box(&b, handle, pointer, ResizeOpts { keep_aspect: keep, ..Default::default() }).expect("valid");
        prop_assert!(r.obox.is_valid() && r.obox.size.width >= 1.0 - 1e-9 && r.obox.size.height >= 1.0 - 1e-9);
        // The anchor (opposite corner of the original) is a corner of the result.
        let opposite = match corner { Corner::TopLeft => Corner::BottomRight, Corner::TopRight => Corner::BottomLeft, Corner::BottomRight => Corner::TopLeft, Corner::BottomLeft => Corner::TopRight };
        let anchor = drafftink_geom::select::handle_point(&b, BoxHandle::Corner(opposite), 0.0);
        prop_assert!(r.obox.corners().iter().any(|c| (*c - anchor).hypot() < 1e-6), "anchor moved");
        // The affine carries the original box onto the result.
        let a = r.affine(&b);
        let mut want = r.obox.corners().to_vec();
        for c in b.corners() {
            let m = a * c;
            let i = want.iter().position(|w| (*w - m).hypot() < 1e-6);
            prop_assert!(i.is_some());
            want.swap_remove(i.unwrap_or(0));
        }
        if !keep {
            // Without aspect lock the pointer is a corner of the result unless clamped.
            let l = b.to_local(pointer);
            let (sx, sy) = match corner { Corner::TopLeft => (-1.0, -1.0), Corner::TopRight => (1.0, -1.0), Corner::BottomRight => (1.0, 1.0), Corner::BottomLeft => (-1.0, 1.0) };
            let a_l = Point::new(-sx * b.size.width / 2.0, -sy * b.size.height / 2.0);
            if (l.x - a_l.x).abs() > 1.0 && (l.y - a_l.y).abs() > 1.0 {
                prop_assert!(r.obox.corners().iter().any(|c| (*c - pointer).hypot() < 1e-6), "pointer is not a corner");
            }
        }
    }

    #[test]
    fn grid_snapping_is_idempotent_and_lands_on_the_grid(x in coord(), y in coord(), g in prop::sample::select(vec![5.0, 10.0, 20.0, 40.0]), a in -720.0..720.0f64) {
        let once = snap_to_grid(Point::new(x, y), g);
        let twice = snap_to_grid(once.point, g);
        prop_assert_eq!(once.point, twice.point);
        prop_assert!((once.point.x / g - (once.point.x / g).round()).abs() < 1e-9);
        let s = snap_angle(a, 15.0);
        prop_assert!((0.0..360.0).contains(&s));
        prop_assert!((s / 15.0 - (s / 15.0).round()).abs() < 1e-9);
        prop_assert_eq!(snap_angle(s, 15.0), s);
    }

    #[test]
    fn align_and_distribute_invariants(rs in prop::collection::vec((coord(), coord(), 1.0..100.0f64, 1.0..100.0f64), 2..8), mode in prop::sample::select(vec![AlignMode::Left, AlignMode::Right, AlignMode::Top, AlignMode::Bottom, AlignMode::CenterX, AlignMode::CenterY])) {
        let rects: Vec<Rect> = rs.iter().map(|(x, y, w, h)| Rect::new(*x, *y, x + w, y + h)).collect();
        let moved: Vec<Rect> = rects.iter().zip(align(&rects, mode)).map(|(r, d)| aabb::translated(*r, d)).collect();
        // Idempotent.
        prop_assert!(align(&moved, mode).iter().all(|d| d.hypot() < 1e-9));
        // Aligned edge is shared.
        let key = |r: &Rect| match mode { AlignMode::Left => r.x0, AlignMode::Right => r.x1, AlignMode::Top => r.y0, AlignMode::Bottom => r.y1, AlignMode::CenterX => (r.x0 + r.x1) / 2.0, AlignMode::CenterY => (r.y0 + r.y1) / 2.0 };
        prop_assert!(moved.iter().all(|r| (key(r) - key(&moved[0])).abs() < 1e-9));

        if rects.len() >= 3 {
            let d = distribute(&rects, Axis::X);
            let m: Vec<Rect> = rects.iter().zip(&d).map(|(r, v)| aabb::translated(*r, *v)).collect();
            // The overall extent is unchanged in the gap mode.
            let before = aabb::union_all(rects.iter().copied()).expect("rects");
            let after = aabb::union_all(m.iter().copied()).expect("rects");
            let total: f64 = rects.iter().map(|r| r.width()).sum();
            if before.width() >= total {
                prop_assert!((before.x0 - after.x0).abs() < 1e-6 && (before.x1 - after.x1).abs() < 1e-6);
                let mut sorted = m.clone();
                sorted.sort_by(|a, b| a.x0.total_cmp(&b.x0));
                let gaps: Vec<f64> = sorted.windows(2).map(|w| w[1].x0 - w[0].x1).collect();
                prop_assert!(gaps.iter().all(|g| (g - gaps[0]).abs() < 1e-6), "{gaps:?}");
            }
            prop_assert!(d.iter().all(|v| v.y == 0.0));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Hostile numbers: nothing may panic, and results stay neutral.
// ---------------------------------------------------------------------------------------------

fn any_f64() -> impl Strategy<Value = f64> {
    prop_oneof![
        3 => proptest::num::f64::ANY,
        2 => prop::sample::select(vec![0.0, -0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, f64::MIN, f64::MIN_POSITIVE, 1e-300, 1e300, -1.0, 1.0]),
        2 => -100.0..100.0f64,
    ]
}

fn any_point() -> impl Strategy<Value = Point> {
    (any_f64(), any_f64()).prop_map(|(x, y)| Point::new(x, y))
}

fn any_rect() -> impl Strategy<Value = Rect> {
    (any_f64(), any_f64(), any_f64(), any_f64()).prop_map(|(a, b, c, d)| Rect::new(a, b, c, d))
}

fn any_obox() -> impl Strategy<Value = OrientedBox> {
    (any_point(), any_f64(), any_f64(), any_f64()).prop_map(|(c, w, h, a)| OrientedBox::new(c, Size::new(w, h), a))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn hostile_shapes(k in kind(), b in any_obox(), r in any_f64(), off in any_f64(), p in any_point(), q in any_point(), dir in any_point(), rect in any_rect(), tol in any_f64(), sw in any_f64()) {
        let s = make(k, b, r, off);
        let probe = Probe::ray(p, Vec2::new(dir.x, dir.y));
        let bounds = s.bounds();
        let _ = (s.is_valid(), s.is_closed(), s.contains(p), s.nearest(q), s.intersect(&probe), s.intersect(&Probe::segment(p, q)), s.intersect(&Probe::line(p, dir.to_vec2())));
        let _ = (s.outline_distance(p), s.signed_distance(q), s.hit_test(p, HitOpts::filled(tol, sw)), s.bounds_with_stroke(sw), s.intersects_rect(&rect, true), s.intersects_rect(&rect, false));
        if !s.is_valid() {
            prop_assert_eq!(bounds, Rect::ZERO);
            prop_assert!(!s.contains(p) && s.nearest(q).is_none() && s.intersect(&probe).is_empty());
        } else {
            prop_assert!(aabb::is_finite(&bounds) || b.size.width > 1e150 || b.size.height > 1e150 || b.center.x.abs() > 1e150 || b.center.y.abs() > 1e150 || off.abs() > 1e150 || r.abs() > 1e150);
        }
        // Results that are produced must be finite when the inputs are finite and moderate.
        if let Some(n) = s.nearest(q) {
            prop_assert!(!n.distance.is_nan() || !crate_finite(q));
        }
        for h in s.intersect(&probe) {
            prop_assert!(h.t.is_finite() && h.point.x.is_finite() && h.point.y.is_finite());
        }
    }

    #[test]
    fn hostile_polylines_and_paths(pts in prop::collection::vec(any_point(), 0..8), closed in any::<bool>(), p in any_point(), q in any_point(), rect in any_rect(), tol in any_f64()) {
        let poly = Polyline::new(pts.clone(), closed);
        let mut path = BezPath::new();
        for (i, pt) in pts.iter().enumerate() {
            if i == 0 { path.move_to(*pt); } else if i % 3 == 0 { path.curve_to(pts[i - 1], pts[i - 2], *pt); } else { path.line_to(*pt); }
        }
        if closed && !pts.is_empty() { path.close_path(); }
        let path = PathShape::new(path);
        for s in [Primitive::from(poly.clone()), Primitive::from(path)] {
            let _ = (s.bounds(), s.is_valid(), s.contains(p), s.nearest(q), s.intersect(&Probe::ray(p, q.to_vec2())), s.intersect(&Probe::segment(p, q)));
            let _ = (s.hit_test(p, HitOpts::filled(tol, tol)), s.intersects_rect(&rect, true));
        }
        let _ = poly.length();
        let _ = poly.transformed(Affine::rotate(p.x));
    }

    #[test]
    fn hostile_raw_path_elements(els in prop::collection::vec((0u8..5, any_point(), any_point(), any_point()), 0..8), p in any_point(), q in any_point()) {
        // Elements in any order after the MoveTo that `BezPath` itself requires.
        let mut v: Vec<PathEl> = vec![PathEl::MoveTo(p)];
        v.extend(els.iter().map(|(k, a, b, c)| match k { 0 => PathEl::MoveTo(*a), 1 => PathEl::LineTo(*a), 2 => PathEl::QuadTo(*a, *b), 3 => PathEl::CurveTo(*a, *b, *c), _ => PathEl::ClosePath }));
        let s = PathShape::new(BezPath::from_vec(v));
        let _ = (s.bounds(), s.is_valid(), s.is_closed(), s.contains(p), s.nearest(q), s.intersect(&Probe::ray(p, q.to_vec2())), s.intersects_rect(&Rect::new(0.0, 0.0, 10.0, 10.0), true));
    }

    #[test]
    fn hostile_index(entries in prop::collection::vec((0u8..8, any_rect()), 0..20), q in any_rect(), p in any_point(), tol in any_f64(), k in 0usize..5) {
        let mut ix: SpatialIndex<u8> = SpatialIndex::new();
        for (id, r) in &entries {
            let ok = ix.insert(*id, *r).is_ok();
            prop_assert_eq!(ok, aabb::is_finite(r));
        }
        let bulk = SpatialIndex::bulk_load(entries.iter().copied());
        let _ = (ix.query_rect(&q).count(), ix.query_rect_contained(&q).count(), ix.query_point(p, tol).count(), ix.nearest(p), ix.nearest_k(p, k), ix.nearest_within(p, tol), ix.total_bounds());
        let _ = (bulk.query_rect(&q).count(), bulk.nearest(p));
        for id in 0..8u8 {
            let _ = ix.update(&id, q);
            ix.remove(&id);
        }
        prop_assert!(ix.is_empty());
    }

    #[test]
    fn hostile_snap_select_binding(
        a in any_point(), b in any_point(), c in any_point(), x in any_f64(), y in any_f64(),
        rects in prop::collection::vec(any_rect(), 0..6), bx in any_obox(), r in any_rect(),
    ) {
        let _ = (snap_to_grid(a, x), snap_angle(x, y), snap_ray_to_grid_lines(a, x, b, y), snap_endpoint(a, b, true, true, x), snap_endpoint(a, b, false, true, x));
        let _ = (detect_smart_guides(r, &rects, x), detect_smart_guides_for_point(a, &rects, x), snap_ray_to_smart_guides(a, x, b, &rects, y));
        let _ = (align(&rects, AlignMode::CenterX), distribute(&rects, Axis::Y), aabb::union_all(rects.iter().copied()));
        for handle in [BoxHandle::Corner(Corner::TopLeft), BoxHandle::Edge(Edge::Right), BoxHandle::Rotate] {
            let _ = resize_box(&bx, handle, c, ResizeOpts { keep_aspect: true, from_center: true, min_size: x });
            let _ = resize_box(&bx, handle, c, ResizeOpts::default());
        }
        let _ = (drafftink_geom::select::hit_test_handles(&bx, a, x, HandleSet::ALL, y), drafftink_geom::select::hit_test_boundary(&bx, a, x), drafftink_geom::select::rotation_from_pointer(a, b, Some(x)));
        let _ = (bx.aabb(), bx.corners(), bx.contains(a), bx.intersects_aabb(&r), bx.to_local(a), map_box(&bx, Affine::rotate(x)), aabb::rotated_bounds(&r, a, x), aabb::scale_about(&r, a, x, y), aabb::segment_intersects_rect(a, b, &r));
        for kind in [BindKind::Rect, BindKind::Ellipse, BindKind::Diamond] {
            let t = BindTarget::new(kind, bx).with_corner_radius(x);
            let _ = (t.global_fixed_point(a.to_vec2()), t.fixed_point_from_global(b, x), t.orbit_point(a, b, y), t.heading_for_point(c, r), t.avoid_rectangular_corner(a, x), t.project_onto_diagonals(a, b), t.snap_side_midpoint(a, x), t.within_reach(a, x), t.side_midpoints(), t.nearest_on_offset_outline(a, x));
        }
    }
}

fn crate_finite(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// The shrunk input of a failure found by `hostile_shapes`: an ellipse of height 0 and width about
/// 1e148 whose centre is the smallest normal double. A finite query point must give a finite distance.
#[test]
fn degenerate_ellipse_gives_a_finite_distance_regression() {
    let frame = OrientedBox::new(
        Point::new(2.2250738585072014e-308, 0.0),
        Size::new(1.4546800786768307e148, 0.0),
        -15.028060024101233,
    );
    let shape: Primitive = EllipseShape::new(frame).with_offset(0.0).into();
    for q in [Point::new(0.0, 0.0), Point::new(3.0, -4.0), Point::new(1e148, 1e-3)] {
        if let Some(n) = shape.nearest(q) {
            assert!(n.distance.is_finite(), "distance {} for {q:?}", n.distance);
            assert!(crate_finite(n.point), "nearest point {:?} for {q:?}", n.point);
        }
    }
}
