# Drafft.ink rewrite: plan and target crate layout

Status: draft for review. Nothing in `/home/ubuntu/git/drafft-ink` has been changed yet.

Research behind this plan (all under `research/`):
- `solidcraft.md`: the command system, layering gates, bridge, tests and builds we copy.
- `drafftink.md`: the full feature, action and shortcut inventory of today's app, with file:line references.
- `excalidraw.md` and `parts/`: the element model, arrow binding, groups, actions, the feature gap list, the embedding API, and Konva.
- `baseline-perf.md` and `web-perf.md`: the baseline tests, build sizes, CPU measurements and benchmarks.

## 1. Goal

1. **The canvas library.** A reusable canvas library, in Rust with a JS/wasm API, in the spirit of Konva but faster. It has a spatial index, culling, exact hit testing, connectors, transactions with undo, and CRDT sync (Loro). The library can also run on a plain in-memory store; see §12.
2. **The editor.** An Excalidraw-class editor built on the library. Every user action is a named command in one registry, and the UI is a thin layer over it.
3. **The app.** The drafft.ink app on top of the editor, for the web (wasm) and native Linux, Windows and macOS. Idle CPU is about zero, and the app redraws only when something changed.
4. **Features.** Every current drafft-ink feature is kept. Excalidraw's missing features are added, with arrow binding and proper groups first.
5. **Evidence.** Every claim is backed by tests and measurements. Unit tests cover each command family. End-to-end tests drive the app through commands on native and web. Perf gates run in CI. A final regression audit compares the baseline with HEAD.

## 2. Baseline facts that shape the plan

- **Tests.** The baseline is HEAD `8ce40ab`, where `cargo test --workspace` gives 158 passing tests (155 in core, 3 in render). The app, server and widgets crates have 0 tests. `clippy -D warnings` fails with 17 errors.
- **No command layer.** Actions live in three places:
  - an inline keyboard handler (`app.rs:4540-5369`);
  - a ~73-variant `UiAction` match of about 1,000 lines;
  - the pointer handlers.

  Some actions disagree between paths. Undo broadcasts to collaborators only from the UI path. Paste behaves differently from the keyboard and from the UI. PNG export has about 5 copies.
- **Groups.** A group owns its children by value, so the children have no ids. Group rotation is cosmetic, and grouping reorders z-order. There is no arrow binding.
- **Sync.** Every local edit clears and re-adds every shape, then sends a full snapshot. That costs 64 ms at 1k shapes. Every received update rebuilds the whole document, which takes 2 s at 1k shapes and grows roughly quadratically.
- **CPU.** Hovering the mouse re-renders a full frame on every move, which costs 53–68% of a core. The scene is re-encoded every frame. There is no spatial index: a hit test takes 9 ms at 100k shapes. Math is re-laid-out on every frame.
- **Web build.** The wasm is 10.3 MB (3.46 MB brotli). It needs WebGPU and has no fallback. The release profile uses `opt-level="z"` on native too, which runs 1.3–2x slower than `opt-level=3`.

## 3. Strategy

I will write a fresh workspace and port the current code into it, module by module. Most of the hard parts change their data model (flat elements, bindings, incremental CRDT, command layer), so refactoring in place would cost more than it saves.

- **Where.** The work happens in a new repo (decision §10.1). The old repo at `/home/ubuntu/git/drafft-ink` stays untouched at `8ce40ab`. It is the porting reference and the source for the baseline test run.
- **File compatibility.** Old drafft-ink JSON documents must keep loading. A migration in `io` handles this, tested against fixtures taken from the old app.

## 4. Target crate layout

The layers follow solidcraft's model. `cargo xtask layers` reads `cargo metadata`, gives each crate a layer, and fails on any edge that points up or sideways. It also fails if `egui`, `eframe`, `winit` or `rfd` appear below L6, and if `wgpu`/`vello` appear below L2.

