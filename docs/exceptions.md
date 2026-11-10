# High-frequency gesture exceptions

AGENTS.md says: everything a user can do is a command, and high-frequency gestures are the only exception. This file is that list. It is closed: an input that changes state per pointer event and is not here must call `Session::execute` per user action, like everything else.

Related: `docs/commands.md` §1.10 (the gesture protocol), §6.2 (text editor keys) and §7.3 (the old pointer and window events).

## 1. What an exception is

An exception is an input that produces many small updates per second and where one `execute` per update is wrong or too costly. A full `execute` does a lookup, argument parsing, gates, a transaction with a rollback copy, an invariant pass over bindings and frames, an undo bookkeeping step, a ring entry and a CRDT delta. A mouse produces 60 to 125 events per second, a pen up to 240 and a touchscreen about 120. At 100 000 elements the frame budget leaves no room for a ring entry and an undo check per event, and an undo step per event would be wrong.

Every exception falls into one of five classes. The class says what may happen between the first and last event, and what the final step is.

| Class | Name | Between first and last event | Final step |
|---|---|---|---|
| A | Gesture protocol | The document changes live through `gesture.begin/update/end`. Typed in-process calls, no JSON, no ring entry or undo step per frame. | `gesture.end`, which equals one plain `execute` of the same command. One undo step. |
| B | Overlay preview | The document does not change. The recognizer draws a preview from its own state. | One plain `execute`. |
| C | View-local | The viewport's camera changes directly. No document, no history. | `view.set` with the final camera (also sent at most every 100 ms during the gesture). |
| D | Editor-local | A text editor owns content, caret and selection. | `text.edit_end`. One undo step. |
| E | Ephemeral | Presence data is shown but never stored. | A throttled command (`collab.set_pointer`, `collab.laser`), or none. |

Rules that hold for all classes:

1. **The final state is a command.** For every row below except E1, E12 and E15 (which change nothing or need no special path), the result of the gesture is also reachable with one plain `execute`, with the same arguments, and gives the same document. A test per row proves it: run the gesture through the recognizer in the testkit, run the final command on a fresh copy of the document, and compare (ids and timestamps aside). The table says which command.
2. **No private state changes.** A recognizer may hold preview state (points collected so far, the marquee rectangle, a hover) but never edits the document, the selection, the tool or the settings by any route other than a command or the gesture protocol.
3. **Cancel is clean.** Esc, window blur, pointer cancel, a second finger landing during a one-finger gesture, and a release over a panel all end the gesture without a trace. Class A calls `gesture.cancel` (state restored to before `begin`). Class B drops the preview. Class C keeps the camera where it is (cancelling a camera move is not a thing). Class D follows `text.edit_end` with commit=false.
4. **Collaborators see the end result.** Live frames are not broadcast (class A) and previews do not exist for others (B). Only the final command is a CRDT edit. Presence (E) is sent on its own volatile channel.
5. **The gesture is not in the ring frame by frame.** The ring gets one entry at the end with the frame count (`docs/commands.md` §1.9).
6. **Gates apply at the start.** A gesture begins only if the command it will end with would be allowed now (view-only, locked targets, text editing). Locked elements are not hit in the first place.
7. **One at a time.** There is at most one running gesture per session (class A). Class B/C/D/E recognizers hold their own state and may overlap with a gesture only as listed per row.
8. **Enforcement.** `cargo xtask exceptions --check` fails when (a) a command with `undo = Gesture` has no row here, (b) a row names a final command that does not exist, (c) the UI crate calls a document-changing session API outside `execute`, `execute_batch` and the `gesture_*` methods (the layering lint in `xtask layers`, which already forbids the UI from holding a `&mut Document`), (d) a row has no test whose name starts with `exception_<row id>_` (for example `exception_E2_move_equals_plain_command`).

## 2. The exceptions

### E1 Hover and cursor feedback

| | |
|---|---|
| Input | Pointer move over the canvas without a button. |
| Class | B (and read-only) |
| What happens | The recognizer hit-tests with the spatial index and picks the mouse cursor icon (resize arrows, move, grab, crosshair, default), the hover outline, and the arrow-bind candidate while an arrow is being drawn. |
| Reason | Hover changes no state. It must not request a redraw unless the hover result changed (the old app redrew a full frame on every mouse move, 53 to 68% of a core). A command per move would add a ring entry, a transaction and a dirty reason per event. |
| Final state | None. Nothing is committed. `element.at` exposes the same hit test to scripts. |
| Test | `exception_E1_hover_requests_redraw_only_on_change` (idle gate). |

### E2 Drag-move

