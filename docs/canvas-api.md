# drafftink-canvas: the canvas library

`drafftink-canvas` (layer L3) is the reusable canvas library: Konva's ideas (a scene, a camera, events that bubble, a transformer) on top of a spatial index with exact hit testing, pointer gesture recognizers and a description of the overlay. It works over any `SceneStore`, including the in-memory store, so a third party can use it without the editor, without Loro and without a UI toolkit. It builds for `wasm32-unknown-unknown` and takes time only from the events it is given.

It knows nothing about commands. It turns input into typed `Intent`s ("a drag started on this handle"); the editor engine (L5) decides what an intent means and runs a command or a gesture. Design context: `docs/plan.md` §1, §4; the Konva comparison is at the end of this page.

| Layer | Crate | Role here |
|---|---|---|
| L0 | `geom` | R-tree, outlines, hit tests, handle math, snapping. Re-exported as `drafftink_canvas::geom` |
| L1 | `scene` | `Element`, `SceneStore`, `ChangeSet`, `MemoryStore`. Re-exported as `drafftink_canvas::scene` |
| L3 | `canvas` | this page |
| L2 | `render` | draws the scene and, in its overlay pass, an `Overlay` (see "Overlay") |

## Quick start (Rust)

```rust
use drafftink_canvas::scene::{Element, ElementId, ElementKind, MemoryStore, SceneRead, SceneTx};
use drafftink_canvas::geom::kurbo::{Point, Size};
use drafftink_canvas::{Canvas, EventKind, InputEvent, Intent, PointerEvent, PointerPhase, Selector, Viewport};

let mut canvas = Canvas::new(MemoryStore::new(), Viewport::new(Size::new(800.0, 600.0), 1.0));

canvas.transact(|tx| {
    let z = tx.next_top_index().unwrap();
    tx.insert(Element::new(ElementId::new("box").unwrap(), ElementKind::Rectangle, z).with_bounds(0.0, 0.0, 100.0, 60.0))
}).unwrap();

canvas.on(EventKind::Click, "rectangle".parse().unwrap(), Box::new(|e| println!("clicked {:?}", e.target)));

// The shell feeds events; the canvas answers with intents and fires the listeners.
let intents: Vec<Intent> = canvas.handle_input(&InputEvent::Pointer(
    PointerEvent::mouse(PointerPhase::Move, 400.0, 300.0, /* ms */ 0.0)));

// Draw only when something changed.
let dirty = canvas.take_dirty();      // { scene, camera, overlay }
```

## Coordinates and units

* **World** space is the scene's: y points down, angles are radians, clockwise on screen.
* **Screen** space is logical pixels (CSS pixels in a browser) with the origin at the top-left of the canvas. Pointer events, hit tolerances, handle sizes and every other pixel threshold use it. `Viewport::dpr` matters only for the final GPU transform, `Viewport::world_to_device`.
* **Time** is a number of milliseconds on the caller's clock, carried by each event. The library never reads a clock, so a recorded event stream replays to the same intents.

## 1. Viewport and camera

`Viewport { camera, size, dpr }` with `Camera { scroll, zoom }`. `scroll` is the world point at the centre of the viewport (the form of `view.get` / `view.set`), `zoom` is logical pixels per world unit, clamped to `0.1..=30` (`MIN_ZOOM`, `MAX_ZOOM`). Nothing animates: a caller animates by calling `set_camera` with `Camera::lerp` results.

| Method | Meaning |
|---|---|
| `world_to_screen`, `screen_to_world`, `world_to_screen_affine`, `screen_to_world_affine`, `world_to_device` | transforms |
| `px_to_world`, `world_to_px` | lengths |
| `visible_rect`, `visible_rect_padded(px)`, `is_visible(rect)` | what is on screen, in world space |
| `pan_by_screen(delta)`, `pan_by_world(delta)`, `center_on(p)` | pan |
| `zoom_at(screen_point, factor)`, `set_zoom_at(screen_point, zoom)`, `set_zoom(zoom)` | zoom keeping the point under the cursor fixed; clamped, and a no-op at the limit |
| `fit_bounds(rect, padding_px, max_zoom)` | centre and fit; a point only centres |
| `scroll_into_view(rect, padding_px)` | least movement that shows a rectangle |
| `set_size`, `set_dpr`, `set_camera` | each returns whether anything changed; non-finite input is refused |

