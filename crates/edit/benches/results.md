# Benchmark results: arrow binding

`cargo bench -p drafftink-edit --bench arrows_binding`

Scene: 1,000 rectangles in a 40-column grid, 1,000 arrows (one per rectangle, the end bound `orbit` to it, the
other end free). Each iteration opens a transaction, moves 100 rectangles (every 10th) and calls the solver once
for the 100 moved ids. The transaction is dropped, so every iteration starts from the same scene. The benchmark
asserts that exactly the 100 arrows of the moved rectangles are re-solved.

Measured on the development machine (Linux, release profile, criterion median):

| solver | time for 100 moved shapes |
|---|---|
| `update_bound_arrows` (finds arrows by scanning the transaction) | 0.37 ms |
| `update_bound_arrows_indexed` (uses `DerivedIndex::bound_arrows`) | 0.27 ms |
| `update_bound_arrows`, every arrow an elbow arrow (A* per arrow) | 0.55 ms |

The move itself (100 `tx.update` calls) is part of each figure. For comparison, the old app rebuilt the whole
document on every local edit (64 ms at 1k shapes, `research/baseline-perf.md`) and had no bound arrows at all.

## Elbow arrows around other shapes

`cargo bench -p drafftink-edit --bench arrows_binding -- elbow_around`

Scene: the 1,000 rectangles above plus 100 elbow arrows, each from a shape to the one three columns to its right with
two shapes in the way, routed once.

| what | time |
|---|---|
| `route_one_arrow`: `reroute_forced` of one such arrow in a fresh transaction (box index) | 20 us |
| `move_one_shape_and_back`: a shape in the way of one arrow moves 3,000 units away and back; each time the commit, then `update_bound_arrows_near` in a fresh transaction and its commit | 56 us for both |
| `route_across_a_row_of_38_shapes`: one arrow from the first to the last shape of a row of 40, 38 shapes in the way | 0.63 ms |

The existing `elbow` row of the first table went from 0.55 ms to 1.2 ms: those 100 moved shapes are obstacles now,
and the 100 elbow arrows near them are routed again; the transaction has written, so the router scans once.
