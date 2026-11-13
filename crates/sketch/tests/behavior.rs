//! Determinism, cache keys, arrows, fills, and hostile inputs.

// Test helpers (not `#[test]` fns) may panic on malformed fixtures.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use drafftink_sketch::{
    Arrowhead, Curve, FillKind, FillStyle, LineDash, MAX_FILL_DOTS, MAX_FILL_SEGMENTS, MAX_HACHURE_LINES, MAX_POINTS,
    PressureProfile, Roundness, SketchCache, SketchGeometry, SketchInput, SketchParams, SketchShape, sketch,
    sketch_key,
};
use kurbo::{BezPath, PathEl, Point, Rect};

fn params(seed: u32) -> SketchParams {
    SketchParams { seed, ..SketchParams::default() }
}

fn rect(w: f64, h: f64, p: SketchParams) -> SketchInput {
    SketchInput::new(SketchShape::Rect { width: w, height: h, roundness: Roundness::Sharp }, p)
}

fn wiggle(n: usize) -> Vec<Point> {
    (0..n).map(|i| Point::new(i as f64 * 3.0, ((i as f64) * 0.4).sin() * 20.0)).collect()
}

fn every_shape(p: SketchParams) -> Vec<SketchInput> {
    let pts = wiggle(12);
    vec![
        rect(120.0, 80.0, p.clone()),
        SketchInput::new(
            SketchShape::Rect { width: 120.0, height: 80.0, roundness: Roundness::Adaptive(32.0) },
            p.clone(),
        ),
        SketchInput::new(SketchShape::Ellipse { width: 100.0, height: 60.0 }, p.clone()),
        SketchInput::new(
            SketchShape::Diamond { width: 100.0, height: 60.0, roundness: Roundness::Radius(8.0) },
            p.clone(),
        ),
        SketchInput::new(SketchShape::Polyline { points: pts.clone(), curve: Curve::Smooth, closed: false }, p.clone()),
        SketchInput::new(
            SketchShape::Polyline { points: pts.clone(), curve: Curve::Straight, closed: true },
            p.clone(),
        ),
        SketchInput::new(
            SketchShape::Arrow {
                points: pts.clone(),
                curve: Curve::Straight,
                start: Arrowhead::CIRCLE,
                end: Arrowhead::TRIANGLE,
                head_size: 15.0,
            },
            p.clone(),
        ),
        SketchInput::new(
            SketchShape::Freehand {
                points: pts.clone(),
                pressures: (0..12).map(|i| f64::from(i) / 11.0).collect(),
                closed: false,
                profile: PressureProfile::default(),
            },
            p.clone(),
        ),
        SketchInput::new(
            SketchShape::Freehand {
                points: pts,
                pressures: vec![],
                closed: false,
                profile: PressureProfile::default(),
            },
            p,
        ),
    ]
}

fn all_finite(g: &SketchGeometry) -> bool {
    let ok = |p: &BezPath| {
        p.elements().iter().all(|e| match e {
            PathEl::MoveTo(a) | PathEl::LineTo(a) => a.is_finite(),
            PathEl::QuadTo(a, b) => a.is_finite() && b.is_finite(),
            PathEl::CurveTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
            PathEl::ClosePath => true,
        })
    };
    g.strokes.iter().all(|(p, s)| ok(p) && s.width.is_finite())
        && g.fills.iter().all(|(p, k)| {
            ok(p)
                && match k {
                    FillKind::Hatch { clip, line_width } => ok(clip) && line_width.is_finite(),
                    _ => true,
                }
        })
}

// ---------------------------------------------------------------- determinism

#[test]
fn same_input_same_output_for_every_shape_and_fill() {
    let fills = [
        FillStyle::None,
        FillStyle::Solid,
        FillStyle::Hachure,
        FillStyle::CrossHatch,
        FillStyle::ZigZag,
        FillStyle::Dots,
        FillStyle::Dashed,
        FillStyle::ZigZagLine,
    ];
    for fill in fills {
        for input in every_shape(SketchParams { fill, seed: 9, ..SketchParams::default() }) {
            let a = sketch(&input);
            let b = sketch(&input.clone());
            assert_eq!(a, b, "{fill:?} {:?}", input.shape);
            assert_eq!(sketch_key(&input), sketch_key(&input.clone()));
        }
    }
}

