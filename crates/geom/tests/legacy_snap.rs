//! Differential test against the old app's snapping code.
//!
//! `legacy/old_snap.rs` is `crates/drafftink-core/src/snap.rs` from drafft-ink at 8ce40ab, copied
//! unchanged. For valid input the new functions must give the same numbers. The known differences
//! (a bad grid size, the `snapped` flag across 360 degrees) are not exercised here and are covered
//! by unit tests in `src/snap.rs`.

#![allow(clippy::all, dead_code, unused)]

#[path = "legacy/old_snap.rs"]
mod old;

use drafftink_geom::guides as new_guides;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_geom::snap as new_snap;
use proptest::prelude::*;

fn config() -> ProptestConfig {
    ProptestConfig { cases: 1500, failure_persistence: None, ..ProptestConfig::default() }
}

/// Coordinates on a coarse grid so that exact ties, which decide the algorithms, are common.
fn coord() -> impl Strategy<Value = f64> {
    (-40i32..40).prop_map(|v| f64::from(v) * 5.0)
}

fn rect() -> impl Strategy<Value = Rect> {
    (coord(), coord(), 0i32..30, 0i32..30)
        .prop_map(|(x, y, w, h)| Rect::new(x, y, x + f64::from(w) * 5.0, y + f64::from(h) * 5.0))
}

fn point() -> impl Strategy<Value = Point> {
    (coord(), coord()).prop_map(|(x, y)| Point::new(x + 0.5, y - 0.5))
}

fn same_guides(a: &[old::SmartGuide], b: &[new_guides::SmartGuide]) -> Result<(), TestCaseError> {
    prop_assert_eq!(a.len(), b.len(), "guide count\nold: {:?}\nnew: {:?}", a, b);
    for (x, y) in a.iter().zip(b) {
        let kind_ok = matches!(
            (x.kind, y.kind),
            (old::SmartGuideKind::Vertical, new_guides::SmartGuideKind::Vertical)
                | (old::SmartGuideKind::Horizontal, new_guides::SmartGuideKind::Horizontal)
                | (old::SmartGuideKind::EqualSpacingH, new_guides::SmartGuideKind::EqualSpacingH)
                | (old::SmartGuideKind::EqualSpacingV, new_guides::SmartGuideKind::EqualSpacingV)
        );
        prop_assert!(kind_ok, "kind {:?} vs {:?}", x.kind, y.kind);
        prop_assert_eq!((x.position, x.start, x.end), (y.position, y.start, y.end));
        prop_assert_eq!(&x.snap_points, &y.snap_points);
    }
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn box_guides_match(dragged in rect(), others in prop::collection::vec(rect(), 0..9), threshold in prop::sample::select(vec![1.0, 5.0, 8.0, 20.0])) {
        let o = old::detect_smart_guides(dragged, &others, threshold);
        let n = new_guides::detect_smart_guides(dragged, &others, threshold);
        prop_assert_eq!(o.point, n.point);
        prop_assert_eq!((o.snapped_x, o.snapped_y), (n.snapped_x, n.snapped_y));
        same_guides(&o.guides, &n.guides)?;
    }

    #[test]
    fn point_guides_match(p in point(), others in prop::collection::vec(rect(), 0..9), threshold in prop::sample::select(vec![1.0, 8.0, 20.0])) {
        let o = old::detect_smart_guides_for_point(p, &others, threshold);
        let n = new_guides::detect_smart_guides_for_point(p, &others, threshold);
        prop_assert_eq!(o.point, n.point);
        prop_assert_eq!((o.snapped_x, o.snapped_y), (n.snapped_x, n.snapped_y));
        same_guides(&o.guides, &n.guides)?;
    }

    #[test]
    fn ray_guides_match(origin in point(), angle in prop::sample::select(vec![0.0, 15.0, 30.0, 45.0, 60.0, 90.0, 135.0, 180.0, 225.0, 300.0]), target in point(), others in prop::collection::vec(rect(), 0..9)) {
        let o = old::snap_ray_to_smart_guides(origin, angle, target, &others, 8.0);
        let n = new_guides::snap_ray_to_smart_guides(origin, angle, target, &others, 8.0);
        prop_assert_eq!(o.point, n.point);
        prop_assert_eq!((o.snapped_x, o.snapped_y), (n.snapped_x, n.snapped_y));
        same_guides(&o.guides, &n.guides)?;
    }

    #[test]
    fn grid_and_angle_match(p in point(), start in point(), angle in 0.0f64..360.0, grid in prop::sample::select(vec![5.0, 10.0, 20.0, 25.0])) {
        let (o, n) = (old::snap_to_grid(p, grid), new_snap::snap_to_grid(p, grid));
        prop_assert_eq!((o.point, o.snapped_x, o.snapped_y), (n.point, n.snapped_x, n.snapped_y));
        prop_assert_eq!(old::snap_angle(angle, 15.0), new_snap::snap_angle(angle, 15.0));

        let (o, n) = (old::snap_ray_to_grid_lines(start, angle, p, grid), new_snap::snap_ray_to_grid_lines(start, angle, p, grid));
        prop_assert_eq!((o.point, o.is_snapped()), (n.point, n.is_snapped()));

        for (a, g) in [(true, true), (true, false), (false, true), (false, false)] {
            let o = old::snap_line_endpoint_isometric(start, p, a, g, true, grid);
            let n = new_snap::snap_endpoint(start, p, a, g, grid);
            // The old code used sqrt(dx^2 + dy^2); the new code uses hypot. They differ by an ulp.
            prop_assert!((o.point - n.point).hypot() < 1e-9 && (o.distance - n.distance).abs() < 1e-9, "{:?} vs {:?}", o, n);
            prop_assert_eq!((o.angle_degrees, o.original_angle_degrees), (n.angle_degrees, n.original_angle_degrees));
            // `snapped` differs only across the 360 degree wrap; see src/snap.rs.
            if (o.original_angle_degrees - o.angle_degrees).abs() < 300.0 {
                prop_assert_eq!(o.snapped, n.snapped);
            }
        }
        let (o, n) = (old::snap_line_endpoint(start, p, true), new_snap::snap_line_endpoint(start, p, true));
        prop_assert!((o.point - n.point).hypot() < 1e-9 && (o.distance - n.distance).abs() < 1e-9);
        prop_assert_eq!(o.angle_degrees, n.angle_degrees);
    }
}
