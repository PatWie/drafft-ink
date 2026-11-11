# Command catalog

Status: contract for the rewrite. Parallel agents implement one family each from this file, so every id, argument, refusal and undo class below is binding. A change to a command starts with a change here.

Companion: `docs/exceptions.md` lists the only things that are allowed to change state without one `Session::execute` call per user action (pointer gestures, camera moves, text caret).

Sources used: `docs/plan.md` §5, §6, §11; the old-app inventory (`research/drafftink.md`, `parts/app-actions.md`, `parts/ui-widgets.md`); the Excalidraw diff and actions research (`parts/04-features-diff.md`, `parts/03-actions-api.md`); groups (`parts/01-elements-groups.md`); bindings (`parts/02f-binding-rust-design.md`); solidcraft's command model (`research/solidcraft.md` Part A).

Abbreviations in the `source` columns:

| Short | Meaning |
|---|---|
| `A:n` | old `crates/drafftink-app/src/app.rs` line n (keyboard handler 4540-5369, `UiAction` match 2603-3667) |
| `U:n` | old `crates/drafftink-app/src/ui.rs` line n |
| `E:n` | old `crates/drafftink-app/src/event_handler.rs` line n |
| `X:name` | Excalidraw: `actions/<name>`, a component, or a key from its help dialog |
| NEW | not in the old app. Either an Excalidraw feature from plan §6 or an agent feature from plan §11 |
| FIX | exists in the old app but is wrong there; the catalog specifies the corrected behaviour |

Contents: 1 command model, 2 families, 3 read-only index, 4 UI decisions turned into arguments, 5 settings keys, 6 keymap, 7 old action to command mapping, 8 coverage of Excalidraw, 9 open questions, 10 declared facts per command (Appendix A).

---

## 1. The command model

### 1.1 Session and state classes

A `Session` (crate `engine`, L5) owns everything a user can change. Commands see it only through `&Session` (`available`) and `&mut Session` (`execute`). It holds these classes of state, and every command declares which classes it touches (`mutates`):

