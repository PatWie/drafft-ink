# Performance model and tooling

This document is the perf contract of the app. It covers the redraw rules, the tooling in `crates/profile` (`drafftink-profile`), the `PerfStats` JSON schema, the budgets file, the scenarios, how the gate and the e2e drivers use them, and the baseline numbers the rewrite starts from.

Related: `docs/plan.md` §7 (design), `docs/testing.md` (test layers), `AGENTS.md` ("Idle means idle").

## 1. The perf model

The app draws only when something changed. An idle app uses no CPU and no GPU. Five rules follow.

1. **A redraw needs a dirty reason.** Every redraw request names a `RedrawReason` (§2.3). The idle gate is the only place that turns a request into a frame. A frame that starts with no pending reason is `unexplained`, and the idle check fails on it.
2. **Hover redraws only when the hover state changes.** A pointer move over the canvas runs the hit test, compares the result with the previous hover (element id, handle, cursor icon). It requests `hover_change` only when they differ. A move that changes nothing increments the `redraws_skipped` counter and does nothing else.
3. **Every repaint request is tied to pending work.** A `request_repaint` or `request_repaint_after` call must be reachable only while something is pending: an animation, a load, a timer deadline, a gesture. When the work ends the requests end. No unconditional repaint request exists, and no poll loop runs without a job to poll.
4. **Deadlines are honoured, not polled.** `request_repaint_after(d)` is passed to the event loop as a wake-up at `now + d`, and the frame it causes is tagged `timer`. The old app dropped the delay and kept only the boolean (baseline §2), so timed effects depended on other events arriving.
5. **Other sources wake the loop explicitly.** Each of these sends a message and wakes the loop, and the frame is tagged with the source:

   | Source | Wake-up | Reason |
   |---|---|---|
   | Network (collab sync, presence) | transport task -> channel -> `request_repaint` | `collab` |
   | Autosave finished | job result -> channel -> `request_repaint` | `autosave` |
   | Text caret | one `request_repaint_after(blink_interval)` while a text field has focus | `caret_blink` |
   | Debug panel | one `Repaint::After(500 ms)` while the panel is open, none when it is closed | `debug_panel` |
   | Async load, paste, drop, image decode | result -> channel -> `request_repaint` | `load` |
   | egui animation or tooltip | egui's own `request_repaint_after` | `ui` |
   | Window or canvas resized, scale changed | event loop | `resize` |
   | Surface lost or visible again | event loop | `surface` |

Cheap while moving, full quality at rest: during pan and zoom the encoder may draw a cheaper form of the scene (no sketch regeneration, no extra detail). When the view has been still for a settle time, one deferred repaint (`timer`) draws the full-quality frame.

The frame, in order. Each step is a `Phase` (§2.2) except the first.

```
event / wake-up -> idle gate (reason) -> [input] -> [commands] -> [layout] -> [encode] -> [ui] -> [gpu_submit] -> present
```

Scene encoding keeps one encoded fragment per element, cached by a version number. The scene is assembled from the cache and culled with the spatial index, so the cost of `encode` depends on the number of visible elements, not on the size of the document. Text, math and image caches are bounded. Math is laid out once per edit.

## 2. The profile crate

`drafftink-profile` is layer L0: it depends on no other workspace crate, uses `serde`, `serde_json` and `web-time`, and builds for `wasm32-unknown-unknown`. The clock is `web_time::Instant`, which is `performance.now()` in the browser.

### 2.1 Spans

```rust
use drafftink_profile as profile;

fn encode_scene(/* ... */) {
    let _g = profile::span("encode");   // ends when _g drops, also on unwind
    // ...
    profile::scope("encode_text", || layout());
}
```

- Names are `&'static str`, short snake_case. A name is a key in `PerfStats.spans`. At most 1024 distinct names are kept; further new names are aggregated as `other`.
- Spans nest. Self time is the span's time minus its children's on the same thread. Total time counts only the outermost occurrence of a name on a thread's stack, so recursion is not counted twice.
- Cost:

  | Build | Cost of one span |
  |---|---|
  | `default-features = false` (no `enabled` feature) | zero: `Span` is a zero-sized type and the calls are empty inline functions |
  | compiled in, flag off (the default at run time) | one relaxed atomic load |
  | flag on | two clock reads, a push and a pop on a thread-local stack, no lock |

  Measure with `cargo bench -p drafftink-profile` (benchmark groups `span`, `always_on`, `frame`). Measured numbers are in §7.4; the bench is the source of truth.
- Threads: each thread keeps its own stack and a small buffer. The buffer moves to the shared bounded ring (32,768 events) when the thread's outermost span ends, so the shared lock is taken once per top-level span. A thread that never ends a span does not publish. On wasm there is one thread.
- Turn recording on with `profile::set_enabled(true)`. Native apps call `profile::enable_from_env("DRAFFTINK_PERF")` at start-up; the web entry exposes `window.drafftink.perf.enable()`. Counters and redraw reasons are always on and need no flag.
- Cargo feature `enabled` (default on) compiles the timing code in. A size-sensitive build can depend on the crate with `default-features = false`; then spans, phases and frames record nothing, and counters and redraw reasons still work. Apps forward the feature as their own `profile` feature.

### 2.2 Frames and phases

```rust
profile::begin_frame();                       // takes the pending redraw reasons
{ let _p = profile::phase(Phase::Input);  /* ... */ }
{ let _p = profile::phase(Phase::Encode); /* ... */ }
profile::end_frame();                         // after present
```

`Phase` values and what they cover:

| Phase | Covers |
|---|---|
| `input` | event handling, gesture updates |
| `commands` | command execution through the registry |
| `layout` | text, math and sketch layout done in this frame |
| `encode` | building the render scene |
| `gpu_submit` | handing the scene to the GPU and presenting |
| `ui` | egui run and tessellation |

Time in a frame outside every phase is reported as the phase `other`. A phase also records a span of the same name. A phase opened inside another counts for itself only (`encode` inside `ui` is not also `ui`), so the phases of a frame add up to the frame; `other` is what is left. Today the shell opens `ui` around `App::frame`, `gpu_submit` around Vello and the composite, and the app opens `encode` around the composition of the scene; `present` is a span, not a phase, and lands in `other`. The profiler keeps the last 1024 frames (`set_frame_window` changes it) and reports, over that window:

- `frame_ms` (begin to end) and `interval_ms` (start to start): count, mean, p50, p95, p99, max. Percentiles use the nearest-rank method on the exact samples, not on the histogram.
- `phase_ms`: the same summary for each phase and `other`.
- A fixed-bucket histogram of frame time. Bucket upper bounds in ms: 1, 2, 4, 8, 12, 16.7, 25, 33.3, 50, 100, 250, 500, and an overflow bucket.
- Totals since reset: frames, frames over 16.7 ms, frames over 33.3 ms, unexplained frames, frames per redraw reason.
- `fps` over the window, from the first to the last frame start (it includes idle gaps, so an idle app reports a low number; use `frames` deltas for idle checks).

### 2.3 Redraw reasons

`profile::redraw(RedrawReason::X)` records a redraw request. The idle gate calls it; nothing else needs to. The next `begin_frame` takes all reasons requested since the previous one, so a frame can have several.

| Reason | Meaning |
|---|---|
| `startup` | first frame after launch |
| `command` | a command changed document or editor state |
| `gesture` | live gesture update: pan, zoom, drag, resize, draw |
| `hover_change` | hovered element, handle or cursor changed |
| `resize` | window or canvas size or scale factor changed |
| `ui` | egui asked for a repaint (widget interaction, animation) |
| `timer` | a `request_repaint_after` deadline arrived |
| `caret_blink` | the text caret toggles |
| `animation` | a running animation needs its next frame |
| `collab` | a collaborator's edit or presence arrived |
| `autosave` | autosave finished and the UI shows it |
| `load` | async load, paste, drop or image decode finished |
| `surface` | surface lost, resized by the OS, or visible again |
| `debug_panel` | the debug panel (`ui.toggle_debug`) is open and refreshes its numbers, twice a second; the frame meter does not count these frames |
| `other` | no better tag. Each use is a review item; the idle gate counts them |

