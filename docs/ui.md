# The UI: `drafftink-ui`

`drafftink-ui` (L6) is the egui user interface. It is thin: it reads the command registry and the session, builds actions, and the actions end in `Session::execute_from(origin, id, args)`. It holds no document logic. The rules are in `AGENTS.md`; the commands are in `docs/commands.md`; how a shell uses a session is in `docs/engine.md` section 4.

## 1. The shape of a frame

```
Session ──refresh──> Model (snapshot) ──draw──> Vec<UiAction> ──dispatch──> Session::execute_from
                                  ^                  ^
                      keymap::resolve_keys ──────────┘   (key events become the same actions)
```

* `Model` (`model.rs`) is a snapshot: every command with its title, shortcuts and `available` result (the refusal text), the tool, the selection count, the style of the selection (`style.get`'s data, read with the same domain function), settings, tabs, history labels. `Model::refresh(&Session)` recaptures only when a cheap stamp over the session changed.
* `draw(ui, &Model, &mut UiState) -> Vec<UiAction>` draws every surface. It has no session in its signature.
* `UiShell::frame(ui, &mut Session, FrameInput)` is what an app calls once per egui pass: refresh, resolve keys, draw, then `dispatch`. It returns a `FrameResult` with the commands that ran (and what they returned), the requests for the app (`HostRequest`), and whether a modal dialog is open (pointer events must not reach the canvas then).
* The UI returns **all** actions of a frame in order. The old `render_ui` returned the first one and dropped the rest.

## 2. Action routing

`action.rs` has the whole routing story.

| Action | Meaning | Goes to |
|---|---|---|
| `UiAction::Command(Call { id, args, origin })` | run a registered command | `Session::execute_from` |
| `UiAction::Scrub(Scrub { phase, command, args })` | one step of a dragged control (opacity slider, docs/exceptions.md E13) | `gesture.begin` / `gesture.update` / `gesture.end` / `gesture.cancel` through `execute_from`; the gesture id is remembered in `UiState` |
| `UiAction::Verb(Verb)` | open or close UI (docs/commands.md §6.3) | `UiState::apply_verb` |
| `UiAction::Host(HostRequest)` | something only the app can do: file pickers, clipboard text, a link | returned to the app in `Report.host` |

`route(&UiAction, &UiState) -> Routed` is the one function that decides this. `dispatch(sink, state, actions, now)` executes in order. `CommandSink` is implemented by `Session`; tests use a recording sink. Origins: `ui` for buttons, `menu`, `context_menu`, `palette` and `shortcut` for the other surfaces. The origin is recorded in the ring and never changes a result.

Refusals: any refusal becomes a toast with the text unchanged (plus a button for the first `suggest` entry). Three cases are handled differently: a refusal that comes from a shortcut with the code `no_target`, `nothing_to_do` or `not_applicable` is silent (`QUIET_FROM_KEYBOARD`); `unsaved_changes` opens a confirm dialog that re-runs the call with `on_unsaved` set; `no_save_target` asks the app for a save picker.

Controls (`widgets.rs`): every control that does something is `Cx::command_widget` underneath. It looks the command up in the model, greys out when `available` refuses and shows the refusal text as the tooltip, names itself for accessibility from the binding title (`Rectangle tool`) or the command title, and pushes the call on a click. A command the registry does not have yet is greyed with "`x` is not available in this build." and cannot run. A property that does not apply to the selection is greyed with its own reason (`Ctl::blocked`).

Nothing mutates session state from a widget. `crates/ui/tests/thin.rs` reads the crate source and fails when: a file other than `shell.rs`, `model.rs` or `action.rs` names `Session`; `&mut Session` appears outside the shell and the sink; any call that changes state is used (`tab_mut`, `set_raw`, `queue_write`, the `gesture_*` methods, `SceneTx`, ...); an `execute*` name other than `execute`, `execute_from`, `execute_batch` appears; or the model reads something from the session that is not on the read list.

## 3. Surfaces

| Surface | File | Commands and verbs |
|---|---|---|
| Tool column (14 tools, tool lock) | `surfaces/toolbar.rs` | `tool.set`, `tool.lock` |
| Top bar: stroke, fill, width | `topbar.rs` | `style.set`; wheels are verbs |
| Right panel: properties of the selection, or of the tool being drawn with | `props.rs` | `style.set`, `arrow.set_type`, `arrow.set_head`, `arrow.swap_heads`, `arrange.*`, `element.*`, `group.*`; opacity is a scrub |
| Bottom bar: undo, redo, grid, background, zoom, centre, fit, snap | `bottom.rs` | `history.*`, `settings.cycle`, `settings.toggle`, `view.*` |
| Tab bar | `tabs.rs` | `tab.switch`, `tab.close`, `tab.new` |
| Menu and collaboration button | `menu.rs` | `tab.new`, `doc.*`, `clipboard.copy_as`, `diagram.import_mermaid`, `settings.set` (export scale, theme), `view.set_view_only`; verbs |
| Colour popover, font popover | `color.rs` | `style.set`, `doc.set_background` |
| Command palette, argument form | `palette.rs`, `args_form.rs` | every `palette: true` command; UI verbs |
| Shortcuts dialog | `shortcuts.rs` | none |
| Context menu | `context_menu.rs` | clipboard, element, group, arrange, view commands |
| Collaboration dialog, presence panel | `collab.rs` | `collab.*` |
| Equation editor | `equation.rs` | `text.edit_end` |
| Confirm, recent documents, search, stats | `dialogs.rs` | `doc.open`, `element.query`; the confirm dialog re-runs the refused call |
| Toasts | `toasts.rs` | suggestion buttons run the suggested command |

Look and feel is the old app's: floating cards with a 1 px border, soft shadow, 8 px radius and 12 px margins, the accent blue `#3b82f6` for the active tool, tab and primary button, GelPen for headings (the font is embedded from `crates/typeset/assets/fonts`, 21 KB), the three canvas font families shown in their own faces in the font picker. Icons are drawn with the egui painter (`icons.rs`): no SVG assets, no image loader. All colours come from `theme::Palette`; the dark palette is chosen by the egui theme, which follows the `theme` setting (`light`, `dark`, `system`). Zen mode hides the chrome and keeps dialogs and toasts.

The palette lists a row per titled binding and one per other palette command, with the first shortcut and, for a refused command, the refusal text inline. A command with required arguments and no binding that fixes them opens a form generated from its JSON Schema (`args_form.rs`; enum, boolean, number, string, `Option`, raw JSON fallback with an inline error). The shortcuts dialog is generated from `keymap::help_rows`, which reads the registry, so it cannot go stale (the old list had 19 stale entries).

## 4. Keymap

Shortcuts are data on the registrations (`Binding`). `keymap::resolve_keys(events, &KeyContext) -> Vec<UiAction>` is the only key dispatcher.

* `Mod` is Cmd on macOS and Ctrl elsewhere (`egui::Modifiers::command` is Ctrl on Linux and Windows, Cmd on macOS). `Ctrl` means the physical Control key on macOS.
* Letters without Alt match the logical key (AZERTY users get the letters they see). Digits, punctuation and every chord with Alt match the physical key, named by its US layout. `?` and `+` are typed with Shift and match the logical key, so Shift is implied.
* Scope order: `Text`, `PointEditor`, `Canvas`, `Always`. `PointEditor` applies only while the line or arrow point editor has a selected point, so `Delete` means `arrow.delete_point` there and `element.delete` otherwise. During a text edit only `Text` and `Always` chords apply, plus `Mod` chords of commands with the `Commit` gate (docs/commands.md §6.2). While a modal dialog is open only `Always` applies.
* Nothing is handled while an egui text field has focus (`egui_wants_keyboard_input`). Text editing keys stay inside the text widgets.
* `Esc` closes an open overlay first (`ui.close_overlays`); with the equation editor open it cancels the edit (`text.edit_end` with `commit: false`); otherwise it runs its registered binding (`tool.cancel`).
* `Space` runs `tool.hold`; releasing it runs `tool.release`. Key repeat of the held key does nothing.
* `$cursor` and `$viewport_center` in binding arguments are filled from `FrameInput.pointer`; a key whose placeholder cannot be filled is dropped.
* The UI verbs that have keys (`Mod+O`, `Mod+Shift+S`, `Mod+/`, `?`, `S`, `G`, ...) are `keymap::VERB_BINDINGS`. A test compares the table with the "UI verb" rows of docs/commands.md §6.1 and another checks that no verb key collides with a command key.
* Labels use words on every platform (`Ctrl+Shift+Z`, `Cmd+Shift+Z`, `Opt`, `Left`) because the bundled egui fonts have no glyph for the Mac modifier symbols or the arrows.

## 5. What stays UI-only, and why

| Thing | Why it is not a command |
|---|---|
| Open or closed state of the menu, popovers, dialogs, palette query, hover, scroll, toasts, zen mode, the stats panel | Not session state (docs/commands.md §1.1). |
| Text-field buffers of dialogs (hex colour, room name, LaTeX draft, form values) | A draft until the user applies it; applying runs a command. They live in egui's temporary memory. |
| File and image pickers, clipboard text for the equation editor, links | Need the OS. The UI returns a `HostRequest`; the app does the work and then calls a command (`doc.open`, `doc.save` with `to`, `image.insert`, `library.import`). |
| Pointer gestures on the canvas (drag, resize, rotate, draw, pan, zoom, hover) | The canvas and interaction layer, not this crate. They use the typed gesture API and are listed in docs/exceptions.md. This crate never executes a command per pointer move. |
| The opacity slider drag | A gesture (E13): `gesture.begin`, `update`, `end` with `style.set`; a click is a plain `style.set`. |
| Style choice during a text edit | `style.set` refuses with `text_editing`; the control hands the props to the text editor as `HostRequest::TextEditStyle` (E16), which applies them and passes the result to `text.edit_end`. |
| Opening the context menu and the equation editor | The canvas layer calls `UiState::open_context_menu` and `UiState::open_equation`. |

## 6. Repaints

The UI requests a repaint only for work that exists: one extra frame after commands ran (the model changed), and toasts (`request_repaint_after` until the fade starts, then 30 Hz during the 0.6 s fade; none afterwards). egui's own animation of a newly opened area and the text caret blink are egui's requests. `tests/frame.rs` checks that a quiet frame asks for no repaint.

## 7. For the app

```rust
let mut shell = UiShell::new(is_mac);            // once
shell.set_collab(view);                          // when the collab client changes
let res = shell.frame(ui, &mut session, FrameInput { pointer });   // every egui pass
for r in res.report.host { /* open a picker, copy text, ... then call a command */ }
```

Pointer events go to the canvas only when `!res.modal` and egui does not own the pointer. The GPU shell should install the egui context's theme through `UiShell` only (it applies the `theme` setting itself). Install fonts happens inside `draw` (`theme::ensure_fonts`).

## 8. Tests

`cargo test -p drafftink-ui`; `egui_kittest` without a GPU.

* `surfaces.rs`: per surface, a click yields the expected command id and args; a refused command greys its control and the tooltip is the refusal text; zen mode.
* `routing.rs`: `route`, `dispatch`, scrubs, toasts, confirm and save-picker flows, the origin in the ring.
* `keymap.rs`: scope resolution, `Mod` per platform, layouts, Esc and Space, and a test that replays every registered binding on both platforms and checks it resolves to its command with its arguments.
* `frame.rs`: a whole `UiShell::frame`: all key actions run, no idle repaint, theme follows the setting.
* `palette.rs`, `args_form.rs`, `shortcuts.rs`, `context_menu.rs`, `color_popover.rs`, `collab_dialog.rs`, `equation_dialog.rs`, `small_dialogs.rs`: the dialogs. `tests/synth` adds stand-ins for commands the engine has not registered yet; the real command wins as soon as one exists.
* `thin.rs`: the grep test of section 2.

## 9. Open points

* The context menu, the menu and the palette show `clipboard.*`, `element.*`, `doc.*`, `collab.*`, `selection.*` greyed until the engine registers them.
* The search dialog only sends `element.query`; it shows no result list yet. The stats panel shows what the model has (selection count, tool, zoom, undo labels); editing bounds waits for `selection.get` and `element.set_bounds`.
* Touch: hit targets are 24 to 34 px; a larger touch mode is not built.
* The UI fonts for the hand-written headings are embedded a second time from `crates/typeset/assets/fonts` (about 65 KB with the two preview fonts). `typeset` could expose the bytes.