#[test]
fn a_different_seed_changes_the_drawing() {
    for (a, b) in every_shape(params(1)).into_iter().zip(every_shape(params(2))) {
        // Freehand with pressure ignores roughness and the seed.
        if matches!(&a.shape, SketchShape::Freehand { pressures, .. } if !pressures.is_empty()) {
            assert_eq!(sketch(&a), sketch(&b));
            continue;
        }
        assert_ne!(sketch(&a), sketch(&b), "{:?}", a.shape);
        assert_ne!(sketch_key(&a), sketch_key(&b));
    }
}

#[test]
fn roughness_zero_is_the_clean_geometry_in_one_stroke() {
    let g = sketch(&rect(100.0, 50.0, SketchParams { roughness: 0.0, ..params(3) }));
    assert_eq!(g.strokes.len(), 1);
    assert_eq!(
        g.strokes[0].0.elements(),
        &[
            PathEl::MoveTo(Point::new(0.0, 0.0)),
            PathEl::LineTo(Point::new(100.0, 0.0)),
            PathEl::LineTo(Point::new(100.0, 50.0)),
            PathEl::LineTo(Point::new(0.0, 50.0)),
            PathEl::ClosePath,
        ]
    );
    // And the seed is irrelevant when nothing is random.
    let g2 = sketch(&rect(100.0, 50.0, SketchParams { roughness: 0.0, ..params(77) }));
    assert_eq!(g, g2);
}

#[test]
fn sketchy_output_stays_near_the_clean_shape() {
    for r in [1.0, 2.0, 3.5] {
        let g = sketch(&rect(200.0, 100.0, SketchParams { roughness: r, ..params(5) }));
        assert_eq!(g.strokes.len(), 2);
        let b = g.bounds().expect("bounds");
        // Ends move by up to 2r; a bow control point by up to r * r * len / 200 (len <= 200).
        let slack = 2.0 * r + r * r + 0.5;
        assert!(Rect::new(-slack, -slack, 200.0 + slack, 100.0 + slack).contains_rect(b), "{r} {b:?}");
    }
}

// ---------------------------------------------------------------- cache keys

#[test]
fn equal_input_equal_key_and_every_relevant_change_changes_it() {
    let base = SketchParams { fill: FillStyle::Hachure, ..params(4) };
    let k = |i: &SketchInput| sketch_key(i);
    let r = rect(100.0, 50.0, base.clone());
    let key = k(&r);
    assert_eq!(key, k(&r.clone()));

    let changed = [
        rect(101.0, 50.0, base.clone()),
        rect(100.0, 51.0, base.clone()),
        rect(100.0, 50.0, SketchParams { seed: 5, ..base.clone() }),
        rect(100.0, 50.0, SketchParams { roughness: 2.0, ..base.clone() }),
        rect(100.0, 50.0, SketchParams { bowing: 2.0, ..base.clone() }),
        rect(100.0, 50.0, SketchParams { stroke_width: 3.0, ..base.clone() }),
        rect(100.0, 50.0, SketchParams { line: LineDash::Dashed, ..base.clone() }),
        rect(100.0, 50.0, SketchParams { fill: FillStyle::CrossHatch, ..base.clone() }),
        rect(100.0, 50.0, SketchParams { fill_gap: Some(9.0), ..base.clone() }),
        rect(100.0, 50.0, SketchParams { fill_angle: Some(10.0), ..base.clone() }),
        SketchInput::new(
            SketchShape::Rect { width: 100.0, height: 50.0, roundness: Roundness::Radius(4.0) },
            base.clone(),
        ),
        SketchInput::new(SketchShape::Ellipse { width: 100.0, height: 50.0 }, base.clone()),
        SketchInput::new(
            SketchShape::Diamond { width: 100.0, height: 50.0, roundness: Roundness::Sharp },
            base.clone(),
        ),
    ];
    for c in &changed {
        assert_ne!(key, k(c), "{c:?}");
    }
    // All different from each other too.
    for (i, a) in changed.iter().enumerate() {
        for b in &changed[i + 1..] {
            assert_ne!(k(a), k(b));
        }
    }
}