Adding a reason changes the schema (§3.4): it adds a key to `redraws.by_reason` and `frames.by_reason`.

### 2.4 Counters and gauges

Counters are monotonic. Gauges hold the last value. All are always compiled in.

| Counter | Counts |
|---|---|
| `cache_hit` | a per-element cache served an entry |
| `cache_miss` | a per-element cache had no valid entry and the work ran |
| `elements_encoded` | elements encoded into the scene (fragment built or replayed) |
| `elements_culled` | elements skipped because they are outside the viewport |
| `commands` | commands run through the registry |
| `hit_tests` | point or area hit tests |
| `spatial_queries` | spatial index queries |
| `relayouts` | text or math layouts that were not served from cache |
| `sketch_regens` | hand-drawn paths generated (not from cache) |
| `scene_builds` | whole-scene assemblies |
| `gpu_submits` | GPU submissions |
| `sync_ops_applied` | sync operations applied to the document |
| `redraws_skipped` | redraw requests the idle gate dropped because nothing was dirty |
| `elements_scanned` | elements visited by a pass over the whole scene (a scan, not an index lookup); 0 for an operation whose cost must not grow with the scene |
| `route_expansions` | nodes the elbow router's A* took off its open list; at most `MAX_EXPANSIONS` (16,384) per search, in practice a few hundred |

| Gauge | Reads |
|---|---|
| `elements_total` | elements in the document |
| `elements_visible` | elements that intersect the viewport |
| `cache_entries` | entries held by render caches |

`profile::count_named("name", n)` adds an ad hoc counter for a quick local check. It takes a lock, so keep it out of hot loops. A named counter that matters gets a `Counter` variant.

### 2.5 Work counters in tests

Tests assert on work, never on time. `WorkScope::begin()` opens a window over the work done by the **current thread**; every update also goes to process-wide totals, but the scope reads thread-local cells. Parallel tests do not see each other, and no clock is involved.

```rust
use drafftink_profile::{Counter, RedrawReason, WorkScope};

#[test]
fn panning_does_not_regenerate_sketch_paths() {
    let mut app = test_app_with_elements(1000);
    let work = WorkScope::begin();
    app.execute("view.pan", json!({"dx": 40, "dy": 0}));
    assert_eq!(work.count(Counter::SketchRegens), 0, "{}", work.summary());
    assert_eq!(work.count(Counter::Relayouts), 0);
}

#[test]
fn hover_over_empty_canvas_redraws_nothing() {
    let mut app = test_app_with_elements(1000);
    let work = WorkScope::begin();
    app.pointer_move(5.0, 5.0);
    assert_eq!(work.redraws(), 0, "{}", work.summary());
    assert_eq!(work.redraw_reasons(), vec![]);
}
```

Rules:
- Run the work on the test's thread. Work that an operation hands to other threads is not seen by a `WorkScope`. Assert on it through `profile::snapshot()` in a test that runs alone.
- `summary()` lists every counter that moved. Put it in the assertion message.
- A scenario test that checks "no redraw in this window" also checks `work.count(Counter::RedrawsSkipped)` when the input must have reached the gate.
- Never assert on `frame_ms`, `phase_ms` or span times in a unit test. Those belong to the perf gate (§5).

### 2.5a The live meter

`profile::FrameMeter` is the small meter behind the debug panel (`ui.md` section 5d). The shell records the start and the CPU time of each frame into it, only while the panel is open, and `reading(now)` returns a `FrameReading`: the frames of the last second (the frame rate), their mean and worst time, the latest frame and its age, and the latest 90 frame times. No frame in the last second is `idle`, which is the normal state of a quiet canvas. It takes explicit timestamps and sorts nothing, unlike `FrameRecorder`, whose longer window feeds the reports.

### 2.6 Chrome trace

`profile::chrome_trace_json()` (or `write_chrome_trace(&mut writer)`) returns the recent spans as Chrome trace-event JSON. Open it in `chrome://tracing` or <https://ui.perfetto.dev>.

- One track per thread with complete events (`ph: "X"`), timestamps in microseconds on the process clock.
- A `frames` track with one event per frame in the window, with `args.reasons`.
- A `redraws` track with one instant event per recent redraw request.
- `otherData.dropped` counts finished spans that fell out of the ring (capacity 32,768). The aggregate in `PerfStats.spans` still counts them.

## 3. PerfStats schema

Schema version: 1

`profile::snapshot()` returns a `PerfStats`; `.to_json()` serializes it. The read-only command `perf.stats` returns the same document, and `window.drafftink.perf.snapshot()` returns it on the web. The native and web drivers read this one shape.

### 3.1 Document

```json
{
  "schema": "drafftink.perf",
  "version": 1,
  "compiled": true,
  "enabled": true,
  "uptime_ms": 12034.5,
  "frames": {
    "total": 600, "window": 600,
    "over_16_7ms": 4, "over_33_3ms": 0, "unexplained": 0,
    "fps": 59.8,
    "frame_ms":    {"count": 600, "mean_ms": 6.1, "p50_ms": 5.8, "p95_ms": 9.9, "p99_ms": 14.2, "max_ms": 18.0},
    "interval_ms": {"count": 599, "mean_ms": 16.7, "p50_ms": 16.7, "p95_ms": 17.3, "p99_ms": 19.0, "max_ms": 22.1},
    "phase_ms": {"input": {}, "commands": {}, "layout": {}, "encode": {}, "gpu_submit": {}, "ui": {}, "other": {}},
    "histogram": {"upper_ms": [1.0, 2.0, 4.0, 8.0, 12.0, 16.7, 25.0, 33.3, 50.0, 100.0, 250.0, 500.0],
                  "counts":   [0, 0, 20, 500, 70, 6, 4, 0, 0, 0, 0, 0, 0]},
    "by_reason": {"gesture": 600, "hover_change": 0}
  },
  "redraws": {
    "total": 600,
    "by_reason": {"startup": 1, "command": 0, "gesture": 599},
    "recent": [{"at_ms": 12001.2, "reason": "gesture"}]
  },
  "counters": {"elements_encoded": 52000, "elements_culled": 548000, "cache_hit": 51000, "cache_miss": 1000},
  "gauges": {"elements_total": 10000, "elements_visible": 87, "cache_entries": 1100},
  "spans": [{"name": "encode", "count": 600, "total_ms": 900.1, "self_ms": 820.0, "max_ms": 4.2}],
  "spans_dropped": 0
}
```

(Abbreviated: `phase_ms` entries are full summaries, and `by_reason` and `counters` list every name.)

### 3.2 Field rules

- Times are milliseconds with microsecond resolution (three decimals).
- `frames` and `spans` cover what was recorded while recording was on. `counters`, `gauges` and `redraws` are always on. All reset with `profile::reset()` (`perf.reset`).
- `frames.total`, `over_*`, `unexplained` and `frames.by_reason` count since reset. Everything else in `frames` describes the rolling window (last 1024 frames).
- `frames.unexplained` counts frames that began with no pending redraw reason. It must be 0.
- `frames.by_reason` counts frames per reason (a frame with two reasons counts in both). `redraws.by_reason` counts requests, which can be more than frames because requests coalesce.
- `counters` has every name from §2.4 even at 0, plus any ad hoc names. `gauges`, `redraws.by_reason`, `frames.by_reason` and `phase_ms` list every known name even at 0.
- `spans` lists at most 64 names, most self time first.
- `compiled` is false and `frames`/`spans` are empty when the build has no `enabled` feature.

### 3.3 Metric paths

Budgets and drivers address numbers by path. `PerfStats::metric(path)` resolves them and returns nothing for an unknown path, so a budget on a misspelled metric fails instead of passing.

| Path | Value |
|---|---|
| `frames`, `fps` | frames since reset, frames per second over the window |
| `frame.{mean,p50,p95,p99,max}_ms` | frame time |
| `frame.over_16_7ms`, `frame.over_33_3ms`, `frame.unexplained` | totals since reset |
| `interval.{mean,p50,p95,p99,max}_ms` | frame start to start |
| `phase.<phase>.{mean,p50,p95,p99,max}_ms` | per phase, including `other` |
| `redraws.total`, `redraws.<reason>` | redraw requests |
| `counter.<name>`, `gauge.<name>` | work counters and gauges |
| `span.<name>.{count,total_ms,self_ms,max_ms}` | span aggregates |
| `external.<name>` | only in a `ScenarioReport`: numbers the driver measured (§4.2) |

