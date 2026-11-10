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
