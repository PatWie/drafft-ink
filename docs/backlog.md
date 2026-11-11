# Feature backlog (coordinator)

## Open

Checked against the code and tests on main at the branch `last-pass`.

- **Command schemas in the wasm** (`schemars`, 2.8% brotli, 89 KB): not moved. Every schema reader is synchronous; a fetched file needs an async refactor. Cheaper first step: a cargo feature that omits the schemas from the web build (`performance.md` section 8.2).
- **O(scene) by design**, without a counter: `arrange.z_order` (it lists every element), `clipboard.copy`/`copy_as` (a snapshot of the document), frame capture after a resize, `element.restore` (the one direct `Relations::build` call left, `edit/src/lifecycle.rs`) and `group_name`. Convert one if it shows up in a profile.
- **Render order**: a new, removed or reordered element, or a commit of more than an eighth of the scene, still rebuilds the whole order (a move or style edit is patched, `render.md` section 8).
- **Autosave**: one huge element and the image-file hash check are still single steps; the native writer thread and the web storage path are covered by unit tests and a type check only, not by a run of the real app.
- **Collab**: a tab that already has content still imports a room snapshot in one piece; the first import call costs about 0.8 us per element (a frame only up to ~8,000 elements).
- **TabDoc load at 100k** is 2.6 s release, spent in Loro (field inserts 1.7 s); debug builds set `opt-level = 3` for `loro*` (`performance.md` section 7.9). Only a different Loro load path would change it.
- **Present**: the X11 copy under Xvfb is 70 to 85 % of the native CPU (1 to 3.5 % without present in `pan@10k`; Fifo and Immediate do not differ). Not our code. Mailbox could not be measured here (the NVIDIA driver offers Fifo and Immediate only).
- Tab bar: dirty state compares content through undo/redo only; a remote update between an edit and its undo keeps the tab dirty on purpose.
- **Mermaid node shapes** hexagon, cylinder, parallelogram, trapezoid, flag, subroutine and double circle are drawn as the closest shape an arrow can bind to, with a warning; a real outline needs a bindable polygon element kind.
- **Text editor**: arrows bound to a container that grows during an edit jump when the edit ends; no I-beam cursor over text; RTL weakly tested; no per-run bold or italic; no real OS input method tried; `Esc` commits and cancel is only `text.edit_end commit=false`.
- **Overlay leftovers**: distance badges for alignment snaps, IME composition in `TextEdit`.
- **Web**: the WebGL2 fallback for browsers without WebGPU is done (D13); it was tried in Chromium only, and a lost WebGL context shows the error page. The Collaborate dialog has no token field (see Collab above).
- **Text editor**: RTL weakly tested; no real OS input method tried; `Esc` commits and cancel is only `text.edit_end commit=false`. (Branch `text-polish` fixed: bound arrows follow a growing container, the I-beam, per-run bold and italic, the preedit underline on both shells.)
- **Overlay leftovers**: distance badges for alignment snaps.
- **Web**: no CPU fallback for browsers without WebGPU. The Collaborate dialog has a token field (`text-polish`).

## Done (branch final-residuals)

- `cargo xtask e2e` on this tree: 74 browser and 15 native tests pass (no fixes needed). The backlog item about `document.spec`, `session.spec` and `startup.spec` was stale: the specs already match the renderer port.
- Mermaid shapes: hexagon, cylinder, parallelogram, trapezoid, flag, subroutine and double circle map to the closest shape an arrow can bind to (rectangle with its own corner rounding, larger ellipse) and the warning names each approximation once; a real polygon outline would need a bindable element kind that holds a label (fb868b7, `commands.md` 2.19).
- `drafftink-cli` is in the AppImage, deb, tar.gz, dmg, zip and MSI (d128923).
- Context-menu rows are greyed with the engine's text instead of hidden: `frame.unwrap`, `frame.select_children`, `frame.remove`, `text.wrap_in_container`, `arrow.edit_points` answer availability from the selection (7a3e777, `ui.md`). Decisions: Copy as text is always listed and the engine refuses at the click; the convert targets, bind/unbind and the frame rename dialog stay conditional because they need an argument the selection cannot give; the properties panel keeps hiding a section that applies to no selected element.
- Windows and macOS: `cargo check` of the four binaries (and `--workspace --all-targets` for windows-gnu and aarch64-apple-darwin) passes with no cfg fixes needed; `scripts/cross-check.sh` repeats it; recorded in `packaging.md` (d128923). Nothing was linked or run; `x86_64-pc-windows-msvc` needs the Windows SDK and was not checked.
- `io::legacy::arrow_bounds` is compiled only with the `facts` feature, which removes the dead-code warning of every build (6ad26b9).

## Done (branch audit-e)

