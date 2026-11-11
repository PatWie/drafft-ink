//! Layout properties: deterministic, no overlapping nodes, respects spacing and direction.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_edit::layout::{Algorithm, Direction, Gap, LayoutError, LayoutGraph, LayoutOpts, compute_layout};
use kurbo::{Rect, Size};
use proptest::prelude::*;

fn rects(g: &LayoutGraph, pos: &[kurbo::Point]) -> Vec<Rect> {
    pos.iter().zip(&g.sizes).map(|(p, s)| Rect::from_origin_size(*p, *s)).collect()
}

/// The distance between two rects along the axis where they are further apart (0 if they overlap).
fn separation(a: &Rect, b: &Rect) -> f64 {
    let gx = a.x0.max(b.x0) - a.x1.min(b.x1);
    let gy = a.y0.max(b.y0) - a.y1.min(b.y1);
    gx.max(gy)
}

fn check_layout(g: &LayoutGraph, o: &LayoutOpts) -> Result<(), TestCaseError> {
    let r = compute_layout(g, o).unwrap();
    prop_assert_eq!(r.positions.len(), g.sizes.len());
    prop_assert_eq!(r.waypoints.len(), g.edges.len());
    for p in &r.positions {
        prop_assert!(p.x.is_finite() && p.y.is_finite());
    }
    let rs = rects(g, &r.positions);
    let want = o.node_gap.min(o.layer_gap) - 1e-6;
    for i in 0..rs.len() {
        for j in i + 1..rs.len() {
            let s = separation(&rs[i], &rs[j]);
            prop_assert!(
                s >= want,
                "{:?}: nodes {i} and {j} are only {s} apart ({:?} vs {:?})",
                o.algorithm,
                rs[i],
                rs[j]
            );
        }
    }
    // The bounding box starts at the origin.
    let min_x = rs.iter().map(|r| r.x0).fold(f64::INFINITY, f64::min);
    let min_y = rs.iter().map(|r| r.y0).fold(f64::INFINITY, f64::min);
    prop_assert!(min_x.abs() < 1e-9 && min_y.abs() < 1e-9, "{min_x} {min_y}");
    // Deterministic: a second run gives exactly the same answer.
    prop_assert_eq!(compute_layout(g, o).unwrap(), r);
    Ok(())
}

fn graph_strategy() -> impl Strategy<Value = LayoutGraph> {
    (1usize..30).prop_flat_map(|n| {
        let sizes = prop::collection::vec((10.0f64..220.0, 10.0f64..120.0), n);
        let edges = prop::collection::vec((0..n, 0..n), 0..(3 * n));
        (sizes, edges).prop_map(|(sizes, edges)| LayoutGraph {
            sizes: sizes.into_iter().map(|(w, h)| Size::new(w, h)).collect(),
            edges,
            fixed: Vec::new(),
        })
    })
}

fn forest_strategy() -> impl Strategy<Value = LayoutGraph> {
    (1usize..30).prop_flat_map(|n| {
        let sizes = prop::collection::vec((10.0f64..220.0, 10.0f64..120.0), n);
        // Node i > 0 either has a parent among the earlier nodes or is a root.
        let parents = prop::collection::vec(prop::option::of(any::<prop::sample::Index>()), n);
        (sizes, parents).prop_map(move |(sizes, parents)| {
            let edges =
                parents.iter().enumerate().skip(1).filter_map(|(i, p)| p.as_ref().map(|ix| (ix.index(i), i))).collect();
            LayoutGraph { sizes: sizes.into_iter().map(|(w, h)| Size::new(w, h)).collect(), edges, fixed: Vec::new() }
        })
    })
}

fn opts(algorithm: Algorithm, direction: Direction, node_gap: f64, layer_gap: f64) -> LayoutOpts {
    LayoutOpts { algorithm, direction, node_gap, layer_gap, columns: None }
}

