# Interaction: from pointer input to commands

`crates/engine/src/interact/` is the layer between the recognizers of `drafftink-canvas` (`Intent`s) and the command registry. The shells (egui, web) and the test harness feed it input events. It reads the tool, the selection and the editing group from the `Session`, and it answers with commands, overlay state and an `Outcome`. A shell holds no gesture logic, and a test drives the same code with scripted events and no window.

The contract is `docs/exceptions.md` (which pointer inputs may bypass `execute`, and how) and `docs/commands.md` sections 1.10 (gestures) and 7.3 (pointer event to command). `docs/canvas-api.md` describes the intents this layer consumes. The layer has no UI toolkit type, takes all time as an argument, and builds for `wasm32-unknown-unknown`.

## 1. Using it from a shell

```rust
let mut session = Session::new(ports);
let mut interaction = Interaction::new();          // one per session; it follows the active tab

// every platform event, translated to a canvas InputEvent (pointer, wheel, leave, cancel):
let out = interaction.input(&mut session, &event);
apply(out);                                        // see below

// Esc and Enter belong to the layer while it has something running:
let out = interaction.key(&mut session, Key::Escape, now_ms);
if !out.handled { session.execute_from(Origin::Shortcut, "tool.cancel", json!({}))?; }

// one timer, only while there is a deadline:
if let Some(t) = interaction.next_deadline() { schedule(t); }       // then: interaction.tick(&mut session, now_ms)

// drawing:
let overlay = interaction.overlay(&session);       // InteractionOverlay: draw it above the scene
let camera = interaction.camera();                 // use this, not session.view(), to draw
```

| Call | Use |
|---|---|
| `Interaction::input(s, &InputEvent)` | the normal door |
| `Interaction::run_script(s, &[InputEvent])` | a recorded list of events: the entry point of the test harness's pointer script |
| `Interaction::handle(s, Intent, now_ms, mods)` | an intent that a test built by hand |
| `Interaction::key(s, Key, now_ms)` | Enter, Escape |
| `Interaction::tick(s, now_ms)` | long press, throttled camera report, laser fade |
| `Interaction::next_deadline()` | when to call `tick`; `None` means no timer |
| `Interaction::overlay(&s)` | `InteractionOverlay { canvas: Overlay (with `link_icons`), preview, faded, badge, laser }` |
| `Interaction::camera()` / `cursor()` / `is_busy()` | live camera, cursor, "a gesture or draft is running" |

`Outcome { redraw, cursor, ui_request, refusals, handled }`:

* `redraw` is what the interaction layer itself needs drawn (`Overlay` for a hover outline, preview, marquee or guides; `Camera` for pan and zoom). A command that ran also sets the session's dirty reason. The shell draws when either says so: `out.redraw.max(session.take_dirty())`.
* `cursor` is set only when the cursor changed.
* `ui_request` asks for UI that is not a command: `OpenTextEditor { element | draft at }`, `OpenEquationEditor { element }`, `OpenContextMenu { at, target }` (`target`: `Canvas`, `Selection` or `Locked { element }`), `OpenUrl { url }` (a link icon was clicked).
* `refusals` are the refusals of the commands the layer ran, in plain words, for a toast.
* `handled` is true when `key` used the key.

## 2. What it keeps, and how it stays in step with the session

* A `canvas::Recognizer` in the mode the effective tool needs: `Select` for select, text and math; `Hand` for hand (and Space-hold: `tool.hold`); `Draw` for the others.
* A `canvas::SceneIndex` of the working store for hit testing, kept by a scan that compares each element's `version` and transform when the store revision moved. It does not run while a gesture runs (live frames rewrite elements and nothing hit-tests during a drag).
* A `canvas::Viewport`. It follows the session camera (`view.set`, `view.fit`, scripts) unless the layer moved it itself and has not reported it yet.
* The selection frame (box or line points) for handle hits, cached by store revision and selection.
* `OverlayParts`: the marquee, the lasso path, guides, highlight ids, the preview element, faded ids, the badge, the active handle, the laser trail. Machines write them; `overlay()` turns them into data for `render`.

A tool change drops drafts and previews. A gesture that disappeared (`tool.cancel`, a script) is forgotten quietly. After a command changed the scene or the camera, the layer picks again under the resting pointer, so an element that moved away from under it stops being hovered.

## 3. Pointer to command, per tool

### Select tool (also text and math when the press lands on an element or a handle)

