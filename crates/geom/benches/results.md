# drafftink-geom benchmark results

Command: `cargo bench -p drafftink-geom --bench spatial -- --warm-up-time 1 --measurement-time 3`
Box: the research machine (Ubuntu x86_64, 64 cores), release profile, 2026-11-07. Medians.
Scene: the old baseline scene (`research/bench-baseline`): grid of 120 x 80 cells, half rectangles
100 x 60, a quarter ellipses, a quarter lines.

"Old" is the linear scan in drafft-ink at 8ce40ab (`research/baseline-perf.md`). "New" is the R-tree
query plus the exact hit test on the candidates, so both sides answer the same question.

| Operation | n | Old | New | Speed-up |
|---|---|---|---|---|
| point hit test (`shapes_at_point`) | 1k | 60 us | 0.27 us | 220x |
| | 10k | 0.70 ms | 0.32 us | 2,200x |
| | 100k | 9.1 ms | 0.45 us | 20,000x |
| marquee 800x600 (`shapes_in_rect`), index only | 1k / 10k / 100k | 66 us / 0.77 ms / 9.6 ms | 0.49 / 0.54 / 0.69 us | 130x / 1,400x / 14,000x |
| marquee 800x600 with exact `intersects_rect` on the candidates | 1k / 10k / 100k | same | 30 / 24 / 35 us | 2x / 32x / 275x |
| smart-guide input (query zone + `detect_smart_guides`) | 1k / 10k / 100k | 157 us / 1.67 ms / 17.0 ms | 4.0 / 4.2 / 3.2 us | 39x / 400x / 5,300x |

The old numbers include the work of finding the shapes and collecting their bounds. The new
smart-guide number caps the candidates at 200 (`MAX_SNAP_CANDIDATES`) as the old app's event handler did.
The old marquee only compared AABBs; the exact variant does a true shape-versus-rect test, which is
why it is slower than the index alone but still far below the old scan at 10k and above.

Other operations (new code only):

| Operation | 1k | 10k | 100k |
|---|---|---|---|
| index candidates for a point (tolerance 5) | 107 ns | 129 ns | 316 ns |
| nearest entry to a point | 198 ns | 298 ns | 925 ns |
| move one element (`update`) | 190 ns | 356 ns | 366 ns |
| remove + insert one element | 189 ns | 319 ns | 350 ns |
| `bulk_load` of the whole scene | 0.26 ms | 3.0 ms | 27 ms |
| one-by-one `insert` of the whole scene | 0.82 ms | 10.8 ms | not run |

The index costs memory and a build step: 27 ms to load 100k elements once, then every query is
sub-microsecond. A drag that moves a few elements pays about 0.35 us per element for the update.
