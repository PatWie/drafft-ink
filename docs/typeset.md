# `drafftink-typeset`: text and math layout (L1)

`typeset` turns text and LaTeX into immutable layouts: glyph runs, rules, line boxes, bounds and
caret geometry. It does not draw. `render` (L2) draws the layouts with Vello, the editor (L4/L5)
reads their bounds for hit testing, and `io` can ask for outlines to write SVG.

The rule it exists for: **bounds are measured once, by layout, when the content changes.** The old
app measured text while rendering (`Text::set_cached_size` through a `RwLock`), so a text that was
never drawn had an estimated size, and culling and hit tests depended on render order.

It depends on `drafftink-profile` (L0), `parley`, `peniko`, `kurbo`, ReX and `ttf-parser`. It has no
dependency on `scene`: callers convert their element data into the input types below.

## API

```rust
let mut ts = Typesetter::new();                        // registers the bundled fonts
let layout: Arc<TextLayout> = ts.text(&spec);          // cached
let size = ts.measure(&spec);                          // same cost, for bounds
let math: Result<Arc<MathLayout>, MathError> = ts.math(&MathSpec::new("\\frac{a}{b}"));
let bound: BoundText = ts.bound_text(&spec, Container::new(ContainerKind::Rectangle, w, h), VerticalAlign::Middle);
ts.stats();                                            // TypesetStats: hits, misses, entries, bytes, evictions
```

Inputs (`style.rs`): `TextSpec { text, style: TextStyle, spans: Vec<Span>, align, wrap, line_height }`.
`TextStyle` has family, weight, italic, size, colour, underline and strikethrough. A `Span` is a byte
range with a `SpanStyle` whose `Option` fields override the base style; later spans win. The old
app's per-character colours convert with `Span::from_char_colors(text, &colors)`; the old
`(family, weight)` pairs convert with `TextStyle::from_legacy` (it reproduces two quirks: Noto Sans
"Light" is the italic face, and Vanilla Extract ignores the weight).

`Wrap::None` breaks only at `\n` (autosize). `Wrap::Width(w)` breaks at `w`; a word wider than `w`
is split. `LineHeight::Normal` is the font's own line height, which is what the old app used;
`FontSize(1.25)` gives Excalidraw's.

### `TextLayout` (`layout.rs`)

All coordinates are relative to the top-left of the layout box, x right, y down. The caller places
the box with a translation (and a rotation about the box centre, if the shape has one).

| Field | Meaning |
|---|---|
| `size` | Width of the longest line (or the widest wrapped line) and the sum of the line heights. Use for bounds. |
| `box_size` | `size`, but as wide as the wrap width when wrapping. Lines are aligned inside it. |
| `lines: Vec<LineInfo>` | Per line: byte range, baseline, top, bottom, ascent, descent, aligned x, content width. Never empty: empty text has one line with a caret height and width 0. |
| `runs: Vec<GlyphRun>` | Font (`peniko::FontData`), size, glyphs `{id, x, y}` (`y` is the glyph baseline), colour, style index, line, baseline, `skew` for a synthetic italic. One call to `draw_glyphs` per run. |
| `decorations` | Underlines and strikethroughs as filled rectangles with colours. |
| `styles` | The distinct resolved styles; `GlyphRun::style` indexes it. Index 0 is the base style. |
| `id` | Unique per laid-out layout. A render fragment can be validated against it. |
| `truncated` | The input was cut (more than `MAX_TEXT_BYTES` or `MAX_SPANS`). |

`first_baseline()`, `glyph_count()`, `hit_test(point, tolerance)` (line boxes, so ragged text is
tighter than its bounds) and `outline()` complete it. `outline()` returns `(colour, BezPath)` in
paint order for exports; it reads glyph outlines with `ttf-parser` and ignores variation
coordinates (the bundled fonts are static).

### Caret and selection (`caret.rs`)

Methods on `TextLayout`, all stateless and tolerant of stale input (indices and points are
clamped, NaN is 0). The editor keeps the text and a `TextSelection`; after the text changes it
lays out again and rebuilds the selection with `selection_at_index` / `selection_from_range`.