## 2. Scene index and hit testing

`SceneIndex` holds a `geom::SpatialIndex` of element bounds (outline plus half the stroke plus arrowheads). It is a cache, built in bulk once and then **updated incrementally from `ChangeSet`s**: per changed element it recomputes bounds only when a geometry field changed (position, size, angle, flip, kind data, style, deleted); a z-order, lock, group, name or custom change costs a field test. It is rebuilt in bulk only when one change set touches more than half of a scene of over 2048 elements (loading a document). `SceneIndex::verify(&scene)` compares the index with a full recomputation; the property test runs it after every random edit.

`Canvas` subscribes to its store, so every commit (local, remote, undo) reaches the index before the next query. Use `Canvas::transact` or the `store_mut()` guard to write; the guard syncs the index when it drops.

### Hit rules

| Kind | Hit when |
|---|---|
| rectangle, diamond, ellipse | near the outline; also inside when the style has a visible fill (alpha > 0) |
| line, arrow | near the polyline (a `round` path uses the same Catmull-Rom spline the renderer draws); inside only for a closed line with a visible fill |
| freehand | near the polyline; inside only for a closed stroke with a visible fill |
| text, math, image | anywhere in the rotated box |
| frame | near its border only. A child of a clipping frame cannot be hit outside the frame |

Reach around an outline is `tolerance_px / zoom + stroke_width / 2` world units: thin lines stay clickable at any zoom, and a fat marker stroke is hit across its full width. Defaults: 5 px for mouse and pen, 12 px for a finger (`CanvasConfig`). Rotation and flips are exact (box shapes are tested in their own frame). Deleted elements are not in the index; locked ones are skipped unless `HitOptions::include_locked`; `HitOptions::filter` hides elements from one query (for example the dragged selection when looking for a binding target).

* `hit(point)` returns the topmost live hit by `(z index, id)`; `hit_all(point)` returns all of them, topmost first. Both return `Hit { id, part: Stroke | Fill | Body }`.
* `marquee(rect, MarqueeMode::Contain | Intersect)`: *Contain* needs the element's bounds inside the rectangle; *Intersect* selects what the rectangle touches, where an unfilled shape counts only if the rectangle crosses or contains its outline (a marquee inside an empty rectangle selects nothing). Frames are selected only by *Contain*. Locked elements are left out. Results are back to front.
* `lasso(&Lasso, MarqueeMode::Contain | Intersect)`: the same with a free-hand closed path (`drafftink_geom::lasso::Lasso`, inside by the non-zero winding rule) instead of a rectangle. *Contain* needs the whole outline inside the path; *Intersect* selects what the path crosses or surrounds, and a shape with a fill that the path lies inside. Frames are selected only by *Contain*. Locked elements are left out.
* `in_rect(rect)` / `Canvas::visible_elements(margin_px)`: the culling query, back to front.
* `unit_group(element, entered_group, is_group)`: the group a click selects. Without an entered group it is the outermost group. With one, it is the next group inside it, or none when the element is a direct member (then the element itself is the unit). The facade fills `Target::Element { unit_group }` with it and ignores groups with fewer than two live members.

Costs (release build, this machine; scene = the baseline grid of rectangles, ellipses and lines, half filled; the old app scanned all shapes: 60 µs at 1k, 0.70 ms at 10k, 9.1 ms at 100k):

| Operation | 1k | 10k | 100k |
|---|---|---|---|
| `hit` at a point inside the scene | 0.43 µs | 0.26 µs | 0.19 µs |
| `hit_all` | 0.46 µs | 0.28 µs | 0.26 µs |
| `hit` far outside the scene | 0.06 µs | 0.06 µs | 0.06 µs |
| culling query, 800x600 world units | 5.6 µs | 6.0 µs | 6.2 µs |
| `marquee` (intersect), 800x600 | 41 µs | 38 µs | 39 µs |
| `SceneIndex::apply`, one element moved | 0.27 µs | 0.27 µs | 0.31 µs |
| `Canvas::transact`, one element moved (store, subscription and index) | 1.2 µs | 1.4 µs | 1.4 µs |
| hover move through `handle_input` (pick, no change) | 0.51 µs | 0.33 µs | 0.25 µs |
| bulk build | 0.45 ms | 5.6 ms | 68 ms |

