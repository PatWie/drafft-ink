# Fixtures from the old Drafft.ink app

These files let the rewrite show that it loads old documents and reproduces old behavior. Every derived file was produced by the **old code** (`drafft-ink` at commit `8ce40ab`, crate `drafftink-core`), not written by hand. The format specs are in `docs/formats.md`.

## Layout

| Path | What it is | Made by |
|---|---|---|
| `legacy/<name>.json` | Native document (`CanvasDocument::to_json` output, ids canonicalized, see below) | old serializer |
| `legacy/<name>.facts.json` | What the old code computes about that document | old code |
| `legacy/large_1000.json.gz`, `large_10000.json.gz` (+ `.facts.json.gz`) | 1,000 and 10,000 generated shapes, seeds 1000 and 10000. Gzip, fixed header (reproducible). Facts for 10k skip the CRDT round trip (the old code is quadratic there) | old serializer |
| `legacy/welcome.json` (+facts) | The shipped intro document, `crates/drafftink-app/assets/intro.json`, byte for byte, original ids | copied |
| `legacy/crdt/<name>.loro` (+`.facts.json`) | Loro snapshot written by the old `CrdtDocument` (peer id 1), plus a dump of the container tree | old CRDT code |
| `excalidraw/*.excalidraw`, `*.excalidrawlib`, `clipboard/*.json` | Importer **inputs** | see Provenance |
| `excalidraw/expected/` | The old importer's output for each input, as a native document + facts | old importer |
| `mermaid/*.mmd`, `mermaid/rejected/*.mmd` | Importer **inputs**, hand-written | authored |
| `mermaid/expected/` | The old importer's output for each accepted input + `summary.json` (accepted or not, for every input) | old importer |
| `render/` | PNG reference renders by the old renderer (offscreen, NVIDIA GPU), with their own README | see `render/README.md` |
| `render-gen/` | The offscreen render tool (standalone cargo project, `gen.sh` regenerates `render/`) | |
| `generator/` | The generator (a standalone cargo project that depends on the old crates by path) | |
| `MANIFEST.json` | sha256 and size of every generated file, sha256 of every hand-written input | generator |

### Legacy documents

One document per concern; each shape type appears with every style variant it supports.

| Name | Content |
|---|---|
| `all_shapes` | One of each shape type, plus a group |
| `styles_rectangle` | 4 sloppiness x 7 fill patterns, widths, opacity, corner radii, rotation, alpha colors, zero and negative sizes |
| `styles_ellipse` | same grid, plus rotation, circles, zero radii |
| `styles_line` | 3 path styles x 3 stroke styles x (2 and 5 points), closed polygons (the `closed` flag is lost in the CRDT), start == end |
| `styles_arrow` | path style x stroke style x point counts, head sizes 0 to 60, reversed and zero-length arrows |
| `styles_freehand` | widths, with and without pressures, closed loops (lost in the CRDT), single point, opacity |
| `text_fonts` | all four `FontFamily` values (GelPen, NotoSans, GelPenSerif, VanillaExtract) x three weights, sizes 8 to 64, multi-line, empty, CJK, emoji, tab, rotation |
| `text_char_colors` | per-character colors (the only per-character style the old app has), unicode, alpha, `char_colors` shorter and longer than the content |
| `math` | LaTeX formulas, an empty and two invalid ones, sizes, rotation, opacity |
| `images` | PNG, JPEG, WebP payloads (real encoded bytes), non-uniform scaling, rotation, border and opacity |
| `groups_nested` | flat group, depth-3 nesting with mixed children, a **rotated group**, a group of every type, a one-child group, an empty group, interleaved loose shapes |
| `z_order` | overlapping squares reordered with the old `bring_to_front`, `send_to_back`, `bring_forward`, `send_backward` |
| `welcome` | the intro document |
| `large_1000`, `large_10000` | mixed shapes (rect, ellipse, line, arrow, freehand, text, math, groups), positions on a square of side `sqrt(n) * 140` |

There is no tab file format: tabs exist only in memory in the old app (`docs/formats.md`, "Canvas tabs"). The only tab content derived from a file is a library tab, and its fixtures are `excalidraw/expected/*.grid.json`.

### Canonicalization

The old code assigns random UUIDv4 ids and keeps shapes in a `HashMap`, so two runs never agree byte for byte. The generator serializes with the old `to_json`, then changes only:

