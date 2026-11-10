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
