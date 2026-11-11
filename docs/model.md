# Scene model (crate `drafftink-scene`, L1)

Pure data. No clock, no global randomness, no I/O. Builds for wasm32. Everything else (hit testing, rendering, edit rules, the Loro store) builds on these types.

Contents: 1 Principles, 2 Ids, 3 Element, 4 Kinds, 5 Style, 6 Z-order, 7 Document and native JSON, 8 Invariants, 9 SceneStore, 10 Old format mapping, 11 Decisions.

## 1. Principles

- Elements are **flat**. A group is not an element; it is an id shared by `group_ids` of its members. Frames, bound text and arrow bindings are ids stored on the element that owns the relation. The reverse direction is **derived** (`DerivedIndex`), never stored, never synced.
- A deleted element stays as a tombstone (`deleted: true`) so undo and sync work. `iter()` hides tombstones, `iter_all()` shows them.
- Units: world units, radians (the command layer converts degrees), y points down, rotation is clockwise about the centre of the unrotated box.
- Ids and seeds come from an injected `IdSource` (`RandomIds<R: Rng64>` in the app, `SeqIds` in tests).
- Every input is hostile. Parsing and validation return typed errors and never panic. Limits are in `limits.rs`.

## 2. Ids

| Type | Form | Notes |
|---|---|---|
| `ElementId` | 1-64 chars of `A-Z a-z 0-9 _ -` | Stable for the life of the element. New ones are 16 random chars (96 bits). Old UUIDs and Excalidraw nanoids are valid and kept on migration. Invalid ids cannot be constructed or deserialised. |
| `GroupId` | same | Not an element id; a different type so they cannot be mixed up. |
| `FileId` | 64 lowercase hex digits | SHA-256 of the encoded image bytes. Equal pictures share one blob. |

## 3. Element

```text
Element { id, kind, transform, style, index, group_ids, frame_id, locked, deleted, name, link, custom, version }
```

| Field | Type | Meaning |
|---|---|---|
| `id` | `ElementId` | Identity. |
| `kind` | `ElementKind` | Tag plus the data only that kind has (section 4). JSON: `{"type": "arrow", ...}`. |
| `transform` | `Transform` | `x, y, w, h` = the **unrotated box** (top-left, size, both >= 0). `angle` radians clockwise about the box centre (default 0). `flip_x`, `flip_y`: mirror flags, used by images only (other kinds mirror their geometry). |
| `style` | `Style` | Section 5. All fields have defaults. |
| `index` | `ZIndex` | Fractional z-order key (section 6). |
| `group_ids` | list of `GroupId` | **Innermost first, outermost last.** Click selects the outermost. Members of one group must agree on the groups around it. |
| `frame_id` | `ElementId?` | The frame this element is a child of. Frames cannot nest. |
| `locked` | bool | Locked elements refuse edits (enforced by commands). |
| `deleted` | bool | Tombstone. |
| `name` | string? | Agent-addressable label (`@name`). Not drawn. 1-256 chars, no control characters. Uniqueness is a warning in the validator; the stricter command rules (64 chars, no leading `@` or `$`) live in `edit`. |
| `link` | string? | Hyperlink, at most 2048 bytes. |
| `custom` | map string to JSON? | Free data for the host app, at most 256 keys. |
| `version` | u64 | Local change counter. The store raises it by exactly one per reported change. Renderers key caches by `(id, version)`. Not a sync clock, not guaranteed equal across stores. 0 = never stored. |

Linear and freehand elements: the box is the bounding box of the points. `points` are in the **local frame whose origin is the top-left of that box**, so the smallest x and y are 0 and the largest are `w` and `h`. This differs from Excalidraw (where `x, y` is the first point and points may be negative); `Element::refit_points()` restores the invariant after any point edit and keeps every point's world position, also when the element is rotated; it returns `Result<(), RefitError>` and changes nothing on an error. `Element::linear(id, LinearKind, index, world_points)` builds an element from world points. The geometry of a `Transform` (`frame()`, `obox()`, `local_to_world()`) is `geom::ElementFrame`; the model holds no second copy of the rotation math.

## 4. Kinds

