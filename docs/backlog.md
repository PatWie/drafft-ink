# Feature backlog (coordinator)

Features decided but not yet assigned to a phase agent.

- **Inline LaTeX inside text** (NEW). The old app has LaTeX as a separate Math shape with an equation editor (old commits 63575f8 "Add LaTeX math rendering support with ReX" and bfba568 "Add math equation editor UI"); both are ported: `typeset` lays out math with ReX, and the catalog has `element.create kind=math`, `text.set_latex`, `text.edit_begin/end` with `latex` and `ui.open_equation_editor`. The README's "inline LaTeX math" (`$...$` inside a text element) did not exist in the old code. We add it: Parley inline boxes holding a MathLayout (design sketch in docs/typeset.md), text runs with a math span kind in `scene`, and editing through `text.set`.
- Bundled font size on the web: `noto-sans` adds 1.9 MB raw wasm; load fonts lazily or as separate assets (size gate).