```
L0  geom        kurbo-based math, transforms, bounds, R-tree spatial index
    profile     span profiler, frame-time histograms, counters (no deps)
    protocol    versioned collab wire types, shared by client and server
L1  scene       element model: flat elements, ids, fractional z-index, groupIds,
                frameId, bindings, styles, serde. Pure data.
    typeset     text (Parley) and math (ReX) layout to outlines, with caches
    sketch      roughr hand-drawn path generation, cached by (seed, geometry)
L2  render      Vello scene encoding: per-element encoded fragments, culling,
                level of detail, grid. Knows nothing about documents or commands.
L3  canvas      THE REUSABLE LIBRARY: SceneStore trait, viewport, hit testing on
                the spatial index, picking, transformer handles, pointer
                gesture recognizers, event dispatch. In-memory store included.
    doc         Loro-backed SceneStore: transactions, undo (Loro UndoManager,
                one commit per command), incremental sync deltas, schema
                version, derived indexes (bound arrows, groups, frames)
L4  edit        domain rules with typed errors: binding solver, elbow routing
                (A*), groups (enter/exit), frames, align/distribute, z-order,
                snapping and smart guides, flowchart helpers, bound text
    io          native JSON (versioned, old-format migration), .excalidraw and
                .excalidrawlib import/export, Mermaid import, SVG export
L5  engine      command registry, Session, execute / execute_batch, refusals,
                recent ring, introspection commands, editor state (selection,
                tool, editing group, view, tabs, settings), gesture protocol,
                ports (clipboard, storage, files, clock) as traits
    collab      sync client over a Transport trait (native: tungstenite;
                web: WebSocket). Presence. Remote edits enter through engine.
L6  gpu         wgpu device and surface, offscreen render (PNG export, pixel
                probes), compositing Vello + egui with no per-frame extra pass
    ui          egui UI (today's widgets merged in). Reads the registry, calls
                execute, shows refusals. No document logic.
    bridge      agent bridge: loopback-only JSON-RPC over WebSocket, per-launch
                token, Origin and Host checks; MCP stdio server (headless or
                attached to a running app)
L7  testkit     headless harness, scenario runner, deterministic scene
                generator (1k/10k/100k), pixel and scene probes
apps/
    drafftink         native desktop shell (eframe or winit), ports wired to OS
    drafftink-web     wasm entry, window.drafftink API, PWA/service worker
    drafftink-server  axum: rooms, incremental sync, limits, RoomStore
                      providers (memory, fs, sqlite, s3), serves the web
                      client (makes "one binary" true)
    drafftink-cli     headless: run command scripts, convert, export
    drafftink-mcp     MCP server: stdio headless, attached to the native app,
                      and --relay for the web page (see §11)
packages/
    drafftink-canvas  npm package: TypeScript types + wasm for the library
                      (canvas layer) and the editor (engine layer)
xtask/              layers, wasm, size, perf, fixtures --check, harness
                    accounting, standards, ci
```

How the three products map onto the crates:

| Product | Crates |
|---|---|
| Canvas library ("Konva on steroids") | `geom`, `scene`, `typeset`, `sketch`, `render`, `canvas`, and optionally `gpu` |
| Excalidraw-style editor | the library plus `doc`, `edit`, `io` and `engine` |
| drafft.ink app | the editor plus `collab`, `ui`, `bridge` and the apps |

Each tier is usable on its own, from Rust or from JS.

## 5. Command layer (the core design)

- **Command entries.** Each command is a static `CommandSpec` with these fields:
  - `id` (`family.verb`), `title`, `family`
  - `shortcut`s (one key registry for every platform, with Cmd and Ctrl handled properly)
  - a typed `Args` struct with a JSON Schema (via `schemars`), so handlers take typed params rather than raw JSON, unlike solidcraft
  - `available(&Session) -> Result<(), Refusal>`
  - `execute(&mut Session, Args) -> Result<Output, Refusal>`
  - `undo`: one of `Undoable`, `NoUndo` or `Gesture`
  - `reads_only`
  - other facts as fields, for example `needs_selection` and `mutates_doc`. A test fails if a registration leaves a fact out.
- **One entry point.** `Session::execute(id, json)` and `execute_batch([...])` are the only way to change state for a user action.
  - A batch preflights every step before it runs any, rolls back on the first error, and becomes one undo step.
  - A panic inside a command rolls back too.