#[test]
fn key_ignores_what_cannot_change_the_output() {
    let k = |p: SketchParams| sketch_key(&rect(100.0, 50.0, p));
    // No fill: fill parameters do not matter.
    assert_eq!(
        k(SketchParams { fill: FillStyle::None, fill_gap: Some(3.0), fill_angle: Some(7.0), ..params(1) }),
        k(SketchParams { fill: FillStyle::None, ..params(1) })
    );
    // Roughness 0 and no pattern: seed and bowing do not matter.
    assert_eq!(
        k(SketchParams { roughness: 0.0, seed: 1, bowing: 1.0, ..params(1) }),
        k(SketchParams { roughness: 0.0, seed: 99, bowing: 3.0, ..params(1) })
    );
    // Solid fill ignores gap and angle.
    assert_eq!(
        k(SketchParams { fill: FillStyle::Solid, fill_gap: Some(3.0), ..params(1) }),
        k(SketchParams { fill: FillStyle::Solid, ..params(1) })
    );
    // -0.0 equals 0.0, NaN equals NaN.
    assert_eq!(sketch_key(&rect(-0.0, 50.0, params(1))), sketch_key(&rect(0.0, 50.0, params(1))));
    assert_eq!(sketch_key(&rect(f64::NAN, 50.0, params(1))), sketch_key(&rect(0.0, 50.0, params(1))));
}

#[test]
fn equal_keys_mean_equal_output_for_ignored_params() {
    let a = SketchParams { fill: FillStyle::None, fill_gap: Some(3.0), ..params(1) };
    let b = SketchParams { fill: FillStyle::None, ..params(1) };
    assert_eq!(sketch(&rect(100.0, 50.0, a)), sketch(&rect(100.0, 50.0, b)));
    let a = SketchParams { roughness: 0.0, seed: 1, ..params(1) };
    let b = SketchParams { roughness: 0.0, seed: 99, ..params(1) };
    assert_eq!(sketch(&rect(100.0, 50.0, a)), sketch(&rect(100.0, 50.0, b)));
}

/// The input is in local coordinates and has no position or rotation, so the same description
/// always has the same key. A moved element is the same cache entry: the caller translates.
/// Resizing is a different entry.
#[test]
fn dragging_never_misses_and_resizing_does() {
    let mut cache = SketchCache::new(16);
    let input = rect(160.0, 90.0, SketchParams { fill: FillStyle::Hachure, ..params(11) });
    let first = cache.get(&input);
    for _ in 0..500 {
        // 500 drag frames: the element is translated by the caller, the description is unchanged.
        let g = cache.get(&input);
        assert!(std::sync::Arc::ptr_eq(&first, &g));
    }
    assert_eq!(cache.stats().misses, 1);
    assert_eq!(cache.stats().hits, 500);
    // A resize drag changes the size every frame: every frame regenerates.
    for i in 0..10 {
        cache.get(&rect(160.0 + f64::from(i) + 1.0, 90.0, input.params.clone()));
    }
    assert_eq!(cache.stats().misses, 11);
}

// ---------------------------------------------------------------- arrows

fn arrow(start: Arrowhead, end: Arrowhead, line: LineDash, roughness: f64) -> SketchGeometry {
    sketch(&SketchInput::new(
        SketchShape::Arrow {
            points: vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)],
            curve: Curve::Straight,
            start,
            end,
            head_size: 15.0,
        },
        SketchParams { roughness, line, ..params(8) },
    ))
}