1. Every shape id (including group children) becomes `00000000-0000-4000-8000-<counter, 12 hex>`, counting in depth-first order over `z_order`.
2. The `shapes` map is written in z-order. The document `id` is a fixed value derived from the name, and `name` is the fixture name.
3. The private `style.seed` of each `Group` (taken from a process-wide counter in the old code) is set to an FNV-1a hash of the group id.

The result is loaded back with the old `from_json` and re-serialized. The generator fails if that differs. `welcome.json` is not canonicalized.

## Facts format (version 1)

`<name>.facts.json` is what a checker compares against. Layout:

```
{
  "facts_version": 1,
  "oracle": {"repo": "drafft-ink", "commit": "8ce40ab", "crate": "drafftink-core"},
  "source": "legacy/all_shapes.json",          // path of the document, relative to tests/fixtures
  "source_sha256": "...",                      // of the uncompressed document bytes
  "doc": {
    "name": "...",
    "shape_count_top_level": N,                // entries in z_order
    "shape_count_total": N,                    // top level + all group descendants
    "counts_top_level": {"Rectangle": n, ...}, // by variant name, only non-zero keys
    "counts_total":     {"Rectangle": n, ...},
    "bounds": [x0, y0, x1, y1] | null,         // CanvasDocument::bounds(), union of Shape::bounds()
    "bounds_estimated": bool,
    "z_order_idx": [idx, ...],                 // back to front, top-level shapes only
    "shapes": [ { "idx", "id", "type", "parent", "depth", "z", "bounds",
                  "bounds_estimated", "rotation", "extra"? }, ... ],
    "groups": { "<group idx>": [child idx, ...] },
    "probes": { "tolerance": 5.0,
                "hit":  [ {"point": [x, y], "hits": [idx, ...]} ],   // shapes_at_point, front to back
                "rect": [ {"rect": [x0,y0,x1,y1], "hits": [idx, ...]} ] },  // shapes_in_rect, z order
    "crdt_roundtrip": null | { "shape_count", "z_order_equal", "changed": [ {"top_level_index","type","fields":[path,...]} ] }
  }
}
```

- `idx` is the pre-order position: top-level shapes in z-order, each group immediately followed by its descendants. It is the stable key. Use it to compare when ids differ (imports, new code). `parent` is the idx of the enclosing group or `null`. `z` is the position among siblings. `depth` is 0 for top level.
- `bounds` is the old `Shape::bounds()`: no stroke width inflation, unrotated AABB for rectangle, ellipse, text, image and group, and the rotated AABB for math only. A group's bounds is the union of its children and ignores the group's rotation.
- `bounds_estimated` is true for text, math, and any group or document that contains them. The old code only knows their real size after the renderer has measured them. These values come from the old fallback estimate (text: longest line in **bytes** x font size x a per-font factor, height lines x size x 1.2, width at least 20; math: latex byte length x size x 0.5, height = size, depth = 0.3 x size). A new text engine will not reproduce them. Compare estimated bounds only if the new code also implements the old estimate, otherwise skip them.
- `extra` carries per-type details: text (`chars`, `char_colors_len`, `char_colors_set`, `font_family`, `font_size`), math (`latex`, `font_size`), image (`format`, `source`, `data_bytes`), group (`children`), line and arrow (`points`, `closed` or `head_size`), freehand (`points`, `pressures`, `closed`).
- Probe points are a fixed 8x8 grid over the document bounds inflated by 20, plus the top-left corner and center of the first 40 top-level shapes. A checker should use the `point` and `rect` values stored in the file and not recompute them. The `hit` lists record the old hit-test semantics, including its quirks (rotation ignored, hollow groups).
- `crdt_roundtrip` records what the old CRDT keeps: top-level shapes were added to a `CrdtDocument` and read back; `changed` lists JSON paths that differ (for example `Line.closed`, `Group.rotation`). `null` for imports and for `large_10000`.
- Numbers: IEEE doubles written by `serde_json` (shortest round-trip). Several values carry float noise from the old arithmetic (for example `32.00000000000001`). **Compare coordinates with an absolute tolerance of 1e-6**, everything else exactly.
- Large facts files are gzipped (`*.facts.json.gz`). The CRDT facts (`legacy/crdt/*.facts.json`) have a different body: `name`, `z_order`, `shape_count`, and `deep_value` (the Loro `get_deep_value()` JSON), and `built_from`.
- Excalidraw library inputs have `*.items.json` (item names, bounds, group child counts as the old `library_from_excalidrawlib` returns them) next to `*.grid.json` (the old `library_layout_grid` placed in a document). Clipboard inputs have `*.result.json` (`accepted`). Inputs the old code rejects have a summary entry and no document.