| | |
|---|---|
| Input | Pointer down on a selected unit (or on an unselected one, which selects it first through `selection.select_at`), drag, release. Alt held at the start means duplicate. |
| Class | A |
| Protocol | `gesture.begin` `element.move` with dx=0, dy=0 at the press once the pointer moved 3 pixels. `gesture.update` with the total dx, dy since the press and the pointer position, for each frame. `gesture.end` at release. |
| Reason | The document must change live so that bound arrows, bound text, frame membership highlight and smart guides follow the drag. One `execute` per frame would create an undo step and a CRDT delta per frame, and the old app showed what that costs (a full snapshot every 10th event). |
| Between events | Frames are absolute: the engine restores the pre-gesture position of every touched element and re-applies (dx, dy) with snapping. Smart guides, grid snap and the frame-drop highlight come back in the frame output for the overlay. Remote edits to other fields of the same elements merge normally. |
| Final state | `element.move { ids, dx, dy, duplicate, snap, frame: auto, pointer }`. |
| Cancel | `gesture.cancel` restores the starting positions, including bound arrows. |
| Test | `exception_E2_move_equals_plain_command`, and a 1000-frame test with 10 000 elements that checks the frame time budget. |

### E3 Resize

| | |
|---|---|
| Input | Drag one of the eight handles of the selection box (corners, and edges which the old app lacked), release. |
| Class | A |
| Protocol | `element.resize` with the handle and the total displacement (dx, dy) of the handle since the press, `keep_aspect` (Shift), `from_center` (Alt), `snap`. |
| Reason | As E2. Text re-measures and bound arrows re-route per frame, so the document must change live. |
| Final state | `element.resize { ids, handle, dx, dy, keep_aspect, from_center, snap }`. |
| Cancel | `gesture.cancel`. |
| Test | `exception_E3_resize_equals_plain_command` for each handle and for a rotated element. |

### E4 Rotate

| | |
|---|---|
| Input | Drag the rotation handle, release. Shift snaps to 15 degrees. Double-click on the handle resets to 0 (that is a plain command, not a gesture). |
| Class | A |
| Protocol | `element.rotate` with `mode: by`. Each frame carries the total angle delta since the press and `snap_degrees`. |
| Reason | As E2. The angle badge and bound arrows follow live. |
| Final state | `element.rotate { ids, angle, mode: by, snap_degrees }`. |
| Cancel | `gesture.cancel`. |
| Test | `exception_E4_rotate_equals_plain_command`. |

### E5 Line and arrow point dragging

| | |
|---|---|
| Input | Drag an end handle, a vertex handle or a segment midpoint of the selected line or arrow, release. |
| Class | A |
| Protocol | `arrow.move_point` with the point index (or segment with insert=true), the pointer position, `bind` (auto, off with Ctrl, inside with Alt) and `snap`. |
| Reason | The bind candidate, its highlight, angle snap and the elbow route must update live. |
| Between events | The frame output has `bind_highlight` (the target under the pointer) and `guides`. |
| Final state | `arrow.move_point { arrow, index, to, insert, bind, snap }`. |
| Cancel | `gesture.cancel`. |
| Test | `exception_E5_point_drag_equals_plain_command`, with a bind, an unbind and an insert case. |

### E6 Drawing a shape (rectangle, diamond, ellipse, frame, line, arrow, text box, multi-point lines)

| | |
|---|---|
| Input | With a drawing tool: press, drag, release. For lines and arrows also click-click-click to add points, ended by double-click, Enter or Esc. |
| Class | B |
| What happens | The recognizer keeps the points and shows a preview built from them with the current style defaults, snapping (grid, angle, isometric for lines, smart guides) and the bind candidate. The document is untouched. On release (or on the ending click of a multi-point line) it calls one command. A drag shorter than 1 unit in a dimension creates nothing (old rule, E:714), and Esc drops the preview. |
| Reason | The element does not exist until the user finishes. Putting a half-made element in the document would show it to collaborators, put it in the undo history and bind arrows to it. A gesture frame per event is not needed because nothing else reacts to a shape that does not exist yet. |
| Final state | `element.create { kind, bounds \| points, arrow_type, start, end, from_tool: true, ... }`. |
| Cancel | Drop the preview. |
| Test | `exception_E6_draw_equals_plain_command` for every kind. |

### E7 Freehand and highlighter stroke

