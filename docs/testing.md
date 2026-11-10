# Testing

Everything a user can do is a command, so almost everything can be tested end to end by running commands against a `Session`. This page says which kind of test goes where, how to write a scenario, and the rules that keep the test code honest. The design is in `plan.md` §8.

## The pyramid

| Level | Where | What it checks | Run by |
|---|---|---|---|
| Unit, per command family | `crates/engine/src/cmd/<family>_tests.rs` | availability, refusal text, effect, one negative case, hostile arguments | `cargo xtask test` |
| Registry-wide | `crates/engine/src/registry_tests.rs`, `catalog_tests.rs` | every registration states every fact; `hostile_params_never_panic` sends junk to every command | `cargo xtask test` |
| Domain crates | `crates/{geom,scene,edit,doc,io,...}` | the rules themselves, with typed errors, properties, hostile input | `cargo xtask test` |
| **Scenarios** | `tests/scenarios/<family>/*.json`, run by `crates/testkit` | a user story as a list of commands with checks. One generated test per file | `cargo xtask test`, `cargo xtask harness` |
| Pixel probes | scenarios with `requires.gpu`, `Harness::pixel_probe` | the picture, by counting and sampling pixels with a tolerance, never by comparing with a golden image | the same; needs an adapter |
| Browser e2e *(planned)* | Playwright against `window.drafftink`, headed Chromium on Xvfb with the GPU | the web shell: input, canvas, PWA, file drop | CI job, not written yet |
| Native e2e *(planned)* | tests that drive the native app through the bridge (`crates/bridge`) | the native shell | not written yet |
| Perf | `cargo xtask perf` and the criterion benches | frame time, idle redraw, scale | `docs/performance.md` |

A new test goes to the lowest level that can see the bug. A command that refuses wrongly is a unit test of its family. A story that crosses families ("move a shape, the arrow follows, undo puts both back") is a scenario. Pixels are a scenario with a probe, because only they need the GPU.

A regression test goes in the area's existing test file, never in a new file named after the bug.

## Writing a scenario

A scenario is a JSON file. Put it in `tests/scenarios/<family>/<name>.json`, where `<family>` is a command family (`view`, `arrow`, ...) or a topic (`fixtures`, `scale`, `pixels`). Names use `a-z`, `0-9` and `_`. `build.rs` generates one `#[test]` per file, named `scenarios::<family>::<name>`, so you add no Rust. Run one:

```sh
cargo test -p drafftink-testkit --test scenarios arrow::bind_unbind_rebind
DRAFFTINK_SCENARIO_TRACE=1 cargo test -p drafftink-testkit --test scenarios arrow:: -- --nocapture   # every step's output
DRAFFTINK_SCENARIO_KEEP_GOING=1 ...        # report every failing step, not only the first
```

JSON was chosen over YAML or JSON5 because command arguments are JSON already (`engine.describe` prints examples you can paste), `serde_json` is in the tree, and there is no second parser to trust. The price is no comments. Use the `note` key.

```json
{
  "about": "One sentence: what this proves.",
  "steps": [
    { "harness": "load_generated", "args": { "count": 5 } },
    { "execute": "arrow.unbind", "args": { "arrow": "e4", "end": "both" },
      "expect": [ { "path": "$.freed", "eq": ["start", "end"] } ] },
    { "undo": 1 },
    { "execute": "arrow.bind", "args": { "arrow": "e0", "end": "end", "target": "e1" },
      "refused": "wrong_kind", "text": "arrows and lines" }
  ]
}
```

### File keys

| Key | Meaning |
|---|---|
| `about` | required; what the scenario proves, so a failure means something |
| `ports` | `"full"` (default: every fake port) or `"headless"` (clock and ids only, for `port_unavailable` and no-storage cases) |
| `requires.commands` | commands the scenario waits for. While one is not registered the test is **skipped with a message that names it**. Use it for a scenario written ahead of its command |
| `requires.gpu` | needs a GPU adapter; skipped without one unless `DRAFFTINK_REQUIRE_GPU=1`, which turns the skip into a failure |
| `steps` | the steps, in order |

### Steps

One verb per step, plus its keys. Any step may have `note`, and `{ "note": "..." }` alone is a step that does nothing. Unknown keys are errors, so a typo cannot turn a check into a no-op.