`caret_at_point`, `caret_rect(caret, width)` (1.5 in the old app), `caret_point`, `selection_rects`,
`word_at_point` (double click), `paragraph_at_point` (triple click), `line_at_point`,
`word_range_at(index)`, `extend_to_point` (drag), `shift_click`, `select_all`, and
`move_selection(sel, Motion, extend)` for left/right/word/up/down/line and paragraph ends/text ends.
Up and down keep the horizontal position. Points are layout-local: the editor subtracts the shape
position and **undoes the shape rotation** first (the old app forgot the rotation).

`selection_rects` and the word motions are our own code, not Parley's. Parley 0.12's
`Selection::geometry` has a `debug_assert!` that fails when a combining mark follows a newline, and its
`next_visual_word` / `previous_visual_word` never return on some wrapped mixed-script text. The
rectangles match Parley's on ordinary text (unit test, all ranges, including right-to-left); word motion
uses the logical word steps, which are finite, and maps right/left by the text direction at the caret.
`tests/hostile.rs` calls every caret method with hostile indices and points on arbitrary text.

Backspace and Delete: move the selection one step with `Motion::Left` / `Right` and delete between
the two indices; that respects grapheme clusters. Only the editor changes text.

### Inline math (`inline.rs`, `layout.rs`)

`TextSpec::math: InlineMath` (`Off` by default in this crate, `Auto` for text elements, see `model.md` for the scanning rules) and `TextSpec::reveal: Option<usize>`. With `Auto` the text is scanned (`formula_ranges`, `has_inline_math`); each `$latex$` is laid out as a `MathLayout` at the size of the text there (cached with the math cache), and the text goes to Parley with the formula replaced by one zero-width space (U+200B) and `\$` by `$`. The formula is a Parley *inline box* in front of its space: width the formula's width, height `height - depth`, baseline `height`, so it sits on the baseline of its line and a tall formula grows the line. U+200B keeps the box and its placeholder on one line (U+2060 did not: Parley breaks between them) and allows a break after the formula. A formula wider than the wrap width stands alone on a line and overflows; `size.width` includes it, so bound text grows its container.

What `TextLayout` adds: `formulas: Vec<InlineFormula>` (`src` byte range with dollars, `math`, `color`, `origin` = baseline origin in layout space, `rect` = the box, `line`) in text order, `formula_errors: Vec<FormulaError>` (source range and `MathError`), `text_len` stays the length of the **source**. `LineInfo::text_range`, `Caret::index`, `TextSelection::range` and every method of `caret.rs` use source indexes; `TextMap` converts (a formula is one cluster of 3 laid-out bytes; an index inside a formula or an escape maps to the nearer edge). `outline()` includes the formulas, `weight()` is unchanged (the formulas are shared `Arc`s owned by the math cache).

Decision: **a formula is one unit** for the caret. The caret is at the left or right edge of its box (`caret_rect` computes the left edge itself, because Parley puts a caret in front of the placeholder behind the box), one `Motion::Left/Right` crosses it, `extend` selects `$...$` whole so Backspace deletes the formula, a click picks the nearer edge, a double click (`word_at_point`) selects it, `selection_rects` covers its box. To edit the source the editor lays the text out again with `reveal = Some(layout.reveal_index(&formula))` (just after the opening `$`); while `reveal` is strictly inside a formula that formula is plain text and the caret moves inside it freely. The editor clears `reveal` when the caret leaves the source. `formula_at_index` and `formula_at_point` find the formula for Enter/F2 and double click.

A broken formula (`MathError`, including too long or too complex) is laid out as its source, dollar signs included, in `FORMULA_ERROR_COLOR` and listed in `formula_errors`; an unmatched `$` is not an error. The cache key of a text covers the text (so the LaTeX), the mode, `reveal`, and every style field; the math cache key is `(latex, size, fallback)`. The hostile limits are the math limits (`MAX_LATEX_BYTES`, `MAX_TOKENS`, `MAX_NESTING`, a precheck) plus `MAX_FORMULAS` per text.

Known limits: a formula is not underlined or struck through with its text, a text colour with transparency is applied by the renderer to the formula as a whole, and right-to-left text around a formula is untested.

### `MathLayout` (`math.rs`)

