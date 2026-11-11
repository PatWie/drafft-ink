# Feature backlog (coordinator)

Features decided but not yet assigned to a phase agent.

- **Inline LaTeX inside text** (NEW). The old app has LaTeX as a separate Math shape with an equation editor (old commits 63575f8 "Add LaTeX math rendering support with ReX" and bfba568 "Add math equation editor UI"); both are ported: `typeset` lays out math with ReX, and the catalog has `element.create kind=math`, `text.set_latex`, `text.edit_begin/end` with `latex` and `ui.open_equation_editor`. The README's "inline LaTeX math" (`$...$` inside a text element) did not exist in the old code. We add it: Parley inline boxes holding a MathLayout (design sketch in docs/typeset.md), text runs with a math span kind in `scene`, and editing through `text.set`.
- Bundled font size on the web: `noto-sans` adds 1.9 MB raw wasm; load fonts lazily or as separate assets (size gate).
- Server: refuse to relay updates that raise `meta.schema` beyond what the server serves (docs/crdt.md §8); the server imports updates into its own Loro doc, so it can read the schema after import and roll back / not relay.
- Engine: show `LoroStore::blocked_by_schema()` as a read-only status.
- Perf: `edit::group::Relations::build` is one pass over all live elements per call; gesture frames at 100k elements need a cached relation index.
- Web: hovering over empty canvas costs ~9% CPU in Chromium with 0 frames drawn (event handling); investigate in the perf phase.
- Dedupe: `edit::paste::text_element` duplicates `edit::create::text_element`; `Bytes`/`At` helpers in cmd/image.rs and cmd/clipboard/common.rs should be shared with doc.drop.
- `element.relabel` labels use the default text style, not the tool's text defaults (`text_bind::relabel` takes no font).
- `image.insert` validates header and size only; damaged pixel data fails at render time.
- Snap types: `Snap` lives in cmd/arrow.rs but element.move/resize need the same type; move to a shared module.
- text.edit_end style is a local StyleProps subset and new text uses TextStyle::default(): switch to edit::style (SEAM(f-style) markers in engine session.rs).
- collab: auth token setting/arg (auth_token always None today); snapshot import blocks the polling thread (move to a worker on native, chunked on web).
- Perf: loading a 100k scene into a TabDoc takes ~6 s release / 40 s debug (testkit finding); tab rename undo leaves tab dirty:true.
- HIGH (data loss): `doc.save` marks the document saved before the queued port write succeeds; a failed write is only a warning. Saved state must follow the write result (Files port write must report back, or the save must be synchronous).
- One-path rule: `clipboard.copy_as` png has its own render path duplicating `doc.export`; route it through the export code.
- `tab.new from=intro` can use `doc::intro_document`; SVG export embeds no fonts yet (marked call site in cmd/doc/export.rs); dark SVG ignored.
- UI reads selection style via `edit::style::read` directly (to avoid a ring entry per frame). Add `Session::query(id, args)` that runs `reads_only` commands without recording to the ring, and have the UI use `style.get` through it; then drop ui's edit/scene deps if possible.
- UI embeds GelPen fonts a second time (include_bytes); expose font bytes from typeset.
- UI: search dialog has no result list; stats panel lacks bounds editing (now possible: selection.get, element.set_bounds); touch-sized mode; context-menu paste should land at the click point.
- Interaction: default type of new arrows lives in `Session::ext` (`interact::NewArrowPath`); make it a tool/style default owned by a command. text.edit_begin needs a width arg (text-tool drag = fixed-width box). Elbow preview while drawing is straight. `edit::binding::find_hovered` scans all elements per pointer move: use the spatial index. Click on a line vertex should call `arrow.edit_points select_point`. Closed-line closing-segment midpoint drag refused. Interaction's scene index follows the store via O(n) scan on revision change: use a change feed. Interaction tests stub text.edit_begin/collab.laser, now registered: drop the stubs.
- Dedupe: canvas-js `src/fonts.rs` duplicates engine `TypesetMeasure`; move the typeset->io/edit measuring adapter into crates/io (L4 may use typeset) and use it from both.
- Size: bundled Noto Sans is ~1 MB brotli of the wasm; load fonts lazily (fetch) or behind a feature; `wasm-opt --all-features` breaks the module with binaryen 133 (use explicit features; fix docs/stack.md and crates/gpu/scripts/build_web_demo.sh).
- `cargo clippy --target wasm32` fails in drafftink-profile spans.rs:213; add a wasm clippy run to xtask wasm.
- canvas-js: remote selections not drawn; no image-insert helper; tiny shapes dominated by handles (move on body should win when the shape is smaller than handles).
