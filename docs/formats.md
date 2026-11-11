# Legacy (old app) file formats

Exact description of the formats written and read by the OLD drafftink app
(`/home/ubuntu/git/drafft-ink`, commit `8ce40ab`, workspace version 0.1.1). The
rewrite must load old documents, so this file records what the old code does,
including its bugs. Nothing here is a design proposal.

Conventions:

- Citations are `path:line` relative to `crates/` in the old repo, taken from the
  checked-out source at `8ce40ab` (not from the research notes).
- "Probe" means I built a throwaway crate against the old `drafftink-core`
  (`CARGO_TARGET_DIR=/tmp/old-target-doc`) and ran `from_json` / `from_excalidraw`
  / `to_json` on crafted inputs. Probe results are stated as facts and were observed,
  not derived.
- The shipped sample document is `drafftink-app/assets/intro.json` (54,882 bytes).
  It is the best real-world old file and is used as evidence below.

Contents: 1 Native JSON, 2 CRDT and collaboration, 3 Local storage, 4 Tabs,
5 Clipboard and export, 6 Excalidraw import, 7 Mermaid import, 8 Gotchas.

---

## 1. Native JSON document format

### 1.1 Where it is written and read

The format is the serde derive of `CanvasDocument`, pretty-printed
(`serde_json::to_string_pretty`, 2-space indent): `drafftink-core/src/canvas.rs:254-261`.
There is **no wrapper, no version field, no magic, no metadata, no checksum**.
The same JSON is used for: native file save/open, web download/upload, IndexedDB
(as a JS object, see 3.3), PNG-embedded scenes (5.4), and `intro.json`.

| Path | File type / extension | Code |
|---|---|---|
| Native save (menu "Save Local", "Save Local As...", Ctrl+S; `UiAction::DownloadDocument` also routes here but is not in the native menu) | `rfd` save dialog, title "Save Document", default file name `"{document.name}.json"`, filter "DrafftInk Document" = `json`. Written with `std::fs::write`, UTF-8, no BOM, no trailing newline guaranteed beyond serde output. The dialog is shown every time (no remembered path). | `drafftink-app/src/app.rs:42-64`, `app.rs:4784-4789`, `app.rs:2698-2705`, `app.rs:2776-2788`, menu `ui.rs:1896-1903` |
| Native open (Ctrl+O; the menu "Open" shows an in-app name list that is empty on native until a name was saved in the session, and picking a name just runs this same dialog, `app.rs:144-147`) | `rfd` open dialog, title "Open Document", filters "DrafftInk Document" = `json` and "Excalidraw" = `excalidraw`. If the extension (lowercased) is `excalidraw` the file goes through `CanvasDocument::from_excalidraw` (section 6), everything else through `CanvasDocument::from_json`. | `app.rs:68-103`, `app.rs:4790-4793` |
| Web download ("Download JSON" menu) | Blob `application/json`, `<a download="{name}.json">`. | `app.rs:451-457`, `app.rs:527-557`, `ui.rs:1934-1937` |
| Web upload ("Import JSON/Excalidraw" menu) | Hidden `<input type=file accept=".json,.excalidraw">`; `.excalidraw` (case-insensitive suffix) -> `from_excalidraw`, otherwise `from_json`. | `app.rs:653-695`, `ui.rs:1938-1941` |
| Web Ctrl+S / "Save Local" | Not a file: writes IndexedDB (3.3). | `app.rs:332-354` |
| Web Ctrl+O / "Open" | Not a file picker: Ctrl+O calls `load_document_async` which loads IndexedDB key `__last__`; "Open" opens an in-app list of IndexedDB keys. | `app.rs:4795-4798`, `app.rs:373-397`, `app.rs:2710-2717` |
| Drag and drop | Only images (`png`, `jpg`, `jpeg`, `webp`). A dropped PNG that contains an embedded scene (5.4) is loaded as a document. `.json` / `.excalidraw` drops are not handled. | native `app.rs:5375-5440`, web `app.rs:970-995`, `app.rs:1028-1055` |

Loading a document replaces the active tab's live `canvas.document` wholesale,
clears the selection, does not push undo, does not reset the camera (except the
PNG-drop path and "Show Intro", which call `camera.reset()`), and does not
broadcast to a collaboration room: `app.rs:2289-2293`, `app.rs:5387-5392`,
`app.rs:2855-2865`. The active tab's name is not changed (section 4).

Parse failures are only logged (`log::error!`); the user sees nothing:
`app.rs:96`, `app.rs:690-693`.

### 1.2 Top level: `CanvasDocument`

Source: `drafftink-core/src/canvas.rs:26-43`. serde derive, field order as listed.

| Field | JSON type | Rust type | Required | Notes |
|---|---|---|---|---|
| `id` | string | `String` | **yes** | Free-form. `Uuid::new_v4().to_string()` for new docs (`canvas.rs:55`); `"intro"` in `intro.json`. On web, "Save Local As" temporarily replaces it with the save name. |
| `name` | string | `String` | **yes** | `"Untitled"` for new docs (`canvas.rs:56`). Used as the default file name and as the IndexedDB key for web Ctrl+S. |
| `shapes` | object | `HashMap<Uuid, Shape>` | **yes** | Key is the hyphenated lowercase UUID string. Iteration order of the writer is random, so key order in files is not stable. |
| `z_order` | array of string | `Vec<Uuid>` | **yes** | Back to front. Only these ids are drawn (`shapes_ordered`, `canvas.rs:162-164`). |
| (undo/redo stacks) | not serialized | | | `#[serde(skip)]`, `canvas.rs:38-42`. |

Observed by probe:

- Unknown extra top-level keys (`version`, `camera`, ...) are **ignored** (no `deny_unknown_fields` anywhere in `drafftink-core`).
- `id`, `name`, `shapes`, `z_order` are all required; a file missing any of them fails to parse.
- A `z_order` entry with no matching `shapes` key parses fine and is skipped when drawing. A `shapes` entry missing from `z_order` parses fine and is never drawn, but still counts in `bounds()` (`canvas.rs:203-213`, which iterates `shapes.values()`) and `len()`.
- The map key and the inner `id` are not cross-checked. All lookups use the map key / `z_order`; `Shape::id()` returns the inner id.
- `Uuid` accepts hyphenated, simple (32 hex), braced and URN forms and either case (probe: 32-hex simple form parsed; the string `"abc"` failed with `UUID parsing failed`). The writer always emits lowercase hyphenated.
- Numbers: `f64` fields accept JSON integers and floats. `u8` (color channels) and `u32` (`seed`, `source_width`, `source_height`) reject floats such as `255.0` or `1.0` and out-of-range values (probe: `255.0`, `-1`, `4294967296` all failed). A `NaN`/`inf` coordinate is written by serde_json as `null` and then **fails to parse** on reload (`null` is not an `f64`); such files cannot round-trip.
- Group children are **not** entries of `shapes` / `z_order`. Grouping removes the members from the top level and nests them in `Group.children` (`canvas.rs:343-355`). Ungrouping reinserts them (`canvas.rs:360-388`).

### 1.3 Shape enum tagging

`Shape` is an externally tagged serde enum with **no rename attributes**:
`drafftink-core/src/shapes/mod.rs:319-330`. Every shape is a single-key object
whose key is the PascalCase variant name:

```json
{ "Rectangle": { ...fields... } }
```

Variants (exact strings): `Rectangle`, `Ellipse`, `Line`, `Arrow`, `Freehand`,
`Text`, `Group`, `Image`, `Math`. Anything else fails parsing (probe: `Diamond`
-> `unknown variant`). There is no `type` field; `{"type":"Rectangle"}` fails.
A file that contains one bad shape fails as a whole (serde does not skip).

### 1.4 Shared types

**`SerializableColor`** (`shapes/mod.rs:29-35`): object `{"r":u8,"g":u8,"b":u8,"a":u8}`,
all four required, integers 0-255. sRGB, straight (non-premultiplied) alpha. No
hex-string form (probe: `"#000000"` failed).

**`ShapeStyle`** (`shapes/mod.rs:166-185`), present in every shape variant as `style`:

| Field | JSON | Required | Default when missing | Notes |
|---|---|---|---|---|
| `stroke_color` | `SerializableColor` | **yes** | none | |
| `stroke_width` | number | **yes** | none | World units. Default for new shapes 2.0. |
| `fill_color` | `SerializableColor` or `null` | no (Option) | `null` | Missing key and `null` are equivalent. `null` = no fill. A fill of `a:0` is possible (Excalidraw "transparent" is mapped to `None` instead). |
| `fill_pattern` | string enum | no (`serde(default)`, `mod.rs:175`) | `"Solid"` | See 1.5. |
| `sloppiness` | string enum | **yes** (no default attribute) | none | Files lacking it fail. See 1.5. |
| `seed` | u32 | no (`serde(default = "generate_seed")`, `mod.rs:180`) | a fresh pseudo-random value per missing field | Seed for the hand-drawn jitter. Generator: process-wide counter, splitmix-like mix (`mod.rs:193-211`); not reproducible across runs. |
| `opacity` | number | no (`serde(default = "default_opacity")`, `mod.rs:183`) | `1.0` | 0..1. Applied by multiplying alpha of stroke and fill (`mod.rs:220-240`). `null` fails. |

Default style for new shapes (`mod.rs:253-265`): stroke black, width 2.0, no fill,
`Solid`, `Artist`, fresh seed, opacity 1.0.

### 1.5 Enums (all unit variants, serialized as the variant name string)

None of these enums has `#[repr]` or numeric serde; the JSON form is always the
name string. An unknown name, a number, or `null` fails the whole parse (probe:
`"Wild"` for sloppiness, `"Gradient"` for fill pattern, `1` for sloppiness, `"Gif"`
for image format, `"Arial"` for font family all failed).

