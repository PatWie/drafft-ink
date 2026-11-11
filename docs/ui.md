# The UI: `drafftink-ui`

`drafftink-ui` (L6) is the egui user interface. It is thin: it reads the command registry and the session, builds actions, and the actions end in `Session::execute_from(origin, id, args)`. It holds no document logic. The rules are in `AGENTS.md`; the commands are in `docs/commands.md`; how a shell uses a session is in `docs/engine.md` section 4.

## 1. The shape of a frame

```
Session ──refresh──> Model (snapshot) ──draw──> Vec<UiAction> ──dispatch──> Session::execute_from
                                  ^                  ^
                      keymap::resolve_keys ──────────┘   (key events become the same actions)
```

* `Model` (`model.rs`) is a snapshot: every command with its title, shortcuts and `available` result (the refusal text), the tool, the selection count, the style of the selection, the common bounds, the selected elements (kind, link, frame, label), the camera, the library while its panel is open, settings, tabs, history labels. The reads go through `Session::query`, which runs a `reads_only` command (`style.get`, `selection.get`, `element.query`, `view.get`, `library.list`, and `library.thumbnail` through `Model::read_thumbnails`) and leaves no ring entry; the crate does not use `drafftink-edit` any more (it still uses `drafftink-scene` for the colour palette and the document name). `Model::refresh(&mut Session)` recaptures only when a cheap stamp over the session changed. `Session::query` needs `&mut self`, so the model's readers take `&mut Session` and the thin test checks that they call only `query` and never `execute*`.
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

The verbs are the 23 of docs/commands.md §6.3 (the last one, `ui.open_about`, is the About dialog with the source offer, `surfaces/about.rs`). This work added `ui.open_export`, `ui.open_preferences`, `ui.toggle_library` and `ui.rename_document`; `ui.save_as` now opens a name dialog first.

`route(&UiAction, &UiState) -> Routed` is the one function that decides this. `dispatch(sink, state, actions, now)` executes in order. `CommandSink` is implemented by `Session`; tests use a recording sink. Origins: `ui` for buttons, `menu`, `context_menu`, `palette` and `shortcut` for the other surfaces. The origin is recorded in the ring and never changes a result.

Refusals: any refusal becomes a toast with the text unchanged (plus a button for the first `suggest` entry). Three cases are handled differently: a refusal that comes from a shortcut with the code `no_target`, `nothing_to_do` or `not_applicable` is silent (`QUIET_FROM_KEYBOARD`); `unsaved_changes` opens a confirm dialog that re-runs the call with `on_unsaved` set; `no_save_target` asks the app for a save picker.

Controls (`widgets.rs`): every control that does something is `Cx::command_widget` underneath. It looks the command up in the model, greys out when `available` refuses and shows the refusal text as the tooltip, names itself for accessibility from the binding title (`Rectangle tool`) or the command title, and pushes the call on a click. A command the registry does not have (a build without it) is greyed with "`x` is not available in this build." and cannot run. Every catalog command is registered now, so the tests build that case with `Fixture::without(&["x"])`. A property that does not apply to the selection is greyed with the engine's reason for it: `style.get` reports it per property (`reason`, the words `style.set` refuses with), and `Ctl::blocked` carries it.