Coordinates are relative to the baseline origin, y down (glyphs above the baseline have negative y).
`width`, `height` (above the baseline) and `depth` (ReX's signed depth, negative when the box goes
below the baseline) are the numbers the old `Math::cached_size` held. `bounds()` is
`Rect(0, -height, width, -depth)`; the old app's bounds are that rectangle moved to the shape position.

Glyphs are `{font: Math | Text, id, x, y, size, color: Option<Rgba8>}`; `font(slot)` returns the font
file. Rules are rectangles. The colour is `None` unless the formula sets `\color{..}`, so the layout
does not depend on the shape colour and is cached once per `(latex, size, fallback)`.

By default letters, digits and symbols that GelPen has are drawn with GelPen at 0.75 of the math
size, as the old app did, so equations match the handwriting; everything else (`\sum`, `\int`,
braces) uses STIX Two Math. `MathSpec::text_font_fallback = false` switches it off.

Scripts (`x^2`, `x_1`, `e^{i\\pi}`) follow the same rule: ReX asks the math font for the `ssty` script forms (`two.ssty`), and the engine maps each form back to the character of its base glyph, so a script is drawn with GelPen at 0.7 of the base size, raised or lowered by ReX's shift. Without that map the script form fell through to STIX Two Math at 0.93 of the base size and looked like a full-size digit (the old app did the same). Characters GelPen lacks, such as `\\pi`, stay in STIX at script size.

Errors are `MathError { kind: MathErrorKind, span: Range<usize>, message }`. Kinds: `TooLong`,
`TooComplex`, `UnknownCommand`, `UnknownSymbol`, `UnknownEnvironment`, `UnknownColor`, `BadDimension`,
`Unbalanced`, `MissingArgument`, `Syntax`, `Layout`, `TooLarge`, `Internal`. ReX reports no
positions, so `span` comes from searching the source: exact for the "unknown X" kinds, the unmatched
brace for `Unbalanced`, the `^`/`_` for a missing script argument, the end of the input for "ends
too early", and the whole input otherwise. Spans are always on `char` boundaries. `line_col(latex)`
converts. A broken formula is cached like a good one, so it costs one parse, not one per frame.

### Bound text (`bound.rs`)

`Container { kind, size }` with `ContainerKind::{Rectangle, Ellipse, Diamond, Arrow}` implements
Excalidraw's rules (`research/parts/02c` §3.2 to §3.4): padding 5 per side; the ellipse and the
diamond give the inscribed rectangle; arrow labels wrap at `max(0.7 w, 11 em)`. `Typesetter::bound_text`
wraps the text at the container's text width, grows the container's height (never shrinks it) when
the text is taller, widens it only for an unbreakable character, and returns the text origin by
vertical alignment, all in the container's frame. `BoundText::container_size` is the grown size; the
caller moves the container if another edge must stay fixed (`edit` owns that). For arrows the origin
is centred in the box; `edit` places the label along the arrow.

## Cache (`typesetter.rs`, `cache.rs`)

Two LRU caches, text and math, each bounded by entries and by estimated bytes
(`CacheConfig`, defaults 8192 entries / 64 MiB for text, 2048 / 16 MiB for math). The key is a
128-bit digest of everything that changes the result (text, every style field including colour and
**alpha**, spans, alignment, wrap width, line height) plus the font set version, which changes in
`register_font`. A layout larger than the whole budget is still served (it evicts everything else).

`stats()` returns `TypesetStats`: text and math hits and misses, entries, bytes, evictions and the
font set version. Every hit and miss also bumps `Counter::CacheHit` / `CacheMiss`, and every layout
that actually ran bumps `Counter::Relayouts`, so scenario tests can assert "no relayout during
pan" with `WorkScope`. Misses run inside a `typeset_text` / `typeset_math` span.

## Fonts (`fonts.rs`, `assets/fonts/`)

| `FontFamily` | Faces |
|---|---|
| `GelPen` | Light, Regular, Heavy (three Parley families: `GelPenLight`, `GelPen`, `GelPenHeavy`) |
| `GelPenSerif` | Light, Medium (= Regular), Heavy |
| `VanillaExtract` | one face |
| `NotoSans` | Regular, Bold, Italic (an `ExtraFont`: built in with the Cargo feature `noto-sans`, or provided by the host, see below) |
| `PlexSans` | Light (300), Regular (400), Heavy (700), real weights of one variable file (`IBMPlexSans-Variable.ttf`, an `ExtraFont`: built in with the Cargo feature `plex-sans`, or provided by the host). Parley picks the weight on the `wght` axis; `TextLayout::outline` draws the run's variation settings |
| `Custom(name)` | any family added with `Typesetter::register_font(bytes)` |