| Verb | Keys | Does |
|---|---|---|
| `execute` | `args`, `refused`, `text`, `expect`, `save` | `Session::execute` |
| `batch` | `opts`, `refused`, `text`, `expect`, `save` | the value is `[{id, args, as}]`; `Session::execute_batch`. Use `"$name"` references as in the batch docs |
| `assert` | `args`, `refused`, `text`, `expect`, `save` | runs a **read-only** command and checks its output. A command that changes state is an error here, so a check cannot change what it checks |
| `undo`, `redo` | `refused`, `text`, `expect`, `save` | `history.undo` / `history.redo`; the value is the number of steps (`1`, or `true` for one) |
| `harness` | `args`, `fails`, `expect`, `save` | a `Harness` method (see below) |
| `snapshot` | `of_command` or `of_harness`, `args`, `select` | compares a text with a stored file |

A step must succeed unless it has `refused: "<code>"`. Then it must be refused with that code, and every string in `text` (one string or a list) must appear in the message. `expect` then checks the refusal as JSON (`code`, `message`, `command`, `suggest`, `details`). A refusal is a value, so test its words: they are what the user and the agent read.

### Checks (`expect`)

A check is an object with a `path` (default `$`, the whole output) and **one** operator.

| Operator | Meaning |
|---|---|
| `eq`, `ne` | equal, not equal. Numbers compare as numbers |
| `contains` | text contains text; array has the item; object has the key |
| `gt`, `ge`, `lt`, `le` | numeric order |
| `near` (+ `tol`, default 1e-6; `plus`) | within `tol` of `near + plus`. `near` may be an array of numbers |
| `exists` | the path leads somewhere (`true`) or nowhere (`false`) |
| `len` | length of an array, string or object |
| `is` | JSON type: `string`, `number`, `bool`, `array`, `object`, `null` |

A check may carry a `note`, shown when it fails.

Paths are JSONPath-lite: `$` the output; `.key` or `['key']`; `[2]` and `[-1]` for items from the front and the back; `[id=e4]` for the first array item whose `id` member is `e4`. No wildcards: if you need one, ask a command for the answer.

### Saved values

`"save": { "x0": "$.element.points[0][0]" }` keeps a value of the step's output. Later steps write `"{{x0}}"` in `args` and in expected values. A string that is exactly `{{x0}}` keeps the value's type; inside a longer string it is text. This is how "the arrow moved by 120" is written without hard-coding the generator's numbers:

```json
{ "path": "$.element.points[0][0]", "near": "{{x0}}", "plus": 120, "tol": 1.0 }
```

### Snapshots

`{ "snapshot": "outline", "of_harness": "scene_probe", "args": { "outline": true }, "select": "$.outline_text" }` compares text with `tests/scenarios/<family>/snapshots/<file stem>__<name>.txt`. A missing or different file fails and shows the first differing line. `DRAFFTINK_UPDATE_SNAPSHOTS=1` writes the file; read the diff before you commit it. The planned form is `of_command: doc.outline`, which replaces the scene probe when that command is registered.

### Order of work

1. State the story in `about`.
2. Build the scene with commands. If the scene is big or comes from a file, use `load_generated` or `load_fixture`.
3. Run the commands. Check the result of each with `expect`, and the state with an `assert` of a read-only command.
4. Include at least one refusal with its `text`, and an `undo` that proves the step is one step.
5. Run it with `DRAFFTINK_SCENARIO_TRACE=1`, read what the commands returned, and make the checks say what the documentation promises, not what the output happens to be.

## The harness

`crates/testkit` (layer L7) has one type for tests, `Harness`: a `Session` with fake ports (`Fakes`: clock, clipboard, storage, files, renderer, net), the **real typesetter**, sequential ids (`e1`, `e2`, ... and seeds 1, 2, ...) and a clock that moves only when told. `h.session` and `h.fakes` are public. Run commands on the session:

```rust
let mut h = Harness::new();
h.load_generated(5, 1)?;
h.session.execute("arrow.unbind", json!({ "arrow": "e4", "end": "both" }))?;
```

`Harness::headless()` has only the clock and ids.

### Methods only for what commands cannot do

Every public method of `Harness` that takes `self` has a doc line `Reason:` that says what a command cannot do here. Today:

| Method | Reason, in short |
|---|---|
| `load_fixture(path)` | reads `tests/fixtures/<path>` from the test machine's disk; the fake `Files` port cannot see it. Becomes "put the bytes into the fake port and run `doc.open`" when that command exists |
| `load_generated(count, seed)` | 1k to 100k elements cannot be made with commands in reasonable time; scenarios and benches share the generator (`drafftink_testkit::scenegen`) |
| `advance_clock(ms)` | time is a port; a command cannot move a fake clock. Undo coalescing and autosave depend on it |
| `scene_probe(id?, outline?)` | no read-only command lists elements yet. Answers from the `drafftink-edit::query` functions that `element.query` and `doc.outline` will call. Goes away with its last caller once those are registered |
| `pixel_probe(size, points)` | pixels exist only on a GPU, outside every port. Renders with Vello through `drafftink-render` and `drafftink-gpu`, reports the adapter and whether it is `software` or `hardware` |

