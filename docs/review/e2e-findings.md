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

### F2. While hovering, the cursor position Ctrl+V uses was missing or one sample behind (not an app bug; the spec was wrong)

The first diagnosis was a lagging cursor in `crates/gpu/src/shell.rs` (`held_cursor`). It was wrong. The spec hovered
(900, 600) with a shape selected, and at the 1280 by 800 test viewport that point lies under the properties panel, which opens
at the right edge (x from 887) as soon as something is selected. A pointer over an egui panel is not on the canvas, so `$cursor`
is omitted (`docs/commands.md` section 6.1) and the paste lands at the viewport centre. The "lag" came from the panel appearing one
frame after the selection: moves made before it were recorded, later ones were not. With the pointer on free canvas every
sample is recorded exactly (checked with one, two and three slow and quick moves, the last position wins).

Fixed in the spec: `keys: Ctrl+V pastes the copy centred on the pointer` hovers (700, 520), and a new spec
`keys: Ctrl+V with the pointer over a panel pastes at the middle of the view` pins the panel behaviour.

### F3. One Ctrl+V on the web pasted twice (fixed)

Two paths ran `clipboard.paste` for one key press: the key binding (`Mod+V`, from the keymap) and the browser's `paste` event
(`ShellEvent::Paste`). Copy and cut had the same pair. Both ran from origin `shortcut`, and the copies stacked on one spot.
The event is the one that carries the system clipboard (images, text from other programs, the private payload), so it owns the
chord now. `Binding::host_clipboard` marks `Mod+C`, `Mod+X`, `Mod+V` and `Mod+Shift+V`; a host that delivers clipboard events passes
`FrameInput::host_clipboard` and the keymap skips those bindings (`ui/tests/keymap.rs`). Ctrl+Shift+V keeps `plain_text` through the
event: the paste handler reads the held modifiers (`EventCx::modifiers`, `clip::is_plain_text_paste`). Native is the same: its
shell turns the shortcut into the event, so the key no longer pastes a second time there either.
A paste event with an empty system clipboard sends nothing (the web host drops it), so Ctrl+V right after Ctrl+C in under a
millisecond can paste an older clipboard. A person takes longer.

Specs: `keys: one Ctrl+V pastes once`, `keys: one Ctrl+C copies once`, and the copy and paste spec now expects exactly one copy.

### F4. A middle-button pan could paste on Linux (fixed)

Linux browsers paste the primary selection when the middle button is released. A cancelled `pointerdown` makes the browser skip
the compatibility `mousedown` and `mouseup` events, so the earlier try (`preventDefault` on `mouseup` and `auxclick`) never got
to run. Now `pointerdown` stays uncancelled for the middle button and `mousedown` and `mouseup` are cancelled for it
(`crates/gpu/src/web/pointer.rs`). The spec seeds the primary selection with `xclip` (`support/x11.ts`), so it fails on the old
behaviour every time and not only after other tests have selected text; without `xclip` it still runs, unseeded.

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
