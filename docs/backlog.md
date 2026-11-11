# Feature backlog (coordinator)

Features decided but not yet assigned to a phase agent.

- **Inline LaTeX inside text** (NEW). The old app has LaTeX as a separate Math shape with an equation editor (old commits 63575f8 "Add LaTeX math rendering support with ReX" and bfba568 "Add math equation editor UI"); both are ported: `typeset` lays out math with ReX, and the catalog has `element.create kind=math`, `text.set_latex`, `text.edit_begin/end` with `latex` and `ui.open_equation_editor`. The README's "inline LaTeX math" (`$...$` inside a text element) did not exist in the old code. We add it: Parley inline boxes holding a MathLayout (design sketch in docs/typeset.md), text runs with a math span kind in `scene`, and editing through `text.set`.
- Bundled font size on the web: `noto-sans` adds 1.9 MB raw wasm; load fonts lazily or as separate assets (size gate).
- Perf: `Relations::build` still serves the one-shot commands (group, frame, z-order, clipboard, library); gesture frames use the derived index (`Reverse::of`). Convert them if one shows up in a profile.
- Web: hovering over empty canvas costs ~9% CPU in Chromium with 0 frames drawn (event handling); investigate when the web scenario driver exists (the engine side counts 0 redraws and 0 scans per hover, perf_work).
- Perf: the native and web scenario drivers (`--perf-scenario`, `runScenario`) are not written; `cargo xtask perf` has the work-counter and bench tiers (perf/README.md).
- Perf: the render order is rebuilt after every edit and every gesture frame while most of a large scene is visible (one pass, 12 ms at 100k); patch it from the change set if a zoomed-out edit of a 100k scene matters.
- collab: auth token setting/arg (auth_token always None today); snapshot import blocks the polling thread (move to a worker on native, chunked on web).
- Perf: loading a 100k scene into a TabDoc takes ~6 s release / 40 s debug (testkit finding).
- SVG export embeds no fonts yet (marked call site in cmd/doc/export.rs); dark SVG ignored.
- UI reads selection style via `edit::style::read` directly (to avoid a ring entry per frame). `Session::query` now exists (backlog-b): have the UI use `style.get` through it; then drop ui's edit/scene deps if possible.
- UI embeds GelPen fonts a second time (include_bytes); expose font bytes from typeset.
- UI: search dialog has no result list; stats panel lacks bounds editing (now possible: selection.get, element.set_bounds); touch-sized mode; context-menu paste should land at the click point.
- Size: bundled Noto Sans is ~1 MB brotli of the wasm; load fonts lazily (fetch) or behind a feature.
- canvas-js: remote selections not drawn; no image-insert helper; tiny shapes dominated by handles (move on body should win when the shape is smaller than handles).
- Tab bar: dirty state compares content through undo/redo only; a remote update between an edit and its undo keeps the tab dirty on purpose.

## Done (branch backlog-b)

- DECIDED, implemented: opening a non-native file (.excalidraw, legacy, PNG/SVG with scene) sets NO save target, so `doc.save` behaves like save-as and never overwrites a foreign file with drafft JSON; it is still a recent file (e8f5768).
- `Storage::put_document`: a stored document save can answer pending and is confirmed by `Session::on_write_done`, like `Files::write` (811e514).
- Flaky `cmd::perf::tests::last_call_reports_only_what_happened_since_the_previous_call`: `Session::count_this_thread_only` and `profile::snapshot_thread` make the test exact (3ed9ec6).
- The 3.5 h compositor hang was a deadlock inside the NVIDIA Vulkan driver (device destroy on one test thread against the driver's thread-exit destructors on another), not an unbounded wait of ours; the tests now run every device on one worker thread (200 runs, no hang) and the readback wait has a 30 s limit (9b54aa0).
- `Bytes`, `InlineBytes`, `At` and `Pt` are shared by `image.insert`, `clipboard.paste` and `doc.drop` (59b7e3e).
- `image.insert` decodes the pixels once and refuses damaged data at insert time (3400a17).
- Interaction tests run over the real `text.edit_begin` and `collab.laser` (4f7a178). `text.edit_begin` has a `width` argument and a text-tool drag opens a fixed-width box, `OpenTextEditor.width` (93a7f4b; the shell's editor must show the box). A click on a line or arrow vertex runs `arrow.edit_points select_point` (592c94a). The closing segment of a closed line takes a midpoint (b41e508). The elbow preview shows the route (75ac38f).
- `cargo xtask wasm` also runs clippy for wasm32; the `spans.rs` finding is fixed (a2b4260). `wasm-opt` runs with explicit features in `build_web_demo.sh` and `docs/stack.md` (221a01a).
- `Session::query(id, args)` runs `reads_only` commands without a ring entry (500c4c9).

## Done (branch refactor-l3-l4)

- `io::typeset_measure::TypesetterMeasure` is the one typesetter adapter for `io::TextMeasure` and `MathSvg`; the engine's `TypesetMeasure` and canvas-js use it (canvas-js `SvgText` is gone).

## Done (branch backlog-a)

- HIGH data loss: `doc.save` marks the document saved only when the write succeeded; a failed write refuses with `io_failed` ("NOT saved"); a pending write (web) keeps the tab dirty until `Session::on_write_done` (f14e8d0; `tab.close on_unsaved=save` follow-up 4fb08f5).
- One-path rule: `clipboard.copy_as` png goes through `doc::export::render_png` (f3f9fc0).
- `tab.new from=intro` uses `doc::intro_document` (a882fb7).
- Dedupe `edit::paste::text_element` and `edit::create::text_element`: one builder with `TextPlace` (61d6709).
- `Snap` argument moved to `engine::snap`, shared by arrow and element move/resize (68f093b).
- `text.edit_end` style is `edit::style::StyleProps`; new text and labels use the tool's text defaults; `text_bind::relabel` takes the font (b4aa7fb).
- Undoing a tab rename (any edit) back to the saved content leaves the tab clean (e773b66).
- Engine status `blocked_by_schema` in `engine.state`; document-mutating commands refuse with `newer_schema` (72393d9).
- Server refuses and rolls back updates that raise `meta.schema` beyond the served schema (69f08a1).
- Accepted (F1): `element.relabel`/`text.set` with empty text on a bound label removes the label; only free text refuses `empty_text`.
