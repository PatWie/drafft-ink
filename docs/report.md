# Final report: the command-layer rewrite of drafft-ink

Written 2026-11-09 against `main` at `f654255` (`merge: audit-b`), branch `final-report`. No code was changed for this report. Numbers marked "run" were produced for this report on 2026-11-09 (Ubuntu x86_64, 64 cores, NVIDIA L40S, Rust 1.97.1). Every other number is copied from the document named next to it, and that document's date and commit are given where they differ from HEAD.

The specification asks for: per family, the commands added, the UI copies of logic removed and those left with the reason, the gates run and their results; and every deviation from the design with its reason. Sections 2 and 6 do that. Sections 1, 3, 4, 5 and 7 give the context the brief asked for.

Sources: `crates/engine/src/cmd/*` and `drafftink-cli commands --json` (command ids), `docs/commands.md` §2 and §7, `docs/ui.md`, `docs/exceptions.md`, `crates/app/tests/fences/allowlist.txt`, `docs/audit/design-compliance.md`, `docs/audit/regression.md`, `docs/parity.md`, `docs/deviations.md`, `docs/performance.md`, `docs/backlog.md`, `git log --merges main`.

## 1. Summary

The old app (`/home/ubuntu/git/drafft-ink` at `8ce40ab`) was rewritten as a Cargo workspace of 19 library crates, 5 apps, the native e2e crate and `xtask`, in the layering used by solidcraft (`docs/architecture.md`, enforced by `cargo xtask layers`). `main` has 458 commits, 94 of them merges (`git log --merges --oneline main`: one merge per task branch, from `p0-protocol` and `p0-xtask` through the `f-*` family branches, `p5-*` UI and interaction, `p6-*` apps, the backlog and audit branches, to `audit-b`).

What exists, by product:

| Product | Crates and apps | State |
|---|---|---|
| Canvas library (Konva-like) | `geom`, `scene`, `typeset`, `sketch`, `render`, `canvas` (L0 to L3), `canvas-js` and `packages/drafftink-canvas` (npm package, wasm) | Stage, shapes, events with bubbling, R-tree hit testing, transformer, gesture recogniser, export. No layer nodes, tweens or cache API. The npm package carries the library only (D8, D9). |
| Editor (Excalidraw-class) | library plus `doc` (Loro store), `edit` (domain rules), `io` (formats), `engine` (registry, `Session`) | 153 commands in 23 families. Bound arrows, elbow arrows, nested groups, frames, lasso, flowchart helpers, library, bound text. |
| App | `collab`, `ui` (egui), `bridge`, `gpu`, `app` (glue), `apps/drafftink` (desktop), `apps/drafftink-web` (wasm, `window.drafftink`) | One winit shell for native and web (D1). Windows and macOS build in CI but never ran (D15). |
| Server | `apps/drafftink-server` | Rooms with incremental sync; stores: memory, filesystem, sqlite, s3 (the last two are non-default cargo features, see section 7). Binds `127.0.0.1` by default. |
| MCP | `apps/drafftink-mcp` | stdio headless, stdio attached to the running app, relay to a web tab, HTTP door in the desktop app, WebMCP in the page. The tool list is generated from the registry. |
| CLI | `apps/drafftink-cli` | `run`, `convert`, `outline`, `inspect`, `commands`. No PNG output (D16). |

Source size (lines of `.rs`, tests included): `engine` 60,414, `edit` 32,473, `ui` 16,704, `io` 14,093, `app` 7,800, all crates and apps together about 218,000.

The design in one paragraph: every user action is `Session::execute(id, args)` or `execute_batch`. A command is a `CommandSpec` with id, title, argument schema (schemars), availability check returning a `Refusal` with plain text, typed handler, and a `Facts` record (undo class, read-only, ports, batch, view-only, gesture and text-edit gates, redraw, ring, palette, agent). The UI, shortcuts, the palette, the bridge, MCP, the CLI and the tests all call the same two entry points. Pointer gestures are the written-down exception (`docs/exceptions.md`).

## 2. Per family

### 2.1 How to read the table

- **Commands** are the registered ids, from `drafftink-cli commands --json` (run: 153 commands, 23 families, 0 warnings). `docs/commands.md` §2 has 153 matching rows, so catalog and registry agree.
- **Old UI logic removed** names the old code (commit `8ce40ab`) that the command replaces. `A:n` is `crates/drafftink-app/src/app.rs`, `U:n` is `ui.rs`, `E:n` is `event_handler.rs`, `X:` is Excalidraw. The mapping is `docs/commands.md` §2 (source column) and §7. The old files do not exist in the new tree. Their logic was not copied into `ui`/`app`; the rewrite's UI crate calls commands (`crates/ui/tests/thin.rs`, `crates/app/tests/fences`). "NEW" means the old app had no such action.
- **Left in the UI and why** is what a user action still does outside `execute`, per `docs/ui.md` §5, `docs/exceptions.md` and the fence allowlist (section 2.3).
- **Tests**: `E` = unit tests in `crates/engine/src/cmd/<family>*_tests.rs` (counted with `cargo nextest list`, run); `S` = JSON scenario files under `tests/scenarios/<family>/` (run by `crates/testkit`; 82 files in 26 directories in all); `W` = Playwright specs under `e2e/web/specs` that call a command of the family; `N` = native e2e under `e2e/native/tests`. `W` and `N` were found by searching the specs for the quoted command ids, so a family can be exercised by more specs than it names here through batches or helper functions.
- **Gate status.** Gates run on the whole workspace, not per family. The engine tests (about 1,350), 4,254 workspace tests, the 82 scenarios and the 153-of-153 command coverage of the harness passed in this run (section 3). The only failing test of this run is a browser e2e in the `text` family (flaky, section 3.3). So every family row is "pass" except `text`/`element` rows that depend on that e2e, marked below.

### 2.2 The table