| Input | Result | Row |
|---|---|---|
| Press on an element | `selection.select_at` (replace; Shift toggles, Ctrl/Cmd `deep`). Shift on an already selected element toggles at the click, so a Shift-drag keeps the selection | |
| Click on empty canvas | `selection.select_at` (clears). Alt-click on an element: `cycle` | |
| Drag on empty canvas | marquee overlay; on release (at least 2 by 2 px) one `selection.select_region` (Shift adds) | E9 |
| Drag on empty canvas with Alt held; or any drag of the lasso tool on empty canvas or on an element that is not selected | lasso path overlay (`OverlayParts::lasso`); on release one `selection.select_lasso` (Shift adds). The lasso tool otherwise behaves as the select tool (handles, moving a selected element, click) | E9 |
| Drag an element or selection | gesture `element.move` (Alt `duplicate`, Ctrl/Cmd no snap, `pointer` for frame drops). Guides and frame highlight from the frame output | E2 |
| Drag one of 8 handles | gesture `element.resize` (Shift `keep_aspect`, Alt `from_center`), size badge | E3 |
| Drag the rotate handle | gesture `element.rotate mode=by` (Shift `snap_degrees` 15), angle badge. Double-click it: `element.rotate mode=to angle=0` | E4 |
| Drag a line or arrow point | gesture `arrow.move_point` (`bind` auto, Ctrl off, Alt inside), bind highlight | E5 |
| Drag a segment midpoint | `arrow.move_point insert=true`; on an elbow arrow gesture `arrow.move_segment`. Double-click an elbow segment handle: `arrow.release_segment` | E5 |
| Double-click an element of a group | `group.enter` | |
| Double-click text, shape, arrow, math | `text.edit_begin {id}` and `OpenTextEditor` (math: `OpenEquationEditor`) | |
| Double-click a line | `arrow.edit_points` | |
| Double-click an image, or `Enter` with one image selected (`Interaction::key`) | `image.crop_begin {id}`: the crop editor opens (below) | E17 |
| Double-click empty canvas | `text.edit_begin {at}` and `OpenTextEditor` | |
| Click on the link icon of an element that is not selected (select tool; the icon sits beside the top-right corner, `edit::link::icon_rect`) | runs `element.open_link` for that element and does not select anything. A web link comes back as `UiRequest::OpenUrl { url }` for the shell to open; an element link has moved the view and selected the target; an unsafe stored link is a refusal toast | Excalidraw `hitLinkElement`, `isPointHittingLink` |
| Right click, long press | selects what is under the pointer when it is not selected, then `OpenContextMenu`. When the topmost thing under the pointer is a locked element (the normal hit test skips it, so the pointer is over "nothing" or over something behind it), nothing is selected and the request is `OpenContextMenu { target: Locked { element } }`, so that the menu can offer Unlock (`element.unlock_all` with that id). Clicks, marquees and drags still skip locked elements. | Excalidraw `handleCanvasContextMenu` (`includeLockedElements`) |
| Esc, pointer cancel, a second finger | `gesture_cancel` or the marquee is dropped: no trace | |

### Crop editor (class B: a draft, one command at the end)

While `image.crop_begin` has opened the editor (`Observed::crop_editor`), the crop machine (`interact/crop.rs`) owns the pointer. It works in the image's own frame, so rotation and mirroring need no special case. The document does not change until the editor ends.

| Input | Result | Row |
|---|---|---|
| Drag one of the 8 handles of the crop window | the window's edges move with the pointer; Shift keeps the ratio; an edge within 6 px of the picture's edge snaps to it; the window stays inside the picture and is at least 10 units (or its starting size) and one picture pixel | E17 |
| Drag anywhere else on the picture | the picture slides under the window, which stays; it always covers the window; its edges snap to the window's | E17 |
| `Esc` during a drag | the window goes back to where the press found it; the editor stays open | |
| `Enter`, `Esc` (no drag running), click outside the window | one `image.crop {id, crop, bounds}`; nothing changed: `image.crop_end`. A click goes on to the select tool and selects what is under it | E17 |
| Click inside the window, press, double-click, right click | nothing | |
| A tool change, another selection, a tab switch | the session closes the editor; the draft is dropped | |

The overlay (`InteractionOverlay::crop`) carries the whole picture (drawn faded), the draft image (drawn over it), the window, the picture's outline, whether a drag runs (the shell then lays a rule-of-thirds grid over the window) and the pointer used last (a finger or a pen gets larger brackets and bars, and a pen grabs them from as far as a finger). The scene leaves out the real image while the editor draws its own.

Cursors in the editor: over a handle the resize arrow of that handle, turned with the image (a rotated image turns the arrows; a flip changes nothing, the handles sit on the window); over the picture, also the dimmed part, the open hand, and the closed hand while the picture is slid; elsewhere the arrow.

### Drawing tools (class B: preview, one command at the end)

