# Rendering: `drafftink-render` (L2)

`render` turns scene elements into Vello scenes. It builds for wasm32 and never creates a GPU device or a
surface: the `gpu` crate renders the scenes it produces. It depends on `scene`, `geom`, `sketch`, `typeset` and
`profile`, plus `vello`, `peniko`, `kurbo` and `image`.

Related: `docs/performance.md` (the perf contract), `docs/typeset.md`, `docs/stack.md` §2 (why culling and cached
fragments).

## 1. The model

```
Element ──ElementPainter──▶ Fragment (vello::Scene in the element's local frame)
                                │ cached by id + version  (FragmentCache, bounded by bytes)
SceneRead + SpatialIndex(paint_bounds) + View
                                │  SceneComposer::compose
                                ▼
                     world Scene = grid, then visible fragments in z order,
                                   each appended with  view × element_to_world
Overlay ──paint_overlay──▶ overlay Scene        combine(world, overlay) ──▶ the scene Vello renders
```

A fragment does not know where the element is, how it is turned, or what the camera does. Panning, zooming,
moving and rotating therefore never encode anything; resizing and restyling do.

### Local frame

The origin is the top-left of the element's unrotated box. `element_to_world(e)` is the transform to world space
(rotation about the box centre; mirror flags apply to images only, because every other kind mirrors its
geometry). The composer appends `view.world_to_device() * element_to_world(e)`.

## 2. Painting (`ElementPainter`)

The look is the old app's, checked against its PNGs (§7):

| Kind | Drawing |
|---|---|
| rectangle, diamond, ellipse, line, arrow, freehand | `sketch` output: fills first (solid; a 15% tint plus hatch lines clipped to the fill outline for patterns; ink for filled heads and pressure outlines), then strokes (double stroke for sloppy styles, dashes `4w,2w` and `w,2w`). `Element::style.opacity` is multiplied into the alpha of every paint (truncating, like the old `stroke_with_opacity`). |
| arrow heads | the sketch generator draws arrow, bar, circle, triangle, diamond (filled or outline). The six cardinality (entity-relationship) markers are drawn by the painter as clean solid strokes. |
| highlighter (`FreehandData::highlighter`) | one wide round-capped stroke inside a layer whose alpha is the colour's alpha, so a stroke that crosses itself does not darken. |
| text | `typeset` layout, one `draw_glyphs` per glyph run, decorations as rectangles, per-character colours from `runs`. Synthetic italic through the run's skew. |
| math | `typeset` math layout (cached per formula), glyphs in the two fonts, rules as rectangles, formula colour `\color` or the stroke colour. A formula that does not parse draws the old pale-red box. |
| image | the decoded picture as a textured rectangle with the crop applied; opacity as the brush alpha; a border when the stroke is visible. A grey box with a cross is the placeholder (a red-tinted one for a picture that cannot be decoded). |
| frame | rectangle border (and a solid fill), and the name above the top-left corner clipped to the frame width, drawn in the stroke colour. |

Dark theme (`RenderOptions::theme`) is a colour mapping applied while encoding: the CSS filter
`invert(93%) hue-rotate(180deg)` that Excalidraw uses, so `#ffffff` becomes `#121212`. Pictures are not mapped.
Each theme has its own fragments, so switching back and forth costs nothing after the first pass.

### One text path for measuring and drawing