| Family | Commands (ids after `family.`) | Old UI logic removed (old ref) | Left in the UI, and why | Tests | Gate status |
|---|---|---|---|---|---|
| `doc` (13) | `new` `open` `drop` `save` `rename` `set_background` `export` `clear` `show_intro` `autosave` `list` `inspect` `outline` | Save/load/download/upload/recent/clear/intro/background/export-PNG branches of the 71-case `UiAction` match (A:2603-3667; rows 5-14, 19, 35, 36, 49 of §7.1): A:2698-2790, 2841, 2855, 3237, 3691; native and web file handlers (file_ops 939+); `Ctrl+S/O/E` (A:4784-4870); drop (A:5376-5468); web autosave keyed on shape count (A:2564-2576, now by revision); start-up restore (A:1860). `doc.new/rename/inspect/outline` are NEW. | File and save pickers and the download write: the UI returns a `HostRequest` and the app calls `doc.open` / `doc.save to=` afterwards (needs the OS). Text buffers of the Save As, Rename and Export dialogs (a draft until Apply). Autosave timer and write on close are engine ticks (`workspace_tick`, `flush_workspace`), on the fence allowlist. Start-up restore (`load_settings`, `restore_workspace`) runs once before the first frame. | E 101 (`doc_tests`, `doc_read_tests`); S 8; W document, persistence, startup, files-and-keys, legacy-fixture, pixels; N bridge, stories, mcp_http | pass |
| `tab` (7) | `new` `switch` `close` `rename` `move` `duplicate` `list` | `SwitchTab`, `CloseTab` (U:691, 705; A:1746, 3675), tab creation by library import only (A:1734). `new` `rename` `move` `duplicate` `list` are NEW. | Tab bar and tab-menu open state. Tab drag to reorder is not built (`tab.move` is a plain command with no drag behind it, exceptions E14). | E 69; S 1; W document, session, files-and-keys | pass |
| `tool` (6) | `set` `lock` `hold` `release` `cancel` `set_arrow` | `SetTool` and the two copies of the tool-key table (A:5245-5295, U:745), `Esc` ladder (A:5305), `Space` as a virtual mouse button (A:4711, replaced by `tool.hold`/`release`). `lock`, `set_arrow` are NEW. | `Esc` first closes UI overlays through the verb `ui.close_overlays` (UI state), then runs `tool.cancel`. | E 45 (`tool_tests`, `tool_arrow_tests`); S 2; W pointer, pixels, session | pass |
| `element` (21) | `delete` `duplicate` `move` `nudge` `resize` `set_bounds` `rotate` `flip` `lock` `unlock_all` `erase` `convert` `create` `set_name` `relabel` `connect` `set_link` `open_link` `query` `at` `create_sticky` | Shape creation per tool (E:566-596, 734-764, 773-846), move with Alt-duplicate (E:514-536, 656-713, 1105-1205), resize (E:609-655, 876-1104), rotate (E:447-462, 611-632, 878-902), eraser (E:1285-1311, one undo step per drag event before), Delete (A:5299), Ctrl+D (A:5210, 3354), arrow-key nudge (A:5338), flip (U:1753, 1761; A:3625-3632). `set_bounds`, `lock`, `convert`, links, names, sticky, `query`, `at`, `connect`, `relabel` are NEW. | Pointer gestures E2 to E8 (move, resize, rotate, point drag, draw, freehand, erase): the recogniser in `engine/interact` runs them as `gesture.begin/update/end` or as an overlay preview and ends in one plain command; hover E1 changes nothing. Stats-panel number buffers. Link dialog buffer. | E 213 (six `element*_tests` files); S 8; W 17 specs; N bridge, stories, mcp_http. `pointer.spec` typing-text test is flaky (3.3) | pass, one flaky e2e |
| `selection` (8) | `select` `select_at` `select_region` `all` `clear` `get` `navigate` `select_lasso` | Click, Shift-click, marquee (E:486-547, 714-729), `Ctrl+A` (A:4777), `Esc` clear (A:5305). `select_lasso`, `get`, `navigate` are NEW. | Marquee rectangle and lasso path are overlay state in the recogniser (E9); only the release runs `selection.select_region` / `select_lasso`. | E 65 (`selection_tests`, `selection_lasso_tests`); S 2; W 11 specs; N bridge, stories | pass |
| `clipboard` (6) | `copy` `cut` `paste` `copy_style` `paste_style` `copy_as` | Copy, cut, paste (A:5061-5106, 3384-3428; dead `UiAction` copies), `CopyPng` (A:4801, 4992, 3756; U:1957). Paste had two paths in the old app; there is one. `copy_style`, `paste_style` are NEW. | The system clipboard itself: the shell fills the `Clipboard` port, then runs `clipboard.paste`. Clipboard inside the text editor is editor-local (E16). | E 44; S 3; W copy-paste, elements, files-and-keys, pointer; N stories | pass |
| `style` (4) | `set` `get` `apply_theme` `list_themes` | The `SetStrokeColor`, `SetFillColor`, `SetStrokeWidth`, font, corner radius, sloppiness, fill pattern, stroke style, opacity, calligraphy and pressure cases (rows 2-4, 29-33, 37-40, 66, 67, 70 of §7.1; U:1020-1855; A:2606-3130, 3639), which each pushed or skipped history on their own. | Opacity slider drag is a gesture (E13: `gesture.begin style.set` / `update` / `end`), one undo step. During a text edit the style goes to the editor, because `style.set` refuses with `text_editing` (E16). Popover open state. | E 37; S 1; W elements, pixels, session | pass |
| `text` (10) | `set` `set_latex` `edit_begin` `edit_end` `style_range` `step_size` `bind` `unbind` `wrap_in_container` `set_autoresize` | In-place text editing and commit (E:234-297, 378-445, 773-791; A:4640), `UpdateMathLatex` (U:3015, A:3651), per-character colour hack `text_selection_state` (A:2578). Bound text, wrap, autosize, size steps are NEW. | The text editor (E16): content, caret, selection, IME composition, in-text keys, the editor's own undo. It lives in `edit::text_edit::Editor` (no UI types) and `crates/app/src/editor`. The session only knows an edit is running and receives the latest result through `Session::stage_text_edit` (not a command; allowlist and N3). Caret blink timer. | E 46; S 6; W elements; native none by id. `typing text` e2e is flaky (3.3) | pass, one flaky e2e |
| `arrange` (3) | `z_order` `align` `distribute` | Bring/send (U:1703-1733; A:3243-3326), align (U:1807-1847; A:3455-3604; the old centre names were swapped), now all undoable. `distribute` is NEW. | None. | E 23; S 1; W structure | pass |
| `group` (5) | `create` `ungroup` `set_name` `enter` `exit` | `Ctrl+G` and `Ctrl+Shift+G` (A:4973, keyboard only in the old app). Group rotation and id changes were wrong in the old app (§7.5, item 11). `set_name`, `enter`, `exit` are NEW. | Double-click on a selected group is routed by the recogniser to `group.enter`. | E 26; S 2; W grouping, structure, files-and-keys; N stories | pass |
| `frame` (6) | `wrap` `unwrap` `rename` `add` `remove` `select_children` | Nothing: the old app had no frames. All NEW (Excalidraw). | Frame name edit opens a rename dialog (`naming.rs`); the dialog buffer is UI state. | E 27; S 1; W structure | pass |
| `arrow` (12) | `set_type` `set_head` `swap_heads` `bind` `unbind` `move_point` `add_point` `delete_point` `edit_points` `reroute` `move_segment` `release_segment` | `SetPathStyle` (U:1603-1615; A:3054), endpoint/intermediate/midpoint handle drags (E:876-1104). Binding, heads, elbow routing and point editing are NEW (Excalidraw). | Arrow endpoint and vertex drag (E5): live through `gesture.*` with a bind highlight in the frame output. Bind candidate while drawing is overlay state (E6). | E 60; S 6; W arrow-attached, structure | pass |
| `image` (1) | `insert` | Native drop of png/jpg/webp (A:5376-5468; file_ops 171-230, 939+). | The file picker and decode of the dropped bytes are the shell's; the decode check is in the command. Crop has no command (section 5). | E 20; S 1; W document | pass |
| `view` (13) | `set` `pan` `zoom_at` `zoom_in` `zoom_out` `zoom_reset` `zoom_to_fit` `center` `scroll_page` `set_viewport` `set_view_only` `get` `screenshot` | Zoom buttons and fit (U:866-930; A:2874-2894, 3328), wheel (A:4434-4458), middle drag and pan tool (A:4212, 4244), touch pan/pinch (A:4461-4538), window resize (A:2267). | Pan and zoom are class C exceptions (E10, E11): the shell moves the camera at once and reports `view.set` at the end and at most every 100 ms. Reason: one matrix update per event, no document. | E 29; S 3; W 9 specs; N bridge, stories, mcp_http | pass |
| `history` (4) | `undo` `redo` `clear` `state` | `Undo`/`Redo` (A:4944-4964, 3131-3145; U:795, 808). Undo from the toolbar did not reach collaborators in the old app; all surfaces do now. | None. | E 20; S 2; W 18 specs | pass |
| `collab` (9) | `connect` `disconnect` `join_room` `leave_room` `set_user` `set_pointer` `laser` `follow` `status` | `Connect`, `Disconnect`, `JoinRoom`, `LeaveRoom`, user name and colour (A:3160-3226; U:2321-2450), URL `?room=` parsing (A:1943-1989), pointer broadcast every 6th event (A:4140-4160). `laser` (old laser was local), `follow`, `status` are NEW. | Laser trail and fade are presence (E12), not stored. Remote edits enter through `Session::apply_remote` (N2), a tick not a user action. Collab dialog text fields. The web dialog has no token field (`collab.join_room token=` is the way). | E 38; S 1; W collab, session | pass |
| `settings` (5) | `get` `set` `toggle` `cycle` `reset` | Grid, snap and export-scale toggles (A:2870-2992), `SetExportScale`. Settings were not persisted in the old app. | The layout choice (touch/desktop) is UI state and is not persisted (no settings key). | E 39; S 2; W document, session; N bridge | pass |
| `library` (7) | `add` `insert` `remove` `rename` `list` `import` `export` | `LoadLibrary` (U:717; A:2833, 3671). The rest is NEW (Excalidraw library). | Import opens a file picker (host request), then `library.import`. A dragged library item runs `library.insert at=` on release (E14, no recogniser). Previews are boxes, not thumbnails. | E 26; S 3; W document | pass |
| `diagram` (3) + `layout` (1) | `diagram.create` `import_mermaid` `extend`; `layout.apply` | `ImportMermaidFromClipboard` (U:1945; A:2798) and core `shapes_from_mermaid`. `create`, `extend`, `layout.apply` are NEW (agents). | None. | E 21 (`diagram`) + 5 (`layout`); S 2 + 1; W flowchart, document, structure; N stories, mcp_http | pass |
| `gesture` (4) | `begin` `update` `end` `cancel` | Touch cancel only (A:4530). The protocol itself is NEW. | It is the protocol the recogniser uses for E2 to E5 and E13. | E 5; S 1; W session; N bridge | pass |
| `engine` (4) + `perf` (1) | `engine.commands` `describe` `recent` `state`; `perf.stats` | NEW. They are the introspection the specification asks for. | None. | E 13 + 6; S 1 + 1; W startup, session; N bridge | pass |