The family-to-face table exists once (`fonts::face_for`); the old app had it twice. Every stack ends
with Noto Sans as the fallback for glyphs a hand-drawn font lacks (accents, Greek, Cyrillic). There
are no system fonts (`parley` without `fontique/system`), so output is the same on web and native.
There is no CJK or emoji font: such characters lay out with the missing-glyph box, as in the old
app. Line breaking of CJK uses per-character fallback (ICU data has no dictionary model; `icu_segmenter`
prints a notice to stderr).

The big fonts are `ExtraFont`s: Noto Sans (three files), IBM Plex Sans (one variable file) and STIX Two Math. The Cargo feature `embedded` (= `noto-sans` + `plex-sans` + `stix-math`) builds them into the binary; `drafftink-engine` turns it on for every target except wasm32, and `canvas-js` always. A build without them (the web app) starts with the seven GelPen and Vanilla Extract files only.
A layout that needs one of the others records that: `Typesetter::wanted_fonts()` lists the files still missing (all three Noto files when the family `noto_sans` is used or a character has no glyph in the loaded fonts; the Plex file when the family `plex_sans` is used; STIX when a formula is laid out), and the host gives
them to `Typesetter::provide_font(font, bytes)`. A formula asked for before STIX is there is the error `MathErrorKind::FontLoading`, which is not cached. Providing a font changes
`font_set_version`, drops the text and math caches and nothing else; the render crate's fragment caches are the caller's to drop (the app does, `CanvasView::sync_fonts`).
`Typesetter::without_extra_fonts()` makes this path testable on the desktop. How the web app fetches the files is in `web-app.md`.

Licences are in `ATTRIBUTION.md`. Two items are open there (GelPen redistribution, Vanilla Extract).

## Limits (hostile input)

Layout never panics. Inputs are clamped: font sizes to 1..2048, NaN to 20, wrap width to at least 1
(NaN and infinity mean no wrap), text to 1 MiB, spans to 4096, invalid span ranges dropped.
Math: source at most `MAX_LATEX_BYTES` (4096), at most `MAX_TOKENS` (700) structural tokens, depth at
most `MAX_NESTING` (48), results larger than `MAX_EXTENT` are `TooLarge`.

Why the depth limit: ReX recurses once per nested group and once per prefix command, and
`\sqrt\sqrt\sqrt...` needs no braces. Measured on a 1 MiB stack (the wasm default) with the limit
lifted: a release build survived 256 chained `\sqrt` and overflowed at 600; a debug build overflowed
between 64 and 128. Nested matrices: debug overflowed at 60, release survived 120 (the most that fits
in 4 KiB of source). A stack overflow aborts and cannot be caught. `precheck` counts open groups plus
runs of consecutive commands and scripts, and 48 leaves a factor of 2.5 or more in release.
A caught panic inside ReX becomes
`MathErrorKind::Internal` on targets that unwind. The shipped web build uses `panic = "abort"`, where
`catch_unwind` catches nothing: there only `precheck` and the fuzz tests protect against a panic in ReX,
so do not rely on the catch. `tests/hostile.rs` runs token-soup, grammar and
proptest inputs and a list of pathological shapes on a 1 MiB stack in release builds (4 MiB in
debug); run `cargo test --release -p drafftink-typeset --test hostile` for the strict check, and
`TYPESET_FUZZ_ITERS=400000` for a longer fuzz.

## ReX

