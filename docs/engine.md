# The engine: adding a command family, and building a shell on `Session`

This page is for two readers. The first adds a command family under `crates/engine/src/cmd/`. The second builds a shell (UI, bridge, wasm) on the `Session` API. The contract that both rely on is `docs/commands.md` section 1. The layering is in `docs/architecture.md`. Where the code differs from the contract, section 6 lists it.

## 1. What the engine is

`crates/engine` (package `drafftink-engine`, layer L5) owns everything a user can change in one `Session`: tabs with their documents, selection, camera, tool, settings, the one running gesture, and the ring of recent calls. Every user action is a registered command. Shells call `Session::execute` or `Session::execute_batch`. The engine runs gates, argument checks, the handler, rollback, the invariant pass, undo, port writes and the ring around the handler, so a handler holds only the command's own logic. The engine depends on no UI, OS or GPU crate. It reaches the outside world through the traits in `crates/engine/src/ports.rs`. `crates/engine/tests/layering.rs` checks this.

Files in `crates/engine/src`:

- `crates/engine/src/lib.rs`: module list, re-exports, and the "you want / use" table.
- `crates/engine/src/spec.rs`: `CommandSpec`, `Facts`, `Typed`, the fact enums (`Undo`, `Gate`, `Redraw`, ...), `Binding`, `NoArgs`.
- `crates/engine/src/session.rs`: `Session` and its helpers (`targets`, `resolve_ref`, `warn`, `live`, dirty reasons, `state_json`).
- `crates/engine/src/executor.rs`: `execute`, `execute_from`, `execute_typed`, `run_nested`, gates, transaction, undo push, ring writes.
- `crates/engine/src/batch.rs`: `execute_batch`, `execute_batch_json`, preflight, `$name` references.
- `crates/engine/src/gesture.rs`: the gesture protocol and the typed fast path.
- `crates/engine/src/ring.rs`: the recent-command ring, `Origin`, `Outcome`.
- `crates/engine/src/refusal.rs`: `Refusal`, the shared codes with their exact wording, constructors.
- `crates/engine/src/args.rs`: JSON to typed arguments with `bad_args` text, and number checks (`world`, `in_range`, `positive`).
- `crates/engine/src/store.rs`: `TabDoc`, the working store with its rollback journal, the replicated (Loro) store and undo. The only file that knows how a document is held.
- `crates/engine/src/state.rs`: plain session data (`Observed`, `View`, `Tool`, `Settings`, `Tab`, `Extensions`).
- `crates/engine/src/ports.rs`: the port traits, `Ports`, `PortWrite`, and in-memory fakes in `ports::fake`.
- `crates/engine/src/registry.rs`: `Registry`, `CommandInfo`, `Session::commands`.
- `crates/engine/src/invariants.rs`: the post-command invariant pass (arrows, then bound text; a marked call site for frame membership). Tests: `invariants_tests.rs`.
- `crates/engine/src/services/`: the `Services` a session owns: the text and formula engine behind the measuring traits of `edit` and `io` (`Measure`, `TypesetMeasure`, `FakeMeasure`).
- `crates/engine/src/remote.rs`: `Session::apply_remote`.
- `crates/engine/src/keymap_tests.rs`: the keymap of `docs/commands.md` section 6.1 against the registered shortcuts.
- `crates/engine/src/testing.rs`: session and element fixtures for tests.
- `crates/engine/src/cmd/mod.rs`: the `families!` line. `crates/engine/src/cmd/*.rs`: one file per family.
- `crates/engine/src/registry_tests.rs` and `crates/engine/src/catalog_tests.rs`: tests that run over every registered command.
- `crates/engine/src/executor_tests.rs`: tests of the executor, batches and gestures with synthetic commands.

## 2. Adding a family

The worked example is the `view` family: `crates/engine/src/cmd/view.rs`, with tests in `crates/engine/src/cmd/view_tests.rs`. The `engine`, `gesture` and `perf` families in the same folder show a read-only family and a family that owns its transaction.

Start from the catalog. Your family's ids, arguments, refusal wording and facts are fixed in `docs/commands.md` (section 2 and Appendix A). A change to a command starts with a change there.

### (a) Files

Create `crates/engine/src/cmd/<family>.rs` and `crates/engine/src/cmd/<family>_tests.rs`. The family file ends by including its tests:

```rust
#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
```

### (b) Arguments and output

One typed struct per command. All five derives and `deny_unknown_fields` are needed. A registry test fails if the schema is not a closed object.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PanArgs {
    /// Screen pixels the content moves to the right (as the pointer would).
    pub dx: f64,
    pub dy: f64,
}
```

`Debug, Clone` are needed because the engine clones arguments for gesture frames and the ring. `Serialize` is needed because the ring records them. Use `#[serde(default)]` for optional fields. A command with no arguments takes `NoArgs`. Add `Default` to a struct whose fields all have defaults so that `{}` works.