Counts check: 3+7+6+21+8+6+4+10+3+5+6+12+1+13+4+9+5+7+3+1+4+4+1 = 153.

Tests for the whole command layer outside the per-family files: `registry_tests.rs` runs `hostile_params_never_panic` (34 hostile values against every command and schema key) and `refusal_leaves_state_unchanged` (6 modes by 153 commands); `catalog_tests.rs` compares the registry with `docs/commands.md` and Appendix A; `exception_tests` has 31 tests for 16 exception rows (`cargo xtask exceptions`, run in `ci`, passed in 78.6 s).

### 2.3 UI logic that stays, in one list

The fence test `crates/app/tests/fences` reads `crates/ui` and `crates/app` with `syn` and fails on any state change that is not a command, a gesture call or an allowlist line. The allowlist has 20 entries and a ceiling of 20 (it can only shrink). Each entry, with its reason:

| Entry | Reason |
|---|---|
| `ui/src/model.rs` `Session::query` | Reads go through the registry (`reads_only` commands). It needs `&mut self`, leaves no ring entry and changes no state. |
| `app/src/app.rs` `query` in `collab_view_of` | Same, for `collab.status`. |
| `collab_poll`, `take_collab_notices` | Engine tick that lets the collab client work and applies remote edits (exceptions §3: remote edits are not user actions); notices become toasts. |
| `flush_workspace`, `workspace_tick`, `workspace_next_due_ms` | Autosave tick, write on blur and drop, and the next wake-up time. |
| `take_dirty` | Clears the redraw flag the frame consumed. Draw bookkeeping. |
| `stage_text_edit` | E16/N3: hands the text editor's latest result to the session. No undo step. |
| `input`, `key`, `tick` (handed out) | The interaction layer turns pointer events into `gesture.*` and view updates (E1 to E11); Esc/Enter reach it during a drag; it follows the session camera. |
| `poll` (handed out) | `BridgeHost::poll` runs agent and page requests with `execute_from`, so it is the command path. |
| `load_settings`, `restore_workspace`, `use_stored_default_target` (`app/src/startup.rs`) | Start-up restore, once, before the first frame. |
| `tab_mut`, `doc`, `observed`, `saved_revision` in `App::install_scene` | N4: the perf scenarios put a generated scene of 1k to 100k elements into the tab. Behind the cargo feature `scenarios`; the test checks the `cfg`. |

