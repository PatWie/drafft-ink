# perf/

`budgets.json` holds the limits that `cargo xtask perf` checks. The rules behind them are in
`docs/performance.md` (section 5 describes the file format and the gate).

## The gate

```bash
cargo xtask perf            # work counters at 1k, 10k, 100k elements; part of `cargo xtask ci`
cargo xtask perf --idle     # only the idle and hover tests, a few seconds once built
cargo xtask perf --bench    # curated criterion benches against the timing limits; not part of ci
```

### Work counters (default)

The crates `doc`, `canvas`, `edit`, `render` and `testkit` each have `tests/perf_work.rs`. The gate runs them in
release mode (`cargo test --release --test perf_work`); a debug `cargo test` runs the same tests at 1k and 10k.
They assert on counted work, never on time:

| Crate | What it pins |
|---|---|
| `edit` | move, resize and rotate of a bound rectangle scan 0 elements and solve only the arrows bound to it; frame membership, label refresh, `frame_at` and the shape under a dragged arrow end are answered from an index |
| `canvas` | an edit updates one index entry; a hit test and a marquee ask the index and scan nothing; no bulk rebuild after the first build |
| `doc` | a local edit writes 1 element; a remote update decodes 1; a snapshot decodes each element once; the load leaves undo working |
| `render` | a repeated frame encodes nothing; one edit re-encodes one fragment; a move replays it; a pan over a warm cache regenerates no sketch and lays no text out |
| `testkit` | whole engine: gesture frames (move, resize, rotate, arrow end, pointer drag) scan 0 elements; hover over empty canvas redraws 0 times; an idle interaction has no deadline and no dirty reason |

`Counter::ElementsScanned` is the main counter: the elements visited by a pass over the whole scene. An operation that
serves a gesture frame must leave it at 0 and touch the same number of elements at every scene size.

The tests that drive a scenario of `docs/performance.md` section 4.1 also write a `ScenarioReport` for it
(`ScenarioReport::save_if_requested`, when `DRAFFTINK_PERF_OUT` names a directory; the gate sets it to
`target/perf/work`). The gate then checks those reports against the deterministic limits of `budgets.json` with
`cargo run -p drafftink-profile --example perf_check -- check ...`. Scenarios the tests write: `idle`, `hover@N`,
`drag@N`, `pan@N`, `zoom@N`. A scenario in `budgets.json` without a report fails the gate.

### Timing (`--bench`)

Runs the benches listed in `xtask/src/perf.rs` with `--warm-up-time 0.3 --measurement-time 1 --sample-size 10`, reads
the medians from `target/criterion`, writes them as the report `bench` (`target/perf/bench/bench.json`) and checks them
against the `bench` entry of `budgets.json`. The limits are about ten times the median measured on the research box
(64 cores, load average 19), so a loaded machine passes and a lost index (a scan instead of a lookup) fails. It takes
a few minutes and needs the machine to be otherwise quiet enough; run it alone, not in `ci`.

To add a bench: add its criterion id (`group/function/parameter`) to `TARGETS` in `xtask/src/perf.rs`, run
`cargo xtask perf --bench`, and add `external.<id with / replaced by .>_us` to the `bench` entry with a limit of about
ten times the median. To change a limit: say why in `note`.

## budgets.json

`scenarios[<key>][<metric path>] = {"max": ..., "min": ..., "note": ...}`. Keys are `idle`, `name@1k|10k|100k` and
`bench`. Metric paths are in `docs/performance.md` section 3.3. Rules:

- Deterministic metrics (`counter.*`, `gauge.*`, `redraws.*`, `frame.unexplained`) are strict and hold on every machine.
- Timing metrics (`frame.*`, `phase.*`, `external.*`) are checked only by the app drivers on the reference runner
  (`docs/performance.md` section 5.1). The entries for `idle`, `pan@N` carry them already; `perf_check` skips them
  without `--timing`.
- Tighten a limit in the commit that makes the number better. Loosening needs a reason in `note`.

## The size budget

`budgets.json` also has a `size` section: `size[<metric>] = {"max": bytes, "note": ...}` for the web app's wasm module and its fonts
(`wasm.opt_bytes`, `wasm.gzip_bytes`, `wasm.brotli_bytes`, `fonts.brotli_bytes`). `cargo xtask size` builds the app, measures it and
checks the limits; on a failure it prints the biggest items of the module (`twiggy`). The perf code ignores the section. The numbers and
the reasons are in `docs/performance.md`, "Wasm size". Tighten a limit in the commit that makes the number better; loosening needs a reason in `note`.

## Not done yet

The native and web scenario drivers (`--perf-scenario`, `window.drafftink.perf.runScenario`) belong to the app shells.
When they exist, `cargo xtask perf` runs them and feeds their reports to the same `perf_check`.