The output is a struct with `Serialize, JsonSchema`. Give it a `changed: bool` when the command can find the state already as requested (see `CameraOut`). The engine adds the common keys (below), so a payload struct holds only the command's own keys. A handler may also return `serde_json::Value`.

Doc comments on fields appear in `engine.describe` and in the MCP tool schema. Write them for an agent.

### (c) The handler

```rust
fn pan(s: &mut Session, a: PanArgs) -> Result<CameraOut, Refusal> {
    let id = s.cmd();
    let (dx, dy) = (args::world(id, "dx", a.dx)?, args::world(id, "dy", a.dy)?);
    with_view(s, |v| { /* change the view */ Ok(()) })
}
```

The signature is `fn(&mut Session, Args) -> Result<Out, Refusal>`. Rules:

- `s.cmd()` is the id of the running command. Pass it as the first argument of every refusal constructor. The executor fills in an empty `command` field, but the constructors need the id for the message text.
- Validate every number with `args::world`, `args::in_range` or `args::positive`. `hostile_params_never_panic` sends `1e308`, `NaN`-like values, 5000-character strings and deep arrays to every argument key.
- Validate everything before the first change. A refusal rolls everything back anyway, but a refusal that comes before any edit is cheaper and easier to read.
- Refusal constructors in `crates/engine/src/refusal.rs` carry the exact wording of `docs/commands.md` section 1.5: `bad_args`, `bad_arg(id, key, problem)`, `no_target`, `not_found`, `locked`, `wrong_kind`, `not_applicable`, `too_few`, `needs_single`, `nothing_to_do(id, verb, reason)`, `port_unavailable`, `io_failed`, `parse_failed`, `limit_exceeded`, `unsaved_changes`, `internal`, `not_supported`. A command-specific code uses `Refusal::custom(code, id, message)`. Add `.suggesting(command, args)` for a machine-readable next step and `.with_details(json)` for data.
- The message is plain words, names the value that was wrong, and ends with what to do. It is the same text for the toast, the greyed palette item and the agent.
- A state that is already as requested returns `Ok` with `changed: false`. A command whose whole purpose is void refuses with `nothing_to_do`. See `zoom_to_fit` for the second case.
- A handler never calls `execute`, never opens a dialog, never touches the OS (files, clock, network, threads), and never uses `unwrap`, `expect`, `panic!` or indexing that can panic. `execute` called from a handler is refused as `internal`. To run another command inside yours, use `Session::run_nested(id, args)`, which skips the transaction and lets the outer command own it.

Document edits go through the tab's document:

```rust
let cmd = s.cmd();
s.tab_mut().doc.edit::<(), StoreError>(|tx| {
    let z = tx.next_top_index().map_err(|e| StoreError::Backend(e.to_string()))?;
    tx.insert(Element::new(id, kind, z))
}).map_err(|e| Refusal::internal(cmd, e.to_string()))?;
```

`TabDoc::edit` hands the closure a transaction that implements `SceneTx`. Call the domain functions of `crates/edit` with it (`crates/edit/src/lib.rs` lists the modules). A rule lives in the domain crate, never in a command. There is no blanket conversion from a domain error to `Refusal`: map each typed error to the refusal that the catalog names for it. Calling `edit` several times in one command is fine. All calls roll back and undo together.

Helpers on `Session` for handlers:

- `s.targets(Some(&refs))` and `s.targets(None)`: the explicit refs (each resolved, duplicates dropped) or the selection. An empty result is `no_target`. Use it for `Selection::Required`.
- `s.resolve_ref(&Ref)`: an id or `"@name"` to a live element id, else `not_found`.
- `s.element(&id)`, `s.store()` (the working store), `s.selection()`, `s.view()`, `s.active_tool()`, `s.tab()`, `s.tab_mut()`.
- `s.ports.ids.element_id()` and `s.ports.ids.seed()` for new ids and sketch seeds, `s.now_ms()` for time. Never use the system clock or a random source.
- `s.require_port(PortKind::Renderer, what, alternative)`: the `port_unavailable` refusal or `Ok`. Optional ports are `Option` fields of `s.ports`.
- `s.queue_write(PortWrite::...)`: a clipboard, storage or file write. It runs only after the command (or batch) succeeded, so a refusal has no side effect. A failed write becomes a warning.
- `s.warn("note")`: a non-fatal note that appears in the output `warnings`.
- `s.live()`: true while a gesture frame runs. Skip work that only the final state needs (the library, expensive re-measures). Do not skip the binding solver or snapping.
- `s.clear_history()`: for commands that replace the document (`doc.new`, `doc.open`). The command then pushes no undo step.
- Family-private state: `s.ext.with::<MyState, _>(|st| ...)` for session-wide state (clipboard buffers, library, recent list) and `s.tab_mut().ext.with(...)` for per-tab state. The type needs `Default + 'static`. `Extensions` are outside the rollback transaction. A command that must stay atomic writes them last.