Other things that stay UI-only (`docs/ui.md` §5): open/closed state of menus, popovers, dialogs, palette and toasts; dialog text-field drafts; OS pickers (host requests); canvas pointer gestures (listed in `docs/exceptions.md`); the opacity slider drag (E13); style choice during a text edit (E16); opening the context menu and the equation editor from the canvas.

High-frequency exceptions (`docs/exceptions.md`, closed list; each has a reason, a final command and an equality test that compares the gesture result with the plain command):

| Row | What | Class | Reason in one line | Final command |
|---|---|---|---|---|
| E1 | Hover and cursor | B | Changes no state; a command per move would cost a ring entry and a transaction (old app redrew a full frame per move, 53-68% of a core). | none |
| E2 | Drag-move | A | Bound arrows, labels and guides must follow live; one undo step, no CRDT delta per frame. | `element.move` |
| E3 | Resize | A | Text re-measures and arrows re-route per frame. | `element.resize` |
| E4 | Rotate | A | Angle badge and arrows follow live. | `element.rotate` |
| E5 | Line/arrow point drag | A | Bind candidate and elbow route update live. | `arrow.move_point` |
| E6 | Draw a shape | B | The element does not exist until release; a half-made one would reach collaborators, undo and bindings. | `element.create` |
| E7 | Freehand/highlighter | B | 240 Hz pen input, thousands of points; element does not exist before release. | `element.create` kind=freehand |
| E8 | Eraser | B | One undo step per stroke; cancel must not delete half. | `element.erase` |
| E9 | Marquee and lasso | B | Per-event selection changes would recompute the panel and fill the ring. | `selection.select_region` / `select_lasso` |
| E10 | Pan | C | Hottest input; one matrix update per event, no document. | `view.set` |
| E11 | Zoom | C | Same. | `view.set` |
| E12 | Laser | E | Presence, not content. | `collab.laser` (no document change) |
| E13 | Opacity slider scrub | A | Live result, one undo step. Width/size scrubs and colour-picker drags are not built. | `style.set` |
| E14 | Release of a dragged library item or file | none | Listed to say that a drag source needs no recogniser. Ghost under the pointer and tab drag are not built. | `library.insert` / `doc.drop` |
| E15 | Held key repeat | none | Each repeat is a plain `execute`; `element.nudge` coalesces a burst into one undo step. | `element.nudge` |
| E16 | Text caret, selection, IME, in-text keys | D | Composition needs preedit state and cannot be commands. | `text.edit_end` |