| Kind | Data | Notes |
|---|---|---|
| `rectangle`, `diamond`, `ellipse` | none | Radius from `style.corner_radius`. |
| `line`, `arrow` | `LinearData` | Same data; `closed` is for lines only. |
| `freehand` | `FreehandData` | |
| `text` | `TextData` | |
| `math` | `MathData` | |
| `image` | `ImageData` | |
| `frame` | `FrameData` | Angle 0, no `frame_id`. |

`LinearData`: `points` (>= 2, or >= 3 when `closed`); `path`: `sharp` / `round` / `elbow`; `start_binding`, `end_binding`: `{target, mode: inside|orbit, fixed_point: [rx, ry]}` (ratios of the target's unrotated box; stored on the arrow only; targets must be rectangle, diamond, ellipse, text, image or frame); `start_head`, `end_head`: `none arrow bar circle circle_outline triangle triangle_outline diamond diamond_outline cardinality_one cardinality_many cardinality_one_or_many cardinality_exactly_one cardinality_zero_or_one cardinality_zero_or_many`; `head_size` (None = from stroke width; the old app used 15); elbow only: `fixed_segments` `[{index, start, end}]` (sorted, `index` is the segment's end point, 1..len-1, local frame), `start_is_special`, `end_is_special`; `closed`.

`FreehandData`: `points` (>= 1), `pressures` (empty or one per point, 0..1), `pressure_mode` `uniform|simulated|device`, `brush` `normal|calligraphy`, `highlighter`, `closed`.