### (d) Registration

```rust
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("view.pan", "view", "Pan", "Pans the camera by screen pixels; ...",
        &Typed::new(pan), QUIET)
        .example(r#"{"dx":-120,"dy":0}"#),
    // ...
];
```

`CommandSpec::new(id, family, title, summary, &Typed::new(handler), Facts { .. })`. The `family` must equal the part of `id` before the dot. `title` is sentence case with a capital first letter and no final period, and unique across the registry. `summary` is one sentence of at least 20 characters ending with a period. `Typed::new` fixes the argument and output types, and the schemas come from them.

`Facts` has 13 fields and no `Default`. Leaving one out is a compile error. Most families define a few base values and override with struct update syntax, as `crates/engine/src/cmd/view.rs` does:

```rust
const QUIET: Facts = Facts { ring: RingMode::Quiet, ..Facts::CAMERA };
const PALETTE: Facts = Facts { palette: true, ..Facts::CAMERA };
// one command:
Facts { needs_selection: Selection::Soft, ..PALETTE }
```

`Facts::READ_ONLY` is for reads: no undo, changes nothing, allowed in every state, `agent: true`, not in the palette. `Facts::CAMERA` is READ_ONLY plus `mutates: VIEW` and `redraw: Camera`. A document command writes its own base, as `DOC` in `crates/engine/src/executor_tests.rs` does: `undo: Undoable`, `mutates: DOC | OBSERVED`, `view_only: Refused`, `while_gesture: Refused`, `redraw: Scene`.

The facts must agree with Appendix A of `docs/commands.md`. `facts_match_appendix_a` compares them column by column.

Optional builders, all `const`, chained after `new`:

| Builder | Use |
|---|---|
| `.available(fn(&Session) -> Result<(), Refusal>)` | State-dependent check. It never reads the arguments. Its text is what a greyed control shows. |
| `.default_targets(fn(&Session) -> Result<(), Refusal>)` | Target checks that use the default arguments (the selection): `too_few`, `needs_single`. `Session::availability` runs it. |
| `.refusals(&["code", ..])` | Command-specific codes only. Shared codes are implied by the facts. Each code must appear in the command's row in `docs/commands.md`. |
| `.example(r#"{..}"#)` | A worked argument example as JSON text. Required when an `agent: true` command has required arguments. It must parse. |
| `.coalesce("key", ms)` | Merge a repeat of the command on the same elements within `ms` of the previous repeat into the previous undo step (section 5). Applies to a plain call only: the last step of a gesture and a batch never merge. |
| `.shortcuts(&[Binding])` | Default keys. `Binding::new(&["Mod+0"], Scope::Canvas)`, then `.with_args(json)`, `.titled("...")`, `.mac(&[..])`. Section 6.1 of `docs/commands.md` is the source of the keys. See item 13 of section 6. |
| `.planned("reason")` | Registered but refuses with `not_supported`. The catalog, palette and tests still see it. |
| `.own_transaction()` | `Txn::Own`: the handler manages document state. Only the `gesture` and `history` families may use it (tested). |

### (e) Register the family

Add the name to the one line in `crates/engine/src/cmd/mod.rs`:

```rust
families! { engine, gesture, perf, view }
```

The macro declares the module and adds `<family>::COMMANDS` to `FAMILIES`. This line is the only edit to a shared Rust file. Every registry-wide test now covers your commands.

### (f) Unregistered list

`crates/engine/unregistered.txt` lists catalog ids that no family has registered. Delete the lines of the ids you register. The file is sorted, has no duplicates, and only shrinks. `catalog_matches_registry` fails if a registered id is still listed, or if a catalog id is neither registered nor listed.

### (g) What the executor does for you

For each call, in order (see `crates/engine/src/executor.rs`):

1. Looks up the id. An unknown id is `unknown_command` with up to three suggestions.
2. Gates from facts: planned status (`not_supported`), `view_only`, `while_gesture`, `while_text_edit` (a `Commit` gate commits the running text edit first).
3. `available`. Then parses the arguments (`bad_args` names the key and, for an unknown key, the closest valid one).
4. Takes a mark in the document journal and copies the small session state (selection, editing group, point editor, tool, camera, text edit, view-only).
5. Runs the handler, catching panics (native builds; a panic becomes `internal`).
6. On `Err` or panic: rolls the document and the small state back, drops queued port writes and warnings, records the refusal.
7. On `Ok`: runs the invariant pass if the document changed (section 4, "The invariant pass"; a failure rolls back with `internal`), writes the change to the replicated store in one transaction labelled with the command `title` (it is an undo step if `undo` is `Undoable` or `Gesture` and something changed; otherwise it syncs without a step), flushes port writes, sets the dirty reason from `redraw`, writes the ring entry.