- Mermaid subgraphs are drawn as titled cluster boxes again, nested, with the layout placing the members inside; edges to a subgraph are drawn and bound to the box. The three old tests are ported (`diagram_tests.rs`, `diagram_create.rs`). Decision: titled rectangles, not frames (`commands.md` 2.19).
- Native binary: `strip` in `release` (55.2 to 46.9 MB), the `dist` profile for packages (42.2 MB), budgets `native.*` in `perf/budgets.json`, `cargo xtask size --native` (`performance.md` 8.3).
- `xtask perf --bench` and `--app` follow `CARGO_TARGET_DIR`; `--app web` builds the scenario web app and the server itself.
- The packaged server has the sqlite and s3 stores; `release.yml` smoke-tests it.
- `drafftink-cli` is a real tool (`docs/cli.md`). README, deviations and parity are current.
- Left open: no CPU fallback for browsers without WebGPU. (The Mermaid node shapes and the CLI in the release packages are done in branch final-residuals, below.)

## Done (branch backlog-c)

Stale (done by earlier branches, verified in code and tests):
- Bundled font size on the web: fonts are on-demand assets (112a991; `cargo xtask size` passes).
- Inline LaTeX editor surface: drawn through `TextLayout` with `reveal` (7e2759e; tests 6f3bb37, `editor/tests.rs`).
- UI style reads go through `Session::query` (5d30cf8); the UI search result list, stats bounds editing, paste at the click point (6217985) and touch layout (50ddf25) exist.
- The native and web scenario drivers exist (120c825, 14a28d1, `xtask perf --app` 456f6ec).

Implemented (branches backlog-c-perf, -collab, -export, -web; hashes are from those branches, `git log --oneline` shows them on backlog-c):
- Autosave is incremental: validation and JSON in slices (8 ms longest `workspace_tick` at 100k), a writer thread on native (ca02c90).
- Render order is patched from the touched ids; a drag frame at 100k takes 23 to 25 ms instead of 60 to 68 (4d122f1).
- One-shot commands (group, arrange, frame, delete, duplicate, lock, clipboard, library, selection) use `Reverse::of`; 0 scene scans at 1k, 10k and 100k (cd7f114).
- The text relayouts in drags were a driver bug (it resized instead of moved); `drag@10k` goes from 671 relayouts to 1 (d849186).
- 100k TabDoc load: cost measured and documented, debug builds speed up from 31.6 s to 9 s (43665b6).
- Collab auth token as `token` argument of `collab.connect` / `collab.join_room`, `--token-file` / `DRAFFTINK_COLLAB_TOKEN`, hidden in the ring and in `Debug` (1d22a25). Decision: argument, not a setting, so it never reaches `settings.get` or the settings file.
- Collab snapshot import is decoded in 8 ms slices on desktop and web (40a9ba3). Decision: no worker thread, the replicated store holds `Rc`.
- A web `batch` with a PNG export is taken back with `pending_render` and rerun by the bridge host (f3e0b42).
- SVG export embeds `@font-face` for the fonts the texts use, and `dark` recolours as the PNG does (15bdfad, 3c2f698, 78c8d5d).
- The UI uses the typesetter's font bytes instead of its own `include_bytes` (1a13afa).
- Present mode: Fifo on every platform, measured against Immediate and the old driver-first default (0492580, numbers in `performance.md`).
- Web hover: the ~9 % is mostly Chromium delivering 125 mouse moves per second (a blank page costs 5.6 to 10 %); the `getCoalescedEvents` call is gone from hover moves (ea166d5); `web_check.py --app` asserts 0 frames while hovering.

## Done (branch render-overlays)

- Overlay (the render crate and `canvas-js`; the app glue does not copy lasso, link icons and remote selections yet, see Open): pointers with name labels, remote selection outlines, peers' laser trails, link icons (also a pick target, `Target::LinkIcon`), distance badges on equal-spacing guides, caret and selection of the on-canvas text editor (`render::TextEdit`); `paint_overlay` now takes the typesetter. Frame names keep a constant size on screen. Mapping for the app in `docs/render.md` §5.
- canvas-js: `setRemoteUsers`, `addImage` (bytes, Blob or File; natural size from `drafftink-io`), the `linkIcon` flag on events; a drag on a tiny selected shape (under 32 px on screen) moves it (`TransformerConfig::tiny_frame_px`, also for the editor).
- Left over: a `Cursor::Pointer` variant for the hand over a link icon (it would break exhaustive matches in the shells, so the icon uses `Cursor::Default` for now); distance badges for alignment snaps; IME composition in `TextEdit`.

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
- Text editor (branch `text-editor`): done and described in `docs/exceptions.md` E16. Left over: the editor draws the grown container of a label but not the arrows bound to it, so they jump when the edit ends; `Esc` commits (E16, `commands.md` §6.2) and there is no key to cancel (`text.edit_end commit=false` from the palette or an agent); no per-run bold or italic (the scene stores colour only); the colour popover does not show the colour of the selected characters; the IME rect and the soft keyboard were checked only on Chromium (the browser textarea path) and never with a real OS input method.