The UI has no refusal text of its own (`tests/refusals.rs`). A control is greyed, not hidden (the context menu is the exception, see `menu_kit.rs` below; so are the groups of the strip, see below), and its tooltip is the engine's text: the availability of its command (`Model::refusal_text`), the `reason` of `style.get`, or, for a dialog choice that is not a command, the engine's own words for the thing (the refusal of a command that needs a selection; the description of the `doc.export` argument). A condition that only the run can check (a count, a kind) leaves the control enabled, and the refusal comes back as a toast with the same text the bridge returns. Where a command can answer from the selection alone, it does: `default_targets` of `frame.unwrap`, `frame.select_children`, `frame.remove`, `text.wrap_in_container` and `arrow.edit_points` runs the same check as `execute`, so a panel control is greyed with that text, and the context menu leaves the row out (`tests/context_menu_rows.rs`). The `element.convert` targets depend on the kinds, `text.bind` and `text.unbind` name the text and the container, and the frame rename dialog needs the frame, so those rows are built from the selection. "Copy as text" is always listed because the check needs the `format` argument; the engine refuses at the click ("none of the elements holds text"). The ribbon leaves a group out when `style.get` says its property applies to no selected element (a rectangle has no heads, no font) or when the engine refuses its command (align needs two elements, distribute three: `arrange.align` and `arrange.distribute`), and greys a control with the engine's reason when it applies to some of them. Every refusal of a `gesture.*` command is a toast, except `no_gesture`, which only means that the drag had already ended. `doc.export` to a file that cannot be written is a refusal (`io_failed`, "did NOT happen"), so the export dialog stays open with the reason; a write that the port finishes later (`pending_write`) leaves the dialog open with a note. Exporting a picture has one path: the menu row "Export as PNG", `Mod+E` and the palette row run the call of the dialog for an export at once (`surfaces::export::complete_args` fills the `to` download named after the document, the scope and the look from the `export.*` settings for every call of a command whose registration says `completes_export`; what the call gave stays). Without `to` the command answers inline, which is for agents. The refusal of such an export is a toast also from the keyboard (`nothing_to_do` and `no_target` are quiet for other shortcuts), so `Mod+E` on an empty drawing says why nothing happened.

Menus (`menu_kit.rs`): the context menu, the hamburger menu and the tab menu are lists of calls on `Menu` (`command`, `verb`, `host`, `local`, `submenu`, `separator`, `heading`), drawn with one row painter at one width (`MENU_WIDTH`). A row has the fixed height of the layout, a 16 px icon slot (which holds the check mark of a toggle), the label, the shortcut dimmed at the right and a chevron when it opens a flyout. The label of a command row is `Model::label(id, args)`: the title of the binding or the `Variant` (`CommandSpec::variants`, a named call without keys) that fixes those arguments, else the command title, so a command reads the same in the menus, the panel and the palette (`context_menu_rows::every_label_is_the_title_the_registry_gives_the_call`). A flyout opens beside its row, flips to the left at the window edge, stays inside the window, and closes 0.25 s after the pointer rests on another row of the menu. The keyboard uses egui's focus: Up and Down move, Right opens a flyout, Left closes it, Enter runs a row, Esc closes the menu; while a popup menu is open the keymap leaves the arrow keys and Enter to it. A menu is a `Role::Menu` node with a name ("Context menu", "Main menu", "Tab menu", the flyout's name), which is how the tests find its rows.

Nothing mutates session state from a widget. `crates/ui/tests/thin.rs` reads the crate source and fails when: a file other than `shell.rs`, `model.rs` or `action.rs` names `Session`; `&mut Session` appears outside the shell and the sink; any call that changes state is used (`tab_mut`, `set_raw`, `queue_write`, the `gesture_*` methods, `SceneTx`, ...); an `execute*` name other than `execute`, `execute_from`, `execute_batch` appears; or the model reads something from the session that is not on the read list.

## 3. Surfaces