**Planned, not here yet:** `pointer_script`, which feeds platform-neutral pointer events through the recognizers of `drafftink-canvas` into the engine. It waits for the intent-to-command controller in `drafftink-engine`. Until then a drag is tested as a gesture command (`gesture.begin`, `gesture.update`, `gesture.end`).

### The accounting test

`crates/testkit/tests/accounting.rs` (run by `cargo xtask harness`, and by `cargo xtask ci`) fails when:

- a `Harness` method has no real `Reason:`;
- a method is declared and no scenario step and no Rust integration test calls it (delete it);
- a scenario or a Rust integration test calls a method that is not declared;
- the scenario dispatcher (`HARNESS_METHODS` in `scenario.rs`) and the declared methods differ;
- a registered command family has no `tests/scenarios/<family>/*.json` (a family that cannot have one yet is listed in `tests/scenarios/pending-families.txt`, which only shrinks);
- a scenario names a command that is not registered and not in its `requires.commands`, or lists in `requires.commands` a command that is registered now (the scenario would be skipped for nothing), or one that is not a command of `docs/commands.md` (a typo that would skip it forever).

It reads the harness with `syn`, so a method cannot hide from it, and it has its own tests that feed it broken inputs. It prints the scenarios that wait for a command and the commands no scenario runs yet; that list is the work to do.

A second guard, `tests/scenario_guard.rs`, changes the expected value of every `eq` check of every committed scenario and requires the run to fail at that step. A check that still passes with a wrong value proves nothing, and the guard names it.

### Pixel probes

A probe renders the active tab's view offscreen and reports counts and single pixels. Compare colours with a tolerance (24 per channel is generous for fills); GPU output differs a little between drivers, so there is no golden image. The probe reports `adapter`, `software`, `hardware`.

- No adapter: a scenario with `requires.gpu` is skipped with a message. `DRAFFTINK_REQUIRE_GPU=1` makes that a failure; set it on the machine that is supposed to have a GPU.
- No GPU machine: `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` selects Mesa lavapipe, which reports `software: true`.

## Fixtures

`tests/fixtures` holds documents from the old app and the facts the old code computed about them; `tests/fixtures/README.md` is the guide. Facts are generated by a tool, never typed. `cargo xtask fixtures` recomputes every fact with the new code and compares; when the old app is checked out next to this repo it also regenerates the facts from the old code and compares bytes. A scenario that checks counts of a fixture cites the facts file in a `note`, and `cargo xtask fixtures` is the independent proof that the facts are right.

## Scale

`drafftink_testkit::scenegen::generate(count, seed)` builds the same document every time: a grid of cells 140 units apart in a cycle of ten kinds (rectangles, ellipses, diamonds, lines, **bound arrows**, freehand, text, math, and a rectangle and an ellipse that form a **group**). Element `i` has the id `e<i>`. Sizes are 1,000, 10,000 and 100,000 (`scenegen::SIZES`). The perf benches use the same generator, so "the 10k scene" is one scene.

## Rules

- **Never weaken an assertion to get green.** Find out whether the app or the test is wrong, and fix that one. Do not widen a tolerance, delete a check, or turn `eq` into `exists` to pass.
- No wall-clock time in tests. The clock is a port; use `advance_clock`.
- No test depends on another's state. Each scenario builds its own session.
- Tests of numbers use the fake text measure (`Services::fake()`), whose widths are exact. Scenarios use the real typesetter, so they check structure and relations, not glyph widths.
- A scenario written ahead of its command uses `requires.commands`. A scenario that fails because of a bug is never marked skipped or loosened: fix the bug, or leave the scenario failing and say so. There is no "known failing" list.
- Code, comments and docs are in English.

## Environment variables

| Variable | Effect |
|---|---|
| `DRAFFTINK_SCENARIO_TRACE=1` | print every step's output (use with `--nocapture`) |
| `DRAFFTINK_SCENARIO_KEEP_GOING=1` | report all failing steps, not only the first |
| `DRAFFTINK_UPDATE_SNAPSHOTS=1` | write snapshot files instead of comparing |
| `DRAFFTINK_REQUIRE_GPU=1` | a missing GPU adapter fails instead of skipping |

## Gates

| Gate | Runs |
|---|---|
| `cargo xtask test` | the whole suite, scenarios included |
| `cargo xtask harness` | a quick shape check of `tests/scenarios`, then the accounting test, the scenario guard and every scenario |
| `cargo xtask fixtures` | the fixture facts |
| `cargo xtask ci` | all gates in order |