For `cargo xtask fixtures --check`: (1) load each document with the new loader, (2) compute the same facts with the new code, (3) compare field by field as above, (4) verify `MANIFEST.json` hashes for every file so nobody edits a fixture by hand. The generator's own `--check` (below) covers step 4 against the old code.

## Regenerating

Needs `cargo`, `python3`, the old checkout at `/home/ubuntu/git/drafft-ink` (commit `8ce40ab`, never modified; override with `OLD_REPO`) and, only for the Excalidraw inputs, `/home/ubuntu/git/excalidraw` (override with `EXCALIDRAW_REPO`).

```
cd tests/fixtures/generator
./regen.sh            # runs author_excalidraw.py, builds fixtures-gen, writes tests/fixtures/**
./regen.sh --check    # builds, regenerates in memory, fails if any file on disk differs
```

`regen.sh` is shorthand for:

```
python3 author_excalidraw.py
CARGO_TARGET_DIR=/tmp/old-target-fixtures cargo build --release      # in tests/fixtures/generator
/tmp/old-target-fixtures/release/fixtures-gen [--check] [--out DIR]
```

The first build compiles the old crates and takes a few minutes. The generator is `tests/fixtures/generator/` (`src/fixtures.rs` builds the documents, `canon.rs` canonicalizes, `facts.rs` derives the facts, `main.rs` orchestrates and writes `MANIFEST.json`). It is its own cargo project with an empty `[workspace]` table, so the new repo's workspace ignores it. A symlink at `/home/ubuntu/git/drafftink-spikes/fixtures-gen` points to it. Output is deterministic: repeated runs, and separate processes, give identical bytes (checked: `--check` passes on repeated runs; gzip headers have mtime 0; Loro snapshots use a fixed peer id).

The generator uses fixed seeds, its own splitmix64 PRNG (no crate dependency for randomness), and sets every shape's `seed` explicitly. The old seed counter is never used for shapes.

## Provenance

- `excalidraw/fixture_library.excalidrawlib` is copied unmodified from Excalidraw (`packages/excalidraw/tests/fixtures/fixture_library.excalidrawlib`, MIT, Copyright (c) 2020 Excalidraw, checkout `/home/ubuntu/git/excalidraw` at `701b901a`). It is a version 1 library. It is the only `.excalidraw` or `.excalidrawlib` file in that repository.
- Every other file in `excalidraw/` is **authored for this repo** by `generator/author_excalidraw.py`. The element field sets follow Excalidraw's element base (`packages/excalidraw/tests/fixtures/elementFixture.ts`, MIT) and the scene envelope. Excalidraw's repository has no scene files for bindings, groups, frames, text containers or elbow arrows, so these were written to cover them:
  `basic_shapes` (all element types, fill and stroke styles, roughness 0 to 2, opacity, colors, deleted and unsupported elements), `bindings`, `groups`, `frames` (frame and magicframe), `text_containers` (shape and arrow labels, font ids 1 to 8, fixed-width text), `elbow_arrows`, `closed_paths`, `images_and_unsupported`, `minimal_fields` (missing fields and odd colors), `no_elements` and `not_json` (rejected), `grouped_library` (nested `groupIds`, unnamed and empty items), `v1_library`, `not_a_library`, and `clipboard/{basic,not_clipboard,empty}.json`.
- `mermaid/*.mmd` are hand-written against the syntax the old importer parses (`docs/formats.md`, Mermaid section). `mermaid/rejected/` holds inputs the old importer returns `None` for.
- Images in `images.json` are tiny gradients encoded with the `image` crate (PNG, JPEG q85, lossless WebP).

## What this does not cover

- Browser storage (IndexedDB, `localStorage`) and the collaboration wire format are described in `docs/formats.md`, but there are no captured files for them.
- Only the shape fixtures are covered by PNGs (`render/`).
- Old Loro snapshots of the app's own sync path (`sync_to_crdt`, which clears and re-adds all shapes) are not captured. `legacy/crdt/*.loro` use plain `add_shape`.