#[test]
fn every_head_kind_draws_at_both_ends() {
    for h in [
        Arrowhead::ARROW,
        Arrowhead::TRIANGLE,
        Arrowhead::TRIANGLE_OUTLINE,
        Arrowhead::CIRCLE,
        Arrowhead::CIRCLE_OUTLINE,
        Arrowhead::BAR,
        Arrowhead::DIAMOND,
        Arrowhead::DIAMOND_OUTLINE,
    ] {
        let none = arrow(Arrowhead::NONE, Arrowhead::NONE, LineDash::Solid, 0.0);
        let end = arrow(Arrowhead::NONE, h, LineDash::Solid, 0.0);
        let start = arrow(h, Arrowhead::NONE, LineDash::Solid, 0.0);
        let both = arrow(h, h, LineDash::Solid, 0.0);
        let n = |g: &SketchGeometry| g.strokes[0].0.elements().len();
        assert!(n(&end) > n(&none), "{h:?}");
        assert_eq!(n(&start), n(&end), "{h:?}");
        assert!(n(&both) > n(&end), "{h:?}");
        let b = both.bounds().expect("bounds");
        assert!(b.x0 <= 0.0 && b.x1 >= 100.0, "{h:?} {b:?}");
        // Filled kinds emit an ink fill per filled head.
        let expect_fills = usize::from(h.is_filled()) * 2;
        assert_eq!(both.fills.len(), expect_fills, "{h:?}");
        assert!(both.fills.iter().all(|(_, k)| *k == FillKind::Ink));
    }
}

#[test]
fn old_arrow_has_the_old_head_geometry() {
    let g = arrow(Arrowhead::NONE, Arrowhead::ARROW, LineDash::Solid, 0.0);
    assert_eq!(
        g.strokes[0].0.elements(),
        &[
            PathEl::MoveTo(Point::new(0.0, 0.0)),
            PathEl::LineTo(Point::new(100.0, 0.0)),
            PathEl::MoveTo(Point::new(100.0, 0.0)),
            PathEl::LineTo(Point::new(85.0, 7.5)),
            PathEl::MoveTo(Point::new(100.0, 0.0)),
            PathEl::LineTo(Point::new(85.0, -7.5)),
        ]
    );
}

/// An open-head arrow through `pts`, with a head at both ends and the given head size.
fn arrow_through(pts: &[(f64, f64)], size: f64) -> SketchGeometry {
    sketch(&SketchInput::new(
        SketchShape::Arrow {
            points: pts.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            curve: Curve::Straight,
            start: Arrowhead::ARROW,
            end: Arrowhead::ARROW,
            head_size: size,
        },
        SketchParams { roughness: 0.0, ..params(8) },
    ))
}

#[test]
fn a_head_shrinks_to_half_of_a_short_end_segment_and_keeps_a_minimum() {
    // Elements of a sharp-edged arrow: shaft, then the wings as move/line pairs. Wing tip = element 3 and 5.
    let wing = |g: &SketchGeometry, i: usize| match g.strokes[0].0.elements()[i] {
        PathEl::LineTo(p) => p,
        other => panic!("not a line: {other:?}"),
    };
    // 100 long: the full head of 15 behind the tip.
    let long = arrow_through(&[(0.0, 0.0), (100.0, 0.0)], 15.0);
    assert_eq!(wing(&long, 3), Point::new(85.0, 7.5));
    // 20 long: half, 10 behind the tip and 5 to the side, at both ends.
    let short = arrow_through(&[(0.0, 0.0), (20.0, 0.0)], 15.0);
    assert_eq!(wing(&short, 3), Point::new(10.0, 5.0));
    // 2 long: the floor of 4.
    let tiny = arrow_through(&[(0.0, 0.0), (2.0, 0.0)], 15.0);
    assert_eq!(wing(&tiny, 3), Point::new(-2.0, 2.0));
    // A head that was asked smaller than the floor is not enlarged.
    let small = arrow_through(&[(0.0, 0.0), (2.0, 0.0)], 3.0);
    assert_eq!(wing(&small, 3), Point::new(-1.0, 1.5));
}