| Enum | Source | Variants (JSON string, then CRDT int code, section 2) | Default |
|---|---|---|---|
| `Sloppiness` | `shapes/mod.rs:75-86` | `"Architect"` 0 (roughness 0), `"Artist"` 1 (1.0), `"Cartoonist"` 2 (2.0), `"Drunk"` 3 (3.5). `roughness()` at `mod.rs:90-97`. | `Artist` |
| `FillPattern` | `shapes/mod.rs:111-128` | `"Solid"` 0, `"Hachure"` 1, `"ZigZag"` 2, `"CrossHatch"` 3, `"Dots"` 4, `"Dashed"` 5, `"ZigZagLine"` 6 | `Solid` |
| `StrokeStyle` | `shapes/mod.rs:146-152` | `"Solid"` 0, `"Dashed"` 1, `"Dotted"` 2 | `Solid` |
| `PathStyle` | `shapes/line.rs:9-18` | `"Direct"` 0 (straight segments), `"Flowing"` 1 (Catmull-Rom spline, tension 0.5, converted to cubic Beziers: `line.rs:188-211`), `"Angular"` 2 (elbow: straight segments through the stored points; when `intermediate_points` is empty the corner points are **not stored** but computed at render time by an A* router on a 20-unit grid, `line.rs:164-172`, `elbow.rs`) | `Direct` |
| `FontFamily` | `shapes/text.rs:10-21` | `"GelPen"` 0, `"NotoSans"` 1, `"GelPenSerif"` 2, `"VanillaExtract"` 3. Renderer names: "GelPen", "Noto Sans", "GelPen Serif", "Vanilla Extract" (`text.rs:25-32`). | `GelPen` |
| `FontWeight` | `shapes/text.rs:56-65` | `"Light"` 0, `"Regular"` 1, `"Heavy"` 2 | `Regular` |
| `ImageFormat` | `shapes/image.rs:9-16` | `"Png"` 0, `"Jpeg"` 1, `"WebP"` 2. No default (field is required). | none |

`Point` is `kurbo::Point` with the `serde` feature: object `{"x":f64,"y":f64}`
(array form `[x,y]` also parsed by probe, but the writer never emits it).

### 1.6 Per-shape fields

Field order is the serde struct order (what the writer emits). "Req" = parse
fails when absent. Rotation is **radians**, positive = clockwise on screen
(y-down), applied around the center of `bounds()` at render time
(`drafftink-render/src/vello_impl.rs:2392-2404`). Only Rectangle, Ellipse, Text,
Image, Math and Group have a rotation field (`shapes/mod.rs:508-518`).

**Rectangle** (`shapes/rectangle.rs:9-25`)

| Field | Type | Req | Default | Notes |
|---|---|---|---|---|
| `id` | uuid string | yes | | |
| `position` | Point | yes | | top-left corner |
| `width`, `height` | f64 | yes | | |
| `corner_radius` | f64 | yes | | absolute world units, 0 = sharp; passed to kurbo `RoundedRect` (`rectangle.rs:119-126`) |
| `rotation` | f64 | no | 0.0 | |
| `style` | ShapeStyle | yes | | |

**Ellipse** (`shapes/ellipse.rs:9-23`): `id`, `center` Point, `radius_x` f64,
`radius_y` f64 (all required), `rotation` (optional, 0.0), `style` (required).

**Line** (`shapes/line.rs:21-42`)

| Field | Type | Req | Default |
|---|---|---|---|
| `id` | uuid | yes | |
| `start`, `end` | Point | yes | |
| `intermediate_points` | [Point] | no | `[]` |
| `path_style` | PathStyle | no | `Direct` |
| `stroke_style` | StrokeStyle | no | `Solid` |
| `closed` | bool | no | `false` |
| `style` | ShapeStyle | yes | |

The polyline is `start`, then `intermediate_points`, then `end` (`all_points`,
`line.rs:103-108`). `closed` only adds a closing segment and, if true, draws it as
a filled path (`line.rs:213-215`, `vello_impl.rs:2421-2430`). The last point is
not repeated for a closed line. No rotation field.

**Arrow** (`shapes/arrow.rs:10-30`): same as Line minus `closed`, plus
`head_size` f64 (**required**, 15.0 for new arrows). One arrowhead at `end`
only; no start head, no head style, no bindings.

**Freehand** (`shapes/freehand.rs:9-22`): `id`, `points` [Point] (req),
`pressures` [f64] (optional, default `[]`; when non-empty it is parallel to
`points`, 0..1; empty means uniform), `closed` bool (optional, `false`), `style`
(req). No rotation field.

**Text** (`shapes/text.rs:84-111`)

| Field | Type | Req | Default | Notes |
|---|---|---|---|---|
| `id` | uuid | yes | | |
| `position` | Point | yes | | top-left of the text box |
| `content` | string | yes | | may contain `\n` |
| `font_size` | f64 | yes | | pixels; default for new text 20.0 |
| `font_family` | FontFamily | yes | | |
| `font_weight` | FontWeight | yes | | |
| `rotation` | f64 | no | 0.0 | |
| `style` | ShapeStyle | yes | | `stroke_color` is the text color |
| `char_colors` | [Color or null] | no | `[]` | see below |
| (cached layout size) | not serialized | | | `#[serde(skip)]`, `text.rs:109-110`. Bounds are therefore recomputed by the renderer (fallback: approximate width/height, `text.rs:332-345`). |

`char_colors` encoding: JSON array with one element per **Unicode scalar value**
of `content` (`content.chars().count()`, `text.rs:137,147`): `null` = use
`style.stroke_color`, otherwise a `SerializableColor` object. The writer keeps the
length equal to the character count (`text.rs:174-176,218-220`), but a file may
hold any length (probe: shorter-than-content array parsed). A loader must treat
missing entries as `null` and ignore extras. Width/height of the text box are
not stored.

**Group** (`shapes/group.rs:10-20`): `id` (req), `children` [Shape] (req;
recursive, any variant including `Group`), `rotation` f64 (optional, 0.0),
`style` (**required** although unused; probe: missing `style` fails). Children
are stored in absolute world coordinates, back to front. `Group.transform`
transforms children and leaves `rotation` unchanged (`group.rs:147-152`); the
group rotation field is set by the rotate handle (`selection.rs:560-577`) and
applied at render time about the center of the group's bounds. Group bounds are
the union of child bounds, `Rect::ZERO` when empty (`group.rs:107-117`).

**Image** (`shapes/image.rs:65-91`)

| Field | Type | Req | Notes |
|---|---|---|---|
| `id` | uuid | yes | |
| `position` | Point | yes | top-left |
| `width`, `height` | f64 | yes | display size in world units |
| `source_width`, `source_height` | u32 | yes | pixel size of the encoded image; integers only |
| `format` | ImageFormat | yes | informational; see below |
| `data_base64` | string | yes | **standard base64 alphabet with `=` padding** (`base64::engine::general_purpose::STANDARD`, `image.rs:106,116,177`). No `data:` prefix, no MIME. Raw bytes of the original PNG/JPEG/WebP file. |
| `rotation` | f64 | no | 0.0 |
| `style` | ShapeStyle | yes | stroke is an optional border |

`format` is set from the file extension (native drop, `app.rs:5406-5411`) or the
clipboard MIME type (`app.rs:790-795`, unknown MIME -> `Png`) and is **not
verified against the bytes**. The renderer ignores it and sniffs the bytes with
`image::load_from_memory` (`vello_impl.rs:1031`); on decode failure it draws a
placeholder (`vello_impl.rs:1045`). A loader should do the same. Pasted/dropped
images are scaled to fit 800x800 keeping aspect (`image.rs:158-173`,
`app.rs:232-242`) by changing only `width`/`height`; `source_*` keep the pixel
size. Images are embedded per shape; identical images are not deduplicated.

**Math** (`shapes/math.rs:10-26`): `id`, `position` Point (baseline origin),
`latex` string, `font_size` f64 (all required; default 20.0), `rotation`
(optional), `style` (required). Cached layout size is `serde(skip)`
(`math.rs:24-26`). Rendered with the `rex` backend (`drafftink-render/src/rex_backend.rs`).

Skipped (not serialized) fields in the whole model: `CanvasDocument.undo_stack`,
`redo_stack`, `Text.cached_size`, `Math.cached_size`.

### 1.7 Real examples

Produced by running the old `to_json()` in the probe, then compacted (one shape
per line; `seed` values are random). The `style` object is abbreviated to `"style":{...}`
only in the first example; the real output always has all seven keys:

```
"style":{"stroke_color":{"r":0,"g":0,"b":0,"a":255},"stroke_width":2.0,"fill_color":null,"fill_pattern":"Solid","sloppiness":"Artist","seed":1275600319,"opacity":1.0}
```

Document wrapper:

```json
{"id":"doc-1","name":"Untitled","shapes":{"<uuid>":{"Rectangle":{...}}, "...": {}},"z_order":["<uuid>","..."]}
```

Rectangle (with fill and corner radius):

```json
{"Rectangle":{"id":"44093825-1401-4f08-9fd9-84d6a593a5d2","position":{"x":10.0,"y":20.0},"width":100.0,"height":50.0,"corner_radius":8.0,"rotation":0.0,"style":{"stroke_color":{"r":0,"g":0,"b":0,"a":255},"stroke_width":2.0,"fill_color":{"r":255,"g":0,"b":0,"a":128},"fill_pattern":"Solid","sloppiness":"Artist","seed":2462723854,"opacity":1.0}}}
```

Ellipse:

```json
{"Ellipse":{"id":"f693bd4e-a9f8-4e6f-8f2a-ee07a0d6c877","center":{"x":50.0,"y":50.0},"radius_x":30.0,"radius_y":20.0,"rotation":0.0,"style":{...}}}
```

Line (one intermediate point):

```json
{"Line":{"id":"de003b29-92bd-4495-a159-df1f9f093c2f","start":{"x":0.0,"y":0.0},"end":{"x":10.0,"y":10.0},"intermediate_points":[{"x":5.0,"y":0.0}],"path_style":"Direct","stroke_style":"Solid","closed":false,"style":{...}}}
```

Arrow:

```json
{"Arrow":{"id":"17e188a1-f392-4d76-a81c-74291a281a82","start":{"x":0.0,"y":0.0},"end":{"x":10.0,"y":10.0},"intermediate_points":[],"path_style":"Direct","stroke_style":"Solid","head_size":15.0,"style":{...}}}
```

Freehand (with pressures):

```json
{"Freehand":{"id":"c7622f1a-e4aa-44c4-b6c1-22953111c0ee","points":[{"x":0.0,"y":0.0},{"x":1.0,"y":2.0}],"pressures":[0.5,0.7],"closed":false,"style":{...}}}
```

Text (second character colored):

```json
{"Text":{"id":"b1a8df87-191e-4823-8be8-8a1d99b96259","position":{"x":1.0,"y":2.0},"content":"ab","font_size":20.0,"font_family":"GelPen","font_weight":"Regular","rotation":0.0,"style":{...},"char_colors":[null,{"r":1,"g":2,"b":3,"a":255}]}}
```

Math:

```json
{"Math":{"id":"eda453eb-0b81-406d-b9f1-8b9e3fb4af9c","position":{"x":1.0,"y":2.0},"latex":"x^2","font_size":20.0,"rotation":0.0,"style":{...}}}
```

Image (7 bytes of PNG magic + 3, not a decodable image; real files carry the full file):

```json
{"Image":{"id":"9bd4ab57-961e-4b23-bcc0-44e7ff3903c7","position":{"x":0.0,"y":0.0},"width":4.0,"height":5.0,"source_width":4,"source_height":5,"format":"Png","data_base64":"iVBORwECAw==","rotation":0.0,"style":{...}}}
```