`text_spec(&TextData, stroke, box_width)` and `math_spec(&MathData)` convert an element into the layout input.
**Everything that stores a text box size must use them**, so that the box the editor measured is the box that is
drawn. The line height is `font.line_height` times the font size (Excalidraw's 1.25 by default; the old app used the
font's own line height). The colour given to the layout is the stored stroke colour: opacity and theme are applied
when the layout is drawn, so changing either never lays out again (the old cache ignored opacity and showed stale
text).

### Bounds

`paint_bounds(&Element)` is the world box of everything the element can paint: stroke, sketch jitter (it grows
with roughness and with the longest segment), arrow heads, text overhang, the frame label. **The spatial index
handed to the composer must hold these**, not hit-test boxes. `build_paint_index(&store)` builds it and
`update_paint_index(&mut index, &store, &changes)` follows a commit. A property test checks that no sketch kind,
fill, roughness, head or stroke width paints outside its margin. Nothing is measured during render: text and math
sizes come from the layouts and from `transform`.

## 3. The cache (`FragmentCache`)

Keyed by element id (one table per theme). Lookup:

1. entry version equals the element's version: **hit** (one hash lookup);
2. newer version: compare the content hash (`paint_hash`: kind data, box size, style; not position, angle, group,
   frame, name, locked, bindings). Equal: **reuse** (move, rotate, rename, lock); different: **miss**, encode again;
3. version 0 (an element that did not come from a store) is always compared by content.

A resize changes the size and misses; a recolour misses but keeps the sketch geometry (the sketch key has no
colour); a text opacity change misses without a new layout. Memory is bounded by `max_bytes` (an estimate of the
encoded streams); passing it drops the least recently used entries down to 90% in one batch, so a hit does no list
work. Call `SceneComposer::apply_changes(&ChangeSet)` for each commit to free removed and hidden elements early, and
`cache_mut().clear()` when a different document is loaded (versions of two documents mean nothing to each other).

Counters (`drafftink-profile`): `cache_hit` (hits and reuses), `cache_miss`, `elements_encoded` (fragments
appended plus cheap stand-ins), `elements_culled`, `sketch_regens`, `relayouts` (from `typeset`), `scene_builds`,
`spatial_queries`; gauges `elements_total`, `elements_visible`, `cache_entries`. `FrameReport` gives the same per
frame, split into hits / reuses / misses.

## 4. Composing a frame (`SceneComposer`)

```rust
let mut composer = SceneComposer::new();
let report = composer.compose(&store, &index, &view, &mut typesetter, &mut world_scene);
```

1. **Who is visible.** If the view contains the whole index, nothing is queried; otherwise one R-tree query. A
   small result is sorted by `(index, id)`; a result larger than a quarter of the store is found by walking the
   store in order (no sort). Cost follows the visible count.
2. **Frames.** A child of a clipping frame outside the frame is culled; one that crosses the frame edge is drawn
   inside a clip layer, and neighbours in z order share a layer.
3. **Level of detail** (`LodConfig`, in device pixels along the longer side of the paint bounds): below 0.75 not
   drawn; below 3 a flat box; a text with a font size under 3.5 px as grey bars, one per line. At most
   `max_cheap_per_frame` (20,000) stand-ins are drawn per frame, so a fully zoomed-out 100k scene stays bounded.
   Stand-ins never touch the fragment cache. `RenderOptions::for_export` turns it off.
4. **Fragments.** `get_or_paint`, then `Scene::append(fragment, view × element_to_world)`.

Other calls: `set_hidden` (the text being edited), `set_grid`, `compose_transient` (a shape being drawn, uncached,
appended after `compose`), `background(&store)` (the document colour in the current theme, for Vello's base
colour), `set_trace`/`drawn_order` (ids drawn, for tests).

## 5. Overlay, grid, pictures

**Overlay.** `paint_overlay(&mut scene, &Overlay, &View, &OverlayStyle)` draws selection boxes with handles (the
handle set comes from `geom::select`), point handles of lines, the hover outline, binding highlights, the marquee, the lasso path,
smart guides, the snap point and remote cursors. Sizes are logical pixels, so handles keep their on-screen size at
any zoom; a dashed box that would need more than 2,000 dashes is drawn solid. The canvas keeps the world scene and
rebuilds only this one when the pointer moves; `combine` joins them (a memcpy of the encodings).

**Grid.** `GridSpec { style, spacing, color }` with styles lines, horizontal lines, crosses and dots. The spacing
is `base × 2^k` with the smallest `k` that keeps cells at least 8 device pixels apart and the count under 400 lines
or 6,000 points, so the cost is bounded at any zoom (the old renderer emitted about 520,000 cells at zoom 0.1).
It is one path in device space, one draw call. `grid_spacing` gives the spacing so snapping can agree with it.

**Pictures.** `ImageCache` holds decoded RGBA by `FileId`, bounded by bytes (192 MiB default), least recently used
first. `DecodePolicy::Inline { pixels_per_frame }` decodes inside the frame up to a pixel budget (always at least
one picture); `DecodePolicy::Deferred` records `DecodeRequest`s for the host to decode on a worker with
`decode_image` and hand back with `complete`. Until then the element shows the placeholder and its fragment is
served from the cache; when the picture is ready (or its blob arrives) the fragment is encoded again. A picture
that fails to decode is remembered, so a broken file costs one decode. Limits: 16,384 px a side, 256 MiB per decode.

## 6. Hostile input

Non-finite numbers, huge sizes and nonsense views are handled without panicking: an element the index refuses is
never drawn; `View::is_valid` guards the composer; dashed strokes with more than 20,000 dashes are drawn solid;
sketch and typeset enforce their own limits. `hostile_views_and_elements_never_panic` exercises this.

## 7. Tests and fidelity

| File | What |
|---|---|
| `tests/compose.rs` | culling against brute force, z order, both collection strategies, warm pan encodes nothing, move/rotate reuse, resize/recolour/opacity, cache bound and LRU, themes, level of detail, frames, hidden, hostile views, index follows commits |
| `tests/bounds.rs` | property tests: nothing paints outside `paint_margin` |
| `tests/painter.rs` | draw order, colours, tint, opacity, dark mapping, heads, highlighter, text runs, math, frames, dashes |
| `tests/grid.rs` | the grid is bounded for any view |
| `tests/images.rs` | decode limits, inline and deferred policies, late blobs, broken pictures, byte bound, crop |
| `tests/overlay.rs` | overlay parts, constant size, independent of the document |
| `tests/gpu.rs` | feature `gpu-tests`: pixel checks on a real adapter |

`cargo test -p drafftink-render --features gpu-tests --test gpu` renders offscreen with Vello and compares with
`tests/fixtures/render` (the old renderer's PNGs, per-channel tolerance 16). Without an adapter the tests skip;
`DRAFFTINK_REQUIRE_GPU=1` makes that a failure. `RENDER_TEST_OUT=dir` writes our PNG and a diff mask. Measured on
the NVIDIA L40S (share of pixels beyond the tolerance):

| Fixture | Beyond tolerance | Notes |
|---|---|---|
| styles_rectangle | 0% (max channel difference 2) | outside the exceptions below |
| styles_freehand | 0% (max 1) | |
| text_char_colors | 0% (identical) | |
| all_shapes | 0.16% | |
| math | 0.9% | glyph runs here, outline fills in the old app |
| text_fonts | 0.9% | |
| groups_nested | 1.1% | old group rotation was cosmetic |
| images | 1.8% | sampling of the scaled picture |

Also: the composer's frame is pixel-identical to painting every visible element fresh (cold and warm cache, three
zoom levels), and a moved element looks like one encoded at the new place.

Known differences from the old renderer (the first two are excluded from the comparison, by region):

- The position of the **dots of a Dots fill** differs from the old PNGs (same density and look). The sketch crate's
  tests match the old geometry for its own cases; this fixture's seeds do not. To be checked in `sketch`.
- A rectangle with a **zero or negative size** was drawn unnormalised by the old app (a dot; a differently jittered
  outline). The model normalises the box, so the zero-size one draws nothing.
- Frame labels scale with the zoom (they are part of the fragment); Excalidraw keeps them a constant screen size.
- Bound-text vertical alignment inside a taller box is not applied.
- Pressure: a freehand stroke is a variable-width outline when its `pressure_mode` is not `uniform` and `pressures`
  has one value per point; otherwise it is the sketchy line. The calligraphy brush and the simulated pressure are not
  drawing modes. They shape the samples while the stroke is drawn (`edit/src/brush.rs`, as in the old app), and the
  stored points and pressures are what the painter draws.

## 8. Cost (criterion, `cargo bench -p drafftink-render --bench compose`)

Release build, the research box (64 cores, **load average 17 to 20 during the runs**, so absolute numbers are
approximate), mixed scene of rectangles, ellipses, diamonds, lines, arrows, freehand, text and math. "Visible" is
the number of elements the view shows. Cold: empty fragment cache and sketch cache, warm typesetter.

| Case | 1k | 10k | 100k |
|---|---|---|---|
| zoomed in (30 visible), warm | 6.5 µs | 6.7 µs | 6.3 µs |
| zoomed in, cold | 139 µs | 93 µs | 102 µs |
| pan, warm | 6.5 µs | 7.3 µs | 6.2 µs |
| drag one element (edit + index + compose) | 6.8 µs | 7.9 µs | 10.3 µs |
| everything visible, full detail, warm | 0.20 ms (was 0.25) | 3.3 ms (was 9.9) | not run |
| everything visible, full detail, cold | 3.6 ms | 58.6 ms | not run |
| everything visible, level of detail, warm | 0.21 ms (was 0.25) | 3.8 ms (was 7.9) | 22.6 ms (was 49.6) |
| everything visible, level of detail, cold | 3.3 ms | 50.0 ms | 57.7 ms |

Old renderer, `build_scene` (`docs/performance.md` §7.2, `opt-level=z`): zoomed in with almost everything culled
0.36 ms, 1.0 ms and 10.4 ms at 1k, 10k, 100k; everything visible 1.6 ms and 15.1 ms at 1k and 10k. The culled case is
now independent of the document size (about 6 µs for 30 elements). The old "everything visible" figure re-encoded
the whole scene each frame; the warm number here is an append of cached fragments, and the cold number includes
sketch generation for every element. At 10k all visible, the warm cost is the copy of about 10k fragments; the level
of detail does not help much there because most elements are still above the 3 px box threshold at that zoom. At
100k, 20,000 boxes are drawn and the rest dropped. The walk over the store and the index (two hash lookups per
element, 23 of the 33 ms in a profile) is gone: the composer keeps the z order of the whole scene between frames,
keyed to the store's `version_stamp` and the index's `generation`, and rebuilds it when either changes. A pan or a
zoom of a large scene therefore costs the candidate loop only (22.6 ms at 100k, the warm rows above). The first frame
and the first frame after an edit still order the scene once (`Counter::ElementsScanned` counts it); a one-shot
frame, as in an export, pays for the order and is about 12 ms slower at 100k (cold row: 57 ms, was 45 ms).

Grid (`cargo bench`, 1920x1080): lines 7 to 18 µs, dots 0.15 to 0.4 ms at any zoom. Overlay with a selection and a
marquee: 12 µs.

## 9. Not done

- No CPU fallback painter (`stack.md` §7): `vello::Scene` is used directly. A `Painter` trait would sit between the
  painter and Vello if the fallback ships.
- Text editing overlay (caret, selection rectangles): `typeset` has the geometry; drawing it belongs with the editor.
- Distance badges on smart guides (text on the overlay).
- Pictures are not downscaled for display; a 16k picture keeps its full decoded size until evicted.