So a handler is atomic without effort. A no-op never creates an undo step. Selection changes alone never create one either. The output gets the common keys: `changed`, `undo_label` (when a step was pushed), `created` and `deleted` (element ids, read from the journal, not from your code), and `warnings`. A key you set yourself wins (`changed` in `CameraOut`).

What rollback does not cover: tabs, settings, `s.ext` values and port writes already flushed. In a batch a step with `mutates` TABS, SETTINGS, PORTS or COLLAB is listed in `not_rolled_back`.

## 3. Testing a family

No test builds a DOM or a UI. A test creates a `Session` over fake ports and calls `execute`. The file is `crates/engine/src/cmd/<family>_tests.rs`. For style, read `crates/engine/src/executor_tests.rs`: small helpers `ok(&mut s, id, args)` and `refused(&mut s, id, args)` that panic with the refusal text, then one `#[test]` per behaviour.

Fixtures from `crates/engine/src/testing.rs`:

- `session()` returns `(Session, Fakes)` with every fake port. `Fakes` keeps handles to inspect the clipboard, storage, files, renderer, net and the clock (`fakes.clock.advance_ms(..)`).
- `headless_session()`: only clock and ids. Use it to test `port_unavailable`.
- `session_with_shapes()`: three rectangles `a`, `b`, `c` with `a` selected.
- `rich_session()`: rectangles, ellipse, named diamond, arrow, text, frame, two selected. It is the fixture of the registry-wide tests.
- `add_rect`, `add`, `select`: put elements in place. They write a normal commit and clear the journal, so no undo step exists afterwards.

For each command write four kinds of test:

1. Availability: `s.availability("x.y")` in the state where it must be greyed out, and an assertion on the refusal code and the text. The text is what the user reads, so assert on the key phrase (`r.message.contains("`view.center`")`). Then check that `execute` returns the same code and text.
2. Refusal text: each command-specific refusal, with the exact wording from the catalog.
3. Effect: the state after a success (the document, selection, camera), the output keys, `s.tab().doc.undo_depth()` (one step, or none for `NoUndo`), and that `s.undo_steps(1)` restores the document. For `view` the effect is on `s.view()`.
4. A negative case: a refusal leaves `s.fingerprint()` unchanged. Take it before and compare after.

```rust
#[test]
fn zoom_to_fit_on_an_empty_document_refuses() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    let r = s.execute("view.zoom_to_fit", json!({})).unwrap_err();
    assert_eq!(r.code, "nothing_to_do");
    assert!(r.message.contains("`view.center`"), "{}", r.message);
    assert_eq!(s.fingerprint(), before);
}
```

Tests may use `unwrap` and `expect`; the crate allows them under `cfg(test)`.

### What the registry-wide tests check for free

These run over every registered command as soon as the family name is in `families!`.

In `crates/engine/src/registry_tests.rs`:

- `ids_are_unique_and_well_formed`: `family.verb`, lower case, one dot, the prefix equals `family`, no duplicates.
- `words_are_plain`: title capitalised without a final period, summary a sentence.
- `facts_are_consistent`: read-only implies no state class and no undo; a command that changes the document refuses in view-only mode and has an undo class (except the listed `DOC_EXEMPT_*` ids); a gesture command changes the document; coalesce needs undo; planned needs a reason.
- `titles_are_unique`.
- `default_shortcuts_do_not_collide_within_a_scope`, on macOS and elsewhere.
- `shortcut_arguments_are_valid_for_their_command`: binding args parse against the schema.
- `every_command_publishes_closed_argument_and_output_schemas`: this is the `deny_unknown_fields` check.
- `examples_parse_and_agent_commands_with_required_arguments_have_one`.
- `engine_describe_and_commands_cover_every_command`.
- `hostile_params_never_panic` and `hostile_params_never_panic_in_a_batch`: 34 hostile JSON values, as the whole argument and under every key of the schema. The result may be a refusal or a success. It may not be `internal`, must name the right command, must use a shared or declared code, and a refusal must leave the fingerprint unchanged.
- `refusal_leaves_state_unchanged`: six situations per command (unknown key, view-only, text edit, no selection, no optional ports, running gesture). Every refusal leaves the fingerprint alone and is in the ring with its code.
- `availability_and_execute_agree_on_state_refusals`.

In `crates/engine/src/catalog_tests.rs`, which read `docs/commands.md` at run time:

- `catalog_matches_registry`: every registered id has its two rows in section 2, every catalog id is registered or listed in `crates/engine/unregistered.txt`.
- `identity_table_matches_registry`: title and undo class in the identity table.
- `facts_match_appendix_a`: every fact in Appendix A.
- `declared_refusal_codes_are_catalogued`: every code in `.refusals(..)` is a shared code or appears in the command's row.