| | |
|---|---|
| Input | Press and drag with the freehand or highlighter tool; with a pen, pressure per sample. |
| Class | B |
| What happens | The recognizer appends samples (position and pressure) to a buffer and draws the stroke from the buffer (calligraphy smoothing and the pressure model run in the preview). On release it simplifies (RDP tolerance 2.0) and calls one command. |
| Reason | Pen input arrives at 240 Hz, and a stroke may have thousands of points. The element does not exist before the release. Real pen pressure comes from the pointer event (`pointerType: pen`, `pressure`), which the old app never read. |
| Final state | `element.create { kind: freehand, points, pressures, highlighter, simplify, from_tool: true }`. |
| Cancel | Drop the buffer. A second finger landing cancels a one-finger stroke. |
| Test | `exception_E7_stroke_equals_plain_command`, and a replay of a recorded pen stroke. |

### E8 Eraser

| | |
|---|---|
| Input | Press and drag with the eraser tool, release. |
| Class | B |
| What happens | The recognizer collects the swept path and, per frame, asks `canvas` which elements the path touches. Those are drawn faded (the soft-delete preview). Nothing is deleted yet. On release it calls one command. |
| Reason | One undo step per stroke is the correct behaviour, and a stroke over many elements must not delete half of them if it is cancelled. The old app pushed an undo step per drag event that hit something (E:1285). |
| Final state | `element.erase { path, radius, units: screen }`. |
| Cancel | Drop the path, restore opacity. |
| Test | `exception_E8_erase_equals_plain_command`. |

### E9 Marquee selection

| | |
|---|---|
| Input | Press on empty canvas and drag, release. |
| Class | B |
| What happens | The recognizer draws the rectangle and previews the units it would select. On release, if the rectangle is at least 2 by 2 pixels, it calls one command with the mode from the modifier (Shift adds). |
| Reason | Selection changes per event would make the properties panel and the overlay recompute for each event, and would put intermediate selections in the ring. |
| Final state | `selection.select_region { rect, mode, contain }`. |
| Test | `exception_E9_marquee_equals_plain_command`. |

### E10 Pan

| | |
|---|---|
| Input | Middle-button drag, Space-drag (`tool.hold` hand), hand-tool drag, mouse wheel and trackpad scroll, two-finger touch drag. (`PgUp`/`PgDn` paging is a plain command, `view.scroll_page`.) |
| Class | C |
| What happens | The shell moves the viewport's camera at once and requests a redraw. It reports the camera to the session with `view.set` when the gesture ends and at most every 100 ms during it, so `view.get`, screenshots and follow mode stay close to the screen. |
| Reason | Pan is the hottest input: it must cost one matrix update per event. It changes no document and no history, so there is nothing to protect with a transaction. The old solidcraft model needed a JSON round trip per orbit frame; we avoid it. |
| Final state | `view.set { scroll, zoom }`. The scriptable relative form is `view.pan { dx, dy }`. |
| Cancel | Not applicable. |
| Test | `exception_E10_pan_ends_with_view_set` (the session camera equals the viewport camera within 1e-9 after the gesture), and the idle gate: a pan frame requests exactly one redraw. |

### E11 Zoom: wheel, pinch, trackpad pinch

| | |
|---|---|
| Input | Ctrl/Cmd+wheel, trackpad pinch (wheel events with the ctrl flag in browsers, native pinch events), two-finger touch pinch. |
| Class | C |
| What happens | As E10. The zoom factor per wheel notch is 1.1 (in) or 0.9 (out), as in the old app. A pinch uses the ratio of the finger distances (applied when it differs from 1 by more than 0.001, as before). Zoom is clamped to 0.1..30 and keeps the point under the cursor or the pinch centre fixed. |
| Reason | As E10. |
| Final state | `view.set { scroll, zoom }`. The scriptable forms are `view.zoom_at { factor \| zoom, at }`, `view.zoom_in`, `view.zoom_out`. |
| Test | `exception_E11_zoom_ends_with_view_set`. |

### E12 Laser pointer

| | |
|---|---|
| Input | Press and drag with the laser tool. |
| Class | E |
| What happens | The shell shows a local trail of up to 50 points that fades over 0.33 s, driven by the clock, not by frame count (the old fade only advanced on redraws). The same points go to peers at most every 33 ms. Nothing is stored in the document or in history. |
| Reason | The trail is presence, not content. |
| Final state | None for the document. The scriptable form is `collab.laser { points }`. |
| Test | `exception_E12_laser_leaves_document_unchanged`. |

### E13 Scrubbing sliders and colour pickers

| | |
|---|---|
| Input | Dragging the opacity slider or a stroke-width slider, dragging inside a colour picker's saturation or hue area, dragging font size. A click on a swatch is not a gesture. |
| Class | A |
| Protocol | `gesture.begin` `style.set` on the first change, `gesture.update` with the full props for each change, `gesture.end` at pointer release (not when the popover closes). |
| Reason | The user sees the result live on the canvas, and a drag must be one undo step. The old opacity slider pushed an undo step on every tick (A:3639) and the other style props pushed none. |
| Between events | Live frames skip the history, the broadcast and the defaults update. The end run also updates the defaults and broadcasts. |
| Final state | `style.set { props, ids, scope }`. A click on a swatch is a plain `style.set`; the colour picker applies a value on release. |
| Cancel | `gesture.cancel` (Esc during the drag restores the old colour). |
| Test | `exception_E13_scrub_equals_plain_command`. |