#[test]
fn each_end_is_fitted_to_its_own_segment() {
    // A short first segment and a long last one: only the start head shrinks.
    let g = arrow_through(&[(0.0, 0.0), (10.0, 0.0), (110.0, 0.0)], 15.0);
    let b = g.bounds().expect("bounds");
    let els = g.strokes[0].0.elements();
    let ys: Vec<f64> = els
        .iter()
        .filter_map(|e| match e {
            PathEl::LineTo(p) | PathEl::MoveTo(p) => Some(p.y.abs()),
            _ => None,
        })
        .collect();
    // End head (first in the path after the shaft) has wings of 7.5, the start head 2.5.
    assert!(ys.iter().any(|y| (y - 7.5).abs() < 1e-9) && ys.iter().any(|y| (y - 2.5).abs() < 1e-9), "{ys:?}");
    assert!(b.height() <= 15.0 + 1e-9, "{b:?}");
}

#[test]
fn a_shrunk_head_stays_inside_the_full_size_head_box() {
    for len in [0.0_f64, 0.5, 3.0, 8.0, 20.0, 29.0, 31.0, 500.0] {
        for h in [Arrowhead::ARROW, Arrowhead::TRIANGLE, Arrowhead::CIRCLE, Arrowhead::DIAMOND, Arrowhead::BAR] {
            let g = sketch(&SketchInput::new(
                SketchShape::Arrow {
                    points: vec![Point::new(0.0, 0.0), Point::new(len.max(0.01), 0.0)],
                    curve: Curve::Straight,
                    start: Arrowhead::NONE,
                    end: h,
                    head_size: 15.0,
                },
                SketchParams { roughness: 0.0, ..params(8) },
            ));
            let b = g.bounds().expect("bounds");
            assert!(b.height() <= 15.0 + 1e-9 && b.x0 >= -15.0 - 1e-9, "{h:?} len {len}: {b:?}");
        }
    }
}

#[test]
fn dashed_shaft_keeps_solid_heads() {
    let g = arrow(Arrowhead::NONE, Arrowhead::TRIANGLE, LineDash::Dashed, 1.0);
    // Two passes, each a dashed shaft and a solid head.
    assert_eq!(g.strokes.len(), 4);
    let dashed = g.strokes.iter().filter(|(_, s)| s.dash.is_some()).count();
    assert_eq!(dashed, 2);
    assert_eq!(g.strokes[0].1.dash, Some([8.0, 4.0]));
    let dotted = arrow(Arrowhead::NONE, Arrowhead::NONE, LineDash::Dotted, 0.0);
    assert_eq!(dotted.strokes[0].1.dash, Some([2.0, 4.0]));
}

#[test]
fn zero_length_arrow_and_single_point_draw_nothing() {
    for pts in [vec![], vec![Point::new(1.0, 1.0)], vec![Point::new(1.0, 1.0); 3]] {
        let g = sketch(&SketchInput::new(
            SketchShape::Arrow {
                points: pts,
                curve: Curve::Straight,
                start: Arrowhead::ARROW,
                end: Arrowhead::ARROW,
                head_size: 15.0,
            },
            params(1),
        ));
        assert!(g.is_empty());
    }
}

#[test]
fn the_default_arrowhead_is_none() {
    assert_eq!(Arrowhead::default(), Arrowhead::NONE);
    assert!(!Arrowhead::NONE.is_filled());
}

// ---------------------------------------------------------------- fills

fn fill_input(w: f64, h: f64, fill: FillStyle, gap: Option<f64>) -> SketchInput {
    rect(w, h, SketchParams { fill, fill_gap: gap, ..params(21) })
}

