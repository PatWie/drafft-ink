# Architecture: layers and dependency rules

The design is in `plan.md` (section 4). This page is the rulebook that `cargo xtask layers` enforces.

## Layers

A crate may depend on workspace crates of strictly lower layers. It may not depend on a crate of its own layer or a higher one.

| Layer | Crate | Role |
|---|---|---|
| L0 | `geom` | kurbo-based math, transforms, bounds, R-tree spatial index |
| L0 | `profile` | span profiler, frame-time histograms, counters |
| L0 | `protocol` | versioned collab wire types, shared by client and server |
| L1 | `scene` | element model: flat elements, ids, fractional z-index, groupIds, frameId, bindings, styles, serde. Pure data |
| L1 | `typeset` | text (Parley) and math (ReX) layout to outlines, with caches |
| L1 | `sketch` | roughr hand-drawn path generation, cached by (seed, geometry) |
| L2 | `render` | Vello scene encoding: per-element fragments, culling, level of detail, grid. Knows nothing about documents or commands |
| L3 | `canvas` | the reusable library (uses the `SceneStore` trait from `scene`): viewport, hit testing, picking, transformer handles, gesture recognizers, event dispatch, in-memory store |
| L3 | `doc` | Loro-backed `SceneStore`: transactions, undo, incremental sync deltas, schema version, derived indexes |
| L4 | `edit` | domain rules with typed errors: binding solver, elbow routing, groups, frames, align/distribute, z-order, snapping, flowchart helpers, bound text |
| L4 | `io` | native JSON (versioned, migration), `.excalidraw` and `.excalidrawlib` import/export, Mermaid import, SVG export |
| L4 | `collab` | sync client over a `Transport` trait, presence. Depends on `doc` and `protocol`; `engine` owns it and routes remote edits into the session |
| L5 | `engine` | command registry, `Session`, `execute` / `execute_batch`, refusals, editor state, gesture protocol, ports (clipboard, storage, files, clock) as traits |
| L6 | `gpu` | wgpu device and surface, offscreen render, compositing Vello and egui |
| L6 | `ui` | egui UI. Reads the registry, calls `execute`, shows refusals. No document logic |
| L6 | `bridge` | agent bridge: loopback JSON-RPC over WebSocket, MCP stdio server |
| L7 | `testkit` | headless harness, scenario runner, deterministic scene generator, probes |
| L8 | apps | `drafftink`, `drafftink-web`, `drafftink-server`, `drafftink-cli`, `drafftink-mcp` |
| none | `xtask` | build tooling. Standalone |

Package names are `drafftink-<crate>`; the desktop app is `drafftink`.

## Rules

1. **Downward only.** An edge to a higher layer is an error. An edge to the same layer is an error, with the exceptions below. This holds for normal, dev and build dependencies.
2. **Same-layer exceptions.** None today. `SAME_LAYER_EDGES` in `xtask/src/layers.rs` is empty and should stay that way. The `SceneStore` trait lives in `scene` (L1), so `canvas` and `doc` both use it without knowing each other. `collab` sits at L4 and depends on `doc` and `protocol`; `engine` (L5) owns the collab client and routes remote updates into the session.
3. **Testkit.** `testkit` (L7) may use any library. Other crates may use it only as a dev-dependency. It may not use an app.
4. **Apps.** An app may use any library and may use `testkit` as a dev-dependency. No crate may depend on an app, and apps do not depend on each other.
5. **xtask** uses no workspace crate, and no workspace crate uses it.
6. **UI and OS crates only at L6 and above.** `egui*`, `eframe`, `winit`, `rfd`, `arboard`, `accesskit*` and `bevy*` may not appear below L6, in any dependency kind. Lower layers reach the OS through port traits in `engine`.
7. **GPU crates.** `vello*` may appear from L2 (`render`). `wgpu*` may appear in `render`, which names wgpu types that Vello exposes, and from L6. `render` must not create a device or a surface. `gpu` owns the device, the surface and the compositing. Crates at L3 to L5 reach the GPU only through `render`'s encoded scenes.
8. **Every workspace crate has a layer.** A crate missing from the table fails. A table row without a crate fails too, so the table cannot drift.
9. **Wasm.** Everything at L0 to L5, and `drafftink-web`, builds for `wasm32-unknown-unknown` (`cargo xtask wasm`). Code below L6 takes time from the `Clock` port: `Instant::now` and `SystemTime::now` are disallowed by `clippy.toml` because they panic in the browser.
10. **No crashes.** `unwrap`, `expect`, `panic!` and `unsafe` are denied through `[workspace.lints]`. `clippy.toml` allows them in tests.

## How it is enforced

| Rule | Gate |
|---|---|
| 1 to 8 | `cargo xtask layers`. It reads `cargo metadata`. The rule engine has unit tests on synthetic graphs. `cargo xtask layers --list` prints each crate's layer |
| 9 | `cargo xtask wasm` and clippy's `disallowed-methods` |
| 10 | `cargo xtask clippy` (`-D warnings`, all targets and features) |
| Formatting | `cargo xtask fmt` (`rustfmt.toml`) |
| Licences, advisories, duplicate versions, sources | `cargo xtask deny` (`deny.toml`). Licences must be compatible with AGPL-3.0-only. Duplicate versions are warnings |
| AGENTS.md length and paths | `cargo xtask standards` |
| Tests | `cargo xtask test` (cargo-nextest, then doc tests) |
| Not yet written | `perf`, `size`, `fixtures`, `harness`. Each passes only while `xtask/pending-gates.txt` lists it. The list only shrinks |

`cargo xtask ci` runs every gate and prints a table. CI (`.github/workflows/ci.yml`) runs one job per gate plus native builds on three operating systems and a wasm build. The `ci` job at the end is the single required check. The `quarantine` job refuses locked crates.io versions younger than seven days (`utils/quarantine_check.py`, exceptions in `utils/quarantine_allow.txt`).

## Changing the layering

Edit `TABLE`, `SAME_LAYER_EDGES` or `EXTERNAL_RULES` in `xtask/src/layers.rs`, update this page and `plan.md`, and add a unit test for the new rule.