### E14 Pointer-driven previews of a pending drag source

| | |
|---|---|
| Input | Dragging a library item from the library panel over the canvas, dragging an image file over the window before the drop, dragging a tab to reorder it. |
| Class | B |
| What happens | The shell draws a ghost of the item at the pointer. Nothing in the session changes until the drop. |
| Reason | The thing being dragged is not yet part of the document. |
| Final state | `library.insert { item, at }`, `doc.drop { items, at }`, `tab.move { tab, to_index }`. |
| Cancel | Esc or leaving the window drops the ghost. |
| Test | `exception_E14_drop_equals_plain_command` for each of the three. |

### E15 Hold-and-repeat keys

| | |
|---|---|
| Input | Holding an arrow key or `Mod+=` etc., so the OS auto-repeats at about 30 Hz. |
| Class | not an exception |
| What happens | Each repeat is one plain `execute`. `element.nudge` carries a `coalesce` rule so a burst becomes one undo step. Zoom keys and page-scroll keys are `NoUndo`. |
| Reason it is listed | To state that key repeat needs no special path. The cost per execute at 30 Hz is negligible. |
| Test | `exception_E15_nudge_burst_is_one_undo_step`. |

### E16 Text caret, selection, composition and in-text keys

| | |
|---|---|
| Input | Everything typed or pointed inside an active text edit: characters, IME composition (preedit and commit, dead keys, CJK input), caret movement, mouse selection and double-click word selection, Backspace and Delete (by character or word), Home and End, `Mod+A`, copy, cut and paste inside the text, the editor's own undo, and per-run colour changes made while a range is selected. |
| Class | D |
| What happens | The text editor widget (Parley editor in `typeset`/`ui`) owns the content, the caret, the selection and the composition. The session knows only that an edit is running (`text_edit` state: target id or draft, bounds) so it can apply the `while_text_edit` gates. The editor requests a redraw for caret blink with a timer, not with a per-frame loop. |
| Reason | A caret move or a keystroke is not a document change until the edit ends; IME composition cannot be expressed as commands (it needs preedit state, cursor ranges and cancel). The old app had no IME at all, and swallowed most `Ctrl+letter` chords, including undo. |
| Final state | `text.edit_end { commit: true, text, latex?, runs }` (one undo step including the element's creation). `Esc` and `Mod+Enter` end it from the keyboard. A click on the canvas outside the text runs the clicked command with the `Commit` gate (`while_text_edit = commit`), which commits first. Cancel is `text.edit_end { commit: false }`. |
| Not covered | Choosing a colour or style while the text edit is open does not call `style.set` (it refuses `text_editing`). The UI sends it to the editor, which applies it to the selected range or to the element, and `text.edit_end` receives the runs and `style`. A text element selected but not being edited uses `style.set` and `text.style_range` as usual. |
| Test | `exception_E16_edit_equals_plain_command` (typing a string then `text.edit_end` equals one `element.create` kind=text with that `text`), an IME replay test with a preedit sequence, and `text_edit_gates` (commands with `while_text_edit = no` refuse). |

## 3. Things that look like exceptions but are commands

| Input | Why it is not an exception |
|---|---|
| Click on a shape or on empty canvas | One event, one `selection.select_at`. |
| Nudge by arrow key | Key repeat is slow enough (E15). |
| Presence: the local pointer position | `collab.set_pointer`, a command that is `NoUndo`, `redraw = overlay`, throttled to 33 ms by time. It changes no document. |
| Pointer position for the status bar or the paste target | Read from the recognizer, passed as `$cursor` to `clipboard.paste`. |
| Autosave and gesture watchdog ticks | Engine ticks (`docs/commands.md` §1.12), not user actions. |
| Remote edits and remote presence | `Session::apply_remote`, same document path, not user actions. |
| Window resize | `view.set_viewport`, throttled by the shell to one call per frame. |
| Hover tooltips, the cursor shape | Pure UI, no state (E1). |

## 4. Adding an exception

Add it to this file in the same change as the code, with a class, a reason that is about cost or about the thing not existing yet, the final command, the cancel behaviour and a test id. A reason of "it is easier" is not accepted. The reviewer checks that the final command exists in `docs/commands.md` and that the equality test is real: it must compare the documents, not only check that the call returned.