#[test]
fn patterned_fill_emits_tint_then_hatch_clipped_to_the_outline() {
    let g = sketch(&fill_input(200.0, 100.0, FillStyle::Hachure, None));
    assert_eq!(g.fills.len(), 2);
    assert_eq!(g.fills[0].1, FillKind::Tint { alpha: 0.15 });
    assert!(matches!(&g.fills[1].1, FillKind::Hatch { line_width, .. } if (*line_width - 1.0).abs() < 1e-12));
    let solid = sketch(&fill_input(200.0, 100.0, FillStyle::Solid, None));
    assert_eq!(solid.fills.len(), 1);
    assert_eq!(solid.fills[0].1, FillKind::Solid);
    assert!(sketch(&fill_input(200.0, 100.0, FillStyle::None, None)).fills.is_empty());
}

#[test]
fn open_lines_take_no_fill() {
    let g = sketch(&SketchInput::new(
        SketchShape::Polyline { points: wiggle(5), curve: Curve::Straight, closed: false },
        SketchParams { fill: FillStyle::Solid, ..params(1) },
    ));
    assert!(g.fills.is_empty());
    assert!(!g.strokes.is_empty());
}

#[test]
fn smaller_gap_means_more_lines() {
    let n = |gap| sketch(&fill_input(200.0, 100.0, FillStyle::Hachure, Some(gap))).element_count();
    assert!(n(4.0) > n(8.0));
    assert!(n(8.0) > n(32.0));
}

/// Number of subpaths of the hatch path.
fn hatch_subpaths(g: &SketchGeometry) -> usize {
    g.fills
        .iter()
        .filter(|(_, k)| matches!(k, FillKind::Hatch { .. }))
        .map(|(p, _)| p.elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count())
        .sum()
}

#[test]
fn hachure_line_count_is_capped() {
    // Without the cap: 1e7 x 1e7 box at gap 1 is 2e7 lines per direction.
    let g = sketch(&fill_input(1.0e7, 1.0e7, FillStyle::Hachure, Some(1.0)));
    let lines = hatch_subpaths(&g);
    assert!(lines > 0);
    // A hachure line is up to two ops after roughr's double stroke; allow for that.
    assert!(lines as f64 <= MAX_HACHURE_LINES * 2.0 + 10.0, "{lines}");
    let g = sketch(&fill_input(1.0e7, 1.0e7, FillStyle::CrossHatch, Some(1.0)));
    assert!(hatch_subpaths(&g) as f64 <= MAX_HACHURE_LINES * 4.0 + 20.0);
}

#[test]
fn the_cap_does_not_touch_ordinary_shapes() {
    // 2000 x 1000 at gap 8: 375 lines needed, under the cap. Same pattern as asking for the cap
    // explicitly would give; the gap is not widened.
    let a = sketch(&fill_input(2000.0, 1000.0, FillStyle::Hachure, Some(8.0)));
    let wider = sketch(&fill_input(2000.0, 1000.0, FillStyle::Hachure, Some(8.001)));
    assert_ne!(a, wider);
}

#[test]
fn dots_dashes_and_zigzags_are_bounded() {
    for (fill, limit) in [
        (FillStyle::Dots, MAX_FILL_DOTS * 24.0),
        (FillStyle::Dashed, MAX_FILL_SEGMENTS * 8.0),
        (FillStyle::ZigZagLine, MAX_FILL_SEGMENTS * 8.0),
        (FillStyle::ZigZag, MAX_FILL_SEGMENTS * 8.0),
    ] {
        let g = sketch(&fill_input(5.0e5, 5.0e5, fill, Some(1.0)));
        assert!((g.element_count() as f64) < limit, "{fill:?}: {}", g.element_count());
    }
}

// ---------------------------------------------------------------- hostile

fn hostile_floats() -> Vec<f64> {
    vec![
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        f64::EPSILON,
        -0.0,
        0.0,
        -1.0,
        1.0,
        1.0e-300,
        1.0e300,
        1.0e15,
    ]
}

fn fills() -> [FillStyle; 8] {
    [
        FillStyle::None,
        FillStyle::Solid,
        FillStyle::Hachure,
        FillStyle::CrossHatch,
        FillStyle::ZigZag,
        FillStyle::Dots,
        FillStyle::Dashed,
        FillStyle::ZigZagLine,
    ]
}

