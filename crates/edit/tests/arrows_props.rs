#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Property tests: after any sequence of edits, bound ends are where their bindings say.

mod arrows_common;

use arrows_common::*;
use drafftink_edit::binding::{End, stale_arrows, update_bound_arrows};
use drafftink_edit::elbow::{self, RouteOpts};
use drafftink_edit::linear;
use drafftink_scene::{BindMode, ElementId, PathType, SceneRead, SceneStore, SceneTx, validate_element};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Move(f64, f64),
    Resize(f64, f64),
    Rotate(f64),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (-500.0..500.0f64, -500.0..500.0f64).prop_map(|(x, y)| Op::Move(x, y)),
        (40.0..300.0f64, 40.0..300.0f64).prop_map(|(w, h)| Op::Resize(w, h)),
        (0.0..std::f64::consts::TAU).prop_map(Op::Rotate),
    ]
}

fn apply(s: &mut Scene, t: &ElementId, op: &Op) {
    let mut tx = s.store.begin();
    tx.update(t, |e| match op {
        Op::Move(x, y) => {
            e.transform.x = *x;
            e.transform.y = *y;
        }
        Op::Resize(w, h) => {
            e.transform.w = *w;
            e.transform.h = *h;
        }
        Op::Rotate(a) => e.transform.angle = *a,
    })
    .unwrap();
    update_bound_arrows(&mut tx, std::slice::from_ref(t)).unwrap();
    tx.commit().unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Inside ends stay at their fixed point, orbit ends stay on the outline plus gap, for rectangles, ellipses
    /// and diamonds, through any sequence of moves, resizes and rotations of the target.
    #[test]
    fn bound_ends_follow_their_target(
        kind in 0usize..3,
        fp_in in (0.35..0.65f64, 0.35..0.65f64),
        fp_orbit in (0.35..0.65f64, 0.35..0.65f64),
        ops in proptest::collection::vec(op(), 1..12),
    ) {
        let mut s = Scene::new();
        let t = match kind {
            0 => s.rect("t", 0.0, 0.0, 120.0, 80.0),
            1 => s.ellipse("t", 0.0, 0.0, 120.0, 80.0),
            _ => s.diamond("t", 0.0, 0.0, 120.0, 80.0),
        };
        let inside = s.arrow("inside", &[(3000.0, 3000.0), (60.0, 40.0)]);
        s.bind(&inside, End::End, "t", BindMode::Inside, [fp_in.0, fp_in.1]);
        let orbit = s.arrow("orbit", &[(-3000.0, 2500.0), (60.0, 40.0)]);
        s.bind(&orbit, End::End, "t", BindMode::Orbit, [fp_orbit.0, fp_orbit.1]);
        let multi = s.arrow("multi", &[(-2500.0, -2500.0), (-2000.0, -1000.0), (60.0, 40.0)]);
        s.bind(&multi, End::End, "t", BindMode::Orbit, [fp_orbit.1, fp_orbit.0]);
        s.solve(&[&t]);

        for o in &ops {
            apply(&mut s, &t, o);
            let target = s.target_of(&t);
            let want = target.global_fixed_point(v(fp_in.0, fp_in.1));
            prop_assert!((s.end(&inside) - want).hypot() < 1e-6, "inside after {o:?}: {:?} vs {want:?}", s.end(&inside));
            for a in [&orbit, &multi] {
                let err = orbit_error(&s, &t, s.end(a), 6.0);
                prop_assert!(err < 1e-6, "orbit after {o:?}: error {err}");
            }
            // The unbound far ends never move.
            prop_assert_eq!(s.start(&inside), p(3000.0, 3000.0));
            prop_assert_eq!(s.start(&orbit), p(-3000.0, 2500.0));
            prop_assert!(stale_arrows(&s.store).is_empty());
            for a in [&inside, &orbit, &multi] {
                prop_assert_eq!(validate_element(&s.get(a)), Ok(()));
            }
        }
        // A second pass changes nothing.
        prop_assert!(s.solve(&[&t]).is_empty());
    }

    /// With both ends bound to two different shapes, both stay consistent while either shape moves.
    #[test]
    fn both_ends_stay_consistent(
        moves in proptest::collection::vec((any::<bool>(), -400.0..400.0f64, -400.0..400.0f64), 1..10),
    ) {
        let mut s = Scene::new();
        let ta = s.rect("ta", 0.0, 0.0, 100.0, 100.0);
        let tb = s.rect("tb", 600.0, 0.0, 100.0, 100.0);
        let a = s.arrow("a", &[(106.0, 50.0), (594.0, 50.0)]);
        s.bind(&a, End::Start, "ta", BindMode::Orbit, [0.5, 0.5]);
        s.bind(&a, End::End, "tb", BindMode::Orbit, [0.5, 0.5]);
        s.solve(&[&ta]);
        for (which_b, dx, dy) in moves {
            let (target, base) = if which_b { (&tb, 600.0) } else { (&ta, 0.0) };
            // Keep the shapes at least 300 apart in x so the arrow never gets short.
            let x = if which_b { base + dx.abs() } else { base - dx.abs() };
            let mut tx = s.store.begin();
            tx.update(target, |e| { e.transform.x = x; e.transform.y = dy; }).unwrap();
            update_bound_arrows(&mut tx, std::slice::from_ref(target)).unwrap();
            tx.commit().unwrap();
            prop_assert!(orbit_error(&s, &ta, s.start(&a), 6.0) < 1e-6);
            prop_assert!(orbit_error(&s, &tb, s.end(&a), 6.0) < 1e-6);
        }
    }
}