Group (one child; the group's own `style` is a throwaway default):

```json
{"Group":{"id":"47361521-2b24-4929-9f95-684705c2d952","children":[{"Rectangle":{"id":"e410d3d2-5e25-4ba1-8111-68d53c6861ac","position":{"x":0.0,"y":0.0},"width":1.0,"height":1.0,"corner_radius":0.0,"rotation":0.0,"style":{...}}}],"rotation":0.0,"style":{...}}}
```

Evidence of older file revisions in the wild: in `intro.json` (document
`id:"intro"`, `name:"intro"`, 49 top-level shapes, 54 including 5 group children;
counts by variant: Text 29, Arrow 11, Line 4, Freehand 3, Rectangle 3, Group 2,
Math 1, Ellipse 1; no Image) the `Line` objects lack `stroke_style` and `closed`,
the `Arrow` objects lack `stroke_style`, the `Freehand` objects lack `closed`.
All parse only because those fields are `serde(default)`. Fonts used: GelPen,
GelPenSerif, VanillaExtract. Fill patterns used: Solid, Hachure, CrossHatch.
All sloppiness values: Artist. Group objects carry a full `style` (required).
Expect the same gaps in user files saved by earlier builds; loaders must apply the
defaults in 1.4/1.6.

### 1.8 Which old files fail to parse (quick checklist)

A document fails (whole file) if any shape, at any depth, has: a missing required
field (1.6), a missing `style`, a missing `sloppiness`/`stroke_color`/`stroke_width`,
an unknown enum string or non-string enum, an unknown shape variant, a non-UUID
`id` or map key, a float where `u8`/`u32` is expected, `null` for a non-Option
number (`opacity`, coordinates after a NaN/inf save), or a missing top-level
`id`/`name`/`shapes`/`z_order`. The list follows from the serde derives; the
per-field cases were confirmed with probes.

---

## 2. CRDT (Loro) schema and collaboration protocol

### 2.1 Role

The CRDT is used **only** for live collaboration. Documents are never persisted
in Loro form (no Loro bytes in any file, IndexedDB value or PNG). The native JSON
model is authoritative and is converted into Loro on demand. Crate: `loro = "1.10"`
(`drafftink-core/Cargo.toml`).

### 2.2 Container layout

Root containers of the `LoroDoc` (`crdt/schema.rs:9-14,59-66,317-321`; summary
comment `crdt/mod.rs:8-13`):

| Root key | Loro type | Content |
|---|---|---|
| `"name"` | `LoroText` | document name (`set_name` deletes all text then inserts, `schema.rs:324-334`) |
| `"shapes"` | `LoroMap` | key = shape UUID string (hyphenated); value = a nested `LoroMap` container holding that shape's fields (`schema.rs:86-104`) |
| `"z_order"` | `LoroList` | plain string values (UUIDs), back to front (`schema.rs:73-83`) |

The document `id` is not stored. Update of a shape = delete key + re-insert a new
map (`schema.rs:144-155`), i.e. no field-level merge: concurrent edits of the same
shape resolve at whole-shape granularity.

### 2.3 Shape map keys (`crdt/convert.rs`)

Constants: `convert.rs:13-79`. Writer: `convert.rs:117-255`. Value types: doubles
(`f64`) for geometry, integers (`i64`) for colors, enum codes, seed and image
pixel sizes, bool for `has_fill`, strings for `type`, `id`, `content`,
`data_base64`.

Common to every shape: `type` (string), `id` (string).
Style keys (written for every shape except Group, `convert.rs:258-280`):
`stroke_r`, `stroke_g`, `stroke_b`, `stroke_a` (i64), `stroke_width` (f64),
`sloppiness` (i64), `seed` (i64 of the u32), `fill_pattern` (i64), `opacity` (f64),
`has_fill` (bool), and **only when `has_fill` is true**: `fill_r`, `fill_g`,
`fill_b`, `fill_a` (i64).

| `type` string | Keys besides common + style | Container values |
|---|---|---|
| `"rectangle"` | `x`, `y` (top-left), `width`, `height`, `corner_radius`, `rotation` | none |
| `"ellipse"` | `x`, `y` = **center**, `width` = **radius_x**, `height` = **radius_y**, `rotation` (the keys are reused with radius semantics, `convert.rs:130-139`) | none |
| `"line"` | `start_x`, `start_y`, `end_x`, `end_y`, `path_style` (i64), `stroke_style` (i64), `intermediate_points` | `intermediate_points`: `LoroList` of `LoroList[x,y]` (one nested list container per point) |
| `"arrow"` | as line plus `head_size` (f64) | same |
| `"freehand"` | `points`; `pressures` only if non-empty | `points`: `LoroList` of `LoroList[x,y]`; `pressures`: `LoroList` of f64 |
| `"text"` | `x`, `y`, `content`, `font_size`, `font_family` (i64), `font_weight` (i64), `rotation`, `char_colors` | `char_colors`: `LoroList`; each entry is `LoroList[r,g,b,a]` of i64, or `Null` |
| `"group"` | `children` (no style keys, no `rotation`) | `children`: `LoroList` of `LoroMap` (recursive, each child a full shape map) |
| `"image"` | `x`, `y`, `width`, `height`, `source_width`, `source_height` (i64), `format` (i64), `data_base64`, `rotation` | none |
| `"math"` | `x`, `y`, `content` (**the LaTeX source**, not `latex`), `font_size`, `rotation` | none. The type string `"math"` is a literal, with no named constant (`convert.rs:244,295`). |

Enum integer codes: Sloppiness 0-3, PathStyle 0-2, StrokeStyle 0-2, FontFamily
0-3, FontWeight 0-2, ImageFormat 0-2, FillPattern 0-6, in the order of the table in
1.5 (`convert.rs:551-673`). The reverse mapping maps every out-of-range value to
the **last** variant (e.g. sloppiness 99 or -1 -> `Drunk`, `i64_to_font_family(9)`
-> `VanillaExtract`, image format 5 -> `WebP`, `convert.rs:560-567,577-583,593-598,610-616,627-633,643-649,663-672`).

### 2.4 Reading (`shape_from_loro`, `convert.rs:283-547`)

A shape map is read from `get_deep_value()` (`schema.rs:129-141`). Any required
value that is missing or has the wrong Loro type makes the whole shape `None`,
and callers silently drop it (`collaboration.rs:127-134`, `schema.rs:158-169`).

Required: `type` (string, one of the 9 known values; unknown -> dropped), `id`
(parseable UUID), geometry keys (`x`,`y`,`width`,`height` for rectangle, ellipse,
image; `start_*`/`end_*` for line/arrow; `x`,`y`,`content` for text/math;
`source_width`, `source_height`, `data_base64` for image), and for every
non-group shape `stroke_r/g/b/a` and `stroke_width`. Group requires `children`
as a list and `id`.

Defaults on read: `rotation` 0.0; `corner_radius` 0.0; `path_style` Direct;
`stroke_style` Solid; `head_size` 15.0; text `font_size` 20.0, `font_family`
GelPen, `font_weight` Regular; math `font_size` 20.0; image `format` Png;
`sloppiness`/`fill_pattern` via `unwrap_or_default`; `seed` = generated;
`opacity` 1.0; `has_fill` absent = no fill; `fill_r/g/b` 0 and `fill_a` 255 when
absent; missing `intermediate_points`/`points`/`pressures`/`char_colors` = empty;
`get_double` accepts `I64` too, `get_i64` accepts `Double` too (truncating cast).
Colors are cast `as u8` (wrapping).

### 2.5 What a CRDT round trip loses

| Lost | Where |
|---|---|
| `Line.closed`, `Freehand.closed` (always `false` after read) | `line.rs:59-79`, `freehand.rs:58-72`; not written by `convert.rs:140-156,175-192`. Closed polygons from Excalidraw / Mermaid become open lines. |
| `Group.rotation` and `Group.style` | not written (`convert.rs:219-228`); `Group::reconstruct` sets rotation 0 (`group.rs:34-41`) |
| Per-shape identity of unknown/unreadable shapes | dropped silently |
| `document.id`, selection, camera, undo history | never part of the CRDT |
| Order of `shapes` keys | n/a (map) |

The `id` strings of group children are preserved (each child map has its own `id`).

### 2.6 When the app writes the CRDT (important for interoperability)

`CollaborationManager::sync_to_crdt` **clears** the whole Loro doc (`z_order`
list and every `shapes` key) and re-adds every shape of the *active tab's*
document, then sets `name` (`collaboration.rs:94-111`, `schema.rs:336-362`). It is
a no-op unless collaboration is enabled. `broadcast_sync` then exports a **full
snapshot** (`ExportMode::Snapshot`, `schema.rs:294-297`) and sends it as a `sync`
message (`collaboration.rs:324-334`). Individual operations
(`add_shape`/`remove_shape`) are also applied to the CRDT directly in some
handlers (e.g. paste, cut: `app.rs:3370-3374,3417-3422`). There are no
incremental updates on the wire in practice (`export_updates` exists but is not
called by the app); every `sync` payload is a full snapshot.

### 2.7 Wire protocol (client <-> `drafftink-server`)

Transport: WebSocket, endpoint `ws://host:3030/ws` (`drafftink-server/src/main.rs:199-208`;
also `/` text index and `/health` returning `ok`; CORS permissive). **All
messages are WebSocket text frames, one JSON object per frame.** There is no
binary framing, no length prefix, no handshake beyond `join`, no authentication,
no protocol version, no heartbeat besides WebSocket ping/pong. The native client
answers pings (`sync.rs:559-562`) and ignores incoming binary frames
(`sync.rs:567`); the web client only handles string frames (`sync.rs:245-247`).

Internal tag: `"type"`, `rename_all = "snake_case"`
(`sync.rs:10-11,28-29`; identical copies in `drafftink-server/src/main.rs:37-103`).

Client -> server (`ClientMessage`, `sync.rs:11-24`):

| JSON | Meaning |
|---|---|
| `{"type":"join","room":"<id>"}` | Join (and implicitly leave the previous room). Room id is an arbitrary string, rooms are created on demand. |
| `{"type":"leave"}` | Leave current room. |
| `{"type":"sync","data":"<base64 Loro snapshot>"}` | Full document state. Base64 = standard alphabet with `=` padding, hand-rolled in `sync.rs:117-180` (decoder tolerates missing padding). |
| `{"type":"awareness","peer_id":<u64>,"cursor":{"x":..,"y":..},"user":{"name":"..","color":"#rrggbb"}}` | `AwarenessState` is `#[serde(flatten)]`; `cursor` and `user` are each omitted when `None` (`sync.rs:19-23,55-76`). `peer_id` is the client's Loro peer id (u64 as JSON number). |

Server -> client (`ServerMessage`, `sync.rs:29-53`):

| JSON | Meaning |
|---|---|
| `{"type":"joined","room":"..","peer_count":N,"initial_sync":"<base64>"}` | Sent to the joiner. `initial_sync` omitted when the room has no snapshot yet. `peer_count` includes the joiner. |
| `{"type":"peer_joined","peer_id":"<server uuid>"}` / `{"type":"peer_left","peer_id":"<server uuid>"}` | Broadcast to others. The id here is the **server-assigned connection UUID** (`main.rs:231`). |
| `{"type":"sync","from":"<server uuid>","data":"<base64>"}` | Relayed from another peer; never echoed to the sender (`main.rs:353-355`). |
| `{"type":"awareness","from":"<server uuid>","peer_id":<u64>, ...flattened state}` | Relayed. |
| `{"type":"error","message":".."}` | Sent for an unparseable client message (`main.rs:311-317`). |

Server behavior (`main.rs`): in-memory only (`DashMap` of rooms, nothing is
written to disk); each room keeps `last_sync`, the most recent `sync` payload
from anyone (`main.rs:172-177,289-298`), delivered as `initial_sync` to new
joiners; a room is deleted when its last peer leaves (`main.rs:161-170`);
broadcast channel capacity 256 (lagging receivers drop messages,
`main.rs:33,345`). A raw binary frame from a client is treated as a `sync`
payload and re-encoded to base64 (`main.rs:320-330`), which the old client never
sends. The server does not parse Loro data.

Client join sequence (`app.rs:2421-2447`, `collaboration.rs:344-368`): send
`join`; on `joined` import `initial_sync` (if present) into the local Loro doc,
rebuild the local document **from the Loro doc, replacing local shapes**
(`sync_from_crdt` clears `shapes`/`z_order`, `collaboration.rs:115-135`), then
`sync_to_crdt` + broadcast the (now identical) state. A joiner with local content
joining a non-empty room therefore loses its local content; joining an empty room
publishes the local content. On `sync` received: import, then rebuild the local
document from Loro (`app.rs:2458-2464`). Whole-document replace means remote and
local edits are last-writer-wins at document granularity in practice.

Web auto-join: URL `?room=<id>&server=<host[:port]|ws://..|wss://..>` or the same
in the hash (`#room=..&server=..`). Server resolution: explicit `ws://`/`wss://`
kept (a `/ws` suffix is added when missing); bare host -> `ws://host/ws`; no
`server` -> page origin with `ws`/`wss` by page protocol (`drafftink-app/src/web.rs:13-125`,
`app.rs:1989-2031`). Native: CLI `--server <url> --room <id>` (`main.rs:12-34`,
`app.rs:1957-1985`). The UI field default for the server URL is `"/ws"`
(`ui.rs:335`).

---

## 3. Local storage layout

### 3.1 What the app actually persists

| Item | Persisted? | Evidence |
|---|---|---|
| The document (shapes, z-order, name, id) | Web: yes, IndexedDB. Native: **no automatic persistence**; only explicit file save. | below |
| Camera (offset, zoom) | **no** | `Camera` derives serde (`camera.rs:14`) but is never serialized anywhere |
| Active tool, tool options, current style, grid style, export scale, snap flags | **no** | `UiState` is plain runtime state (`ui.rs:253-353`) |
| Tabs, tab names, library tabs | **no** | section 4 |
| Collaboration room, server URL, user name/color | **no** (room/server only from URL or CLI) | `web.rs`, `main.rs` |
| Recent-documents list | runtime `Vec<String>` (max 10); on web refreshed from IndexedDB keys; on native always starts empty | `app.rs:2742-2746`, `app.rs:424-449` |
| localStorage / sessionStorage / cookies | **never used** (`web-sys` `Storage` feature is enabled but no call exists) | grep: no `local_storage`/`localStorage` in any `.rs` or in `web/index.html` |
| Undo/redo | no | `canvas.rs:38-42` |

### 3.2 Native: no autosave, no app data directory is created

`drafftink-core/src/storage/` contains `FileStorage` (`storage/file.rs`),
`AutoSaveManager` (`storage/autosave.rs`, `DEFAULT_AUTOSAVE_INTERVAL_SECS = 30`,
`LAST_DOCUMENT_KEY = "__last_document__"`, `autosave.rs:16-19`), `MemoryStorage`
and `create_autosave_manager`. **None of them is called by the app crate**
(grep over `drafftink-app`, `drafftink-render`, `drafftink-widgets`: zero uses of
`AutoSaveManager`, `FileStorage`, `create_default_storage`,
`create_autosave_manager`). So on native there is no autosave and no
`documents/` directory written by the app. For completeness, if it were used the
location would be `dirs::data_local_dir()/drafftink/documents/<sanitized id>.json`
(`file.rs:33-41`, id chars outside `[A-Za-z0-9_-]` replaced by `_`,
`file.rs:43-56`; Linux `~/.local/share/drafftink/documents`, macOS
`~/Library/Application Support/drafftink/documents`, Windows
`%LOCALAPPDATA%\drafftink\documents`); the doc comment at `file.rs:27-31` says
`~/.drafftink/documents` and is wrong. Files there would be the same JSON as 1.2.
No legacy data from it should exist.

Native startup always loads `intro.json` into the live document
(`app.rs:1859-1866`); no previous session is restored.

### 3.3 Web: IndexedDB

Implementation: `drafftink-core/src/storage/indexeddb.rs` using `rexie` 0.6,
`serde-wasm-bindgen` 0.6.

| Item | Value |
|---|---|
| Database name | `drafftink` (`indexeddb.rs:8`) |
| Database version | `1` (`indexeddb.rs:24`) |
| Object store | `documents` (`indexeddb.rs:9,25`), created with `ObjectStore::new(name)`: no key path, no auto-increment, so keys are out-of-line |
| Key | JS string (`put(&js_val, Some(&JsValue::from_str(&id)))`, `indexeddb.rs:59-62`) |
| Value | the `CanvasDocument` serialized by `serde_wasm_bindgen::to_value` (`indexeddb.rs:56`). This is **a structured-cloneable JS value, not a JSON string**. |

How `serde_wasm_bindgen::to_value` (default `Serializer::new()`,
`serde-wasm-bindgen-0.6.5/src/ser.rs:228-240`, `lib.rs:78-80`) renders the model:

- structs -> plain JS objects with the same field names as the JSON in section 1;
- `HashMap<Uuid, Shape>` (`shapes`) -> a JS **`Map`** (not a plain object; `serialize_maps_as_objects` is false);
- `Uuid` -> string (human-readable serializer);
- enums with data (`Shape`) -> `{ "Rectangle": {...} }` object, same external tagging; unit enums -> the variant name string;
- `Option::None` -> **`undefined`** (not `null`; `serialize_missing_as_null` is false), so `fill_color` and `char_colors` entries appear as `undefined`;
- numbers -> JS numbers (u64 above 2^53 would error; none exist);
- `Vec` -> Array; `Point` -> `{x,y}`.

Reading uses `serde_wasm_bindgen::from_value`; `undefined`/`null` both load as
`None`. A reader outside Rust must treat `shapes` as `Map` (or plain object) and
missing/undefined `fill_color` as no fill. Byte-for-byte JSON conversion:
`Object.fromEntries(map)` for `shapes`, and `undefined` -> `null`.

Keys written (all in the one store):

| Key | Written by | Content |
|---|---|---|
| `__last__` | every autosave and every "Save Local"/Ctrl+S (`app.rs:345,363`) | the most recent document; loaded at startup |
| `<name>` | "Save Local" / Ctrl+S: key = `document.name`, value keeps its own `id` (`app.rs:2698-2704,4784-4788`, `app.rs:333-354`); "Save Local As" / `SaveLocalWithName`: key = chosen name, and the stored value's `id` is temporarily set to the name (`app.rs:2726-2740`) and `name` to the name (`app.rs:2728`) | a document |

Key listing for the "Open"/"Open Recent" dialogs filters out `__last__`
(`app.rs:432-433`). `LoadLocal(name)` reads key `name` (`app.rs:405-421`). Key
`__last_document__` is only defined by the unused `AutoSaveManager`; the web app
uses `__last__`.

Autosave cadence (web only, `app.rs:2564-2576`, state `app.rs:1585-1589`):
checked on each `RedrawRequested`; fires when
`shapes.len() + z_order.len()` differs from the last saved value **and** at least
5 seconds passed since the last autosave (timer starts at app init, version
starts at 0). Consequences: moving, restyling or editing text without adding or
removing a shape is never autosaved until the count changes; loading the 98-count
intro document autosaves it to `__last__` about 5 s after start without any user
edit. Autosave writes only `__last__`.

Startup on web (`app.rs:1910-1914`, `app.rs:373-397`): after init the app reads
`__last__`; if the read fails for any reason (missing key, parse failure) it
loads the embedded intro instead. The restored document replaces the live canvas
document asynchronously (`app.rs:2289-2293`); the camera is default.

### 3.4 The welcome / intro document

- Source: `drafftink-app/assets/intro.json`, embedded with
  `include_str!("../assets/intro.json")` at `app.rs:386`, `1862`, `2856`.
- Content: ordinary document JSON (section 1), `id:"intro"`, `name:"intro"`.
  A tutorial layout of text, arrows, groups, freehand strokes and one math
  equation spread over roughly x 46..1894, y 27..1025 in world units.
- When shown:
  - native: always at startup (`app.rs:1859-1866`);
  - web: at startup only when IndexedDB has no readable `__last__` (`app.rs:384-393`);
  - both: menu "Show Intro" (`ui.rs:1924-1927`, `app.rs:2855-2865`) replaces the
    live document, clears selection, resets the camera.
- It is parsed with `CanvasDocument::from_json` at runtime, so a parse failure
  silently leaves an empty document.

---

## 4. Canvas tabs

Source: `drafftink-app/src/app.rs:1531-1541` (`TabState`), `1707-1765` (operations),
`1888-1893` (initial tab), UI `ui.rs:655-723`.

```rust
struct TabState { name: String, document: CanvasDocument, camera: Camera }
```

- `AppState.tabs: Vec<TabState>`, `active_tab: usize` (`app.rs:1563-1566`).
- Initial state: exactly one tab, `name = "Canvas"`, `document = CanvasDocument::new()`
  (empty, stale: the live document is `canvas.document`), `camera = Camera::new()`.
- The active tab's live document and camera live in `AppState.canvas`; `tabs[i]`
  is a snapshot refreshed only when switching away (`snapshot_active_tab`,
  `app.rs:1709-1713`). `tabs[active].document` is therefore stale while active.
- `add_tab(name, document)`: snapshot the active tab, push, activate the new one
  with `Camera::new()` (no fit-to-content) (`app.rs:1734-1743`).
- `close_tab(i)`: ignored when only one tab remains or index is out of range;
  otherwise removes and activates a neighbour (`app.rs:1746-1765`). The close
  buttons are only drawn when there is more than one tab (`ui.rs:663`).
- `switch_to_tab` snapshots the old tab, loads the target's document and camera
  (clones), and clears the selection (`app.rs:1716-1731`).
- Tabs are created only by "+ Library" (`ui.rs:704-717`, `UiAction::LoadLibrary`,
  `app.rs:3673-3678`, `app.rs:2295-2298`). There is no "new tab" button.
- **Library tabs are ordinary tabs**: no flag, no read-only mode. Content =
  a `CanvasDocument::new()` (random id, name `"Untitled"`) containing one `Group`
  per library item laid out on a grid (section 6.5). Tab name = file stem native,
  or file name minus `.excalidrawlib` on web (a `.json` file keeps its full file
  name), `"Library"` if blank (`app.rs:110-137,600-646,1689-1705`). Item names
  from the library file are discarded.
- **Not saved anywhere**: tab list, names, per-tab camera and per-tab documents
  vanish on restart. Save/Download/autosave serialize only the live
  `canvas.document` of the active tab. Opening a file or showing the intro replaces
  the active tab's document without touching its name.
- Collaboration operates on whatever document is active in `canvas`
  (`broadcast_doc_changes`, `app.rs:1595-1636`): switching tabs while in a room
  makes the next sync overwrite the room with the other tab's content.
- Pasting between tabs goes through the in-memory clipboard (5.1).

---

## 5. Clipboard, export and import entry points

### 5.1 Internal copy / cut / paste

- Copy (Ctrl+C or `UiAction::CopyShapes`), cut (Ctrl+X): `serde_json::to_string(&Vec<Shape>)`
  of the selected top-level shapes (selection order), stored in
  `UiState.clipboard_shapes: Option<String>` (`ui.rs:298`; `app.rs:3384-3427,5061-5104`).
  Format = compact JSON array of externally tagged shapes (section 1.3), no
  wrapper. It is **memory-only**: never written to the system clipboard, lost on
  exit, not shared between windows or app instances, not versioned.
- Paste (Ctrl+V or `UiAction::PasteShapes`): parse as `Vec<Shape>`, call
  `regenerate_id()` on each top-level shape only (children of groups keep their
  ids, `shapes/mod.rs:479-492`), then `place_shapes_centered_at`: translate so the
  union of bounds is centered on the mouse position (Ctrl+V) or viewport center
  (menu) (`app.rs:1638-1684`, `app.rs:3428-3454`, `app.rs:5106-5134`). One undo step.
- Ctrl+V order on native (`app.rs:5105-5189`): (1) internal clipboard, if it
  holds parseable shapes, **always wins** over whatever is on the system
  clipboard; (2) system text: Excalidraw clipboard JSON (5.3), then Mermaid (section 7);
  (3) system image. On web (`app.rs:5191-5207`) when (1) did not paste, it starts
  two async tasks: text (Excalidraw, then Mermaid; `app.rs:908-930`) and image (`app.rs:748-840`).
- Duplicate (Ctrl+D): clone + new id + translate (20, 20) (`app.rs:5210-5225`).
- Text editing has its own clipboard path: Ctrl+C/X put plain text on the system
  clipboard (`arboard`, or `navigator.clipboard.writeText` on web), Ctrl+V inserts
  plain text (`app.rs:4563-4592,4669-4676`). Per-character colors are not copied.
  The math editor has copy/paste buttons for the LaTeX string (`ui.rs:2984-2992`).
- There is no HTML, rich-text, SVG, or custom MIME clipboard format on write.

### 5.2 System clipboard images

- Write ("Copy as PNG", Ctrl+Shift+C / Ctrl+Shift+E; menu item enabled only with a
  selection but the handler falls back to the whole document when nothing is
  selected, `ui.rs:1956-1960`, `app.rs:3756-3817`, `app.rs:4991-5058`):
  - native: `arboard::Clipboard::set_image` with **raw RGBA8** pixels
    (`ImageData{width,height,bytes}`), not a PNG (`app.rs:169-189`);
  - web: PNG blob via `ClipboardItem` `image/png` (`app.rs:482-525`), no embedded scene.
- Read (paste image): native `arboard` `get_image` -> RGBA -> re-encoded as 8-bit RGBA
  PNG with the `png` crate -> `Image` shape with `format: Png` (`app.rs:191-258`).
  Web: first clipboard item type starting with `image/`, bytes stored unmodified,
  format from MIME (`image/png|jpeg|webp`, anything else labelled `Png`), pixel size
  from an `<img>` element (`app.rs:748-875`). Both: centered at viewport center,
  limited to 800x800 (`app.rs:232-242`, `app.rs:820-829`).
- Dropped image files: same, `Image::new(...)` with bytes untouched
  (`app.rs:1028-1099`, native `5375-5440`).

### 5.3 Excalidraw clipboard (`excalidraw/clipboard`)

`CanvasDocument::shapes_from_excalidraw_clipboard(text)` (`canvas.rs:285-299`):
parse JSON; require `type == "excalidraw/clipboard"` exactly (other values,
including `excalidraw/clipboard_with_api`, `excalidraw`, or a missing `type`,
return `None`); run the same conversion as `.excalidraw` files (6.1) on the
`elements` array; `None` if the result is empty. The result shapes are
recentered on the cursor. `files` (images) is ignored. Excalidraw library
clipboard payloads are not recognized.

### 5.4 PNG export

Menu "Export PNG" / Ctrl+E (`ui.rs:1952-1955`, `app.rs:3691-3755`, `app.rs:4870-4941`),
scale selector 1x/2x/3x, default **2x** (`ui.rs:253,326,1972`), stored only in
runtime state.

- Content: the selection if any, else the whole document (`build_export_scene`
  / `build_export_scene_selection`, `drafftink-render/src/vello_impl.rs:380-510`):
  no grid, no selection chrome. Bounds = union of `bounds()` of the shapes,
  **padded by 20 world units on every side**, then scaled: output size =
  `ceil((w+40)*scale) x ceil((h+40)*scale)` pixels (`vello_impl.rs:401-411`,
  `app.rs:3705-3706`). Empty document -> nothing is exported (logged).
- Background: opaque white (`Color::WHITE`) both as the render base color
  (`app.rs:1165`, web `app.rs:1304`) and as a filled rect (`vello_impl.rs:413-421`).
  No transparent-background option.
- Encoding: `png` crate 0.17.16, RGBA, 8-bit (`app.rs:1438-1474`); Rgba8Unorm
  readback with row-padding removed. Native writes via an `rfd` save dialog
  (title "Export PNG", default `"{document.name}.png"`, filter `png`,
  `app.rs:149-167`); web downloads `"{name}.png"` as Blob `image/png`
  (`app.rs:477-480,559-590`).
- **Embedded scene**: the **entire** document (even when only a selection was
  exported) is serialized with `to_json()` (pretty JSON) and stored in one
  compressed `zTXt` chunk with keyword `application/vnd.drafftink+json`
  (`PNG_METADATA_KEYWORD`, `app.rs:1435,1450-1457`). Extraction:
  `extract_scene_from_png` reads `compressed_latin1_text` and returns the first
  chunk with that keyword (`app.rs:1477-1487`). PNGs produced by "Copy as PNG" have no
  such chunk (`app.rs:3811,5052`). Dropping such a PNG on the window loads it as a
  document (replacing the live one, selection cleared, camera reset,
  `app.rs:5375-5400`, web `app.rs:1048-1055,2348-2362`).
- Probe-verified limitation: `zTXt` text is Latin-1 in `png` 0.17.16. If the
  document JSON contains any character above U+00FF (CJK, emoji, arrows, curly
  quotes, ...), `write_header` fails (`BadTextEncoding(Unrepresentable)`),
  `encode_png` returns `None` and **no PNG is produced** (only an error log).
  Text with characters up to U+00FF (e.g. `é`) works and round-trips.
- No SVG export, no PDF, no clipboard-as-JSON export.

### 5.5 JSON export / import menu paths

| Action | Native | Web |
|---|---|---|
| Export JSON | "Save Local" / "Save Local As..." / Ctrl+S / "Download JSON" all open the save dialog (`app.rs:2698-2788`, native `ui.rs` hides the Download/Import items: `ui.rs:1932`) | "Download JSON" (file); "Save Local" writes IndexedDB |
| Import JSON | "Open" / Ctrl+O -> dialog (`json`, `excalidraw`) | "Import JSON/Excalidraw" (`.json,.excalidraw`) |
| Import Mermaid | "Import Mermaid (Clipboard)" reads the system clipboard (`app.rs:2798-2832`) | same via async clipboard read |
| Import `.excalidrawlib` | tab bar "+ Library" (`ui.rs:704-717`) | same; input accepts `.excalidrawlib,.json` (`app.rs:616`) |

---

## 6. Excalidraw import (`drafftink-core/src/excalidraw.rs`)

### 6.1 Entry points

- `.excalidraw` file (extension check, 1.1) and clipboard: `CanvasDocument::from_excalidraw`
  (`canvas.rs:263-280`): parse JSON (error text `"Invalid JSON: ..."`); require a
  top-level `elements` **array** (else `Err("Missing 'elements' array")`; a JSON
  array at top level also errors); `type`, `version`, `appState`, `files` are not
  checked or used. For each element in order, `element_to_shape` (`excalidraw.rs:243-266`);
  `None` results are skipped. The new document has a fresh random `id`, `name`
  `"Untitled"`, shapes added in element order (z-order = array order), **flat**
  (scene import ignores `groupIds`).
- A `.json` file that contains an Excalidraw scene is *not* recognized (extension
  decides; `from_json` then fails).
- Library: `library_from_excalidrawlib` + `library_layout_grid` (6.5).
- Never panics on bad structure except one probe-found case (6.4).

### 6.2 Element conversion table

Common: an element with `"isDeleted": true` is skipped (`excalidraw.rs:244-250`).
`type` defaults to `""`. `x`, `y` default 0.0 (`as_f64`, so non-numbers also give 0).

| Excalidraw `type` | Result | Fields read | Mapping / defaults |
|---|---|---|---|
| `rectangle` | `Rectangle` | `x,y,width,height` (default 100 each), `roundness` | position = (x,y); `rotation` 0. If `roundness` is present and non-null: `corner_radius = min(32, width/4, height/4)`; else 0 (`excalidraw.rs:318-335`) |
| `diamond` | `Rectangle` (same as above) | same | **A diamond becomes its bounding rectangle** (`excalidraw.rs:258`) |
| `ellipse` | `Ellipse` | `x,y,width,height` (default 100) | center = (x+w/2, y+h/2), radii w/2, h/2 (`excalidraw.rs:338-351`) |
| `freedraw` | `Freehand` | `points` | points translated by (x,y); `pressures` empty (Excalidraw `pressures` ignored); dropped if no valid points; `closed` = at least 3 points and first/last differ by less than 10 in both x and y (`CLOSED_PATH_THRESHOLD`, `excalidraw.rs:30,371-378,381-391`) |
| `line` | `Line` | `points`, `roundness` | needs >= 2 valid points; `start` = first, `end` = last, rest = `intermediate_points`; `path_style` = `Flowing` if `roundness` non-null else `Direct`; `closed` by the same 10-unit rule (`excalidraw.rs:394-409`). `polygon` flag not read. |
| `arrow` | `Arrow` | `points`, `elbowed`, `roundness` | needs >= 2 points; `path_style` = `Angular` if `elbowed == true`, else `Flowing` if `roundness` non-null, else `Direct`; `head_size` 15 (default); only an end head; start/end arrowheads (`startArrowhead`/`endArrowhead`) are **not read** (`excalidraw.rs:412-431`) |
| `text` | `Text` | `text`, `fontSize` | `content = text` (not `originalText`), `font_size = fontSize` (default 20), `font_family` GelPen, `font_weight` Regular, position = (x,y) top-left, `char_colors` all `None` (`excalidraw.rs:434-448`) |
| anything else (`image`, `frame`, `magicframe`, `embeddable`, `iframe`, `selection`, unknown) | **dropped** | | `_ => None` (`excalidraw.rs:264`) |

`points`: array of `[dx, dy]` arrays; entries that are not arrays or lack two
numbers are filtered out (`excalidraw.rs:354-368`).

Style (`parse_style`, `excalidraw.rs:269-315`), applied to every converted type
including text (so `backgroundColor` becomes the text's `fill_color`):

| Excalidraw field | Native | Default / rule |
|---|---|---|
| `strokeColor` | `stroke_color` | `#000000`; parsed by `parse_color` (below) |
| `backgroundColor` | `fill_color` | string `"transparent"` or absent -> `None`; otherwise parsed color (so a non-hex name like `"red"` becomes black) |
| `strokeWidth` | `stroke_width` | 2.0 |
| `roughness` (integer) | `sloppiness` | 0 -> Architect, 1 -> Artist, anything else (2, ...) -> Cartoonist; absent -> Artist. A float `roughness` (e.g. 1.5) is not an i64 and gives the default 1 (Artist). `Drunk` is never produced. |
| `fillStyle` | `fill_pattern` | `"hachure"` -> Hachure, `"cross-hatch"` -> CrossHatch, `"zigzag"` -> ZigZag, **everything else including `"solid"`, `"dots"`, `"dashed"`, `"zigzag-line"` -> Solid** |
| `seed` | `seed` | `as_u64 as u32` (wraps; probe: 4294967297 -> 1); absent -> 0 (note: 0, not random) |
| `opacity` | `opacity` | value `> 1.0` is divided by 100 (Excalidraw uses 0-100); values `<= 1` are taken as already 0..1. An Excalidraw opacity of exactly 1 (meaning 1 %) therefore becomes 1.0 (opaque). Absent -> 1.0. |

`parse_color` (`excalidraw.rs:457-487`): `"transparent"` -> (0,0,0,0);
`#rgb` -> each nibble x17, alpha 255; `#rrggbb`; `#rrggbbaa`; any other string
(named colors, `rgb()`, `#rgba`, wrong length) -> black. **Probe: a `#`-prefixed
string whose byte length is 3, 6 or 8 but which contains a multi-byte character
(e.g. `"#aé"`) panics** with "not a char boundary" (byte slicing, line 465/466);
the old app would crash on such a file.

### 6.3 Explicit list of what is dropped or defaulted

Dropped (never read): `angle` (rotation of every element: rectangles, ellipses,
text and images import unrotated), `strokeStyle` (dashed/dotted -> always Solid,
including for lines and arrows), `startArrowhead` / `endArrowhead` / arrowhead
kinds, `startBinding` / `endBinding` / `boundElements` / `containerId` (bindings
and bound text: a text inside a shape imports as a free text at its own x,y; the
container is imported as an empty shape), `fontFamily` (always GelPen), `textAlign`,
`verticalAlign`, `lineHeight`, `autoResize`, text `width`/`height`, `originalText`,
`roundness.type` and `.value` (only null/non-null matters; radius is always the
fixed adaptive formula), `groupIds` (in scene import), `frameId` and frames,
`link`, `locked`, `index` / fractional index, `version`/`versionNonce`/`updated`,
`pressures` and `simulatePressure` of freedraw, `polygon` of lines, `crop`,
`scale`, `status`, `fileId`, all `files`, `appState` (background color, grid,
zoom, scroll), `libraryItems` metadata (`id`, `status`, `created`).

Defaulted: new random element ids (Excalidraw ids are not kept, so bindings could
not be reconstructed anyway), `Text.font_family = GelPen`, `font_weight = Regular`,
arrow `head_size = 15`, element size 100x100 when `width`/`height` missing,
rectangle radius 0 without `roundness`, `seed` 0 without `seed`.

Not importable: images and their `files` map (element `image` is dropped),
frames, embeddables, diamond geometry, arrow bindings, rotation, dashed strokes.

### 6.4 Behavior on malformed input

- Wrong-typed fields silently fall back to defaults (`as_f64` / `as_str` /
  `as_bool` `unwrap_or`).
- `elements` containing non-object entries: `get("type")` on a non-object yields
  `None`, `elem_type == ""`, element dropped.
- The only known crash is the `parse_color` multi-byte slice above.

### 6.5 `.excalidrawlib`

`library_from_excalidrawlib(json)` (`excalidraw.rs:56-84`):

- Requires `type == "excalidrawlib"`, else `None` (the file is then rejected with a
  log "Not a valid Excalidraw library", `app.rs:130`). `version` is ignored.
- If `libraryItems` is an array (v2): each entry must be an **object** with an
  `elements` array; `name` is read if a non-blank string, else `"Item {i+1}"`.
  Entries that are arrays (an array-of-arrays `libraryItems`) have no `elements`
  key and are skipped.
- Else if `library` is an array (v1): each entry must be an array of elements;
  names are `"Item {i+1}"`. If `libraryItems` exists, `library` is never consulted.
- Item elements go through the same `element_to_shape` (6.2); unconvertible
  elements are skipped; an item with no convertible element is skipped; zero items
  -> `None`.
- **Groups are reconstructed** from each element's `groupIds` (innermost first,
  outermost last): elements sharing the outermost id form a group, recursively
  peeling one id per level; a group with exactly one child collapses to that child
  (`assemble_groups`, `excalidraw.rs:191-235`). The top level of an item is always
  a single `Group`: if the assembled top level has one child that already is a
  `Group` it is used as is, one non-group child is wrapped in a new `Group`, several
  children are wrapped in a new `Group` (`excalidraw.rs:155-177`). All group ids are fresh
  UUIDs; group `rotation` 0; z-order within a group follows the element order of
  first occurrence.
- Item names are used nowhere in the produced document (the UI discards them).

Grid layout `library_layout_grid` (`excalidraw.rs:86-138`), used by
`build_library_document` (`app.rs:1689-1705`):

- constants: `GRID_CELL_PADDING = 24`, `GRID_MAX_COLUMNS = 10`.
- cell width/height = max item bounds width/height + 2*24 (uniform cells).
- `columns = clamp(ceil(sqrt(n)), 1, 10)`; item `i` -> row `i / columns`, col `i % columns`.
- cell center = `(col*cell_w + cell_w/2, row*cell_h + cell_h/2)`; each item is
  translated so its bounding-box center sits on its cell center. Grid origin is
  (0,0), growing right/down.
- Result is a `CanvasDocument` with one top-level `Group` per item, in item order.

---

## 7. Mermaid import (`drafftink-core/src/mermaid/`)

Entry: `shapes_from_mermaid(text) -> Option<Vec<Shape>>` (`mermaid/mod.rs:121-123`).
Returns `None` (never an error to the user) when the text is not a recognized
diagram or yields no drawable content. Used by Ctrl+V fallthrough and the
"Import Mermaid (Clipboard)" menu item (5.1, 5.5). Success returns **exactly one
shape**: a `Group` of all geometry (`mod.rs:126-139`), later recentered on the
cursor/viewport. Result shapes are native Rectangle/Ellipse/Line/Arrow/Text; no
images. Never panics by design (CRITICAL PATH comment, `mod.rs:17-22`).

### 7.1 Detection

1. Trim; if the text starts with ` ``` ` drop the first line (the fence tag, e.g.
   `mermaid`) and a trailing ` ``` ` (`mod.rs:143-156`).
2. The first non-empty line that does not start with `%%` decides (`mod.rs:158-175`):
   starts with `sequenceDiagram` -> sequence; starts with `graph` or `flowchart`
   -> flowchart (prefix match, so `graphX` also matches); anything else
   (`classDiagram`, `stateDiagram`, `erDiagram`, `gantt`, `pie`, `journey`, `mindmap`,
   `gitGraph`, plain text, ...) -> `None`.
3. If detection succeeds but nothing drawable was produced -> `None`.

### 7.2 Flowchart (`flowchart.rs`)

Header and parse loop (`flowchart.rs:165-192`): the **first line of the
fence-stripped body is assumed to be the header** and is only used for direction
(`parse_direction`: case-insensitive whole-word match for `LR`, `RL`, `TB`, `TD`,
`BT`, checked in that order; default `TB`; a token such as `LR;` does not match
and falls back to `TB`). If a comment or blank line precedes the header (allowed
by detection) the header is then parsed as a statement and creates a spurious
node named `graph`/`flowchart`. Every following line: remove everything from
`%%`, split on `;` (even inside labels), trim, skip empty.

Statements (`parse_statement`, `flowchart.rs:196-217`):

| Statement | Behavior |
|---|---|
| `subgraph ID["Title"]`, `subgraph ID[Title]`, `subgraph ID` | opens a cluster (nestable); `end` closes it. Only the first identifier token is the id; a title without brackets is not used (`subgraph My Title` -> id/label `My`). |
| `direction XX` | inside a subgraph sets that cluster's flow direction (same token rules); outside a subgraph ignored |
| `end` | pops the current subgraph |
| lines starting with `style`, `classDef`, `class`, `click`, `linkStyle` (word-boundary) | **ignored silently** (no styling is applied) |
| everything else | `parse_chain`: node, then repeated (link, node) |

Unsupported or mangled constructs are **ignored or mis-parsed without error**:
`A & B --> C` (probe: only node `A` is created; `B`, `C` and the edge are lost), `A --> B & C` (edge A->B kept, `C` lost),
`:::class` suffixes (parsing of that line stops there), `A@{...}` shape syntax,
`%%{init}%%` (removed as a comment), `accTitle`, markdown strings, HTML other than
`<br>`, `<br/>`, `<br />`, `click` callbacks, edge ids, asymmetric `A>text]`, cylinder `[(db)]`, parallelogram `[/x/]`, `[\x\]`, trapezoid,
double-circle `(((x)))`.
Probe-confirmed: `A[a;b] --> B` splits at `;` and yields nodes `A` and `b` with no edge; a leading `%% comment` line before `graph LR` yields a spurious node labelled `graph`; `A:::foo --> B` yields only `A`; `pie ...` and `graph TD` alone return `None`.
Concretely: `[(db)]` imports as a rectangle labelled `(db)`; `[/x/]` as a rectangle
labelled `/x/`; `A>text]` is treated as node `A`, a link `>`, node `text`, then junk.

Nodes (`flowchart.rs:362-426`): id = run of alphanumeric chars and `_` (Unicode
alphanumerics allowed; `-` and `.` are not id chars). Shape brackets directly after
the id; two-char openers tested first:

| Syntax | `NodeShape` | Drawn as |
|---|---|---|
| `A[text]` or bare `A` | Rectangle | `Rectangle` radius 0, fill white |
| `A(text)` | Rounded | `Rectangle` radius `min(half_h/2, 16)` |
| `A([text])` | Stadium | `Rectangle` radius `half_h` |
| `A[[text]]` | Subroutine | Rectangle plus two vertical `Line`s inset `min(0.08*w, 10)` |
| `A((text))` | Circle | `Ellipse::circle` radius `half_w` |
| `A{text}` | Diamond | closed `Line` polygon (4 points) |
| `A{{text}}` | Hexagon | closed `Line` polygon (6 points, inset `min(half_h, half_w)`) |

The label is the bracket content with surrounding `"` trimmed and `<br>` variants
turned into newlines (`mod.rs:199-205`). The first definition that carries an
explicit shape wins; later bare references do not overwrite it (`intern_node`,
`flowchart.rs:331-350`). A node first seen inside a subgraph belongs to it.

Links (`parse_link`, `flowchart.rs:441-487`): a run of characters from `- . = < >`.
Arrowhead present if the run contains `>` or `<`, or ends with a cross/circle head
`x`/`o` at a token boundary. Style: contains `.` -> dotted; contains `=` -> thick;
else solid. Label forms: pipe `-->|text|`, embedded `-- text -->` or
`-- "quoted" -->` (only when the first run has no arrowhead and a second run
follows), and combined styles `-. text .->`.
Mapping: arrow -> `Arrow` (single head at the **target**, `PathStyle::Direct`),
no arrow -> `Line`; dotted -> `StrokeStyle::Dotted`; **thick is rendered as Solid**
with normal width; `<-->` gets one head only (at the target); a leading `<--`
(`A <-- B`) still draws the head at B (direction semantics lost). Edge endpoints are
border points on the straight segment between node centers (box, circle or
rhombus intersection; clusters use their box), not routed paths. Self-loops draw a
four-point loop above the node. An edge label becomes a white-filled borderless
rect (transparent stroke, radius 2, 4 padding) plus a 14 px text centered on the
segment midpoint.

### 7.3 Flowchart layout and output constants

Text metrics are estimates (`mod.rs:41-58,181-195`): font 16 px (edge labels 14),
glyph advance `0.6 * font_size` per char, line height `1.2 * font_size`;
width = longest line chars * advance; height = lines * line height.

| Constant | Value | Source |
|---|---|---|
| `LABEL_FONT_SIZE` | 16 | `mod.rs:41` |
| `EDGE_LABEL_FONT_SIZE` | 14 | `flowchart.rs:38` |
| `BOX_PADDING_X` / `BOX_PADDING_Y` | 20 / 12 | `mod.rs:52,55` |
| `MIN_BOX_WIDTH` | 72 | `mod.rs:58` |
| `NODE_HEIGHT` (minimum) | 46 | `flowchart.rs:23` |
| `RANK_GAP` (between layers) | 84 | `flowchart.rs:26` |
| `CROSS_GAP` (between siblings in a layer) | 44 | `flowchart.rs:29` |
| `DIAMOND_SCALE` | 1.5 | `flowchart.rs:35` |
| `CLUSTER_PADDING` | 22 | `flowchart.rs:41` |
| `CLUSTER_LABEL_BAND` (title band height) | 32 | `flowchart.rs:44` |
| `CLUSTER_CORNER_RADIUS` | 8 | `flowchart.rs:47` |
| `CLUSTER_FILL` | rgb(241,245,249) | `flowchart.rs:50-55` |
| `STROKE_WIDTH` | 2 | `mod.rs:61` |
| `STROKE_COLOR` | rgb(30,41,59) | `mod.rs:64-69` |
| `BOX_FILL` | white | `mod.rs:73-78` |
| `NOTE_FILL` | rgb(255,249,196) | `mod.rs:81-86` |

Node size (`node_size`, `flowchart.rs:941-954`): `base_w = max(text_w + 40, 72)`,
`base_h = max(text_h + 24, 46)`; Circle: `d = max(base_w, base_h)` square;
Diamond: `(1.5*base_w, 1.5*base_h)`; Stadium/Hexagon: `(base_w + base_h, base_h)`;
others `(base_w, base_h)`.

Layout (`flowchart.rs:623-857`): hierarchical. Each scope (root or subgraph) is
laid out independently with longest-path ranking over edges that connect its direct
children (edges to deeper nodes are lifted to the direct-child ancestor); back
edges found by iterative DFS and self-loops are ignored when ranking. Ranks run
along the flow axis (x for `LR`/`RL`, y for `TB`/`TD`/`BT`), reversed for `RL`/`BT`;
within a layer nodes keep **declaration order** and are centered on the cross
axis (no crossing minimization). A layer's thickness is its largest node on the
flow axis; layers are `RANK_GAP` apart, siblings `CROSS_GAP` apart. A cluster's box
= content bbox grown by 22 on all sides, 32 more on top for the title, widened to
fit the title (`finalize_scope`, `flowchart.rs:706-739`); an empty cluster is
`72 x (46+32)`. The root frame is centered at the origin; the caller recenters
the group on the paste target.

Emit order (z-order inside the group, `flowchart.rs:962-995`): clusters sorted by
nesting depth (outer first), each as Rectangle (radius 8, fill `CLUSTER_FILL`) then
a centered Text (title); then all edges in source order (line/arrow, then label
rect + label text); then leaf nodes (outline then centered Text). All texts:
`Text` 16 px (14 for edge labels), GelPen, Regular, stroke `STROKE_COLOR`, style
via `base_style` (`mod.rs:208-215`: stroke width 2, no fill, `Solid`, `Artist`
sloppiness, fresh seed). Text position = center minus half the estimated size.
Outlines use `base_style(Some(BOX_FILL))` (white fill). Mermaid-created shapes
never use per-node colors, classes, or `style` directives.

The rewrite (`diagram.import_mermaid`, `crates/edit/src/layout/cluster.rs`, `docs/commands.md` 2.19) keeps the
cluster box: padding 22, title band 32, corner radius 8, fill rgb(241,245,249), outer boxes first. What differs: the
box is laid out with the layered algorithm of `edit` (crossing reduction, not declaration order), a box is at least
120 wide, the title is a text element in the band, nodes get bound arrows, and an edge to a subgraph id is drawn and
bound to the box.

### 7.4 Sequence diagrams (`sequence.rs`)

Parse (`sequence.rs:125-152`): per line: remove `%%...` comment, trim; skip empty
and the exact line `sequenceDiagram`. Then, in order:

| Syntax | Behavior |
|---|---|
| `participant X`, `participant X as Label`, `actor X [as Label]` | declares a participant (id = text before ` as `, label = text after). `actor` is drawn like `participant` (no stick figure). Re-declaring updates the label. |
| `Note over A[,B]: text`, `Note left of A: text`, `Note right of A: text` (also lowercase `note`) | note event; `over` with several ids spans min..max column; text after the first `:` (may be empty) |
| `A->>B: text` and the other arrows | message |
| any other line | skipped |

Arrow tokens (longest first, leftmost match wins; `sequence.rs:55-57`): `<<-->>`, `<<->>`,
`-->>`, `--x`, `--)`, `-->`, `->>`, `-x`, `-)`, `->`. Token containing `--` -> dashed
(`StrokeStyle::Dashed`), else solid. All render as an `Arrow` with one head at the
receiver (`x`/`)`/bidirectional heads are not distinguished). Messages split at
the first `:`; sender/receiver text is trimmed and used as participant ids; unseen
ids create implicit participants in order of first appearance.

Intentionally skipped (no error): `title`, `autonumber`, `activate`/`deactivate`,
`loop`, `alt`/`else`, `opt`, `par`/`and`, `critical`, `break`, `rect`, `end`,
`box`, `create`/`destroy`, `link`, `links`. Caveat: any *unrecognized* line that
contains an arrow token becomes a message, e.g. `alt x -> y` or `A->>+B: hi` (the
`+`/`-` activation markers become part of the participant id `+B`, which creates a
separate participant `+B`).

Layout constants (`sequence.rs:20-51`, shared ones in 7.3):

| Constant | Value |
|---|---|
| `PARTICIPANT_BOX_HEIGHT` | 44 |
| `COLUMN_GAP` | 64 |
| `MESSAGE_GAP_Y` | 56 |
| `TOP_MARGIN` | 56 |
| `BOTTOM_MARGIN` | 40 |
| `MESSAGE_LABEL_OFFSET` | 10 |
| `NOTE_MIN_HEIGHT` | 40 |
| `NOTE_GAP_Y` | 24 |
| `SELF_LOOP_WIDTH` / `SELF_LOOP_HEIGHT` | 72 / 40 |
| `BOX_CORNER_RADIUS` | 4 |

Geometry (`sequence.rs:289-460`): participant box width = `max(text_w + 40, 72)`;
all columns use pitch `max_box_width + 64`; column `i` center x = `i*pitch + pitch/2`.
Participant boxes at y=0, height 44, fill white, radius 4, label centered.
Dashed vertical lifelines from y=44 down to `last_event_y + 40`. First event y =
`44 + 56 = 100`; each message advances 56; a message is a horizontal `Arrow` at y
from column `from` to column `to`, label centered above it (10 above the arrow).
Self-message: 4-point loop `(cx,y)->(cx+72,y)->(cx+72,y+40)->(cx,y+40)`, label to
its right, advance `40 + 56`. Note: height `max(label_h + 24, 40)`, width
`max(label_w + 40, span_width)`, fill `NOTE_FILL`, centered over the span
(`over`) or beside the column (`left of`/`right of`: offset by half the widest box
plus half the note width), advance `note_h/2 + 24 + 28`. Output order:
lifelines, then events in source order, then participant boxes and labels.
Styles via `base_style` as in 7.3.

---

## 8. Gotchas for loaders

1. **No version field** anywhere (document, PNG chunk, IndexedDB, clipboard). The
   only versioning is the `serde(default)` set. Detect old files structurally.
2. **Whole-file failure on any bad shape**: the old loader never skips a single
   malformed shape (1.8). A rewrite should either match (to reject) or be more
   lenient; note which old files the old app could not open at all.
3. **Defaults for omitted optional fields** (Line/Arrow `intermediate_points`,
   `path_style`, `stroke_style`; Line `closed`; Freehand `pressures`, `closed`;
   Text `char_colors`; all `rotation`; ShapeStyle `fill_pattern`, `seed`, `opacity`;
   `fill_color`). `intro.json` itself relies on them.
4. **Enums are name strings**, never numbers, in JSON; numbers only in the Loro
   schema. Unknown names are fatal in JSON, and are silently mapped to the *last*
   variant in Loro.
5. **`closed` is lost** through the CRDT round trip and was only ever set by
   Excalidraw import and Mermaid diamonds/hexagons (`excalidraw.rs:389,407`,
   `flowchart.rs:1109`). In JSON files it can be `true`; a closed line draws a
   closing segment and a fill, and its point list does not repeat the first point.
6. **Group rotation** is stored in JSON (`Group.rotation`) and applied at render
   time about the union-bounds center, but is dropped by the CRDT and not
   propagated to children by `Group.transform`. Group children are in absolute
   coordinates; a rotated group's children are unrotated in their own fields.
7. **Rotation semantics are shape-local**: radians, around the shape's own bounds
   center. Text and Math bounds depend on a cached renderer measurement that is not
   stored, so the rotation pivot of text changes slightly with font metrics.
   Line, Arrow and Freehand have no rotation field; rotating them is not
   representable.
8. **Shape ids**: map keys and inner ids are separate; keep the map key for
   `z_order` lookups. Group children keep ids but are not in the top-level map.
   Internal paste regenerates only the top-level id.
9. **`format` on Image can lie**; sniff the bytes. `data_base64` is padded standard
   base64; `source_width/height` are pixel counts, `width/height` the display size.
10. **Text `char_colors` indexes Unicode scalar values**, not UTF-16 or graphemes;
    it may be shorter than the content.
11. **`seed` is not stable**: absent seeds are regenerated per load, so the
    hand-drawn jitter of such shapes differs from run to run; Excalidraw import
    sets seed 0 when absent and truncates to u32.
12. **NaN/inf coordinates** serialize to `null` and make the file unreadable.
13. **IndexedDB values are JS objects, not JSON** (`shapes` is a `Map`, missing
    options are `undefined`); keys are document names, plus `__last__`. The web
    autosave only triggers on shape-count changes. A reload may therefore lose edits.
14. **Native has no autosave**; nothing is stored in an app directory. `intro.json`
    is the only document ever auto-loaded on native.
15. **Tabs and cameras are not persisted**; do not expect them in any file.
16. **PNG export embeds the whole document** (Latin-1 `zTXt`, keyword
    `application/vnd.drafftink+json`) even for selection exports; documents with
    any char above U+00FF could not be exported by the old app, so such PNGs do
    not exist. Copied PNGs have no embedded scene.
17. **Collaboration payloads are full Loro snapshots** in base64 inside JSON text
    frames; the server stores only the last snapshot per room in memory. No
    compatibility with old rooms is needed after a restart, but the wire format
    above is what old clients speak.
18. **Excalidraw import is shallow**: no images, no rotation, no dash styles, no
    arrowheads, no bindings/containers, diamond -> rectangle, fill patterns other
    than hachure/cross-hatch/zigzag -> solid, roughness >= 2 -> Cartoonist,
    `opacity` 1 treated as 100 %. Library import preserves groups; scene import does not.
19. **Excalidraw `.json` scenes are not recognized**, only the `.excalidraw`
    extension and `excalidraw/clipboard` text.
20. **Mermaid**: only `graph`/`flowchart` and `sequenceDiagram`; thick links look
    like solid; `A & B` lists, class/style directives and non-listed node shapes are
    dropped or mis-read; the header must be the first line after the fence; `;`
    splits statements even inside labels; output is always one `Group`.
21. **Panic risk** in the old Excalidraw color parser on non-ASCII `#...` strings
    (6.2); a rewrite must not copy the byte-slice approach.
22. **HashMap order**: old files list shapes in random key order; always sort by
    `z_order`, never by file order.