fn direction() -> impl Strategy<Value = Direction> {
    prop_oneof![Just(Direction::Down), Just(Direction::Up), Just(Direction::Left), Just(Direction::Right)]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn layered_has_no_overlap_and_is_deterministic(g in graph_strategy(), d in direction(), ng in 0.0f64..60.0, lg in 0.0f64..120.0) {
        check_layout(&g, &opts(Algorithm::Layered, d, ng, lg))?;
    }

    #[test]
    fn force_has_no_overlap_and_is_deterministic(g in graph_strategy(), ng in 0.0f64..60.0, lg in 0.0f64..120.0) {
        check_layout(&g, &opts(Algorithm::Force, Direction::Down, ng, lg))?;
    }

    #[test]
    fn grid_has_no_overlap_and_is_deterministic(g in graph_strategy(), ng in 0.0f64..60.0, lg in 0.0f64..120.0, cols in prop::option::of(1u32..8)) {
        let mut o = opts(Algorithm::Grid, Direction::Down, ng, lg);
        o.columns = cols;
        check_layout(&g, &o)?;
    }

    #[test]
    fn tree_has_no_overlap_and_is_deterministic(g in forest_strategy(), d in direction(), ng in 0.0f64..60.0, lg in 0.0f64..120.0) {
        check_layout(&g, &opts(Algorithm::Tree, d, ng, lg))?;
    }

    #[test]
    fn layered_flows_forward_on_acyclic_graphs(
        sizes in prop::collection::vec((10.0f64..200.0, 10.0f64..100.0), 2..25),
        raw in prop::collection::vec((any::<prop::sample::Index>(), any::<prop::sample::Index>()), 0..40),
        d in direction(),
    ) {
        let n = sizes.len();
        let edges: Vec<(usize, usize)> = raw.iter().map(|(a, b)| (a.index(n), b.index(n))).filter(|(a, b)| a < b).collect();
        let g = LayoutGraph { sizes: sizes.into_iter().map(|(w, h)| Size::new(w, h)).collect(), edges: edges.clone(), fixed: Vec::new() };
        let r = compute_layout(&g, &opts(Algorithm::Layered, d, 30.0, 60.0)).unwrap();
        let rs = rects(&g, &r.positions);
        for (a, b) in edges {
            let ok = match d {
                Direction::Down => rs[a].y1 <= rs[b].y0 + 1e-6,
                Direction::Up => rs[b].y1 <= rs[a].y0 + 1e-6,
                Direction::Right => rs[a].x1 <= rs[b].x0 + 1e-6,
                Direction::Left => rs[b].x1 <= rs[a].x0 + 1e-6,
            };
            prop_assert!(ok, "edge {a}->{b} does not flow {d:?}: {:?} {:?}", rs[a], rs[b]);
        }
    }

    #[test]
    fn up_is_down_mirrored(g in graph_strategy()) {
        let down = compute_layout(&g, &opts(Algorithm::Layered, Direction::Down, 30.0, 60.0)).unwrap();
        let up = compute_layout(&g, &opts(Algorithm::Layered, Direction::Up, 30.0, 60.0)).unwrap();
        let total = rects(&g, &down.positions).iter().map(|r| r.y1).fold(0.0, f64::max);
        for ((d, u), s) in down.positions.iter().zip(&up.positions).zip(&g.sizes) {
            prop_assert!((d.x - u.x).abs() < 1e-6);
            prop_assert!((total - d.y - s.height - u.y).abs() < 1e-6);
        }
    }
}

fn graph(sizes: &[(f64, f64)], edges: &[(usize, usize)]) -> LayoutGraph {
    LayoutGraph {
        sizes: sizes.iter().map(|(w, h)| Size::new(*w, *h)).collect(),
        edges: edges.to_vec(),
        fixed: Vec::new(),
    }
}

#[test]
fn crossing_reduction_untangles_a_simple_crossing() {
    // a -> d and b -> c would cross with the input order c, d.
    let g = graph(&[(50.0, 30.0); 4], &[(0, 3), (1, 2)]);
    let r = compute_layout(&g, &opts(Algorithm::Layered, Direction::Down, 30.0, 60.0)).unwrap();
    let cx = |i: usize| r.positions[i].x + 25.0;
    assert_eq!(r.positions[0].y, r.positions[1].y);
    assert_eq!(r.positions[2].y, r.positions[3].y);
    assert!(r.positions[2].y > r.positions[0].y);
    // a is left of b, so d (a's target) is left of c (b's target).
    assert_eq!(cx(0) < cx(1), cx(3) < cx(2));
}

#[test]
fn cycles_are_handled() {
    let g = graph(&[(40.0, 20.0); 3], &[(0, 1), (1, 2), (2, 0)]);
    let r = compute_layout(&g, &opts(Algorithm::Layered, Direction::Right, 20.0, 40.0)).unwrap();
    let xs: Vec<f64> = r.positions.iter().map(|p| p.x).collect();
    // A three-cycle becomes a chain of three layers.
    let mut sorted = xs.clone();
    sorted.sort_by(f64::total_cmp);
    assert!(sorted[0] < sorted[1] && sorted[1] < sorted[2]);
}

#[test]
fn self_loops_and_duplicate_edges_do_not_confuse_layering() {
    let g = graph(&[(40.0, 20.0); 2], &[(0, 0), (0, 1), (0, 1), (1, 1)]);
    let r = compute_layout(&g, &opts(Algorithm::Layered, Direction::Down, 20.0, 40.0)).unwrap();
    assert!(r.positions[1].y > r.positions[0].y);
}

#[test]
fn long_edges_get_waypoints_in_edge_direction() {
    // 0 -> 1 -> 2 -> 3 and 0 -> 3 (skips two layers); 3 -> 0 is the same pair reversed.
    let g = graph(&[(40.0, 20.0); 4], &[(0, 1), (1, 2), (2, 3), (0, 3), (3, 0)]);
    let r = compute_layout(&g, &opts(Algorithm::Layered, Direction::Down, 20.0, 40.0)).unwrap();
    assert_eq!(r.waypoints[0].len(), 0);
    assert_eq!(r.waypoints[3].len(), 2);
    assert_eq!(r.waypoints[4].len(), 2);
    // Forward waypoints go down, the reversed edge lists them going up.
    assert!(r.waypoints[3][0].y < r.waypoints[3][1].y);
    assert!(r.waypoints[4][0].y > r.waypoints[4][1].y);
    assert_eq!(r.waypoints[3][0], r.waypoints[4][1]);
}

