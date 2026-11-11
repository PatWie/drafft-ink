# End-to-end findings

What the browser and native suites (`docs/testing.md`, "Browser and native e2e") found in the apps, and what they do not cover.
Numbers are from `cargo xtask e2e` on the NVIDIA L40S under Xvfb, Chromium 1208 (Playwright 1.58), 2026-11-08.

## Bugs found

### F1. A paste event with the pointer over the canvas pasted nothing (fixed)

`ShellEvent::Paste` in `crates/app/src/app.rs` built the arguments of `clipboard.paste` as `{"at": {"x": .., "y": ..}}`. The
command takes a point as `[x, y]`, so every paste from the system clipboard (Ctrl+V of text, an image, Mermaid text, copied elements)
made while the pointer was over the canvas was refused with `bad_args` ("`at`: data did not match any variant of untagged enum At").
Without a known pointer the arguments were `{}` and the paste worked, which is why nothing else caught it. Found by
`specs/input/files-and-keys.spec.ts` with a recorded ring entry `clipboard.paste shortcut {"at":{"x":905.0,"y":605.0}} -> bad_args`.

Fixed in this branch: `crate::clip::paste_args` builds `{"at": [x, y]}`, with a unit test (`clip::tests::a_paste_event_names_the_pointer_as_a_point_array`).
The browser specs for pasting text and Mermaid text pass with the fix and would have failed without it only when the pointer had been
over the canvas first; the Ctrl+C / Ctrl+V spec covers the keyboard path.

### F2. While hovering, the cursor position the app keeps is missing or one pointer sample behind (open)

The position behind `$cursor` (Ctrl+V pastes "at the cursor", `docs/commands.md` section 6.1) comes from the last pointer sample the app has
processed. Observed with real mouse moves on a freshly loaded page, then Ctrl+V with a copied shape:

| pointer moves before Ctrl+V | `at` the paste received |
|---|---|
| one move | none (`{}`): the paste lands at the viewport centre |
| two moves, 250 ms apart | the first position |
| three moves, 250 ms apart | the second position |
| three quick moves | the first position |

After a frame has been drawn in between (for example by an earlier paste) the next move is recorded correctly. So the app lags one
sample behind while the page is idle, and a person who moves the mouse onto the canvas and presses Ctrl+V pastes at the viewport
centre. `crates/gpu/src/shell.rs` holds the hover event (`held_cursor`: "remember the event, tell egui nothing, draw nothing"),
which is the first place to look. Not fixed: the idle rules (no frame for hover) make this a design question for the shell owner.

The spec `keys: Ctrl+V pastes the copy centred on the pointer` holds the full assertion and is marked `test.fixme` with this finding as
the reason; it is reported as skipped. It must be switched on when F2 is fixed.

### F3. One Ctrl+V on the web pastes twice (open)

Ctrl+C then, a moment later, Ctrl+V with a shape selected gives **three** elements, not two. The recent ring shows two
`clipboard.paste` entries from origin `shortcut` for the one key press: the key binding (`Mod+V`, `docs/commands.md` section 6.1, arguments
`{}`) and the browser's `paste` event (`ShellEvent::Paste` in `crates/app/src/app.rs`, which runs `clipboard.paste` with the system
clipboard). The first Ctrl+C has written the system clipboard by then, so the paste event carries data. When Ctrl+V follows Ctrl+C within
a millisecond (a script), the system clipboard is still empty and only the binding pastes, which hides the bug from a fast test. Both
pastes land on the same spot, so the copies are stacked and the user sees one.

Copy has the same shape: the ring shows two `clipboard.copy` entries for one Ctrl+C. Not fixed: which path owns copy and paste on the web
(the binding or the browser events) is a decision for the shell owner. Native uses egui's paste event; the native suite does not press keys.

The spec `keys: one Ctrl+V pastes once` holds the assertion and is `test.fixme` with this finding as the reason. The plain copy and
paste spec accepts one or two copies so that it does not depend on the race.

### F4. A middle-button pan can paste on Linux (open)

After earlier tests in the same Chromium process, a middle-button drag (pan) ran `clipboard.paste` once (origin `shortcut`) and made an
undo step, so the pan pasted what the Linux primary selection held. Linux browsers paste the primary selection on a middle click into the
focused field, and the field here is the hidden text agent that `pointerdown` focuses (`crates/gpu/src/web/pointer.rs`,
`focus_for_pointer_down`). The run on a fresh Chromium (empty primary selection) does not show it, which is why it comes and goes.
A try with `preventDefault` on `mouseup` and `auxclick` for button 1 did not stop it (reverted); the paste event is probably raised
on `mousedown`, or the focus change has to go. Not fixed.

The spec `a middle-button drag pans the content and pastes nothing` is `test.fixme` with this finding; the wheel and ctrl+wheel spec
runs without the middle-button part.

## Observations that are not bugs

- Plain shapes without a fill are grabbed by their outline only, as in Excalidraw; specs that drag a shape give it a fill.
- `doc.autosave`, `doc.drop`, `text.edit_*`, `gesture.*`, `tool.hold`, `view.set_viewport` and `collab.set_pointer` are refused for agents
  (`not_for_agents`). The specs check the refusal; the user's path to the same effect (a drop, typing, a drag) is covered by real input.
- On the web `doc.export` with `format: png` and `view.screenshot` are refused with `port_unavailable` (no renderer port; `docs/web-app.md`,
  "Known gaps"). The native suite checks the screenshot.
- `ws://` to another machine is accepted by `collab.connect` when the page itself is on a loopback host (`docs/web-app.md`).
- `arrow.reroute` on an arrow that is already on its route reports `rerouted: []`.

## Not covered

- Touch and pen input, IME, the on-screen keyboard (the web host tests in `crates/gpu` cover the events, not the app).
- Safari and Firefox: only Chromium is driven.
- Native pointer input: the native suite uses the bridge only. `scripts/native_check.py` still drives the real mouse through XTEST.
- The agent relay pairing with a real `drafftink-mcp --relay` process (the page side of the refusals and the address scrub is covered).
- Two-peer collaboration on the native app; the browser suite covers two pages in one room.
- Frame-time budgets: `cargo xtask perf --bench` and `scripts/web_check.py` measure them. Xvfb caps the NVIDIA present path near 30 fps,
  so the e2e suites check frame *counts* (idle means 0), not speed.