| Surface | File | Commands and verbs |
|---|---|---|
| Tool column (15 tools, sticky note, tool lock, library) | `surfaces/toolbar.rs` | `tool.set` (the active Lasso button returns to Select), `element.create_sticky`, `tool.lock`; verb `ui.toggle_library` |
| Top strip: one bar across the window, flush with the top edge, two control rows tall. Left: menu and rooms over the title. Then the colour group (stroke over fill), then the contextual groups (section 5c), "More" at the right end. When the colour group does not fit, it becomes one style button that opens a popover | `strip.rs` places `menu::buttons`, `topbar.rs` and the groups of `ribbon/` | `style.set`; wheels are verbs; the style button is UI state |
| Strip groups: the properties of the selection, or of the tool being drawn with, in the two rows of the top strip (section 5c) | `ribbon/` | `style.set` (also vertical align and line height), `tool.set_arrow` (path and heads of new arrows, with the arrow tool and nothing selected), `arrow.set_type`, `arrow.set_head`, `arrow.swap_heads`, `text.set_autoresize`, `text.format`, `arrange.*`, `element.*`, `group.*`; the link buttons (`element.open_link`, link dialog); the font button opens the font popover (a verb); opacity is a scrub |
| Bottom bar: undo, redo, grid, background, zoom, centre, fit, snap | `bottom.rs` | `history.*`, `settings.cycle`, `settings.toggle`, `view.*` |
| Tab bar and tab menu | `tabs.rs` | `tab.switch`, `tab.close`, `tab.new`; a double click renames (`tab.rename`); a right click opens the menu: rename, `tab.duplicate`, `tab.move`, `tab.close` |
| Menu (opens under the menu button), document title and collaboration button | `menu.rs` | `tab.new`, `doc.*`, `clipboard.copy_as`, `diagram.import_mermaid`, `settings.set` (export scale, theme), `view.set_view_only`; verbs (the title chip is `ui.rename_document`) |
| Rename and Save As dialogs | `naming.rs` | `doc.rename`, `tab.rename`, `library.rename`, `frame.rename`; Save As runs `doc.rename` when the name changed and then hands the app `PickSaveTarget` |
| Export dialog | `export.rs` | `doc.export` (`format` png, svg, json or excalidraw; `scope` selection or all; `scale`, `background`, `dark`, `embed_scene`; `to` a download named after the document), `clipboard.copy_as` |
| Preferences dialog | `preferences.rs` | `settings.set` / `settings.toggle` for theme, interface font (`ui.font`: GelPen by default, any bundled canvas font), grid, snapping, nudge, box select, arrow and frame keys, export defaults, autosave, history depth, `collab.set_user`; the layout choice is UI state |
| Library drawer | `library.rs`, `thumbs.rs` | A panel from the bottom of the window (see section 5b): `library.add`, `library.insert` (click: middle of the view; drag: the world point under the release), `library.rename`, `library.remove`, `library.rename_section`, `library.remove_section`, `library.export`, pictures with `library.thumbnail`; Import is a host request |
| Convert popup | `convert.rs` | `element.convert`; opened by Tab or Shift+Tab (origin `shortcut`) |
| Colour popover, font popover | `color.rs` | `style.set`, `doc.set_background` |
| Command palette, argument form | `palette.rs`, `args_form.rs` | every `palette: true` command; UI verbs |
| Shortcuts dialog | `shortcuts.rs` | none |
| Context menu | `context_menu.rs`, rows and flyouts in `menu_kit.rs` | Built per target. Empty canvas: Paste, Select all, Zoom to fit everything, View (`settings.cycle`, `settings.toggle`, `view.set_view_only`, `element.unlock_all`, zen and stats verbs). Selection: Cut, Copy, Paste, Duplicate, Delete; Group or Ungroup; flyouts Arrange (`arrange.z_order`, `frame.wrap`), Align (`arrange.align`, `arrange.distribute`), Flip & convert (`element.flip`, `element.convert`, `arrow.edit_points`, `text.wrap_in_container`), Frame (`frame.unwrap`, `frame.select_children`, `frame.remove`, rename frame), Text (`text.bind`, `text.unbind`), Copy as (`clipboard.copy_as`); Lock, Link, Open link, Add to library; Copy style, Paste style. A locked element: Unlock, Paste, Select all, Zoom to fit everything. A row whose command the engine refuses now is left out (`Model::refusal_text`), except Paste, which greys. A command the registry lacks greys, named by its id |
| Hamburger menu | `menu.rs` | Rows under the headings File, Export, Insert, View and Help; every row is a command or a UI verb |
| Tab menu | `tabs.rs` | `tab.duplicate`, `tab.move`, `tab.close`; rename opens the rename dialog |
| Collaboration dialog, presence panel | `collab.rs` | `collab.*`; each peer in the presence panel has a Follow button (`collab.follow` with the peer id, or with no peer to stop) |
| Equation editor | `equation.rs` | `text.edit_end` |
| Confirm, recent documents, search, stats | `dialogs.rs` | `doc.open`, `element.query`, `selection.select`, `view.center`, `element.set_bounds`; the confirm dialog re-runs the refused call |
| Toasts | `toasts.rs` | suggestion buttons run the suggested command |