- **Refusals.** A refusal is a value with plain-words text. The UI shows it as a toast or as a disabled palette item with the reason inline, and an agent gets the same text.
- **No dialogs.** Commands never open a dialog. A decision is an argument with a safe default, and file pickers live in the UI, which passes a path or bytes to the command.
- **Gestures.** A drag, pan or resize is the documented exception. It streams `live` updates and ends with a `*_end` command that commits once. The final state is always reachable as a plain command (`view.set`, `element.move`, `element.resize`). Every exception is listed in `docs/exceptions.md` with its reason.
- **Introspection.** These calls are available on `window.drafftink` on the web and through the bridge on native:
  - `commands()` lists ids, schemas and current availability.
  - `recent()` reads a bounded ring of commands with their outcomes.
  - Read-only commands: `doc.inspect`, `selection.get`, `view.get`, `element.query`, `element.at`, `history.state`, `perf.stats`.
- **Remote edits.** Collaborator edits and agent edits use the same document and CRDT path as local edits.

**Command families.** Each family owns its own source file and its own test file.

| Family | Covers |
|---|---|
| `doc` | new, open, save, import, export |
| `tab` | canvas tabs |
| `tool` | tool switching |
| `element` | create, delete, duplicate, move, resize, rotate, flip, nudge, lock |
| `selection` | selection |
| `clipboard` | copy, cut, paste, paste-style |
| `style` | styles |
| `text` | text and math |
| `arrange` | z-order, align, distribute |
| `group` | group, ungroup, enter, exit |
| `frame` | frames |
| `arrow` | binding, arrowheads, elbow |
| `image` | images |
| `view` | view controls |
| `history` | undo and redo |
| `collab` | collaboration |
| `settings` | settings, persisted |
| `library` | the shape library |
| introspection | the read-only commands above |

## 6. Features

**Keep every current feature.** The full list is in `research/drafftink.md` §1. It includes:
- the 12 tools and 9 shape kinds
- pressure freehand, now reading real pen pressure
- the sketch style
- smart guides, equal spacing and angle snap
- the three font families and per-character styling
- LaTeX math
- images by drag-drop and paste
- PNG/JSON export and import
- Mermaid import and Excalidraw library import
- canvas tabs
- touch gestures
- the laser pointer
- collaboration over wss with auto-join
- the elbow route

**Fix while porting.** These are in the inventory's §8 checklist:
- the undo and paste divergence
- Cmd on macOS
- the dropped simultaneous UI actions
- autosave dirty detection
- settings persistence
- the elbow route being render-only
- group rotation

**Add from Excalidraw, in priority order:**
1. Arrow binding with inside and orbit modes, kept attached when shapes move, resize, rotate or are deleted, with a highlight during drag.
2. Flat groups using a `groupIds` stack: click selects the outermost group, double-click enters it, Esc leaves it, and ids are stable.
3. Text bound to containers and arrow labels.
4. Diamond.
5. Text align and wrap.
6. Arrowhead choices and real elbow arrows with A* routing.
7. Stroke styles.
8. Lock.
9. Distribute.
10. Edge resize handles.
11. Context menu.
12. The command palette.
13. Frames.
14. Eraser.
15. Tool lock.
16. Draw modifiers.
17. SVG and `.excalidraw` export.
18. Search.
19. A stats panel.

Not planned: the AI features (text-to-diagram, diagram-to-code, magic frame).

## 7. Performance design and tooling

- **Redraw.** The app redraws only when something is dirty. Pointer hover redraws only if the hover state changed. Every `request_repaint_after` is honoured. Network, autosave and caret blink wake the loop explicitly.
- **Scene encoding.** Each element keeps its own encoded fragment, cached by a version number. The scene is assembled from the cache, and culling uses the R-tree.
- **Caches.** Text, math and image caches are bounded. Math is laid out once per edit, not once per frame.
- **Sync.** Sync sends incremental Loro updates and applies them as patches. Nothing does a full rebuild.
- **Build profiles.** Native builds use `opt-level=3` with thin LTO. The web build uses its own size-tuned profile with `wasm-opt`, a size budget checked by `xtask size`, and code splitting where wasm allows.
- **Tooling.** The `profile` crate provides:
  - per-phase spans: input, commands, layout, encode, GPU submit
  - frame-time histograms and counters
  - Chrome-trace export
  - output through `perf.stats` and `window.drafftink.perf`
- **Gates.**
  - Criterion benches cover hit test, snapping, the binding solver, encoding, CRDT apply and import/export at 1k, 10k and 100k elements.
  - An idle check requires at most 1 redraw in 10 s and under 1% CPU.
  - Scripted e2e perf runs (pan, zoom, drag, hover) on native and web are compared against a checked-in `perf/budgets.json`.
  - The baseline numbers from `research/baseline-perf.md` serve as the "before" column.