How to read a failure: the message names the command id first, then what differs. A mismatch with the catalog means one of two things is wrong. Decide which: change the registration, or change `docs/commands.md` first (the catalog is the contract). Do not edit a test to make it pass. For `hostile_params_never_panic`, the message prints the argument JSON that broke the command. Add a validation for it, then add a regression test in your family's test file with that JSON.

`crates/engine/tests/layering.rs` is separate: `cargo test -p drafftink-engine --test layering`. It fails when a handler uses `std::fs`, `Instant::now` and similar tokens, or when `crates/engine/Cargo.toml` names a UI, OS or GPU crate.

## 4. The Session API for shells and the bridge

```rust
use drafftink_engine::{Origin, Session};
use drafftink_engine::ports::fake::Fakes;
use serde_json::json;

let fakes = Fakes::new();                     // tests; a real shell builds Ports itself
let mut s = Session::new(fakes.ports());
```

`Ports::new(clock, ids)` takes the two mandatory ports. Add the optional ones with `with_clipboard`, `with_storage`, `with_files`, `with_renderer`, `with_net`. A command that needs a missing port refuses with `port_unavailable`. A shell implements the traits in `crates/engine/src/ports.rs` (`Clipboard`, `Storage`, `Files`, `Clock`, `Renderer`, `Net`). Ids come from the `IdSource` trait of the `scene` crate. Nothing in a port may await: the shell resolves anything asynchronous first and passes the result as arguments.

### Running commands

```rust
let out = s.execute("view.pan", json!({"dx": 40, "dy": 0}))?;        // origin: Origin::Script
let out = s.execute_from(Origin::Palette, "view.zoom_in", json!({}))?;
let out = s.execute_typed(Origin::Ui, "view.pan", &PanArgs { dx: 40.0, dy: 0.0 })?;
```

- `execute` uses `Origin::Script`. `execute_from` records who asked. `Origin` never changes a result. A shell sets `Ui`, `Shortcut`, `Menu`, `ContextMenu` or `Palette`. The bridge sets `Agent`. The JSON names are `ui`, `shortcut`, `menu`, `context_menu`, `palette`, `script`, `agent`, `remote`, `test`; `keyboard` and `mcp` are accepted as aliases.
- `execute_typed` takes the command's argument struct and builds no JSON. The type must be the command's own; a mismatch is an `internal` refusal.
- The result is `Result<Value, Refusal>`. `Value` holds the payload and the common keys. `Refusal` has `code`, `command`, `message`, `suggest`, `details`. Show `message` unchanged. A UI may add buttons from `suggest`. Codes in `QUIET_FROM_KEYBOARD` (`no_target`, `nothing_to_do`, `not_applicable`) are not toasted when they come from a shortcut; the shell applies that rule.
- Commands never run inside commands. Calling `execute` while a handler runs is an `internal` refusal.

### Batches

```rust
use drafftink_engine::{BatchOpts, BatchStep};
let steps = [
    BatchStep::new("element.create", json!({"kind": "rectangle", "bounds": {"x":0,"y":0,"w":120,"h":60}})).named("box"),
    BatchStep::new("style.set", json!({"ids": ["$box"], "props": {"fill_color": "blue-100"}})),
];
let out = s.execute_batch(Origin::Agent, &steps, &BatchOpts::default());   // BatchOutput
if let Some(f) = &out.failed { /* f.index, f.id, f.refusal; out.rolled_back */ }
```

`BatchOpts` has `label`, `atomic` (default true) and `dry_run`. A batch is one transaction and one undo step. Preflight checks every step before step 0 runs. `execute_batch_json(origin, &json)` takes `{"steps": [...], "opts": {...}}` and returns `Result<Value, Refusal>`: a malformed batch is a refusal, a batch that ran and failed is a normal result with `failed` set. A batch is one ring entry with id `batch`. Gesture commands, `history.*` and any command with `Batch::Never` cannot be steps. The `element.create` and `style.set` ids above are catalog examples that are not registered yet.

### Greyed-out controls

```rust
for c in s.commands() {            // Vec<CommandInfo>: id, title, family, summary, available
    match &c.available { Ok(()) => enable(c.id), Err(r) => disable(c.id, &r.message) }
}
let one = s.availability("view.zoom_in");          // Result<(), Refusal>
```

`availability` runs the same gates as `execute`, `available`, the selection check for `Required` commands and `default_targets`. A disabled control shows the refusal text as its tooltip, and `execute` returns the same text. Call it when the dirty reason says state changed, not on every frame. Shortcuts are in `spec.shortcuts` (`Session::spec(id)`); `engine.commands` returns them with the availability as JSON.

### Gestures: the typed fast path

For drag, resize, rotate: one call per pointer event, no JSON. Frames are absolute: every update carries the full arguments relative to the state at `begin`.