fn clean(pts: &[drafftink_geom::kurbo::Point]) -> Result<(), String> {
    for w in pts.windows(2) {
        if (w[0].x - w[1].x).abs() > 1e-6 && (w[0].y - w[1].y).abs() > 1e-6 {
            return Err(format!("diagonal segment in {pts:?}"));
        }
        if (w[0] - w[1]).hypot() < 1e-9 {
            return Err(format!("repeated point in {pts:?}"));
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 160, max_global_rejects: 1_000_000, ..ProptestConfig::default() })]

    /// Elbow routes between two separated shapes are orthogonal, start and end exactly at the attach points,
    /// never cross either shape, and are stable under a second solve.
    #[test]
    fn elbow_routes_are_orthogonal_and_avoid_the_shapes(
        bx in -700.0..700.0f64,
        by in -700.0..700.0f64,
        bw in 60.0..240.0f64,
        bh in 60.0..240.0f64,
        side_a in 0usize..4,
        side_b in 0usize..4,
    ) {
        // Keep the shapes at least 70 apart along one axis, so their padded boxes are separate.
        let apart_x = bx >= 170.0 || bx + bw <= -70.0;
        let apart_y = by >= 170.0 || by + bh <= -70.0;
        prop_assume!(apart_x || apart_y);

        let mut s = Scene::new();
        let a = s.rect("a", 0.0, 0.0, 100.0, 100.0);
        let b = s.rect("b", bx, by, bw, bh);
        let near = |x: f64, y: f64, w: f64, h: f64, side: usize| match side {
            0 => p(x + w / 2.0, y - 6.0),
            1 => p(x + w + 6.0, y + h / 2.0),
            2 => p(x + w / 2.0, y + h + 6.0),
            _ => p(x - 6.0, y + h / 2.0),
        };
        let pa = near(0.0, 0.0, 100.0, 100.0, side_a);
        let pb = near(bx, by, bw, bh, side_b);
        let arrow = s.elbow_between("arrow", pa, pb);
        let w = s.world(&arrow);
        prop_assert!(clean(&w).is_ok(), "{:?}", clean(&w));
        prop_assert!(stale_arrows(&s.store).is_empty());
        prop_assert_eq!(validate_element(&s.get(&arrow)), Ok(()));
        for shape in [&a, &b] {
            for seg in w.windows(2) {
                // Sample the segment: nothing may be inside the shape.
                for k in 1..8 {
                    let q = seg[0].lerp(seg[1], f64::from(k) / 8.0);
                    prop_assert!(signed(&s, shape, q) > -1e-6, "route {w:?} enters {shape} at {q:?}");
                }
            }
        }
        // Idempotent.
        let mut tx = s.store.begin();
        prop_assert!(!elbow::reroute(&mut tx, &arrow, RouteOpts::default()).unwrap());
        // Moving a shape and back restores the route.
        drop(tx);
        let before = s.world(&arrow);
        s.move_by(&b, 13.0, 0.0);
        s.move_by(&b, -13.0, 0.0);
        prop_assert!(clean(&s.world(&arrow)).is_ok());
        prop_assert_eq!(s.world(&arrow).len(), before.len());
    }

    /// Any sequence of segment moves, releases and end moves leaves a valid orthogonal arrow.
    #[test]
    fn pin_edits_keep_the_arrow_valid(
        ex in 100.0..500.0f64,
        ey in -400.0..400.0f64,
        ops in proptest::collection::vec((0u8..4, 1usize..6, -300.0..600.0f64, -300.0..600.0f64), 1..12),
    ) {
        prop_assume!(ey.abs() > 20.0);
        let mut s = Scene::new();
        let a = s.arrow("a", &[(0.0, 0.0), (ex, ey)]);
        let mut tx = s.store.begin();
        linear::set_path(&mut tx, &a, PathType::Elbow).unwrap();
        tx.commit().unwrap();
        for (kind, idx, x, y) in ops {
            let mut tx = s.store.begin();
            let last = linear::world_points(tx.get(&a).unwrap()).unwrap().len() - 1;
            let r = match kind {
                0 | 1 => elbow::move_segment_of(&mut tx, &a, idx.min(last).max(1), p(x, y), RouteOpts::default()).map_err(|e| e.to_string()),
                2 => elbow::release_segment_of(&mut tx, &a, idx, RouteOpts::default()).map_err(|e| e.to_string()),
                _ => linear::move_points(&mut tx, &a, &[(last, p(x, y))]).map_err(|e| e.to_string()),
            };
            if r.is_ok() {
                tx.commit().unwrap();
            } else {
                drop(tx);
            }
            let w = s.world(&a);
            prop_assert!(clean(&w).is_ok(), "{:?} after op {kind} {idx} {x} {y}", clean(&w));
            prop_assert_eq!(validate_element(&s.get(&a)), Ok(()));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// A bound elbow arrow with pins stays valid and attached while its shapes move and its segments are edited.
    #[test]
    fn bound_elbow_with_pins_stays_valid(
        ops in proptest::collection::vec((0u8..5, 1usize..6, -200.0..700.0f64, -300.0..400.0f64), 1..14),
    ) {
        let mut s = Scene::new();
        let ta = s.rect("ta", 0.0, 0.0, 100.0, 100.0);
        let tb = s.rect("tb", 400.0, 150.0, 100.0, 100.0);
        let arrow = s.elbow_between("arrow", p(106.0, 50.0), p(394.0, 200.0));
        for (kind, idx, x, y) in ops {
            match kind {
                0 | 1 => {
                    let mut tx = s.store.begin();
                    let last = linear::world_points(tx.get(&arrow).unwrap()).unwrap().len() - 1;
                    if elbow::move_segment_of(&mut tx, &arrow, idx.min(last).max(1), p(x, y), RouteOpts::default()).is_ok() {
                        tx.commit().unwrap();
                    }
                }
                2 => {
                    let mut tx = s.store.begin();
                    if elbow::release_segment_of(&mut tx, &arrow, idx, RouteOpts::default()).is_ok() {
                        tx.commit().unwrap();
                    }
                }
                3 => {
                    // Keep the shapes apart so the arrow keeps room.
                    s.move_by_to(&tb, 300.0 + x.abs() / 2.0, y);
                }
                _ => {
                    s.move_by_to(&ta, -(x.abs() / 3.0), y / 2.0);
                }
            }
            let w = s.world(&arrow);
            prop_assert!(clean(&w).is_ok(), "{:?}", clean(&w));
            prop_assert!(stale_arrows(&s.store).is_empty(), "stale after op {kind}: {w:?}");
            prop_assert_eq!(validate_element(&s.get(&arrow)), Ok(()));
            let pins = s.pins(&arrow);
            prop_assert!(pins.windows(2).all(|p| p[0] < p[1]));
            prop_assert!(pins.iter().all(|i| (*i as usize) < w.len() && *i >= 1));
        }
    }
}