## 8. Testing

- **Unit tests.** Each command family has its own test file, run against `Session` with no UI and no GPU. The tests check availability, refusal text, the effect, and at least one negative case. A registry-wide test sends hostile params to every command and checks that none of them panic.
- **Headless scenarios.** JSON scenario files run in the testkit harness, which uses egui kittest and no GPU. One `#[test]` is generated per file.
- **Browser e2e.** Playwright drives `window.drafftink` in headed Chromium on Xvfb with the NVIDIA GPU. agent-5 verified that WebGPU renders this way and that headless mode renders blank. The specs are typechecked with TypeScript.
- **Native e2e.** Tests drive the native app through the bridge.
- **Harness discipline.** The harness methods are only what commands cannot do, such as fixture loading and pixel or scene probes, and each method states its reason. A test fails if a method is declared but never called, or called but not declared.
- **Fixtures.** Fixture facts are generated by `xtask fixtures`, which has a `--check` mode.
- **Pixel probes.** Offscreen Vello renders on real hardware, with tolerance-based checks rather than exact golden images.
- **CI.** fmt, clippy `-D warnings`, `xtask layers`, `xtask wasm`, unit and scenario tests via nextest, the wasm build with a size gate, browser e2e, the perf budgets, and cargo-deny. A single aggregate `ci` job is the required check. Native builds run for Linux, Windows and macOS.

## 9. Phases

Each phase ends with its gates green before the next one starts.

0. **Setup (me plus 1 agent).** Create the new repo, the workspace skeleton and `AGENTS.md`, adapted from solidcraft as a router of "rule: enforcer" lines. Add `xtask layers`, `xtask wasm`, the CI skeleton and the baseline test log.
1. **Foundations (parallel, one crate per agent):**
   - `geom` with the R-tree
   - `scene` with the flat model and migration of the old format
   - `typeset`
   - `sketch`
   - `profile`
   - `protocol`

   The benches start here.
2. **Library:** `render`, then `canvas` (with the JS API skeleton), then `doc`, built on Loro with transactions, undo and incremental sync.
3. **Engine core:** registry, executor, batch, refusals, recent ring, introspection, `window.drafftink`, the hostile-params test, and the layering test.
4. **Domain and families (parallel, one family per agent, no shared files).** Port the `edit` rules and `io` formats with their families:
   - `element`, `selection`, `clipboard`, `style`, `text`, `arrange`, `group`, `view`, `history`, `doc`, `tab`, `tool`, `settings`
   - then the new features: `arrow` binding, `frame`, `library`
5. **Shells:** `gpu`, `ui` (thin, palette, context menu), the native app, the web app with its PWA, then `bridge`/MCP and `collab`, plus the new server with persistence.
6. **E2E and perf:** the scenario suite, browser and native e2e, perf gates, and tuning against the budgets.
7. **Audit:**
   - The baseline suite runs against legacy, and the new suite against HEAD, with pass, fail and skip counts for both.
   - The feature parity checklist is ticked item by item against the inventory.
   - An independent reviewer agent checks the work.
   - A report per family covers commands added, UI logic removed or left with its reason, and gates run. It lists every deviation.

**Agents.** Sonnet agents do the implementation, one per crate or family. A fresh-context reviewer agent checks each phase before I merge it. Shared files, such as the workspace `Cargo.toml` and the registry list, are edited by me only.

## 10. Decisions (answered 2026-11-07)

1. **Location: a new repo** at `/home/ubuntu/git/drafftink` (name to confirm at setup). The old repo stays untouched as the baseline and the porting reference. Phase 0 changes accordingly: no `rewrite` branch or `legacy/` directory. The baseline suite runs in the old repo.
2. **Licence: everything stays AGPLv3**, the library tier and the npm package included.
3. **Native shell: eframe**, with Vello rendering into a texture that egui composites.
4. **Web renderer: WebGPU first.** WebGPU is the primary target and gets all the perf work. A CPU fallback (`vello_cpu` or `vello_hybrid`, drawing to a 2D canvas) is added once it costs nothing on the WebGPU path, behind a renderer trait in `gpu`. If the fallback can't reach usable speed, it ships as read-only viewing and the gap is reported.
5. **Font: keep `VanillaExtract.ttf`.** The font's metadata says only "Personal and Commercial Use Allowed" (Shara Weber, 2016). It grants use, but it doesn't explicitly allow redistribution or modification, and we redistribute the font inside an AGPL repo and binaries. The font is a separate work, so the AGPL doesn't cover it. I'll record it in `ATTRIBUTION.md` and keep it out of the AGPL claim. Asking the author for written permission to redistribute would close the gap.
6. **Server storage: pluggable providers.** A `RoomStore` trait backed by memory, filesystem, SQLite and S3 (any S3-compatible service, e.g. MinIO). The provider is chosen by config, with filesystem as the default. Each provider passes the same conformance test suite. Optional room auth is still open; I'll make it a config flag that is off by default.
7. **No AI generation features.** Drafft shines by being scriptable instead: §11 makes agents first-class users.