`TextData`: `content` (source of truth, `\n` for breaks), `runs` `[{start, end, color?, bold?, italic?}]` (per-character style: a colour, the heavy face, the italic face; a run with none of them is the same as no run; **ranges count Unicode scalar values**, sorted, non-overlapping; bold uses the family's heavy face and italic its italic face, or a synthetic slant where it has none; in the CRDT a run is the Loro marks `color`, `bold` and `italic` on its characters, so they move with the text), `font` (`family` `gelpen|noto_sans|gelpen_serif|vanilla_extract`, `size`, `weight` `light|regular|heavy`, `align` `left|center|right`, `vertical_align` `top|middle|bottom`, `line_height` default 1.25, `inline_math` `auto|off`, default `auto`, see Inline math below), `auto_resize` (true: box follows content; false: fixed width, wraps), `container_id` (**bound text**: the shape or arrow this text labels; the only authoritative side), `label_position` (arrow label, 0..1 by arc length). The text box size in `transform` is measured by `typeset`; the model only stores it.

`MathData`: `latex`, `font_size`.

**Inline math** (a formula inside a text). The text stays plain: `content` is the source, `$E = mc^2$` with its dollar signs, and there is no span kind and no second list next to `runs`. Whether `$...$` is a formula is decided when the text is laid out, by the rules below, and `font.inline_math` (`auto`, the default and not written, or `off`) switches the rules off for one element. Why not math spans in the runs: the content is a `LoroText` (`crdt.md`), so a formula typed by two people merges character by character like any text, a colour run that covers a formula is still a plain range, an Excalidraw text carries the same string, and nothing in the schema changes except one font key.

The rules (Pandoc's `tex_math_dollars`, so prices stay prices):
- A formula is `$latex$` on one line. The opening `$` is followed by a character that is not white space. The closing `$` is the next unescaped `$`; it follows a character that is not white space and is not followed by a digit. When the next `$` does not close, the opening one is an ordinary character and the search continues after it. "it costs $5 and $10" has no formula; "Energy $E = mc^2$ here" has one.
- `\$` is a dollar sign, in the text and inside a formula. With inline math `auto` the backslash is not drawn (in `off` it is).
- Two or more `$` in a row are ordinary characters; there is no display math.
- At most 256 formulas per text. A formula longer than 4096 bytes, or one that ReX refuses, is drawn as its source in red (`#e03131`), dollar signs included; the layout never fails.

The formula is one unit for the caret, selection, Backspace and double-click. To edit its source, the editor lays the text out with `reveal` set to an index inside it (`typeset.md`). Formulas take the size and the colour of the text at the opening `$` (a colour run over it colours the formula, `\color{..}` inside it wins). The element box is measured with the formulas in it: a tall formula makes its line taller.

Export: Excalidraw has no inline math, so `text` and `originalText` are the source with its delimiters and `customData.drafftink.inline_math` is written only when the setting is `off`. SVG draws a text that has a formula as glyph outlines (no `<text>`, no font needed), with the source in `data-text`; plain text stays `<text>`. `doc.outline` and `element.query` show the source.

`ImageData`: `file_id` (None while the blob is not there), `natural_width`, `natural_height` (pixels), `crop` `{x, y, width, height}` in source pixels (None = whole picture).

`FrameData`: `name` (None shows "Frame"), `clip` (default true).

## 5. Style

| Field | Values | Default |
|---|---|---|
| `stroke_color` | colour | `#6366f1` (indigo-500) |
| `stroke_width` | 0..1000 | 2 |
| `stroke_style` | `solid dashed dotted` | solid |
| `fill_color` | colour or absent (= no fill) | absent |
| `fill_pattern` | `solid hachure zigzag cross_hatch dots dashed zigzag_line` | solid |
| `sloppiness` | `architect artist cartoonist drunk` (roughness 0, 1, 2, 3.5) | artist |
| `opacity` | 0..1 | 1 |
| `corner_radius` | >= 0 world units | 0 |
| `seed` | u32, makes the jitter reproducible | 0 (creators take it from `IdSource::seed`) |

Colours are strings: `#rrggbb`, or `#rrggbbaa` when not opaque; `#rgb` and `transparent` are accepted on input. `Color` converts to and from `peniko::Color`. Text colour is `stroke_color`. Font settings live in `TextData.font`, not in `Style`, so shapes carry no dead fields.

## 6. Z-order

`ZIndex` is a base-62 fractional index (the Rocicorp / Excalidraw scheme). It sorts as plain text, a key can always be made between two keys, and a reorder rewrites only the moved elements. `ZIndex::between(lower, upper)`, `n_between`, `rebalance(n)`; `ZIndex::repair(&mut keys)` fixes ties and inversions after a merge, keeping well-ordered keys. Keys longer than 128 characters are refused (rebalance). Order of elements is `(index, id)`: ties between concurrent inserts break by id. Tombstones keep their keys and take part in the order.

## 7. Document and native JSON

```json
{ "format": "drafftink", "version": 1, "name": "...", "background": "#ffffff",
  "elements": [ { "id": "...", "kind": {"type": "rectangle"}, "transform": {"x":0,"y":0,"w":10,"h":10}, "index": "a0" } ],
  "files": { "<sha256 hex>": { "mime": "image/png", "bytes": "<base64>" } } }
```

- `format` must be `"drafftink"`. `version` is the schema version (1). A reader refuses a **newer** version with `NewerVersion` and the text "Update the app"; the same version is read strictly (unknown fields are errors, so typos are caught).
- `elements` are in z-order, tombstones included, keys strictly increasing.
- Omitted fields take their defaults; defaults are not written. Numbers round-trip exactly (`float_roundtrip`).
- `Document::from_json` validates and refuses any document with an error-level issue. `to_json` refuses an invalid document too, so a file we write can be read back (the old app wrote NaN as `null` and could not reload).
- `scene::json_schema()` returns the JSON Schema (schemars) for agents and tools.
- Not in the file: selection, camera, tabs, undo history, derived indexes. Those belong to the session.

A golden file `crates/scene/tests/golden/native_v1.json` fixes the format; a change that alters it must bump the version or be a bug.

## 8. Invariants

`validate_element(&Element)` checks one element: finite numbers and ranges, point counts and the point-box rule, pressures, fixed segments, text runs, names, group depth and duplicates, self references, frames (no rotation, no nesting), crops. `element_errors` returns all of them.

`validate_document(&Document)` and `validate_scene(&impl SceneRead)` add the relations and return a `Report` of `Issue`s (`Severity::Error` or `Warning`):

| Rule | Severity |
|---|---|
| duplicate ids; z keys not strictly increasing (file) | error |
| `frame_id` names no element, or a non-frame | error |
| frame deleted but child live; child above its frame in z | warning |
| bound text: container missing; container cannot hold a label (labels: rectangle, diamond, ellipse, arrow); two live labels on one container | error |
| container deleted but label live; label below its container | warning |
| binding target missing; target not bindable | error |
| binding target deleted | warning |
| group stacks inconsistent (members of a group disagree about the groups around it) | error |
| group with a single live member | warning |
| duplicate `name` among live elements | warning |
| image blob missing from `files` | warning |
| blob bytes do not hash to the id; bad media type; too large | error |
| bad document name; schema version 0 or unknown; too many elements or blobs | error |

A reference to an id that never existed is an error. A reference to a tombstone is a warning, because concurrent edits make it and the owner of the reference resolves it as "not attached". In a live store (`validate_scene`) z-key ties are warnings.

## 9. SceneStore

`SceneRead`: `get`, `iter` (live, back to front), `iter_all`, `len`, `len_all`, `meta`, `file`, `files`, `top_z`, `bottom_z`, `z_neighbors`.
`SceneStore: SceneRead`: `begin() -> Tx`, `revision()`, `derived()`, `subscribe(callback)`, `unsubscribe`.
`SceneTx: SceneRead`: `insert`, `update(id, |e| ...)`, `remove`, `set_name`, `set_background`, `put_file`, `remove_file`, `commit() -> ChangeSet`, `rollback()`; dropping a transaction rolls it back.

Guarantees (tested by `conformance`): reads inside a transaction see its writes; invalid elements and duplicate ids are refused and leave the store unchanged; a commit reports the **net** change per element (`Added`/`Updated`/`Removed` and the `Fields` that differ: `position size angle flip kind style index groups frame locked deleted name link custom`); a commit with no net change is a no-op (no revision, no notification, no version bump); every reported change raises the element's `version` by one; subscribers run once per commit after the store is consistent.

`DerivedIndex` (kept current by the store from each `ChangeSet`, equal to a fresh rebuild): `bound_arrows(target)`, `group_members(group)`, `group_len`, `is_group` (>= 2 live members), `frame_children(frame)`, `label_of(container)`. It indexes live elements only.

`MemoryStore` is the in-memory implementation. `conformance::run(make)` (feature `conformance`) is the suite the Loro store must pass. `snapshot(&store) -> Document` and `load_document(&mut store, &doc)` move whole documents.

## 10. Old format mapping

Old = `docs/formats.md` section 1 (commit `8ce40ab`). The migration itself belongs to `io`; `tests/old_format.rs` holds a small converter that follows this table and runs on every legacy fixture, so each field below has a tested home.

| Old | New | Note |
|---|---|---|
| `CanvasDocument.id` | none | Not kept. |
| `.name` | `Document.name` | |
| `.shapes` + `.z_order` | `elements` in z-order, `index` | Only shapes listed in `z_order` were drawn; the rest are dropped. Keys from `ZIndex::rebalance`. File key order is random, so sort by `z_order`. |
| (no version) | `format`, `version` | Detected structurally by `io`. |
| (no background) | `Document.background` | White. |
| `SerializableColor {r,g,b,a}` | `Color` | Hex string. |
| `ShapeStyle.stroke_color` | `style.stroke_color` | |
| `.stroke_width` | `style.stroke_width` | |
| `.fill_color` (null = none) | `style.fill_color` | |
| `.fill_pattern` Solid, Hachure, ZigZag, CrossHatch, Dots, Dashed, ZigZagLine | `style.fill_pattern` | Same seven. |
| `.sloppiness` Architect, Artist, Cartoonist, Drunk | `style.sloppiness` | Same four. |
| `.seed` (absent: random) | `style.seed` | `io` takes it from `IdSource::seed` when absent. |
| `.opacity` | `style.opacity` | |
| `Rectangle.position, width, height` | `transform.x, y, w, h` | Negative sizes: normalise the box. |
| `.corner_radius` | `style.corner_radius` | |
| `.rotation` | `transform.angle` | Radians, clockwise, about the box centre: unchanged. |
| `Ellipse.center, radius_x, radius_y` | box `(cx-rx, cy-ry, 2rx, 2ry)` | |
| `Line.start, intermediate_points, end` | `LinearData.points` (local frame) | `Element::linear` from the world points. |
| `Line.path_style` Direct, Flowing, Angular | `path` sharp, round, elbow | Angular was never stored as points; `edit` routes it. |
| `.stroke_style` | `style.stroke_style` | |
| `Line.closed` | `LinearData.closed` | A closed line needs 3 points; with fewer, `closed` is dropped. |
| `Arrow.*` as Line | `ElementKind::Arrow` | |
| `Arrow.head_size` | `head_size = Some(n)`, `end_head = arrow` | The old arrow had one open head at the end. |
| `Freehand.points` | `FreehandData.points` | Local frame. |
| `.pressures` (empty = uniform) | `pressures`, `pressure_mode` uniform / device | Length mismatch: dropped. |
| `.closed` | `closed` | |
| `Text.position` | `transform.x, y` | Size measured by `typeset` (not stored in the old file). |
| `.content` | `content` | |
| `.font_size` | `font.size` | |
| `.font_family` GelPen, NotoSans, GelPenSerif, VanillaExtract | `font.family` | The old app has these four. The new model adds `plex_sans` (IBM Plex Sans); old documents never name it. |
| `.font_weight` Light, Regular, Heavy | `font.weight` | |
| `.char_colors` (per scalar value, null = default, any length) | `runs` | `TextData::runs_from_char_colors` merges, pads and truncates. |
| `Math.position` (baseline origin) | `transform.x, y` | `io` converts the baseline to the box top once the formula is laid out. |
| `.latex`, `.font_size` | `MathData.latex`, `font_size` | |
| `Image.position, width, height` | `transform` | |
| `.source_width, source_height` | `natural_width, natural_height` | |
| `.format` Png, Jpeg, WebP | `FileEntry.mime` | Informational in the old app; `io` should sniff the bytes. |
| `.data_base64` | `Document.files[hash].bytes` | Embedded once per content hash. |
| `Image.style` | `style` | Stroke is a border. |
| `Group.children` (by value, absolute coordinates) | children become elements; `group_ids` | New `GroupId` per group, appended after the child's own groups (innermost first). A group with fewer than two children gets no id. |
| `Group.rotation` | none | Cosmetic in the old app; `io` bakes it into the children's `angle` and positions. |
| `Group.style` | none | Throwaway. |
| `Text/Math/Group cached_size` (not serialized) | `transform.w, h` | Measured. |

Loro schema fields (`formats.md` section 2.3) are covered by the same rows (`has_fill` = `fill_color` present, `intermediate_points`, `content` for math = `latex`).

Excalidraw import (for `io`, not tested here): `angle` -> `transform.angle`; `strokeStyle` -> `style.stroke_style`; `roughness` -> `sloppiness`; `fillStyle` -> `fill_pattern`; `opacity/100` -> `style.opacity`; `roundness` -> `corner_radius` or `path round`; `groupIds` -> `group_ids`; `frameId` -> `frame_id`; `containerId` -> `TextData.container_id`; `boundElements` -> derived, not imported; `startBinding/endBinding {elementId, fixedPoint, mode}` -> `Binding` (`skip` -> none); `startArrowhead/endArrowhead` -> `start_head/end_head`; `elbowed` -> `path elbow`; `fixedSegments`, `startIsSpecial`, `endIsSpecial` -> same names; `index` -> `index`; `isDeleted` -> `deleted`; `locked`, `link`, `customData` -> same; `fontFamily`, `textAlign`, `verticalAlign`, `lineHeight`, `autoResize`, `labelPosition` -> `TextData`; `crop`, `fileId`, `scale` -> `ImageData`, `flip_x/y`; `name` of a frame -> `FrameData.name`; `version/versionNonce/updated/created` -> dropped.

## 11. Decisions and deviations

- **Font settings are in `TextData.font`**, not in `Style`. The brief listed them under style; keeping them on text avoids dead fields on every shape.
- **String ids** (not UUIDs or numbers) so foreign ids survive migration.
- **Point box convention** (origin at the bounding-box corner) differs from Excalidraw; see section 3. It keeps `transform` meaning the same for every kind.
- **`Head::None` is a variant** instead of `Option<Head>`, so JSON has one spelling.
- **Tombstones are part of the model** (catalog: delete is soft). Hard `remove` exists for undoing a creation and for garbage collection.
- **Duplicate names are a warning**, not an error: merges create them.
- **`SceneStore` is not object-safe** (it uses `impl Iterator` and generic `update`). Consumers are generic over `S: SceneStore`.