### 3.4 Schema versioning

- Adding a field, a counter, a gauge, a phase or a redraw reason keeps the version. Readers ignore fields they do not know.
- Renaming or removing a field or name, or changing a unit or meaning, bumps `version` (and this document's "Schema version" line). A reader rejects a document whose `version` is newer than it knows.
- A unit test pins the v1 key set (`schema_v1_key_set_is_pinned`), and `tests/docs.rs` fails if a counter, gauge, reason or phase is missing from this document or the version line is stale.

## 4. Scenarios and reports

### 4.1 Scenarios

Scenarios run inside the app (native `--perf-scenario <name>`, web `window.drafftink.perf.runScenario(name)`) on a scene from the deterministic generator in `testkit` (same output on every platform). Driving the app from inside avoids the flaky synthetic input the baseline hit (baseline §3.1, §3.3).

| Scenario | Scene | Action | What it must show |
|---|---|---|---|
| `idle` | the loaded scene | no input for 10 s | at most 1 redraw, 0 unexplained frames, CPU under 1% of a core |
| `pan` | 1k, 10k, 100k | middle-button pan, a sinusoid at 125 moves per second for 10 s | frame time within budget; `elements_encoded` tracks visible elements, not total; 0 `sketch_regens` and `relayouts` after the first frame |
| `wheel` | 1k, 10k, 100k | 100 wheel ticks per second for 10 s, direction reversed every half second | one frame per tick at most (`redraws.ui` small), otherwise as pan |
| `zoom` | 1k, 10k, 100k | ctrl+wheel, 60 ticks per second for 10 s, direction reversed every half second | same as pan, plus bounded cache growth |
| `drag` | 1k, 10k, 100k | drag a selection of 100 elements (`e0` to `e99`, zoom 0.5) with the left button for 10 s | commit cost independent of scene size; only the dragged elements re-encode |
| `hover` | 1k, 10k, 100k | 1000 pointer moves in 8 s along a Lissajous curve over the canvas; the hover state changes on about 5% of them | redraws equal the number of hover changes; `redraws_skipped` equals the rest |

A scenario key is `name@size`, with size `1k`, `10k` or `100k`: `pan@10k`. `idle` has no size. `ScenarioKey` in `drafftink-profile` parses and prints it, so both drivers read it the same way.

The scene is `drafftink_scene::scenegen::generate(size, 1)`, installed by the app itself (`App::scene_loader`, `SceneInstall`: a hook next to the commands, like the test rig's `load_generated`, with no history). The input is real: X11 XTEST events on a private Xvfb for the desktop app, Chromium DevTools mouse, wheel and key events for the web app (the page sees trusted events). Only the scene and the camera are installed from inside. The driver starts a window when the app has drawn the scene and been quiet for a second, and the profiler is reset at that moment.

### 4.2 ScenarioReport

A driver wraps a `PerfStats` snapshot with what only it can measure:

```json
{
  "schema": "drafftink.perf.scenario",
  "version": 1,
  "scenario": "pan",
  "scene_size": 10000,
  "meta": {"platform": "web", "adapter": "nvidia/lovelace", "commit": "abc1234", "browser": "chromium 1208"},
  "external": {"cpu_pct": 41.5, "wasm_bytes": 3210000},
  "stats": { "...": "a PerfStats" }
}
```

- `external.cpu_pct` is process CPU in percent of one core over the window. Native: `utime + stime` of `/proc/self/stat`, read by the app at the start and the end of the window (the app is the one process that knows both moments). Web: the sum over the Chromium process tree, read by the driver from `/proc`; it includes the cost of Chromium receiving the injected input, about 25 to 30 % of a core at 125 events per second (`hover` shows it: almost nothing is drawn).
- `meta.platform` is `native` or `web`. `meta.adapter` records the GPU adapter, because absolute frame times are only comparable on the same adapter.
- Other `external` names are free (`wasm_bytes`, `gpu_ms` where timestamp queries exist). A budget may reference any `external.<name>`. The drivers write `window_s`, `input_events`, `first_frame_ms` (native: process start to the first frame), `scene_to_frame_ms` (the scene is handed over to the first frame that shows it) and `startup_*_ms` (native: spans of start-up; web: the marks of the page and of `start()`, in ms since navigation).
- `meta.driver` is `app` for a run of the real app. Such a report is budgeted per platform: the key in the budgets file is `native/pan@10k` or `web/pan@10k` (`ScenarioReport::budget_key`). The work-counter tests write plain keys.

## 5. Budgets and the perf gate

### 5.1 `perf/budgets.json`

```json
{
  "version": 1,
  "scenarios": {
    "idle": {
      "redraws.total":        {"max": 1,   "note": "idle means idle"},
      "frame.unexplained":    {"max": 0},
      "external.cpu_pct":     {"max": 1.0}
    },
    "hover@10k": {
      "redraws.other":              {"max": 0},
      "counter.redraws_skipped":    {"min": 900}
    },
    "pan@10k": {
      "frame.p99_ms":               {"max": 16.7, "note": "60 fps on the reference GPU"},
      "phase.encode.p95_ms":        {"max": 4.0},
      "counter.sketch_regens":      {"max": 0}
    }
  }
}
```

- Keys: scenario key (§4.1), then a metric path (§3.3), then a limit with `max` and/or `min` (inclusive) and an optional `note`. A run of the real app has its own keys, `native/<key>` and `web/<key>`: a frame of the real app does more than a gesture of the test rig, and timing limits belong to one platform. The plain keys hold the limits of the work-counter tests.
- A budget on a metric the report lacks is a violation. A report whose scenario has no entry produces a warning in the gate output ("unbudgeted scenario"), not a failure.
- `Budgets::from_json` rejects an unknown `version`, a limit with neither `max` nor `min`, non-finite limits, and `min` above `max`.
- Deterministic metrics (`counter.*`, `gauge.*`, `redraws.*`, `frame.unexplained`, `span.*.count`) give the same value on every machine and gate everywhere. Timing metrics (`frame.*`, `phase.*`, `interval.*`, `fps`, span times, `external.*`) depend on the hardware: they gate only on the reference runner with a real GPU, and run in report-only mode on software adapters (llvmpipe, SwiftShader). `Budgets::check_filtered(report, false)` does this split.
- Tighten a limit when a fix lands, in the same commit, and say so in the commit message. Loosening a limit needs a reason in `note`.

### 5.2 `cargo xtask perf`

`cargo xtask perf` is the gate. Its checks are written in `perf/README.md`; in short:

1. **Work counters** (default, part of `cargo xtask ci`). Runs `tests/perf_work.rs` of `doc`, `canvas`, `edit`, `render` and `testkit` in release mode at 1k, 10k and 100k elements. The tests assert on counted work (`elements_scanned`, `cache_miss`, `redraws`, the store's `WorkStats`). The tests that drive a scenario of §4.1 (`idle`, `hover`, `drag`, `pan`, `zoom`) also write a `ScenarioReport` to `target/perf/work` (`ScenarioReport::save_if_requested`, `DRAFFTINK_PERF_OUT`), and the gate checks the reports against the deterministic limits of `perf/budgets.json`. A budgeted scenario without a report fails the gate. A debug `cargo test` runs the same tests at 1k and 10k.
2. `cargo xtask perf --idle` runs only the idle and hover tests.
3. `cargo xtask perf --bench` runs a curated set of criterion benches with short measurement times, writes the medians as the report `bench` and checks the `bench` limits of `perf/budgets.json` (about ten times the measured median). Timing on a busy machine is noisy, so this is not part of `ci`; run it alone.

The budget arithmetic is `Budgets::check_filtered`; xtask uses no workspace crate, so it runs the `perf_check` example of this crate (`cargo run -p drafftink-profile --example perf_check`).

4. `cargo xtask perf --app [native|web|all]` runs the scenario drivers of §5.3 against the real app and checks their reports, timing included, against the `native/...` or `web/...` entries of `perf/budgets.json`. It is not part of `ci`: it needs a GPU and Xvfb. Run it as a separate CI job on the reference runner (a machine with a GPU, `Xvfb`, Chromium for the web, and the Python environment of `scripts/perf_scenarios.py`: Xlib, Pillow, Playwright; `PERF_PY` names the interpreter). A software adapter (llvmpipe, SwiftShader) runs the scenarios but its timing numbers mean nothing: check those reports without `--timing`. The web run builds what it needs: the web app with `--features scenarios` (the generated scenes are not part of the shipped wasm) into `target/perf/web-dist`, which is not `dist/`, and `drafftink-server`. `--no-build` reuses the last build. All outputs follow `CARGO_TARGET_DIR` when it is set: the benches read `$CARGO_TARGET_DIR/criterion`, the drivers get the binaries from `$CARGO_TARGET_DIR/release`, and the reports go to `$CARGO_TARGET_DIR/perf`.

### 5.3 What the drivers do

`scripts/perf_scenarios.py {native|web} KEY...` runs scenarios and writes one `ScenarioReport` per key (`target/perf/app-native/pan@10k.json`). Both platforms use the same input scripts and the same report.

- **Native:** starts `drafftink --perf-scenario <key> --perf-out <path>` under a private Xvfb (NVIDIA Vulkan). The app installs the scene, waits until it has drawn it and been quiet for a second, resets the profiler, prints `PERF-READY` and measures. The driver sends XTEST events (pointer motion, buttons, wheel buttons, Control) and then a line on the app's stdin, which ends the window: the app writes the report (stats, `cpu_pct` of its own process) and closes the window (`Waker::exit`). `--perf-secs N` makes the window self-timed instead (an `idle` run needs no driver).
- **Web:** serves `dist/` with `drafftink-headless serve`, opens it in headed Chromium on Xvfb, calls `await window.drafftink.runScenario(key)` (installs the scene, waits for quiet, resets the profiler) and sends Playwright mouse, wheel and key events. `window.drafftink.scenarioReport()` returns the report; the driver adds the CPU of the Chromium process tree, the adapter and the start-up marks.
- Both enable recording (`DRAFFTINK_PERF` is implied by `--perf-scenario`; `runScenario` turns it on) and reset the profiler before the window, so a report covers only that scenario.

## 6. Redraw rules, checked

| Rule (§1) | Check |
|---|---|
| A redraw needs a dirty reason | `frame.unexplained` is 0 in every scenario; `redraws.other` is budgeted per scenario |
| Idle means idle | `idle`: `redraws.total` at most 1, CPU under 1% |
| Hover redraws only on change | `hover@*`: redraws equal hover changes; unit tests with `WorkScope` |
| Repaint tied to pending work | after the work ends, `idle` shows no `timer`, `animation` or `ui` redraws |
| Deadlines honoured | unit test: `request_repaint_after` yields one `timer` frame at the deadline |
| Wake-ups explicit | collab, autosave, load tests assert the reason of the frame they cause |

## 7. Baseline: the "before" column

All numbers are from `research/baseline-perf.md` on the research box (Ubuntu x86_64, 64 cores, NVIDIA L40S), old app at commit `8ce40ab`. The rewrite is measured against them. Items marked n/m were not measured.

### 7.1 App behaviour

| Metric | Before (`8ce40ab`) | Rewrite budget |
|---|---|---|
| Native idle CPU (5 s and 10 s windows) | 0.3 - 0.6% of a core | under 1%, 0 redraws |
| Web idle CPU, whole Chromium tree (NVIDIA / SwiftShader) | 0.6% / 0.9%, 0 app frames/s | same |
| Native wheel pan, ~113 events/s | 52 - 65% of a core | within budget, tied to frames drawn |
| Native hover over canvas, ~125 moves/s, nothing changing | 53 - 68% of a core (a full frame per move) | 0 redraws when hover state is unchanged |
| Web pan, 100 events/s (NVIDIA) | 94.6% CPU, 33 fps | n/m |
| Web ctrl+wheel zoom, 60 events/s (NVIDIA) | 106% CPU, 40 fps | n/m |
| Web middle-button drag, 125 moves/s (NVIDIA) | 94.5% CPU, 32 fps | n/m |
| Web hand-tool drag, 125 moves/s (NVIDIA) | 92.6% CPU, 33 fps | n/m |
| Web pan, SwiftShader | 876% CPU, 6 fps | report only |
| Web pan frame cost (NVIDIA, wasm main thread) | ~1.8 ms per frame, ~3 `queue.submit` per frame | fewer submits |
| Web startup, NVIDIA | first app frame 507 ms, first visible ~1.0 s | n/m |
| Frame statistics available | none (no timing, no counters) | `PerfStats` |

The ~30 fps ceiling on the web was most likely an Xvfb present artifact (`SwapBuffers` ~18 ms per frame), so web fps figures are comparable only on the same display setup.

### 7.2 CPU cost by scene size (Rust microbenchmarks, shipping `opt-level=z`)

| Operation | 1,000 | 10,000 | 100,000 |
|---|---|---|---|
| `shapes_at_point` (hit test, linear scan) | 60 us | 0.70 ms | 9.1 ms |
| `shapes_in_rect` 800x600 (marquee) | 66 us | 0.77 ms | 9.6 ms |
| Smart-guide input: collect bounds and detect | 157 us | 1.67 ms | 17.0 ms |
| `push_undo` (document snapshot clone) | 52 us | 1.2 ms | 17.0 ms |
| `build_scene`, warm cache, almost all culled | 0.36 ms | 1.0 ms | 10.4 ms |
| `build_scene`, warm cache, everything visible | 1.6 ms | 15.1 ms | n/m |
| `to_json` / `from_json` | 3.4 / 1.8 ms | 34 / 18 ms | 382 / 199 ms |

The same benches with `opt-level=3` are 1.3x to 2x faster (10k: hit test 0.41 ms, `build_scene` culled 0.59 ms, fit-all 8.9 ms).

Sync (Loro, through the old `CollaborationManager`):

| Operation | 1,000 | 10,000 |
|---|---|---|
| `sync_to_crdt` (runs on every local edit in a room) | 64 ms | 427 ms |
| `sync_from_crdt` (runs on every received message) | 2.05 s | not finished (over 80 s) |
| `update_shape` for one shape | 40 us | 41 us |
| Apply one remote single-shape update | 103 us | 103 us |
| Snapshot export / import | 2.5 / 15 ms | 33 - 35 / 207 ms |

### 7.3 Size and tooling cost

| Item | Before |
|---|---|
| Wasm, shipped | 10.33 MB raw, 4.66 MB gzip, 3.46 MB brotli |
| JS glue | 118.6 KB |
| Native release binary | 24.4 MB |
| Release profile | `opt-level="z"` on native and wasm |
| Frame statistics, spans, counters | none existed |

### 7.4 Cost of the tooling itself

`cargo bench -p drafftink-profile --bench span_overhead` on the research box (Ubuntu x86_64, release profile, load average about 5, so the figures are approximate):

| Call | Time |
|---|---|
| `span` with recording off | 0.56 ns |
| `phase` with recording off | 3.1 ns |
| `span` with recording on | 114 ns |
| three nested spans, recording on | 337 ns |
| `phase` with recording on | 212 ns |
| `count` (always on) | 1.8 ns |
| `redraw` (always on, takes a lock for the recent log) | 38 ns |
| `WorkScope::begin` | 17 ns |
| `begin_frame` + `end_frame`, recording on | 77 ns |
| `snapshot().to_json()` | 25 us |

At 60 fps with 20 spans per frame, recording costs about 2.3 us per frame (0.014% of a frame).

### 7.5 Targets that come from the baseline

- Idle: under 1% of a core and 0 renders. Hover with no change: 0 renders.
- Pan at 10k visible elements: p99 frame under 16.7 ms on the reference GPU.
- `build_scene` independent of off-screen element count (spatial index).
- Local commit and remote-op apply under 2 ms at 10k elements, proportional to what changed.
- No UI-thread stall over 50 ms from collaboration at any scene size.
- Wasm brotli size not larger than 3.5 MB.

### 7.6 Measurement rules

- Profile first. Time `main` against the branch with alternating runs. Check the load average before a run and wait while the machine is busy.
- Report medians with the range, the machine, the adapter and the cases.
- Prove that the output is unchanged (scene golden, pixel probe) before claiming a speed-up.
- Use exact bounds for culling, never a coarse box.
- Prefer a work-counter assertion (§2.5) to a timing when the claim is "this no longer happens".

### 7.7 Fixes of the first perf pass

Release build, the research box (load average 8 to 20), medians. "Frame" is one `gesture.update` of `element.move` of one rectangle with a bound arrow, as the engine runs it for every pointer move of a drag. Work counters (`elements_scanned`) are in `crates/*/tests/perf_work.rs`.

| What | Before | After |
|---|---|---|
| Move frame, 1,000 / 10,000 / 100,000 elements | 105 us / 1.15 ms / 37 ms | 15 us / 16 us / 14 us |
| `gesture.end` of the move, same sizes | 0.38 ms / 2.7 ms / 85 ms | 80 us / 84 us / 81 us |
| `element.move` with smart guides, `arrow.move_point`, hover while placing an arrow end | scanned the scene once or twice per call | 0 elements scanned |
| Interaction's spatial index after an edit | one pass over the scene per revision | the touched elements (`TabDoc::scene_index`) |
| `LoroStore::from_document`, 100,000 elements | 5.2 s | 2.7 s |
| Load a generated scene into a `TabDoc` (engine), 1,000 / 10,000 / 100,000 | 44 ms / 0.49 s / 5.5 s | 26 ms / 0.29 s / 3.2 s |
| Open a Loro snapshot, 100,000 elements | 1.3 to 1.4 s | unchanged |
| Compose, everything visible, level of detail, warm, 10,000 / 100,000 | 7.9 ms / 49.6 ms | 3.8 ms / 22.6 ms |

What changed. The reverse indexes of the store (bound arrows, group members, frame children, labels, and now the frames) answer the lookups of `transform`, frame membership, label refresh and `update_bound_arrows` for a store and for a transaction that has not written yet (`SceneRead::derived_index`); a transaction that has written falls back to a scan, which `elements_scanned` makes visible. `find_hovered_among`, `plan_end_drag_among`, `drag_end_among` and `snap_candidates_among` take their candidates from the tab's spatial index. The tab's index follows the ids its commits touched. Loro hands every commit to an attached undo manager, which doubled the cost of the one big load commit; `from_document` attaches it after the load.

A move or a turn no longer copies elements: `transform::apply` builds a new `Transform` per element and writes only that, so a translate of a 1000-point freehand copies no point vector (the copy is made only for a map of the points or a font scale, and for an arrow whose fixed point is carried).

What did not change, and why. Opening a snapshot is bound by Loro (its map iteration is 0.4 s of 1.3 s) and our field decode (0.6 s, mostly `read_element` building a JSON value per element); a typed decoder would save about 0.3 s and give up the rule that the stored form follows the model's serde output. Writing the 100,000 elements into Loro is 770,000 field inserts at the cost the CRDT library has (1.5 s measured with Loro alone). A fully zoomed-out frame at 100,000 elements is now 22.6 ms: the loop over the candidates remains. The order was at first rebuilt after every edit and gesture frame (one pass over the scene); `render::zorder` now patches it from the touched ids (`docs/render.md` section 8): a drag frame with all 100,000 elements in view composes in 23 to 25 ms where it took 60 to 68 ms, and `elements_scanned` is 0 for it.

### 7.8 The scenario drivers and the second perf pass

The drivers of §5.3 run the real app, so they show what the work-counter tests cannot: the cost of a frame with the UI, the display server and the GPU driver in it. Numbers are from the research box (NVIDIA L40S, 64 threads, Xvfb 1400x900, window 1280x800, release build, load average 3 to 20), medians of three alternating runs of 8 s unless said otherwise. "Before" is `main` at `7e97970` with the drivers added and nothing else; "after" is this branch. Frames and CPU come from the report (`stats.frames.total`, `external.cpu_pct`).

| Scenario (events in the window) | Before | After |
|---|---|---|
| `wheel@10k`, 800 ticks: frames | 1922 (2.4 per tick, 1920 of them reason `ui`) | 531 (at most one per tick, 2 of reason `ui`) |
| `wheel@10k`: CPU of one core | 55% | 33% |
| `zoom@10k`, 480 ticks: frames | 2000 (4.2 per tick) | 482 (1.0 per tick) |
| `zoom@10k`: CPU | 54% | 30% |
| `pan@10k`, 1000 moves: frames | 1046 (1.05 per move) | 529 (paced to 66 per second) |
| `pan@10k`, `pan@100k`: CPU | 34%, 47% | 33%, 43% |
| `pan@10k`: CPU without the X11 present (estimate) | 13% | 1% to 3% |
| `drag@100k`, 100 elements: frame time p50, p99 | 26.7 ms, 35.4 ms | 8.9 ms, 9.7 ms |
| `drag@100k`: worst frame (first drag frame, 10 s windows) | 300 to 811 ms | 56 ms |
| `drag@100k`: CPU | 89% | 59% |
| `hover@10k`, 1000 moves: CPU, frames | 6.5%, 78 (one per hover change) | 6.6%, 78 |
| Web `zoom@100k`: frame p50, frames in 10 s, scene passes | 66 ms, 148, 92 passes of 100,000 elements | 1.6 ms, 271, 0 |
| Web `drag@N`: scene passes | 3 of N | 0 |
| `idle` (native, web) | 0 frames, 0.5% (web 1.0% to 2.5%) | same |

What the drivers showed, and what changed for each:

- **Wheel: two frames per tick, three to four with ctrl.** egui smooths a scroll over several frames and asks for a repaint for each. The shell now offers wheel, trackpad scroll and pinch over the canvas to the app first (`ShellEvent::Wheel`, `docs/shell.md` §2); egui never sees what the app takes. One frame per tick at most, reason `gesture`; `redraws.ui` is 2.
- **Frame pacing.** Native frames start at least 0.9 refresh periods apart (`RepaintState::set_min_interval`). A 125 Hz mouse used to draw 130 frames per second; now 66. The app applies every event to the camera at once, so only the drawing is coalesced. Under Xvfb this does not lower the CPU of `pan` (next point), but it halves the frames, and with them the Vello runs and the GPU work, and it keeps a 1000 Hz pen at the display rate.
- **Where the CPU of a pan goes.** Spans (`present`, `acquire_surface`, `egui_input`, `tessellate`, `ui`, `gpu_submit`, `encode`) split a frame. In `pan@10k` before: 2.8 ms of 3.8 ms are `present`, the NVIDIA driver's copy to the X server and wait; the app's own work is 1.0 ms (egui 0.3, Vello submit 0.2, `encode` 0.05, tessellation 0.08). Per second, `present` costs about 33% of a core whether 65 or 130 frames are drawn, so it is a property of the display path, not of the app; `perf` could not be used (kernel `perf_event_paranoid=4`), the spans are the profile. `external.cpu_pct_ex_present` subtracts it (an estimate: a wall-clock span against CPU time). The encode side is as the plan says: a pan frame builds 30 to 50 fragments in 10 s (`cache_miss`, the elements that come into view), replays 60 cached fragments per frame (`elements_encoded`), and `encode` is 0.05 to 0.1 ms. Nothing is regenerated. A camera move re-composites cached fragments; what remains is Vello drawing the scene again, which is GPU work.
- **100k elements.** Pan, wheel and zoom frames cost the same at 1k, 10k and 100k (p50 7.2 / 7.0 / 7.7 ms under pacing, `encode` below 0.2 ms at all three). Three things scaled with the scene and are fixed:
  1. The app's paint index compared every element's version after each edit (`CanvasView::sync_index`: 32 ms per drag frame at 100k). It now re-reads the ids the commits touched (`TabDoc::take_touched`, the same feed as the spatial index). A drag frame at 100k: 31.9 ms before, 4.1 ms after.
  2. `style::read` and `style::apply` built the label map of the whole scene for every selection (the properties panel asks on every selection change): 0.8 s at 100k when a drag began. They look up the labels of the selected ids through the derived index (`text_bind::labels_of`).
  3. The availability checks of `group.ungroup`, `group.create`, `arrange.align` and `arrange.distribute` built the relations of the whole scene (`Relations::build`). In the browser that was 47 ms per frame while zooming at 100k. They use `Reverse::of`, the index when there is one.
  4. The one-shot commands (`group.create`, `group.ungroup`, `arrange.*`, `frame.*`, `element.delete`, `element.duplicate`, `element.lock`, `clipboard.*`, `library.add`, `selection.navigate`) walked the scene once to three times each (`Relations::build`, `arrows_bound_to` after a write). They read first and write after, from the index (`Reverse::of`; `plan_delete`, `plan_add`, `plan_leavers` hold what a write would take away), and `structure_commands_on_a_few_elements_scan_nothing` counts 0 passes at 1k, 10k and 100k. Still O(scene) by design, without a counter: `arrange.z_order` (it lists every element), `clipboard.copy` and `copy_as` (`snapshot()` of the document), `frame` capture after a resize (`free_candidates`), `element.restore`, `group_name`.
  The spatial index of a big document is built in the frame after the load (`App::frame`), not by the first hover (0.7 s at 100k).
- **Autosave inside a drag.** A drag at 100k showed one frame of 0.8 s: the autosave wrote the whole document (`snapshot().to_json()`) one second after the drag began. A write now waits for the running gesture (`Session::workspace_tick`). The write itself was 0.8 s at 100k on the UI thread after the gesture; section 7.10 has what replaced it.
- **Phases.** `encode` is now measured in the app, and a phase inside another counts for itself only, so `ui` no longer includes the scene composition and the phases add up to the frame.
- **The drag driver scaled its selection.** `drag` showed 0.7 to 1 text or formula layouts per frame (`relayouts` 671 in `drag@10k`), 12 cache misses and 5 sketches per frame. The driver pressed 4 points from the top left corner of the selection, which is a resize handle, so it scaled 100 elements. It now presses the middle of `e0`: `relayouts` 0 to 1, `cache_miss` 260 to 480, `sketch_regens` 39 to 67 in 10 s (`drag@1k`, `10k`, `100k`). A move replayed its layouts all along (`the_drag_scenario_moves_its_selection_and_lays_nothing_out_again` presses where the driver does).

Start-up. Native: the first frame is on screen 320 to 340 ms after the process started (`external.first_frame_ms`); 265 to 290 ms of it is the Vulkan instance, adapter and device (`startup_gpu_ms`), the session and the workspace restore are 6 ms (`startup_app_ms`, `startup_restore_ms`). The driver and the NVIDIA ICD dominate, so there is nothing to gain in our code. Web (headed Chromium on Xvfb, localhost, the precompressed `dist/` of `build-web.sh`; marks in ms since navigation, medians of 8 runs, range in brackets): the page script is loaded at 55 ms [37 to 183], the wasm is instantiated at 160 ms [134 to 289], `start()` is entered at 165 ms, the platform (IndexedDB opened and read) is ready 35 to 40 ms later, `App::new` (session, UI, workspace restore) takes 80 ms, `run()` returns at 305 ms, and the first frame is on screen at 590 ms [556 to 1241; the two slow runs were on a loaded machine]. The 285 ms from `run()` to the first frame are the WebGPU adapter and device, the pipelines and the first frame. Tried and dropped: asking for the adapter from the page script before the module loads (`navigator.gpu.requestAdapter()`), to wake the GPU process while the wasm downloads: no change (590 ms against 590 ms, 8 runs each). The page marks are `window.__marks` and `performance.mark` entries `drafftink:start`, `drafftink:platform`, `drafftink:app`; the driver copies them into the report as `external.startup_*_ms`. The old app's 507 ms to the first app frame and 1.0 s to the first visible one (§7.1) were measured with a cold profile and another harness, so the two are not a like-for-like comparison.

### 7.9 Loading a large scene into a tab document

`TabDoc::from_document` of the generated scene, 100,000 elements (research box, load average 12; the pieces are
timed inside `Core::write`, 770,000 field inserts in all):

| Piece | Release | Share |
|---|---|---|
| `LoroStore::from_document` (the CRDT copy) | 2.7 s | 90 % |
| of it: `LoroMap::insert` of the fields | 1.7 s | 57 % |
| of it: `codec::flatten` (`serde_json::to_value` of the style and the key strings) | 0.49 s | 16 % |
| of it: `ensure_mergeable_map` (one container per element) | 0.29 s | 10 % |
| of it: Loro's own commit, the view commit | 0.13 s, 0.08 s | 7 % |
| `MemoryStore::from_document` (the working copy) | 0.3 s | 10 % |
| Dropping the tab afterwards | 0.3 s | |

The cost is the CRDT: Loro takes about 2 microseconds per field insert, and an element has seven or eight fields. Reading
a value before the insert (`map.get`) is not the cost: leaving it out for new elements saved 0.05 s. Not done, with their
gains: flatten on several threads (0.4 s, native only), a typed style writer (0.3 s, and the stored form would no longer be
what serde writes), a document opened without its CRDT copy until the first edit (3 s, a different design). A debug test
build took 31.6 s because Loro was not optimised (29 s of it); `[profile.dev.package.loro*]` is now `opt-level = 3`, as for
`brotli`, and a debug load of 100,000 is 9 s. Loading is still not a test to run at 100k in debug (`sizes()` in the
work-counter tests stops at 10k there).

### 7.10 Autosave in pieces

The autosave of a document of 100,000 elements (generated scene, release, the research box): 63 ms to copy the document, 20 ms
to check it, 115 ms to write 45 MB of JSON, and then the storage port: a native `put` ends with an `fsync` (a 30 MB file
with `fsync` took 4 s on this machine's disk), and the web cache copied the bytes three times. All of it was one call of
`workspace_tick` on the UI thread.

Now the write is a job (`cmd/tab/workspace_save.rs`). `MemoryStore` keeps its elements behind `Arc` in both lists (the map
by id and the z-ordered tree), so `share_document` is a walk of one tree that clones pointers (7 ms at 100k; the copy was
63 ms). `Validator` checks in steps (the rules of `validate_document`, 25,000 elements a step; the last step, which needs every
element, is 5 ms at 100k), `JsonJob` writes about 2 MB of text a step (the text of `Document::to_json`, byte for byte), and the
finished text goes to `Storage::put_later`. The native store hands it to a writer thread; the web cache keeps the vector
without a copy. The job pauses for a running gesture, as before.

| At 100,000 elements | Before | After |
|---|---|---|
| Longest call of `workspace_tick` | about 800 ms (with the disk) | 8.2 ms |
| Calls for one write | 1 | 28 (one is the 7 ms take, three are the check, 24 write text) |
| Longest stall of the UI thread by the disk | the whole `fsync` | none: the writer thread |
| Work counted by `autosave_elements` per call | all | at most 4,700 |

The tests count the work (`the_autosave_of_a_big_document_is_written_in_pieces_and_is_the_document`: every element is written
once, at most 8,000 per call, the text is the text of `to_json` at 1k, 10k and 100k; `an_autosave_under_way_waits_for_a_gesture_...`:
no piece during a drag, and the text is the document at the start of the write, not the drag). Times are not asserted.
Not done: the check of the image files (`FileId::of`, a hash of every picture) is one step, and a single huge element (a
freehand path of a million points) is one step of its own.

### 7.11 Present mode, and hover on the web

**Present mode.** `PresentSurface::configure` used to take the first mode the driver lists (`get_default_config`). That order is the
driver's: Mesa lists `Immediate` or `Mailbox` before `Fifo` on some setups, which means tearing and frames as fast as they are asked for,
or extra images and GPU work for frames nobody sees. The default is now `Fifo` on every platform (`choose_present_mode`, tested);
`DRAFFTINK_PRESENT_MODE=fifo|mailbox|immediate|auto_vsync|auto_no_vsync` still overrides it for measurements when the surface supports
the mode, and the chosen and the supported modes are logged at `info` (`drafftink_gpu::gpu`).

Measured (NVIDIA L40S, Xvfb 1400x900, release build, `pan@10k`, 8 s, three alternating runs per mode, load average 20 to 35; CPU of
the process in % of one core, median [range]):

| Mode | CPU | Frames | `present` per frame | p50 frame |
|---|---|---|---|---|
| driver's first (it was `Fifo`) | 34.1 [32.2 to 40.9] | 521 to 529 | 4.8 to 5.8 ms | 7.2 to 8.7 ms |
| `fifo` | 30.1 [24.9 to 36.9] | 521 to 527 | 3.7 to 5.3 ms | 5.0 to 7.1 ms |
| `immediate` | 31.4 [30.9 to 35.7] | 521 to 529 | 4.6 to 5.8 ms | 5.0 to 8.2 ms |
| `mailbox` | not offered: the NVIDIA driver lists only `[Fifo, Immediate]` on this display; the override falls back to `Fifo` | | | |

The modes are equal within the spread of a loaded machine, so there is no number that favours `Immediate`, and the 70 to 85% of CPU in
`present` (section 7.8) is the X11 copy, not the mode. `Mailbox` could not be measured here. Reasons for `Fifo`: every Vulkan, Metal and
DX12 surface has it, it does not tear, the app already paces its own frames (`RepaintState::set_min_interval`), and a frame that is
replaced before it is shown (`Mailbox`) costs a Vello run for nothing. **Web:** the browser's WebGPU canvas offers `Fifo` only (the
console line says `present mode: Fifo (supported: [Fifo])`); nothing to choose. Re-measure `Mailbox` on a Mesa or Wayland machine before
changing the default.

**Hover on the web.** The backlog said that hovering over empty canvas costs about 9% CPU in Chromium with 0 frames drawn. Measured on the
same machine (headed Chromium on Xvfb, `scripts/perf_scenarios.py` for the scene, 125 CDP mouse moves per second for 8 s, per-thread CPU
from `/proc`, CDP `Performance.getMetrics`; load average 15 to 35, so absolute numbers are inflated and only the comparison holds):

| Page | CPU, all Chromium processes | Page script per move |
|---|---|---|
| `about:blank` | 5.6 to 10 | 0 |
| the app, pointer over empty canvas, 0 frames | 6.7 to 10 | 150 to 370 us (two `pointermove` listeners, ours and winit's, 35 to 60 us each in `performance.now()` timing) |
| the app, pointer over shapes (hover changes, 16 to 36 frames in 8 s) | 12 to 22 | |

The cost is the browser's: a blank page with the same input costs 5.6% to 10%, mostly the browser process and the renderer's compositor
and input threads, plus Xvfb. What is ours is 1 to 3 points: the page script. One avoidable part was in our listener: it asked the browser for
`getCoalescedEvents()` on every hover move (a JS call and an array of event objects), although only a move with contact uses those points
(`PointerPhase::reads_coalesced`). It is skipped now. The effect is below the noise of a loaded machine (script per move 150 to 250 us with
the call stubbed out against 150 to 370 us), so nothing is claimed beyond "one JS call and one allocation fewer per hover move". Not
changed: the second listener (winit's) and `getBoundingClientRect` (stubbing it to a cached value changed nothing measurable).

The guard is `scripts/web_check.py --app`: 125 moves per second over empty canvas for 5 s must draw 0 frames, and the CPU of the browser
tree is printed (7.8% in the run above, 302 moves, load average 330). `web/hover@N` in `perf/budgets.json` already bounds redraws and
`counter.elements_scanned`; its CPU limit is generous because the number is mostly the input injection.

### 7.12 Elbow arrows around other shapes

An elbow arrow with a bound end goes around the shapes between its ends (`docs/commands.md`, `arrow.set_type`). The work is bounded in three ways, and `crates/edit/tests/perf_work.rs` asserts each on 1,000 and 10,000 elements (and 100,000 in release builds):

- The shapes come from the box index of the store (`DerivedIndex::ids_in_rect`, one R-tree query for the corridor of the arrow), so `elements_scanned` is 0 and the cost does not grow with the scene. A transaction that has written has no index; it is scanned once per call, not once per arrow (`ObstacleFinder`).
- A search starts with the 24 nearest boxes (`MAX_OBSTACLES`) and takes in a box that its route crosses, up to 48 in all. A route that would need more falls back to the route without obstacles. A search stops after 16,384 nodes (`MAX_EXPANSIONS`; counter `route_expansions`). A five-shape detour at 1,000 and at 10,000 elements takes 182 expansions.
- When a shape moves, only the unpinned elbow arrows whose box touches the shape's new box or the box it left (grown by 24 units) are routed again, found with the same index. In `perf_work` that is the one arrow that passed the shape plus the arrow bound to it.

`cargo bench -p drafftink-edit --bench arrows_binding` (release, criterion median, 1,000 shapes in 40 columns, results in `crates/edit/benches/results.md`):

| What | Time |
|---|---|
| One arrow routed again, two shapes in the way | 20 us |
| One arrow from the first to the last shape of a row, 38 shapes in the way (the search is repeated with the shapes it crosses) | 0.63 ms |
| A shape moved out of an arrow's way and back, with the arrow routed again each time, commit and pass | 56 us for the two |
| `update_bound_arrows`, 100 moved shapes, every arrow elbow, in a transaction that has written (one scan) | 1.2 ms (0.55 ms before the router looked at other shapes) |

The store keeps one more R-tree for this (the boxes of the live elements, in `DerivedIndex`), updated from every change set: one removal and one insertion per element that moved.

## 8. Wasm size

The web app (`apps/drafftink-web`) is the whole editor in one module. `cargo xtask size` builds it as a release is built
(`scripts/build-web.sh`: `release-wasm`, wasm-bindgen, wasm-opt), measures the module and the font files that the page loads next to it,
and checks `perf/budgets.json` (section `size`: `wasm.opt_bytes`, `wasm.gzip_bytes`, `wasm.brotli_bytes`, `fonts.brotli_bytes`). On a
failure it prints the biggest items of the unstripped module (`twiggy top`) and says how to go on. It runs in `cargo xtask ci` (about a
minute with a warm cache) and as its own CI job. `--no-build` checks the last build; `--wasm FILE` measures a finished module.

### 8.1 Result (2026-11-08, same machine, MiB)

| | Before | After |
|---|---|---|
| Module as wasm-bindgen writes it | 28.43 | 24.62 |
| Module after wasm-opt (what the server sends) | 17.62 | **11.79** |
| gzip -9 | 6.51 | 4.36 |
| brotli -11 | 4.48 | **3.06** (3.12 at `17667fd`, 3,267,207 bytes, after later commits) |
| Fonts as separate files, brotli (loaded on demand) | in the module | 1.44 |
| Brotli sent for the first visit of the welcome page (module, Ubuntu Light, STIX) | 4.48 | 3.57 (3.64 at `17667fd`: 3,816,214 bytes with the JS glue, 9.7% above the old app's 3,478,229) |
| Old app, for reference (module, brotli) | 3.46 | |
| Start: navigation to first frame, cold / service worker warm (median, ms) | 676 / 439 | 638 / 394 |
| Frame of the pan scenario (1000 rectangles, 200 `view.pan`), mean / p95 (ms) | 3.2 to 4.2 / 4.4 to 4.9 | 3.5 to 3.9 / 4.3 to 4.5 |

Start and frame times come from `scripts/web_startup.py` (headed Chromium on Xvfb, NVIDIA L40S, localhost; ten cold loads per build, the
two builds alternated). They are equal within the noise of the machine (a run varies by 10% to 20%), and the budget "wasm brotli not above 3.5 MB"
of section 7.5 is met for the module alone. The first visit, with the two fonts the welcome page loads and the JS glue, is above it (3.64 MiB at `17667fd`); `docs/deviations.md` D17 records this. The budget file limits the module and the fonts separately and has no limit for the sum.

### 8.2 What was tried

Brotli is the number that counts for the first load; "opt" is the module after wasm-opt. Every row was built and measured on its own, on the
state of main at 7e97970 unless said.

| Step | opt | brotli | Decision |
|---|---|---|---|
| Baseline: `opt-level="s"`, wasm-opt `-Os`, fonts inside | 17.63 | 4.48 | |
| `opt-level="3"`, `-O3` | 19.64 | 4.99 | faster to run is not the goal here; rejected |
| `opt-level="z"`, `-Oz` | 15.98 | 4.21 | see the mixed profile below |
| `-C target-feature=+simd128`, `-Os` | 15.92 | 4.36 | **kept** |
| `s` + simd + `-Oz` | 15.70 | 4.37 | |
| `z` + simd + `-Oz` | 13.97 | 4.02 | smallest, but the frame is 20% to 40% slower |
| `z` + simd + `-Oz --converge` | 13.96 | 4.01 | no gain; slower build; rejected |
| Fonts out of the module (7 files, see `web-app.md`), on the `z` + simd row | 11.05 | 2.87 | **kept**. The wire total does not shrink by itself: Noto Sans (0.67), STIX (0.37), Hack and the icon font (0.27) are loaded only when used |
| Mixed profile: `s` for the per-frame code, `z` for 33 cold crates (engine, io, doc, collab, Loro, serde, wgpu, image decoders, browser glue), wasm-opt `-Oz` | 11.79 | 3.06 | **kept**: 0.19 MiB bigger than `z` everywhere, and as fast as the baseline |
| Same plus `z` for the text stack (parley, harfrust, skrifa, ICU, ReX) and Loro without its `counter` feature | 11.64 | 3.03 | 1%: not worth the risk of a slower first layout; rejected |
| Without `serde_path_to_error` (a shim with no key path) | -0.21 | -0.03 | rejected: 1% for losing `bad_arg`'s key path (`bounds.w`), which agents need |

Frame cost of the profiles (UI phase of the pan scenario, mean): baseline 2.6 to 2.8 ms, `z` everywhere 3.0 to 3.9 ms, mixed 2.8 ms. The UI phase
is egui (layout and tessellation), which is why egui, Vello, kurbo and the render, canvas and ui crates stay at `s`.

What the questions found:

- **`vello_cpu` and `fearless_simd`** (2.1 MiB before wasm-opt) are not ours and not behind a feature. `epaint` 0.37 depends on `vello_cpu`
  unconditionally, for the glyph rasteriser of egui's text; it is not the PNG export (that is the GPU, `Compositor::render_to_image`). Without
  SIMD, `fearless_simd` compiles its fallback once per pipeline and per blend mode; with `simd128` the code that remains is 0.45 MiB.
  Removing it altogether needs a patched `epaint`; not done.
- **`serde_path_to_error`** (0.5 MiB before wasm-opt) is kept, see the table.
- **`schemars`** (about 0.7 MiB before wasm-opt) is out of the web module. The derive stays on the 70 types in four crates, but the code
  it makes is only linked where something calls it, and the web build calls nothing: `drafftink-engine` has a `schemas` cargo feature
  (default on) that generates the argument and output schemas from the types, and `drafftink-web` and the crates it uses depend on the
  engine with `default-features = false` (the desktop apps and every test turn it on again; `--all-features` does too, so
  `crates/engine/tests/without_schemas.rs` has a gate of its own, `cargo xtask test`). Every reader goes through
  `CommandSpec::args_schema` / `output_schema` / `arg_keys` (`engine/src/schemas.rs`), which read a table when the feature is off. The table is
  `drafftink-headless schemas`, written by `scripts/build-web.sh` as `schemas/commands.<hash>.json` (33,875 bytes with brotli), listed in
  `window.__assets` and preloaded; `drafftink_app::web::schemas_ready()` fetches it once at start and installs it. Measured on 2026-11-09,
  same machine, `main` at `abd8892` against this change:

  | | main | now |
  |---|---|---|
  | module after wasm-opt | 12,598,210 | 11,922,866 (-675,344, -5.4%) |
  | module, brotli -11 | 3,300,460 | 3,218,493 (**-81,967, -2.5%**) |
  | schemas file, brotli -11 | - | 33,875 |
  | first visit, module plus file | 3,300,460 | 3,252,368 (-48,092, -1.5%) |

  The saving on the path that blocks the first frame is the full 82 KB; the file is fetched in parallel and nothing waits for it before the
  first frame. What waits for it: `describe` and `commands` with schemas in `window.drafftink` (a promise), the WebMCP tools (built after it;
  `webmcp.ready`), the arguments forms of the palette (a few milliseconds after the first frame, the app is woken when it arrives), and a request
  that reaches the engine first is refused with `schemas_loading` (`web-app.md`, "Command schemas"). The one thing the web module lost: before
  the table is in, a misspelt key is refused without the "did you mean" (serde still names the key).
- **Duplicate crates** (`cargo tree -d`): `hashbrown` x2, `heapless` x3, `itertools` x3, `miniz_oxide` x2, `smol_str` x2, `thiserror` x2, `rustc-hash` x2
  and some proc-macro crates. They come from Loro, egui, Parley and ReX with different pins; none is large (`miniz_oxide` is the biggest, 50 KB).
- **Loro features:** the default `counter` feature is the only one; dropping it is in the 1% row above.
- **Remaining weight** (twiggy, before wasm-opt, about, MiB): `drafftink_engine` 1.3, Loro 0.9, `read_fonts`/`skrifa`/`harfrust` 1.1, `serde_json` 0.5,
  `serde_path_to_error` 0.5, `drafftink_edit` 0.4, `schemars` 0.3, `egui` 0.3, `generic_btree` 0.2, data (GelPen fonts, ICU tables, the welcome
  document) 2.0.

### 8.3 Native binaries

`cargo xtask size --native` builds the three shipped binaries with the profile `dist` (the packaging scripts use it) and checks `native.drafftink_bytes`, `native.mcp_bytes` and `native.server_bytes` of the `size` section of `perf/budgets.json`. `--no-build` measures `target/dist`. It is not part of `ci` (a build with fat LTO takes seven minutes); `release.yml` runs it on the Linux x86_64 package.

The desktop app was 24.4 MB in the first version (`opt-level = "z"`, full LTO, no strip) and 55.2 MB in the rewrite (`opt-level = 3`, thin LTO, no strip). Measured on 2026-11-08, Linux x86_64, in MB:

| Build | `drafftink` | What changed |
|---|---|---|
| `release` before | 55.2 | `opt-level = 3`, thin LTO, symbols kept |
| `release` now | 46.9 | `strip = "symbols"` (8.3 MB of symbol tables). Debug info was already off |
| `dist` | 42.2 | fat LTO, and `opt-level = "s"` for Loro, serde, schemars, rmcp, rustls and the image decoders (the code that runs on a command or a file, not per frame) |

`panic = "abort"` was not used: the executor turns a panic in a command into a refusal and a rollback (`deviations.md` D3), which needs unwinding. `opt-level = "s"` for the engine, `edit`, `render` and `ui` was not tried, because the frame loop runs them (the web profile keeps them at `s` for the same reason and measured 20 to 40% slower frames with `z`). The rest of the size is the code that does the work (symbol sizes of the `release` build before `dist`): `drafftink_engine` 3.4 MB, Loro 1.9 MB, naga and wgpu 2.5 MB, rmcp 0.9 MB (the MCP door of the desktop app), egui and winit 1.2 MB, and 2.7 MB of bundled fonts. `cargo tree -d` shows small duplicates (`base64` 0.22 and 0.23, two `sha2` generations, `miniz_oxide`, three `getrandom`) that come from different pins in Loro, wgpu and egui; none is above 100 KB.

### 8.4 Rules

- A size limit is raised only with the reason in its `note`, and lowered in the commit that makes the number better.
- Do not enable `--all-features` in wasm-opt (`web-app.md`). Do not move a crate that runs every frame to `z`: measure with `scripts/web_startup.py`.
- A new big asset is a file under `dist/fonts/` (or another hashed folder), not an `include_bytes!` in the module.