## 11. Agents and MCP (added after review)

**MCP, native.** MCP is part of the app.
- `drafftink-mcp` is an MCP server over stdio, in two modes:
  - Headless: it runs its own `Session` and exports files, for scripts and CI.
  - Attached: it connects to a running desktop app over the loopback bridge (127.0.0.1, a per-launch token in a 0600 file, Origin and Host checks), so the user watches the agent draw live.
- The app can also serve MCP over streamable HTTP on loopback with the same token.

**MCP, web.**
- The browser page cannot listen on a socket. The page connects out to a local `drafftink-mcp --relay` on 127.0.0.1. The page refuses any bridge URL that is not loopback. The user pastes the relay's one-time token, or opens a link the relay prints. The agent then drives the open browser tab the same way as the native app.
- The page also registers its commands with WebMCP (`navigator.modelContext`), the emerging browser API, where the browser supports it. That needs no relay at all. It is feature-detected and optional, because the API is still in preview.

**Tools exposed.**
- **Generated:** MCP tools are generated from the command registry, so each command's schema and refusal text reach the agent unchanged, and nothing is maintained by hand.
- **Grouped:** the tools are grouped by family. `commands()` and `describe(id)` are included so the tool list stays small.
- **Batch:** `execute_batch` is exposed as a single tool, so a whole diagram can be built in one atomic, undoable call.

**Agent ergonomics.** These are commands in the registry, so the UI and tests get them too:
- **Create diagrams from a description.**
  - `diagram.create` takes `{nodes, edges, layout}` with symbolic ids and returns real ids. Edges are bound arrows.
  - Layout uses the `edit` crate's auto-layout: grid, layered/flowchart (a Sugiyama-style algorithm), tree and force.
  - Mermaid import is one more input to the same path.
- **Address elements by meaning.**
  - `element.query` filters by text, kind, style, region or connection.
  - Agents can label elements with names (`name` field) and refer to them by name in later calls.
  - `element.at` finds what is at a point.
- **Read the state as structure.**
  - `doc.outline` is a compact text and JSON summary: nodes, their labels, edges, groups, frames and bounds.
  - `doc.inspect` gives full detail.
  - `view.screenshot` returns a PNG of a region, so the agent can check what it drew.
- **Make relative edits.**
  - `arrange.align`, `arrange.distribute` and `layout.apply` work on a selection or a query.
  - `element.connect(a, b)` adds a bound arrow.
  - `element.relabel` changes a label.
  - Styles can be set through named themes.
- **Recover from mistakes.**
  - Every refusal text says what to do instead.
  - `history.undo` works for agent batches too.
  - `recent()` shows what happened.
- **Docs.** `docs/agents.md` and an MCP `prompts` entry teach the agent the model in one page, with worked examples. These examples run as e2e tests.

**Tests.** The MCP tool list is snapshot-tested against the registry. An e2e test has a scripted MCP client build a flowchart against the headless app, the native app and the web relay, then checks `doc.outline` and a pixel probe.

## 12. CRDT: supported, not optional for the app

"Optional CRDT sync" in §1 only meant this: the library tier (`canvas`) works against a `SceneStore` trait.
- **In the app.** The drafft.ink app and the editor always use the Loro-backed `doc` store. Collaboration, undo and incremental sync are fully supported.
- **For library users.** A third party who embeds just the canvas library, for example a static diagram viewer, can use the plain in-memory store without bringing in Loro. That keeps their wasm smaller.
- **Testing.** Both stores pass the same `SceneStore` conformance tests.