ReX (https://github.com/KenyC/ReX, MIT) is the only Rust TeX math layout engine with an OpenType MATH
table backend. It stays a Git dependency pinned to `aeccdba38f3fa54195c469319b65c423e17a77ae`, the
commit the old app used, with feature `ttfparser-fontparser`. Checked on 2026-11-07 with the pinned
stack: it builds natively and for `wasm32-unknown-unknown` (debug and release), and it pulls no
`pathfinder`, `cairo` or other renderer crates (those are optional features). Its dependencies
are `unicode-math` (in the same repo), `serde`, `log` and `ttf-parser` (`cargo tree -p rex`).

Consequences, and options if they start to hurt:

- `deny.toml` allows the Git source `https://github.com/KenyC/ReX` and ignores RUSTSEC-2026-0192
  (`ttf-parser` is unmaintained, no vulnerability). ReX can only read fonts through `ttf-parser`.
  Options: wait for a ReX backend on `skrifa`/`read-fonts`; vendor ReX under `third_party/` (MIT,
  about 11 kLOC with `unicode-math`) and port its font trait to `skrifa`; or switch to ReX's `fontrs-fontparser` feature (pulls the pdf-rs `font` crate and pathfinder
  types from Git; not tried).
- A Git dependency has no crates.io quarantine; the pinned commit is the control.
- ReX panics are possible in principle. The precheck and fuzz tests cover what was found; none was
  found with the limits in place.
- ReX supports a subset of LaTeX. Checked: `\text{..}` takes plain text only; `\newcommand`, `\over`,
  `\textbf` and the `cases` environment are unknown (`matrix`, `pmatrix`, `bmatrix`, `array` work). The old
  app had the same subset.

## Not done

- **IME composition.** The layout has no composition API. The editor model (`edit::text_edit`) puts the preedit text into the string it lays out, at the caret, and underlines its range with `selection_rects`; the text joins the buffer when the input method commits.
- **Right-to-left text** is laid out by Parley (the carets use visual order) but untested here and
  no bundled font covers Arabic or Hebrew.

## Differences from the old renderer

| Old | Here |
|---|---|
| Bounds estimated until the first render | Bounds from layout, available right after `text()` |
| Cache keyed by shape id, never evicted, alpha missing from the key | Keyed by content, bounded, alpha in the key |
| Cache hit skipped `normalized_coords` | Runs carry them in all cases |
| Math parsed, laid out and cmap-mapped on every frame | Cached; the cmap map is built once (100 equations: 67 ms old style, 1.3 ms cold, 15 us warm) |
| Family table twice | Once |
| Editing italic missing for Noto "Light" | One code path for display and editing |
| Math rotation possibly applied twice (render and shape) | Layout has no rotation; the caller rotates once |

## Measurements

`cargo bench -p drafftink-typeset` (release, this machine; 1000 labels of 14 to 22 px, a third with a
colour span, a quarter wrapped):

| | time |
|---|---|
| 1000 labels, empty cache | 9.3 ms (9.3 us each) |
| 1000 labels, all cached | 0.32 ms (0.32 us each) |
| 100 equations, empty cache | 1.3 ms |
| 100 equations, cached | 15 us |
| 100 equations, old style (fresh font and map each time) | 67 ms |
| `caret_at_point` + `caret_rect` | 1.4 us |

## Tests

| File | Covers |
|---|---|
| `tests/inline.rs` | inline math: baselines, bounds, wrapping, alignment, errors, source-index carets, selections, reveal, cache keys, outline |
| `tests/text.rs` | determinism, fonts, spans, line height, wrapping, alignment, outlines, hit test |
| `tests/legacy.rs` | the old fixtures: ink extent of the 12 family/weight rows and 7 equations against the old renderer's PNGs (within 1.5 px), the old estimates (0.6 to 1.25 of real), char colours, multi-line |
| `tests/math.rs` | layout, colours, fallback, typed errors with spans, limits, error caching |
| `tests/caret.rs` | point and caret round trips, selection rectangles, words, motions, drag |
| `tests/bound.rs` | Excalidraw formulas and growth |
| `tests/cache.rs` | counters, key sensitivity, LRU order, byte budget, profile counters |
| `tests/hostile.rs` | fuzz, proptest, pathological formulas, clamped numbers |
| `benches/typeset.rs` | the table above |

The old fixtures' bounds are estimates (`bounds_estimated` in the facts), so they are not a
reference. The old renderer's PNGs are: `legacy.rs` finds the ink in them and compares it with the
outline of the new layout. One row (GelPen Serif Heavy) overlaps another text in the fixture and the
three Vanilla Extract rows touch their neighbours, so those are compared horizontally only.