| Tool | Input | Result | Row |
|---|---|---|---|
| Rectangle, diamond, ellipse, frame | drag | `element.create {kind, bounds, from_tool}`. Shift square, Alt from centre, grid and smart-guide snap. A drag under 1 unit makes nothing | E6 |
| Line, arrow | drag | `element.create {points: 2}` | E6 |
| Line, arrow | clicks | a draft with a rubber band; double-click, Enter or Esc ends it with one `element.create`. A click within 6 px of the last point is ignored | E6 |
| Line, arrow | any | `bind` auto, Ctrl off, Alt inside; the highlighted shape is the one that gets bound; Shift locks the angle; elbow arrows send their two ends | E5, E6 |
| Freehand, highlighter | drag | samples (pressure from a pen only) in a buffer; one `element.create {kind: freehand, points, pressures?, highlighter, simplify: 2}` | E7 |
| Eraser | drag | swept path; touched elements drawn faded; one `element.erase {path, radius: 10, units: screen}` | E8 |
| Laser | drag | local trail of up to 50 points, faded by the clock; `collab.laser` at most every 33 ms; nothing is stored | E12 |
| Text | click | `text.edit_begin {at | id}` and `OpenTextEditor` | |
| Math | click on empty canvas | `element.create {kind: math, at, from_tool}` | |

`from_tool: true` returns the tool to select after the element exists, unless `tool.lock` is on (the engine does it in the same undo step).

### View (class C)

| Input | Result | Row |
|---|---|---|
| Middle-button drag, hand-tool drag, Space-hold drag, wheel, two-finger drag | the layer's viewport moves at once; `Outcome.redraw = Camera` | E10 |
| Ctrl/Cmd wheel (factor 1.1 per notch), trackpad pinch, two-finger pinch | the viewport zooms about the pointer, clamped to 0.1..30 | E11 |
| End of a pan or pinch (`ViewEnd`) | `view.set {scroll, zoom}` | E10, E11 |
| During a pan or zoom | `view.set` at most every `REPORT_EVERY_MS` (100 ms); `next_deadline()` says when a held report is due | |

Any other intent first reports a pending camera, so a command never sees a stale camera.

## 4. Rules the code keeps

* No document, selection, tool or settings change except through `Session::execute` or the typed gesture methods. Hover, marquee, previews and the camera are layer state.
* A gesture frame builds typed arguments in place and calls `Session::gesture_update`. No JSON per pointer move.
* A gesture begins only if its final command would be allowed. A refusal at `begin` is shown once and the rest of the drag does nothing.
* Cancel is clean: Esc (`key`), `InputEvent::CancelAll`, a pointer cancel and a second finger end a drag with `gesture_cancel`, or drop the preview or the marquee. The ring gets no entry for a cancelled drag besides the gesture's own cancel line.
* Hover requests a redraw only when the outline it shows changes (E1). An idle layer has no deadline.

## 5. Testing without a window

`crates/engine/src/interact/testutil.rs` is the rig that the tests use: a `Session` with the fake text engine and a camera at zoom 1 over an 800 by 600 canvas (a screen position equals a world position), an `Interaction`, and scripted real `PointerEvent`s that go through the real `Recognizer` (`down`, `drag_to`, `up`, `hover`, `drag`, `click`, `double_click`, `key`, `tick`, modifiers in `rig.mods`). `normalized_doc()` returns the elements without ids, versions, z keys and seeds, so a gesture can be compared with the plain command run on a fresh twin: this is the equality test that `docs/exceptions.md` asks for, one per row (`exception_E<n>_*`).

Test files: `interact/interaction_tests.rs` (hover, pan, zoom, context menu, scripts), `interact/select_tests.rs` (select, move, resize, rotate, points, marquee, double-click), `interact/draw_tests.rs`, `draw_line_tests.rs` and `draw_stroke_tests.rs` (shapes, lines and arrows, freehand, eraser, laser, text, math).

**For the test harness (`crates/testkit`).** Keep an `Interaction` next to the `Session` and expose `pointer_script(events: Vec<InputEvent>)`, which calls `Interaction::run_script`. `InputEvent`, `PointerEvent` and `WheelEvent` are `serde` types, so a scenario can carry the events as JSON. Give every event its own `time_ms` for a deterministic run (double-click and long-press depend on it). Read the result through the usual probes (`engine.recent`, `element.query`); the `Outcome`s are returned for assertions on redraw, cursor and refusals.

## 6. Open points and deviations

* **Binding candidate cost.** `edit::binding::find_hovered` scans all elements per pointer move while an arrow end is placed. It should use the spatial index for large documents.
* **Index sync** compares `version` and transform per element when the store revision moved (an O(n) scan, outside gestures). A change feed from the store would remove the scan.