**Geometry.** No surface knows the height of another as a number. `chrome::ChromeRects` (in `UiState::chrome`, reset at the start of every `draw`) holds the rectangle of each surface that takes room, recorded when it is drawn; `surfaces::show_all` draws in the order of dependencies (strip, bottom bar, tabs, then the panels, popovers, toasts and the stats panel, which read the rectangles). The tool column stays centred on the window and scrolls when it would reach the strip (`scroll_when_taller`); the strip is two rows tall, so on a window of 800 points or less the column scrolls. A group that must be measured before it is placed (the strip's groups, a column that may scroll) is laid out first in a hidden sizing pass.

**Tokens.** `theme/tokens.rs` has the type scale (`TextSize`), the spacing scale (`space`), the radius ladder (`radius`) and the control heights (`Control`, with the touch heights). A surface asks for `TextSize::Body`, not `13.0`; `tests/tokens.rs` fails on a new literal font size in `surfaces/`. The UI font is the `ui.font` setting (`theme::UiFontChoice`); headings that keep GelPen use `TextSize::hand`.

Look and feel is the old app's: floating cards with a 1 px border, soft shadow, 8 px radius and 12 px margins, the accent blue `#3b82f6` for the active tool, tab and primary button, GelPen for headings (the bytes come from `drafftink_typeset::font_file`, so the binary holds each font file once), the three canvas font families shown in their own faces in the font picker. Icons are drawn with the egui painter (`icons.rs`): no SVG assets, no image loader. All colours come from `theme::Palette`; the dark palette is chosen by the egui theme, which follows the `theme` setting (`light`, `dark`, `system`). Zen mode hides the chrome and keeps dialogs and toasts.

### Colours the UI shows

The canvas draws stored element colours through the dark mapping in the dark theme (`render.md`, "Dark theme"), so
the UI never shows a stored colour as it is: `colors.rs` is the one place that applies `Color::for_dark_theme` and its
inverse `Color::from_dark_theme` (the same functions as the renderer, no second copy). In the light theme both are
the identity and nothing below changes.

* **Swatches show the display.** The quick swatches of the top strip, the last-picked
  swatch, the cells of the colour popover (grid and quick column) and the page-colour button show
  `for_dark_theme(stored)`. A palette cell is a stored colour: a pick sends the palette name as before
  (`style.set {stroke_color: "blue-500"}`), so the document holds the same colour in both themes and a light-theme
  viewer sees the intended one. The swatch is exactly the colour of the line that gets drawn.
* **Colours named by their look store the inverse.** The Black and White cells of the quick column, and a colour
  typed in the hex field, mean "this colour on screen": the pick stores `from_dark_theme(seen)` and the swatch shows
  what that draws. In the dark theme White stores `#010001` and is drawn `#eeedee`, the lightest the dark canvas has.
  A colour the filter cannot show is stored as the one whose display is nearest; the swatch shows that display.
* **The ring** (which swatch is current) compares stored colours, so it is the same in both themes.
* Not recoloured: peer cursors and user colours (`collab.rs`), the accent and the chrome (`theme::Palette`).

Tests: `tests/color_popover.rs` reads the circle painted for each swatch (dark: every grid cell equals
`for_dark_theme(palette colour)`, picks store the palette colour or the inverse, the top bar, the last-picked swatch
and the page button follow) and `app/tests/app.rs` compares the swatch with the colour the painter encodes.

The palette lists a row per titled binding and one per other palette command, with the first shortcut and, for a refused command, the refusal text inline. A command with required arguments and no binding that fixes them opens a form generated from its JSON Schema (`args_form.rs`; enum, boolean, number, string, `Option`, raw JSON fallback with an inline error). The shortcuts dialog is generated from `keymap::help_rows`, which reads the registry, so it cannot go stale (the old list had 19 stale entries).

## 4. Keymap

Shortcuts are data on the registrations (`Binding`). `keymap::resolve_keys(events, &KeyContext) -> Vec<UiAction>` is the only key dispatcher.

* `Mod` is Cmd on macOS and Ctrl elsewhere (`egui::Modifiers::command` is Ctrl on Linux and Windows, Cmd on macOS). `Ctrl` means the physical Control key on macOS.
* Letters without Alt match the logical key (AZERTY users get the letters they see). Digits, punctuation and every chord with Alt match the physical key, named by its US layout. `?` and `+` are typed with Shift and match the logical key, so Shift is implied.
* Scope order: `Text`, `PointEditor`, `Canvas`, `Always`. `PointEditor` applies only while the line or arrow point editor has a selected point, so `Delete` means `arrow.delete_point` there and `element.delete` otherwise. During a text edit only `Text` and `Always` chords apply, plus `Mod` chords of commands with the `Commit` gate (docs/commands.md §6.2). While a modal dialog is open only `Always` applies.
* Nothing is handled while an egui text field has focus (`egui_wants_keyboard_input`). Text editing keys stay inside the text widgets. The canvas text editor is not an egui widget: the app takes its keys from the frame's egui events before the keymap runs (`app/editor/keys.rs`), its characters and IME from `ShellEvent::TextInput`, and its clipboard from `ShellEvent::Paste` / `CopyRequested`.
* `Esc` closes an open overlay first (`ui.close_overlays`); with the equation editor open it cancels the edit (`text.edit_end` with `commit: false`); otherwise it runs its registered binding (`tool.cancel`).
* `Space` runs `tool.hold`; releasing it runs `tool.release`. Key repeat of the held key does nothing.
* `$cursor` and `$viewport_center` in binding arguments are filled from `FrameInput.pointer` with a world point `[x, y]`, the form the commands take; a key whose placeholder cannot be filled is dropped.
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
| Opening the context menu and the equation editor | The canvas layer calls `UiState::open_context_menu(screen, world)` (the world point is where `Paste` puts its content) and `UiState::open_equation`. |

## 5a. The touch layout

`Layout` (`widgets.rs`, carried by `Cx`) holds `compact`, `narrow` and the window size; the sizes of targets come from it (`tool_side` 34 or 44, `small_side` 28 or 40, rows 28 or 40, toggles 24 or 36, dialog buttons 32 or 44). `UiState::compact(width)` decides: the preference `LayoutPreference` (`Auto`, `Touch`, `Desktop`, set in the preferences) wins; `Auto` means touch-first (a touch event was seen, or the app called `UiShell::set_touch_first`) or a window narrower than 720 points. In the touch layout the tool column collapses to one button (it starts collapsed on a narrow window and open on a wide one, until the user chooses; `UiState::set_panel_collapsed`), the tool column, the colour bar and the bottom bar scroll when they do not fit, the strip is two rows on a tablet (rows of 40 points) and one row on a narrow window, where the colours and the groups wait behind their buttons; on a narrow window the tab bar sits above the bottom bar. The desktop layout is unchanged. The web shell calls `set_touch_first` at start when `(pointer: coarse)` matches. On the touch layout the tool column also keeps clear of the bottom bar (`ChromeRects::centred_panel_height_above_bottom_bar`): on a window 820 points high (an iPad in landscape) the last tools lay under the bar's first buttons before. The preference is not persisted by this crate (no settings key exists for it); an app that wants to keep it reads and writes `UiShell.state.layout`.


## 5b. The library drawer

`surfaces/library.rs` draws the library as a panel that opens from the bottom of the window (`ui.toggle_library`, from the tool column or the menu).

* **Place.** It is as wide as the window less the margins and its bottom edge sits `8` points above the bottom bar and the tab bar, whose rectangles it reads from `ChromeRects` (`bottom_bar`, `tabs`; they are drawn before it, so it follows them if they move) and it records its own there (`library`). Where the tool column (`tools`) reaches down to its height it is narrower by its width, so that its buttons stay free, and its top edge stays under the strip (`strip`, `top_limit`); the toasts, the convert popup, the stats panel and the background popover sit above it (`bottom_floor`, `above_bottom_left`); on a narrow window, where those are collapsed to a button each, it covers them. Its height is 30 % of the window until the person drags the strip on its top edge (the control "Resize the library"); the drag is clamped from 224 points (360 on the touch layout) to 70 % of the window, and what was chosen is `UiState::library_height`.
* **Header.** Title and count ("3 items", or "1 of 3 items" while the search hides some), the search field (it filters by name in the UI, whatever the case), the name for the next Add, Add, Import…, Export and Close. The touch layout makes two rows of it.
* **Sections.** The items are grouped by `library.list`'s `sections`: the person's own items first ("My items"), then one section per imported library, named after its file. A header (`Collapse {name}` / `Expand {name}`; chevron, name, "3 items", or "1 of 3 items" while the search hides some) folds its section on a click (`UiState::library_collapsed`; the tiles of a folded section are not drawn, so none of its pictures is asked for). At its right, rename (`Aa`, not on My items: `library.rename_section` through the rename dialog) and remove (the bin). Removing asks first (a dialog, `UiState::library_remove`) and then runs `library.remove_section`: the library has no undo. A right click on a header opens `Library section menu` (Collapse or Expand, Rename…, Remove section…). The search keeps the matching items of each section, hides the sections with none, and shows the matches of a folded section. A section that is new since the last frame (an import) is scrolled to the top of the list.
* **Tiles.** A wrapping grid in a vertical scroll area, 104 points of picture (112 on the touch layout) and the name under it. A click inserts the item in the middle of the view; a drag onto the canvas inserts it where the pointer is released and shows the picture under the pointer; a release over the drawer cancels. A right click opens a menu built with the menu kit (`Library item menu`: Insert, Rename…, Remove; origin `context_menu`; `UiState::library_menu`) with the same calls as the click and the corner buttons. Rename (`Aa`) and remove (the bin) appear in the corner of the tile under the pointer, and always on the touch layout; they are painted over the tile and take no room in the grid, so hovering moves no tile (`hovering_a_tile_moves_and_resizes_no_tile`). The tooltip shows a larger picture, the name and the element count.
* **Pictures.** `library.thumbnail` (a read, answered with a PNG that has a transparent background) draws an item with the `Renderer` port, the same one `doc.export` uses, so the desktop and the web (WebGPU and the WebGL2 fallback) have it. `thumbs.rs` keeps the answers by item: an entry is good while the item's `content` fingerprint (`library.list`) and the palette (`Palette::of(ctx).dark`, so the dark canvas recolours as it does on the canvas) stay the same, so renaming keeps it and a new drawing or the other theme draws it again. The model reads `library.list` again whenever `Session::library_revision` changes (the library is outside the document, and the app runs an import from its inbox without going through the shell). A tile gets a picture only when it is on screen, and `UiShell::frame` asks for at most two a frame (`thumbs::PER_FRAME`), the larger picture of an open tooltip first; more work left over asks for another frame, and a picture the browser is still drawing (`render_pending`) is asked for again when its renderer wakes the app, without a repaint of its own. Until a picture is there, and when it cannot be drawn (no renderer, an answer that is not a PNG), the tile shows an outline with the item's proportions. At most 200 pictures stay for items off screen; textures are dropped with them.

## 5c. The top strip and its groups

`surfaces/strip.rs` draws one bar across the whole window, flush with its top edge, two control rows tall (72 points on the desktop layout, 96 on a tablet's touch layout; one row, 52 points, on a narrow window). The height depends on the layout only, never on what is selected or drawn, so the canvas never jumps. `surfaces/ribbon/` holds the contextual groups of the strip. They show the properties of the selection, or, with nothing selected, of the tool being drawn with ("new elements": `style.set` writes the defaults, and `tool.set_arrow` the path and heads of new arrows).

* **Place, left to right.** The left cluster: menu and rooms on the first row, the document title under them. The colour group (`topbar.rs`): stroke on the first row (label, quick swatches, the last picked colour, the wheel) and fill on the second ("no fill" first, then the same), swatches starting at the same x. Then the contextual groups, packed left to right from the colour group, each laid out in the same two rows, with a thin divider (6 points, a hairline, 6 points) between two parts. The "More" button is pinned to the right end. `ChromeRects::strip` is the only rectangle; the menus, the colour popovers, the font popover, the library drawer and the toasts read it.
* **Groups, in this order, never reordered** (first row over second row): stroke (width over style: solid, dashed, dotted); shape (fill pattern, only while there is a fill, over corners as sharp and round icons); arrow (path over the Start and End head buttons, which show the heads of the selected arrow, `style.get`'s `heads`, or the settings of new arrows); brush; font (family dropdown over weight); size (S to XL over bold and italic); text align (left, centre, right over top, middle, bottom for a label or a box); opacity (slider over align and distribute); layering (back, backward, forward, front over flip); actions (duplicate, group, ungroup, lock over edit link, open link, delete). A group is there when `style.get` says its property applies to the selection (for a tool, `ribbon::tool_shows` lists the properties; that table is presentation, the engine skips what does not apply), or when the engine says the command can run: the align row follows `arrange.align`, its distribute buttons `arrange.distribute`. A group with one control row has it on the first row. Layering and the actions need a selection. The width is a row of the stroke group, so it is there only while a stroke applies (a text has none).
* **Nothing selected, a tool that draws nothing** (select, lasso, hand, eraser, laser): only the colour group, which sets the defaults for new elements. The alternative, an empty strip, would change with the tool and hide the controls people use first. The strip keeps its height either way.
* **Controls.** All controls in a row are one height (a small button: 28 or 40 points); no headings, the icons, pictograms and tooltips name the controls (`Corners: Round`, `Fill pattern: Hatch`, `Path: Elbow`, `Text align: Left`, `Vertical align: Middle`).
* **Fitting** (`ribbon/layout.rs`, `fit`). The widths of the parts are measured in a hidden pass, so the plan is right in the frame it is made. When the groups do not fit the window, the ones at the right end (the least used) go behind the "More" button, whose popover (menu kit, 288 points wide, under the strip, right-aligned with the button) shows them one under the other in their two rows, and under them the properties without a control of their own: sloppiness, pressure, line height, the text box, and the corners of arrows and lines (whose path chips take the place of the corners). The button stays while it has such properties, even when every group fits. When the colour group does not fit beside the left cluster and the button, it collapses to one style button that opens a popover with the same controls (`UiState::style_open`). The last resort is a sideways scroll of everything right of the left cluster. A narrow window (`Layout::narrow`, the phone) has one row: the left cluster, the style button and "More", with every group behind "More" (a scrolling row of 40 point targets on 390 points would show about three of them).
* **Greyed or left out.** A control whose property applies to some of the targets is greyed with the engine's reason. A group the engine says cannot apply at all is left out.
* **State.** `UiState::ribbon_menu` (which menu is open, and its button) is UI-only; Esc and a click outside close it. The opacity slider is the one scrub (E13).

## 6. Repaints

The UI requests a repaint only for work that exists: one extra frame after commands ran (the model changed), and toasts (`request_repaint_after` until the fade starts, then 30 Hz during the 0.6 s fade; none afterwards). egui's own animation of a newly opened area and the text caret blink are egui's requests. `tests/frame.rs` checks that a quiet frame asks for no repaint.

## 7. For the app

```rust
let mut shell = UiShell::new(is_mac);            // once
shell.set_collab(view);                          // when the collab client changes
let res = shell.frame(ui, &mut session, FrameInput { pointer });   // every egui pass
for r in res.report.host { /* open a picker, copy text, ... then call a command */ }
```

Things the app does for the surfaces of this work:

* `HostRequest::PickLibraryImport`: show a file picker for an `.excalidrawlib` and call `library.import` with `mode: merge` (the library panel's Import button; `PickLibrary` opens a library as a tab instead).
* `HostRequest::PickSaveTarget` now follows the Save As name dialog: the document is already renamed, so the picker should suggest `<name>.json` or the like and then call `doc.save` with `to`.
* Collaboration: `Peer` has two new fields, `id` (the `peers[].id` of `collab.status`, which `collab.follow` takes) and `followed`. `CollabView::from_status(&collab.status output)` builds the whole view, so the app can replace `collab_view_of` with `CollabView::from_status(&session.query("collab.status", json!({}))?)`.
* Export and the library export write with `to: {download: name}`; the app's files port saves the bytes (native: the downloads folder; web: a download).
* Touch: call `UiShell::set_touch_first(true)` on a touch-first device (web: `matchMedia("(pointer: coarse)")`). Touch events seen by egui set it too.
* The layout preference lives in `UiShell.state.layout` and is not stored; persist it if wanted.
* For the canvas overlay: `Model.selection` has the id, kind, link, frame and label of each selected element (the first 50), and `Model.selection_link` the link of a single selection. The link icons of unselected elements are `InteractionOverlay::link_icons` (render's job).

Pointer events go to the canvas only when `!res.modal` and egui does not own the pointer. The GPU shell should install the egui context's theme through `UiShell` only (it applies the `theme` setting itself). Install fonts happens inside `draw` (`theme::ensure_fonts`).

## 8. Tests

`cargo test -p drafftink-ui`; `egui_kittest` without a GPU.

* `surfaces.rs`: per surface, a click yields the expected command id and args; a refused command greys its control and the tooltip is the refusal text; zen mode.
* `routing.rs`: `route`, `dispatch`, scrubs, toasts, confirm and save-picker flows, the origin in the ring.
* `keymap.rs`: scope resolution, `Mod` per platform, layouts, Esc and Space, and a test that replays every registered binding on both platforms and checks it resolves to its command with its arguments.
* `frame.rs`: a whole `UiShell::frame`: all key actions run, no idle repaint, theme follows the setting.
* `palette.rs`, `args_form.rs`, `shortcuts.rs`, `context_menu.rs`, `color_popover.rs`, `collab_dialog.rs`, `equation_dialog.rs`, `small_dialogs.rs`: the dialogs. `tests/dialog` has the harness and the set-up helpers (a connected collaboration session, a session editing a formula, a stored document). The dialog tests run the real commands: they dispatch what the click produced and check the session. `common::Fixture::without(&[ids])` builds a session whose registry leaves those commands out, for the "not available in this build" look of a control.
* `naming.rs`, `export_dialog.rs`, `preferences.rs`, `library_panel.rs`, `context_menu_rows.rs`, `text_props.rs`, `ribbon.rs`, `convert_popup.rs`, `follow.rs`, `touch_layout.rs`: the surfaces of this work. Each control is clicked and the call it makes is compared with the command and arguments; the dispatched call then runs on the real session and the test checks its effect (a tab renamed, a file in the download store, a setting changed, an element converted, the camera following a second real session on the in-memory server).
* `frame.rs` also checks that the model's reads leave no ring entry and that the library is read only while its panel is open.
* `thin.rs`: the grep test of section 2. It allows `&mut Session` in `model.rs` only in the readers (`refresh`, `capture`, `read_*`) and checks that the file calls `.query(` and never `execute`.

## 9. Open points

* Search results show the text, the kind and the first page (100) of `element.query`; there is no paging control.
* The stats panel reads the bounds of the selection with `selection.get` (through `Session::query`, so no ring entry) and edits them with `element.set_bounds`, one value at a time. A rotated selection shows the bounds `selection.get` reports.
* The height the person drags the library drawer to is kept in `UiState::library_height` for the session and is not stored; an app that wants it between runs reads and writes the field, as for the layout preference.
* The first picture on the desktop opens the app's offscreen GPU device on the UI thread, which can take a moment; the pictures after it are drawn two a frame.
* The auto-resize buttons say Fit to text and Fixed width but do not show the current state: no query reports it.
* The head choices in the strip are the six common ones; the other heads are reachable through the palette.
* The menu, the context menu and the palette no longer wait for commands: every catalog command is registered.
* Touch: the touch layout is built (section 5a) and tested headless at iPad and iPhone sizes, also with safe-area insets (`ui/tests/touch_layout.rs`); no real tablet or phone has run it. The canvas gestures are the canvas layer's.
