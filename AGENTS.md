# Drafft.ink: instructions for agents

Drafft.ink is an infinite-canvas whiteboard in Rust (Vello on WebGPU, egui, Loro CRDT) for web and native. It has three tiers: a reusable canvas library, an Excalidraw-class editor engine, and the app. The design is in `docs/plan.md`, the layering is in `docs/architecture.md`, and the command catalog is in `docs/commands.md`.

## Hard rules (and what enforces each)
- Everything a user can do is a command in the registry (`crates/engine`). Buttons, shortcuts, menus, the palette, scripts, MCP and tests all call `Session::execute` / `execute_batch`. Nothing else changes state for a user action: `cargo xtask layers`, the registry tests.
- High-frequency gestures (pointer drag, pan, pinch) are the only exception. Each one is listed in `docs/exceptions.md` with its reason, and its final state is reachable as a command.
- Layering: geom/profile/protocol (L0), scene/typeset/sketch (L1), render (L2), canvas/doc (L3), edit/io/collab (L4), engine (L5), gpu/ui/bridge (L6), testkit (L7), then apps. No upward or sideways edges, and no egui, eframe, winit or rfd below L6: `cargo xtask layers`.
- A refusal is a value (`Refusal`) with plain-words text that says how to proceed, never a panic or a silent no-op. Commands never open dialogs: a decision is an argument with a safe default.
- Domain rules live in the domain crate (`edit`, `doc`, `io`) with typed errors. A command validates and delegates, and never holds a second copy of a rule.
- Facts about a thing go on its registration (a field), not into name lists. A test fails when a registration omits the field.
- Never crash: no `unwrap`, `expect`, `panic!` or `unsafe` in non-test code, xtask included. `unreachable!` and `dbg!` are fine where they make sense; `todo!`/`unimplemented!` only temporarily. Errors are `Result`, and every input is hostile: clippy `-D warnings`, `hostile_params_never_panic`.
- Idiomatic Rust: `?` and typed errors (`thiserror`), iterators over index loops, borrow instead of clone, newtypes and enums over strings and flags, no needless `pub`, small modules, rustdoc on public items. Clippy `-D warnings` is the floor, not the bar.
- Everything below L6 builds for `wasm32-unknown-unknown`: `cargo xtask wasm`.
- Idle means idle: no redraw without a dirty reason, every repaint request is tied to pending work: `cargo xtask perf --idle`.
- Never weaken an assertion to get green. Find out whether the app or the test is wrong, and fix that one.
- A regression test goes in the area's existing test file.
- Code, comments, commit messages and docs are in English. Write plainly.
- Ported or adapted code from Excalidraw (MIT) or other projects gets a row in `ATTRIBUTION.md`.
- Run `cargo xtask ci` before every commit.

## Working on X: read first
| Working on | Read |
|---|---|
| A command or a crate boundary | `docs/commands.md`, `docs/architecture.md` |
| Tests, scenarios, fixtures | `docs/testing.md` |
| Rendering, the frame loop, wasm size | `docs/performance.md` |
| Agent bridge, MCP | `docs/agents.md` |
| File formats | `docs/formats.md` |