/// A path element is about 6 numbers; this is far above anything legitimate and far below
/// anything that hurts.
const OUTPUT_BUDGET: usize = 400_000;

#[test]
fn hostile_params_never_panic_and_stay_bounded() {
    let vals = hostile_floats();
    for &v in &vals {
        for fill in fills() {
            let p = SketchParams {
                roughness: v,
                bowing: v,
                seed: u32::MAX,
                stroke_width: v,
                line: LineDash::Dashed,
                fill,
                fill_gap: Some(v),
                fill_angle: Some(v),
            };
            for input in every_shape(p) {
                let g = sketch(&input);
                assert!(all_finite(&g), "{v:e} {fill:?} {:?}", input.shape);
                assert!(g.element_count() < OUTPUT_BUDGET, "{v:e} {fill:?}: {}", g.element_count());
                let _ = sketch_key(&input);
            }
        }
    }
}

#[test]
fn hostile_sizes_never_panic_and_stay_bounded() {
    let vals = hostile_floats();
    for &w in &vals {
        for &h in &vals {
            for fill in fills() {
                let p = SketchParams { fill, ..params(3) };
                // Every fill on a plain rectangle; the other shapes share the fill code, so one
                // patterned fill each keeps the debug-build run short.
                let mut shapes = vec![SketchShape::Rect { width: w, height: h, roundness: Roundness::Sharp }];
                if matches!(fill, FillStyle::None | FillStyle::Hachure) {
                    shapes.extend([
                        SketchShape::Rect { width: w, height: h, roundness: Roundness::Radius(w) },
                        SketchShape::Rect { width: w, height: h, roundness: Roundness::Adaptive(h) },
                        SketchShape::Rect { width: w, height: h, roundness: Roundness::Proportional(w) },
                        SketchShape::Ellipse { width: w, height: h },
                        SketchShape::Diamond { width: w, height: h, roundness: Roundness::Radius(h) },
                    ]);
                }
                for shape in shapes {
                    let input = SketchInput::new(shape, p.clone());
                    let g = sketch(&input);
                    assert!(all_finite(&g), "{w:e}x{h:e} {fill:?} {:?}", input.shape);
                    assert!(g.element_count() < OUTPUT_BUDGET, "{w:e}x{h:e} {fill:?}: {}", g.element_count());
                    let _ = sketch_key(&input);
                }
            }
        }
    }
}

#[test]
fn hostile_points_never_panic_and_stay_bounded() {
    let vals = hostile_floats();
    let mut pts = Vec::new();
    for &x in &vals {
        for &y in &vals {
            pts.push(Point::new(x, y));
        }
    }
    let pressures: Vec<f64> = vals.iter().cycle().take(pts.len() + 5).copied().collect();
    for fill in fills() {
        let p = SketchParams { fill, ..params(3) };
        for len in [0, 1, 2, 3, pts.len()] {
            let slice = pts[..len].to_vec();
            let shapes = [
                SketchShape::Polyline { points: slice.clone(), curve: Curve::Straight, closed: false },
                SketchShape::Polyline { points: slice.clone(), curve: Curve::Smooth, closed: true },
                SketchShape::Arrow {
                    points: slice.clone(),
                    curve: Curve::Smooth,
                    start: Arrowhead::DIAMOND,
                    end: Arrowhead::CIRCLE,
                    head_size: f64::NAN,
                },
                SketchShape::Arrow {
                    points: slice.clone(),
                    curve: Curve::Straight,
                    start: Arrowhead::BAR,
                    end: Arrowhead::TRIANGLE,
                    head_size: f64::MAX,
                },
                SketchShape::Freehand {
                    points: slice.clone(),
                    pressures: pressures.clone(),
                    closed: false,
                    profile: PressureProfile { size_scale: f64::NAN, thinning: f64::INFINITY },
                },
                SketchShape::Freehand {
                    points: slice.clone(),
                    pressures: vec![],
                    closed: true,
                    profile: PressureProfile::default(),
                },
            ];
            for shape in shapes {
                let input = SketchInput::new(shape, p.clone());
                let g = sketch(&input);
                assert!(all_finite(&g), "{fill:?} len {len} {:?}", input.shape);
                assert!(g.element_count() < OUTPUT_BUDGET, "{fill:?} len {len}: {}", g.element_count());
                let _ = sketch_key(&input);
            }
        }
    }
}