| Class | Contents | Persists | In undo history |
|---|---|---|---|
| Document (per tab) | elements, groups, frames, files, name, background colour. Loro-backed (`doc`). | yes, in the file and in the CRDT | yes |
| Observed editor state (per tab) | selection, selected groups, editing group, linear-editor point mode | no | yes, restored by undo (Excalidraw's `ObservedAppState`) |
| View (per tab) | camera (scroll, zoom), viewport size | no | no |
| Tool | active tool, tool lock, temporary tool, style defaults for new elements | defaults persist as a setting | no |
| Tabs | tab list, active tab | no | no |
| Settings | the keys in §5 | yes, via `Storage` | no |
| Gesture | the one running gesture, if any (§1.10) | no | one step on `gesture.end` |
| Text edit | the element or draft being edited (§1.10, `exceptions.md`) | no | one step on `text.edit_end` |
| Collab | connection, room, peers, presence | no | no |
| Ring | recent-command ring (§1.9) | no | no |

UI state is not in the Session: open menus, popovers, dialogs, hover, scroll position of panels, zen mode, the stats panel. See §1.12 (UI verbs).

### 1.2 `CommandSpec`

Every command is one `static CommandSpec`. Families keep their specs in `crates/engine/src/cmd/<family>.rs` and their tests in `cmd/<family>_tests.rs`. The registry is a plain list of the family `COMMANDS` slices (no macro magic).

```rust
pub struct CommandSpec {
    // identity
    pub id: &'static str,            // "family.verb", lower snake_case, one dot, unique
    pub title: &'static str,         // palette and menu text, sentence case
    pub family: &'static str,        // equals the part of `id` before the dot (tested)
    pub summary: &'static str,       // one sentence for `engine.describe` and MCP
    // contract
    pub args: fn() -> Schema,        // JSON Schema of the typed Args struct (schemars), deny_unknown_fields
    pub output: fn() -> Schema,      // JSON Schema of the typed Output struct
    pub available: fn(&Session) -> Result<(), Refusal>,
    pub execute: fn(&mut Session, Args) -> Result<Output, Refusal>,  // wrapper deserialises the typed args
    // history
    pub undo: Undo,                  // Undoable | NoUndo | Gesture
    pub coalesce: Option<Coalesce>,  // merge rapid repeats into one undo step
    // facts (every one is required; a test fails if a registration omits one)
    pub reads_only: bool,            // true: never changes any state class
    pub mutates: Mutates,            // bit set: Doc, Observed, View, Tool, Tabs, Settings, Collab, Ports
    pub needs_selection: Selection,  // No | Soft | Required (see below)
    pub ports: Ports,                // required ports: Clipboard, Storage, Files, Renderer, Net, Clock
    pub batch: Batch,                // Allowed | Last | Never
    pub view_only: Gate,             // Allowed | Refused  (allowed while the document is view-only)
    pub while_gesture: Gate,         // Allowed | Refused  (allowed while a gesture runs)
    pub while_text_edit: TextGate,   // Allowed | Refused | Commit  (Commit: the running text edit is committed first, then the command runs)
    pub redraw: Redraw,              // None | Overlay | Camera | Scene: the dirty reason on success
    pub ring: Ring,                  // Always | Quiet (Quiet: recorded only when it refuses)
    pub palette: bool,               // listed in the command palette
    pub agent: bool,                 // exposed as an MCP tool and in `window.drafftink.commands()`
    pub shortcuts: &'static [Binding], // default keys with fixed args (§1.12)
    pub status: Status,              // Stable | Planned("reason"): registered but refuses with `not_supported`
}
```

Facts that matter beyond the obvious:

- `undo`: `Undoable` pushes one undo step when the command changed the document, and pushes nothing when it did not (a no-op never creates a step). The step also records the observed editor state (selection, editing group) before and after. `NoUndo` never touches history (view, tool, settings, reads, undo itself, document replacement). `Gesture` commands run as `gesture.begin/update/end` (§1.10) and are one `Undoable` step at `end`; called plainly they behave as `Undoable`.
- `coalesce`: `Some({ key, ms })` merges a repeat of the same command id on the same targets (the elements the step changed) within `ms` of the previous repeat into the previous undo step. Used for nudge, font-size steps and zoom-free repeated style taps. Default `None`. Applies to a plain call only: the last step of a gesture and a batch never merge, and undo, redo, another command's step or a collaborator's edit ends the run.
- `needs_selection`: `Required` means the command has an `ids` argument whose default is the current selection, and an empty resolved list refuses with `no_target`. `Soft` means the selection is used only as a default when relevant and an empty selection is not an error (`style.set` then edits the defaults). `No` means the selection is not used.
- `ring = Quiet` keeps high-rate commands from flooding the ring: `view.set`, `view.set_viewport`, `view.pan`, `view.zoom_at`, `collab.set_pointer`, `collab.laser`, `gesture.update`. A Quiet command is written to the ring only when it refuses.
- `batch = Last` means the command may only be the final step of a batch (`doc.new`, `doc.open`, `doc.drop`, `doc.show_intro`, `tab.new`, `tab.switch`, `tab.duplicate`, `tab.close`). A batch holds one journal, the one of the tab it started in, so a command that makes another tab active must end it. `Never` means it cannot appear in a batch at all (gestures, `history.*`, `collab.connect`).
- `view_only = Refused` for every command whose `mutates` contains `Doc`.
- `while_text_edit = Commit` is how a click, a tab switch, `Mod+S` or a tool change ends a text edit without the UI having to call `text.edit_end` first: the engine commits the edit (one undo step), then runs the command.
- `redraw`: `Scene` re-encodes changed element fragments, `Camera` re-composites the cached fragments with a new transform (pan, zoom), `Overlay` redraws handles, guides and cursors only, `None` requests nothing. This is the dirty reason that `cargo xtask perf --idle` relies on.

Registry tests (in `crates/engine/src/registry_tests.rs`, all run in `cargo xtask ci`):

1. ids are unique, match `^[a-z]+\.[a-z_]+$`, and the prefix equals `family`.
2. Every fact above is declared. `reads_only` implies `mutates` is empty and `undo = NoUndo`. A command whose `mutates` contains `D` declares how it relates to the document (`doc_write`, Appendix A2): an `edit` refuses in view-only mode and has an undo class other than `NoUndo`; a command that `replaces` the document (`doc.new`, `doc.open`, `doc.show_intro`, `collab.join_room`) refuses in view-only mode and is not a gesture; the commands that are `managed` (`history.undo`, `history.redo`, the `gesture` commands, `tool.cancel`) carry no further rule. The rule is derived from the declared fact, not from a list of ids in the test, and a command that manages its own transaction is never an `edit` or a `replaces`.
3. Titles are unique. Default shortcuts do not collide within the same `when` scope.
4. Every `Refusal` code a command can return is in the catalog of §1.5 (checked by `refusal_codes_are_catalogued`, which scans the specs' declared `refusals: &[&str]` lists).
5. `hostile_params_never_panic`: every command, with every hostile JSON value for every argument key, never panics and never changes the document when it refuses.
6. `refusal_leaves_state_unchanged`: for every command, a refusal leaves all state classes byte-identical (document frontiers, selection, tool, view).
7. `catalog_matches_registry`: `cargo xtask commands --check` parses this file. In §2 (between the headings `## 2. Families` and `## 3.`), every table row whose first cell is a backticked id, appearing exactly twice per id (once in the identity table, once in the details table), must name a registry id, and every registry id must appear. It also compares the facts in Appendix A (§10) and the keymap in §6.1 with the registrations. §6.1 is the only source of default shortcuts: the `keys` cells in the §2 identity tables are summaries for the reader and are not parsed. Keep the first-cell format.

### 1.3 Arguments

Arguments are one typed Rust struct per command with `#[derive(Deserialize, JsonSchema)]` and `#[serde(deny_unknown_fields)]`. The handler receives the struct, not a `Value`. The JSON Schema is published by `engine.describe` and used to generate MCP tools. Unknown keys and wrong types become a `bad_args` refusal that names the key and, for unknown keys, the closest valid key.

In tables an argument is written `name: type = default`. `name?: type` means optional with no default (absent means "not given"). A default written as `selection` means "the current selection, resolved when the command runs".

Shared types:

| Type | Meaning |
|---|---|
| `Id` | element id, an opaque string. Group and frame ids are separate types (`GroupId`, element id of the frame). |
| `Ref` | an `Id`, or `"@name"` to address an element by the `name` set with `element.set_name`. Resolution happens in step 4 of §1.4. Unknown refs give `not_found`. |
| `Refs` | `Ref[]`. A group id is not a `Ref`; commands that accept groups have a separate `group: GroupId` argument. |
| `Point` | `[x, y]` in world units (f64). |
| `ScreenPoint` | `[x, y]` in logical pixels from the top-left corner of the canvas widget (before the device scale factor). |
| `Rect` | `{x, y, w, h}` in world units. |
| `Angle` | degrees, clockwise, f64. Internally radians. |
| `Color` | `"#rrggbb"`, `"#rrggbbaa"`, `"transparent"`, or a palette name `"<family>-<shade>"` such as `"blue-500"` (22 Tailwind families by shades 25 to 950, from the old `TAILWIND_COLORS`). |
| `FileRef` | `{path}` (native), `{stored}` (a name in the `Storage` documents namespace, used by the web app and by "recent documents"), `{download}` (a file name; the shell offers the bytes as a download), `{handle}` (an opaque id of a File System Access handle). |
| `Bytes` | `{base64, name?, mime?}` or `{path}`. Large payloads should use `{path}`. |
| `Query` | the filter object of `element.query` (§2.4). |
| `Tab` | tab id, index, or name. Omitted means the active tab. |
| `Source` | who called: `ui`, `keyboard`, `menu`, `context_menu`, `palette`, `script`, `mcp`, `test`. Recorded in the ring; `view_only` and `agent` gates may use it. Never changes the result of a command. |

Units: positions and sizes are world units; the old app's `GRID_SIZE` is 20. Opacity is `0..1`. Angles are degrees. Zoom is a scale factor (screen px per world unit); the UI shows `zoom / 1.68 * 100` as percent, which keeps the old "100% = BASE_ZOOM 1.68" display.

### 1.4 Execution

`Session::execute(id, args, source) -> Result<Output, Refusal>` is the only entry point for user actions. `execute_batch` calls the same steps. In order:

1. Look up `id`. Unknown ids refuse with `unknown_command` and up to three suggestions (same last segment, then edit distance).
2. Gates from facts: `view_only`, `while_gesture`, `while_text_edit` (a `Commit` gate commits the text edit here, before step 3).
3. `available(&Session)`. It depends on session state only, never on the arguments. A refusal here is the same text the palette shows greyed out.
4. Deserialise and validate args (`bad_args`), resolve `Ref`s (`not_found`), apply defaults, then check the targets: `no_target` for a `Required` command with an empty resolved list, and the per-command `too_few`/`needs_single`/`wrong_kind` checks. Explicit `ids` therefore work with an empty selection. For the palette and for disabled buttons the UI calls `Session::availability(id)`, which is `available` plus the target checks evaluated with the default arguments (the current selection), so a disabled button shows the same text that `execute` would return.
5. Open a transaction: remember Loro frontiers and clone the small non-document session state (selection, editing group, point editor, tool, view). Tabs, settings, library, buffers and ports are outside the transaction (§1.7).
6. Run `execute` inside `catch_unwind`.
7. On `Err` or panic: roll the document back to the remembered frontiers, restore the session state, record the refusal, return it. A panic becomes an `internal` refusal. A command never has to be atomic by hand. **wasm32 caveat:** `wasm32-unknown-unknown` builds with `panic=abort`, so a panic cannot be caught there. On the web the guarantee rests on typed errors and on `hostile_params_never_panic`, and the panic rollback is tested on native only. This differs from plan §5, which does not mention it (open question 15).
8. On `Ok`: run the invariant pass inside the same transaction (bindings, bound text, frame membership, group integrity, z-order indices; section 02f of the binding design). Then commit once, label the undo step with `title`, push it if `undo = Undoable` and something changed, apply `coalesce`, flush port writes, set the dirty reason from `redraw`, hand the incremental CRDT delta to `collab`, record the ring entry.

Commands never call each other through `execute`. Shared behaviour lives in the domain crates (`edit`, `doc`, `io`), and a command validates and delegates to it. The one allowed internal composition is `Session::run_nested(spec, args)`, which skips steps 5 to 8 and lets the outer command own them (used by `clipboard.cut`, which is a copy plus a delete in one step).

### 1.5 Refusals

```rust
pub struct Refusal {
    pub code: &'static str,        // stable snake_case, from the catalog below
    pub command: String,           // the id that refused
    pub message: String,           // plain words, one to three sentences, says how to proceed
    pub suggest: Vec<Suggestion>,  // machine-readable next steps: { command, args? }
    pub details: Option<Value>,    // e.g. offending ids, a parse position
}
```

Rules for the `message`:

- Plain words, no jargon, no stack traces. Name the thing and the value that was wrong.
- End with what to do: either the command to run (in backticks) or the argument to change.
- The same text goes to a toast, to the greyed palette item, and to the agent. The UI may add buttons from `suggest` but never rewrites the text.
- A state-dependent refusal (`available`) must not depend on the arguments, so the palette can show it. A refusal that depends on arguments comes from `execute` and is validated before any mutation.
- A command that finds the state already as requested (a style set to its current value, `element.lock` on a locked element, `tool.release` without a hold, `gesture.cancel` with none running) returns `Ok` with `changed: false` and pushes no undo step. A command whose whole purpose would be void refuses with `nothing_to_do` so that an agent notices a wrong assumption: nothing to clear, unlock, unbind, unwrap or export.
- A refusal with code `no_target`, `nothing_to_do` or `not_applicable` that comes from a keyboard shortcut is recorded in the ring but shown without a toast (pressing Delete with nothing selected is not an error). Every other refusal is shown.

Shared catalog. Placeholders in braces are filled in; the wording is exact.

| Code | Message |
|---|---|
| `unknown_command` | Unknown command `{id}`. Did you mean `{a}` or `{b}`? List commands with `engine.commands`. |
| `bad_args` | Invalid arguments for `{id}`: {detail}. See `engine.describe` for the schema. |
| `internal` | `{id}` failed unexpectedly and was rolled back, so the document is unchanged. Details: {msg}. |
| `no_target` | Nothing to act on: no elements are selected. Select elements first, or pass `ids`. |
| `not_found` | No element matches `{ref}`. Look up ids with `element.query` or `doc.outline`. |
| `locked` | Element `{id}` is locked. Unlock it first with `element.lock` (locked=false) or `element.unlock_all`. |
| `view_only` | The document is view-only, so `{id}` cannot change it. Turn view-only off with `view.set_view_only` (on=false). |
| `gesture_active` | A drag is in progress. Finish it with `gesture.end` or cancel it with `gesture.cancel` before running `{id}`. |
| `text_editing` | A text element is being edited. Finish with `text.edit_end` before running `{id}`. |
| `unsaved_changes` | This document has unsaved changes. Save it with `doc.save`, or pass on_unsaved="discard" to drop them, or on_unsaved="save" to save and continue. |
| `newer_schema` | This document was changed by a newer version of drafft. Update the app to edit it. |
| `wrong_kind` | `{id}` does not work on {kind} elements. It works on {allowed}. |
| `not_applicable` | None of the targeted elements support {props}. They apply to {kinds}. |
| `too_few` | `{id}` needs at least {n} {things}, and {have} were given. Select more, or pass `ids`. |
| `port_unavailable` | `{id}` needs {port}, which this session does not provide ({where}). {alternative} |
| `io_failed` | Could not {read\|write} {target}: {msg}. Check the location and permissions, or pass the data as `bytes`. |
| `parse_failed` | {what} could not be read: {msg}. {hint} |
| `limit_exceeded` | `{id}` was given {n} {things}, and the limit is {max}. Split the work into several calls. |
| `needs_single` | `{id}` needs exactly one {thing} selected, and {have} are selected. Select one {thing} first, or pass it by id. |
| `nothing_to_do` | Nothing to {verb}: {reason}. |
| `not_supported` | `{id}` is registered but not available yet: {reason}. |
| `render_pending` | The {what} is being drawn and is not ready yet. The app runs `{id}` again when it is; a script that talks to the engine directly runs it again too. |

Command-specific codes appear inline in the §2 tables as `code: "message"`. They follow the same rules and are added to the catalog check (test 4).

### 1.6 Output

A successful command returns a JSON object defined by its `output` schema. Every output has these common keys:

```json
{ "changed": true, "undo_label": "Move", "created": ["id..."], "deleted": ["id..."], "warnings": [] }
```

`created`/`deleted` list element ids when the command made or removed any. `warnings` carries non-fatal notes (an unsupported property was skipped, a port write after commit failed). Commands add their own keys (`ids`, `bounds`, `guides`). Read-only commands return only their payload plus `changed: false`.

### 1.7 Batches

`Session::execute_batch(steps, opts) -> BatchOutput`.

```json
{ "steps": [ { "id": "element.create", "args": { "kind": "rectangle", "bounds": {"x":0,"y":0,"w":120,"h":60} }, "as": "box" },
             { "id": "style.set", "args": { "ids": ["$box"], "props": { "fill_color": "blue-100" } } } ],
  "opts": { "label": "Draw box", "atomic": true, "dry_run": false } }
```

- **Preflight.** Before step 0 runs, every step is checked for: known id, `batch != Never`, `batch = Last` only on the final step, args that deserialise against the schema (symbolic `$refs` count as valid strings), duplicate `as` names, `$refs` that point to an earlier step. Preflight cannot know state-dependent availability of later steps, so it checks only step 0's `available`. A preflight failure returns `{completed: 0, failed: {index, id, refusal}, note: "nothing was run"}` and changes nothing.
- **Atomic.** All steps run in one transaction. The first refusal rolls back document and session state to the start of the batch (selection, tool, view included) and returns `{completed: i, failed: {index, id, refusal}, rolled_back: true}`. With `atomic: false` the steps before the failure stay applied and still form one undo step.
- **One undo step.** The whole batch commits once, with `opts.label` (default: the first step's title plus the count). `history.undo` reverts all of it.
- **Symbolic references.** `as: "name"` binds the step's `created[0]` (or its `ids` output). Later args may use `"$name"` anywhere an `Id`/`Ref` is accepted, `"$name.created"` for the whole list, and `"$1"` for the output of step 1 (0-based). `diagram.create` additionally exposes its symbolic node ids as `"$name.<node>"`.
- **Ports.** Port writes (file, clipboard, download) from steps are queued and flushed only after the last step succeeds, so a rolled-back batch has no side effect.
- **What rollback covers.** The document, selection, editing group, point editor, tool and view. Tabs, settings, the library and the clipboard buffers are outside the transaction. A step that changes them (`mutates` has B, S or P) is allowed, is not undone by a rollback, and is listed in the result under `not_rolled_back` so the caller knows.
- **`execute_batch` is not a command.** It contains commands. MCP exposes it as the tool `batch`, and `window.drafftink` as `executeBatch`.
- **Limits.** 10 000 steps, 10 MB of JSON. `limit_exceeded` otherwise.
- **Not allowed inside a batch:** commands with `batch = Never`. `doc.new`, `doc.open`, `doc.drop`, `doc.show_intro`, `tab.new`, `tab.switch`, `tab.duplicate` and `tab.close` must be the last step (`batch = Last`). The executor also checks at run time: a step that made another tab active (`library.import` with `mode: open_as_tab`) while steps follow stops the batch with `must_be_last`, and an atomic batch rolls back the tab it started in.
- `dry_run: true` runs preflight and then the steps, reports the outputs, and rolls everything back.
- **A picture that is still being drawn.** On the web a PNG export answers `render_pending` until the GPU has read the picture back (`web-app.md`). A batch treats that as a wait, not a failure.
  - **When no later step can change the picture** (every step after it is `reads_only` or mutates only ports: another export, a screenshot), the batch carries on and completes as a whole, like a `doc.save` whose write the port finishes later: it commits as one undo step, and the result lists the waiting steps as `pending: [{index, id, token}]`, with `{pending_render: token}` as their entry in `results`. The shell hands each token to `Session::finish_render` when the renderer wakes it. That runs the step as a command of its own, on the document the batch left, and returns its output (`Finished::Done`) or `Finished::Waiting`. The bridge does this by itself and answers the caller with the finished result, so a caller of `window.drafftink.batch` sees the picture in `results` and nothing else. A step with `settings`, `tabs` or other steps outside the transaction before it no longer stops the wait.
  - `render_stale`: the document, the selection or the active tab changed between the end of the batch and the moment the picture was read back, so the step was not run for a state it would not show. The rest of the batch stays applied; the bridge answers with `failed: {index, id, refusal}` and `null` at that index. `render_unknown`: `finish_render` was given a token that is not waiting.
  - **When a later step does change the picture** (or with `dry_run`), the old rule applies: the batch takes every step back (also with `atomic: false`), returns `pending_render: true` with the refusal in `failed`, and changes nothing. Run the same batch again when the renderer wakes the shell. The bridge does this by itself. If a step outside the transaction already ran, the batch does not wait; it fails with `render_pending`.
- Output: `{completed, results: [Output...], failed?, rolled_back, undo_label, pending?, pending_render?}`.

### 1.8 Undo

- Local, per-peer undo using Loro's `UndoManager`. One `execute` or one batch or one gesture is one undo step. Undo of a step restores the document and the observed editor state (selection, editing group) as of before the step.
- Undo is a new edit for collaborators and travels the same CRDT path as any edit. This fixes the old divergence where only the UI path broadcast an undo.
- Depth is the `history.depth` setting (default 200, range 10 to 1000; decision §9 item 3). Redo clears only when a new document change is made, not when the selection changes.
- Selection changes alone never create a step (clicking around must not fill the history). The next document step records the selection from before and after it, so undo restores what was selected.
- Property changes (style, font, flip, align, z-order) are undoable. In the old app most were not (FIX).
- A rapid repeat of a command with `coalesce` merges into the previous step.
- `history.undo` and `history.redo` refuse during a gesture or a text edit.

### 1.9 Recent ring

A bounded ring (default 256 entries, `engine.recent`) of every top-level `execute`, `execute_batch` and gesture:

```json
{ "seq": 412, "t_ms": 1730000000000, "id": "element.move", "args": "{\"ids\":[\"a\"],\"dx\":20,...}",
  "source": "keyboard", "outcome": { "ok": true, "changed": true, "undo_label": "Move" }, "duration_us": 310 }
```

- `args` is the JSON text truncated to 512 bytes. Image bytes are replaced by their length.
- A refusal is recorded with its `code` and `message`: `outcome: { "ok": false, "code": "locked", "message": "..." }`.
- A batch is one entry with `steps: n`. A gesture is one entry written at `end` or `cancel`, with `frames: n` (live frames are not recorded).
- Read-only commands are recorded too (an agent debugging its own calls needs them) but `engine.recent` can filter them with `mutating_only: true`. Commands with `ring = Quiet` (§1.2) are recorded only when they refuse.
- The ring is session state, not document state. Remote edits do not appear in it.

### 1.10 Gestures

A gesture is a user action that streams many small updates and must end as one undo step: dragging, resizing, rotating, dragging a line point, scrubbing a slider.

```
gesture.begin  { command, args }       -> { gesture: "g1", ...output of the first frame }
gesture.update { gesture, args }       -> live frame; output may include overlay data
gesture.end    { gesture, args? }      -> same as execute(command, args), committed once
gesture.cancel { gesture? }            -> restores the state from before gesture.begin
```

Rules:

1. Only commands whose spec has `undo = Gesture` can be begun: `element.move`, `element.resize`, `element.rotate`, `arrow.move_point`, `arrow.move_segment`, `style.set`. Calling any of them plainly is also valid and is the same final result.
2. **Frames are absolute.** Every `gesture.update` carries the full args relative to the state at `begin` (for move: the total `dx, dy` since the start), never an increment. The engine keeps the pre-gesture values of every element a frame touches. A frame restores those values and re-runs the command with `live = true`. Dropping or reordering frames is harmless.
3. `live = true` is an internal flag that the gesture machinery sets on the typed args (it is not in any published schema). It skips work that is only needed for the final state: undo bookkeeping, collab broadcast, the library, and expensive re-measures. It does not skip the binding solver or snapping, so bound arrows follow a dragged shape and smart guides are computed. Overlay data (`guides`, `bind_highlight`, `angle_badge`, `size_badge`) comes back in the frame output for the renderer.
4. `gesture.end` re-runs the command from the base state with `live = false`, commits once, labels the step with the command title, and broadcasts once. **Property:** `begin + any updates + end(args)` produces the same document as `execute(command, args)` on the base state (ids and timestamps aside). A test per gesture command checks this.
5. One gesture at a time per session. A second `begin` refuses `gesture_active`. While a gesture runs, commands with `while_gesture = Refused` refuse. Allowed are: reads, `view.*` camera moves, `collab.set_pointer`, `engine.*`, and `gesture.*`.
6. The UI cancels a gesture on Esc, window blur, pointer cancel, a switch to view-only mode (`view.set_view_only` cancels a running gesture), and when the pointer is released over a panel (this fixes the old stuck-drag bug, A:4259). The engine also cancels a gesture that received no frame for 30 seconds (`Clock` port).
7. A refusal inside `update` is returned but leaves the last good frame in place. A refusal at `end` aborts the gesture and restores the base state.
8. Gestures cannot be used in batches. Concurrent remote edits merge normally; `end` re-runs against the current state, so remote changes to other fields of the same elements are kept.
9. Live frames are not recorded in the ring and are not broadcast to collaborators. Peers see the result at `end`.
10. The UI calls the typed in-process path (`Session::gesture_update(GestureId, TypedArgs)`) so no JSON is built per pointer event. The JSON route is for the bridge.

Gestures that do not use this protocol are listed in `docs/exceptions.md`, each with its reason and the command that reaches its final state.

### 1.11 Ports

Commands never touch the OS, the network or the UI. Everything outside the process comes through traits in `crates/engine/src/ports.rs`, passed in `Session::new(ports)`. A missing optional port makes the commands that need it refuse with `port_unavailable`. The testkit provides in-memory fakes of all of them.

```rust
pub trait Clipboard {
    fn read(&self, want: &[ClipKind]) -> Result<Option<ClipPayload>, PortError>; // Elements, Text, Image, Svg
    fn write(&mut self, payload: ClipPayload) -> Result<(), PortError>;
}
pub trait Storage {   // namespaces: Documents, Autosave, Settings, Library
    fn get(&self, ns: Ns, key: &str) -> Result<Option<Vec<u8>>, PortError>;
    fn put(&mut self, ns: Ns, key: &str, bytes: &[u8]) -> Result<(), PortError>;
    fn put_later(&mut self, ns: Ns, key: &str, bytes: Vec<u8>) -> Result<(), PortError>;  // a write the caller does not wait for (default: put)
    fn take_write_error(&mut self) -> Option<PortError>;                                    // the first failure of a put_later, once
    fn delete(&mut self, ns: Ns, key: &str) -> Result<(), PortError>;
    fn list(&self, ns: Ns) -> Result<Vec<StoredInfo>, PortError>;               // name, size, modified_ms
}
pub trait Files {
    fn read(&self, file: &FileRef) -> Result<Vec<u8>, PortError>;
    fn exists(&self, file: &FileRef) -> bool;
    fn write(&mut self, file: &FileRef, bytes: &[u8], mime: &str) -> Result<FileRef, PortError>; // Download = hand to the shell
}
pub trait Clock { fn now_ms(&self) -> u64; fn mono_ms(&self) -> u64; }          // wall time for timestamps, monotonic for timing
pub trait Ids   { fn element_id(&mut self) -> Id; fn seed(&mut self) -> u32; }   // random in the app, sequential in tests
pub trait Renderer {                                                              // offscreen render, for PNG export and screenshots
    fn render_png(&self, scene: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError>;
}
pub trait Net { /* the collab Transport: connect, send, poll; defined in crate collab */ }
```

Rules:

- **No awaiting.** A command is synchronous. Anything asynchronous is resolved by the shell before the call and passed as arguments, or is already present in the port. The web shell fills the `Clipboard` port from the browser `paste` event, which delivers data synchronously, before it calls `clipboard.paste`. If the browser only gives async access, the shell reads first and passes `clipboard.paste { source: {text} }` or `{image}`.
- **No dialogs.** File pickers, confirm dialogs and prompts live in the UI. The UI passes the chosen path or bytes. A decision that used to be a dialog is an argument with a safe default (§4).
- **Pure domain parts stay pure.** SVG export, `.excalidraw` read/write, Mermaid parse, layout are `io` and `edit` functions without ports. Only the PNG render needs `Renderer`.
- **Time and randomness** come from `Clock` and `Ids`, so scenarios replay deterministically.
- `Net` is owned by `collab` (L5). `engine` holds a handle to it. Remote edits enter through `Session::apply_remote(delta)`, which is not a command (§1.12) but uses the same document and CRDT path as local edits.

### 1.12 Keymap, UI verbs, surfaces

**Shortcuts** are data in each spec: `Binding { keys: &[Chord], args: json, when: Scope, title: Option<&str> }`. A binding fixes some args of the command, so one command can have many bindings (`tool.set` has one per tool). The palette lists one entry per binding that has a `title`. Notation in this file:

- `Mod` is Cmd on macOS and Ctrl elsewhere. `Alt` is Option on macOS. `Ctrl` means the physical Control key on every platform. `Shift` is Shift.
- Where macOS and others differ, the cell reads `mac: ... / other: ...`.
- The old app recognised only the physical Control key, so Cmd did nothing on macOS (FIX). Every `Ctrl+x` of the old app is `Mod+x` here.
- `when`: `canvas` (focus on the canvas, no text edit), `text` (inside a text edit; handled by the editor, listed in `exceptions.md`), `always`.
- Placeholders in binding args: `$cursor` is replaced by the shell with the pointer's world position (omitted when the pointer is outside the canvas), `$viewport_center` likewise.
- Matching: letters and the unmodified tool keys match the logical key (so AZERTY users get the letters they see). Digits, punctuation, and every chord that includes Alt or a shifted digit or punctuation key match the physical key code, named by its US layout (`Digit1`, `BracketLeft`, `Period`, `Comma`, `Quote`, `Equal`, `Minus`, `Slash`, `KeyH`), because on macOS Option+H produces a different character and Shift+1 produces `!`. Arrow, function, page and editing keys are physical. A test replays the keymap on US, German, French (AZERTY) and macOS layouts.
- A shortcut is never handled while an egui text field has focus.

**UI verbs** are not commands. They are the thin set of actions that open or close UI (dialogs, popovers, panels, pickers). They are in the same keymap so conflicts are detected, they are never in `engine.commands` and never exposed to agents, and they may only call commands. The full list is in §6.3. Rule of thumb: if it needs a dialog, a picker or a panel, it is a UI verb that ends by calling a command with the chosen values.

**Surfaces.** Toolbar buttons, menus, the context menu, shortcuts, the palette, the bridge, MCP, `window.drafftink` and tests all call `Session::execute`. A button reads `engine.commands` for title, shortcut and availability. A disabled control shows the refusal message as its tooltip. The UI shows a refusal returned by `execute` as a toast (text unchanged) and may attach buttons from `suggest`. There is no "one action per frame" bottleneck (FIX of U:645): the UI collects all user actions of a frame and executes them in order.

**Not commands.** Remote edits, presence updates and window events are applied by the engine without being user actions: `Session::apply_remote`, `Session::set_remote_presence`, and `Session` ticks (autosave timer, gesture watchdog, collab poll). `Session::apply_remote` writes one ring entry per received batch (id `remote.apply`, origin `remote`); the other two appear only when they fail.

### 1.13 Views of the catalog

For each family in §2 the first table gives identity, undo class, keys and source. The second table gives arguments, availability with refusal text, and effect. Rows named in `docs/exceptions.md` as the final command of a gesture say so in the effect.

---

## 2. Families

Twenty-three families. Each table row's first cell is the backticked id (checked by `catalog_matches_registry`). Facts that are the same for every command of a family are not repeated; the per-command fact values are in Appendix A (§10).

How to read the "availability and refusals" column: text before `Execute:` is a state condition checked by `available` (it depends on session state only). Codes after `Execute:` come from step 4 or from the effect, after the arguments are known. The generic target codes `no_target`, `too_few` and `needs_single` also come from step 4, but the UI evaluates them with the default arguments (the current selection) through `Session::availability`, so a disabled control shows them too. A code written with a text (`code: "message"`) uses exactly that text; a code written alone uses the §1.5 catalog text.

Shared enums used in the tables (these are the exact accepted strings):

```
Tool        = select | hand | rectangle | diamond | ellipse | arrow | line | freehand | highlighter | text | math | frame | eraser | laser
Kind        = rectangle | diamond | ellipse | line | arrow | freehand | text | math | image | frame
Snap        = auto | none | { grid?: bool, smart?: bool, angle?: bool }   (auto: follow the settings snap.grid, snap.smart_guides, snap.angle; angle snaps by snap.angle_step)
OnUnsaved   = refuse | discard | save            (default refuse)
Handle      = nw | n | ne | e | se | s | sw | w
Query       = { ids?, text?, kind?, style?, region?, connected_to?, group?, frame?, name?, locked?, selected?, has_label? }
Head        = arrow | bar | circle | circle_outline | triangle | triangle_outline | diamond | diamond_outline
              | cardinality_one | cardinality_many | cardinality_one_or_many | cardinality_exactly_one
              | cardinality_zero_or_one | cardinality_zero_or_many
StyleProps  = { stroke_color, fill_color, fill_pattern, stroke_width, stroke_style, sloppiness, corner_radius, opacity,
                freehand_style, pressure_mode, font_family, font_weight, font_size, text_align, vertical_align, line_height }
```

The old numeric enums map as follows (they were bare `u8` in `UiAction`): sloppiness 0..3 = architect, artist, cartoonist, drunk; fill pattern 0..6 = solid, hachure, zigzag, cross_hatch, dots, dashed, zigzag_line; path style 0..2 = sharp, round, elbow (`arrow.set_type`); stroke style 0..2 = solid, dashed, dotted; font family 0..3 = gelpen, noto_sans, gelpen_serif, vanilla_extract; font weight 0..2 = light, regular, heavy.

### 2.1 `doc`: documents

A document is the content of a tab. "Dirty" means the document revision differs from the revision last saved or opened; a freshly opened, blank or intro document is not dirty.

| id | title | undo | keys (mac / other) | source |
|---|---|---|---|---|
| `doc.new` | New document | NoUndo | none | NEW (A:2841 is `doc.clear`) |
| `doc.open` | Open document | Undoable | `Mod+O` is the UI verb `ui.open_file`, which calls this | A:4790, 2749, 2766, 2789; U:1907-1912 |
| `doc.drop` | Open dropped files | Undoable | none (drag and drop) | A:5376-5468; file_ops 939+ |
| `doc.save` | Save | NoUndo | `Mod+S` | A:4784, 2698, 2726, 2760, 2776 |
| `doc.rename` | Rename document | Undoable | none | NEW (A:2726 sets the name on save) |
| `doc.set_background` | Set background colour | Undoable | none | A:3237, U:1007; FIX (was not saved or exported) |
| `doc.export` | Export | NoUndo | `Mod+E` (png, selection or all) | A:4870, 3691; X:actionExport |
| `doc.clear` | Clear document | Undoable | `Mod+Delete` | A:2841; X:actionClearCanvas |
| `doc.show_intro` | Show intro document | NoUndo | none | A:2855 |
| `doc.autosave` | Autosave now | NoUndo | none (timer) | A:2564-2576; FIX |
| `doc.list` | List saved documents | NoUndo, read-only | none | A:2710, 2718 |
| `doc.inspect` | Inspect document | NoUndo, read-only | none | NEW §11 |
| `doc.outline` | Outline document | NoUndo, read-only | none | NEW §11 |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `doc.new` | `name?: string`, `on_unsaved: OnUnsaved = refuse`, `in_new_tab: bool = false` | Always available. Execute refuses: `empty_name` (as `doc.rename`); `too_many_tabs` (as `tab.new`) with in_new_tab=true; `unsaved_changes` (catalog text) when the active document is dirty, on_unsaved is refuse and in_new_tab is false. `in_room`: "This tab is connected to room {room}, and a new document would replace the shared one. Leave the room with `collab.leave_room`, or pass in_new_tab=true to open the new document in a new tab." | Replaces the active tab's document with a blank one (named "Untitled", background from the setting `doc.default_background`; history cleared, selection cleared, save target cleared, camera reset; a text edit is committed first and a drag is refused by the gates) or opens it in a new tab. A blank document is not dirty. on_unsaved=save runs the `doc.save` logic first and fails with `no_save_target` if there is no target. |
| `doc.open` | `source: FileRef \| Bytes`, `format: "auto" \| "drafftink" \| "excalidraw" \| "png" = auto`, `mode: "replace" \| "new_tab" \| "merge" = replace`, `on_unsaved: OnUnsaved = refuse`, `at?: Point`, `select: bool = true` | Always available. Execute refuses: `io_failed` (catalog); `parse_failed`: "The file is not a drafft.ink or Excalidraw document: {msg}. Open a .json, .drafftink or .excalidraw file, or a PNG exported with the scene embedded."; `is_library`: "`{name}` is an Excalidraw library, not a document. Add it to the library with `library.import`."; `newer_version`: "This file was saved by a newer drafft.ink (format {n}), and this one reads up to format {max}. Update the app to open it."; `unsaved_changes` (replace mode only) and `in_room` (replace mode only) as in `doc.new`, with the last words of `in_room` changed to "or pass mode=new_tab to open the file in a new tab"; `no_save_target` (on_unsaved=save, as `doc.new`); `too_many_tabs` (new_tab, as `tab.new`). Mermaid text is `parse_failed` with a suggestion of `diagram.import_mermaid`. | Reads and migrates the file (old drafft-ink JSON without a version field loads through the `io` migration). replace: loads into the active tab, clears history, selection, gesture and text-edit state, restores the saved camera or fits to content. new_tab: opens a tab. merge: inserts the file's elements with new ids, new group ids and new seeds (the `edit::library` insert rule: bound text, frame membership and bindings among them follow, references to elements outside the file are cut, names are dropped) on top of the z-order, with the block centred on `at` (default: original coordinates), selects them, one undo step. Replace and new_tab push no undo step (the history of a replaced document is cleared); the command is `Undoable` for the sake of merge. A PNG with an embedded scene (zTXt `application/vnd.drafftink+json`, or Excalidraw's tEXt) restores that scene. `{stored:"autosave"}` restores the last autosave: the document of the active tab in the workspace record (§2.2), marked as having unsaved changes (this is what `Ctrl+O` did on the web, A:373-398). A drafft.ink file opened from a path, a stored name or a handle becomes the save target of the tab; any other file (Excalidraw, old drafft-ink JSON, gzip or snapshot, PNG or SVG with a scene) does not, so `doc.save` then needs `to` and never overwrites it with drafft.ink JSON. Every file opened from a path, a stored name or a handle becomes the first entry of the recent list; opened bytes and downloads do not. |
| `doc.drop` | `items: {name: string, bytes: Bytes, mime?: string}[]`, `at: Point = viewport_center`, `scene_png: "insert" \| "open" = insert`, `documents: "new_tab" \| "replace" \| "merge" = new_tab`, `on_unsaved: OnUnsaved = refuse` | Always available. Execute refuses: `nothing_to_do`: "Nothing to open: none of the {n} dropped files is an image, document or library. Supported: PNG, JPEG, WebP, .json, .drafftink, .excalidraw, .excalidrawlib."; `limit_exceeded` (more than 50 items, or more than 20 images); `in_room`, `no_save_target`, `too_many_tabs` as in `doc.open` (documents=replace or new_tab). | Routes each item by sniffed content, not by extension alone. PNG/JPEG/WebP go to the `image.insert` logic (a PNG with an embedded scene is inserted as an image unless scene_png=open, which restores the scene as the old app did and is then the same as `doc.open`). Documents go to `doc.open` with `documents` as the mode. `.excalidrawlib` goes to `library.import` (merge). Several images are placed with a 20 unit cascade (FIX: the old app stacked them, A:5376). Unsupported items are skipped and listed in `warnings`; so is a document that cannot be read, and the images when `image.insert` is not registered in the session. A drop that opens a document in replace mode clears the history, so no undo step remains for it; an image drop is one undo step. batch: Last. |
| `doc.save` | `to?: FileRef`, `format: "drafftink" \| "excalidraw" = drafftink`, `remember: bool = true`, `overwrite: bool = false` | Refused with `room_loading` while a room is still being decoded in slices: "The room is still loading ({done} of {total} items), so `doc.save` would leave elements out. Wait until it has loaded, then run `doc.save` again." Execute refuses: `no_save_target`: "This document has no save location yet. Pass `to` (a path, a stored name or a download name)." The web shell configures a default target, `{stored: <document name>}`, so `Mod+S` works there without a dialog; the native UI reacts to this code by opening its picker and calling again with `to`. `would_overwrite`: "`{target}` already exists and is not this document's file. Pass overwrite=true to replace it."; `io_failed` (catalog); `port_unavailable` when the port for `to` is missing. | Serialises (versioned JSON, images embedded; the excalidraw format warns about dropped fields), writes through `Files` or `Storage`, sets the saved revision (document is no longer dirty), and with remember=true stores `to` as the save target. Adds the target to the recent list (10 entries). The autosave of the web is the workspace record (§2.2), written by the engine; `doc.save` no longer has a slot of its own (A:343). The web shell turns on `doc::use_stored_default_target`, a session setting that is not a command. The write is queued and runs after the command. The saved state follows the result: a failed write refuses with `io_failed` ("The document was NOT saved to ...") and leaves the document dirty, with the save target and recent list unchanged. A port that finishes later (the browser; `Files::write` or `Storage::put_document`, which defaults to a synchronous `put`) answers pending: the output has `pending_write` and `changed: false`, the document stays dirty, and the shell calls `Session::on_write_done(token, result)` to mark it saved (or to report the failure). `on_unsaved=save` writes at once and refuses unless the write is done. Replaces the dialog-driven native save and the IndexedDB save of the old app (one code path). |
| `doc.rename` | `name: string` | Always available. Execute refuses: `empty_name`: "The name is empty. Give a name of 1 to 256 characters." | Sets the document name (the `tab.rename` rule: trimmed, cut to 256 characters, no control characters: `bad_args`). The same name is Ok with changed=false. The tab title follows. |
| `doc.set_background` | `color: Color` | Always available. | Sets the background colour stored in the document (saved, synced, used by export and screenshots). `transparent` is allowed. The default for new documents is the setting `doc.default_background` (`#ffffff`; the old app had two defaults, `#fafafa` and white). |
| `doc.export` | `format: "png" \| "svg" \| "json" \| "excalidraw" = png`, `scope: "auto" \| "all" \| "selection" \| "ids" \| "frame" = auto`, `ids?: Refs`, `frame?: Ref`, `scale: number = settings export.scale`, `padding: number = settings export.padding`, `background: bool = settings export.background`, `dark: bool = settings export.dark`, `embed_scene: bool = settings export.embed_scene`, `to: "inline" \| FileRef = inline` | Refused with `room_loading` while a room is still being decoded in slices (as `doc.save`). Execute refuses: `no_target` for scope selection or ids with nothing selected; `nothing_to_do`: "Nothing to export: the document has no elements. Draw something first, or pass `ids`."; `port_unavailable`: "`doc.export` with format png needs the renderer, which this session does not provide (headless without a GPU). Use format svg or json." (png only); `render_pending` (the web: the picture is still being drawn; the shell runs the command again, see `web-app.md`); `io_failed`. | auto means the selection if there is one, else everything (as the old app did). Renders at `scale` (0.25 to 4; the UI offers 1, 2, 3) with `padding` units around the content and the document background. png embeds the scene as zTXt `application/vnd.drafftink+json` when embed_scene is true, and can be re-opened with `doc.open`. `inline` returns `{bytes: {base64, mime}}`. `{download}` and `{path}` write through `Files`. This is the single export implementation (the old app had five copies, A:3691, 3756, 4801, 4870, 4992). A scope=frame export clips to the frame and omits other frames' labels. svg embeds the font files (full, not subset) of the families and weights its texts use, as base64 `@font-face` rules (Noto Sans only where it is built in, so not on the web), and with `dark` recolours every colour with the mapping of the dark canvas (`Color::for_dark_theme`); the embedded scene keeps the original colours. `clipboard.copy_as` svg goes through the same function. |
| `doc.clear` | none | `empty_document`: "The document is already empty, so there is nothing to clear." | Deletes every element (including locked ones), clears the selection, keeps name, background and files table entries until garbage collection. One undo step. In a room it deletes for everyone, like any edit. |
| `doc.show_intro` | `mode: "replace" \| "new_tab" = replace`, `on_unsaved: OnUnsaved = refuse` | Always available. Execute refuses `unsaved_changes`, and `in_room` as in `doc.new` with "pass mode=new_tab" in place of in_new_tab=true; `no_save_target` (on_unsaved=save); `too_many_tabs` (mode=new_tab). | Loads the built-in intro document (`assets/intro.json`). Replace resets the camera, history and selection. |
| `doc.autosave` | none | `port_unavailable`: "`doc.autosave` needs persistent storage, which this session does not provide (storage port missing)." | The command door to the workspace record of §2.2 (decision §9.6): calls `Session::save_workspace`, which writes the tabs that changed since the last write (by revision and name) and then the index. Output `{saved, tabs}`: saved is true when a document was written. Refuses `workspace_newer`: "The stored workspace was written by a newer drafft.ink (format {n}), so it is left alone. Update the app to use it." The timer, the debounce and the write on window close are `Session::workspace_tick` and `flush_workspace` (not commands). FIX: the old detector counted shapes only (A:2564). |
| `doc.list` | `from: "recent" \| "stored" \| "autosave" = recent` | Always available. | Returns `[{name, ref: FileRef, modified_ms, size}]`. `recent` is the setting `doc.recent` (the last 10 saved or opened documents); `stored` is the web document store; `autosave` is the active tab of the workspace record, as `{stored:"autosave"}`. `modified_ms` and `size` are null when unknown. `stored` and `autosave` need the storage port. |
| `doc.inspect` | `ids?: Refs`, `detail: "summary" \| "full" = summary`, `include_deleted: bool = false`, `limit: int = 200`, `offset: int = 0` | Always available. `not_found` for unknown ids. | Summary: name, revision, dirty, save target, background, tab, `elements`, `labels`, counts per kind, bounds, group and frame counts, `bound_arrows` (arrows with at least one end bound), locked count. `elements` counts what `element.query` counts by default: a bound label follows its container and is not counted, so a rectangle with a label is 1 element. `labels` is the number of bound labels, and `kinds`, `groups`, `frames`, `locked` and `bounds` leave them out too. With detail `full`, `total` lists every element, so it is `elements + labels`. Full adds every field of each element (id, kind, geometry, angle, style, group ids, frame id, bindings, bound text, name, locked, z index). |
| `doc.outline` | `format: "text" \| "json" \| "both" = both`, `scope: "all" \| "selection" \| "visible" \| "frame" = all`, `frame?: Ref`, `max_nodes: int = 500` | Always available. | A compact structure for agents: nodes (id, kind, name, label text, bounds), edges (from, to, label, arrowheads, type), groups (group id with member ids), frames (frame with children), unconnected text. The text form is one line per node and per edge, for example `a "Start" rectangle (0,0,120x60)` and `a -> b "yes"`. Truncates at `max_nodes` and says so. |

### 2.2 `tab`: canvas tabs

A tab holds one document with its own selection, camera and history (the old app cleared history on switch and left stale text-edit and gesture state, A:1725; FIX). Tab commands never touch the history of the tab they leave. Tabs, their order and the active tab are outside the rollback transaction (§1.7). A tab is named in arguments as a `Tab`: a number is the position shown by `tab.list` (counted from 0), text is a tab id such as `t2`, else a tab name (the first tab with that name). At most 50 tabs are open.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `tab.new` | New tab | NoUndo | none | NEW (old: only via library import, A:1734) |
| `tab.switch` | Switch tab | NoUndo | none | U:691, A:3675 |
| `tab.close` | Close tab | NoUndo | none | U:705, A:1746 |
| `tab.rename` | Rename tab | Undoable | none | NEW |
| `tab.move` | Move tab | NoUndo | none | NEW |
| `tab.duplicate` | Duplicate tab | NoUndo | none | NEW |
| `tab.list` | List tabs | NoUndo, read-only | none | NEW |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `tab.new` | `name?: string` (1 to 256 characters after trimming), `from: "blank" \| "intro" \| "active" = blank` | `gesture_active` (catalog) while a gesture runs. Execute: `empty_name` (as `tab.rename`); `too_many_tabs`: "There are already 50 tabs, which is the limit. Close one first with `tab.close`." | Adds a tab after the active one and activates it. A running text edit ends first (committed). blank: an empty document named "Untitled" with the background of the setting `doc.default_background`. intro: the built-in intro document (`doc::intro_document`, as `doc.show_intro`), clean. active: a copy of the active tab (content, selection and camera; no history, no room, no save target; dirty when the source is dirty), named "<name> copy". The new tab takes over the viewport size of the tab it was opened from. |
| `tab.switch` | `tab: Tab` | `no_such_tab`: "There is no tab `{tab}`. List tabs with `tab.list`." `gesture_active`. | Ends a running text edit (committed), leaves the linear point editor of the tab being left, and activates the tab. The tab entered takes over the viewport size. Does not clone documents. Collab rooms stay bound to their tab. Switching to the active tab is Ok with changed=false. Hover is UI state and is cleared by the shell. |
| `tab.close` | `tab?: Tab`, `on_unsaved: OnUnsaved = refuse`, `leave_room: bool = false` | `last_tab`: "The last tab cannot be closed. Open another tab first with `tab.new`, or clear this document with `doc.clear`." `unsaved_changes` (catalog) when the tab is dirty and on_unsaved is refuse. `in_room`: "Tab `{name}` is connected to room {room}. Leave the room first with `collab.leave_room`, or pass leave_room=true." `no_such_tab`. `bad_args` when on_unsaved=save names a tab that is not the active one. | Closes the tab and activates its right neighbour (the left one if it was last), as the old `close_tab` did; closing another tab keeps the active tab. on_unsaved=save runs `doc.save` on the active tab first. leave_room=true runs `collab.leave_room` first (active tab). batch: Last. |
| `tab.rename` | `tab?: Tab`, `name: string` | `empty_name`: "The name is empty. Give a name of 1 to 256 characters." (as `doc.rename`); `bad_args` for a name with a control character. `no_such_tab`. | Renames that tab's document (trimmed, cut to 256 characters). One undo step in that tab's history, also when the tab is not the active one. The same name is Ok with changed=false. |
| `tab.move` | `tab?: Tab`, `to_index: int` | `no_such_tab`. `bad_args` if the index is out of range: "`to_index`: must be a position from 0 to {last}". | Reorders the tab list; the active tab stays the same tab. The same position is Ok with changed=false. |
| `tab.duplicate` | `tab?: Tab`, `name?: string` | `no_such_tab`. `empty_name`. `too_many_tabs`. | Copies the document into a new tab right after its source (content, selection and camera; new document, no history, not in any room, no save target; dirty when the source is dirty) and activates it. |
| `tab.list` | none | Always available. | `{tabs: [{id, index, name, active, dirty, room?}]}`. |

#### The workspace record

Decision §9 item 6: the session keeps one workspace record in the `Storage` port and restores all tabs from it. It is written by the engine, not by a command (the shell calls `Session::workspace_tick`, `workspace_next_due_ms` and `flush_workspace`; restoring at start is `Session::restore_workspace`). `doc.autosave` is the command door to the same write (`Session::save_workspace`).

| Part | Where | Content |
|---|---|---|
| Index | `Autosave` namespace, key `workspace` | JSON `{format: "drafftink-workspace", schema: 1, saved_ms, active: <tab id number>, tabs: [{id, key, name, dirty, revision, view: {scroll, zoom}, selection: [ids]}]}` |
| One document per tab | `Autosave` namespace, key `workspace.t<id>` | the native document JSON, as in a saved file |

- History is not saved. A restored tab starts with an empty undo stack, as an opened file does. The viewport size is not saved; the shell reports it with `view.set_viewport`.
- A save writes only the documents that changed since the last write (by revision and name), then the index, then removes the documents of closed tabs. The index is last, so a crash in between leaves the old index and documents that are at worst newer.
- `workspace_tick` writes in pieces. A write takes the documents of the changed tabs without copying them (`TabDoc::share_document`: a list of pointers to the store's elements, the moment the write began), checks them, writes the JSON a slice at a time (`drafftink_scene::JsonJob`, text identical to `Document::to_json`), and hands the finished text to `Storage::put_later`. A call does about 5 ms of work (`workspace_save::TICK_BYTES`), so a small document is written in one call and a document of 100,000 elements in about 25. While a write is under way `workspace_next_due_ms` answers `Some(0)`; a running gesture holds the pieces back (it answers 500). A change made meanwhile is not in the text and is written afterwards. `save_workspace` (the command `doc.autosave`, `flush_workspace`) does the whole write in the call. A native `put_later` is carried out by a writer thread, in order; every other call of the port waits for the writes before it. A failure of a queued write comes back through `take_write_error` and makes the next tick write everything again.
- Debounce, on the `Clock` port's monotonic time: a write happens when the document has been quiet for 1 second, and in any case no later than `autosave.interval_s` (5) after the first unsaved change. With `autosave.enabled` off nothing is written by the tick or the flush. `workspace_next_due_ms` returns `None` when nothing waits, so an idle session needs no timer. A failed write is retried after a full interval and the reason is in `workspace_error`.
- A change is a different tab list, order or active tab, or per tab a different revision, name or unsaved state. Camera and selection are saved with the next write but do not cause one.
- Restore reads the index and every tab it names. A tab whose document is missing or damaged is skipped and listed in the report; the others open. With no readable tab the session stays as it was. A restored tab is dirty when it was dirty at the save. A record with a newer `schema` is neither read nor overwritten.


### 2.3 `tool`: tools

Tool changes are never undoable and never touch the document. `tool.set` is the one place that decides what a tool change implies (FIX: the old `set_tool` did not end text editing or cancel a manipulation, and was implemented three times, A:5245, 5305, U:745).

| id | title | undo | keys (mac / other) | source |
|---|---|---|---|---|
| `tool.set` | Set tool | NoUndo | see the binding table below | A:5245-5295; U:745; X:Tools.tsx |
| `tool.lock` | Keep tool active | NoUndo | `Q` | X:LockButton, actions/shortcuts `toolLock`; NEW |
| `tool.hold` | Hold tool temporarily | NoUndo | `Space` (hold) with tool=hand | X:App.pan.ts; NEW (replaces A:4711 Space as mouse button) |
| `tool.release` | Release held tool | NoUndo | `Space` (release) | NEW |
| `tool.cancel` | Cancel | Undoable | `Esc` | A:5305 |
| `tool.set_arrow` | Set new arrow style | NoUndo | none | NEW; X:currentItemArrowType, currentItemStartArrowhead, currentItemEndArrowhead |

Bindings of `tool.set` (keys fire only without Ctrl/Cmd/Alt, and also without Shift; the old app accepted Shift+letter as the tool key, A:5245, which would collide with Shift+H and Shift+V flip):

| Tool | Keys | Source |
|---|---|---|
| select | `V`, `1` | A:5245 |
| lasso | none (toolbar button after select, palette `tool.set` tool=lasso; the select tool's Alt-drag is a lasso too). Excalidraw gives its lasso no letter; `S` is the stroke colour picker here, as there. Its select button switches to lasso when clicked while active, which is a separate button here | X:Tools.tsx `lasso`, App.tsx |
| hand | `H` | A:5250 |
| rectangle | `R`, `2` | A:5255 |
| diamond | `D`, `3` | X (NEW; key 3 was unused) |
| ellipse | `O`, `4` | A:5260 |
| arrow | `A`, `5` | A:5265 |
| line | `L`, `6` | A:5270 |
| freehand | `P`, `7`, `X` | A:5275; `X` from Excalidraw |
| highlighter | none (as in the old app; toolbar and palette) | U:594 claimed `H`, wrong |
| text | `T`, `8` | A:5280 |
| math | `M`, `9` | A:5285 (Excalidraw uses `9` for image; see §6.4) |
| frame | `F` | X (NEW) |
| eraser | `E`, `0` | A:5290; `0` from Excalidraw |
| laser | `Z`, `K` | A:5295; `K` from Excalidraw |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `tool.set` | `tool: Tool`, `keep_selection?: bool` (default true for select, lasso and hand, false for the other tools) | Always available. Execute refuses: `view_only_tool`: "In view-only mode only the select, hand and laser tools are available. Turn view-only off with `view.set_view_only` (on=false)." `gesture_active`. | Ends a running text edit (committed), leaves the linear point editor, then sets the tool. (A half-drawn multi-point line is a draft of the `element` family, which is abandoned there; `tool.set` has no state of its own for it.) Setting the tool that is already active is Ok with changed=false unless a rung above changed something. A held tool (`tool.hold`) stays held; `tool.set` changes the tool that returns. Switching to a drawing tool clears the selection unless keep_selection is true. The lasso tool is a selection tool: it works in view-only mode and keeps the selection. The highlighter and the freehand tool share one recognizer and differ in defaults (width 12 or more, alpha 128, see `element.create`). |
| `tool.lock` | `on?: bool` (omitted toggles) | Always available. | Tool lock keeps the drawing tool after an element is created instead of returning to select. Session state, not persisted. Output: the tool state `{tool, active, held, tool_lock}`, as for every `tool.*` command. |
| `tool.hold` | `tool: Tool = hand` | `gesture_active`. `view_only_tool` (as `tool.set`). | Remembers the current tool and activates `tool` until `tool.release`. The same hold again (key repeat) is Ok with changed=false. A hold of another tool replaces the held tool. Used by Space-drag panning. |
| `tool.release` | none | Always available. A release without a hold is not an error: Ok with changed=false. | Restores the tool that was active before `tool.hold`. |
| `tool.cancel` | `all: bool = false` | Always available (a stray Esc is Ok with changed=false). | One Esc ladder, in this order, each rung only if applicable: cancel the running gesture (`gesture.cancel`); end a text edit (committed, as `text.edit_end`); leave the linear point editor; leave the editing group (`group.exit`); clear the selection (`selection.clear`); switch to the select tool and turn tool lock off. With all=false (the default, decision §9 item 5) only the first applicable rung runs: Excalidraw's step-by-step Esc. With all=true every applicable rung runs (the old app's single Esc, A:5305). Output `{changed, rungs}` lists the rungs that ran, by name: `gesture`, `text_edit`, `point_editor`, `group`, `selection`, `tool`. The text-edit rung changes the document, so this command is Undoable: a text edit it commits is one undo step labelled "Cancel" (a call that changes no document pushes none). It runs in view-only mode too, because it is how a user gets out of a state; that, the document change and the gesture rung put it on the exception list of §1.2 test 2. A rung that is a command (`group.exit`, `selection.clear`, `text.edit_end`) runs through `run_nested` when its family is registered. The UI closes open dialogs and popovers first (verb `ui.close_overlays`); that is not part of this command. |
| `tool.set_arrow` | `arrow_type?: "sharp" \| "round" \| "elbow"`, `start_head?: Head \| null`, `end_head?: Head \| null` | Always available (view-only too). `gesture_active`. Execute: `bad_args` when none of the three is given: "give at least one of `arrow_type`, `start_head` and `end_head`, for example {\"arrow_type\": \"elbow\"}" (and for a word that is not a path type or head). | Sets what the arrow tool gives a new arrow (decision: the choice is a tool default, not a style property, so `style.set` does not carry it and cannot re-type an existing arrow without routing it). A part that is left out keeps its value; `null` or `"none"` is no head. The defaults are session tool state (`Session::tool.arrow_default`) and are kept in the settings `arrow.type`, `arrow.start_head`, `arrow.end_head` (§5), so `settings.set`, `settings.cycle` and `settings.reset` reach the same state and the choice survives a restart. The arrow tool reads them and passes them to `element.create` as `arrow_type`, `start_head` and `end_head`; `element.create` itself ignores the defaults (omitted means sharp with the built-in heads), so a script is not changed by what the tool is set to. The preview of the arrow being drawn shows the heads. Existing arrows are not changed (`arrow.set_type`, `arrow.set_head`). Excalidraw cycles the type when the arrow key is pressed on the active arrow tool; that key behaviour is not adopted (the key runs `tool.set`). Output `{changed, arrow_type, start_head, end_head}` with the values in effect. |

### 2.4 `element`: create, delete, move, resize, rotate, lock

An element is flat and has a stable id (`scene` crate). Groups are `groupIds` on elements, not a shape kind. Moving, resizing, rotating, flipping and deleting always keep arrows bound to the elements, bound text with its container, and frame membership consistent, because the invariant pass (§1.4 step 8) runs inside every command. A command never fixes those by hand.

**Transform rule.** `element.move`, `element.nudge`, `element.resize`, `element.set_bounds`, `element.rotate`, `element.flip`, `arrange.align`, `arrange.distribute` and `layout.apply` all follow the same rules, in the `edit` crate, never in the command:

1. Bound text moves, scales and rotates with its container; frame children move with their frame.
2. An arrow whose bound target is in the transformed set keeps the binding and is transformed rigidly with it. An arrow whose bound end is not in the set (an arrow moved, resized, rotated or flipped alone, or a multi-selection that leaves the target behind) unbinds that end first. Rotating or resizing a single arrow unbinds both ends. `element.nudge` follows the same rule, and re-snapshots the fixed points of the bindings that remain.
3. Arrows bound to a transformed target follow it: an orbit end stays on the outline, an inside end keeps its ratio. Elbow arrows are re-routed once, after all elements have moved.
4. Locked elements refuse the whole command (`locked`, with `details.ids`). Nothing is changed partially.
5. Frame membership is recomputed only by `element.move` (frame=auto) and `element.resize` of a frame, on the committed run.

`Query` fields (all optional, combined with AND): `ids: Refs`, `text: string | {contains?: string, regex?: string}` (case-insensitive, matches text, labels and LaTeX), `kind: Kind | Kind[]`, `style: StyleProps` (exact match on the given props), `region: {rect: Rect, mode: "overlap" | "contain" | "contained_by"}`, `connected_to: {ref: Ref, direction: "in" | "out" | "any"}` (arrows bound to the element, or elements bound by an arrow), `group: GroupId`, `frame: Ref`, `name: string` (glob), `locked: bool`, `selected: bool`, `has_label: bool`.

| id | title | undo | keys (mac / other) | source |
|---|---|---|---|---|
| `element.create` | Create element | Undoable | none (tools call it) | E:566-596, 734-764, 773-846 |
| `element.delete` | Delete | Undoable | `Delete`, `Backspace` | A:5299; X:actionDeleteSelected |
| `element.duplicate` | Duplicate | Undoable | `Mod+D` | A:5210, 3354 |
| `element.move` | Move | Gesture | none (drag); `Alt` while dragging sets duplicate=true | E:656-713, 1105-1205, 514-536 |
| `element.nudge` | Nudge | Undoable (coalesce 1000 ms) | `Arrow keys`, `Shift+Arrow keys` (fine) | A:5338 |
| `element.resize` | Resize | Gesture | none (drag handles) | E:876-1104, 609-655 |
| `element.set_bounds` | Set position and size | Undoable | none | NEW (stats panel, agents) |
| `element.rotate` | Rotate | Gesture | none (drag handle); double-click handle calls mode=to, angle=0 | E:878-902, 611-632, 447-462 |
| `element.flip` | Flip | Undoable | `Shift+H` flip horizontal, `Shift+V` flip vertical | U:1753, 1761; A:3625-3632; X:actionFlip |
| `element.lock` | Lock or unlock | Undoable | `Mod+Shift+L` | X:actionElementLock; NEW |
| `element.unlock_all` | Unlock all | Undoable | none (context menu) | X:actionElementLock; NEW |
| `element.set_link` | Link | Undoable | `Mod+K` is the UI verb `ui.open_link_editor`, which calls this | X:actionLink |
| `element.open_link` | Open link | NoUndo | none (link icon, context menu) | X:Hyperlink, App.redirectToLink |
| `element.set_name` | Name element | Undoable | none | NEW §11 |
| `element.relabel` | Change label | Undoable | none | NEW §11 |
| `element.connect` | Connect elements | Undoable | none | NEW §11 |
| `element.create_sticky` | Add sticky note | Undoable | `N` | X:stickyNote.ts; NEW |
| `element.convert` | Convert element type | Undoable | `Tab`, `Shift+Tab` | X:ConvertElementTypePopup.tsx; NEW |
| `element.erase` | Erase along path | Undoable | none (eraser tool) | E:1285-1311 |
| `element.query` | Find elements | NoUndo, read-only | none | NEW §11 |
| `element.at` | Element at point | NoUndo, read-only | none | NEW §11 |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `element.create` | `kind: Kind` (not image: use `image.insert`), plus per kind (below), common: `style?: StyleProps` (default: the style defaults), `name?: string`, `id?: Id`, `select: bool = true`, `frame: "auto" \| Ref \| null = auto`, `capture: bool = true`, `after?: Ref` (insert directly above that element in z-order; default the top, or just below the frame for a frame child), `from_tool: bool = false` | Always available. Execute refuses: `too_small`: "The {kind} is smaller than 1 unit in {width/height}. Give bounds with w and h of at least 1."; `too_few` (catalog) for a line, arrow or freehand with fewer than 2 points (1 point is allowed for freehand: it becomes a dot); `empty_text`: "A text element cannot be empty. Give `text`, or start typing with `text.edit_begin`."; `invalid_latex` (see `text.set_latex`); `id_in_use`: "The id `{id}` is already used by another element. Omit `id` to get a fresh one."; `name_taken` and `bad_name` (see `element.set_name`); `not_bindable` for a start or end target (see `arrow.bind`); `already_bound` (see `text.bind`) when `container` already has a label; `wrong_kind` for kind=image: "`element.create` cannot create images. Use `image.insert`."; `locked` for a locked container or frame target. | Creates one element (or a text with its container label) and selects it. Per kind. rectangle, diamond, ellipse, frame: `bounds: Rect` (negative w or h is normalised). line, arrow: `points: Point[]` (2 or more, world coordinates), `arrow_type: "sharp" \| "round" \| "elbow" = sharp`, `closed: bool = false` (line only), `start?: Bind`, `end?: Bind` with `Bind = {target: Ref, mode?: "inside" \| "orbit", fixed_point?: [rx, ry]}`, `bind: "auto" \| "off" \| "inside" = auto` (auto binds an end that lies inside a bindable element or within the bind distance of its outline, per the binding design), `start_head?: Head \| null`, `end_head?: Head \| null` (arrow defaults: no start head, `arrow` end head, as the old app). freehand: `points: Point[]`, `pressures?: number[]` (0..1, same length; absent means uniform or simulated by `pressure_mode`), `highlighter: bool = false` (width at least 12 and alpha 128 unless the style says otherwise), `simplify: number = 2.0` (RDP tolerance in world units, 0 keeps all points; old `Freehand::simplify(2.0)`). text: `text: string`, `at: Point` (top-left), `width?: number` (fixed width wraps; absent auto-sizes), `container?: Ref` (create as that shape's label). math: `latex: string = "x^2 + y^2 = r^2"` (the old default, E:792-803), `at: Point` (top-left of its box). `frame: auto` puts the new element into the frame under the bounds' centre; a new frame with capture=true takes in every unlocked element that lies completely inside it (groups only if all members are inside, no frames). `from_tool: true` is passed by the drawing recognizer: afterwards the tool returns to select unless `tool.lock` is on, in the same undo step. Bound arrows and elbow routes are solved before the command returns. |
| `element.delete` | `ids: Refs = selection`, `frame_children: "delete" \| "release" = delete` | `no_target`. Execute: `locked`, `not_found`. | Soft-deletes the elements (tombstones, so undo and sync work). A deleted container deletes its bound text. An arrow bound to a deleted element keeps its position and becomes unbound at that end. Deleting a frame deletes its children (release keeps them, frame_id cleared). Deleting a group member removes just that member; a group with fewer than 2 live members counts as no group everywhere (derived); its stale `groupIds` entry is removed the next time the element is edited. Inside an editing group the selection falls back to the remaining members. |
| `element.duplicate` | `ids: Refs = selection`, `dx: number = 20`, `dy: number = 20`, `select: bool = true` | `no_target`. Execute: `locked`. | Copies the elements with new ids and new seeds, offset by (dx, dy). Group ids are regenerated consistently (all copies of a group's members share one new group id; inside an editing group the copy stays in the parent group). Bindings are remapped to the copies; an end whose target was not copied becomes unbound. Frame membership is remapped to a copied frame or cleared. Copies are inserted directly above the last original, group members stay contiguous. The selection becomes the copies. One undo step. |
| `element.move` | `ids: Refs = selection`, `dx: number = 0`, `dy: number = 0` (total since the start, world units), `duplicate: bool = false`, `snap: Snap = auto`, `frame: "auto" \| "keep" = auto`, `pointer?: Point` (world position of the pointer; frame hit-testing uses it, default the centre of the moved bounds) | `no_target`. Execute: `locked`. | Translates the elements. Follows the transform rule above. With snap, grid snap (20) and smart guides (threshold 8, equal spacing) adjust (dx, dy); the reference is the moved bounds, deterministic (the old code used `HashMap::values().next()`, E:1105, FIX). duplicate=true leaves the originals and moves copies (Alt-drag), and selects the copies. With frame=auto, on the committed (non-live) run elements dropped over a frame join it and elements dragged out of it leave it, by the pointer rules. Output: `{dx, dy}` after snapping, `guides`, `frame_highlight?`. Gesture command (§1.10). |
| `element.nudge` | `dir: "left" \| "right" \| "up" \| "down"`, `ids: Refs = selection`, `step?: number`, `fine: bool = false` | `no_target`. Execute: `locked`. | Moves by `step` (default the setting `input.nudge_step`, 20 = the old `GRID_SIZE`), or by 1 unit when fine=true (Shift+arrow, NEW). Follows the transform rule above, and re-snapshots the fixed points of the bindings that stay. Repeated nudges of the same elements within 1 s merge into one undo step (the old app pushed one step per key repeat, A:5338). |
| `element.resize` | `ids: Refs = selection`, `handle: Handle = se`, `dx: number`, `dy: number` (total displacement of the handle since the start, world axes), `keep_aspect: bool = false`, `from_center: bool = false`, `snap: Snap = auto` | `no_target`. Execute: `locked`. | Scales the common bounds of the elements: corners scale about the opposite corner, edges stretch one axis (edge handles are NEW; the old app declared them but never produced them). from_center scales about the centre (Alt). keep_aspect is forced for text and images, and when a rotated element or a group member takes part (Excalidraw rule). Text scales its font size on a corner handle, and an edge handle sets a fixed width (wrap on, auto-resize off). Linear elements and freehand scale their points. Resizing a frame recomputes membership on the committed run. Minimum size is 1 unit (clamped, never a refusal). Bound arrows, bound text and elbow routes update. Output: `{bounds, guides, size_badge}`. Gesture command. |
| `element.set_bounds` | `ids: Refs = selection`, `x?: number`, `y?: number`, `w?: number`, `h?: number`, `anchor: "top_left" \| "center" = top_left`, `keep_aspect: bool = false` | `no_target`. Execute: `locked`; `bad_args` if w or h is not above 0. | Sets the position and/or size of the common bounds of the selection (one element: exactly that box). Uses the same scaling rules as `element.resize`. Used by a numeric stats panel and by agents. |
| `element.rotate` | `ids: Refs = selection`, `angle: number`, `mode: "by" \| "to" = by`, `snap_degrees?: number` (the UI passes 15 while Shift is held) | `no_target`. Execute: `locked`; `bad_args`: "mode \"to\" sets an absolute angle and needs exactly one element; use mode \"by\" to rotate several."; `not_rotatable`: "Elbow arrows cannot be rotated. Change the arrow type first with `arrow.set_type` (sharp or round)." or "Frames cannot be rotated." (the whole command refuses and `details.ids` lists the offending elements; nothing is skipped silently) | Rotates about the common centre. Every element's own `angle` changes by the delta (there is no group angle, which fixes the old cosmetic group rotation). mode=to with angle 0 resets the rotation (double-click on the rotate handle, E:447). A single rotated arrow unbinds both its ends; arrows bound to rotated targets follow. snap_degrees rounds the resulting delta. Output: `{angle, angle_badge}`. Gesture command. |
| `element.flip` | `ids: Refs = selection`, `axis: "horizontal" \| "vertical"` | `no_target`. Execute: `locked`. | Mirrors the elements about their common centre. A group moves as a unit (fixes the old displaced children, core `Group::transform`). Angles are negated, linear points mirrored, images toggle their scale sign, text keeps readable glyphs and only moves. Bound arrows follow. |
| `element.lock` | `ids: Refs = selection`, `locked?: bool` (omitted toggles: unlock if all are locked, else lock) | `no_target`. | Sets `locked`. Locking an element locks every member of its outermost group, so a group stays one unit. Locked elements leave the selection, are not hit by click or marquee, are not binding targets, and every element, style, arrange, group and text command refuses them with `locked`. They can still be found by `element.query` and addressed by id. |
| `element.unlock_all` | `ids?: Refs`, `select: bool = true` | `nothing_to_do`: "Nothing to unlock: no element is locked." Execute with `ids` when none of the given elements (nor their groups) is locked: \"Nothing to unlock: none of the given elements is locked.\" | Unlocks the given elements, or every locked element, and selects them. With `ids` this is the unlock of the canvas: a locked element cannot be selected, so the context menu of a locked element (`ContextTarget::Locked`, `docs/interaction.md`) calls it with that one id, as Excalidraw's menu does (it selects the locked element and runs the lock toggle). A locked group unlocks as a whole. `element.lock` with `locked=false` unlocks without selecting. |
| `element.set_link` | `id?: Ref` (default the single selected element), `link?: string \| null` | `needs_single` when `id` is absent and the selection is not exactly one element. Execute: `invalid_link`, with the text saying what is wrong and how to fix it: "Links can only use http://, https:// or mailto:, not `javascript:`. Use a web address such as https://example.com, or link to an element with `#id` or `@name`." (also for `data:`, `file:`, `vbscript:` and every other scheme), spaces or line breaks, quotes, angle brackets, backslashes or control characters, a link over 2048 bytes, a user name or password in front of the host, a text that is not a web address ("`hello` is not a web address. Write it like https://example.com/page, link to an element with `#id` or `@name`, or pass null to remove the link."); `not_found` for an element link whose target does not exist or is deleted; `locked`; `view_only` as every document command. | Sets, replaces or removes `Element.link`. A web link is `http`, `https` or `mailto`; the scheme is lower-cased and a bare `example.com/page` becomes `https://example.com/page`. An element link is `#e12` or `@name`; both are stored as `#<id>` (the id, so renaming the target keeps the link). `null`, an absent `link` and an empty text remove the link. Setting the link an element already has is Ok with `changed: false` and no undo step. Output `{id, link, kind: "url" \| "element" \| null, changed}`. The link round-trips through the native JSON and `.excalidraw` (`link`); a `.excalidraw` link is checked again when it is opened. Excalidraw's link dialog is the UI verb `ui.open_link_editor` (`Mod+K`, §6.3). |
| `element.open_link` | `id?: Ref` (default the single selected element) | Available when some element has a link; otherwise `nothing_to_do`: "Nothing to open: no element has a link. Add one with `element.set_link`." Execute: `needs_single`; `no_link`: "Element `{id}` has no link. Add one with `element.set_link`."; `invalid_link` when the stored text is not a safe link (checked again, because a document from outside can hold anything); `not_found` for a link to an element that is gone. | A web link: returns `{kind: "url", url}` and changes nothing; the UI turns it into `HostRequest::OpenUrl` and the shell opens it (the engine never opens a browser). An element link: selects the target (a locked target is not selected) and brings it into view with `view.zoom_to_fit` scope=ids, zooming out when it does not fit but never in (Excalidraw `fit: scale-down`); returns `{kind: "element", target, changed}`. Allowed in a view-only document. Not exposed to agents (they read the link with `element.query` fields=link). |
| `element.set_name` | `id: Ref`, `name: string \| null` | Execute: `name_taken`: "The name `{name}` is already used by element `{id}`. Names must be unique so that `@{name}` is unambiguous. Pick another name, or clear the other with `element.set_name`."; `bad_name`: "A name must be 1 to 64 characters, must not start with `@` or `$`, and must not contain control characters."; `not_found`. | Sets or clears the element's `name` field (stored in the document and synced). Agents then address the element as `@name`. Not shown on the canvas. |
| `element.relabel` | `id: Ref`, `text: string`, `create: bool = true` | Execute: `wrong_kind` (allowed: text, rectangle, diamond, ellipse, arrow); `empty_text` (on a free text element only: "A text element cannot be empty. Delete it with `element.delete`."; a bound label given as `id` counts as its container, so empty text removes the label, as with `text.set`); `locked`; `not_found`. | On a text element, replaces its content. On a shape or an arrow, replaces its bound label, creating one when create=true, removing it when `text` is empty. On a bound label, empty `text` removes that label. Re-measures, grows the container when the text no longer fits, and re-centres the label. |
| `element.connect` | `from: Ref`, `to: Ref`, `kind: "arrow" \| "line" = arrow`, `arrow_type: "sharp" \| "round" \| "elbow" = sharp`, `bind_mode: "orbit" \| "inside" = orbit`, `label?: string`, `start_head?: Head \| null`, `end_head?: Head \| null`, `style?: StyleProps`, `name?: string` | Execute: `name_taken` and `bad_name` (see `element.set_name`); `same_element`: "Cannot connect an element to itself with `element.connect`. Pass two different elements."; `not_bindable`: "{kind} elements cannot be arrow targets. Bindable kinds: rectangle, diamond, ellipse, text, image, frame."; `locked`; `not_found`. | Creates an arrow whose ends are bound to the two elements (the binding is stored on the arrow), starting on the side facing the other element, routed and z-ordered above both. The label becomes the arrow's bound text (kind=arrow only; blank means none). Selects the new arrow. Output `{created: [arrow id]}`. |
| `element.create_sticky` | `at?: Point \| "viewport_center" \| "$cursor"` (the middle of the note, default the visible canvas), `size: number = 250` (side, at least 75), `bounds?: Rect` (instead of `at` and `size`), `text?: string`, `color: "yellow" \| "pink" \| "green" \| "blue" \| "orange" = yellow`, `font_size?: number` (largest label size, 16 to 512, default 28), `name?`, `id?`, `select: bool = true`, `frame: "auto" \| Ref \| null = auto`, `edit: bool = false` | Always available. Execute refuses: `too_small`: "The sticky note is smaller than 75 units in {width/height}. Give a size, or bounds with w and h, of at least 75."; `bad_args` for `bounds` together with `at` or `size`, and for a `font_size` outside 16 to 512; `id_in_use`, `name_taken`, `bad_name` (see `element.create`); `locked` for a locked frame target; `limit_exceeded` for text over the text limit. | Creates a rectangle with the sticky preset (Excalidraw's sticky note): solid fill in the paper colour (`#ffdf6b` yellow, pink `#fcc2d7`, green `#b2f2bb`, blue `#a5d8ff`, orange `#ffd8a8`), ink `#1e1e1e`, corner radius a quarter of the shorter side up to 32, line width, dash, roughness and opacity from the style defaults. Non-blank `text` becomes its bound label, centred both ways, with the largest font size from `font_size` (default 28) down to 16 in steps of 2 at which the wrapped text fits; when it does not fit at 16 the note grows downwards. No `text` makes an empty note (type with `text.edit_begin`). `edit=true` then starts editing its label (the key `N` passes it). The note is an ordinary rectangle: there is no sticky element kind, no creation-date footer and no drop shadow (the renderer has none), and the label keeps the size it was fitted to when the note is resized or the text is edited. One undo step. Output `{id, label?, bounds, frame?, font_size?}`. |
| `element.convert` | `ids: Refs = selection`, `to?: "rectangle" \| "diamond" \| "ellipse" \| "line" \| "arrow"`, `arrow_type?: "sharp" \| "round" \| "elbow"` (only with `to`=arrow, default sharp), `reverse: bool = false` (only without `to`) | `no_target`. Execute: `locked`; `not_found`; `wrong_kind` (allowed: rectangle, diamond, ellipse, line, arrow; text, math, image, frame and freehand are refused); `mixed_kinds`: "Shapes (rectangle, diamond, ellipse) and lines or arrows cannot be converted together. Select only shapes, or only lines and arrows."; `not_convertible`: a target from the other family ("`{id}` is a {from} and cannot become a {to}. Shapes become a rectangle, diamond or ellipse; lines and arrows become a line or an arrow."), an arrow bound to a shape or carrying a label that would become a line ("... Detach it first with `arrow.unbind` ...", "... Remove the label first with `element.relabel` (empty text) ..."), a closed line that would become an arrow; `nothing_to_do`: "Nothing to convert: every element is already a {type}."; `bad_args` for `arrow_type` without `to`=arrow and for `reverse` with `to`. | Changes the type in place, keeping the id, box, angle, style, groups, frame and z-order. Shapes (rectangle, diamond, ellipse): the bound label is laid out again for the new inner box and the shape grows about its centre when the text no longer fits; arrows bound to the shape are put back on the new outline (`reanchor_to_outline`) and solved, elbow arrows re-routed. Lines and arrows: the points stay; a line becomes an arrow with an `arrow` end head, an arrow becomes a line without heads, a path change is `arrow.set_type`. Without `to` the element takes the next type of its family (Tab; `reverse` the previous one, Shift+Tab): rectangle, diamond, ellipse; line, sharp arrow, round arrow, elbow arrow (Excalidraw's order). Several elements of one family convert together; elements already of the target type are left alone (listed in `unchanged`). Selection is kept. One undo step. Output `{to, converted, unchanged, followers}`. |
| `element.erase` | `path: Point[]` (1 or more), `radius: number = 10`, `units: "screen" \| "world" = screen` | Always available. Erasing nothing is Ok with deleted=[] (changed=false). | Deletes every unlocked element hit by the swept path, using exact hit testing, with the rules of `element.delete`, as one undo step (the old app pushed an undo step per drag event and used a fixed 10 world units, E:1285; FIX). Screen units are converted with the current zoom. |
| `element.query` | `filter?: Query`, `limit: int = 100` (max 1000), `offset: int = 0`, `fields: string[] = [id, kind, name, text, bounds]`, `order: "z" \| "reading" \| "position" = z` | Always available. `bad_args` on a malformed query. | Returns `{total, items, truncated}`. Selectable fields: id, kind, name, text, bounds, style, angle, group_ids, frame, bindings, bound_text, locked, link, z, points (a line or arrow's points in world coordinates, `[[x, y], ...]`, `null` for other kinds). In the filter, `ids`, `frame`, `bound_text_of` and `connected_to.ref` accept ids and `@name`; an unknown one is `not_found`. Bound text is left out unless the filter asks for kind text, names `ids` or sets `bound_text_of`. `fields` needs 1 to 32 entries; `limit` outside 1..1000 is `bad_args`. |
| `element.at` | `point: Point`, `tolerance: number = 6` (screen px), `all: bool = false`, `deep: bool = false`, `include_locked: bool = false` | Always available. | Hit-tests the point (`tolerance` screen pixels, divided by the zoom). Returns `{count, hits}`: the topmost unit (the outermost group, or the member with deep=true), or all hits top to bottom. A hit has id, kind, name, text and bounds; a hit that counts as a whole group also has `group` and `members`. Bound text is hit as its container. Frames are hit only on their outline, not their interior. |

### 2.5 `selection`: selection

State: selected element ids, selected groups, the editing group, and the linear point editor. Selection commands never create undo steps (§1.8).

| id | title | undo | keys (mac / other) | source |
|---|---|---|---|---|
| `selection.select` | Select | NoUndo | none | E:486-538; X:App.selection |
| `selection.select_at` | Select at point | NoUndo | none (click) | E:486-538; X:handleSelectionOnPointerDown |
| `selection.select_region` | Select region | NoUndo | none (marquee) | E:539-547, 714-729 |
| `selection.select_lasso` | Select with a lasso | NoUndo | none (lasso tool; Alt-drag with the select tool) | NEW; X:lasso/index.ts, lasso/utils.ts |
| `selection.all` | Select all | NoUndo | `Mod+A` | A:4777; X:actionSelectAll |
| `selection.clear` | Clear selection | NoUndo | none (`Esc` runs `tool.cancel`) | A:5305 |
| `selection.get` | Get selection | NoUndo, read-only | none | NEW §11 |
| `selection.navigate` | Select connected element | NoUndo | `Alt+Arrow keys` | X:flowchart; NEW |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `selection.select` | exactly one of `ids: Refs`, `group: GroupId`, `filter: Query`; `mode: "replace" \| "add" \| "remove" \| "toggle" = replace`, `deep: bool = false`, `include_locked: bool = false` | Always available. Execute: `bad_args`: "Give exactly one of `ids`, `group` or `filter`."; `not_found`. | Selecting a member selects its outermost group (inside an editing group: the next level down) unless deep=true, in which case it selects the element itself and sets the editing group to its innermost group when all targets share it. A selected frame excludes its children. Bound text follows its container. Locked elements are skipped unless include_locked: they are listed in the output as `skipped_locked` and in `warnings`, and a call whose targets are all locked leaves the selection as it was. mode=toggle removes the targets when all of them are selected, else adds them. `group` selects the members of that group with the same rules (a nested group is expanded to its outermost group unless deep). `not_found` also covers a group that has fewer than two members and a filter that matches nothing ("No element matches the filter (kind: freehand). Loosen the filter, or look up ids with `element.query` or `doc.outline`.", the set fields of the filter in plain words, `details.filter` the same text); an empty `ids` list is `bad_args`; more than 10 000 ids is `limit_exceeded`. Output `{changed, selected, groups, editing_group, skipped_locked?}`, `selected` back to front. Shift-click is mode=toggle. |
| `selection.select_at` | `point: Point`, `mode: "replace" \| "add" \| "remove" \| "toggle" = replace`, `deep: bool = false`, `cycle: bool = false`, `tolerance: number = 6` (screen px), `include_locked: bool = false` | Always available. | Hit-tests at the point and selects the unit as above. A click on empty canvas with mode=replace clears the selection and leaves the editing group unless the point lies inside it. cycle=true (Alt-click) steps through the units under the point, top to bottom. With mode=replace (no deep, no cycle), clicking inside the common bounds of an existing multi-selection of two or more elements keeps it when nothing is under the point or the top hit is already selected (the click that starts a drag); a hit on an unselected element selects that element. Output as `selection.select`. |
| `selection.select_region` | `rect: Rect`, `mode: "replace" \| "add" = replace`, `contain: "overlap" \| "contain" = settings input.box_select`, `include_locked: bool = false` | Always available. | Selects elements overlapping (the old "touch, not contain" semantics) or fully inside the rectangle. Any member inside selects its whole outermost group. Ignores locked elements and bound text, and never selects the children of a selected frame together with the frame. The recognizer ignores rectangles under 2 by 2 pixels (old minimum-size rule). |
| `selection.select_lasso` | `points: Point[]` (3 or more, world units; the last point joins the first), `mode: "replace" \| "add" = replace`, `contain: "overlap" \| "contain" = settings input.box_select`, `include_locked: bool = false`, `simplify: number = 1` (world units, 0 keeps every point) | Always available. Execute: `bad_args` naming `points` when the path has fewer than 3 distinct points after simplifying, when all points lie on one line, when a coordinate is not a finite world number, or when `simplify` is negative; `limit_exceeded` above 200 000 points. The messages are "the path has only {n} distinct point(s) after simplifying, and a lasso needs 3" and "the points lie on one line, so the path encloses no area", each followed by "Draw a loop around what you want to select, or give 3 or more points that make a closed shape". | The free-form marquee, with the same rules as `selection.select_region` for groups (any member selected selects its whole outermost group), frames (children are not selected with their frame), locked elements, bound text and the modes replace and add; a lasso over nothing clears in replace mode. Which shapes the path selects is `drafftink_geom::lasso`: `overlap` selects a shape whose outline the path crosses, a shape that lies inside the path, and a shape with a fill that the path lies inside (an unfilled shape is not selected by a path drawn in its hollow middle, as with the marquee); `contain` selects only a shape whose whole outline lies inside the path. "Inside" is the non-zero winding rule, so a path that crosses itself encloses its loops, and a concave path leaves out what sits in its notches. Deviations from Excalidraw: Excalidraw selects while the path grows and applies its `boxSelectionMode` the same way; here the command runs once at the end, as the marquee does, and `contain` selects a group when any member is inside (Excalidraw needs all members). Output as `selection.select`. |
| `selection.all` | `within?: Ref` (a frame or a group) | `nothing_to_do`: "There are no elements to select." | Selects all unlocked elements, as groups (inside an editing group: that group's members; with `within`: that frame's children or group's members). |
| `selection.clear` | none | Always available (idempotent). | Clears the selection and leaves the editing group and the linear point editor. |
| `selection.get` | `detail: "ids" \| "summary" \| "full" = summary` | Always available. | `{ids, groups, editing_group, count, bounds, kinds, linear_editor?}` (`bounds` is null and `kinds` empty with nothing selected); `ids` returns only `{ids, count}`; full adds `elements` with every field of `element.query`. |
| `selection.navigate` | `direction: "up" \| "down" \| "left" \| "right"` | `needs_single` (thing: "element"). Execute: `nothing_to_do`: "`{id}` has no connected element to the {direction}." | Selects the node connected by an arrow in that direction (the visually nearest, as Excalidraw's flowchart navigation); only that element is selected, not its group. A locked target refuses with `locked`. Output as `selection.select`. |

### 2.6 `clipboard`: copy, cut, paste

One path for every surface: the keyboard, the context menu and the palette all call these (the old app had different keyboard and button paste, A:5106 vs A:3428; FIX). The old copy buffer was an in-process JSON string and never reached the system clipboard (FIX).

Payload written by `copy`/`cut`: custom MIME `application/vnd.drafftink.clipboard+json` (elements, files, bound text, frame children) and, when the setting `clipboard.excalidraw_compat` is true (default), the same selection as Excalidraw clipboard JSON in `text/plain` so it pastes into Excalidraw.

| id | title | undo | keys (mac / other) | source |
|---|---|---|---|---|
| `clipboard.copy` | Copy | NoUndo | `Mod+C` | A:5061, 3384; X:actionCopy |
| `clipboard.cut` | Cut | Undoable | `Mod+X` | A:5078, 3400; X:actionCut |
| `clipboard.paste` | Paste | Undoable | `Mod+V` (at: `$cursor`), `Mod+Shift+V` (plain_text=true) | A:5106, 3428; X:actionPaste |
| `clipboard.copy_style` | Copy style | NoUndo | `Mod+Alt+C` | X:actionCopyStyles; NEW |
| `clipboard.paste_style` | Paste style | Undoable | `Mod+Alt+V` | X:actionPasteStyles; NEW |
| `clipboard.copy_as` | Copy as image | NoUndo | `Mod+Shift+E`, `Mod+Shift+C` (both from the old app), `Shift+Alt+C` (Excalidraw) | A:4801, 4992, 3756; U:1957; X:actionCopyAsPng |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `clipboard.copy` | `ids: Refs = selection`, `system: bool = true` | `no_target`. | Stores the copy in the session buffer and writes the payload through the `Clipboard` port. A missing port with system=true is not an error: the internal copy still works and `warnings` says so. |
| `clipboard.cut` | `ids: Refs = selection`, `system: bool = true` | `no_target`. Execute: `locked`. | `clipboard.copy` followed by `element.delete` in one undo step. |
| `clipboard.paste` | `source: "system" \| "internal" \| {text: string} \| {image: Bytes} \| {elements: object} = system`, `at: Point \| "viewport_center" \| "$cursor" = viewport_center`, `select: bool = true`, `plain_text: bool = false` | Execute: `clipboard_empty`: "The clipboard has nothing drafft.ink can paste. Supported: copied elements, Excalidraw data, images, Mermaid diagrams and plain text."; `port_unavailable`: "`clipboard.paste` with source system needs the system clipboard, which this session does not provide. Pass `source` with the data instead."; `parse_failed` for explicit data (and for Excalidraw data that cannot be read, and for a clipboard image that is not PNG, JPEG or WebP when nothing else can be pasted); `mermaid_unsupported` (see `diagram.import_mermaid`); `bad_args` for `plain_text` with source internal; `limit_exceeded` for more elements or text than a document holds. | Routing for source=system, in this order (the old native order): drafft.ink clipboard payload, Excalidraw clipboard JSON, text that parses as a Mermaid diagram, an image, plain text (a new text element). plain_text=true skips straight to a text element. Pasted elements get new ids and new group ids (they never join an existing group), are centred on `at` (the UI passes `$cursor`; the old keyboard path centred on the cursor and the button path on the viewport, A:5106 vs A:3428), keep their frame only when the frame was pasted too, and have bindings remapped or nulled. The selection becomes the pasted elements. `$cursor` is the placeholder a shell replaces with the pointer position; left in, it means the viewport centre. Mermaid text is handed to `diagram.import_mermaid`; while that command is not registered the text is pasted as text with a warning. Structure rules are `drafftink_edit::paste`; parsing is `drafftink_io::clipboard`. |
| `clipboard.copy_style` | `from?: Ref` (default the first selected element) | `no_target`. | Remembers the style props of that element (default: the first selected element in z-order) (stroke, fill, pattern, width, stroke style, sloppiness, corner radius, opacity, and the font props for text) in the session style buffer. |
| `clipboard.paste_style` | `ids: Refs = selection` | `no_target`. Execute: `nothing_to_do`: "No style has been copied yet. Copy one with `clipboard.copy_style` first."; `locked`. | Applies the remembered props to the targets that support them, with the semantics of `style.set` (scope selection). |
| `clipboard.copy_as` | `format: "png" \| "svg" \| "text" = png`, `scope: "selection" \| "all" \| "ids" = selection`, `ids?: Refs`, `scale`, `padding`, `background`, `dark` (as `doc.export`; default the setting `export.dark`) | `no_target` for scope selection with nothing selected (the old menu item was disabled then, U:1957). Execute: `port_unavailable` for png without the `Renderer` or for any format without the `Clipboard`; `render_pending` for png on the web (see `doc.export`); `nothing_to_do` for format text when no text is in scope. | Renders and writes the image (or SVG, or the concatenated text of text elements and labels) to the system clipboard. One implementation replaces the three copies of the old copy-PNG block, and the PNG is made by the same code as `doc.export` (`render_png`: region, size limits, request); the clipboard limit is 8192 pixels on a side. |

### 2.7 `style`: styles

One command sets any number of style props on the selection and on the defaults for new elements. It replaces the old `SetStrokeColor`, `SetFillColor`, `SetStrokeWidth`, `SetCornerRadius`, `SetSloppiness`, `SetFillPattern`, `SetStrokeStyle`, `SetOpacity` and the font actions, which each repeated about 25 lines and mostly pushed no undo step (FIX: all style changes are undoable and all reach collaborators).

The props (each applies only to the kinds listed; others are skipped and reported):

| Prop | Type | Applies to | Old |
|---|---|---|---|
| `stroke_color` | `Color` | all except image | U:1043, A:2606 |
| `fill_color` | `Color \| null` (null is no fill; the old colour grid returned transparent instead of None, FIX) | rectangle, diamond, ellipse, frame, closed line, freehand | U:1097-1182, A:2659 |
| `fill_pattern` | `solid \| hachure \| zigzag \| cross_hatch \| dots \| dashed \| zigzag_line` (all seven; the old UI showed four) | the kinds that can have a fill (as `fill_color`), whether or not a fill colour is set yet | U:1572, A:3025 |
| `stroke_width` | number above 0, at most 1000 (presets 1, 2, 4, 8) | all except image, text, math | U:1156, A:2679 |
| `stroke_style` | `solid \| dashed \| dotted` | every shape: rectangle, diamond, ellipse, frame, line, arrow, freehand (the old app only had it on line and arrow) | U:1631, A:3097 |
| `sloppiness` | `architect \| artist \| cartoonist \| drunk` | all except text, math, image, freehand | U:1542, A:2996 |
| `corner_radius` | number from 0 (sharp), at most 1000000. The old "on" was 32. | rectangle, diamond, line, arrow, frame | U:1515, A:2970 |
| `opacity` | 0 to 1 | all | U:1778, A:3639 |
| `freehand_style` | `normal \| calligraphy` (smooths the samples of strokes drawn afterwards; `element.create` stores the points it is given) | freehand | U:1659, A:3609 |
| `pressure_mode` | `uniform \| simulated \| device` (device is NEW: real pen pressure). `uniform` draws a constant width, the others use the stored `pressures`; `simulated` makes them from the pointer speed when a stroke is drawn with a mouse or finger | freehand | U:1675, A:3617 |
| `font_family` | `gelpen \| gelpen_serif \| vanilla_extract \| noto_sans` | text (a math element has no font family); on a shape or arrow it goes to its label | U:1380, A:2939 |
| `font_weight` | `light \| regular \| heavy` | text (or the label of a shape or arrow) | U:1417, A:2955 |
| `font_size` | number above 0, at most 10000 (presets 16, 20, 28, 36; math uses the same sizes, the old `SetMathFontSize` is merged) | text, math (or the label of a shape or arrow) | U:1446-1496, A:2918-2928 |
| `text_align` | `left \| center \| right` | text (or the label of a shape or arrow) (NEW) | X:actionChangeTextAlign |
| `vertical_align` | `top \| middle \| bottom` | text bound to a container, so also a container that has a label (free text has nothing to align in, so it skips the prop) (NEW) | X:actionChangeVerticalAlign |
| `line_height` | number from 1.0 to 3.0 (default 1.25) | text (or the label of a shape or arrow) (NEW) | X |

Arrow props (type, arrowheads) are in the `arrow` family. Background colour is `doc.set_background`.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `style.set` | Set style | Gesture (coalesce by prop, 1000 ms, when called plainly) | none (`S`, `G`, `Shift+F` are the UI verbs that open the colour and font pickers) | U:1020-1855; A:2606-3130, 3639 |
| `style.get` | Get style | NoUndo, read-only | none | NEW |
| `style.apply_theme` | Apply theme | Undoable | none | NEW §11 |
| `style.list_themes` | List themes | NoUndo, read-only | none | NEW §11 |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `style.set` | `props: StyleProps` (at least one), `ids?: Refs`, `scope: "auto" \| "selection" \| "defaults" \| "both" = auto`, `include_bound_text: bool = true` | Always available (with no selection it edits the defaults). Execute: `bad_args` for a value out of range, of the wrong type, an unknown prop, no prop at all, or `ids` together with scope `defaults` (the message names the prop, e.g. `props.stroke_width`, and the accepted values); `no_target` for scope `selection` or `both` (or an empty `ids` list) with nothing to act on; `not_applicable` (catalog) when no targeted element supports any given prop: "None of the targeted elements support `font_size`. They apply to text and math, and shapes and arrows that have a label."; `locked` when a target (given or selected) is locked, checked for all targets before anything is written. | scope auto: if there is a target (given `ids` or a selection) apply to it and also update the defaults, else update only the defaults. Defaults are the style new elements get and are persisted as the setting `style.defaults`. A prop that an element does not support is skipped for that element and listed in `skipped`. Text props on a container also reach its bound label (include_bound_text). A shape or arrow that has a label passes text props to the label: the label's id then appears in `applied` and the container does not list them as skipped. Output: `{changed, applied: {id: [props]}, skipped: [{id, props}], defaults_updated}`; `changed` is true when an element or the defaults differ afterwards, and `defaults_updated` is true when the defaults were part of the call. It does not re-measure text boxes after a font change; that waits for the text measurer service (`text.*` family). The UI no longer overwrites its controls from the first selected shape each frame (FIX of A:2499); it reads `style.get`. Gesture command: sliders and colour scrubbing call it live and commit once. During a text edit `style.set` refuses with `text_editing`: the UI sends the choice to the editor, which applies it to the selected range or to the element and returns it in `text.edit_end` (runs, style). Persisting `style.defaults` makes the command touch settings (`mutates` has S); a live gesture frame does not write the setting, only the final run does. |
| `style.get` | `ids?: Refs`, `scope: "auto" \| "selection" \| "defaults" = auto`, `props?: string[]` | Always available. | For each prop returns `{value \| "mixed", applicable: bool}` over the targets, plus the set of kinds. `applicable` is true when at least one target supports the prop; the value is taken over the targets that support it, `"mixed"` when they differ, and absent when none does (a removed fill is `null`). A shape or arrow with a label reports the label's text props. Output `{changed: false, source: "selection" \| "defaults", ids, kinds, props: {name: {applicable, value?}}}`. Refuses: `no_target` for scope `selection` with nothing to read; `not_found`; `bad_args` for an unknown prop name or `ids` with scope `defaults`. This drives the properties panel: which sections show, and the "mixed" state for multiple selections (the old panel looked at `selection[0]` only, U:809). |
| `style.apply_theme` | `name: string`, `ids?: Refs`, `scope: "auto" \| "selection" \| "defaults" \| "both" = auto`, `set_background: bool = false` | Execute: `unknown_theme`: "Unknown theme `{name}`. Available themes: {list}. See them with `style.list_themes`."; `no_target` only when scope is selection or `ids` resolves to nothing. | A theme is a named `StyleProps` bundle (and optionally a background colour). Applying it is `style.set` with that bundle. Built-in names and their exact bundles: `default` = the initial style defaults (below) with background `#ffffff`; `monochrome` = stroke `#000000`, no fill, pattern solid, width 2, sloppiness architect, background `#ffffff`; `pastel` = stroke `slate-700`, fill `blue-100`, pattern solid, width 2, sloppiness artist, background `#fafafa`; `blueprint` = stroke `#ffffff`, no fill, width 2, sloppiness architect, background `blue-900`; `high_contrast` = stroke `#000000`, fill `yellow-200`, pattern solid, width 4, sloppiness architect, background `#ffffff`. The background is applied only when `set_background` is true, in the same undo step, and does not depend on `scope`. The name ignores case, `-` and spaces (`High-Contrast` is `high_contrast`). Props a kind does not support are skipped as in `style.set` (a text takes only the stroke colour), and `not_applicable` follows `style.set`. Output `{changed, theme, applied, skipped, defaults_updated, background_set}`. Tests assert these exact bundles. |
| `style.list_themes` | none | Always available. | `{changed: false, themes: [{name, props, background?}]}`, in the order default, monochrome, pastel, blueprint, high_contrast. `props` holds only the props the theme sets. |

Initial style defaults (the old `UiState::default`, kept): stroke `indigo-500` (`#6366f1`), no fill, pattern solid, width 2, stroke style solid, sloppiness artist, corner radius 0, opacity 1, font gelpen, weight regular, size 20, align left.

### 2.8 `text`: text, labels and math

Typing, caret movement, selection and IME composition inside an active text edit belong to the text editor and are not commands (see `docs/exceptions.md`, E16). The editor hands its result to the engine with `text.edit_end`. A text edit is one undo step, including the creation of the element.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `text.set` | Set text | Undoable | none | NEW (E:234-297 edited in place) |
| `text.set_latex` | Set LaTeX | Undoable | none | U:3015, A:3651 `UpdateMathLatex` |
| `text.edit_begin` | Start editing text | NoUndo | `Enter` (edit the selected element), double-click | E:378-445, 773-791 |
| `text.edit_end` | Finish editing text | Undoable | `Esc`, click away, `Mod+Enter` (these are handled by the editor, then call this) | E:253-297; A:4640 |
| `text.style_range` | Style text range | Undoable | none | A:2606 (range colour), `char_colors` |
| `text.format` | Format text | Undoable | none (inside a text edit `Mod+B` bold and `Mod+I` italic are editor keys, §6.2) | NEW |
| `text.step_size` | Step font size | Undoable (coalesce 1000 ms) | `Mod+Shift+.` increase, `Mod+Shift+,` decrease | X:actionIncreaseFontSize; NEW |
| `text.bind` | Bind text to container | Undoable | none | X:actionBoundText bindText; NEW |
| `text.unbind` | Unbind text | Undoable | none | X:actionBoundText unbindText; NEW |
| `text.wrap_in_container` | Wrap text in shape | Undoable | none | X:wrapTextInContainer; NEW |
| `text.set_autoresize` | Set auto-resize | Undoable | none | X:actionTextAutoResize; NEW |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `text.set` | `id: Ref`, `text: string`, `keep_runs: bool = true`, `inline_math?: "auto" \| "off"` | Execute: `wrong_kind`: "`text.set` edits text elements and labels. For math use `text.set_latex`."; `empty_text`: "A text element cannot be empty. Delete it with `element.delete`." (labels may be emptied: that removes the label); `locked`; `text_editing`; `not_found`. | Replaces the content of a text element, or of a container's or arrow's label. Per-character colours of the unchanged prefix and suffix are kept. Re-measures through the `TextMeasure` service and grows or re-centres the container. `$...$` in the text is a formula unless `inline_math` is `off` (`docs/model.md`, Inline math): the content keeps the dollar signs and the measure includes the formula. `inline_math` sets the element's switch in the same undo step; absent keeps it (`auto` for a new text). A formula that does not parse is not an error: it is kept and drawn as its source in red. |
| `text.set_latex` | `id: Ref`, `latex: string` | Execute: `wrong_kind`: "`text.set_latex` edits math elements. For text use `text.set`."; `invalid_latex`: "The LaTeX could not be parsed: {msg}. Fix the expression. Examples: `x^2`, `\frac{a}{b}`, `\sqrt{x}`, `\sum_{i=1}^n i`."; `locked`; `text_editing`. | Replaces the formula. On a parse error the element is unchanged. Layout happens once here, not per frame. |
| `text.edit_begin` | `id?: Ref`, `at?: Point`, `container?: Ref` (with `at` only), `width?: number` (with `at`, not with `container`: a fixed width for the new text, which then wraps and does not auto-resize; a text-tool drag), `caret: "end" \| "start" \| "all" \| int = end` | Execute: `wrong_kind` (allowed: text, math, rectangle, diamond, ellipse, arrow); `locked`; `view_only` (catalog); `bad_args`: "Give `id`, or `at`, or neither (then exactly one editable element must be selected)."; `needs_single` (thing: "text, math, shape or arrow") when neither is given. | Ends any other text edit (committed), selects the target and starts the session. `at` starts a draft: nothing exists in the document until `text.edit_end`. A container (shape or arrow) edits its label, creating it on commit. Math: the session is opened the same way; the UI shows its equation dialog and ends with `latex`. Output `{kind, text, bounds, latex?}`. |
| `text.edit_end` | `commit: bool = true`, `text?: string`, `latex?: string`, `runs?: {start: int, end: int, color?: Color, bold?: bool, italic?: bool}[]`, `style?: StyleProps` (text props chosen during the edit, applied in the same step: `stroke_color`, `opacity` and the font props; any other prop is `not_applicable`; numbers are checked as in `style.set`), `inline_math?: "auto" \| "off"` (as in `text.set`; a new text takes it too) | `not_editing`: "No text is being edited. Start with `text.edit_begin`." Execute: `empty_text` is not an error here; a blank result discards a draft or deletes the element, as the old `exit_text_edit` did (E:253). `invalid_latex`. | commit=true applies `text`/`latex`/`runs`/`style` (the editor's final content; an absent `text` keeps the element's current content and discards a draft)  and creates, updates or removes the element and its label in one undo step, then selects it. commit=false discards the session. A new text and a new label take the tool's text defaults (`style.set` with scope defaults), then `style`. A container grows to fit the label. The editor's own undo (Ctrl+Z inside the text) is local and ends here. The editor stages its result in the session (`Session::stage_text_edit`), and a command with the `Commit` gate or the `Esc` rung of `tool.cancel` ends the edit with that result instead of an empty commit. |
| `text.style_range` | `id: Ref`, `start: int`, `end: int`, `color?: Color \| null` | Execute: `bad_range`: "The range {start}..{end} is outside the text, which has {len} characters. Use a range within 0..{len}."; `wrong_kind` (text only); `locked`; `text_editing`. | Sets (or with null clears) the colour of characters `start..end`. Bold and italic of those characters stay (`text.format` changes them). During a live text edit the editor owns the runs and sends them with `text.edit_end`. |
| `text.format` | `ids?: Refs = selection`, `range?: [int, int]` (one text), `bold?: bool`, `italic?: bool` | `no_target`. Execute: `bad_args` when neither `bold` nor `italic` is given, or when `range` is given for more than one text; `bad_range` as in `text.style_range`; `not_applicable` when no target is a text or a shape or arrow with a label; `locked`, checked for all targets before anything is written; `text_editing`. | Makes the text (or the characters of `range`) bold or italic with `true`, regular or upright with `false`, and leaves a face alone when its argument is absent. Colours stay. A shape or arrow with a label formats the label. Bold and italic letters are wider, so the boxes are measured again and a container grows as needed. Stored as style runs (`{start, end, color?, bold?, italic?}`), which `element.query` reports with the field `runs`. During a text edit the command refuses with `text_editing`: the style panel sends `bold` and `italic` to the editor, which applies them to its selection (or to what is typed next when nothing is selected) and returns them in `text.edit_end` `runs`. Output `{formatted, unchanged, skipped}`. |
| `text.step_size` | `direction: "increase" \| "decrease"`, `ids: Refs = selection` | `no_target`. Execute: `not_applicable` when no target is text or math. | Moves to the next preset in 16, 20, 28, 36, then by 8 above 36 or below 16 (minimum 8, maximum 200). |
| `text.bind` | `text: Ref`, `container: Ref` | Execute: `wrong_kind`; `already_bound`: "`{container}` already has a label. Remove it first with `text.unbind`."; `locked`. | Makes the text the label of the container or arrow (stored on the text as `container`, the single authoritative side). Centres it, grows the container, and keeps the text directly above it in z-order. |
| `text.unbind` | `text: Ref` | Execute: `nothing_to_do`: "`{id}` is not a label of any element." | Detaches the label; it stays where it is as a free text element. |
| `text.wrap_in_container` | `ids: Refs = selection`, `shape: "rectangle" \| "ellipse" \| "diamond" = rectangle`, `padding: number = 16` | `no_target`. Execute: `not_applicable` when no target is a free text element. | Creates a shape around each free text element and binds the text as its label. |
| `text.set_autoresize` | `ids: Refs = selection`, `on: bool` | `no_target`. Execute: `not_applicable` (text only). | on: the box follows the content. off: the box keeps its width and the text wraps. |

### 2.9 `arrange`: z-order, align, distribute

Groups, frames and bound text count as one unit. Plain elements and whole outermost groups are the units that move.

| id | title | undo | keys (mac / other) | source |
|---|---|---|---|---|
| `arrange.z_order` | Change layer order | Undoable | forward `Mod+]`; backward `Mod+[`; front mac `Mod+Alt+]` / other `Mod+Shift+]`; back mac `Mod+Alt+[` / other `Mod+Shift+[` | U:1703-1733; A:3243-3326; X:actionZindex |
| `arrange.align` | Align | Undoable | left `Mod+Shift+Left`, right `Mod+Shift+Right`, top `Mod+Shift+Up`, bottom `Mod+Shift+Down` (centres: none) | U:1807-1847; A:3455-3604; X:actionAlign |
| `arrange.distribute` | Distribute | Undoable | horizontal `Alt+H`, vertical `Alt+V` | NEW; X:actionDistribute |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `arrange.z_order` | `op: "front" \| "back" \| "forward" \| "backward"`, `ids: Refs = selection` | `no_target`. Execute: `locked`. Already at the end of the order is Ok with changed=false. | front/back move to the top/bottom. forward/backward move one step over the neighbouring unit, jumping over a whole outermost group (or, inside an editing group, over the next level down) and over a frame's contiguous range. Inside an editing group the move is clamped to that group's z-range; children of a frame stay below their frame. Bound text travels with its container. Only the moved elements get new fractional indices. Reaches collaborators through the CRDT (the old app did not broadcast z-order, A:3243). |
| `arrange.align` | `edge: "left" \| "right" \| "top" \| "bottom" \| "center_x" \| "center_y"`, `ids: Refs = selection`, `to?: Ref` | `too_few` (n=2, things "elements (a group counts as one)", unless `to` is given): "Align needs at least 2 elements (a group counts as one), and {have} were given. Select more, or pass `to` to align to a reference element." Execute: `locked`. | Aligns the units' edge or centre to the selection bounds, or to the edge of the `to` element. `center_x` makes the x-centres equal (the old `AlignCenterV`, icon "Center (Vertical)"); `center_y` makes the y-centres equal (the old `AlignCenterH`). The old names were inverted against the axis (U:1807). Bound arrows follow. |
| `arrange.distribute` | `axis: "horizontal" \| "vertical"`, `ids: Refs = selection`, `gap?: number` | `too_few` (n=3): "Distribute needs at least 3 elements (a group counts as one), and {have} were given. Select more, or pass `ids`." Execute: `locked`. | Without `gap`, keeps the two outermost units fixed and equalises the gaps between all units along the axis. With `gap`, spaces units by exactly that gap from the first unit. |

### 2.10 `group`: groups

A group is a shared `groupIds` entry on its members, innermost first. Element ids never change when grouping or ungrouping. The old `Shape::Group` (children by value) is gone; old documents migrate to flat groups in `io`.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `group.create` | Group | Undoable | `Mod+G` | A:4973 (keyboard only in the old app); X:actionGroup |
| `group.ungroup` | Ungroup | Undoable | `Mod+Shift+G` | A:4973; X:actionGroup |
| `group.set_name` | Name group | Undoable | none | NEW §9 item 11 |
| `group.enter` | Enter group | NoUndo | none (double-click on a selected group) | X:App double-click; NEW |
| `group.exit` | Leave group | NoUndo | none (`Esc` runs `tool.cancel`) | X; NEW |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `group.create` | `ids: Refs = selection` | `no_target`. `too_few` (n=2): "Grouping needs at least 2 elements, and {have} were given. Select more, or pass `ids`." Execute: `already_grouped`: "These elements already form one group at this level. Ungroup them with `group.ungroup`, or select more elements."; `frame_and_children`: "A frame and its own children cannot be grouped together. Select either the frame or its children."; `locked`. | Adds a new group id as the outermost entry (inside an editing group: just inside it). Members become contiguous in z-order at the position of the topmost member, keeping their relative order; anything that sat between them ends up below. Members from different frames are first removed from their frames. Selection becomes the group. Output `{group, members}`. |
| `group.ungroup` | `group?: GroupId`, `ids: Refs = selection` | `no_group`: "No group is selected. Select a group first (click it, or `selection.select` with `group`), or pass `group`." Execute: `locked`. | Removes only the selected group level from its members; z-order is unchanged; frame membership of the members is recomputed. Selection becomes the next level (inner groups or members). |
| `group.set_name` | `group?: GroupId` (default the single selected group), `name: string \| null` | `no_group` (same text as above). Execute: `needs_single` when several groups are selected and `group` is not given; `bad_name`: "A group name must be 1 to 64 characters and must not contain control characters."; `locked`. | Sets or clears (null) the name of the group. The name is stored with every member (`custom["drafftink.group.<id>"]`, deviation D4), so it follows undo, sync and duplicate, and is removed by `group.ungroup`. `doc.outline` shows it. Output `{group, name}`. |
| `group.enter` | `group?: GroupId` (default the single selected group), `select?: Ref` | `no_group` (same text as above). | Sets the editing group; clicks now select members one level down. `select` selects that element's unit inside the group (the double-click target). |
| `group.exit` | none | `not_in_group`: "You are not inside a group, so there is nothing to leave." | Clears the editing group and selects that group again. |

### 2.11 `frame`: frames

Frames clip their children when rendered and export as a unit. Frames cannot be nested. Children sit directly below their frame in z-order. Membership changes by dragging (`element.move`, frame=auto), by drawing a frame around elements (`element.create`, kind=frame, capture=true), and by these commands. Rendering flags are settings: `frame.clip`, `frame.show_names`, `frame.outline`.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `frame.wrap` | Wrap in frame | Undoable | `W` | X:actionWrapSelectionInFrame; NEW |
| `frame.unwrap` | Unwrap frame | Undoable | none | X:unwrapFrame; NEW |
| `frame.rename` | Rename frame | Undoable | none | X:frame label edit; NEW |
| `frame.add` | Add to frame | Undoable | none | X:addElementsToFrame; NEW |
| `frame.remove` | Remove from frame | Undoable | none | X:actionRemoveAllElementsFromFrame; NEW |
| `frame.select_children` | Select frame contents | NoUndo | none | X:actionSelectAllElementsInFrame; NEW |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `frame.wrap` | `ids: Refs = selection`, `name?: string`, `padding: number = 16` | `no_target`. Execute: `nested_frame`: "Frames cannot be nested. Remove the frame from the selection, or unwrap it with `frame.unwrap`."; `locked`. | Creates a frame around the common bounds plus padding (0 when an image is included), adds the elements (with the rules of `frame.add`), places the frame directly above them, selects the frame. |
| `frame.unwrap` | `frame?: Ref` (default the selected frame) | `no_target`; with the default argument also `wrong_kind` and `needs_single` from the selection alone (`Session::availability`). Execute: `wrong_kind`: "`frame.unwrap` works on frames, and `{id}` is a {kind}."; `locked`. | Releases the children (frame id cleared), deletes the frame, selects the children. |
| `frame.rename` | `frame: Ref`, `name: string \| null` | Execute: `wrong_kind` (frames only); `locked`. | Sets the label; null restores the default "Frame". |
| `frame.add` | `frame: Ref`, `ids: Refs = selection` | `no_target`. Execute: `wrong_kind`; `locked`. | Makes the elements children of the frame (moving them out of another frame). Frames, and groups that contain a frame, are skipped and listed in `warnings`. Bound text joins with its container. An arrow joins only if one of its ends is bound to or lies inside a member, and leaves the frame again when neither end is inside. Children are moved to just below the frame in z-order. |
| `frame.remove` | `ids?: Refs`, `frame?: Ref` | `nothing_to_do` when no given element is in a frame (also as availability when no selected element is in one). | Clears the frame id of the given elements, or of all children of `frame`. Z-order is unchanged. |
| `frame.select_children` | `frame?: Ref` (default the selected frame) | `no_target`, `wrong_kind` and `needs_single` from the selection alone, as for `frame.unwrap`. Execute: `wrong_kind`; `nothing_to_do`: "`{id}` has no children." | Selects the frame's children, without bound text entries. |

### 2.12 `arrow`: bindings, heads, routing, point editing

Bindings are stored on the arrow only (`start_binding`, `end_binding` with `target`, `fixed_point` as ratios of the target's unrotated box, and `mode`). The reverse index is derived, never stored or synced. Arrows are `arrow` and `line` elements; commands that only make sense for arrows say so. Bindable targets: rectangle, diamond, ellipse, text, image and frames (from outside). Locked and deleted targets are ignored. Excalidraw's third mode `skip` is not exposed; the importer maps it to no binding. Bind distance in screen pixels is `clamp(15 / (min(z, 1) * 1.5), 15, 30)` with `z = zoom / 1.68` (the displayed zoom, where 1 is 100%); in world units divide by `zoom`. Settings `arrow.binding` (default on) and `arrow.midpoint_snap` (default off) switch automatic binding and midpoint snapping.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `arrow.set_type` | Set arrow type | Undoable | none | U:1603-1615, A:3054 `SetPathStyle`; X:actionChangeArrowType |
| `arrow.set_head` | Set arrowhead | Undoable | none | NEW (old: one open V head, no choice); X:actionChangeArrowhead |
| `arrow.bind` | Bind arrow end | Undoable | none | NEW; X:binding |
| `arrow.unbind` | Unbind arrow end | Undoable | none | NEW |
| `arrow.move_point` | Move arrow point | Gesture | none (drag) | E:876-1104 (Endpoint, IntermediatePoint, SegmentMidpoint handles) |
| `arrow.add_point` | Add arrow point | Undoable | none | NEW; X:linearEditor |
| `arrow.delete_point` | Delete arrow point | Undoable | `Delete` while point editing | X:actionDeleteSelected (point mode); NEW |
| `arrow.edit_points` | Edit points | NoUndo | `Mod+Enter`, double-click on a line | X:actionLinearEditor; NEW |
| `arrow.reroute` | Re-route arrows | Undoable | none | NEW |
| `arrow.swap_heads` | Swap arrowheads | Undoable | none | NEW (domain `linear::swap_heads`; not in the first catalog) |
| `arrow.move_segment` | Move elbow segment | Gesture | none (drag) | NEW; X:elbow arrow segment handle (not in the first catalog) |
| `arrow.release_segment` | Release elbow segment | Undoable | none | NEW; X:elbow arrow segment handle, double-click (not in the first catalog) |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `arrow.set_type` | `ids: Refs = selection`, `type: "sharp" \| "round" \| "elbow"` | `no_target`. Execute: `not_applicable` when no target is an arrow or line; `locked`. | sharp = straight segments (old Direct), round = smooth curve through the points (old Flowing), elbow = orthogonal route found by A* around the bound targets (old Angular, which was a fixed Z shape that ignored shapes and existed only at render time, FIX). Switching to elbow replaces intermediate points by the routed path and forces angle 0; switching away keeps the corner points. |
| `arrow.set_head` | `ids: Refs = selection`, `end: "start" \| "end" \| "both" = end`, `head: Head \| null` | `no_target`. Execute: `not_applicable` when no target is an arrow; `locked`. | Sets the arrowhead at the given end; null (or "none") removes it. The old app had one open V head at the end (size 15). |
| `arrow.bind` | `arrow: Ref`, `end: "start" \| "end"`, `target: Ref`, `mode: "orbit" \| "inside" = orbit`, `fixed_point?: [number, number]` | Execute: `wrong_kind` (arrow or line); `not_bindable`: "{kind} elements cannot be arrow targets. Bindable kinds: rectangle, diamond, ellipse, text, image, frame."; `locked`; `not_found`. | Binds that end. orbit puts the end on the target's outline plus a gap of `5 + stroke_width / 2`; inside keeps the end inside the target. Default fixed point: the outline point facing the other end. Updates the route (elbow arrows re-route once). |
| `arrow.unbind` | `arrow: Ref`, `end: "start" \| "end" \| "both" = both` | Execute: `nothing_to_do`: "`{id}` is not bound at that end."; `locked`. | Clears the binding; the geometry does not change. |
| `arrow.move_point` | `arrow: Ref`, `index: int \| "start" \| "end"`, `to: Point`, `insert: bool = false`, `bind: "auto" \| "off" \| "inside" = auto`, `snap: Snap = auto` | Execute: `locked`; `bad_args` for an index out of range (message gives the range); `not_movable`: "Elbow arrows only allow moving their two ends. Move the whole arrow, or change its type with `arrow.set_type`." (middle points of an elbow, and insert on an elbow: use `arrow.move_segment`). | Moves one point to `to`. With insert=true, `index` is a segment number (from 1: segment n joins point n-1 and point n; a closed line also has segment n+1, which joins the last point back to the first and gets its new point last) and a point is inserted at that segment's midpoint first (dragging the midpoint handle). For an end: bind=auto binds to a target under the pointer (inside the shape: inside mode; in the outline margin: orbit mode), unbinds when dragged away, and keeps the binding otherwise; bind=off ignores targets (Ctrl held); bind=inside forces inside (Alt held). Smart guides, grid and angle snap (step `snap.angle_step`, default 15 degrees; isometric snap for lines) apply per `snap` (old `ENDPOINT_SNAP_RADIUS`). Output: `{index, point, bind_highlight?: Id, guides}` for the overlay. A bind candidate wins over snapping. The binding reach uses the displayed zoom (1 is 100%) as the domain `max_binding_distance` does, in world units. Gesture command. |
| `arrow.add_point` | `arrow: Ref`, `segment: int` (from 1: segment n joins point n-1 and point n; a closed line also has the closing segment), `at?: Point` | Execute: `wrong_kind` (arrow or line); `not_movable` (elbow, same text as in `arrow.move_point`); `locked`; `bad_args` for a segment out of range. | Inserts a vertex on that segment (default at its midpoint). Output `{index}`. |
| `arrow.delete_point` | `arrow?: Ref` (default the line in the point editor), `index?: int` (default the point selected in the point editor) | Execute: `nothing_to_do` when the point editor is closed and no `arrow` is given, or no point is selected and no `index` is given; `bad_args` for an index out of range; `too_few`: "A line needs at least 2 points, so no point can be deleted from `{id}`." (a closed line needs 3); `not_movable` (elbow); `locked`. | Removes one point, or every point selected in the point editor. Deleting an end frees its binding. Clears the editor's selected points. |
| `arrow.edit_points` | `arrow?: Ref` (default the single selected line or arrow), `on?: bool` (omitted toggles, except that `select_point` alone turns the editor on), `select_point?: int \| null` (selects or clears the highlighted point; the UI sets it when a vertex handle is clicked; `bad_args` when the editor is off or the point does not exist) | `needs_single` (thing: "line or arrow"). Execute: `wrong_kind`. | Turns the linear point editor on or off. While on, `Delete` removes the selected point (`arrow.delete_point`) and clicking a midpoint adds one. The selected point is observed editor state: it is restored by undo. |
| `arrow.reroute` | `ids: Refs = selection` | `no_target`. Execute: `not_applicable` when no target is an arrow or line. | Clears fixed elbow segments, re-solves the bindings from the targets' current positions and re-routes. A repair tool for agents and users. Lines are accepted as well as arrows, since a line can be bound. |
| `arrow.swap_heads` | `ids: Refs = selection` | `no_target`. Execute: `not_applicable` when no target is an arrow; `locked`. | Swaps the start and end arrowheads. An elbow route is made again, as the heads need room. |
| `arrow.move_segment` | `arrow: Ref`, `segment: int` (from 1: segment n joins point n-1 and point n), `to: Point` | Execute: `wrong_kind` (not an elbow arrow); `locked`; `bad_args` for a segment out of range (message gives the range) and for a segment the router cannot pin. | Drags that segment across its axis so it passes through `to`, and pins it there; the pin survives later moves of the arrow's ends. Gesture command. |
| `arrow.release_segment` | `arrow: Ref`, `segment: int` | Execute: `wrong_kind` (not an elbow arrow); `locked`; `bad_args`; `nothing_to_do`: "Nothing to release: segment {n} of `{id}` is not pinned." | Unpins the segment and routes that part of the arrow again. |

### 2.13 `image`: images

Images are stored once per file (content hash) in the document's files table. Paste, drop and the picker all use this command.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `image.insert` | Insert image | Undoable | `I` is the UI verb `ui.insert_image` (picker), which calls this; `Mod+V` and drops also route here | A:5376-5468; file_ops 171-230, 939+; X:image tool |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `image.insert` | `images: Bytes[]` (1 to 20), `at: Point = viewport_center`, `fit: number = 800`, `offset: number = 20`, `select: bool = true`, `downscale: bool = true`, `name?: string` (used in messages for pictures that carry no name) | Always available. Execute: `unsupported_format`: "`{name}` is not a supported image ({found}). Use PNG, JPEG or WebP."; `parse_failed`: "{name} could not be read as an image: {msg}. Try another file."; `limit_exceeded` (more than 20 images, or an image over 25 MiB). | Decodes the header for the size, stores the bytes (re-encoded to at most 2048 px on the long side when over 2 MiB and downscale=true), and creates image elements whose longest side is at most `fit` (smaller images keep their size, as the old 800 px rule), centred on `at`, each further image shifted by `offset`. Selects them. Replaces the old paste, drop and picker code paths, and the stale camera captured by the web drop handler (A:1917, FIX). |

### 2.14 `view`: camera, modes, screenshots

The camera is a scroll point (the world point at the centre of the viewport) and a zoom. Zoom is clamped to 0.1..30. The UI shows percent as `zoom / 1.68 * 100`. Per-frame camera movement by pointer, wheel or touch is an exception (`docs/exceptions.md`, E10 and E11): the UI applies it to the viewport directly and reports the final camera with `view.set`. The commands below are the scriptable equivalents and the final-state commands.

| id | title | undo | keys (mac / other) | source |
|---|---|---|---|---|
| `view.set` | Set camera | NoUndo | none | NEW (final state of pan and zoom gestures) |
| `view.pan` | Pan | NoUndo | wheel; `PgUp`/`PgDn` and `Shift+PgUp`/`Shift+PgDn` call `view.scroll_page` | A:4434, 4244, 4212 |
| `view.zoom_at` | Zoom at point | NoUndo | `Mod`+wheel (factor 1.1 or 0.9) | A:4449; `camera.zoom_at` |
| `view.zoom_in` | Zoom in | NoUndo | `Mod+=`, `Mod++` | U:900, A:2874 (x1.25); X |
| `view.zoom_out` | Zoom out | NoUndo | `Mod+-` | U:866, A:2881 (x0.8); X |
| `view.zoom_reset` | Reset zoom | NoUndo | `Mod+0` | U:887, A:2890; X |
| `view.zoom_to_fit` | Zoom to fit | NoUndo | `Shift+1` (all), `Shift+2` and `Shift+3` (selection) | U:930, A:3328; X:actionCanvas |
| `view.center` | Centre view | NoUndo | none | U:915, A:2894 |
| `view.scroll_page` | Scroll by page | NoUndo | `PgUp`, `PgDn`, `Shift+PgUp`, `Shift+PgDn` | X (NEW) |
| `view.set_viewport` | Set viewport size | NoUndo | none (the shell calls it on resize) | A:2267 |
| `view.set_view_only` | View-only mode | NoUndo | `Alt+R` | X:actionToggleViewMode; NEW |
| `view.get` | Get view | NoUndo, read-only | none | NEW §11 |
| `view.screenshot` | Screenshot | NoUndo, read-only | none | NEW §11 |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `view.set` | `scroll?: Point`, `zoom?: number` | Always available. `bad_args` for a non-finite number. | Sets the camera (zoom clamped). The UI calls it at the end of a pan or zoom gesture and, at most every 100 ms, during one so that `view.get`, follow mode and screenshots stay fresh. |
| `view.pan` | `dx: number`, `dy: number` (screen pixels; the content moves by the same amount as the pointer would) | Always available. | Pans. A wheel line is 20 px (A:4434). `Shift`+wheel pans horizontally (NEW). |
| `view.zoom_at` | exactly one of `factor: number`, `zoom: number`; `at: ScreenPoint \| "viewport_center" = viewport_center` | Execute: `bad_args`: "Give exactly one of `factor` or `zoom`." | Zooms keeping the point under `at` fixed. The wheel passes factor 1.1 (in) or 0.9 (out) as the old fixed step; a pinch passes the finger-distance ratio. Clamps to 0.1..30. |
| `view.zoom_in` | `at: ScreenPoint \| "viewport_center" = viewport_center` | Always available. | Multiplies zoom by 1.25. |
| `view.zoom_out` | `at: ScreenPoint \| "viewport_center" = viewport_center` | Always available. | Multiplies zoom by 0.8. |
| `view.zoom_reset` | `recenter: bool = false` | Always available. | Sets zoom to 1.68 (shown as 100%). The old command did not keep the centre; recenter=true moves the content centre to the viewport centre. |
| `view.zoom_to_fit` | `scope: "auto" \| "all" \| "selection" \| "ids" \| "rect" = auto`, `ids?: Refs`, `rect?: Rect`, `padding: number = 50`, `max_zoom: number = 3.36` (200% displayed; a single small element is not blown up) | Execute: `nothing_to_do`: "There is nothing to fit: the document is empty. Draw something, or use `view.center`." (also for scope selection with no selection: "...nothing is selected...") | auto fits the selection if there is one, else all content (the old rule, A:3328). Replaces the unused `Canvas::fit_to_content` duplicate. |
| `view.center` | `on: "origin" \| "content" \| "selection" \| "point" = origin`, `point?: Point` | Execute: `nothing_to_do` for content or selection that is empty. | Moves the camera without changing zoom. The old `CenterCanvas` set the offset to zero, which is `on=origin`. |
| `view.scroll_page` | `direction: "up" \| "down" \| "left" \| "right"`, `fraction: number = 0.8` | Always available. | Scrolls by that fraction of the viewport. |
| `view.set_viewport` | `width: number`, `height: number`, `scale_factor: number = 1` | `bad_args` unless width and height are above 0. | Tells the engine the canvas size in logical pixels. Needed for fit, centre and screenshots. Not exposed to agents. |
| `view.set_view_only` | `on?: bool` (omitted toggles) | Always available. | While on, every command with `view_only = Refused` refuses with `view_only`; only select, hand and laser tools work; copying and exporting still work. Session state, not saved with the document. |
| `view.get` | none | Always available. | `{scroll, zoom, percent, viewport: {w, h, scale_factor}, visible: Rect, view_only, tool, tool_lock}`. |
| `view.screenshot` | `region: "viewport" \| "all" \| "selection" \| Rect \| {ids: Refs} = viewport`, `scale: number = 1`, `max_side_px: int = 2048`, `background: bool = true`, `overlay: bool = false` | Execute: `port_unavailable`: "`view.screenshot` needs the renderer, which this session does not provide (headless without a GPU). Read the structure with `doc.outline` instead."; `nothing_to_do` for an empty region; `render_pending` (the web, as for `doc.export`). | Renders a PNG and returns `{bytes: {base64, mime}, width, height}` so an agent can check what it drew. overlay=true includes selection handles and guides. Never changes state, and never writes files (use `doc.export` for that). |

### 2.15 `history`: undo and redo

Local undo per peer, backed by Loro's `UndoManager`. See §1.8.

| id | title | undo | keys (mac / other) | source |
|---|---|---|---|---|
| `history.undo` | Undo | NoUndo | `Mod+Z` | A:4944, 3131; U:795; X:actionHistory |
| `history.redo` | Redo | NoUndo | `Mod+Shift+Z`, `Mod+Y` (all platforms, as in the old app) | A:4944, 4964, 3145; U:808 |
| `history.clear` | Clear history | NoUndo | none | X:history.clear (API); NEW |
| `history.state` | History state | NoUndo, read-only | none | NEW §11 |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `history.undo` | `steps: int = 1` (1 to `history.depth`) | `nothing_to_undo`: "There is nothing to undo in this tab." `gesture_active`, `text_editing` (catalog; inside the text editor, undo is the editor's own). | Reverts the last step(s) of this peer, restores the selection and editing group, clears nothing else. Entries that would change nothing visible (for example after a remote delete) are skipped. The inverse is a normal CRDT edit, broadcast like any other (FIX: the old keyboard path did not broadcast, A:4944). Asking for more steps than exist takes all that exist; if every remaining step had become a no-op, they are dropped and the command refuses `nothing_to_undo`. Output `{undone: [labels], can_undo, can_redo}`. batch: Never. |
| `history.redo` | `steps: int = 1` (1 to `history.depth`) | `nothing_to_redo`: "There is nothing to redo in this tab." `gesture_active`, `text_editing`. | Re-applies undone steps and restores the selection from after them. Output `{redone: [labels], can_undo, can_redo}`. batch: Never. |
| `history.clear` | none | `gesture_active`. | Drops the undo and redo stacks of the active tab; the document stays. Output `{undo_dropped, redo_dropped}`. `doc.open`, `doc.new` and `tab.duplicate` do this themselves. |
| `history.state` | none | Always available. | `{can_undo, can_redo, undo_label, redo_label, undo_depth, redo_depth}`. |

### 2.16 `collab`: rooms and presence

Remote edits arrive through `Session::apply_remote` and the same document and CRDT path as local edits. They are not commands and are never undoable by this peer. The engine repairs what a merge of two peers' edits leaves broken (docs/crdt.md section 10a); those writes are not undoable either. The wire protocol is versioned and shared with the server (`protocol` crate). Commands only start work: they change the inputs of the collab client and never touch a socket. The shell supplies the transport through the `Net` port and calls `Session::collab_poll` (on a transport wake, at the time it asked for, and after commands), which moves the bytes, hands every local commit (undo and redo included: the poll compares `TabDoc::local_writes`) to the client and applies what the room sent. The connection state is read with `collab.status` or `Session::collab_view`.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `collab.connect` | Connect to server | NoUndo | none | A:3160; U:2321 |
| `collab.disconnect` | Disconnect | NoUndo | none | A:3180; U:2324 |
| `collab.join_room` | Join room | NoUndo | none | A:3192; U:2369; web `?room=`, `#room=`, CLI `--room` |
| `collab.leave_room` | Leave room | NoUndo | none | A:3202; U:2359 |
| `collab.set_user` | Set name and colour | NoUndo | none | A:3215, 3226; U:2394, 2450 |
| `collab.set_pointer` | Publish pointer | NoUndo | none (pointer move) | A:4140-4160 |
| `collab.laser` | Laser trail | NoUndo | none (laser tool) | NEW (the old laser was local only, E:561) |
| `collab.follow` | Follow collaborator | NoUndo | none | NEW (the client supports following a viewport) |
| `collab.status` | Collab status | NoUndo, read-only | none | NEW |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `collab.connect` | `url: string = settings collab.server_url`, `room?: string`, `on_local: "refuse" \| "merge" \| "discard" = refuse`, `token?: string` (1 to 4096 bytes) | Execute: `invalid_url`: "`{url}` is not a valid server address. Use a ws:// or wss:// URL such as `wss://host/ws`. A bare `/ws` works only in the web app, where it means this page's server." followed by the reason (for example that plain ws:// to another machine sends the drawing unencrypted); `local_content` as in `collab.join_room`, when `room` is given; `already_connected`: "Already connected to `{url}`. Disconnect first with `collab.disconnect`."; `port_unavailable` (no `Net` port). | With `room`: starts the connection and joins that room, and returns at once with `{state: "connecting", url, room, link}` (FIX: the web client lost the join sent while CONNECTING, A:1989; the client sends the join when the socket opens). Without `room`: only chooses and checks the server and returns `{state: "ready"}`, because the protocol's join message carries the room, so no socket can open before a room is known; `collab.join_room` then joins. A path such as `/ws` is resolved by the `Net` port against the page's server (web only). The shell reads the URL parameters and CLI flags (`RoomLink::parse` applies the old app's forms: `host:port` becomes `ws://host:port/ws`) and calls this. `token` is the access token for a server that asks for one (`docs/server.md`). With `room` it goes into the join; without, it is kept with the chosen server for the next `collab.join_room`, which may carry its own (that one wins). `leave_room` keeps it, `disconnect` forgets it. It is deliberately an argument and not a setting: a setting would be readable with `settings.get` by every agent, written to disk and echoed in outputs. The ring records `"<hidden>"` for it (`CommandSpec::secret_args`), no output or status shows it, and a bad token is `bad_args` without the text. batch: Never. |
| `collab.disconnect` | none | `not_connected`: "Not connected, so there is nothing to disconnect." | Leaves the room (sending what is still waiting), closes the socket, forgets the chosen server and clears the peer list. The local document stays. |
| `collab.join_room` | `room?: string` (omitted creates a random 10-character id), `on_local: "refuse" \| "merge" \| "discard" = refuse`, `token?: string` | `not_connected`: "Not connected to a server. Connect first with `collab.connect`." Execute: `already_in_room`: "Already in room `{room}`. Leave it first with `collab.leave_room`."; `local_content`: "This tab already has content, which joining would merge into room `{room}`. Pass on_local=\"merge\" to merge it, on_local=\"discard\" to replace it with the room's content, or open a new tab first with `tab.new`."; `bad_args` for an invalid room name. | Binds the room to the active tab (`cmd::tab::RoomBinding`) and starts incremental sync (Loro updates, never snapshots). Returns at once; the catch-up of a big room is applied by `collab_poll`. An empty tab takes it in slices of a few milliseconds (`loading: importing {done, total}` in `collab.status`; `doc.save` and `doc.export` refuse with `room_loading` meanwhile; `collab.leave_room` finishes it). A tab with content gets it in one piece after `collab_poll` held it for one poll so the UI can draw `loading: receiving` first. `bad_args` for a room name that is not 1 to 64 characters of letters, digits, `-` and `_`. With an untouched document (blank or intro) `on_local` is not consulted. discard clears history. Output `{room, link}` where `link` is `#room=<id>&server=<url>`. |
| `collab.leave_room` | none | `not_in_room`: "This tab is not in a room, so there is nothing to leave." | Sends what is still waiting, leaves, stops syncing this tab and removes its peers. The local copy of the document stays, and the server stays chosen. |
| `collab.set_user` | `name?: string` (0 to 40 characters), `color?: Color` (`#rrggbb`) | Execute: `bad_args` for a bad colour. | Stores the name and colour in the settings `collab.user_name`, `collab.user_color` and updates presence (a name is cut to 64 bytes on the wire). Default colour `#6366f1`. Nine preset colours in the picker: `#ef4444 #f97316 #eab308 #22c55e #06b6d4 #3b82f6 #6366f1 #a855f7 #ec4899`. |
| `collab.set_pointer` | `point: Point \| null`, `tool?: "pointer" \| "laser"` | Always available. | Publishes the local pointer in world coordinates (null: left the canvas, which clears the remote cursor; the old app never cleared it, A:4140). Throttled by time by the client, at most one awareness message per 50 ms (the protocol constant `AWARENESS_MIN_INTERVAL_MS`, or the server's slower rate; the old throttle counted events). `tool: laser` makes peers draw the recent path as a trail (protocol v1 has no laser flag). Ephemeral. Not exposed to agents. |
| `collab.laser` | `points: Point[]`, `fade_ms: int = 330` | Always available. | Shows a fading trail locally (`collab.status` `laser`, each point gone after `fade_ms`) and to peers (the last point becomes the pointer). At most 1024 points, `fade_ms` 1 to 10000. Nothing is stored. |
| `collab.follow` | `peer?: string` (an id or name from `collab.status`; omitted stops following) | `not_in_room` as in `collab.leave_room`. Execute: `no_such_peer`: "There is nobody called `{peer}` in this room. List the people with `collab.status`." | The camera of the room's tab follows the peer's visible rectangle until the user moves the view (the poll notices a camera it did not set and stops, with a notice). |
| `collab.status` | none | Always available. | `{state: "disconnected" \| "ready" \| "connecting" \| "connected" \| "error", url?, room?, tab?, text, error?, unsynced, loading?: {phase: "connecting" \| "syncing" \| "receiving", bytes?}, peers: [{id, name, color, pointer?, selection, viewport?, trail, followed}], laser: [...]}`. `ready`: a server is chosen, no room joined. |

### 2.17 `settings`: persisted settings

Settings live in the `Storage` settings namespace and are loaded at start (FIX: the old app persisted nothing, so colours, snap toggles, grid, export scale, name and server reset on every launch). Keys and defaults are in §5. Setting commands are never undoable and never touch the document.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `settings.get` | Get settings | NoUndo, read-only | none | NEW |
| `settings.set` | Set setting | NoUndo | none | NEW; A:2898-2992 (snap, export scale) |
| `settings.toggle` | Toggle setting | NoUndo | `Alt+S` (key=snap.smart_guides); see §6.1 | A:2870, 2898, 2902, 2906; X:actionToggleGridMode, ObjectsSnapMode |
| `settings.cycle` | Cycle setting | NoUndo | `Mod+'` (key=view.grid_style); see §6.1 | A:2870 `ToggleGrid` |
| `settings.reset` | Reset settings | NoUndo | none | NEW |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `settings.get` | `key?: string` | Execute: `unknown_setting` (below). | Without `key`: `{settings: {key: value}}` for every setting. With `key`: the same for that one key, plus `default`, `accepts` (what the key takes, as a phrase) and `summary`. |
| `settings.set` | `key: string`, `value: any` | Execute: `unknown_setting`: "Unknown setting `{key}`. Did you mean `{a}`? List settings with `settings.get`." (the "Did you mean" part only when a key is close); `bad_args`: "Setting `{key}` must be {expected}, and got {got}." (`{expected}` is the phrase `settings.get` returns as `accepts`); `bad_args` for a read-only key (`doc.recent`): "Setting `{key}` is read only: `doc.open` and `doc.save` maintain it. Read it with `settings.get`." | Validates against the key's type and range (§5), applies the value to the session at once and queues the write to `Storage`. The same value is Ok with changed=false and writes nothing. Output `{changed, key, value, previous, default}`. Applying live: most settings are read where they are used; `style.defaults` also rebuilds the style of new elements (`tool.style_defaults`); `history.depth` is read at every commit. A session without a storage port keeps the value for the session. |
| `settings.toggle` | `key: string` | Execute: `unknown_setting`; `bad_args`: "Setting `{key}` is not an on/off setting. Use `settings.set` or `settings.cycle`." | Flips a boolean setting; output as `settings.set`. |
| `settings.cycle` | `key: string`, `step: int = 1` | Execute: `unknown_setting`; `bad_args`: "Setting `{key}` has no fixed list of values. Use `settings.set`." | Moves to the next value of an enumerated setting, wrapping. Grid style order: none, lines, horizontal_lines, crosses, dots, none (the old `GridStyle::next`). |
| `settings.reset` | `key?: string`, `all: bool = false` | Execute: `unknown_setting`; `bad_args`: "Give either `key` or all=true." (neither or both given); `bad_args` for a read-only key. | Restores defaults and stores them. all=true leaves read-only keys alone. Output `{changed, reset: [keys that changed]}`. Never changes the document. |

#### The stored record

The settings are one JSON record in the `Settings` namespace of `Storage`, key `settings`: `{"schema": 1, "values": {"snap.grid": true, "theme": "dark"}}`. `values` holds only the keys that differ from their default, so a default that changes in a later release reaches everyone who never chose a value. `Session::load_settings` reads it once at start (not a command). A value that fails its check, or a key this version does not know, is skipped and listed in the report; a damaged record is reported and replaced by the next change. A record with a newer `schema` is read as far as it validates and is never overwritten, so a downgrade cannot destroy what a newer version stored. A write that empties `values` deletes the record.

### 2.18 `library`: the shape library

Items are saved groups of elements for reuse. Every library command needs the storage port (`port_unavailable` otherwise, also as availability, because the library is kept there). The library is persisted in the `Storage` library namespace, is not part of any document, and has no undo. It reads and writes `.excalidrawlib` (v1 and v2) and replaces the old import-into-a-new-tab flow (A:1689, 2294).

| id | title | undo | keys | source |
|---|---|---|---|---|
| `library.add` | Add to library | NoUndo | none | X:actionAddToLibrary; NEW |
| `library.insert` | Insert from library | Undoable | none | X:LibraryUnit; NEW |
| `library.remove` | Remove from library | NoUndo | none | NEW |
| `library.rename` | Rename library item | NoUndo | none | NEW |
| `library.list` | List library | NoUndo, read-only | none | NEW |
| `library.import` | Import library | NoUndo | none | U:717, A:2833, 3671 `LoadLibrary`; X:data/library.ts |
| `library.export` | Export library | NoUndo | none | X:serializeLibraryAsJSON |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `library.add` | `ids: Refs = selection`, `name?: string` | `no_target`. Execute: `empty_name` (as `doc.rename`); `library_newer`: "The stored library was written by a newer drafft.ink (format {n}), so it is left alone. Update the app to change it." (every command that writes the library); `limit_exceeded` (10 000 items). | Stores a copy of the elements (ids regenerated on insert; groups, bindings among them and bound text kept; a bound text comes with its container and a frame with its children) as a new item, moved so that its top-left corner is at 0, 0. Output `{item: {id, name, element_count, bounds, created}}`. |
| `library.insert` | `item: string` (id or name), `at: Point = viewport_center`, `select: bool = true` | Execute: `no_such_item`: "There is no library item `{item}`. List items with `library.list`." Two items with the same name: `bad_args` (pass the id). | Inserts the item like a paste (the `edit::library` insert rule, as `doc.open` mode=merge): new element and group ids, centred on `at`, on top of the z-order, selected. Output `{item, ids}`. |
| `library.remove` | exactly one of `items: string[]`, `all: bool` (all=true is the only way to remove everything) | Execute: `bad_args`: "Give either `items` or all=true."; `no_such_item`; `library_newer`. | Removes items. Not undoable, so removal is always explicit. |
| `library.rename` | `item: string`, `name: string` | `no_such_item`; `empty_name`; `library_newer`. | Renames an item. The same name is Ok with changed=false. |
| `library.list` | `search?: string` | Needs storage. | `{items: [{id, name, element_count, bounds, created}]}`. `search` matches id or name, case does not matter. Thumbnails are drawn by the UI from the items. |
| `library.import` | `source: FileRef \| Bytes`, `mode: "merge" \| "replace" \| "open_as_tab" = merge` | Execute: `parse_failed`: "The file is not an Excalidraw library: {msg}. Libraries are .excalidrawlib files."; `io_failed`; `library_newer`; `too_many_tabs` (open_as_tab, as `tab.new`); `limit_exceeded` (10 000 items). | merge adds items and skips duplicates by content hash (the same shapes, whatever their ids, seeds, names and position). replace replaces the whole library (use with care: not undoable). open_as_tab is the old behaviour: lays the items out on a grid in a new tab. Output `{added, skipped, total, tab?}`. |
| `library.export` | `items?: string[]` (default all), `to: "inline" \| FileRef = inline` | Execute: `nothing_to_do`: "The library is empty, so there is nothing to export."; `no_such_item`; `io_failed`. | Writes `.excalidrawlib` v2. Output `{items, size, bytes?, saved_to?}`. |

### 2.19 `diagram` and `layout`: build diagrams from structure

Both are agent-first (plan §11) and also serve the UI (Mermaid import, a "tidy up" action). Layout algorithms live in `edit`: `layered` (Sugiyama style, handles cycles), `tree` (needs a forest), `grid`, `force`, and `none` (use given positions). Distances are world units.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `diagram.create` | Create diagram | Undoable | none | NEW §11 |
| `diagram.import_mermaid` | Import Mermaid diagram | Undoable | none (`clipboard.paste` also routes here) | U:1945, A:2798; core `shapes_from_mermaid` |
| `diagram.extend` | Add connected node | Undoable | `Mod+Arrow keys` | X:flowchart; NEW |
| `layout.apply` | Auto-layout | Undoable | none | NEW §11 |

Types: `Node = {id: string, label?: string, shape?: "rectangle" | "ellipse" | "diamond" | "text", width?: number, height?: number, name?: string, style?: StyleProps}`, `Edge = {from: string, to: string, label?: string, arrow_type?: "sharp" | "round" | "elbow", start_head?: Head | null, end_head?: Head | null, style?: StyleProps, name?: string}`, `LayoutOpts = {algorithm: "layered" | "tree" | "grid" | "force" | "none" = layered, direction: "down" | "right" | "up" | "left" = down, node_gap: number = 40, layer_gap: number = 80, columns?: int}`. Node `id` is a symbolic name chosen by the caller. Beyond the types above, a node also accepts `size: [w, h]`, and `x`, `y` (required for every node when the algorithm is `none`), and an edge accepts `arrowheads: none | end | start | both` (default `end`; `start_head` and `end_head` override it).

| id | args | availability and refusals | effect |
|---|---|---|---|
| `diagram.create` | `nodes: Node[]`, `edges: Edge[] = []`, `layout: LayoutOpts`, `at: Point \| "viewport_center" \| "below_content" = viewport_center`, `group: bool = false`, `frame?: string` (wraps the result in a frame with this name), `select: bool = true` | Execute: `bad_args`: "Give at least one node."; `duplicate_node`: "Node id `{id}` appears twice. Node ids must be unique."; `unknown_node`: "Edge {index} refers to node `{id}`, which is not in `nodes`. Known nodes: {list}."; `tree_not_forest`: "The tree layout needs every node to have at most one incoming edge and no cycles, and node `{id}` breaks that. Use algorithm \"layered\", which handles cycles."; `limit_exceeded` (2000 nodes, 5000 edges); `name_taken`, `bad_name` (as `element.set_name`, for a node or edge `name`). | Creates the nodes (size measured from the label with a minimum of 80 by 48, label stored as bound text), places them with the layout, and creates one bound arrow per edge (both ends bound in orbit mode, edge label as the arrow's bound text, routed). One undo step. Output `{nodes: {symbolic: id}, edges: [ids], bounds, created}`. In a batch the symbolic ids are available as `$<step>.<node>`. |
| `diagram.import_mermaid` | `source: "clipboard" \| {text: string}`, `at: Point \| "viewport_center" = viewport_center`, `group: bool = true`, `select: bool = true` | Execute: `mermaid_unsupported`: "Mermaid diagram type `{type}` is not supported yet. Supported: flowchart/graph and sequenceDiagram."; `parse_failed`: "The Mermaid text could not be read: {msg} (line {n}). Fix that line and try again."; `clipboard_empty`; `port_unavailable` for source clipboard without the `Clipboard`. | Parses with the native Mermaid parser (flowchart and graph with LR/RL/TB/TD/BT, fourteen node shapes, solid/dotted/thick edges, labels, subgraphs; sequenceDiagram with participants, messages, self-messages, notes), creates elements with bound arrows (the old import made straight arrows that knew nothing about the shapes), groups them when group=true (the old import always made one group), and uses a layered layout in the Mermaid direction. Each `subgraph` becomes a **cluster box**: a rounded rectangle with a light slate fill and a text element with the title centred in a 32 unit band at the top, plus 22 units of padding around the members. Boxes nest as the subgraphs do and lie behind their members. Titled rectangles were chosen over frames because a frame clips what it holds and an element belongs to one frame, so frames cannot nest. With group=true the box, its title and everything inside form one group, and the groups nest innermost first; with group=false the boxes are drawn and nothing is grouped. The members of a subgraph are laid out as a graph of their own, so they always lie inside the box and sibling boxes never overlap; a `direction` line inside a subgraph sets the flow inside it. An edge to a subgraph id ends at its box. **Node shapes:** arrows bind only to rectangles, ellipses and diamonds, and only those hold a label, so every Mermaid outline maps to one of the three: rectangle (`[x]`, subroutine `[[x]]`), rounded rectangle (`(x)` radius 16, stadium `([x])` 24), ellipse (`((x))` 80 by 80, double circle `(((x)))` 96 by 96), diamond (`{x}`). Hexagon `{{x}}` (radius 20), cylinder `[(x)]` (10), parallelogram `[/x/]` `[\x\]` (6), trapezoid `[/x\]` `[\x/]` (4), flag `>x]` (2), subroutine and double circle are approximations: the warnings name each one once ("…were drawn as the closest shape…: cylinder, hexagon"). The old app drew a hexagon as a polygon line; a line takes neither a label nor a binding here, so it would not stay connected when moved. Output is as `diagram.create`, plus `clusters` (subgraph id to the element id of the box). |
| `diagram.extend` | `from?: Ref` (default: the single selected element), `direction: "up" \| "down" \| "left" \| "right"`, `label?: string`, `gap: number = 80` | Execute: `wrong_kind` (allowed: rectangle, diamond, ellipse); `locked`; `not_found`; `needs_single` when `from` is omitted and the selection is not one element. | Creates a node of the same kind and style next to `from` in that direction with a bound arrow from `from` to it, and selects it. If the spot is taken it picks the next free one in the same direction. |
| `layout.apply` | `algorithm: "layered" \| "tree" \| "grid" \| "force" = layered`, `ids?: Refs`, `filter?: Query`, `direction`, `node_gap`, `layer_gap`, `columns` (as LayoutOpts), `anchor: "keep" \| "top_left" \| "center" = keep`, `at?: Point` (with anchor=top_left: where the top-left of the result goes; without `at`, top_left is the same as keep), `reroute: bool = true` | Default targets: the selection. `too_few` (n=2, things "elements"). Execute: `tree_not_forest` (as above); `locked`; `limit_exceeded`. | Treats non-arrow elements as nodes and arrows bound between them as edges, moves the nodes, re-routes the bound arrows (elbow routes once, after all moves), and keeps bound text with its shapes. anchor=keep leaves the top-left of the set's bounds where it was. |

### 2.20 `gesture`: gesture protocol

The protocol is specified in §1.10. These four commands are the registry face of it. They are not exposed to agents (agents call the plain command), are not palette entries, and are rejected inside batches.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `gesture.begin` | Begin gesture | NoUndo | none | NEW (solidcraft `live` frames) |
| `gesture.update` | Update gesture | NoUndo | none | NEW |
| `gesture.end` | End gesture | Undoable (the committing step) | none | NEW |
| `gesture.cancel` | Cancel gesture | NoUndo | none (UI: `Esc`, blur, pointer cancel) | A:4530 (only touch cancel in the old app) |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `gesture.begin` | `command: string`, `args: object` | Execute: `gesture_active` (catalog); `not_gesturable`: "`{command}` cannot run as a gesture. Gestures work with: element.move, element.resize, element.rotate, arrow.move_point, arrow.move_segment, style.set."; plus any refusal of the first frame. | Captures the base state, runs the first live frame, returns `{gesture, ...frame output}`. |
| `gesture.update` | `gesture: string`, `args: object` | Execute: `no_gesture`: "There is no running gesture `{gesture}`. Start one with `gesture.begin`." A refusal keeps the last good frame. | Restores the base values of touched elements and re-runs the command live with the absolute args. |
| `gesture.end` | `gesture?: string` (default the running one), `args?: object` (default the last frame's) | `no_gesture`. Execute: any refusal of the command; it aborts the gesture and restores the base state. | Restores the base, runs the command non-live, commits one undo step labelled with the command title, broadcasts once, and writes one ring entry. |
| `gesture.cancel` | `gesture?: string` | Always available. With no running gesture it is Ok with changed=false (a stray cancel on window blur is normal). | Restores the base state, discards the frames. |

### 2.21 `engine`: introspection of the registry and session

These are what `window.drafftink.commands()`, `recent()` and the MCP tool list are built from. All are read-only and allowed in every state.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `engine.commands` | List commands | NoUndo, read-only | none | NEW (solidcraft `engine.commands`); X:ActionManager |
| `engine.describe` | Describe command | NoUndo, read-only | none | NEW (solidcraft `engine.describe`) |
| `engine.recent` | Recent commands | NoUndo, read-only | none | NEW §5 |
| `engine.state` | Session state | NoUndo, read-only | none | NEW |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `engine.commands` | `family?: string`, `search?: string`, `available_only: bool = false`, `include_schemas: bool = false` | Always available. With `include_schemas` on the web build: `schemas_loading` or `schemas_unavailable`, as for `engine.describe`. | `[{id, title, family, summary, undo, reads_only, shortcuts: {mac, other}, available: true \| {code, message}, facts, args_schema?, output_schema?}]`. `shortcuts` are the effective keys (defaults plus user overrides). |
| `engine.describe` | `id: string` | Execute: `unknown_command` with suggestions. `schemas_loading`: the web build fetches the argument schemas as a file when the page starts and this call came before they arrived; try again in a moment (`window.drafftink.describe` waits for them by itself). `schemas_unavailable`: that file could not be loaded; reload the page. `engine.commands` refuses the same way only with `include_schemas`. | The spec for one command: summary, args and output JSON Schema, facts, the refusal codes it can return, and one worked example. |
| `engine.recent` | `limit: int = 50` (max 256), `mutating_only: bool = false`, `since_seq?: int` | Always available. | The newest entries of the ring (§1.9), oldest first. |
| `engine.state` | none | Always available. | `{tabs: [{id, name, active, dirty, blocked_by_schema}], tool, tool_lock, view_only, selection: {count, groups, editing_group}, gesture?, text_edit?, revision, dirty, blocked_by_schema, collab: {state, room?}, settings_revision}`. `blocked_by_schema` is the schema version of a newer app that made the document read-only here (null normally). While it is set, every command that mutates the document refuses with `newer_schema`, except those declared `allowed_when_blocked` (`doc.new`, `doc.open`, `doc.show_intro`, `gesture.cancel`, `text.edit_end`): replacing the document starts a fresh store and clears the block (docs/crdt.md §8). |

### 2.22 `perf`: counters

| id | title | undo | keys | source |
|---|---|---|---|---|
| `perf.stats` | Performance counters | NoUndo, read-only | none | NEW (plan §5, §7) |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `perf.stats` | `since: "start" \| "last_call" = start`, `trace: bool = false` | Always available. | Returns the `profile` crate's counters and per-phase histograms (input, commands, layout, encode, GPU submit), redraw counts with their dirty reasons, and frame-time percentiles. since=last_call returns only what happened after the previous call (a mark kept in `profile`, not in the session), so a scenario can measure one phase. trace=true adds a Chrome-trace JSON of the retained spans. |

### 2.23 Removed ideas

These were considered and are not commands: `doc.import` (merging is `doc.open` with mode=merge), per-tool ids such as `tool.rectangle` (they are bindings of `tool.set`), a separate `group.select` (use `selection.select` with `group`), a separate `edit.escape` (it is `tool.cancel`), and `tool.image` (inserting an image is `ui.insert_image` then `image.insert`).

---

## 3. Read-only and introspection index

Read-only commands never change any state class (`reads_only = true`). They are allowed in every state: view-only, during a gesture, during a text edit. They are what an agent uses to look before it acts. `window.drafftink` and the bridge expose them under the same ids.

| id | Returns | Used for |
|---|---|---|
| `engine.commands` | every command with availability and effective shortcuts | palette, toolbar, MCP tool list, `window.drafftink.commands()` |
| `engine.describe` | schemas, facts and refusal codes of one command | agents, docs generation |
| `engine.recent` | the recent ring | `window.drafftink.recent()`, debugging |
| `engine.state` | tabs, tool, mode, selection summary, gesture, collab | quick orientation |
| `doc.inspect` | document summary or full element data | detailed read |
| `doc.outline` | compact text and JSON structure (nodes, edges, groups, frames) | agent orientation after edits |
| `doc.list` | recent and stored documents | open dialogs |
| `tab.list` | tabs | tab bar |
| `selection.get` | selected ids, groups, editing group, bounds | properties panel, stats panel |
| `style.get` | common value or "mixed" per prop, applicability | properties panel |
| `style.list_themes` | named style bundles | theme pickers, agents |
| `element.query` | elements matching a filter | find by text, kind, style, region, connection |
| `element.at` | element at a point | agents, hit-test checks |
| `view.get` | camera, viewport, visible rect, modes | orientation |
| `view.screenshot` | PNG of a region | agents verify what they drew |
| `history.state` | can undo or redo and their labels | undo and redo buttons |
| `collab.status` | connection, room, peers | collab button, presence panel |
| `library.list` | library items | library panel |
| `settings.get` | setting values | settings UI |
| `perf.stats` | counters, histograms, optional trace | perf gates, `window.drafftink.perf` |

The hot-path reads of the renderer and of the pointer recognizer (hit-testing under the pointer, handle hit-tests, bind and snap candidates) use `canvas` and `edit` APIs directly. They change nothing, so they are not commands. `element.at` and `arrow.move_point` expose the same logic to scripts.

---

## 4. UI decisions turned into arguments

The old app decided these with dialogs, confirm boxes, hidden modifiers or hard-coded behaviour. Each is now an argument with a safe default, so the UI, a script and an agent get the same behaviour.

| Old decision | Where | Argument | Safe default | Why this default |
|---|---|---|---|---|
| Replace the document with unsaved changes | open, new, intro, drop, tab close | `on_unsaved: refuse \| discard \| save` | refuse | Nothing is lost silently. The refusal says how to continue; the UI turns it into its confirm box. |
| Where an opened file goes | `doc.open`, `doc.drop` | `mode: replace \| new_tab \| merge` | replace for `doc.open`, new_tab for drops | A drop can happen by accident; a deliberate open replaces. |
| Overwriting an existing file | `doc.save` | `overwrite: bool` | false | The OS dialog asks; scripts do not have one. |
| No save location yet | `Mod+S` | refusal code `no_save_target` | n/a | The UI opens its picker on that code and calls again with `to`. |
| Restoring the scene of a dropped PNG | `doc.drop` | `scene_png: insert \| open` | insert | Opening replaces work; inserting does not. (Old: always replaced, A:5376.) |
| Joining a room with local content | `collab.join_room` | `on_local: refuse \| merge \| discard` | refuse | Merging into a shared room cannot be taken back by the others. |
| Closing a tab that is in a room | `tab.close` | `leave_room: bool` | false | Leaving a room is visible to others. |
| Replacing the document inside a room | `doc.new`, `doc.open` | refusal `in_room` | n/a | Replace would overwrite the shared document. Use `in_new_tab=true`. |
| What to do with frame children on delete | `element.delete` | `frame_children: delete \| release` | delete | Excalidraw behaviour; undo restores both. |
| Export region | `doc.export`, `clipboard.copy_as` | `scope: auto \| all \| selection \| ids \| frame` | auto (selection if any, else all) | The old behaviour. |
| Export look | `doc.export` | `scale`, `padding`, `background`, `dark`, `embed_scene` | the `export.*` settings (2x, 20, on, off, on) | The old fixed values, now persisted. |
| Where paste lands | `clipboard.paste` | `at: Point` | viewport centre (keyboard passes `$cursor`) | The old keyboard and button paths disagreed. |
| Plain-text paste of text that looks like Mermaid or Excalidraw | `clipboard.paste` | `plain_text: bool` | false | The old sniffing is kept; `Mod+Shift+V` turns it off. |
| Finish or discard a text edit | click away, `Esc` | `text.edit_end(commit)` | true | Matches the old exit; blank text is removed. |
| Math editor Apply or Cancel | math dialog | `text.edit_end(commit, latex)` | commit | Cancel is commit=false. |
| Ctrl held: ignore snapping | drags | `snap: Snap` | auto | A modifier is an argument, not a hidden global. |
| Ctrl held: do not bind arrow ends; Alt: bind inside | `arrow.move_point`, `element.create` | `bind: auto \| off \| inside` | auto | |
| Shift held: keep aspect, snap rotation to 15 degrees | resize, rotate | `keep_aspect`, `snap_degrees` | off | |
| Alt held: resize from the centre, duplicate on drag | resize, move | `from_center`, `duplicate` | off | |
| Shift held on click, marquee | selection | `mode: replace \| add \| remove \| toggle` | replace | |
| Ctrl held on click: pick inside a group | selection | `deep: bool` | false | |
| Esc | `Esc` | `tool.cancel(all)` | false | Excalidraw's step-by-step Esc (decision §9 item 5). `all=true` is the old do-everything Esc. |
| Which tool after drawing | tool lock | `element.create(from_tool)` and `tool.lock` | back to select | Old behaviour; lock is NEW. |
| Elbow conversion drops hand-placed points | `arrow.set_type` | n/a (undoable) | n/a | Undo is the safety net. |
| Removing library items | `library.remove` | `items` or `all: true` | no default | Not undoable, so never implicit. |
| Replacing the library on import | `library.import` | `mode: merge \| replace \| open_as_tab` | merge | |
| Clearing the document | `doc.clear` | none | no confirm | Undoable, so no confirm is needed. |
| Large image on insert | `image.insert` | `downscale: bool`, `fit` | true, 800 | Old fit rule, plus a size cap for the file. |
| Mermaid import as one group | `diagram.import_mermaid` | `group: bool` | true | The old behaviour. |
| Server URL | `collab.connect` | `url` | setting `collab.server_url` | The old default `/ws` was invalid on native. |
| Style change with nothing selected | `style.set` | `scope: auto \| selection \| defaults \| both` | auto | Edits the defaults for new elements. |
| Mixed values in a multi-selection | `style.get` | returns `"mixed"` | n/a | The old panel showed the first shape only. |
| Camera animation | zoom to fit, centre | none | instant | Smoothing, if any, is the shell calling `view.set` per frame (exceptions E10, E11). |

---

## 5. Settings keys

Persisted through the `Storage` settings namespace; read with `settings.get`, written with `settings.set`, `settings.toggle`, `settings.cycle`, `settings.reset`. A change takes effect at once and is stored at once. An unknown key refuses with `unknown_setting`. Numbers are checked against their range, a whole-number key accepts 15 and 15.0, colours are stored as `#rrggbb` (or `#rrggbbaa`). `doc.recent` is read only for commands.

| Key | Type and range | Default | Old app |
|---|---|---|---|
| `view.grid_style` | `none \| lines \| horizontal_lines \| crosses \| dots` | `lines` | `config.grid_style` (U:817, not persisted) |
| `snap.grid` | bool | false | `ui_state.grid_snap_enabled` |
| `snap.smart_guides` | bool | false | `smart_snap_enabled` |
| `snap.angle` | bool | false | `angle_snap_enabled` |
| `snap.angle_step` | number 1..90 | 15 | 15 degrees |
| `export.scale` | number 0.25..4 | 2 | `export_scale` |
| `export.padding` | number 0..200 | 20 | export padding |
| `export.background` | bool | true | white |
| `export.dark` | bool | false | none |
| `export.embed_scene` | bool | true | zTXt scene |
| `doc.default_background` | `Color` | `#ffffff` | two defaults (`#fafafa`, white) |
| `input.nudge_step` | number 0.1..1000 | 20 | `GRID_SIZE` |
| `input.box_select` | `overlap \| contain` | `overlap` | overlap only |
| `style.defaults` | `StyleProps` object | see `style` family | `UiState` colours, width, sloppiness; not persisted |
| `arrow.binding` | bool | true | none |
| `arrow.midpoint_snap` | bool | false | none |
| `arrow.type` | `sharp \| round \| elbow`; the path type of new arrows (`tool.set_arrow`) | `sharp` | none (`SetPathStyle`, selected arrows only) |
| `arrow.start_head` | a `Head` name (`none`, `arrow`, `bar`, `circle`, ...); the start head of new arrows | `none` | none |
| `arrow.end_head` | a `Head` name; the end head of new arrows | `arrow` | the open V |
| `frame.clip` | bool | true | none |
| `frame.show_names` | bool | true | none |
| `frame.outline` | bool | true | none |
| `history.depth` | int 10..1000 | 200 | 50 |
| `autosave.enabled` | bool | true | web only |
| `autosave.interval_s` | int 1..300 | 5 | 5 s |
| `clipboard.excalidraw_compat` | bool | true | none |
| `collab.server_url` | string (`ws://`, `wss://`, or `/ws` on the web) | web: this page's `/ws`; native: `ws://localhost:3030/ws` | `"/ws"` |
| `collab.user_name` | string 0..40 | empty | not persisted |
| `collab.user_color` | `Color` | `#6366f1` | not persisted |
| `agent.mcp_http` | bool; a person can write it, an agent cannot (`settings.set`, `toggle` and `reset` refuse it when the origin is `agent`) | false | none (new): serves MCP over HTTP on `127.0.0.1`, desktop only (`agents.md` section 5.4) |
| `doc.recent` | list of `FileRef`, at most 10; read only | empty | `recent_documents` |
| `theme` | `light \| dark \| system` | `system` | none (decision §9 item 13) |

Constants carried over unchanged from the old app, which are not settings: grid size 20; smart guide threshold 8; equal-spacing radius 800 with candidate caps 50 and 200; handle hit tolerance 24; rotate-handle offset 25; handle size 16 divided by zoom; `BASE_ZOOM` 1.68; zoom range 0.1..30; wheel zoom step 1.1 and 0.9; button zoom 1.25 and 0.8; double-click 500 ms and 5 px; minimum shape size 1; eraser radius 10; freehand simplify 2.0; highlighter width 12 and alpha 128; image fit 800; presence panel shows 8 peers; recent documents 10; duplicate offset 20; laser trail 50 points with a 0.33 s fade.

---

## 6. Keymap

One keymap for every platform. `Mod` is Cmd on macOS and Ctrl elsewhere. Bindings call a command with fixed args. A binding with no entry in the "command" column is a UI verb (§6.3). Keys are inert while an egui text field has focus. While the text editor has focus only the `text` entries and the chords that run with the `Commit` gate apply (§6.2). A shortcut that refuses with `no_target` or `nothing_to_do` is silent (§1.5). `when` values: `canvas` (focus on the canvas, no text edit), `text`, `always`.

### 6.1 Default bindings

| Keys (mac / other) | Command and fixed args | When | Source |
|---|---|---|---|
| `V` `1` | `tool.set` tool=select | canvas | A:5245 |
| `H` | `tool.set` tool=hand | canvas | A:5250 |
| `R` `2` | `tool.set` tool=rectangle | canvas | A:5255 |
| `D` `3` | `tool.set` tool=diamond | canvas | NEW |
| `O` `4` | `tool.set` tool=ellipse | canvas | A:5260 |
| `A` `5` | `tool.set` tool=arrow | canvas | A:5265 |
| `L` `6` | `tool.set` tool=line | canvas | A:5270 |
| `P` `7` `X` | `tool.set` tool=freehand | canvas | A:5275 |
| `T` `8` | `tool.set` tool=text | canvas | A:5280 |
| `M` `9` | `tool.set` tool=math | canvas | A:5285 |
| `F` | `tool.set` tool=frame | canvas | NEW |
| `N` | `element.create_sticky` at=`$cursor`, edit=true | canvas | X; NEW |
| `E` `0` | `tool.set` tool=eraser | canvas | A:5290 |
| `Z` `K` | `tool.set` tool=laser | canvas | A:5295 |
| `Q` | `tool.lock` | canvas | NEW |
| `Space` (hold) | `tool.hold` tool=hand; on key up the shell ends the hold | canvas | NEW |
| `Esc` | `ui.close_overlays` when an overlay is open, else `tool.cancel` | always | A:5305 |
| `Mod+Z` | `history.undo` | canvas | A:4944 |
| `Mod+Shift+Z`, `Mod+Y` | `history.redo` | canvas | A:4944, 4964 |
| `Mod+A` | `selection.all` | canvas | A:4777 |
| `Mod+C` | `clipboard.copy` | canvas | A:5061 |
| `Mod+X` | `clipboard.cut` | canvas | A:5078 |
| `Mod+V` | `clipboard.paste` at=`$cursor` | canvas | A:5106 |
| `Mod+Shift+V` | `clipboard.paste` at=`$cursor`, plain_text=true | canvas | NEW |
| `Mod+Alt+C` | `clipboard.copy_style` | canvas | NEW |
| `Mod+Alt+V` | `clipboard.paste_style` | canvas | NEW |
| `Mod+Shift+E`, `Mod+Shift+C`, `Shift+Alt+C` | `clipboard.copy_as` format=png | canvas | A:4801, 4992; X |
| `Mod+D` | `element.duplicate` | canvas | A:5210 |
| `Delete`, `Backspace` | `arrow.delete_point` (the point editor has a selected point) | point_editor | A:5299 |
| `Delete`, `Backspace` | `element.delete` | canvas | A:5299 |
| `Arrow keys` | `element.nudge` dir | canvas | A:5338 |
| `Shift+Arrow keys` | `element.nudge` dir, fine=true | canvas | NEW |
| `Shift+H`, `Shift+V` | `element.flip` axis=horizontal, vertical | canvas | NEW |
| `Tab`, `Shift+Tab` | `element.convert` reverse=false, true | canvas | X; NEW |
| `Mod+Shift+L` | `element.lock` | canvas | NEW |
| `Mod+G` | `group.create` | canvas | A:4973 |
| `Mod+Shift+G` | `group.ungroup` | canvas | A:4973 |
| `Mod+]` `Mod+[` | `arrange.z_order` op=forward, backward | canvas | NEW |
| mac `Mod+Alt+]` `Mod+Alt+[` / other `Mod+Shift+]` `Mod+Shift+[` | `arrange.z_order` op=front, back | canvas | NEW |
| `Mod+Shift+Left/Right/Up/Down` | `arrange.align` edge=left, right, top, bottom | canvas | NEW |
| `Alt+H`, `Alt+V` | `arrange.distribute` axis=horizontal, vertical | canvas | NEW |
| `W` | `frame.wrap` | canvas | NEW |
| `Mod+Shift+.` `Mod+Shift+,` | `text.step_size` direction=increase, decrease | canvas | NEW |
| `Enter` | `text.edit_begin` (no args: the single selected editable element) | canvas | NEW |
| `Mod+Enter` | `arrow.edit_points` | canvas | NEW |
| `Mod+Enter` | `text.edit_end` commit=true | text (editor-local key, §6.2) | NEW |
| `Mod+Arrow keys` | `diagram.extend` direction | canvas | NEW |
| `Alt+Arrow keys` | `selection.navigate` direction | canvas | NEW |
| `Mod+=` `Mod++` | `view.zoom_in` | canvas | NEW |
| `Mod+-` | `view.zoom_out` | canvas | NEW |
| `Mod+0` | `view.zoom_reset` | canvas | NEW |
| `Shift+1` | `view.zoom_to_fit` scope=all | canvas | NEW |
| `Shift+2` `Shift+3` | `view.zoom_to_fit` scope=selection | canvas | NEW |
| `PgUp` `PgDn` `Shift+PgUp` `Shift+PgDn` | `view.scroll_page` up, down, left, right | canvas | NEW |
| `Alt+R` | `view.set_view_only` | always | NEW |
| `Mod+'` | `settings.cycle` key=view.grid_style | canvas | NEW |
| `Alt+S` | `settings.toggle` key=snap.smart_guides | canvas | NEW |
| `Mod+S` | `doc.save` | always | A:4784 |
| `Mod+E` | `doc.export` format=png | canvas | A:4870 |
| `Mod+Delete` | `doc.clear` | canvas | NEW |
| `Mod+O` | UI verb `ui.open_file` | always | A:4790 |
| `Mod+Shift+S` | UI verb `ui.save_as` | always | NEW |
| `Mod+/`, `Mod+Shift+P` | UI verb `ui.open_palette` | always | NEW |
| `Mod+F` | UI verb `ui.open_search` | canvas | NEW |
| `Mod+K` | UI verb `ui.open_link_editor` | canvas | X:actionLink |
| `?` | UI verb `ui.open_shortcuts` | canvas | U:1998 label only |
| `Alt+Z` | UI verb `ui.toggle_zen` | canvas | NEW |
| `Alt+/` | UI verb `ui.toggle_stats` | canvas | NEW |
| `S` `G` | UI verb `ui.open_color_picker` target=stroke, fill | canvas | NEW |
| `Shift+F` | UI verb `ui.open_font_picker` | canvas | NEW |
| `I` | UI verb `ui.insert_image` | canvas | NEW |

`Mod+C`, `Mod+X`, `Mod+V` and `Mod+Shift+V` are host clipboard chords (`Binding::host_clipboard`). A shell that delivers clipboard events (the browser, the native shell) runs the command from the event, which carries the system clipboard, and the keymap skips the binding for that host. One key press is one command. Hosts without clipboard events, such as a test fixture, still resolve them from the keymap.

### 6.2 Text editor keys (local, `docs/exceptions.md` E16)

Inside a text edit these are handled by the editor and never reach the registry (table `edit/src/text_edit/keys.rs`): characters and Space; Enter (new line); Backspace, Delete (with `Mod`/`Alt`: by word); arrows, Home, End (`Shift`: extend selection; `Mod`/`Alt`: by word or to the ends; on a Mac `Cmd` goes to the ends of the line and of the text); `Mod+A` select all; `Mod+C`, `Mod+X`, `Mod+V` (clipboard events of the shell, the editor's own copy); `Mod+Z`, `Mod+Shift+Z`, `Mod+Y` (editor-local undo and redo, one step per run of typing, per paste, per style change); `Mod+B` and `Mod+I` (bold and italic: they switch the faces of the selection off when all of it has them and on otherwise; with a caret they set the style of the text typed next, until the caret moves; typed text otherwise takes the style of the character before it); `F2` opens the source of the formula at the caret; `Mod+Shift+M` turns the typesetting of `$...$` off or on for this text (sent as `inline_math`); a click, a drag, a double click (word, or a formula's source) and a triple click (paragraph); IME composition. Two keys end the edit and call the registry: `Esc` and `Mod+Enter` call `text.edit_end` (commit=true) with the editor's result; a click on the canvas outside the text lets `selection.select_at` run with its `Commit` gate. Chords bound to a command with the `Commit` gate (`Mod+S`, `Mod+E`, `Mod+Shift+S` and the other `doc.*` and `tab.*` bindings) and clicks on the toolbar commit the edit and then run: the gate commits with the result the editor staged (`Session::stage_text_edit`, docs/exceptions.md E16) and not with an empty commit, so nothing typed is lost. All other `Mod`+letter chords are ignored inside a text edit (as in the old app). Behaviour changes from the old editor: `Mod+Enter` used to insert a newline (`text_editor.rs:263`) and now ends the edit; word movement with `Alt`, mouse selection, per-character colour, bold and italic and IME composition are new; the old `Mod+Z` swallow (A:4586) is replaced by editor-local undo.

### 6.3 UI verbs

Not commands, never in `engine.commands`. Each ends, when it produces a result, by calling a command.

| UI verb | Opens or does | Ends with | Old |
|---|---|---|---|
| `ui.close_overlays` | closes menu, popovers, dialogs | none | A:5305-5335 flags poked from the keyboard |
| `ui.toggle_menu` | the hamburger menu | none | `menu_open` |
| `ui.open_file` | file picker (native `rfd`, browser input) or the list of stored documents | `doc.open` | U:1907-1912, A:373-398 |
| `ui.open_recent` | recent documents list | `doc.open` | U:1912 |
| `ui.save_as` | name field, then the save picker | `doc.rename` (when the name changed), then `doc.save` with `to` | U:1900, 2631 |
| `ui.insert_image` | image picker | `image.insert` | NEW |
| `ui.open_color_picker` | colour popover (stroke, fill or background) | `style.set` or `doc.set_background` | U:1166-1195, 1007 |
| `ui.open_font_picker` | font family popover | `style.set` | NEW |
| `ui.open_palette` | command palette, listing `engine.commands` | the chosen command | NEW |
| `ui.open_search` | search box over `element.query` | `selection.select`, `view.zoom_to_fit` | NEW |
| `ui.open_link_editor` | link field for the single selected element (the field shows the current link) | `element.set_link` | X:actionLink |
| `ui.open_shortcuts` | shortcuts dialog built from the keymap | none | U:1998 |
| `ui.open_collab` | collaboration dialog | `collab.*` | U:2105 |
| `ui.open_context_menu` | context menu listing available commands | the chosen command | NEW |
| `ui.toggle_stats` | stats panel (reads `selection.get`, edits with `element.set_bounds`, `style.set`) | those commands | NEW |
| `ui.toggle_zen` | hides the chrome | none | NEW |
| `ui.open_export` | export dialog (format, scope, scale, background, dark, embed scene) | `doc.export` or `clipboard.copy_as` | X:ImageExportDialog |
| `ui.open_preferences` | preferences dialog | `settings.set`, `settings.toggle`, `collab.set_user` | X:DefaultItems |
| `ui.toggle_library` | library panel | `library.*` | X:LibraryMenu |
| `ui.rename_document` | rename field for the document | `doc.rename` | X:ProjectName |
| `ui.open_about` | About dialog: version, commit, licence, the link to the source code (AGPL-3.0 section 13) and a note that the fonts have their own licences (`ATTRIBUTION.md`). The address is the project repository unless the operator sets another (`drafftink-server --source-url`, `DRAFFTINK_SOURCE_URL` for a native build) | none (a `HostRequest::OpenUrl` for the link) | NEW |
| `ui.open_equation_editor` | LaTeX dialog (opened after `text.edit_begin` on a math element) | `text.edit_end` | U:2911 |

### 6.4 Conflicts and how they were resolved

| Conflict | Decision |
|---|---|
| `9`: old Math, Excalidraw image | Math keeps `M`/`9` (old app). Image insertion is `I`, a UI verb. |
| `Mod+E`: old export PNG; Excalidraw `Mod+Shift+E` opens the export dialog | `Mod+E` stays `doc.export`. `Mod+Shift+E` stays the old copy-as-PNG. The export dialog has no default key. |
| `Mod+Shift+C` is also the browser devtools chord | Kept because the old app had it; `Mod+Shift+E` and `Shift+Alt+C` do the same. |
| Old tool keys accepted Shift+letter; Excalidraw uses Shift+H and Shift+V for flip | Tool keys fire without Shift. `Shift+H`/`Shift+V` flip. |
| `Space` was a virtual left mouse button | Space is now pan-while-held (`tool.hold`). The virtual click is dropped (open question 1). |
| `Mod+O` on the web restored the last autosave, on native it opened a picker | Always a picker (`ui.open_file`). Restoring the autosave is `doc.open` with `{stored: "autosave"}`, offered at start-up. |
| Plain arrow key: old nudge 20; Excalidraw 1 unit | Plain arrow keeps 20 (`input.nudge_step`). `Shift` is the fine step of 1. |
| `Mod+Y` redo on macOS | Kept on all platforms (old behaviour). |
| `Z` laser (old) and `K` laser (Excalidraw) | Both. |
| `X` freehand alias (Excalidraw) and Excalidraw's `Shift+X` autoshape | `X` is freehand. Autoshape is not planned. |
| Browser chords (`Mod+D` bookmark, `Mod+S` save page, `Mod+E`, `Mod+G`) | The web shell calls `preventDefault` for every bound chord while the canvas has focus (the old page did it for `E`, `S`, `G` only, web/index.html:70). It never claims `Mod+R`, `Mod+T`, `Mod+W`, `Mod+N`, `Mod+L`. |
| A shortcut inside an egui text field | Never handled (the old app skipped everything when egui wanted input). |

---

## 7. Old action to command mapping

Every `UiAction` variant, every keyboard binding, every pointer gesture and every other user-reachable action of the old app (commit `8ce40ab`) is below. "UI verb" means §6.3. "Editor-local" means `docs/exceptions.md`.

### 7.1 All 71 `UiAction` variants (U:396-538)

| # | Variant | New command | Notes |
|---|---|---|---|
| 1 | `SetTool(kind)` | `tool.set` tool | One implementation; ends text edit (FIX) |
| 2 | `SetStrokeColor(c)` | `style.set` props.stroke_color | During a text edit with a selected range the colour goes to the editor's runs (editor-local) and is committed by `text.edit_end` (replaces the `text_selection_state` hack, A:2578) |
| 3 | `SetFillColor(opt)` | `style.set` props.fill_color | `None` is null, not transparent (FIX) |
| 4 | `SetStrokeWidth(w)` | `style.set` props.stroke_width | Undoable now |
| 5 | `SaveLocal` | `doc.save` | Web stores by name; native picks via `no_save_target` |
| 6 | `SaveLocalAs` | UI verb `ui.save_as`, then `doc.save` to | |
| 7 | `ShowOpenDialog` | UI verb `ui.open_file` (lists `doc.list` stored on the web), then `doc.open` | |
| 8 | `ShowOpenRecentDialog` | UI verb `ui.open_recent` using `doc.list` recent, then `doc.open` | |
| 9 | `SaveLocalWithName(name)` | `doc.rename` name, then `doc.save` to={stored: name} (a batch) | Drops the `doc.id` swap hack (A:2731) |
| 10 | `LoadLocal(name)` | `doc.open` source={stored: name} | Native ignored the name (FIX) |
| 11 | `SaveDocument` (dead) | `doc.save` | |
| 12 | `LoadDocument` (dead) | `doc.open` via `ui.open_file` | |
| 13 | `DownloadDocument` (web) | `doc.save` to={download: "<name>.json"} | |
| 14 | `UploadDocument` (web) | `ui.open_file`, then `doc.open` source=bytes | |
| 15 | `ImportMermaidFromClipboard` | `diagram.import_mermaid` source=clipboard | |
| 16 | `SwitchTab(i)` | `tab.switch` tab=i | |
| 17 | `CloseTab(i)` | `tab.close` tab=i | |
| 18 | `LoadLibrary` | picker, then `library.import` mode=open_as_tab | Exact old behaviour; merge is the new default |
| 19 | `ExportPng` | `doc.export` format=png, scope=auto | |
| 20 | `CopyPng` | `clipboard.copy_as` format=png | |
| 21 | `ToggleGrid` | `settings.cycle` key=view.grid_style | |
| 22 | `ZoomIn` | `view.zoom_in` | |
| 23 | `ZoomOut` | `view.zoom_out` | |
| 24 | `ZoomReset` | `view.zoom_reset` | |
| 25 | `CenterCanvas` | `view.center` on=origin | |
| 26 | `ToggleGridSnap` | `settings.toggle` key=snap.grid | |
| 27 | `ToggleSmartSnap` | `settings.toggle` key=snap.smart_guides | |
| 28 | `ToggleAngleSnap` | `settings.toggle` key=snap.angle | |
| 29 | `SetFontSize(px)` | `style.set` props.font_size | |
| 30 | `SetMathFontSize(px)` | `style.set` props.font_size | Merged: the separate action only existed to invalidate a cache |
| 31 | `SetFontFamily(u8)` | `style.set` props.font_family | 0 gelpen, 1 noto_sans, 2 gelpen_serif, 3 vanilla_extract |
| 32 | `SetFontWeight(u8)` | `style.set` props.font_weight | |
| 33 | `SetCornerRadius(r)` | `style.set` props.corner_radius | |
| 34 | `SetExportScale(n)` | `settings.set` key=export.scale | |
| 35 | `ClearDocument` | `doc.clear` | |
| 36 | `ShowIntro` | `doc.show_intro` | |
| 37 | `SetSloppiness(u8)` | `style.set` props.sloppiness | |
| 38 | `SetFillPattern(u8)` | `style.set` props.fill_pattern | |
| 39 | `SetPathStyle(u8)` | `arrow.set_type` | 0 sharp, 1 round, 2 elbow |
| 40 | `SetStrokeStyle(u8)` | `style.set` props.stroke_style | |
| 41 | `Undo` | `history.undo` | |
| 42 | `Redo` | `history.redo` | |
| 43 | `Connect(url)` | `collab.connect` url | |
| 44 | `Disconnect` | `collab.disconnect` | |
| 45 | `JoinRoom(room)` | `collab.join_room` room | |
| 46 | `LeaveRoom` | `collab.leave_room` | |
| 47 | `SetUserName(name)` | `collab.set_user` name | |
| 48 | `SetUserColor(hex)` | `collab.set_user` color | |
| 49 | `SetBgColor(c)` | `doc.set_background` color | Saved with the document now |
| 50 | `BringToFront` | `arrange.z_order` op=front | |
| 51 | `SendToBack` | `arrange.z_order` op=back | |
| 52 | `BringForward` | `arrange.z_order` op=forward | |
| 53 | `SendBackward` | `arrange.z_order` op=backward | |
| 54 | `ZoomToFit` | `view.zoom_to_fit` scope=auto | |
| 55 | `Duplicate` (dead) | `element.duplicate` | |
| 56 | `CopyShapes` (dead) | `clipboard.copy` | |
| 57 | `CutShapes` (dead) | `clipboard.cut` | |
| 58 | `PasteShapes` (dead) | `clipboard.paste` | |
| 59 | `AlignTop` | `arrange.align` edge=top | |
| 60 | `AlignBottom` | `arrange.align` edge=bottom | |
| 61 | `AlignLeft` | `arrange.align` edge=left | |
| 62 | `AlignRight` | `arrange.align` edge=right | |
| 63 | `AlignCenterH` | `arrange.align` edge=center_y | Old name was inverted: it aligned the y-centres |
| 64 | `AlignCenterV` | `arrange.align` edge=center_x | It aligned the x-centres |
| 65 | `ShowShortcuts` | UI verb `ui.open_shortcuts` | |
| 66 | `ToggleCalligraphy` | `style.set` props.freehand_style | Toggle becomes an explicit value |
| 67 | `TogglePressureSimulation` | `style.set` props.pressure_mode | uniform or simulated; device is NEW |
| 68 | `FlipHorizontal` | `element.flip` axis=horizontal | |
| 69 | `FlipVertical` | `element.flip` axis=vertical | |
| 70 | `SetOpacity(f)` | `style.set` props.opacity | Slider drag is one gesture, one undo step (FIX of A:3639) |
| 71 | `UpdateMathLatex(id, s)` | `text.set_latex` | Or `text.edit_end` latex when ending a math edit session |

### 7.2 Keyboard (A:4540-5369, canvas context)

| Old binding | New command | Notes |
|---|---|---|
| `Space` (press, release) | `tool.hold` tool=hand, `tool.release` | The virtual left click is dropped (open question 1) |
| `Ctrl+A` | `selection.all` | |
| `Ctrl+S` | `doc.save` | |
| `Ctrl+O` | UI verb `ui.open_file` | |
| `Ctrl+Shift+E` | `clipboard.copy_as` format=png | |
| `Ctrl+E` | `doc.export` format=png | |
| `Ctrl+Z` | `history.undo` | Broadcasts now |
| `Ctrl+Shift+Z` | `history.redo` | |
| `Ctrl+Y` | `history.redo` | |
| `Ctrl+G` | `group.create` | |
| `Ctrl+Shift+G` | `group.ungroup` | |
| `Ctrl+Shift+C` | `clipboard.copy_as` format=png | Duplicate of Ctrl+Shift+E, kept |
| `Ctrl+C` | `clipboard.copy` | |
| `Ctrl+X` | `clipboard.cut` | |
| `Ctrl+V` | `clipboard.paste` at=`$cursor` | Routing: own payload, Excalidraw, image, Mermaid, text |
| `Ctrl+D` | `element.duplicate` | |
| `V` `1` | `tool.set` select | |
| `H` | `tool.set` hand | |
| `R` `2` | `tool.set` rectangle | |
| `O` `4` | `tool.set` ellipse | |
| `A` `5` | `tool.set` arrow | |
| `L` `6` | `tool.set` line | |
| `P` `7` | `tool.set` freehand | |
| `T` `8` | `tool.set` text | |
| `M` `9` | `tool.set` math | |
| `E` | `tool.set` eraser | |
| `Z` | `tool.set` laser | |
| `Delete`, `Backspace` | `element.delete` | |
| `Escape` | `ui.close_overlays`, else `tool.cancel` | Closing overlays is no longer done by the keyboard handler poking UI flags |
| `Arrow keys` | `element.nudge` | One undo step per burst |
| Text edit: `Esc` | editor-local, then `text.edit_end` | |
| Text edit: `Ctrl+C`, `Ctrl+X`, `Ctrl+V` | editor-local | Through the `Clipboard` port |
| Text edit: `Ctrl+A` | editor-local | |
| Text edit: `Backspace`, `Delete`, `Enter`, arrows, `Home`, `End`, `Space`, characters (with `Ctrl` word/document variants and `Shift` extension) | editor-local | |
| Text edit: other `Ctrl+letter` | ignored | As before |
| Highlighter tool: no key | `tool.set` highlighter, no default key | The toolbar tooltip no longer claims `H` |
| `3`, `0` unused | `3` is diamond, `0` is eraser | NEW |

### 7.3 Pointer, touch, window and system events

| Old input or event | New route | Notes |
|---|---|---|
| Click on a shape; Shift+click toggle | `selection.select_at` (mode=replace; toggle with Shift) | E:486-538 |
| Click on empty canvas | `selection.select_at` mode=replace (clears) | |
| Marquee drag, release over 2 by 2 px | overlay in the recognizer, then `selection.select_region` | E:539-547, 714-729; E9 |
| Lasso: drag with the lasso tool (from empty canvas, or from an element that is not selected), or Alt-drag from empty canvas with the select tool; Shift adds | the path is an overlay part; on release one `selection.select_lasso` (`points`, `simplify` = 4 screen px, mode add with Shift). A path under 2 by 2 px or without area selects nothing and says nothing. A press on an unselected element with the lasso tool selects on release when it was a click. Esc drops the path | X:App.tsx lasso trail (Alt-drag `fromSelection`); E9 |
| Drag a selected shape (grid and smart snap) | `gesture.begin` with `element.move`, `update`, `end` | E2 |
| Alt+drag to duplicate | the same with duplicate=true | E:514-536 |
| Corner or edge handle drag, Shift keeps aspect | gesture of `element.resize` (keep_aspect) | E3 |
| Endpoint, intermediate point, segment-midpoint handles | gesture of `arrow.move_point` (insert=true for the midpoint); elbow segment handles: gesture of `arrow.move_segment`, double-click: `arrow.release_segment` | E5 |
| Rotate handle, Shift 15 degrees | gesture of `element.rotate` (snap_degrees=15) | E4 |
| Double-click the rotate handle | `element.rotate` mode=to, angle=0 | E:447-462 |
| Draw rectangle, ellipse | overlay, then `element.create` kind, from_tool=true | E6 |
| Draw line or arrow (grid, angle and isometric snap) | overlay, then `element.create` kind=line or arrow | E6 |
| Freehand, highlighter stroke | stroke buffer, then `element.create` kind=freehand | E7 |
| Text tool click on empty canvas | `text.edit_begin` at | creates a draft; E:773-791 |
| Text tool click on existing text; Select-tool double-click on text | `text.edit_begin` id | E:378-445 |
| Math tool click | `element.create` kind=math with the placeholder `x^2 + y^2 = r^2`, selected (the old code did only this; the editor opens on a double-click) | E:792-803 |
| Select-tool double-click on math | `text.edit_begin` id, then `ui.open_equation_editor` | A:4366, E:437 |
| Double-click empty canvas | `text.edit_begin` at | NEW (Excalidraw) |
| Double-click a selected group | `group.enter` select=hit | NEW |
| Eraser drag | hover preview overlay, then `element.erase` path | E:1285; E8 |
| Laser drag | ephemeral trail, `collab.laser` | E12 |
| Middle-button drag; Pan-tool drag | camera applied in the UI, then `view.set` | A:4244, 4212; E10 |
| Mouse wheel | `view.pan`; `Mod`+wheel `view.zoom_at` factor 1.1 or 0.9 | A:4434-4458; E10, E11 |
| Two-finger touch pan and pinch (max 2 fingers) | camera in the UI, then `view.set` | A:4461-4538; E10, E11 |
| Single-finger touch | the pointer routes above | |
| Touch cancel | `gesture.cancel` | A:4530 |
| Trackpad pinch (unhandled in the old app) | `view.zoom_at` through E11 | NEW |
| Right button (received and dropped) | UI verb `ui.open_context_menu` | NEW |
| Hover cursor icons | derived from hit-testing in `canvas`, no command | E1 |
| Pointer move (collab cursor, every 6th event) | `collab.set_pointer` (time throttled) | A:4140-4160 |
| Mouse release over an egui panel (swallowed, stuck drag) | UI cancels the gesture: `gesture.cancel` | A:4259 FIX |
| Window resized | `view.set_viewport` | A:2267 |
| Window focus lost | `gesture.cancel` | NEW |
| Window close requested | old: exit only (A:2263). NEW: the shell calls `doc.autosave` first | A:2263 |
| Modifier keys changed | no command; the shell tracks modifiers and passes them as arguments | A:5371 (a no-op in the old app) |
| `DroppedFile` (native), DOM drop (web) | `doc.drop`. Old native accepted only png, jpg and webp; accepting `.json`, `.excalidraw` and `.excalidrawlib` is NEW | A:5376; file_ops 939+ |
| Text-tool drag on an existing text's handle or edge | resize or move through the gestures of E3 and E2 | E:378-420 |
| Paste event (web), `Ctrl+V` | the shell fills the `Clipboard` port, then `clipboard.paste` | |
| Web start-up: restore `__last__`, else intro | `doc.open` source={stored: "autosave"}, else `doc.show_intro` | A:1860, `try_load_last_document` |
| Native start-up: load intro | `doc.show_intro` | A:1860 |
| Web `?room=&server=`, `#room=&server=`; CLI `--server`, `--room` | `collab.connect` url, room. In the old CLI `--room` without `--server` only prefilled the field; now it connects to the setting `collab.server_url` | A:1943-1989 |
| Web autosave to `__last__` | `doc.autosave` on a timer | A:2564 |
| Web `index.html` keydown `preventDefault` for E, S, G | the web shell prevents every bound chord | web/index.html:70 |

### 7.4 Panels, menus and dialogs of the old UI

| Old control | New |
|---|---|
| Tool toolbar (12 tools) | `tool.set`; adds diamond and frame |
| Top properties bar: stroke swatches, wheel, fill swatches, widths | `style.set`; wheel opens `ui.open_color_picker` |
| Colour grid popover | UI verb; its picks call `style.set` or `doc.set_background`; "None" picks null |
| Right panel sections | all `style.set`, `arrow.set_type`, `arrange.*`, `element.flip` |
| Hamburger menu rows | see §7.1 rows 5 to 20, 35, 36, 65 |
| Bottom-left toolbar: undo, redo, grid, background, zoom, centre, fit, snap toggles | §7.1 rows 41, 42, 21, 49, 22 to 25, 54, 26 to 28 |
| Tab bar: tabs, close, "+ Library" | `tab.switch`, `tab.close`, `library.import` open_as_tab; `tab.new` is NEW |
| Collab button and modal | `ui.open_collab`; actions `collab.connect`, `collab.join_room`, `collab.leave_room`, `collab.disconnect`, `collab.set_user` |
| Presence panel | `collab.status` |
| Remote cursors | `Session` presence state fed by `collab` |
| Save dialog | `ui.save_as`, then `doc.save` |
| Open and open-recent dialogs | `ui.open_file`, `ui.open_recent` |
| Math editor dialog (Apply, Cancel, Copy, Paste) | `ui.open_equation_editor`; Apply is `text.edit_end` with latex; Cancel is commit=false; Copy and Paste use the `Clipboard` port in the dialog |
| Shortcuts modal | `ui.open_shortcuts`, generated from the keymap (the old list had 19 stale entries) |
| Export scale buttons 1x 2x 3x | `settings.set` key=export.scale |

### 7.5 Behaviour changes from the old app

These are deliberate; each fixes an inventory finding. Reviewers should expect them.

1. Every style, font, flip, align, z-order, opacity and delete action is undoable, reaches collaborators, and goes through one path.
2. Cmd works on macOS (`Mod`).
3. Undo and redo broadcast from every surface.
4. Paste has one path; the keyboard passes `$cursor`, buttons use the viewport centre.
5. The system clipboard carries copied elements.
6. Tool keys do not fire with Shift (Shift+H and Shift+V flip).
7. Space pans while held; it is no longer a virtual mouse click.
8. Esc is one ladder (`tool.cancel`), and overlays close through `ui.close_overlays` first.
9. Align names are by axis result: `center_x`, `center_y`.
10. Eraser radius is 10 screen pixels, and a whole stroke is one undo step.
11. Group rotation is real (each member's own angle changes); grouping never changes element ids or reorders elements outside the group.
12. Arrow nudge and similar key repeats merge into one undo step per burst.
13. Opening or creating a document never silently discards unsaved changes (`on_unsaved=refuse`).
14. A pending web room join is queued until the socket opens.
15. A drag ends cleanly on blur, pointer cancel or release over a panel.
16. Settings, style defaults, user name, server URL and recent documents persist.
17. The background colour is part of the document and of exports.
18. Autosave watches the document revision, not the shape count.
19. In the text editor `Mod+Enter` ends the edit (it inserted a newline), `Alt` moves by word, IME composition works, and `Mod+Z` is the editor's own undo.
20. Closing a tab activates the right neighbour as before; a tab can be created empty with `tab.new`.
21. Paste routing keeps the old native order: own payload, Excalidraw, Mermaid, image, then plain text as a new text element.
22. The maximum zoom is 30 (it was 10). The minimum is still 0.1 and the default is still 100% (zoom 1.68). `view.set` and `view.zoom` clamp to this range (`canvas::Viewport`).
23. The camera is a centre point (`scroll`) and a `zoom`, not a screen offset, so the same document opens at the same place in a window of any size.
24. A Mermaid subgraph is a titled box around its members (section 2.19), as in the first version. Edges that start or end at a whole subgraph are drawn, bound to the box (the first version drew them from the box edge without binding).
25. The native document extension is `.drafftink`; `.json` and the first version's files still open (`docs/deviations.md` section 4).

---

## 8. Coverage of Excalidraw

### 8.1 Excalidraw shortcuts (research `04-features-diff.md` §4.1 to §4.3)

| Excalidraw | Here |
|---|---|
| `H` hand (toggles back) | `tool.set` hand; Space-hold is `tool.hold` |
| `V`/`1`, `R`/`2`, `D`/`3`, `O`/`4`, `A`/`5`, `L`/`6`, `P`/`7`/`X`, `T`/`8` | `tool.set` (§2.3) |
| `N` sticky note | `element.create_sticky` (the key places a note at the pointer and starts the text edit) |
| `9` image | `I` (UI verb), conflict §6.4 |
| `E`/`0` eraser, `F` frame, `K` laser | `tool.set` |
| `B` bucket fill, `Shift+X` autoshape | not planned |
| `I`, `Shift+S`, `Shift+G` eyedropper | not planned (`I` is image here) |
| `Q` tool lock | `tool.lock` |
| `Esc` | `tool.cancel` |
| `Enter` edit text or label | `text.edit_begin` |
| `Mod+Enter` edit line points / finish text | `arrow.edit_points` / editor-local |
| `Tab`, `Shift+Tab` convert element type | `element.convert` (cycles to the next or previous type of the family) |
| `Mod` held: do not bind | `bind=off` argument |
| click, click, click: curved arrow | recognizer collects points, `element.create` points, arrow_type=round |
| Double-click or `Enter` on image: crop | not planned |
| `Mod+=`, `Mod+-`, `Mod+0` | `view.zoom_in`, `view.zoom_out`, `view.zoom_reset` |
| `Shift+1`, `Shift+2`, `Shift+3` | `view.zoom_to_fit` |
| `PgUp`, `PgDn`, with `Shift` | `view.scroll_page` |
| Space-drag, wheel-button drag, hand tool, wheel, `Shift`+wheel, `Mod`+wheel | `tool.hold`, E10, `view.pan`, `view.zoom_at` |
| `Alt+Z` zen | UI verb `ui.toggle_zen` |
| `Alt+S` objects snap | `settings.toggle` snap.smart_guides |
| `Mod+'` grid | `settings.cycle` view.grid_style |
| `Alt+R` view mode | `view.set_view_only` |
| `Shift+Alt+D` theme | not planned |
| `Alt+/` stats | UI verb `ui.toggle_stats` |
| `Mod+F` search | UI verb `ui.open_search` (uses `element.query`) |
| `Mod+/`, `Mod+Shift+P` palette | UI verb `ui.open_palette` (uses `engine.commands`) |
| `?` help | UI verb `ui.open_shortcuts` |
| `Mod+Arrow` flowchart create | `diagram.extend` |
| `Alt+Arrow` flowchart navigate | `selection.navigate` |
| Arrow, `Shift+Arrow` nudge | `element.nudge` |
| `Delete`, `Backspace` | `element.delete` |
| `Mod+Delete` clear canvas | `doc.clear` |
| `Mod+X`, `Mod+C`, `Mod+V`, `Mod+Shift+V` | `clipboard.cut/copy/paste` |
| `Mod+A` | `selection.all` |
| `Shift+click`, `Mod+click`, `Mod+drag` | `selection.select_at` mode and deep; `selection.select_region` |
| `Shift+Alt+C` | `clipboard.copy_as` png |
| `Mod+Alt+C`, `Mod+Alt+V` | `clipboard.copy_style`, `clipboard.paste_style` |
| `Mod+[`, `Mod+]`, with `Shift` (mac `Alt`) | `arrange.z_order` |
| `Mod+Shift+Arrows` align | `arrange.align` |
| `Alt+H`, `Alt+V` distribute | `arrange.distribute` |
| `Mod+D`, Alt-drag | `element.duplicate`, `element.move` duplicate=true |
| `Mod+Shift+L` | `element.lock` |
| `Mod+Z`, `Mod+Shift+Z`, `Mod+Y` | `history.undo`, `history.redo` |
| `Mod+G`, `Mod+Shift+G` | `group.create`, `group.ungroup` |
| `W` wrap selection | `frame.wrap` |
| `Shift+H`, `Shift+V` | `element.flip` |
| `S`, `G` colour pickers; `Shift+F` font picker | UI verbs |
| `Mod+Shift+<`, `Mod+Shift+>` | `text.step_size` |
| `Mod+K` link | UI verb `ui.open_link_editor`, then `element.set_link` |
| `Mod+O`, `Mod+S`, `Mod+Shift+S` | `ui.open_file`, `doc.save`, `ui.save_as` |
| `Mod+Shift+E` export dialog | conflict §6.4 (old copy-as-PNG kept) |
| `Shift` drag constrain, `Alt` drag from centre | `keep_aspect`, `from_center`, `duplicate` arguments |

### 8.2 Plan §6 features and where they live

| Plan §6 item | Commands |
|---|---|
| 1 Arrow binding, highlight during drag | `arrow.bind`, `arrow.unbind`, `arrow.move_point` (`bind_highlight` in the frame output), `element.create` bind, `element.connect`, settings `arrow.binding`, `arrow.midpoint_snap` |
| 2 Flat groups, enter and leave | `group.create`, `group.ungroup`, `group.enter`, `group.exit`, `selection.select` deep |
| 3 Bound text, arrow labels | `text.bind`, `text.unbind`, `text.wrap_in_container`, `element.relabel`, `text.edit_begin` container |
| 4 Diamond | `element.create` kind=diamond, tool diamond |
| 5 Text align and wrap | `style.set` text_align, vertical_align, line_height; `text.set_autoresize` |
| 6 Arrowheads, elbow with A* | `arrow.set_head`, `arrow.set_type`, `arrow.reroute` |
| 7 Stroke styles | `style.set` stroke_style (all shapes) |
| 8 Lock | `element.lock`, `element.unlock_all` (also the unlock of a locked element from the context menu) |
| 9 Distribute | `arrange.distribute` |
| 10 Edge resize handles | `element.resize` handles n, e, s, w |
| 11 Context menu | UI verb `ui.open_context_menu` over `engine.commands` |
| 12 Command palette | UI verb `ui.open_palette` over `engine.commands` |
| 13 Frames | `frame.*`, `element.create` kind=frame |
| 14 Eraser | `element.erase`, tool eraser |
| 15 Tool lock | `tool.lock` |
| 16 Draw modifiers | arguments `keep_aspect`, `from_center`, `snap`, `bind`, `snap_degrees` |
| 17 SVG and `.excalidraw` export | `doc.export` format svg, excalidraw |
| 18 Search | UI verb `ui.open_search` over `element.query` |
| 19 Stats panel | UI verb `ui.toggle_stats` over `selection.get`, `element.set_bounds`, `style.set` |
| §11 `diagram.create`, `element.query`, `doc.outline`, `element.connect`, `view.screenshot`, `layout.apply`, `element.relabel`, `element.at`, named themes | the same ids, §2.19, §2.4, §2.1, §2.14, §2.7 |
| §5 introspection: `commands()`, `recent()`, `doc.inspect`, `selection.get`, `view.get`, `element.query`, `element.at`, `history.state`, `perf.stats` | §3 |

### 8.3 Not cataloged

These Excalidraw features are outside the plan. They have no ids. If one is added, its family section is added here first.

Embeddable and iframe elements, bucket fill, autoshape, eyedropper, image crop, list markers in text, chart paste, magic frame and every AI feature, dark theme and canvas theme switching, i18n, follow mode, room encryption and passwords, pen-only mode, Excalidraw+ export, shareable encrypted links, scrollbars, welcome screen, hint bar.

---

## 9. Decisions on the open questions (coordinator, 2026-11-07)

1. Space: pan while held. The virtual mouse button is dropped.
2. Key `9` stays Math; image insertion uses `I`.
3. Undo depth default is 200 (setting range 10..1000).
4. `tool.set` to a drawing tool clears the selection, as in Excalidraw.
5. Esc: `tool.cancel` defaults to step-by-step (`all=false`, Excalidraw behaviour). The old do-everything Esc is `all=true`.
6. Autosave stores a workspace record with all tabs, and restores all of them. Specify it in the `doc`/`tab` family when implementing.
7. `Mod+Shift+C` kept.
8. Accepted as written, with the stated fallback if the perf gate fails.
9. Accepted.
10. Zoom range is 0.1..30.
11. Add `group.set_name` (agents name groups like elements; `doc.outline` shows the name).
12. Deferred to the `ui` implementer; record the outcome in §6.3.
13. Add a `theme` setting (`light` | `dark` | `system`, default `system`) and `settings.set` handles it; the renderer gets dark canvas colours.
14. Accepted.
15. Native rolls back panics. On wasm, a panic hook reports the panic, and the page offers a reload that restores the last autosave (taken at least after every committed command batch, debounced). The hostile-params test is the main defence.
16. Deferred to the `ui` implementer.

## 10. Appendix A: declared facts per command

These are the values for the facts of §1.2 that the tables of §2 do not repeat. They are the registration values the family implementers must use, and `cargo xtask commands --check` compares them with the registry. Where an implementer finds a value wrong, the fix is a change to this table in the same commit.

Legend:

| Column | Values |
|---|---|
| mutates | `-` nothing. `D` document. `O` observed editor state (selection, editing group, point editor). `V` view. `T` tool and style defaults in memory. `B` tabs and per-tab document metadata (save target, saved revision). `S` settings. `C` collab session. `P` ports and session buffers (a file, the clipboard, storage, the clipboard and style buffers, the library, the recent list). `E` a gesture or text-edit session. |
| selection | `-` not used. `req` has an `ids` argument defaulting to the selection; an empty list refuses `no_target`. `soft` the selection is only a default and an empty one is not an error. |
| ports | the ports the command may need. A port is required only when the requested path needs it (`doc.export` needs `Renderer` for png only, `doc.new` needs `Files` or `Storage` only for on_unsaved=save). A missing required port gives `port_unavailable` with the path named. |
| batch | `ok` allowed anywhere. `last` only as the final step. `never` not allowed in a batch. |
| view-only | `ok` runs while the document is view-only. `no` refuses with `view_only`. |
| during gesture | `ok` allowed while a gesture runs. `no` refuses with `gesture_active`. |
| during text edit | `ok` allowed. `no` refuses with `text_editing`. `commit` the engine commits the running text edit first. |
| redraw | the dirty reason on success: `scene`, `camera`, `overlay`, or `-` for none. |
| ring | `always` or `quiet` (recorded only when it refuses). |
| palette | listed in the command palette. |
| agent | exposed as an MCP tool and in `window.drafftink.commands()`. |

| id | mutates | selection | ports | batch | view-only | during gesture | during text edit | redraw | ring | palette | agent |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `doc.new` | D,O,B,V | - | Files,Storage | last | no | no | commit | scene | always | y | y |
| `doc.open` | D,O,B,V | - | Files,Storage | last | no | no | commit | scene | always | y | y |
| `doc.drop` | D,O,B,V | - | Files,Storage | last | no | no | commit | scene | always | n | n |
| `doc.save` | P,B | - | Files,Storage | ok | ok | no | commit | - | always | y | y |
| `doc.rename` | D,B | - | - | ok | no | no | commit | scene | always | y | y |
| `doc.set_background` | D | - | - | ok | no | no | commit | scene | always | n | y |
| `doc.export` | P | soft | Files,Renderer | ok | ok | no | commit | - | always | y | y |
| `doc.clear` | D,O | - | - | ok | no | no | commit | scene | always | y | y |
| `doc.show_intro` | D,O,B,V | - | - | last | no | no | commit | scene | always | y | y |
| `doc.autosave` | P | - | Storage | ok | ok | ok | ok | - | always | n | n |
| `doc.list` | - | - | Files,Storage | ok | ok | ok | ok | - | always | y | y |
| `doc.inspect` | - | - | - | ok | ok | ok | ok | - | always | y | y |
| `doc.outline` | - | - | - | ok | ok | ok | ok | - | always | y | y |
| `tab.new` | B | - | - | last | ok | no | commit | scene | always | y | y |
| `tab.switch` | B | - | - | last | ok | no | commit | scene | always | y | y |
| `tab.close` | B | - | - | last | ok | no | commit | scene | always | y | y |
| `tab.rename` | D,B | - | - | ok | no | no | commit | scene | always | y | y |
| `tab.move` | B | - | - | ok | ok | no | commit | scene | always | y | y |
| `tab.duplicate` | B | - | - | last | ok | no | commit | scene | always | y | y |
| `tab.list` | - | - | - | ok | ok | ok | ok | - | always | y | y |
| `tool.set` | T,O | - | - | ok | ok | no | commit | overlay | always | y | y |
| `tool.lock` | T | - | - | ok | ok | no | ok | overlay | always | y | y |
| `tool.hold` | T | - | - | ok | ok | no | ok | overlay | always | n | n |
| `tool.release` | T | - | - | ok | ok | ok | ok | overlay | always | n | n |
| `tool.cancel` | D,O,T,E | - | - | ok | ok | ok | ok | overlay | always | y | y |
| `tool.set_arrow` | T,S | - | Storage | ok | ok | no | ok | overlay | always | y | y |
| `element.create` | D,O,T | - | - | ok | no | no | commit | scene | always | n | y |
| `element.delete` | D,O | req | - | ok | no | no | commit | scene | always | y | y |
| `element.duplicate` | D,O | req | - | ok | no | no | commit | scene | always | y | y |
| `element.move` | D,O | req | - | ok | no | no | commit | scene | always | n | y |
| `element.nudge` | D,O | req | - | ok | no | no | commit | scene | always | n | y |
| `element.resize` | D,O | req | - | ok | no | no | commit | scene | always | n | y |
| `element.set_bounds` | D,O | req | - | ok | no | no | commit | scene | always | n | y |
| `element.rotate` | D,O | req | - | ok | no | no | commit | scene | always | n | y |
| `element.flip` | D,O | req | - | ok | no | no | commit | scene | always | y | y |
| `element.lock` | D,O | req | - | ok | no | no | commit | scene | always | y | y |
| `element.unlock_all` | D,O | soft | - | ok | no | no | commit | scene | always | y | y |
| `element.set_link` | D | soft | - | ok | no | no | commit | scene | always | y | y |
| `element.open_link` | O,V | soft | - | ok | ok | no | commit | camera | always | y | n |
| `element.set_name` | D,O | - | - | ok | no | no | commit | scene | always | n | y |
| `element.relabel` | D,O | - | - | ok | no | no | commit | scene | always | n | y |
| `element.connect` | D,O | - | - | ok | no | no | commit | scene | always | n | y |
| `element.create_sticky` | D,O | - | - | ok | no | no | commit | scene | always | y | y |
| `element.convert` | D,O | req | - | ok | no | no | commit | scene | always | y | y |
| `element.erase` | D,O | - | - | ok | no | no | commit | scene | always | n | y |
| `element.query` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `element.at` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `selection.select` | O | - | - | ok | ok | no | commit | overlay | always | n | y |
| `selection.select_at` | O | - | - | ok | ok | no | commit | overlay | always | n | y |
| `selection.select_region` | O | - | - | ok | ok | no | commit | overlay | always | n | y |
| `selection.select_lasso` | O | - | - | ok | ok | no | commit | overlay | always | n | y |
| `selection.all` | O | - | - | ok | ok | no | commit | overlay | always | y | y |
| `selection.clear` | O | - | - | ok | ok | no | commit | overlay | always | y | y |
| `selection.get` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `selection.navigate` | O | - | - | ok | ok | no | commit | overlay | always | n | y |
| `clipboard.copy` | P | req | Clipboard | ok | ok | no | ok | - | always | y | y |
| `clipboard.cut` | D,O,P | req | Clipboard | ok | no | no | commit | scene | always | y | y |
| `clipboard.paste` | D,O,P | - | Clipboard | ok | no | no | commit | scene | always | y | y |
| `clipboard.copy_style` | P | req | Clipboard | ok | ok | no | ok | - | always | y | y |
| `clipboard.paste_style` | D | req | Clipboard | ok | no | no | commit | scene | always | y | y |
| `clipboard.copy_as` | P | soft | Renderer,Clipboard | ok | ok | no | ok | - | always | y | y |
| `style.set` | D,T,S | soft | - | ok | no | no | no | scene | always | n | y |
| `style.get` | - | soft | - | ok | ok | ok | ok | - | always | n | y |
| `style.apply_theme` | D,T,S | soft | - | ok | no | no | no | scene | always | y | y |
| `style.list_themes` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `text.set` | D | - | - | ok | no | no | no | scene | always | n | y |
| `text.set_latex` | D | - | - | ok | no | no | no | scene | always | n | y |
| `text.edit_begin` | O,E | soft | - | ok | no | no | commit | overlay | always | n | n |
| `text.edit_end` | D,O,E | - | - | ok | no | no | ok | scene | always | n | n |
| `text.style_range` | D | - | - | ok | no | no | no | scene | always | n | y |
| `text.format` | D | req | - | ok | no | no | no | scene | always | n | y |
| `text.step_size` | D | req | - | ok | no | no | no | scene | always | y | y |
| `text.bind` | D | - | - | ok | no | no | no | scene | always | n | y |
| `text.unbind` | D | - | - | ok | no | no | no | scene | always | n | y |
| `text.wrap_in_container` | D | req | - | ok | no | no | no | scene | always | y | y |
| `text.set_autoresize` | D | req | - | ok | no | no | no | scene | always | y | y |
| `arrange.z_order` | D | req | - | ok | no | no | commit | scene | always | y | y |
| `arrange.align` | D | req | - | ok | no | no | commit | scene | always | y | y |
| `arrange.distribute` | D | req | - | ok | no | no | commit | scene | always | y | y |
| `group.create` | D,O | req | - | ok | no | no | commit | scene | always | y | y |
| `group.ungroup` | D,O | soft | - | ok | no | no | commit | scene | always | y | y |
| `group.set_name` | D | soft | - | ok | no | no | commit | scene | always | n | y |
| `group.enter` | O | - | - | ok | ok | no | commit | scene | always | y | y |
| `group.exit` | O | - | - | ok | ok | no | commit | scene | always | y | y |
| `frame.wrap` | D,O | req | - | ok | no | no | commit | scene | always | y | y |
| `frame.unwrap` | D,O | soft | - | ok | no | no | commit | scene | always | y | y |
| `frame.rename` | D,O | - | - | ok | no | no | commit | scene | always | y | y |
| `frame.add` | D,O | req | - | ok | no | no | commit | scene | always | y | y |
| `frame.remove` | D,O | soft | - | ok | no | no | commit | scene | always | y | y |
| `frame.select_children` | O | - | - | ok | ok | no | commit | scene | always | y | y |
| `arrow.set_type` | D | req | - | ok | no | no | commit | scene | always | y | y |
| `arrow.set_head` | D | req | - | ok | no | no | commit | scene | always | y | y |
| `arrow.bind` | D | - | - | ok | no | no | commit | scene | always | n | y |
| `arrow.unbind` | D | - | - | ok | no | no | commit | scene | always | n | y |
| `arrow.move_point` | D | - | - | ok | no | no | commit | scene | always | n | y |
| `arrow.add_point` | D | - | - | ok | no | no | commit | scene | always | n | y |
| `arrow.delete_point` | D | - | - | ok | no | no | commit | scene | always | n | y |
| `arrow.edit_points` | O | soft | - | ok | no | no | commit | scene | always | n | y |
| `arrow.reroute` | D | req | - | ok | no | no | commit | scene | always | y | y |
| `arrow.swap_heads` | D | req | - | ok | no | no | commit | scene | always | y | y |
| `arrow.move_segment` | D | - | - | ok | no | no | commit | scene | always | n | y |
| `arrow.release_segment` | D | - | - | ok | no | no | commit | scene | always | n | y |
| `image.insert` | D,O | - | - | ok | no | no | commit | scene | always | n | y |
| `view.set` | V | - | - | ok | ok | ok | ok | camera | quiet | n | y |
| `view.pan` | V | - | - | ok | ok | ok | ok | camera | quiet | n | y |
| `view.zoom_at` | V | - | - | ok | ok | ok | ok | camera | quiet | n | y |
| `view.zoom_in` | V | - | - | ok | ok | ok | ok | camera | always | y | y |
| `view.zoom_out` | V | - | - | ok | ok | ok | ok | camera | always | y | y |
| `view.zoom_reset` | V | - | - | ok | ok | ok | ok | camera | always | y | y |
| `view.zoom_to_fit` | V | soft | - | ok | ok | ok | ok | camera | always | y | y |
| `view.center` | V | - | - | ok | ok | ok | ok | camera | always | y | y |
| `view.scroll_page` | V | - | - | ok | ok | ok | ok | camera | always | n | y |
| `view.set_viewport` | V | - | - | ok | ok | ok | ok | camera | quiet | n | n |
| `view.set_view_only` | V | - | - | ok | ok | ok | ok | overlay | always | y | y |
| `view.get` | - | - | - | ok | ok | ok | ok | - | always | y | y |
| `view.screenshot` | - | - | Renderer | ok | ok | ok | ok | - | always | n | y |
| `history.undo` | D,O | - | - | never | no | no | no | scene | always | y | y |
| `history.redo` | D,O | - | - | never | no | no | no | scene | always | y | y |
| `history.clear` | O | - | - | never | ok | no | no | scene | always | n | y |
| `history.state` | - | - | - | ok | ok | ok | ok | - | always | y | y |
| `collab.connect` | C | - | Net | never | ok | ok | ok | overlay | always | y | y |
| `collab.disconnect` | C | - | Net | never | ok | ok | ok | overlay | always | y | y |
| `collab.join_room` | C,D,B | - | Net | never | no | no | commit | overlay | always | y | y |
| `collab.leave_room` | C | - | Net | never | ok | ok | ok | overlay | always | y | y |
| `collab.set_user` | S | - | - | never | ok | ok | ok | overlay | always | n | y |
| `collab.set_pointer` | C | - | - | never | ok | ok | ok | overlay | quiet | n | n |
| `collab.laser` | C | - | - | never | ok | ok | ok | overlay | quiet | n | y |
| `collab.follow` | C,V | - | - | never | ok | ok | ok | overlay | always | n | y |
| `collab.status` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `settings.get` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `settings.set` | S | - | Storage | ok | ok | ok | ok | scene | always | n | y |
| `settings.toggle` | S | - | Storage | ok | ok | ok | ok | scene | always | y | y |
| `settings.cycle` | S | - | Storage | ok | ok | ok | ok | scene | always | y | y |
| `settings.reset` | S | - | Storage | ok | ok | ok | ok | scene | always | n | y |
| `library.add` | P | req | Storage | ok | ok | no | commit | - | always | y | y |
| `library.insert` | D,O | - | Storage | ok | no | no | commit | scene | always | y | y |
| `library.remove` | P | - | Storage | ok | ok | no | commit | - | always | y | y |
| `library.rename` | P | - | Storage | ok | ok | no | commit | - | always | y | y |
| `library.list` | - | - | Storage | ok | ok | ok | ok | - | always | y | y |
| `library.import` | P,B | - | Files,Storage | ok | ok | no | commit | - | always | y | y |
| `library.export` | P | - | Files,Storage | ok | ok | no | commit | - | always | y | y |
| `diagram.create` | D,O | - | - | ok | no | no | commit | scene | always | n | y |
| `diagram.import_mermaid` | D,O | - | Clipboard | ok | no | no | commit | scene | always | y | y |
| `diagram.extend` | D,O | - | - | ok | no | no | commit | scene | always | y | y |
| `layout.apply` | D | req | - | ok | no | no | commit | scene | always | y | y |
| `gesture.begin` | D,E | - | - | never | ok | no | no | scene | always | n | n |
| `gesture.update` | D,E | - | - | never | ok | ok | ok | scene | quiet | n | n |
| `gesture.end` | D,O,E | - | - | never | ok | ok | ok | scene | always | n | n |
| `gesture.cancel` | D,E | - | - | never | ok | ok | ok | scene | always | n | n |
| `engine.commands` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `engine.describe` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `engine.recent` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `engine.state` | - | - | - | ok | ok | ok | ok | - | always | n | y |
| `perf.stats` | - | - | - | ok | ok | ok | ok | - | always | n | y |

Notes on the table:

- Every `Undoable` command has `mutates` containing `D`. Every `Gesture` command (`element.move`, `element.resize`, `element.rotate`, `arrow.move_point`, `arrow.move_segment`, `style.set`) accepts `gesture.begin`.
- Commands with `agent` = `n` are UI plumbing (`doc.drop`, `doc.autosave`, `tool.hold`, `tool.release`, `text.edit_begin`, `text.edit_end`, `view.set_viewport`, `collab.set_pointer`, `gesture.*`). For `gesture.*` an agent uses the plain command; the others have no meaning outside a running UI.
- `settings.*` use `redraw = scene` because grid style, frame rendering and snap overlays change what is drawn. A setting that changes nothing visible still sets the reason, and the idle gate counts a redraw only if one was requested.
- `view.*` camera commands use `redraw = camera`: the renderer re-composites cached element fragments with the new transform and does not re-encode them.

### Appendix A2: document, host and agent-tool facts

Four more facts of a registration, kept apart from the table above because most commands have the default value. A command that is not listed has the default for all of them, and `registry_tests` / `catalog_tests` (`facts_match_appendix_a2`) fail when a registration differs from its row, when a row lists nothing but defaults, and when a row names an unregistered id.

| Column | Values |
|---|---|
| doc write | `not_document`: the command does not change the document (`mutates` has no `D`). `edit`: an ordinary edit of the open document: an undo class other than `NoUndo`, refuses in view-only mode. `replaces`: replaces the document wholesale (`doc.new`, `doc.open`, `doc.show_intro`, `collab.join_room`): refuses in view-only mode and is not a gesture. `managed`: manages the document state itself (`history.undo`, `history.redo`, the `gesture` commands, `tool.cancel`). Default: `edit` when `mutates` has `D`, else `not_document`. |
| host effect | What a host (the UI) does after the command ran, read from the registration instead of matched by id. `-` is the default. `completes_rename`, `completes_rename_then_save_as`, `completes_export`, `completes_library_add`, `completes_link_edit`: success closes the dialog or clears the field that asked for it. `shows_convert_popup`: success after a keyboard conversion shows the popup. `returns_url_to_open`: success returns a `url` for the host to open. `shows_search_results`: success fills the open search panel. `begins_gesture`, `ends_gesture`, `ends_gesture_on_refusal`: the host's scrub follows the gesture commands. `ends_equation_edit`: any answer closes the equation editor. |
| refusal toast | `show` (default), or `silent`: the host does not toast a `no_gesture` refusal (a scrub that lost its gesture just stops); any other refusal of the command is shown. |
| key repeat | `repeats` (default), or `first_press_only`: auto-repeat of a held key does not run the command again (`tool.hold`). |
| editor takes style | `y`: during a text edit the host sends a style choice to the text editor instead of running this command, which refuses with `text_editing` (`style.set`, `text.format`). Default `n`. |
| own agent tool | `y`: the agent bridge gives the command a tool of its own, so no family tool lists it. Default `n`. |

| id | doc write | host effect | refusal toast | key repeat | editor takes style | own agent tool |
|---|---|---|---|---|---|---|
| `collab.join_room` | replaces | - | show | repeats | n | n |
| `doc.new` | replaces | - | show | repeats | n | n |
| `doc.open` | replaces | - | show | repeats | n | n |
| `doc.rename` | edit | completes_rename_then_save_as | show | repeats | n | n |
| `doc.export` | not_document | completes_export | show | repeats | n | n |
| `doc.show_intro` | replaces | - | show | repeats | n | n |
| `element.convert` | edit | shows_convert_popup | show | repeats | n | n |
| `element.set_link` | edit | completes_link_edit | show | repeats | n | n |
| `element.open_link` | not_document | returns_url_to_open | show | repeats | n | n |
| `element.query` | not_document | shows_search_results | show | repeats | n | n |
| `engine.commands` | not_document | - | show | repeats | n | y |
| `engine.describe` | not_document | - | show | repeats | n | y |
| `engine.recent` | not_document | - | show | repeats | n | y |
| `frame.rename` | edit | completes_rename | show | repeats | n | n |
| `gesture.begin` | managed | begins_gesture | show | repeats | n | n |
| `gesture.update` | managed | ends_gesture_on_refusal | silent | repeats | n | n |
| `gesture.end` | managed | ends_gesture | silent | repeats | n | n |
| `gesture.cancel` | managed | ends_gesture | silent | repeats | n | n |
| `history.undo` | managed | - | show | repeats | n | n |
| `history.redo` | managed | - | show | repeats | n | n |
| `library.add` | not_document | completes_library_add | show | repeats | n | n |
| `library.rename` | not_document | completes_rename | show | repeats | n | n |
| `style.set` | edit | - | show | repeats | y | n |
| `text.format` | edit | - | show | repeats | y | n |
| `tab.rename` | edit | completes_rename | show | repeats | n | n |
| `text.edit_end` | edit | ends_equation_edit | show | repeats | n | n |
| `tool.hold` | not_document | - | show | first_press_only | n | n |
| `tool.cancel` | managed | - | show | repeats | n | n |