`cargo bench -p drafftink-canvas` reproduces them (`crates/canvas/benches/canvas.rs`).

## 3. Input and intents

The shell translates its events into `InputEvent`s: `Pointer(PointerEvent)`, `Wheel(WheelEvent)`, `PointerLeave { id }` and `CancelAll` (Escape, blur, a release over a panel). A `PointerEvent` has `id`, `kind` (`Mouse`, `Pen`, `Touch`), `phase` (`Down`, `Move`, `Up`, `Cancel`), `pos`, the changed `button`, the held `buttons`, `modifiers`, `pressure`, `tilt` and `time_ms`. A trackpad pinch is a wheel event with `ctrl` set (what browsers send); a native pinch gesture should be sent the same way. Everything serialises with serde, so the JS API takes and returns plain JSON.

`Recognizer` is the state machine; `Canvas::handle_input` runs it against the scene. The recognizer reads the world only through the `Picker` trait (viewport, "what is under this point"), so gestures are tested with a fake scene and scripted events (`tests/recognizer.rs`).

### Intents

| Intent | When |
|---|---|
| `Press { at, target, meta }` | a pointer went down, at once (editors select on press so the drag moves the right thing) |
| `Click`, `DoubleClick`, `LongPress`, `ContextMenu` | release without a drag; a second click in time and distance on the same target (after its `Click`); a hold (`tick`); the secondary button released without a drag |
| `DragStart { origin, at, target }` | the pointer moved more than the slop from the press. `target` is what was under the press. An `Empty` target is a marquee |
| `DragUpdate { origin, at, delta_screen, delta_world, target }` | each move; deltas are totals since the press |
| `DragEnd { …, cancelled }` | release, or an abort (Escape, pointer cancel, a second finger) |
| `Hover { from, to, at, cursor }` | **only when the target under an idle mouse or pen changes** |
| `Pan { delta, source }`, `Zoom { at, factor, source }` | wheel, two-finger touch, middle-button drag, hand tool. `Canvas` applies them to its viewport (clamping the zoom) before returning them |
| `ViewEnd` | a pan or pinch that lasted several events ended: report the camera now |