```rust
let (g, first) = s.gesture_begin_typed(Origin::Ui, "element.move", &MoveArgs { dx: 0.0, dy: 0.0, ..args })?;
s.gesture_update(g, &MoveArgs { dx: 12.0, dy: 3.0, ..args })?;      // every pointer move
s.gesture_end_typed(g, None)?;                                     // one undo step, ring entry with frame count
// or on Esc, blur, pointer cancel:
s.gesture_cancel(Some(g))?;
```

`element.move` is a catalog id that no family registers yet; `MoveArgs` stands for its argument struct. `gesture_end_typed(g, None)` commits with the last frame's arguments, or pass `Some(&args)`. `end` restores the base state and runs the command through the ordinary pipeline, so `begin + updates + end(args)` equals `execute(command, args)`. A refused `update` returns the refusal and keeps the last good frame. A refusal at `end` aborts the gesture and restores the base. Only commands with `Undo::Gesture` can begin. One gesture runs at a time. Other commands refuse with `gesture_active` unless their `while_gesture` is `Allowed`.

The watchdog: the shell calls `s.tick()` on a timer while a gesture runs (`s.gesture_running()`). It cancels a gesture that got no frame for `WATCHDOG_MS` (30 000 ms of the `Clock` port's monotonic time) and returns true. Start the timer at `begin` and stop it at `end`, so an idle app has no timer. `cancel_gesture_if_running()` is for tab switches and view-only changes. The JSON door is the `gesture.begin`, `gesture.update`, `gesture.end` and `gesture.cancel` commands, for the bridge.

### Start, settings and the workspace

Three calls that are not commands (docs/commands.md §1.12) bring a session up and keep it saved. All need the `Storage` port and do nothing without it.

```rust
let mut s = Session::new(ports);
let settings = s.load_settings()?;       // SettingsLoad: applied, ignored (with reasons), unreadable, newer_schema
let workspace = s.restore_workspace()?;  // WorkspaceRestore: restored, skipped tabs with reasons
// ... later, whenever take_dirty() says something changed, and when the timer below fires:
s.workspace_tick();                      // writes when a write is due; true when it wrote
if let Some(ms) = s.workspace_next_due_ms() { set_one_timer(ms) }   // None: no timer at all
s.flush_workspace();                     // window close
```

Call `load_settings` and `restore_workspace` once, before the first command. Settings changes are queued by the `settings.*` commands and written after the command succeeds. The workspace is written by `workspace_tick`, debounced on the `Clock` port (a quiet second, or `autosave.interval_s` after the first unsaved change). The record layouts and the rules are in docs/commands.md §2.2 (workspace) and §2.17 (settings). The engine starts no timer.

Another family that changes a setting calls `cmd::settings::set_internal(s, key, &value)`, which checks the value like `settings.set`, applies it and queues the write.

### Redraw: idle means idle

```rust
if let Some(level) = s.take_dirty() {      // Redraw::Overlay < Camera < Scene
    schedule_one_frame(level);
}
```

Every successful command that changed something raises a dirty reason from its `redraw` fact. `take_dirty` returns the strongest pending reason and clears it. `dirty()` reads without clearing. A shell requests a frame only when `take_dirty` returns `Some`. When it returns `None`, nothing needs drawing and the shell must not draw or poll. A refused command and a rolled-back batch leave the dirty reason alone. Gesture frames, `end` and `cancel` raise it.

### State, ring, history

- `s.state_json()` (also `engine.state`): tabs, tool, view-only, selection summary, running gesture, text edit, revision, dirty, settings revision. Use it for status bars and tests.
- `s.ring().recent(limit, mutating_only, since_seq)` or the command `engine.recent`: the newest entries, oldest first. Each has `seq`, `t_ms`, `id`, `args` (JSON text cut to 512 bytes), `source`, `outcome`, `duration_us`, and `steps` or `frames`. The ring holds 256 entries. A command with `ring: Quiet` is recorded only when it refuses. A gesture is one entry written at `end` or `cancel`.
- `s.undo_steps(n)` and `s.redo_steps(n)` are the engine side of the `history` family. A shell calls the `history.undo` and `history.redo` commands, not these: the commands add the gates, the refusal texts, the step count and the restore of the selection (`crates/engine/src/cmd/history.rs`).
- `s.tab()` and `s.tabs` give read access to the tabs, `s.view()` to the camera of the active tab. Read what you need. Change state only through commands.

### Services: measuring text and formulas

`Session::services` (`crates/engine/src/services/`) is the one text and formula engine of a session, seen through the three traits that lower crates define: `drafftink_edit::text_bind::TextMeasure` (`s.services.edit()`), `drafftink_io::TextMeasure` (`.io()`) and `drafftink_io::svg::MathSvg` (`.math_svg()`). Pass those to the functions of `edit` and `io`:

```rust
let Session { tabs, services, .. } = &mut *s;       // two fields, two borrows
tabs[0].doc.edit(|tx| text_bind::bind_text(tx, services.edit(), &text, &container))?;
```

`Services::new()` is the real engine (`drafftink_typeset::Typesetter` with its LRU caches, built on first use). `Services::fake()` measures with the monospace measure of `edit` (every character is half the font size wide) and the estimate of `io`: exact numbers, no fonts. `testing::session()` and `testing::headless_session()` use the fake; `testing::session_typeset()` and `Session::new` use the real one. `Session::with_services(ports, services)` injects either, and `Services::with(impl Measure)` takes any other. `Services::math_layout(latex, size)` returns the `MathLayout` of a formula; the typesetter caches it, so compute it when the formula is edited and ask again freely. `Services::sharing(handle)` lets the renderer and the session use one typesetter and one set of caches.

### The invariant pass

After a handler returns `Ok` and the document changed, the executor calls `invariants::run` (`crates/engine/src/invariants.rs`) before the Loro commit, so its edits are part of the command: they roll back with it and share its undo step. A batch runs the pass after each step. The pass reads what the command did (`TabDoc::changes_since(mark)`, a `ChangeSet` of the journal) and runs, in order:

1. `binding::update_bound_arrows` for the elements whose geometry changed (position, size, angle, flip, kind), were added, or were deleted;
2. `text_bind::refresh_labels` for those elements and the arrows step 1 moved;
3. frame membership: a marked call site (`FRAME-MEMBERSHIP`). The rules in `edit::frame` need the drop point of the pointer, so the transform commands call them; a rule that depends on the change set alone is called here.

A label that makes its container grow moves arrows bound to the container, so the pass repeats until a round writes no label (at most 4 rounds). A command that changed no geometry costs one read of the change set. A handler does not call these rules for the follow-ups; it calls them only for what it must do itself (for example `refresh_labels` with a chosen `GrowAnchor` after a resize).

### Remote edits

`Session::apply_remote(tab, bytes)` (`crates/engine/src/remote.rs`) takes a Loro update or snapshot for a tab (any tab, not only the active one). It applies it through `TabDoc::apply_remote` (replicated store first, then the touched elements into the working store), drops deleted elements from the selection and the point editor, raises the dirty reason `Scene` when something changed, and returns the `ChangeSet`. It pushes no undo step, runs no invariant pass and is not a command. A bad update is a `parse_failed` refusal and changes nothing; an unknown tab is `no_such_tab`. Each call writes one ring entry, `remote.apply` with origin `remote` and `{tab, bytes, elements}` as arguments (the failure case too), so `engine.recent` shows that something arrived without listing every element. `Session::set_remote_presence` is not built yet.

## 5. The undo model and the one swap point

`crates/engine/src/store.rs` is the only file that knows how a tab's document is held. A tab has two stores:

- **The working store** (`WorkStore`, the in-memory scene of the `scene` crate). Commands and the renderer read and edit it. `TabDoc::edit` journals every edit as a `Patch`: the previous value of each touched element, the previous metadata and the previous file blobs. A failed command, a failed batch, every gesture frame and a gesture cancel replay patches backwards. The cost is what the command touched, not the size of the document.
- **The replicated store** (`DocStore`, which is `drafftink_doc::LoroStore`). It is the CRDT truth and owns undo history and sync. `TabDoc::commit_since(mark, &StepMeta)` writes to it once per command, batch or gesture end, in one transaction labelled with the title and carrying the editor state (selection, editing group) from before and after. Live gesture frames and refused commands never reach it.

Undo and redo (`TabDoc::undo`, `TabDoc::redo`) run in Loro's undo manager. The touched elements are then copied into the working store, and the stored editor state is restored. A remote update (`TabDoc::apply_remote`) takes the same copy path. The price is a second in-memory copy of the scene. A follow-up in `crates/doc` (a transaction that stays open across a command) would remove it. The module doc of `crates/engine/src/store.rs` says nothing outside that file would change.

The swap point is `crates/engine/src/store.rs`. Before the Loro store was connected, `DocStore` was `MemoryStore`, patches held the undo steps, and a `UndoHistory` type lived in the same file. The swap changed the bodies of `TabDoc` and the type aliases. The rest of the engine did not need a change beyond calling `commit_since`, `undo`, `redo`, `undo_depth` and `clear_history` on `TabDoc`.

What stays the same (the contract of the module doc):

- `TabDoc::edit` is the only way to change a document. It commits one transaction and records how to take it back. Handlers see a `SceneTx` and call `crates/edit` functions on it.
- `TabDoc::mark` and `TabDoc::rollback_to` return the document to an earlier point. The executor uses them for a failed command and a failed batch. The gesture machinery uses them for each frame and for cancel.
- One `execute`, one batch or one gesture is one undo step. A step restores the selection and editing group from before it on undo and from after it on redo. The depth is the `history.depth` setting (default 200). A step that has become a no-op (a collaborator already changed the same things) is consumed and not counted.
- `TabDoc::revision` rises with every kept change, including undo, redo and remote updates, and drives the dirty flag. A change that was rolled back does not count.
- Callers outside `crates/engine/src/store.rs` use `mark`, `rollback_to`, `reapply`, `summary_since`, `changed_since`, `commit_since`, `undo`, `redo`, `undo_depth`, `clear_history`. They never look inside a `Patch`.

Coalescing (`.coalesce(key, ms)`): `StepMeta` carries a `CoalesceReq`. `TabDoc::commit_since` folds the step into the previous one when the tab's last step had the same key, changed the same elements, is at most `ms` older (measured from the previous repeat, so a held key keeps merging) and is still the top of the undo stack. Folding is `LoroTx::merge_with_previous` in the doc crate, which raises the undo manager's merge interval for that one commit. The merged step keeps the label and the selection from before the first repeat, and redo restores the selection from after the last. Undo, redo, `clear_history`, any step of another command, a batch, the end of a gesture and a collaborator's edit since the last repeat all end the run. The tests are in `executor_tests.rs` (`coalesce_*`) and `crates/doc/tests/undo.rs` (`merge_with_previous_*`).

Known limits:

- A patch stores whole previous elements. A gesture rollback (each frame, cancel, and the base restore at `end`) overwrites, in the working store, a concurrent remote change to the same element, including fields the gesture did not touch. The final command re-runs at `end` against the current state. `docs/commands.md` section 1.10 rule 8 promises more: remote changes to other fields of the same elements are kept.
- The scene is held twice in memory.
- The journal holds element copies until the command or gesture ends.

## 6. Deviations and open points

Where the code departs from `docs/commands.md`. Items 1 to 7 are intended. Items 8 to 13 are differences that the catalog should pick up.

1. **Origin names.** The catalog calls the field `Source` with `keyboard` and `mcp`. The code has `Origin` with `shortcut` and `agent`, and accepts `keyboard` and `mcp` as serde aliases. It adds `remote`.
2. **`live` is a Session flag.** The catalog says the gesture machinery sets `live = true` on the typed args. The code sets a flag that handlers read with `s.live()`. No args struct has a `live` field, so no published schema can contain it.
3. **`Txn::Own`.** Gesture and history commands manage their own document state (`.own_transaction()`). The executor still does lookup, gates, availability, argument parsing, panic catching and the ring for them. A registry test allows the flag only for the `gesture` and `history` families.
4. **The invariant pass.** `crates/engine/src/invariants.rs` runs arrows, then bound text, repeated up to 4 rounds; frame membership is a marked call site. It runs once per command (per step in a batch) and only if the document changed. See section 4.
5. **`serde_path_to_error`.** `crates/engine/Cargo.toml` depends on it so that `bad_args` names the failing key path (`props.stroke_width`).
6. **Rollback bumps no logical revision.** `TabDoc::revision` subtracts the commits that a rollback undid, so a refused command does not make the document look edited. The working store's own revision counter still goes up.
7. **`perf.stats` with `since: last_call`.** The engine computes the delta between two snapshots of the profiler (`delta` in `crates/engine/src/cmd/perf.rs`, mark in `Session`). The profile crate reports totals only.
8. **Signatures.** The catalog writes `Session::execute(id, args, source)`. The code has `execute(id, args)`, `execute_from(origin, id, args)` and `execute_typed`.
9. **Registry list.** The catalog says "a plain list, no macro magic". `crates/engine/src/cmd/mod.rs` uses the small `families!` macro so that one line adds a family.
10. **Test locations and names.** The catalog puts all registry tests in `crates/engine/src/registry_tests.rs` and calls one `refusal_codes_are_catalogued`. The catalog tests are in `crates/engine/src/catalog_tests.rs`, and that test is `declared_refusal_codes_are_catalogued`.
11. **`Net` placement.** Section 1.11 of the catalog says `collab` is L5. After D2 in `docs/deviations.md` it is L4, and the engine owns the client. The `Net` trait in `crates/engine/src/ports.rs` is a placeholder until `collab` lands.
12. **Panic rollback on wasm.** D3 in `docs/deviations.md`: on `wasm32-unknown-unknown` a panic cannot be caught. The panic tests run natively only.
13. **Keymap check.** The comparison of section 6.1 with the registered shortcuts is `crates/engine/src/keymap_tests.rs`, not `catalog_matches_registry`. It covers registered commands only, both platforms, scopes and fixed arguments; the module doc lists the notation it understands (`mac ... / other ...`, `Left/Right/Up/Down`, `Arrow keys`, `key=a, b` paired with the chords in order). A new shortcut needs its row in section 6.1 in the same change.