Writes that are not commands (`docs/exceptions.md` §5): N1 `Session::on_write_done` (an asynchronous file write ended), N2 `Session::apply_remote` (a collaborator's edit), N3 `Session::stage_text_edit`, N4 `install_scene` (perf scenarios only, behind the `scenarios` feature). `cargo xtask exceptions` found "16 rows, 31 tests, 4 non-command writes" in this run.

## 3. Gates and counts

### 3.1 `cargo xtask ci` (run, `timeout 3600`, exit 0, 845.9 s, fresh worktree target dir)

| Gate | Result | Time | What it covers |
|---|---|---|---|
| fmt | pass | 2.0 s | rustfmt |
| standards | pass | 0.0 s | `AGENTS.md` size and the paths and xtask commands it names; the toolchain matches CI; shrink-only lists (`pending-gates.txt`, `planned-paths.txt`, `unregistered.txt`) |
| exceptions | pass | 78.6 s | 16 rows, 31 tests, 4 non-command writes; every `Undo::Gesture` command has a row |
| layers | pass | 0.0 s | crate layering and the ban on egui/winit/rfd below L6 |
| clippy | pass | 109.0 s | `--workspace --all-targets -D warnings` |
| wasm | pass | 41.4 s | every crate below L6 builds for `wasm32-unknown-unknown`, with clippy |
| deny | pass | 1.5 s | advisories, bans, licences, sources |
| test | pass | 143.4 s | nextest: 4254 tests run, 4254 passed, 0 skipped; 13 e2e tests skipped by design (they run under `cargo xtask e2e`) |
| perf | pass | 214.6 s | work-counter tests and `perf/budgets.json` |
| size | pass | 127.0 s | wasm and font size budgets |
| fixtures | pass | 60.9 s | "regenerated from the old app": the old app's source was present, so the regeneration half ran |
| harness | pass | 67.4 s | 82 scenarios, 11 accounting tests, 1 scenario guard |

The compile warning `drafftink-io (lib) generated 1 warning` in the log is the `arrow_bounds` dead-code warning outside the `facts` feature (design audit item 27). Clippy passes with `-D warnings`.

### 3.2 `cargo xtask e2e` (run; Xvfb, NVIDIA L40S, headed Chromium with WebGPU, 74 browser tests)

| Suite | Passed | Failed | Skipped |
|---|---|---|---|
| Browser (Playwright, `e2e/web`) | 73 | 1 | 0 |
| Native (`e2e/native`, real `drafftink --bridge`) | 15 | 0 | 0 |

The gate exits 1 because of the one browser failure.

### 3.3 The failing test

`specs/input/pointer.spec.ts:124 typing text: the text tool, a click, the keyboard and Ctrl+Enter make one text element` expected `"hello canvas"` and received `"ello canvas"`: the first typed key is lost. I reran only this test through `DRAFFTINK_E2E_FILTER="typing text"` three times: it passed twice and failed once with the same message. So it is intermittent (2 failures in 4 runs, counting the full run), and the cause looks like the first key arriving before the on-canvas text editor takes keyboard focus. The test was not changed and its assertion stands. It belongs to the `text` and `element` families (`text.edit_begin`, `text.edit_end`, the editor in `crates/app/src/editor`). The design audit of 2026-11-08 recorded 73 of 73 browser tests passing at `17667fd`; the suite has since grown by one test and this one is flaky. It is open issue 1 in section 7.

### 3.4 Baseline against HEAD (the "same tests" requirement)

The old unit tests exercise types that no longer exist (`Shape`, `CanvasDocument`, `Camera`, `CrdtDocument`, `ToolManager`), so the same test binary cannot run on both sides. `docs/audit/regression.md` (run 2026-11-08 at `b7f8011`) therefore counts both sides and maps the 158 old tests to new tests by behaviour. This is a deviation (section 6, P2).

| | Passed | Failed | Skipped / ignored |
|---|---|---|---|
| Baseline `cargo test --workspace` at `8ce40ab` (`regression.md` §1.1) | 158 (155 in `drafftink-core`, 3 in `drafftink-render`) | 0 | 0 |
| HEAD `cargo nextest run --workspace --all-features` at `b7f8011` (`regression.md` §1.2) | 4,072 | 0 | 0 |
| HEAD at `17667fd`, design audit | 4,127 | 0 | 0 |
| HEAD at `f654255`, `cargo xtask ci` (run, this report) | **4,254** | 0 | 0 (13 e2e excluded from this gate) |
| HEAD harness (run) | 94 (82 scenarios, 11 accounting, 1 guard) | 0 | 0 |
| HEAD browser e2e (run) | 73 | 1 (flaky) | 0 |
| HEAD native e2e (run) | 15 | 0 | 0 |
| HEAD fixtures: 20 legacy fixtures, facts from the old code (`regression.md` §1.3) | 53,420 checks, 0 failures | 0 | 0 |

Mapping of the 158 old tests to the rewrite (`regression.md` §2, updated by `deviations.md` after the audit-e branch): 126 covered and passing, 29 covered differently by deliberate change, 3 not covered (Mermaid cluster boxes), and those 3 were then covered when subgraph boxes were restored (`diagram_tests.rs`, `diagram_create.rs`). The mapping was checked for existence of the cited tests and a green run. No mutation check shows that each mapped test fails when its behaviour breaks.

Other evidence against the old app that passes at HEAD: `geom/tests/legacy_snap.rs` (proptest against the old `snap.rs`), `sketch/tests/old_app.rs`, `edit/tests/brush_old_app.rs`, `typeset/tests/legacy.rs`, `render/tests/gpu.rs` (pixels against the old renderer's PNGs), `io/tests/{legacy,excalidraw,mermaid}.rs`.

### 3.5 Gates that exist but were not run for this report

`cargo xtask perf --bench` (19 criterion ids), `perf --app native` (16 scenarios), `perf --app web`, `size --native`. They need a quiet machine or a long build; this host had a load average of 2 to 21 during the run. The design audit ran the first three on 2026-11-08 and all were within `perf/budgets.json` (native app 16 of 16). `regression.md` records `perf --app web` at 15 of 16 on the first run, the 16th (`wheel@10k`, p99 135.9 ms against 100) passing in 3 of 3 reruns. These are not part of `cargo xtask ci` (deviation D7).

## 4. Performance, before and after

Before is the old app at `8ce40ab` (`docs/performance.md` §7, from `research/baseline-perf.md`). After is the rewrite; each figure carries the section it comes from. All on the research box (NVIDIA L40S, 64 threads, Xvfb) except where noted. Not rerun for this report; the dates of the sources are 2026-11-07 and -08.

| Metric | Before (old app) | After (rewrite) | Source |
|---|---|---|---|
| Native idle CPU | 0.3 to 0.6% of a core | 0.5%, 0 frames | §7.1, §7.8 |
| Native hover, 125 moves/s, nothing changing | 53 to 68% of a core (a frame per move) | 6.6% in `hover@10k`; frames only when the hover result changes (78 frames for 1000 moves) | §7.1, §7.8 |
| Native wheel pan | 52 to 65% of a core | 33% (`wheel@10k`, one frame per tick at most) | §7.1, §7.8 |
| Drag 100 elements in a 100k scene, frame p50 / p99 | not measured (first rewrite build before perf pass: 26.7 / 35.4 ms) | 8.9 / 9.7 ms, 59% CPU; design audit rerun p99 9.95 ms | §7.8, audit §4 |
| Pan frame p99 at 10k and 100k | n/m (CPU 52 to 65% of a core for wheel pan) | 8.71 ms and 8.41 ms (budget 25 ms) | audit §4 |
| Web `zoom@100k` frame p50 | 66 ms before the second pass | 1.6 ms | §7.8 |
| Hit test, 10,000 / 100,000 elements | 0.70 ms / 9.1 ms (linear scan) | 271 ns at 10k (R-tree, `canvas/hit/10000`) | §7.2, audit §4 |
| Undo push (document clone), 10,000 / 100,000 | 1.2 ms / 17.0 ms | see the CRDT row: a commit writes only the touched elements | §7.2 |
| Move frame, 1k / 10k / 100k | 105 us / 1.15 ms / 37 ms (rewrite before its first perf pass) | 15 / 16 / 14 us | §7.7 |
| `build_scene`, warm, mostly culled, 10k | 1.0 ms | `compose/pan_warm/100k`: 7.05 us (a different benchmark of the same path) | §7.2, audit §4 |
| CRDT: `sync_to_crdt` on every local edit in a room, 1,000 / 10,000 elements | 64 ms / 427 ms | local commit of one moved element at 10k: 12.67 us (`--bench`, audit §4); different operation, same purpose | §7.2, audit §4 |
| CRDT: `sync_from_crdt` on every received message, 1,000 elements | 2.05 s (10,000: over 80 s) | incremental remote apply (no figure in the performance doc); a room snapshot is decoded in 8 ms slices | §7.2, backlog |
| Wasm module, brotli | 3,462,376 B (3.46 MB) | 3,267,207 B (-5.6%) | §7.3, §8.1 |
| First visit, brotli (module, JS glue, two fonts) | 3,478,229 B | 3,816,214 B (+9.7%, deviation D17) | `deviations.md` D17 |
| Wasm after wasm-opt, MiB | 17.62 (bundled fonts) | 11.79; fonts as 1.44 MiB brotli on demand | §8.1 |
| Web start, cold / warm service worker, ms | 676 / 439 | 638 / 394 (equal within 10-20% noise) | §8.1 |
| Native binary | 24.4 MB | 46.9 MB `release` (stripped); 42.2 MB `dist`. 55.2 MB before `strip`. About 1.7 to 1.9 times the old binary. | §8.3, D10 |
| Native first frame | not measured | 310 to 340 ms, 265 to 290 ms of it the Vulkan device | §7.8, audit |
| Load of a 100k-element document into a tab | n/m | 2.6 s release, spent in Loro (field inserts 1.7 s) | backlog, §7.9 |

Where the rewrite is not better: the binary is larger, the first-visit download is 9.7% above the old one, a 100k load takes seconds, and the pan budget on the native app is 25 ms p99 instead of the planned 16.7 ms (D12).

## 5. Features: kept, added, missing

Counts from `docs/parity.md` (audited at branch `audit-e` `25fa072`; `main` has moved since, so these may be a few rows off). Every `path::name` evidence citation was checked to exist by script (505 citations); the Playwright specs behind browser rows were not run for that audit, but 73 of 74 passed in this report's run.

| Part | done | partial | missing | out of scope | rows |
|---|---|---|---|---|---|
| A: old drafft-ink features | 178 | 14 | 4 | 0 | 196 |
| B: Excalidraw features | 152 | 32 | 21 | 7 | 212 |
| Total | 330 | 46 | 25 | 7 | 408 |

Rows of B that restate an A row count twice.

### 5.1 Kept from the old app

178 of 196 rows are done: shapes, hand-drawn style (sloppiness, fill patterns), freehand with pressure and calligraphy, text with three font families and per-character colour, inline LaTeX, images, snapping and smart guides, tabs, library tab, Mermaid import, collaboration with cursors and laser, undo/redo, PNG/JSON export and import, touch gestures.

Kept but changed on purpose: 25 behaviour changes in `docs/commands.md` §7.5 (undo from every surface, `Cmd` on macOS, one paste path, group rotation, Esc ladder, max zoom 30, camera as centre plus zoom, `.drafftink` extension, background colour saved with the document, others).

### 5.2 Added (Excalidraw and agent features)

Bound arrows (move, resize, rotate, flip, delete, undo, focus, gap, highlight), elbow arrows with segment handles, six cardinality heads and other arrowheads, nested groups with enter and exit, frames, lock, z-order, align and distribute, flip, lasso, convert, sticky notes, links, element names, flowchart helpers (`selection.navigate`, `diagram.extend`), library, styles copy/paste, command palette generated from the registry, view-only mode, `diagram.create`, `layout.apply`, `doc.outline`, `element.query`, the CRDT collaboration with a new binary protocol, MCP in five modes, the CLI, three server stores, own profile crate with span and counter budgets.

### 5.3 Missing or partial, with the reason

| Feature | Status | Reason (from `deviations.md` §3 and `parity.md` §4) |
|---|---|---|
| Embeddables (iframes) | missing | Needs a sandboxed web view per element; the native app has none. |
| Image crop (command, handles) | missing | The `crop` field exists and the painter honours it; no command or UI sets it. Planned. |
| Bucket fill, eyedropper, pen mode, autoshape, hover "+" connectors, list markers in text, chart paste | missing | Smaller than the work they would take next to bound arrows and frames; not on the plan's priority list. The eyedropper needs a pixel read-back that browsers gate behind a permission. |
| Paste SVG and URL | missing | SVG needs a rasteriser in `io`. |
| Mermaid types beyond flowchart and sequence | missing | A parser and a layout per type. They are refused with the list of supported types. Node shapes other than rectangle, round, stadium, circle and diamond are drawn as rectangles with a warning. |
| Old guides: angle-snap rays, snap crosshair, eraser radius ring | missing | Overlay parts not ported (A-086, A-088, A-046; the crosshair producer in `engine/interact/frame_out.rs` passes an empty list). |
| Web page loading percentage (A-222) | missing | Page fetches wasm itself in the old app; not ported. |
| Last-picked colour swatch, laser head glow, rotation guides | partial or missing | Not ported (A-042, A-047, A-087). |
| Localisation, scrollbars, hint bar, room encryption | missing | Not planned or not started. |
| Native pen pressure on Linux and macOS | partial | Needs a platform tablet backend (XInput2, tablet-v2, NSEvent). Web pen pressure works through pointer events. |
| Touch layout on a real tablet or phone | partial | Built and tested headless only. |
| AI features (text-to-diagram, diagram-to-code, magic frame) | out of scope | Excluded by the user. Agents use MCP. |
| Excalidraw+, sign-in, cloud, comments | out of scope | A hosted service. |
| Windows and macOS runs | not verified | No machine available; CI only builds them (D15). |
| CPU fallback for browsers without WebGPU | missing | D13. |

## 6. Deviations

All of `docs/deviations.md` is reproduced in short, then the deviations from the design specification as found by the audit, then the ones this report found.

### 6.1 From the plan (`deviations.md` §1)

| # | Plan said | Done instead | Why |
|---|---|---|---|
| D1 | Native shell on eframe | Own shell on winit + egui-winit + egui-wgpu for native and web | eframe's web runner used 3.8 to 4.2% of a core while idle (requestAnimationFrame loop); the own shell uses 0.7 to 1.2%. One shell for both targets. Cost: IME, clipboard, file drop and touch on the web are supplied by the shell. |
| D2 | `collab` at L5, `SceneStore` in `canvas` | `collab` at L4, `SceneStore` in `scene` (L1) | Removes both same-layer exceptions. |
| D3 | A panic in a command rolls back | Native only. On wasm (`panic=abort`) a hook reports and offers reload from autosave | `catch_unwind` needs unwinding. |
| D4 | Group names as a first-class field | Stored per member in `custom["drafftink.group.<id>"]` | No group table in the scene model; merges under CRDT without a new container. |
| D5 | One document store per tab | A working in-memory store plus the Loro store, written once per command, batch or gesture end | Refused commands and live gesture frames must not reach Loro; no open transaction across a command. Cost: a second in-memory copy. |
| D6 | Headless scenarios use egui kittest | JSON scenarios run by `crates/testkit` against a `Session`; kittest only for `crates/ui` tests | A scenario is a list of commands; a UI-driven runner would test widgets, not the command layer. |
| D7 | CI runs browser e2e and perf budgets; one aggregate `ci` job | `cargo xtask ci` has 12 gates. Not in it: browser and native e2e, timing tiers, `size --native`, regeneration half of fixtures (it ran here because the old app was present) | They need a display, a GPU and a quiet machine, or the old source tree; flaky gates get turned off. `e2e.yml` runs nightly and on app-touching PRs, outside the required check. |
| D8 | npm package has canvas and editor layers | Canvas library only | The editor needs the registry and Loro (the package is 7.4 MB wasm, the app 12.5 MB). The editor is `window.drafftink` of the app. |
| D9 | The editor sits on the canvas API facade | The editor uses the canvas primitives, not `Canvas<S>` and the dispatcher | Every pointer event must end in a command or gesture so the layering test sees state changes. Two integration paths exist. |
| D10 | Native `opt-level=3`, thin LTO | `release` plus `strip`; shipped binaries use profile `dist` (fat LTO, size-optimised cold crates) | 55 MB was 2.2 times the old 24.4 MB; `dist` gives 41 to 42 MB at twice the build time. |
| D11 | Criterion benches at 1k, 10k, 100k for hit test, snapping, solver, encoding, CRDT, import/export | 19 curated ids, mostly 10k and 100k; no bench for snapping or import/export | Work-counter tests cover scaling at all three sizes exactly; benches add timing of the hot ones. |
| D12 | Pan frames 16.7 ms p99 | Native app budget 25 ms p99 (`efe3681`) | Measured 8 to 13 ms (pan) and 5 to 20 ms (drag) under Xvfb, whose X11 copy is most of the CPU. |
| D13 | CPU fallback for web without WebGPU | None; the page says so | `vello_cpu` needs about 350 ms per frame in wasm. |
| D14 | Server serves the web client in one binary | Server serves a folder (`--web-dir`) | Embedding 12.5 MB plus fonts in every server build was not worth it. |
| D15 | Native builds for three OSes in CI | Built in CI; tests on Ubuntu only; Windows and macOS packaging jobs never ran | No Mac or Windows machine. |
| D16 | CLI runs scripts, converts, exports | It does, since 2026-11-08 (before, `main()` was empty). No PNG | PNG needs a GPU device and would pull wgpu into the tool. |
| D17 | First load within the old app's transfer (3.5 MB brotli) | Module 5.6% smaller; first visit 3,816,214 B, 9.7% above the old 3,478,229 B | Larger editor; fonts load on demand. Budgets limit module and fonts, not their sum. |

### 6.2 From the design specification (`deviations.md` §2, found by `docs/audit/design-compliance.md`)

The audit judged 38 clauses at `17667fd`: 20 met, 17 partly met, 1 not met (the per-family report, which this file now provides). `deviations.md` lists what is still open:

| Spec clause | The specification says | The code does | Audit issue |
|---|---|---|---|
| D2 | Nothing else changes app state for a user action | `install_scene` replaces a tab's document without history for the perf drivers, behind the `scenarios` feature (the native binary enables it). `Session` has public fields; the fence is a text/syntax test, not the type system | 5 |
| D3 | No UI dependency, enforced by lint and a layering test | The layering test is a deny list of external crates (an allow list per layer where `xtask layers` has it), with a source scan on `engine` only; `clippy.toml` has no `disallowed-types` for UI types | 9 |
| D5 | The UI shows the reason; an agent gets the same text | A few UI texts are the UI's own; some rows are hidden rather than disabled with a reason; MCP adds a `Try:` line | 8 |
| D7 | No second copy of a rule | Zoom limits, snap composition, export limits, Mermaid keyword lists and image sniffing exist twice in places | 7 |
| D8 | Facts are fields, not name lists | About 60 registrations inherit a family base; a few name lists remain (`DOC_EXEMPT_UNDO` and others) | 6 |
| D9 | `window.<app>` with `execute`, `batch`, `commands()`, `recent()` | Present, with more. Calls wait while the tab is hidden (the bridge queue drains inside a frame); schemas are returned on request | 3 |
| D10 | Each exception written down | Done. Check `exceptions.md` rows that describe features not built (E13 width/size scrubs, E14 ghost and tab drag; both say "Not built") | 4 |
| T5 | Fixture facts from a tool with `--check` | The tool exists. When the old app source is absent the regeneration half is skipped (it ran in this report's `ci`). Some scenario files hold hand-typed fixture counts | 12 |
| T6 | Never weaken an assertion | None found. GPU tests pass silently without an adapter unless `DRAFFTINK_REQUIRE_GPU` is set | 13 |
| P2 | Same tests on baseline and HEAD | Old unit tests cannot run on HEAD; behaviour map instead (section 3.4) | 14 |
| P3 | A report per family | This file | 14 |
| Process | Agents can easily create and edit diagrams | `diagram.create` and `diagram.import_mermaid` do; other Mermaid types refuse with a message | none |

### 6.3 Other deviations from the design specification and the requirements

| Item | Deviation | Reason or state |
|---|---|---|
| "Nothing else changes app state" | Four non-command writes (N1 to N4), the allowlist of 20 entries | Written down in `exceptions.md` §5 and the allowlist, each with a reason and a test. |
| "Test-only harness limited to what commands cannot do" | Seven methods (4 Rust, 3 browser), each with a stated reason and an accounting test; one undeclared stub (`window.__modelContextStub` in `webmcp.spec.ts`) escapes the accounting (audit T3) | WebMCP needs a stand-in `modelContext` in Chromium. |
| "Typecheck the specs" | Done in `cargo xtask e2e`, not in `ci` | e2e needs a display. |
| Browser and native e2e in the final audit | Not in the required gate | D7. |
| Wire protocol | New binary frames; an old client cannot join a new server | The old protocol had no version and sent whole snapshots (`deviations.md` §4). |
| Server bind | `127.0.0.1` by default, was `0.0.0.0` | Specification: loopback by default; `--bind 0.0.0.0:3030` opts in and logs a warning. |
| Document extension | `.drafftink`; `.json` still opens and saves as native JSON | `deviations.md` §4. |
| Maximum zoom | 30, was 10 | Listed in §7.5, item 22. |
| Mermaid subgraphs | Regressed once (grouped members, dropped box), restored as titled rectangles, not frames | `commands.md` 2.19. |
| Remote merges (CRDT semantics) | Remote edits get no relation repair (section 7 item 14) | Found by the design audit; open as far as I could check. |
| Excalidraw features not built | Section 5.3 | `deviations.md` §3. |

## 7. Known open issues

From `docs/backlog.md` (open items at the head of the file), `docs/audit/design-compliance.md` §3 (ranked list, at `17667fd`, some since fixed by later branches that I did not re-audit), `docs/parity.md` §4, and this report's run.

Found by this report's run:

1. **Flaky browser e2e** `pointer.spec.ts` "typing text": the first typed key is lost in about 1 run in 2 to 4 (section 3.3). The test is right; the app takes the first key before the text editor has focus. Not fixed here.

From the backlog (open items):

2. Schemas in the wasm (`schemars`, 89 KB brotli, 2.8%): not moved out; every schema reader is synchronous. First step: a cargo feature that omits schemas from the web build.
3. `Relations::build` still serves `arrange.z_order`, `clipboard.copy`/`copy_as`, frame capture after a resize, `element.restore` and `group_name` (O(scene)). Render order is rebuilt whole for a new, removed or reordered element (a move or style edit is patched).
4. Autosave: one huge element and the image-file hash check are single steps. The native writer thread and the web storage path have unit tests and a type check only, not a run of the real app.
5. Collab: the web dialog has no token field; a tab that already has content imports a room snapshot in one piece; the first import call costs about 0.8 us per element.
6. A 100k `TabDoc` load takes 2.6 s in release, spent in Loro.
7. The backlog line "Web e2e: `document.spec` and `session.spec` still expect ... refused on the web ... `startup.spec` lacks `runScenario`" is stale: `startup.spec.ts:20` lists `runScenario` and `scenarioReport`, and both specs passed in this run. It should be deleted from the backlog.
8. Under Xvfb, `present` is 70 to 85% of the native process CPU. Not our code; not measured on real hardware. Mailbox present mode could not be measured (the NVIDIA driver offers Fifo and Immediate).
9. Text editor: arrows bound to a container that grows during an edit jump when the edit ends; no I-beam cursor over text; RTL weakly tested; no per-run bold or italic; no real OS input method tried; `Esc` commits and cancel is only `text.edit_end commit=false`.
10. Tab bar dirty state compares content through undo and redo only.
11. Web PNG export inside a batch: the renderer is asynchronous; a batch with a pending render step is taken back and rerun by the bridge host, and the pending step is refused with `render_pending` inside a batch.
12. Release: `drafftink-cli` is not in the release packages. Mermaid shapes beyond the basic set render as rectangles. No CPU fallback for browsers without WebGPU.
13. Overlay leftovers: a `Cursor::Pointer` variant for the link icon, distance badges for alignment snaps, IME composition in `TextEdit`.

From the design audit and not listed as closed in `deviations.md` (verify before relying on them):

14. **High:** remote merges run no relation repair (`engine/src/remote.rs`); two peers moving different targets of one arrow can leave it detached, and a delete racing `arrow.bind` can leave a binding to a deleted element. The property-test generator never emits group, binding, frame, point or undo operations. `docs/protocol.md:210` claims a gesture abort that the code does not do.
15. **High:** a batch that changes tab (`tab.new`, `tab.switch` are `Batch::Allowed`) is not atomic: rollback covers only the start tab.
16. **High:** calls through `window.drafftink` and WebMCP wait while the tab is hidden (the queue drains inside a frame). The browser e2e now has `shell/hidden.spec.ts` (it simulates a hidden page) and it passed in this run; I did not check whether it covers a call made while hidden, so treat the item as unverified.
17. `doc.export` to a path outside the allowed folder returns success with only a warning (`engine/src/ports.rs:475`).
18. The design audit said the packaged server was built without `sqlite` and `s3`. That is closed: `scripts/package-linux.sh:48` now passes `--features drafftink-server/sqlite,drafftink-server/s3` and `release.yml` smoke-tests `--storage sqlite`. Still open from that item: the real-S3 test returns early and passes when its environment variable is unset.
19. Windows and macOS: CI builds only; no run on either system.
20. AGPL gaps: no in-app source offer for modified hosted copies; two font redistribution permissions (GelPen, Vanilla Extract) unconfirmed (`ATTRIBUTION.md`).
21. Parity P0 gaps from `parity.md` §4: angle-snap rays, snap crosshair, eraser radius ring, loading percentage, last-picked swatch, phone and tablet layout run on a real device, app overlay for lasso path, link icons and remote selection outlines (the last was done by branch `render-overlays`, per the backlog; the parity file predates it).
22. `docs/parity.md` was audited at `25fa072`, and its counts are not regenerated at HEAD.