#[test]
fn a_million_points_are_decimated() {
    let n = 1_000_000;
    let pts: Vec<Point> = (0..n).map(|i| Point::new(f64::from(i) * 0.1, f64::from(i % 17))).collect();
    let pressures: Vec<f64> = (0..n).map(|i| f64::from(i % 10) / 10.0).collect();
    let line = sketch(&SketchInput::new(
        SketchShape::Polyline { points: pts.clone(), curve: Curve::Smooth, closed: false },
        params(1),
    ));
    // Two strokes, one element per kept point (+1 for the move).
    assert!(line.element_count() <= 2 * (MAX_POINTS + 2), "{}", line.element_count());
    let free = sketch(&SketchInput::new(
        SketchShape::Freehand { points: pts, pressures, closed: false, profile: PressureProfile::default() },
        params(1),
    ));
    // Left and right edge: two elements per kept point.
    assert!(free.element_count() <= 2 * (MAX_POINTS + 2) + 4, "{}", free.element_count());
    assert!(!free.is_empty());
    // The decimated stroke still reaches both ends.
    let b = free.bounds().expect("bounds");
    assert!(b.x0 < 10.0 && b.x1 > 99_000.0, "{b:?}");
}

#[test]
fn zero_and_negative_sizes_draw_nothing_or_a_clean_shape() {
    for (w, h) in [(0.0, 0.0), (-5.0, -5.0), (f64::NAN, f64::NAN)] {
        assert!(sketch(&rect(w, h, params(1))).is_empty(), "{w} {h}");
        assert!(sketch(&SketchInput::new(SketchShape::Ellipse { width: w, height: h }, params(1))).is_empty());
    }
    // A zero-height rectangle is a line: still drawn, never NaN.
    let g = sketch(&rect(100.0, 0.0, SketchParams { fill: FillStyle::Hachure, ..params(1) }));
    assert!(all_finite(&g));
    assert!(!g.strokes.is_empty());
}

#[test]
fn huge_sizes_are_clamped_not_overflowed() {
    let g = sketch(&rect(f64::MAX, f64::MAX, params(1)));
    let b = g.bounds().expect("bounds");
    // Clamped to 1e9, plus the bow of a 1e9-long edge (roughness 1: len / 200) and the noise.
    assert!(b.x1 <= 1.0e9 * 1.01 && b.y1 <= 1.0e9 * 1.01, "{b:?}");
    assert!(b.x0 >= -1.0e7 && b.y0 >= -1.0e7, "{b:?}");
}

#[test]
fn rounded_rect_radius_is_limited_to_half_the_short_side() {
    assert_eq!(Roundness::Radius(1000.0).radius(100.0, 40.0), 20.0);
    assert_eq!(Roundness::Proportional(0.25).radius(100.0, 40.0), 10.0);
    assert_eq!(Roundness::Adaptive(32.0).radius(100.0, 40.0), 10.0);
    assert_eq!(Roundness::Adaptive(32.0).radius(1000.0, 500.0), 32.0);
    assert_eq!(Roundness::Radius(f64::NAN).radius(100.0, 40.0), 0.0);
    assert_eq!(Roundness::Sharp.radius(100.0, 40.0), 0.0);
}

#[test]
fn old_defaults() {
    let p = SketchParams::default();
    assert_eq!((p.roughness, p.bowing, p.stroke_width), (1.0, 1.0, 2.0));
    let _: Option<Rect> = sketch(&rect(10.0, 10.0, p)).bounds();
}