`Target` is `None` (pointer away), `Empty`, `Element { id, unit_group }`, `Handle(Handle)` or `LinkIcon { id }`. Handles beat elements, and the link icon of an element that has a link and is not selected beats what lies under it (the icon sits beside the element's top-right corner, `geom::link_icon`; `Target::element()` is `None` for it, `Target::without_link_icon()` reads it as `Empty` for a host that has no use for it). A click on it is the cue to open the link. `Handle` is `TopLeft | Top | TopRight | Right | BottomRight | Bottom | BottomLeft | Left | Rotate | Point { element, index } | Midpoint { element, segment }`. `Meta` carries pointer id and kind, button, modifiers, pressure, tilt and time. `Cursor` suggests the mouse cursor (resize arrows turn with a rotated box).

### Rules of the state machine

* One pointer owns a gesture. A second finger turns a one-finger gesture into a two-finger view gesture: a running drag ends with `cancelled: true`, then `Pan` (movement of the midpoint) and `Zoom` (ratio of finger distances, emitted when it differs from 1 by more than 0.001, accumulating below that) follow, pan first. When a finger lifts, `ViewEnd` is sent and the other finger is ignored until it lifts too: no click, no drag.
* Releasing a different mouse button than the one that started the gesture is ignored. A press from the pointer that already owns a gesture (its release was lost outside the window) aborts the old gesture (`cancelled: true`) and starts a new one.
* A middle-button drag pans. The primary button depends on `PointerMode`: `Select` (default) drags content or a marquee; `Hand` pans; `Draw` starts the drag at the first movement (`draw_slop_px`, 0 by default) and a press without movement is still a click. The app switches mode for the hand tool and for Space-drag.
* A long press needs a touch or pen (a mouse only with `long_press_mouse`), no movement past the slop, and `long_press_ms`. `Canvas::next_deadline()` says when to call `tick`; a release after the time has passed counts as a long press even without a tick. After a long press the release is not a click.
* Wheel: Ctrl or Cmd zooms by `1.1^(-delta_y / 100)` about the pointer (one 100 px notch is the old 1.1 step; a trackpad pinch sends small deltas and zooms smoothly; clamped to 0.25..4 per event); otherwise it pans, and Shift turns a vertical wheel sideways. `wheel_zooms` swaps the two. Lines and pages are converted to pixels.
* Positions inside the intents of one event use the viewport from before that event's own `Pan`/`Zoom`.
* A press is not undone by a later cancel: if a second finger lands, `Press` has already been sent. An editor that selects on press can undo that itself on `DragEnd { cancelled }`.

### Thresholds (`InputConfig`)

| Field | Default | Meaning |
|---|---|---|
| `drag_slop_px` / `drag_slop_touch_px` / `draw_slop_px` | 3 / 8 / 0 | movement from the press that starts a drag (strictly more than) |
| `double_click_ms` / `double_click_dist_px` | 500 / 5 | double click window |
| `long_press_ms` / `long_press_mouse` | 500 / off | `None` turns long press off |
| `middle_button_pans` | on | |
| `wheel_zooms` | off | plain wheel zooms, Ctrl wheel pans |
| `wheel_zoom_step` / `wheel_line_px` | 1.1 / 33 | |
| `pinch_epsilon` / `pinch_min_distance_px` | 0.001 / 4 | |

### Hover and idle

`Hover` is emitted only when the target changes, and `Canvas::take_dirty()` reports `overlay` only then. A thousand moves across one element give one intent and one redraw (`hovering_the_same_target_costs_no_redraw`). After a scene change the facade picks again under the resting pointer, so an element that moved away from under it produces a `Hover` without any input. `Canvas::tick` with nothing due returns nothing and dirties nothing.

## 4. Transformer

`Canvas::set_selection(ids)` tells the canvas what the transformer wraps (Konva's `transformer.nodes`); the canvas never decides what is selected. `SelectionFrame::of(scene, ids)` picks the frame:

| Selection | Frame | Handles |
|---|---|---|
| one box-like element | its rotated box | 4 corners, 4 edge middles, rotation |
| one line or arrow | its points | one per vertex (`Point`), one per segment middle (`Midpoint`, inserts a point) |
| two or more elements or a whole group | the axis-aligned box around all of them | as for a box |

Handle size, hit radius and the rotation offset are in screen pixels, so they scale with the zoom (defaults in `TransformerConfig`: drawn 16 px; hit radius 12 px for a mouse or pen and 24 px for a finger; rotation handle 25 px above the top edge). A side shorter than 40 px on screen loses its edge handle, a segment shorter than 40 px loses its midpoint. A selection whose longer side is under `tiny_frame_px` (32 px) on screen is *tiny*: a press inside it grabs the shape (a drag moves it) instead of a corner handle, because the handle hit areas would cover the whole body. The handles stay live outside the shape, and the rotation handle (25 px away) is never lost. The same rule holds for the editor, whose pointer path asks `SelectionFrame::hit`. Hit priority: the nearest handle within the radius, corners before the rotation handle before edges, vertices before midpoints. `SelectionFrame::resize(handle, pointer, ResizeOpts)` and `rotate(pointer, snap)` return what a drag would produce (via `geom::select`), for hosts without an editor; the editor turns the same drag into `element.resize` / `element.rotate` / `arrow.move_point`.

## 5. Overlay

`Canvas::overlay()` returns plain data in world space for the render crate's overlay pass:

| Field | Content |
|---|---|
| `selection` | one rotated box per selected element (omitted above `max_selection_boxes`, default 256) |
| `transformer` | the frame, the visible handles with positions, the handle size in px, the active handle |
| `hover` | the outline (`geom::Primitive`) of the hovered element, unless it is selected |
| `marquee` | the rectangle being dragged |
| `lasso` | the path being drawn (world points; the painter closes it). Set by the host: the canvas recognizer itself only drags a marquee |
| `guides` | smart guides, set by the host with `set_guides` |
| `bind_candidates` | outlines of elements to highlight, from `set_bind_candidates(ids)` |
| `remote_cursors`, `remote_selections`, `remote_trails` | from `set_remote_users` (`RemoteUser { id, name, color, cursor, selection, trail }`); selection boxes capped per user; the work is `remote_parts(store, users, cap)`, which a host with its own overlay calls too |
| `link_icons` | `LinkIcon { element, rect, to_element }` of the elements in view that have a link and are not selected |
| `zoom` | for line widths and dashes |

`trait OverlaySink { fn draw_overlay(&mut self, overlay: &Overlay, viewport: &Viewport) }` is the render-facing seam. The render crate sits below the canvas and cannot name `Overlay`, so the app (or a small glue module above both) implements `OverlaySink` for render's overlay pass once `p2-render` lands; until then nothing in this crate depends on render. `take_dirty()` returns `Dirty { scene, camera, overlay }`: re-encode changed elements, re-composite the cached fragments with the new camera, or redraw the overlay only.

## 6. Event dispatch (Konva-style)

```rust
let id = canvas.on(EventKind::Click, Selector::Id(box_id), Box::new(|e| { /* … */ e.stop_propagation(); }));
canvas.on(EventKind::DragMove, ".draggable".parse()?, Box::new(|e| println!("{:?}", e.delta_world)));
canvas.off(id);
```

Events: `PointerDown`, `Click`, `DoubleClick`, `LongPress`, `ContextMenu`, `DragStart`, `DragMove`, `DragEnd`, `PointerEnter`, `PointerLeave`, `TransformStart`, `Transform`, `TransformEnd`. Selectors: `Id`, `Kind`, `Name`, `Group`, `Stage`, `Any`; as strings `#id`, `rectangle`, `.name`, `group:g1`, `stage`, `*`. An element's `name` is a space-separated word list like a CSS class, so `.card` matches `name = "card big"`.

* The event goes to the target element's listeners, then to its groups from the innermost to the outermost (the `group_ids` stack), then to the stage. Listeners on one node run in registration order. `stop_propagation()` stops the bubble after the current node's listeners have all run (Konva's `cancelBubble`).
* Empty space and handles reach only the stage. A drag is delivered to the element that was pressed, wherever the pointer goes. A handle drag arrives as `TransformStart/Transform/TransformEnd` with `handle` set.
* `PointerEnter` and `PointerLeave` do not bubble. They fire on the element and on each group the pointer enters or leaves with it: moving between two members of a group fires nothing on the group.
* Callbacks cannot call back into the canvas (it is mutably borrowed while they run); push to a queue and handle it afterwards.

## 7. The facade

`Canvas<S: SceneStore>` owns the store, index, viewport, recognizer, dispatcher and overlay state.

| Group | Methods |
|---|---|
| construct | `new(store, viewport)`, `with_config`, `into_store` |
| store | `store()`, `store_mut()` (guard), `transact(f)`, `index()`, `sync()` |
| view | `viewport()`, `camera()`, `set_camera`, `set_viewport_size(size, dpr)`, `update_viewport`, `zoom_at`, `pan_by_screen`, `fit_to_content`, `fit_to_elements` |
| context | `set_selection`, `selection`, `selection_frame`, `set_entered_group`, `set_pointer_mode`, `config`, `set_config` |
| queries | `hit_at`, `hit_with`, `hit_all_at`, `pick`, `marquee`, `lasso`, `elements_in_rect`, `visible_elements` |
| events | `on`, `off`, `dispatch` |
| input | `handle_input`, `tick`, `next_deadline`, `hover` |
| overlay | `overlay`, `set_guides`, `set_bind_candidates`, `set_remote_users`, `take_dirty` |

## 8. How the editor uses it

The engine owns tools, selection rules and commands; the app routes intents. Intents map to the exception classes of `docs/exceptions.md`:

| Intent | Editor reaction |
|---|---|
| `Hover` | E1: set the cursor, show the hover outline, compute the bind candidate; no command |
| `Press` / `Click` on an element | `selection.select_at` with `unit_group` and the modifiers |
| `DragStart`/`Update`/`End` on an element | E2: `gesture.begin/update/end` of `element.move` |
| … on a `Handle` (box) | E3 / E4: `element.resize` / `element.rotate` |
| … on `Handle::Point` / `Midpoint` | E5: `arrow.move_point` |
| … on `Empty` | E9: marquee; at the end `selection.select_region` (the overlay already shows the rectangle) |
| `DoubleClick` on a group member | enter the group, then `Canvas::set_entered_group` |
| `Pan`, `Zoom`, `ViewEnd` | E10 / E11: already applied to the viewport; report the camera with `view.set` on `ViewEnd` and at most every 100 ms |
| drawing tools (`PointerMode::Draw`) | E6 / E7: use `DragStart.origin`, the `DragUpdate` stream and `meta.pressure` |
| eraser | E8: `marquee` or `hit_all_at` along the swept path |

## 9. The JS package

`packages/drafftink-canvas` is the npm package `drafftink-canvas`: this library in a browser, for developers who want "Konva, with a spatial index and exact hit testing" and do not want an editor. The wasm module comes from `crates/canvas-js` (`wasm-bindgen`, in-memory store, no Loro, no `doc`). Types: `src/index.d.ts` (hand-written shape model and `Stage`) and `dist/drafftink_canvas_js.d.ts` (generated, for the raw class `RawStage`). Quick start and build: `packages/drafftink-canvas/README.md`; runnable pages: `packages/drafftink-canvas/examples/`.

```js
import { Stage } from "drafftink-canvas";

const stage = await Stage.create(document.querySelector("canvas"), { defaultStyle: { sloppiness: "architect" } });
stage.add({ id: "box", type: "rectangle", x: 0, y: 0, w: 160, h: 90, style: { fill_color: "#a5d8ff" }, name: "card" });
stage.on("click", ".card", (e) => { console.log(e.target, e.world); e.stopPropagation(); });
stage.transact((tx) => { tx.add({ type: "text", x: 20, y: 30, content: "Hello" }); tx.update("box", { w: 200 }); });
stage.fitToContent({ padding: 60 });
const svg = stage.toSVG(), png = await stage.toPNG({ scale: 2 }), doc = stage.toJSON();
```

### Layer and dependencies

`drafftink-canvas-js` is an app-class crate (`Class::App` in `xtask/src/layers.rs`) and is also checked by `cargo xtask wasm` (`WASM_EXTRA`). It needs `canvas` (L3), `render` (L2), `scene` (L1), `typeset` (L1), `edit` and `io` (L4) and `gpu` (L6). `gpu` is L6, so the crate cannot be a library below L7; nothing depends on it, so it is a leaf. It uses `edit` for the editor's own rules (move, resize, rotate, group, z-order, delete) rather than a second copy, and `io` for SVG, PNG metadata and the file readers. It does not depend on `doc` or `loro` directly. `io` pulls Loro in only through `io::open`, which reads the old app's Loro snapshots; the package therefore routes formats itself (`src/load.rs`) and the linker drops the Loro engine (7.4 MB instead of 9.1 MB).

### What is on top of the Rust facade

| Part | Where | Role |
|---|---|---|
| `Stage` | `crates/canvas-js/src/web` | mounts a WebGPU surface on the canvas, owns the scene, answers queries, draws a frame when one is due |
| `Core` | `crates/canvas-js/src/core.rs` | everything but the browser: tested natively |
| shape model | `json.rs`, `ops.rs` | the scene's element layout with a flat shorthand; add, merge-patch, remove, batches |
| built-in pointer behaviour | `interact.rs` | select, drag, rubber band, transformer drags, with the editor's rules from `drafftink-edit` |
| overlay glue | `overlay.rs` | `canvas::Overlay` to `render::Overlay` (the `OverlaySink` seam of section 5) |
| export | `export.rs`, `load.rs` | JSON, SVG, PNG with the scene inside; the formats `load` reads |
| DOM glue | `src/input.js`, `src/index.js` | pointer, wheel, key and resize listeners; one `requestAnimationFrame` only when a frame is due; one timer for the next long press |

### Shapes

A shape is the scene's element, with the same field names as the document file (`docs/model.md`), so `stage.get(id)` is what `toJSON().elements` holds. `add` and `update` also take a flat shorthand: `x`, `y`, `w`, `h`, `angle` at the top level go to `transform`; `type` and every key that is not an element field (`points`, `content`, `latex`, ...) go to `kind`. Points of a line, arrow or stroke are relative to `x`, `y` and may be negative: the box is fitted to them. A text or formula gets the size its content needs. A key a rectangle, diamond or ellipse does not have (a typo such as `colour`) is an error.

| Call | Meaning |
|---|---|
| `add(shape) -> id` | on top of everything; a missing `id` is made; `defaultStyle` sits under the shape's `style`; each shape gets its own sketch seed |
| `update(id, patch)` | RFC 7396 merge patch: objects merge, `null` removes a field. The type cannot change |
| `remove(...ids)` | removed for good, with bound text and the children of a frame; no tombstone stays |
| `transact(tx => ...)` | `tx.add/update/remove/order/group/ungroup/lock/move`; one commit, or none when a step fails |
| `order(ids, "front"\|"back"\|"forward"\|"backward")`, `group(ids)`, `ungroup(ids)` | the rules of `edit::zorder` and `edit::group` |
| `get`, `all`, `find(selector)`, `count` | `find` takes the selectors of section 6 |
| `toJSON()`, `exportJSON()`, `load(doc \| text \| bytes)` | native document; `load` also opens a PNG or SVG made by `toPNG`/`toSVG`, `.excalidraw` and old drafft-ink JSON, and replaces the scene in one commit |
| `toSVG(opts)`, `toPNG(opts)` | `drafftink-io` SVG (sketch or clean, selection, embedded scene); PNG drawn offscreen with the export options of `render` (no level of detail), with the scene in a text chunk, `scale`, `padding`, transparent background, selection |

### Camera, queries, selection

`camera` (`{ scroll, zoom }`, clamped 0.1 to 30), `panBy`, `zoomAt`, `fitToContent({ padding, maxZoom, ids })`, `worldToScreen`, `screenToWorld`, `resize(w, h, dpr)` (automatic with a `ResizeObserver` unless `autoResize: false`; the device pixel ratio follows page zoom). `hit(x, y, { tolerancePx })`, `hitAll`, `marquee(rect, mode)`, `inRect(rect)`, `pick(x, y)` (handles first). `setSelection(ids)` tells the transformer what to wrap (section 4); `selection`, `setEnteredGroup`, `setPointerMode("select"\|"hand"\|"draw")`, `setHighlights(ids)`, `overlay()` (selection boxes, handles, hover outline and highlights as SVG path data, marquee, remote cursors, remote selections, link icons).

`addImage(source, { id, x, y, width, height, maxSide, name })` (async) adds a PNG, JPEG or WebP picture from a `Blob`, `File`, `ArrayBuffer` or bytes and returns the shape's id. `drafftink-io` identifies it by its first bytes (never by a name or media type), reads the natural size from the header and decodes it once, so a damaged or unsupported file rejects with plain words and changes nothing. The shape has the natural size unless `width`, `height` or `maxSide` say otherwise (one of width and height keeps the proportions; `maxSide` only scales down). The bytes are stored once per content in the scene's file table and travel with `toJSON()`.

`setRemoteUsers([{ id, name?, color, cursor?, selection?, trail?: [{ x, y, ageMs }] }])` draws the people in a room: a pointer with the name in a chip in their colour, an outline round what they have selected, and their laser trail. It replaces the list; unknown shape ids in `selection` are skipped.

### Events

`on(event, selector, callback)` with the events of section 6 under DOM-like names: `pointerdown click dblclick longpress contextmenu dragstart dragmove dragend pointerenter pointerleave transformstart transform transformend`. Without a selector the listener is on the stage. The callback gets `{ type, target, currentTarget, screen, world, deltaScreen, deltaWorld, modifiers, pointer, button, handle, linkIcon, cancelled, stopPropagation() }` (`linkIcon` is true when the pointer was on the link icon of `target`, not on the shape). The stage's own events: `selectionchange` (ids), `change` (the scene changed because of input), `intent` (every `Intent` of section 3, in the canvas crate's serde layout). `handleInput(event)` takes the canvas crate's `InputEvent` (JSON of section 3) for replay and for `interactive: false`.

**Listeners may change the stage.** A listener runs while the stage is mutably borrowed. A write made then (`add`, `update`, `remove`, `transact`, `setSelection`, camera, `on`, `off`, ...) is queued and applied when the event has been delivered, in order; `add` returns its id at once. A read (`get`, `hit`, `count`, ...) cannot be answered at that moment and throws "the stage is busy running a listener". The event carries what a listener usually needs.

### Built-in behaviour

`interaction` options (all on by default): a press selects the element or its outermost group (Shift toggles; empty space clears); a drag moves it, or the whole selection when it is part of it (`custom: { draggable: false }` opts a shape out); a drag on a handle of the transformer resizes (Shift keeps the ratio, Alt resizes from the centre), rotates (Shift snaps to 15 degrees) or moves a point of a line, and a midpoint handle inserts one; a drag on empty space is a rubber band (`"contain"`, `"intersect"` or `"off"`). The rules are `edit::transform` and `edit::linear`, so bound text follows its container and arrows follow their targets. A drag does not add up small steps: the first update records what the transform touches, every update restores that and applies the transform for the total movement, and Escape (or a second finger) restores it. A small shape (under 32 px on screen) is moved by a drag on its body, not resized: its handles are hit just outside it (section 4).

### Drawing and idle

The wrapper asks for a frame (`requestAnimationFrame`) only when the stage says one is due (`needsFrame`): the scene, the camera, the size, the theme or the overlay changed. An idle stage has no animation-frame loop, no timer except one for a pending long press, and touches no GPU resource. Moving the pointer over empty space, or inside one element, draws nothing (section 3). Only the elements in view are given to Vello (section 2). Browser test (`test/smoke.py`, headed Chromium on Xvfb with WebGPU on NVIDIA): 1000 shapes, a click event, a drag, hand-tool and wheel pans, 0 frames and 0 animation frames in 5 s idle, 150 moves over empty space with 0 frames, one change gives one frame.

Pictures that are still decoding make the stage ask for one more frame until they are done. A lost GPU device is recovered with a new device and one redraw (`isLost`, `recover`). Without WebGPU `Stage.create` rejects with a plain message; `errorPage: true` replaces the page with the explanation of `drafftink-gpu`.

## 10. What it does not do

* No selection model, no tools, no commands, no undo: the editor tier has them. `set_selection` only tells the transformer what to wrap.
* No drawing: `Overlay` is a description; `render` paints it.
* No palm rejection, no inertia, no gesture for three or more fingers (they are ignored), no pointer capture across windows. The shell supplies capture and `CancelAll`.
* Hit testing is geometric. A round-path line is tested against the spline, not against a sketchy (roughened) rendering, and arrowheads count only through the grown bounds, not the exact head outline.
* No Konva-style pixel hit graph, caching layers or filters: caching of drawn fragments is the render crate's job.

## 11. Konva compared

| Konva | Here |
|---|---|
| Stage / Layer / Group / Shape object tree | flat elements (`groupIds` stack, `frame_id`); no tree to maintain |
| Hit graph canvas, one colour per shape, ~41 MB per layer at 1080p | geometric hit test on an R-tree: O(log n) and no memory per layer; stroke tolerance in screen pixels |
| draws and hit-tests every node | spatial index plus viewport culling query |
| events with bubbling; `mouseenter/leave` | the same model; enter/leave fire on groups once, as the DOM's `pointerenter` |
| `find('.name')`, `#id`, class name | `Selector` (`#id`, `.name`, kind, `group:`), by id, kind, name or group |
| Transformer scales `scaleX/Y` | handles only describe a drag; resize semantics per kind live in the editor (`element.resize`) |
| rubber band built by the app | marquee state, overlay rectangle and `marquee(rect, mode)` provided |
| no selection model, snapping, undo, sync | selection and snapping are in the editor tier, undo and sync in `doc`; the canvas follows any store by change sets |
| `batchDraw` | `take_dirty()` with three reasons; idle is zero |