#[test]
fn grid_fills_rows() {
    let g = graph(&[(40.0, 20.0); 5], &[]);
    let mut o = opts(Algorithm::Grid, Direction::Down, 10.0, 10.0);
    o.columns = Some(2);
    let r = compute_layout(&g, &o).unwrap();
    assert_eq!(r.positions[0].y, r.positions[1].y);
    assert_eq!(r.positions[2].y, r.positions[3].y);
    assert_eq!(r.positions[4].x, r.positions[0].x);
    assert_eq!(r.positions[1].x - r.positions[0].x, 50.0);
    // Default: ceil(sqrt(5)) = 3 columns.
    let r = compute_layout(&g, &opts(Algorithm::Grid, Direction::Down, 10.0, 10.0)).unwrap();
    assert_eq!(r.positions[0].y, r.positions[2].y);
    assert!(r.positions[3].y > r.positions[0].y);
}

#[test]
fn force_pulls_connected_nodes_closer_than_unconnected_ones() {
    let mut sizes = vec![(40.0, 40.0); 6];
    sizes[5] = (40.0, 40.0);
    // 0-1-2 are connected in a chain, 3-4-5 in another.
    let g = graph(&sizes, &[(0, 1), (1, 2), (3, 4), (4, 5)]);
    let r = compute_layout(&g, &opts(Algorithm::Force, Direction::Down, 20.0, 40.0)).unwrap();
    let c = |i: usize| r.positions[i] + kurbo::Vec2::new(20.0, 20.0);
    let d = |a: usize, b: usize| (c(a) - c(b)).hypot();
    let near = (d(0, 1) + d(1, 2) + d(3, 4) + d(4, 5)) / 4.0;
    let far = (d(0, 3) + d(2, 5) + d(0, 5) + d(2, 3)) / 4.0;
    assert!(near < far, "{near} vs {far}");
}

#[test]
fn force_separates_nodes_stacked_on_one_spot() {
    // Same size, no edges, many nodes: the spiral start and the overlap removal must still separate them.
    let g = graph(&[(100.0, 100.0); 40], &[]);
    check_layout(&g, &opts(Algorithm::Force, Direction::Down, 10.0, 10.0)).unwrap();
}

#[test]
fn tree_is_rejected_for_graphs_that_are_not_forests() {
    let g = graph(&[(40.0, 20.0); 3], &[(0, 1), (2, 1)]);
    assert_eq!(
        compute_layout(&g, &opts(Algorithm::Tree, Direction::Down, 10.0, 10.0)),
        Err(LayoutError::NotForest { node: 1 })
    );
    let g = graph(&[(40.0, 20.0); 3], &[(0, 1), (1, 2), (2, 1)]);
    assert!(matches!(
        compute_layout(&g, &opts(Algorithm::Tree, Direction::Down, 10.0, 10.0)),
        Err(LayoutError::NotForest { .. })
    ));
    let g = graph(&[(40.0, 20.0); 1], &[(0, 0)]);
    assert_eq!(
        compute_layout(&g, &opts(Algorithm::Tree, Direction::Down, 10.0, 10.0)),
        Err(LayoutError::NotForest { node: 0 })
    );
}

#[test]
fn bad_input_is_a_typed_error() {
    let g = graph(&[(40.0, 20.0), (f64::NAN, 5.0)], &[]);
    assert_eq!(compute_layout(&g, &LayoutOpts::default()), Err(LayoutError::BadSize { node: 1 }));
    let g = graph(&[(40.0, 20.0)], &[(0, 3)]);
    assert_eq!(compute_layout(&g, &LayoutOpts::default()), Err(LayoutError::BadEdge { edge: 0 }));
    let g = graph(&[(40.0, 20.0)], &[]);
    let bad = LayoutOpts { node_gap: -1.0, ..LayoutOpts::default() };
    assert_eq!(compute_layout(&g, &bad), Err(LayoutError::BadGap(Gap::Node)));
    let none = LayoutOpts { algorithm: Algorithm::None, ..LayoutOpts::default() };
    assert_eq!(compute_layout(&g, &none), Err(LayoutError::MissingPosition { node: 0 }));
    // An empty graph is fine.
    assert!(compute_layout(&LayoutGraph::default(), &LayoutOpts::default()).unwrap().positions.is_empty());
}

#[test]
fn large_graphs_finish() {
    let n = 600;
    let sizes = vec![(80.0, 48.0); n];
    let edges: Vec<(usize, usize)> = (0..n).flat_map(|i| [(i, (i * 7 + 1) % n), (i, (i * 13 + 5) % n)]).collect();
    let g = graph(&sizes, &edges);
    for a in [Algorithm::Layered, Algorithm::Grid, Algorithm::Force] {
        let r = compute_layout(&g, &opts(a, Direction::Down, 40.0, 80.0)).unwrap();
        assert_eq!(r.positions.len(), n);
        assert!(!drafftink_edit::layout::rects_overlap(&rects(&g, &r.positions), 1e-6), "{a:?}");
    }
}
