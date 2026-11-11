# Feature parity audit

Audit of the rewrite against (A) every feature of the old drafft-ink app (`/home/ubuntu/git/drafft-ink` at 8ce40ab) and (B) every Excalidraw feature we want, except the AI features (text-to-diagram, diagram-to-code, magic frame), which the user excluded.

Audited tree: `main` at 5a8f9c6 (merge: backlog-a). The registry has 147 commands (`Registry::global()`, dumped with a throw-away example, not committed). The desktop and web shells (`crates/app`, `apps/drafftink`, `apps/drafftink-web`) are **not on main**: `apps/drafftink/src/main.rs` and `apps/drafftink-web/src/main.rs` are `fn main() {}`. They are on branches `p6-apps` and `p6-apps-web`, which I read with `git show` but did not build.

## How to read this

**Status**

| Status | Meaning |
|---|---|
| done | The behaviour exists, a user can reach it through a real surface (pointer path, key, button, menu, or the shell), and a test exercises it. |
| partial | Something exists but a piece is missing. The "Where" column says what exists and the gap list in section 4 says what is missing. A command with no pointer path, no dedicated UI control, or only a stored flag with no behaviour is partial. |
| missing | Nothing implements it (a field in the model with no way to set it counts as missing or partial, as stated). |
| out of scope | Excluded by the user (AI features) or a hosted service we will not run (telemetry, sign-in, Excalidraw+). |

**†** in the "Where" column: the last link in the chain is the app shell or the web shell, which is on `p6-apps` / `p6-apps-web` and not on main. The row counts as `done` because engine, UI and shell code exist and are tested at their own layer, but a user gets it only after those branches merge. Native macOS and Windows builds were never run by anyone (`docs/building.md` on p6-apps).

**Palette only**: every `palette: true` command is reachable through the command palette (`Mod+/`), with an argument form generated from its JSON Schema. I count that as a UI surface for agents and power users, but not as a replacement for a button, menu row or key that the old app or Excalidraw has. A feature that exists only that way is `partial`.

**Source column**
`A:n` old `app.rs` line, `U:n` old `ui.rs`, `EH:n` old `event_handler.rs`, `W:` old `drafftink-widgets`, `inv §n` `research/drafftink.md`, `X:` Excalidraw (file or key), `04 §n` `research/parts/04-features-diff.md`, `README` old README claim, `plan §6` `docs/plan.md`.

**Evidence column** names a test or scenario that exercises the feature. Paths: `eng/` = `crates/engine/src/`, `eng/int/` = `crates/engine/src/interact/`, `eng/cmd/` = `crates/engine/src/cmd/`, `S:` = `tests/scenarios/`, other crate names are under `crates/`. "none" means I found no test that exercises it; a feature with a partial status may still have tests for the part that exists.

Row ids (`A-nnn` old app, `B-nnn` Excalidraw) are stable so the gap list can refer to them.

---

## 1. Old app features (A)

### 1.1 Tools

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-001 | Select tool (`V`, `1`) | A:5245, inv §1 | done | `tool.set` select; toolbar button; keymap; pointer: `eng/int/select/` | `ui/tests/surfaces::every_tool_button_runs_tool_set_with_its_tool`, `ui/tests/keymap::every_registered_binding_dispatches_to_its_command_on_both_platforms` |
| A-002 | Pan tool (`H`) | A:5250, A:4226 | done | `tool.set` hand; canvas `PointerMode`; toolbar | `canvas/tests/recognizer::hand_mode_pans_and_draw_mode_drags_at_once` |
| A-003 | Rectangle tool (`R`, `2`) | A:5255 | done | `tool.set`, `element.create` kind rectangle; `eng/int/draw/shape.rs` | `eng/int/draw_tests::every_shape_tool_draws_one_element_with_one_command_and_one_undo_step` |
| A-004 | Ellipse tool (`O`, `4`) | A:5260 | done | same, kind ellipse | same test |
| A-005 | Arrow tool (`A`, `5`) | A:5265 | done | `tool.set`, `element.create` kind arrow; `eng/int/draw/linear.rs` | `eng/int/draw_line_tests::a_drag_with_the_arrow_tool_makes_an_arrow_with_a_head` |
| A-006 | Line tool (`L`, `6`) | A:5270 | done | same, kind line | `eng/int/draw_line_tests::a_drag_with_the_line_tool_makes_one_line_in_one_undo_step` |
| A-007 | Freehand tool (`P`, `7`) | A:5275 | done | `tool.set`; `eng/int/draw/stroke.rs`; one `element.create` per stroke, RDP 2.0 | `eng/int/draw_stroke_tests::a_mouse_stroke_is_one_create_with_no_pressures`, `eng/cmd/element_create_tests::a_freehand_stroke_is_simplified_with_the_old_tolerance_and_keeps_its_pressures` |
| A-008 | Highlighter tool (width at least 12, alpha 128; no key in the old app, toolbar only) | A:307, U:594 | done | toolbar button, `tool.set` highlighter; no key by design (the old tooltip's wrong "H" is gone) | `eng/int/draw_stroke_tests::the_highlighter_preview_is_already_wide_and_translucent`, `eng/cmd/element_create_tests::a_highlighter_is_wide_and_half_transparent_unless_the_style_says_otherwise` |
| A-009 | Eraser tool (`E`) | A:5290, EH:1285 | done | `tool.set`, `element.erase`; sweep fades what it touches, deletes at release as one undo step; radius in screen pixels (old: fixed 10 world px, one undo entry per hit) | `eng/int/draw_stroke_tests::the_eraser_fades_what_the_path_touches_and_deletes_it_in_one_step_at_the_release`, `eng/int/draw_stroke_tests::the_eraser_radius_is_in_screen_pixels` |
| A-010 | Text tool (`T`, `8`): click on empty creates, click on text edits | A:5280, EH:773 | done | `tool.set`, `text.edit_begin`, `text.edit_end`; editor is the app's egui field (†) | `eng/int/draw_stroke_tests::a_click_on_empty_canvas_with_the_text_tool_starts_a_draft_at_the_click`, `eng/cmd/text_tests::a_draft_is_created_by_edit_end_in_one_undo_step` |
| A-011 | Math tool (`M`, `9`): click creates `x^2 + y^2 = r^2`, click on a formula opens the editor | A:5285, EH:792 | done | `tool.set` math, `element.create` kind math, `ui.open_equation_editor` | `eng/int/draw_stroke_tests::a_math_click_on_empty_canvas_creates_the_placeholder_and_opens_no_editor`, `eng/int/draw_stroke_tests::a_math_click_on_a_formula_opens_the_equation_editor` |
| A-012 | Laser pointer (`Z`): fading local trail | A:5295, inv §1 | done | `tool.set` laser (`Z`, also `K`); `eng/int/draw/laser.rs` | `eng/int/draw_stroke_tests::the_trail_fades_by_the_clock_and_idle_means_idle` |
| A-013 | Calligraphy brush (mass-spring-damper smoothing) | A:3609, inv §1 | done | `style.set` `freehand_style` normal or calligraphy, panel row "Brush". The freehand gesture runs the pointer through a mass on a spring (M 0.5, K 120, C 15.49) and keeps a point once it is more than 2 units from the last one (`edit/src/brush.rs`). Same as the old app at 60 events a second; the spring now advances by the real time between events, so the feel no longer depends on the event rate. Applies to strokes drawn after the setting, as in the old app: `style.set` on an existing stroke changes the flag, not its points. The painter draws the stored points, so the brush has no render effect of its own. | `edit/tests/brush_old_app::the_calligraphy_brush_keeps_the_points_the_old_app_kept` (5 calligraphy cases of the old `ToolManager`, 1e-9), `eng/int/draw_stroke_tests::the_calligraphy_brush_trails_behind_the_pointer_and_drops_the_small_moves`, `eng/int/draw_stroke_tests::the_calligraphy_brush_rounds_a_corner`, `eng/int/draw_stroke_tests::exception_E7_a_brush_stroke_equals_the_plain_command_given_its_points`, `render/tests/painter::the_brush_is_applied_while_drawing_so_the_painter_draws_the_stored_points` |
| A-014 | Speed-based pressure simulation | A:3617, tools/mod.rs:205 | done | `style.set` `pressure_mode` uniform, simulated, device; panel row "Pressure". In `simulated`, a mouse or finger stroke gets one pressure per point: `exp(-v/800)` for the speed `v` in world units per second (at most 3 times 800, at least 0.2), blended 0.3 into the running value (`edit/src/brush.rs`). A pen keeps its own pressure. The painter uses the stored pressures unless the mode is `uniform`, so changing the mode of a drawn stroke in the panel changes how it looks (old: a global flag for new strokes only). | `edit/tests/brush_old_app::the_simulated_pressure_is_the_pressure_the_old_app_computed` (4 cases, 1e-9, injected clock), `eng/int/draw_stroke_tests::a_simulated_pressure_stroke_is_thick_where_the_mouse_was_slow_and_thin_where_it_was_fast`, `eng/int/draw_stroke_tests::the_simulated_pressure_shows_in_the_preview_while_drawing`, `eng/int/draw_stroke_tests::a_touch_stroke_gets_the_simulated_pressure_because_the_finger_has_none`, `render/tests/painter::the_pressure_mode_decides_whether_the_stored_pressures_shape_the_stroke` |
| A-015 | Real pen pressure (new in plan §6; the old app ignored `Touch.force`) | plan §6 | partial | Web: DOM pointer events carry pressure and tilt to `PointerMeta` (†). Native: winit gives none on X11, Wayland and macOS (`docs/shell.md` §6, §8); Windows is untested. | `eng/int/draw_stroke_tests::a_pen_stroke_sends_the_pressure_of_every_sample`, `eng/int/draw_stroke_tests::a_pen_keeps_its_own_pressure_when_the_mode_is_simulated`, `canvas/tests/recognizer::pen_samples_carry_pressure` |
| A-016 | `Esc` cancels the tool, closes dialogs, clears selection, returns to Select | A:5305 | done | `tool.cancel` (steps one rung per press; `all=true` is the old behaviour), `ui.close_overlays` first | `eng/cmd/tool_tests::esc_peels_one_rung_at_a_time_by_default`, `ui/tests/keymap::esc_closes_an_overlay_first_and_otherwise_cancels` |

### 1.2 Shapes (the 9 old `Shape` kinds)

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-020 | Rectangle with corner radius and rotation | inv §1 | done | scene `Kind::Rectangle`, `corner_radius`, `transform.angle` | `sketch/tests/behavior::rounded_rect_radius_is_limited_to_half_the_short_side`, `eng/cmd/element_tests::rotate_by_and_to_with_snapping` |
| A-021 | Ellipse with rotation | inv §1 | done | `Kind::Ellipse` | `eng/cmd/element_create_tests::diamond_ellipse_and_frame_are_boxes_of_their_kind` |
| A-022 | Line: start, end, intermediate points, closed | inv §1 | done | `Kind::Line`, `LinearData`; point handles and midpoints | `canvas-js/tests/interaction::point_and_midpoint_handles_reshape_a_line`, `canvas/tests/hit::closed_lines_and_freehand_are_hit_inside_only_when_filled` |
| A-023 | Arrow (same plus head size); old had one open V head | inv §1 | done | `Kind::Arrow`; now start and end heads, bindings | `eng/cmd/arrow_tests::set_head_sets_removes_and_undoes` |
| A-024 | Freehand: points, per-point pressure, closed | inv §1 | done | `Kind::Freehand`; variable-width outline `sketch/src/freehand.rs` | `render/tests/gpu::freehand_matches_the_old_renderer`, `io/tests/legacy::every_legacy_document_loads_validates_and_round_trips` |
| A-025 | Text shape (font, weight, size, per-character colour) | inv §1 | done | `Kind::Text`, `TextData` with `runs` | `typeset/tests/legacy.rs` (old text sizes), `S:fixtures/legacy_text_fonts_loads.json` |
| A-026 | Group shape (nested, children embedded by value) | inv §4 | done | Changed on purpose to flat `group_ids` stacks (plan §6 item 2): children stay addressable elements | `eng/cmd/group_tests::old_weakness_children_stay_addressable_elements`, `S:group/group_is_one_unit.json`, `S:fixtures/legacy_groups_nested_loads.json` |
| A-027 | Image shape (embedded bytes, PNG/JPEG/WebP) | inv §1 | done | `Kind::Image`, content-addressed file table | `eng/cmd/image_tests::several_pictures_cascade_by_offset_and_the_same_bytes_are_stored_once`, `S:fixtures/legacy_images_loads.json` |
| A-028 | Math shape (LaTeX via ReX, rotation) | inv §1 | done | `Kind::Math`, `typeset/src/math.rs` | `typeset/tests/math.rs`, `S:fixtures/legacy_math_loads.json` |
| A-029 | Group rotation (old: cosmetic, lost on save and ungroup) | inv §4 items 1, 2 | done | Fixed: each member's own angle changes | `eng/cmd/group_tests::old_weakness_member_rotation_survives_group_and_ungroup`, `io/tests/legacy::group_rotation_is_baked_into_children` |

### 1.3 Styles, sketch look, grid

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-030 | Stroke colour, full RGBA, 22 families by shades, hex input | U:1020, W/colors.rs | done | `style.set` `stroke_color`; top bar swatches; colour popover with hex field (`#rrggbbaa`) | `ui/tests/color_popover::the_grid_has_22_families_by_11_shades_and_a_quick_column_of_9`, `ui/tests/surfaces::top_bar_swatches_and_widths_run_style_set` |
| A-031 | Fill colour or none | U:1094 | done | `style.set` `fill_color` (null = none); top bar | same tests |
| A-032 | Stroke width 1, 2, 4, 8 | U:197 | done | `style.set` `stroke_width`; top bar | `ui/tests/surfaces::top_bar_swatches_and_widths_run_style_set` |
| A-033 | Opacity slider | U:1778 | done | `style.set` `opacity`, scrubbed as one gesture (one undo step) | `eng/cmd/style_tests::scrubbing_opacity_is_one_gesture_and_one_undo_step`, `ui/tests/routing::a_drag_of_the_opacity_slider_is_one_undo_step_on_a_real_session` |
| A-034 | Fill patterns: Solid, Hachure, ZigZag, CrossHatch, Dots, Dashed, ZigZagLine (old UI exposed only 4) | U:1572 | done | `style.set` `fill_pattern`; panel row shows all 7 | `eng/cmd/style_tests::all_seven_fill_patterns_are_accepted`, `sketch/tests/old_app::fill_patterns_match_old_app` |
| A-035 | Sloppiness: Architect, Artist, Cartoonist, Drunk | U:1530 | done | `style.set` `sloppiness`; `sketch` crate | `sketch/tests/behavior::roughness_zero_is_the_clean_geometry_in_one_stroke`, `sketch/tests/old_app.rs` |
| A-036 | Hand-drawn outline (custom jitter) and seeded stable look | vello_impl.rs:130 | done | `drafftink-sketch`, seed per element | `render/tests/gpu::freehand_matches_the_old_renderer`, `sketch/tests/old_app.rs` |
| A-037 | Rounded corners on or off (32) | U:1505 | done | `style.set` `corner_radius`; panel "Corners" Sharp / Round | `eng/cmd/style_tests::get_corner_radius_ignores_elements_that_cannot_have_one` |
| A-038 | Stroke style solid, dashed, dotted (old: lines and arrows only) | U:1622 | done | `style.set` `stroke_style`, now on every shape except text, math, image | `render/tests/painter::dashes_change_the_stroke_style`, `eng/cmd/style_tests::applicability_of_fill_pattern_stroke_style_and_freehand_props` |
| A-039 | Path style Direct, Flowing, Angular for lines and arrows | U:1594 | done | Changed to `arrow.set_type` sharp, round, elbow (Angular is now a real routed elbow, stored and hit-testable) | `eng/cmd/arrow_tests::set_type_round_keeps_points_and_undoes`, `edit/tests/arrows_elbow.rs` |
| A-040 | Page background colour | U:838 | done | `doc.set_background`; stored with the document and used by export (old: app-only, not exported) | `eng/cmd/doc_tests::set_background_is_saved_undoable_and_accepts_palette_names` |
| A-041 | Grid styles None, Lines, HorizontalLines, Crosses, Dots; grid size 20 | U:817 | done | `settings.cycle` `view.grid_style`; bottom bar button; `render/src/grid.rs` with a cell cap | `eng/cmd/settings_tests::cycle_walks_the_grid_styles_in_the_old_order_and_wraps`, `render/tests/grid::the_old_worst_case_is_now_small` |
| A-042 | Colour popover: 22 families, quick column, last-picked | W/colors.rs:805 | partial | Popover, hex field and quick column exist. The "last picked" swatch of the old top bar and recent or custom colours do not. | `ui/tests/color_popover::the_quick_column_and_the_shade_rows` |
| A-043 | Style panel follows the selection and the tool (old: fought the selection every frame) | inv §2 | done | `style.get` mixed values; panel shows "mixed" | `eng/cmd/style_tests::get_follows_a_set_and_ignores_view_only_and_gestures`, `ui/tests/surfaces::right_panel_shows_for_a_selection_and_its_buttons_run_style_set` |
| A-044 | Style defaults for new elements | inv §2 | done | `settings` `style.defaults`; persisted (old: reset every launch) | `eng/cmd/settings_tests::style_defaults_reach_the_style_of_new_elements_at_once` |
| A-045 | Every style change is undoable and reaches peers (old: only opacity, no peers for fonts) | A:2606-3130 | done | one path: `style.set` | `eng/cmd/style_tests.rs`, `S:style/set_get_theme.json` |

### 1.4 Text, fonts, math, images

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-050 | Font families GelPen, GelPen Serif, Vanilla Extract, Noto Sans (old UI button commented out) | U:1371 | done | `style.set` `font_family`; panel buttons and font popover (`S` key verb) | `ui/tests/color_popover::every_font_family_has_its_setting_value` |
| A-051 | Font weights Light, Regular, Heavy | U:1408 | done | `style.set` `font_weight` | `eng/cmd/style_tests.rs` |
| A-052 | Font sizes S/M/L/XL (16/20/28/36) and math size | U:1436 | done | `style.set` `font_size`; `text.step_size` (`Mod+Shift+,` and `.`) | `eng/cmd/text_tests::step_size_walks_the_presets_and_undoes` |
| A-053 | Per-character colour (`char_colors`) | inv §1 | done | Model (`TextRun`), `text.style_range`, rendering, legacy import. In the app: select characters in the on-canvas editor and pick a colour in the style panel; the range colour goes with `text.edit_end` (`runs`). Typed text takes the colour of the character before it. | `eng/cmd/text_tests::style_range_sets_and_clears_character_colours`, `edit/tests/text_edit_model::colouring_a_selection_only_changes_that_range_and_is_reported_in_characters`, `app/editor/tests::a_colour_chosen_with_a_selection_colours_that_range_and_goes_with_the_result`, `app/editor/pixels::the_editor_draws_the_pixels_of_the_committed_text`, `scripts/web_check.py --app` |
| A-054 | Text editing: caret, selection, clipboard, word moves, IME | A:4548 | done | Editor-local (`docs/exceptions.md` E16): the on-canvas editor (model `edit::text_edit`, drawn by the renderer like the committed text, caret and selection in the overlay) with mouse selection, double and triple click, word and line motions, clipboard, its own undo and IME composition (†); `Mod+Enter` and `Esc` end it; edits are one undo step (old: not undoable) | `eng/cmd/text_tests::editing_an_existing_text_replaces_it_and_a_blank_result_deletes_it`, `edit/tests/text_edit_model`, `app/editor/tests`, `app/editor/pixels`, `eng/int/select_tests::text_editor`, `ui/tests/keymap::during_a_text_edit_only_text_always_and_commit_gate_chords_apply`; a real OS input method: not tried (`docs/shell.md` §8), the browser textarea path: `scripts/web_check.py --app` |
| A-055 | LaTeX math: separate Math shape plus equation editor dialog | U:2911, README | done | `element.create` math, `text.set_latex`, `text.edit_begin` with `latex`, `ui.open_equation_editor`, dialog with Apply, Cancel, Copy, Paste | `eng/cmd/text_tests::set_latex_lays_the_formula_out_once_and_is_undoable`, `ui/tests/equation_dialog::apply_sends_the_edited_text`, `eng/int/select_tests::double_click_on_math_opens_the_equation_editor` |
| A-056 | Inline LaTeX inside text (README claim, never existed) | README | done | **Done** below the UI: `$...$` in a text is typeset as Parley inline boxes holding ReX layouts (`docs/typeset.md`, rules in `docs/model.md`), measured, drawn (light, dark, LOD), wrapped, exported (SVG outlines, Excalidraw source text) and accepted by `text.set` / `text.edit_end` (`inline_math`). The app's text editor shows the typeset formula and opens its source while the caret is inside it (`TextSpec::reveal`, `F2` or a double click opens it, `Mod+Shift+M` turns formulas off for the text). | `typeset/tests/inline`, `render/tests/painter::a_formula_inside_text_is_drawn_with_the_math_font_in_the_text_colour`, `render/tests/gpu::inline_math_draws_ink_where_the_layout_put_the_formula`, `eng/cmd/text_tests::set_measures_a_formula_as_the_typesetter_draws_it`, `scenarios/text/inline_math_text` |
| A-057 | Images: drag-and-drop, paste from clipboard, fit to 800 | A:5376, A:171 | done | `image.insert` (fit 800, cascade, dedupe), `doc.drop`, `clipboard.paste`; shell reads files and clipboard (†) | `eng/cmd/clipboard_tests::an_image_on_the_clipboard_is_inserted_with_the_800_rule`, `eng/cmd/doc_tests::drop_hands_images_to_image_insert_and_warns_when_it_is_missing` |
| A-058 | Image from a file picker (no old equivalent; menu row "Insert image") | NEW | done | `ui.insert_image` (key `I`, menu) then `image.insert` (†) | `ui/tests/keymap::ui_verb_keys_do_not_collide_with_command_keys`; the picker itself: none |
| A-059 | Dropped PNG with an embedded scene restores the document | A:5376 | done | `doc.drop`, `doc.open` format png; the old `zTXt` chunk is read | `io/tests/formats::png_reads_the_old_apps_ztxt_chunk`, `eng/cmd/doc_tests::drop_with_a_scene_png_inserts_it_as_an_image_unless_asked_to_open_it` |

### 1.5 Selection and transform

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-060 | Click select, Shift-click toggle, select all | EH:486, A:4777 | done | `selection.select_at`, `selection.all` (`Mod+A`); `eng/int/select/click.rs` | `eng/int/select_tests::a_click_selects_shift_click_toggles_and_a_click_on_empty_canvas_clears` |
| A-061 | Marquee (touch semantics; Shift adds) | EH:539 | done | `selection.select_region`; setting `input.box_select` overlap or contain | `eng/int/select_tests::shift_marquee_adds_to_the_selection`, `eng/cmd/selection_tests::the_default_of_contain_is_the_setting_input_box_select` |
| A-062 | Multi-move with grid and smart-guide snap | EH:1105 | done | `element.move` gesture | `eng/int/select_tests::dragging_a_rectangle_moves_it_in_one_undo_step_and_one_ring_entry`, `eng/cmd/element_tests::move_snaps_to_the_grid_and_to_smart_guides_but_not_a_click` |
| A-063 | Alt-drag duplicate | EH:514 | done | `element.move` with `duplicate` | `eng/int/select_tests::alt_drag_duplicates_and_selects_the_copy` |
| A-064 | Corner resize, Shift keeps aspect | EH:876 | done | `element.resize` gesture | `eng/int/select_tests::resizing_from_a_handle_changes_the_box_and_shows_the_size`, `eng/int/select_tests::resizing_a_multi_selection_scales_all_of_it` |
| A-065 | Line endpoint, intermediate point and segment-midpoint handles | EH:876 | done | `arrow.move_point`, `arrow.add_point` gestures | `eng/int/select_tests::dragging_a_vertex_of_a_polyline_moves_only_that_vertex` |
| A-066 | Rotate handle (Shift snaps 15 degrees), double-click resets | EH:447 | done | `element.rotate` gesture | `eng/int/select_tests::shift_rotates_in_steps_of_fifteen_degrees`, `eng/int/select_tests::double_click_on_the_rotate_handle_resets_the_angle` |
| A-067 | Arrow-key nudge by 20 | A:5338 | done | `element.nudge` (setting `input.nudge_step` 20; `Shift` = 1); one undo step per burst | `eng/cmd/element_tests::nudge_moves_by_the_setting_one_unit_when_fine_or_a_given_step`, `eng/executor_tests::coalescing_keeps_targets_commands_and_gaps_apart` |
| A-068 | Flip horizontal and vertical | U:1753 | done | `element.flip`; panel buttons; `Shift+H`, `Shift+V` | `eng/cmd/element_tests::flip_mirrors_about_the_common_centre` |
| A-069 | Group, ungroup (old: `Ctrl+G` only, no UI, no enter) | A:4973 | done | `group.create`, `group.ungroup`; panel and context menu buttons; double-click enters, `Esc` leaves | `S:group/group_click_enter_move_exit_ungroup.json`, `eng/int/select_tests::double_click_on_a_group_enters_it_and_the_next_click_selects_a_member` |
| A-070 | Z-order: front, back, forward, backward | U:1703 | done | `arrange.z_order`; panel buttons, context menu, `Mod+[` `Mod+]` | `eng/cmd/arrange_tests::z_order_front_back_forward_backward_and_undo` |
| A-071 | Align six ways (old: names inverted) | U:1807 | done | `arrange.align` (`left`, `center_x`, ...); panel buttons for 2 or more, context menu; `Mod+Shift+Arrow` | `eng/cmd/arrange_tests::align_edges_and_centres_by_the_axis_they_equalise`, `ui/tests/context_menu::the_align_rows_cover_every_edge_of_the_command` |
| A-072 | Duplicate (`Ctrl+D`, offset 20) | A:5210 | done | `element.duplicate` | `eng/cmd/element_tests::duplicate_copies_offsets_and_selects_the_copies` |
| A-073 | Delete | A:5299 | done | `element.delete`; `Delete`, `Backspace`; panel; context menu | `S:element/create_transform_delete_undo.json` |
| A-074 | Hover cursor icons (resize, move, grab, crosshair) | A:4103 | done | `Outcome.cursor` from the interaction layer; shell maps it (†) | `canvas/tests/canvas::handles_win_over_elements_and_report_their_cursor` |
| A-075 | Edge resize handles (declared, never produced in the old app) | inv §1 | done | `Handle::Top` etc. map to `BoxHandle::N` ...; `element.resize` handles n, e, s, w | `eng/cmd/element_tests::resize_edge_handles_stretch_one_axis_and_default_handle_is_se` |
| A-076 | Text and math resize by their handles (scale the font) | EH:378 | done | `element.resize`: a corner scales the font, a side changes the wrap width (text) | `edit/tests/structure_transform::text_scales_its_font_on_a_corner_and_wraps_on_a_side`, `edit/tests/structure_transform::math_scales_its_font_size` |
| A-077 | Size and angle badges while resizing or rotating; selection and hover outlines | EH:1105 | done | `Outcome` overlay badges from the gesture frame output | `eng/int/select_tests::rotating_with_the_handle_turns_the_element_and_shows_the_angle`, `eng/int/frame_out.rs::a_frame_output_fills_guides_highlight_and_badge` |

### 1.6 Snapping

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-080 | Grid snap (20) | U:956 | done | setting `snap.grid`; bottom bar toggle; moves, resizes, new shapes, line ends | `eng/int/select_tests::grid_snap_follows_the_settings_and_ctrl_turns_it_off`, `eng/int/draw_tests::the_grid_snaps_both_corners_when_the_setting_is_on` |
| A-081 | Smart guides (alignment, threshold 8) | U:975, snap.rs | done | setting `snap.smart_guides` (`Alt+S`); guides in the overlay, equal-spacing guides with a distance badge | `eng/int/draw_tests::smart_guides_snap_the_dragged_corner_to_another_element_and_show_while_dragging`, `eng/cmd/settings_tests::alt_s_toggles_smart_guides_and_mod_quote_cycles_the_grid` |
| A-082 | Equal-spacing guides (radius 800, caps 50 and 200) | snap.rs | done | `drafftink-geom::guides`, used by move and resize | `geom/src/guides.rs::equal_spacing_between_two_boxes`, `geom/src/guides.rs::equal_spacing_continues_a_row` |
| A-083 | Angle snap 15 degrees for lines | U:988 | done | setting `snap.angle`, `snap.angle_step`; `Shift` locks the angle | `eng/int/draw_line_tests::shift_locks_the_angle_to_fifteen_degrees_and_keeps_the_length`, `eng/int/draw_line_tests::the_angle_step_setting_changes_the_lock` |
| A-084 | Endpoint snap to guides and grid while dragging an arrow end | snap.rs | done | `arrow.move_point` snap argument | `eng/cmd/arrow_tests::smart_guides_align_the_point_with_other_elements_and_report_the_guide` |
| A-085 | Three snap toggles in the bottom bar (the old "isometric" angle snap parameter was never used, so there is nothing to port) | U:960-1000 | done | `settings.toggle` | `ui/tests/surfaces::bottom_bar_buttons_run_their_commands` |

### 1.7 Clipboard

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-090 | Copy, cut, paste of shapes (old: in-process only; keyboard and button pasted at different places) | A:5061, A:3384 | done | `clipboard.copy`, `clipboard.cut`, `clipboard.paste` (one path, `at` = cursor or viewport centre); payload goes to the system clipboard (†) | `eng/cmd/clipboard_tests::paste_centres_the_copy_on_at_with_new_ids_and_selects_it`, `eng/cmd/clipboard_tests::paste_works_across_the_system_clipboard_between_sessions` |
| A-091 | Paste routing: own payload, Excalidraw JSON, Mermaid text, image, then plain text as a new text element | A:5106 | done | `clipboard.paste` | `eng/cmd/clipboard_tests::excalidraw_clipboard_text_is_pasted_as_elements`, `eng/cmd/clipboard_tests::mermaid_text_goes_to_the_diagram_command_when_it_exists`, `eng/cmd/clipboard_tests::plain_text_skips_the_sniffing` |
| A-092 | Copy selection as PNG to the OS clipboard (`Ctrl+Shift+E`, `Ctrl+Shift+C`) | A:4801, A:4992 | done | `clipboard.copy_as` png; menu "Copy selection as PNG"; one render path with `doc.export` (†) | `eng/cmd/clipboard_tests::copy_as_png_renders_the_selection_and_writes_the_image`, `eng/cmd/clipboard_tests::copy_as_png_and_doc_export_make_the_same_render_request` |
| A-093 | Text clipboard inside the editor | A:4575 | done | editor-local, egui field (†) | none (egui behaviour) |
| A-094 | Paste at the click point of the context menu | new | done | context menu `Paste` passes the world point | `ui/tests/context_menu::paste_puts_the_content_where_the_menu_was_opened` |

### 1.8 Import and export

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-100 | Save document as JSON (old JSON has no version field) | A:4784 | done | `doc.save` (versioned native format, `Mod+S`); target is a file, the browser download or storage | `eng/cmd/doc_tests::save_writes_the_document_marks_it_saved_and_remembers_the_target`, `io/tests/formats::native_save_load_round_trip_and_errors` |
| A-101 | Open the old app's JSON documents and clipboard arrays | A:4790 | done | `doc.open` sniffs old JSON, gzip and Loro snapshots (`io/src/legacy`) | `io/tests/legacy::every_legacy_document_loads_validates_and_round_trips`, `S:fixtures/legacy_all_shapes_loads.json` and 13 more `legacy_*` fixtures |
| A-102 | Save As with a document name dialog | U:2631 | partial | `ui.save_as` opens the host's save picker; `doc.rename` and `tab.rename` have **no UI** (palette only), so a user cannot name a document in the app | `ui/tests/small_dialogs::save_first_repeats_the_call_with_on_unsaved_save_and_keeps_the_origin` (the confirm path only) |
| A-103 | Open dialog and Open Recent (list of stored documents, cap 10) | U:2720, U:2800 | done | `ui.open_file`, `ui.open_recent`, setting `doc.recent`, `doc.open` | `ui/tests/small_dialogs::picking_a_recent_document_opens_it_replacing_the_current_one` |
| A-104 | Clear canvas, Show Intro | U:1919, U:1924 | done | `doc.clear` (undoable, `Mod+Delete`), `doc.show_intro`; menu rows | `eng/cmd/doc_tests::clear_deletes_everything_locked_ones_too_and_undoes_in_one_step`, `eng/cmd/doc_tests::show_intro_replaces_the_document_with_the_welcome_document` |
| A-105 | Export PNG: selection or all, scale 1x/2x/3x, padding 20, scene embedded in the file | A:4870, U:1991 | done | `doc.export` png (`Mod+E`); menu row and scale row (setting `export.scale`); renderer port (†) | `eng/cmd/doc_tests::export_png_goes_through_the_renderer_with_the_scene_embedded`, `io/tests/formats::png_scene_round_trip_with_unicode` |
| A-106 | Export JSON | A:2776 | done | `doc.export` json | `eng/cmd/doc_tests::export_json_and_excalidraw_round_trip_through_open` |
| A-107 | Excalidraw `.excalidraw` import, file and clipboard | inv §1 | done | `doc.open` / `doc.drop` / `clipboard.paste`; bindings, groups, frames, text containers, images, elbow kept (old importer dropped them) | `io/tests/excalidraw::basic_shapes_map_every_style_field`, `io/tests/excalidraw::bindings_keep_both_ends_with_their_targets` |
| A-108 | Excalidraw library `.excalidrawlib` opened as a new tab, grid layout | U:717, A:1689 | done | `library.import` mode `open_as_tab`; menu row "Open a library as a tab" (†); `doc.drop` | `io/tests/excalidraw::grid_layout_matches_the_old_positions`, `eng/cmd/library_tests::import_open_as_tab_lays_the_items_out_in_a_new_tab_and_leaves_the_library_alone` |
| A-109 | Mermaid import from the clipboard: flowchart and sequence only | U:1945 | done | `diagram.import_mermaid` (menu, paste sniffing); now bound arrows, nested groups for subgraphs, styles | `io/tests/mermaid::flowchart_fixtures_match_the_old_output`, `eng/cmd/diagram_tests::a_mermaid_flowchart_becomes_a_grouped_diagram_with_bound_arrows` |
| A-110 | Download JSON, Import JSON or Excalidraw rows (web only) | U:1934 | done | folded into `doc.save` / `ui.open_file`; browser download and file input in the web shell (†) | none for the web glue (`p6-apps-web`) |
| A-111 | PNG embeds the scene (`zTXt` chunk) and re-imports on drop | A:1438 | done | `iTXt` now (UTF-8), the old `zTXt` is still read | `io/tests/formats::png_reads_the_old_apps_ztxt_chunk` |

### 1.9 View, camera, input devices

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-120 | Wheel pans, Ctrl+wheel zooms at the cursor (step 1.1) | A:4434 | done | canvas recognizer, `view.zoom_at`, `view.pan`, then `view.set` | `canvas/tests/canvas::wheel_pans_the_view_and_ctrl_wheel_zooms_at_the_cursor`, `eng/int/interaction_tests::a_plain_wheel_pans_and_a_pinch_zooms_and_ends_with_view_set` |
| A-121 | Middle-button drag pans | A:4244 | done | recognizer `middle_button_pans` | `eng/int/interaction_tests::a_middle_button_drag_pans_whatever_the_tool` |
| A-122 | Two-finger touch pan and pinch | A:4461, input.rs:208 | done | recognizer; a second finger cancels a one-finger gesture (old: left the stroke running) | `canvas/tests/recognizer::two_fingers_pinch_and_pan_and_the_rest_is_ignored`, `eng/int/draw_stroke_tests::a_second_finger_cancels_a_one_finger_stroke` |
| A-123 | Trackpad pinch (not handled in the old app) | inv §1 | done | egui zoom events become wheel zoom events in the app (†) | `eng/int/interaction_tests::a_plain_wheel_pans_and_a_pinch_zooms_and_ends_with_view_set` |
| A-124 | Zoom out, reset, in buttons and percent label | U:862-920 | done | `view.zoom_out`, `view.zoom_reset`, `view.zoom_in`; bottom bar; also `Mod+-`, `Mod+0`, `Mod+=` | `eng/cmd/view_tests::zoom_in_and_out_step_by_a_quarter_and_clamp`, `ui/tests/surfaces::the_bottom_bar_changes_the_zoom_and_the_undo_button_undoes` |
| A-125 | Centre canvas at the origin; zoom to fit selection or all (padding 50) | U:924, U:937 | done | `view.center`, `view.zoom_to_fit`; bottom bar; `Shift+1/2/3` | `eng/cmd/view_tests::center_moves_the_camera_and_keeps_the_zoom`, `eng/cmd/view_tests::fit_auto_prefers_the_selection_and_scope_ids_resolves_refs` |
| A-126 | Zoom range 0.1 to 10 | inv §3 | done | now 0.1 to 30 (decision 10) | `canvas/tests/canvas::zoom_limits_hold_through_input` |
| A-127 | Touch input drives tools (draw, move) with larger slop | A:4461 | done | `PointerKind::Touch`, slop 8 px for fingers | `eng/int/select_tests::a_touch_drag_moves_an_element`, `canvas/tests/recognizer::the_slop_is_configurable_and_larger_for_fingers` |
| A-128 | Mobile or phone layout | README | missing | Panels are fixed-size egui areas; "a larger touch mode is not built" (`docs/ui.md` §9); no native mobile target | none |
| A-129 | Cmd on macOS as the modifier | inv §8 | done | `Mod` = Cmd on macOS, Ctrl elsewhere, in the keymap and the web shell. Never run on a Mac. | `ui/tests/keymap::mod_is_ctrl_on_linux_and_windows_and_cmd_on_macos` |
| A-130 | Space as a virtual left button | A:4711 | done | Replaced by design (decision 1): Space held = hand tool (`tool.hold`) | `ui/tests/keymap::holding_space_holds_the_hand_tool_and_releasing_ends_it`, `eng/int/interaction_tests::space_hold_pans_instead_of_selecting_or_moving` |

### 1.10 Undo and redo

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-135 | Undo and redo (50 snapshot steps, keyboard and button diverged) | A:4944, A:3131 | done | `history.undo`, `history.redo`: delta steps, depth 200 (setting `history.depth`), restore selection | `eng/cmd/history_tests::undo_takes_back_the_last_step_and_restores_the_selection` |
| A-136 | Style, text and font changes undoable (old: not) | inv §8 | done | all are commands with `Undo::Undoable` | `eng/cmd/text_tests::set_replaces_the_content_remeasures_and_undoes_in_one_step` |
| A-137 | Undo in a room does not rewrite peers' edits (old: whole-doc snapshot) | inv §3 | done | per-user undo over incremental CRDT updates | `eng/cmd/history_tests::undo_never_reverts_a_collaborators_edit_and_its_inverse_is_a_normal_edit_for_peers`, `eng/cmd/collab_tests::undo_from_the_keyboard_is_sent_and_leaves_the_other_peers_edit_alone` |

### 1.11 Panels, menus, dialogs

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-140 | Tool column (12 tools, now 14 with diamond and frame) plus tool lock | U:726 | done | `ui/src/surfaces/toolbar.rs` | `ui/tests/surfaces::every_tool_button_runs_tool_set_with_its_tool`, `ui/tests/surfaces::tool_lock_button_runs_tool_lock` |
| A-141 | Properties bar (stroke, fill, width) | U:1020 | done | `topbar.rs` | `ui/tests/surfaces::top_bar_swatches_and_widths_run_style_set` |
| A-142 | Right panel: font, weight, size, corners, sloppiness, fill pattern, path, stroke, brush, pressure, layer, flip, opacity, align | U:1326 | done | `props.rs`; shown for a selection or a drawing tool | `ui/tests/surfaces::right_panel_shows_for_a_selection_and_its_buttons_run_style_set` |
| A-143 | Hamburger menu and collab button with status dot | U:1860, U:2105 | done | `menu.rs`; menu rows are commands or UI verbs, origin `menu` | `ui/tests/surfaces::menu_opens_by_verb_and_its_rows_run_with_the_menu_origin` |
| A-144 | Bottom-left toolbar: undo, redo, grid, background, zoom, centre, fit, 3 snap toggles | U:756 | done | `bottom.rs` | `ui/tests/surfaces::bottom_bar_buttons_run_their_commands` |
| A-145 | Tab bar (switch, close, new) | U:659 | done | `tabs.rs`; `tab.new` is a real blank tab now (old: only via library) | `ui/tests/surfaces::tab_bar_switches_closes_and_adds_tabs`, `S:tab/new_rename_switch_close.json` |
| A-146 | Presence panel (max 8 peers) | U:2517 | done | `collab.rs` | `ui/tests/collab_dialog::presence_shows_eight_and_the_rest_as_more` |
| A-147 | Collaboration modal: server URL, connect, room, join, leave, name, 9 colours | U:2169 | done | `collab.rs`; `collab.connect`, `join_room`, `leave_room`, `set_user` | `ui/tests/collab_dialog::a_connected_session_leaves_its_room_and_joins_again_through_the_buttons`, `ui/tests/collab_dialog::nine_colours_send_set_user_and_the_current_one_is_marked` |
| A-148 | Keyboard shortcuts modal (old: 19 stale entries) | U:2558 | done | generated from the registry; `?` opens it; searchable | `ui/tests/shortcuts::every_bound_command_and_titled_binding_of_the_registry_has_a_row` |
| A-149 | Math editor dialog | U:2911 | done | `equation.rs` | `ui/tests/equation_dialog::mod_enter_applies_and_does_not_add_a_line` |
| A-150 | Unsaved-changes confirm (new) and refusals as toasts | new | done | `dialogs.rs`, `toasts.rs` | `ui/tests/small_dialogs::discard_repeats_the_call_with_on_unsaved_discard`, `ui/tests/routing.rs` |
| A-151 | All UI actions of a frame are applied (old: first one only) | inv §2 | done | `UiShell::frame` returns every action | `ui/tests/routing.rs`, `ui/tests/frame.rs` |
| A-152 | Export scale row in the menu | U:1991 | done | `settings.set export.scale` | `ui/tests/surfaces::menu_opens_by_verb_and_its_rows_run_with_the_menu_origin` |

### 1.12 Collaboration

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-155 | WebSocket relay server, rooms by name, Loro updates | inv §7 | done | `apps/drafftink-server` (room per document, persistence, limits, auth token, serves the web client with `--web-dir`), `crates/collab` | `apps/drafftink-server/tests/sync.rs`, `apps/drafftink-server/tests/persistence.rs`, `collab/tests/server.rs::auth_off_ignores_the_token_and_auth_on_checks_it` |
| A-156 | Join, leave, connect, disconnect, name, colour | A:3160-3236 | done | `collab.*` | `eng/cmd/collab_tests::join_room_needs_a_server_a_valid_name_and_no_current_room`, `S:collab/connect_join_leave_presence.json` |
| A-157 | Auto-join by `?room=&server=` or `#room=&server=`; native `--server --room` | A:1957, A:1989 | done | `collab.connect` link forms; app start-up link (†) | `eng/cmd/collab_tests::room_links_follow_the_old_apps_forms`, `crates/app/tests/app.rs::a_room_link_joins_the_room_at_start` (on p6-apps) |
| A-158 | `wss://` support | inv §1 | done | `ServerUrl` accepts ws and wss; plain ws only to loopback unless opted in | `eng/cmd/settings_tests::server_urls_must_be_websocket_addresses_or_a_path` |
| A-159 | Presence: cursor, name, colour; remote cursor markers | A:3924 | done | Awareness carries cursor, name, colour and selection. `render::RemoteCursor` draws the marker in the peer's colour with the name in a chip beside it (†: the app maps `collab_view()` into the overlay, `docs/render.md` §5). | `eng/cmd/collab_tests::pointers_names_and_selections_reach_the_other_peer` |
| A-160 | Real CRDT merge (old: whole-document snapshot replace) | inv §3 | done | per-field maps over Loro, incremental updates, schema version | `eng/cmd/collab_tests::concurrent_edits_made_with_commands_converge`, `crates/doc/tests/` |
| A-161 | Reconnect with backoff (old: none), protocol version (old: none) | inv §8 | done | `crates/collab` state machine; `meta.schema` | `collab/tests/state_machine::the_backoff_is_capped_and_resets_after_a_stable_connection` |
| A-162 | Laser trail visible to peers (old: local only) | 04 §1.14 | done | `collab.laser` throttled; peers' trails drawn by `render::LaserTrail` (†: the app maps `PeerView::trail`) | `eng/cmd/collab_tests::the_laser_fades_locally_and_reaches_peers_as_a_trail` |

### 1.13 Persistence, settings, tabs

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-165 | Autosave (web only, shape-count detector, 5 s, only on redraw) | A:2564 | done | `doc.autosave` workspace record with every tab, revision-based, setting `autosave.interval_s`; timer from `next_deadline`; all platforms (†) | `eng/cmd/tab_tests::steady_work_is_written_at_the_latest_after_the_interval`, `eng/cmd/tab_tests::all_tabs_come_back_after_a_restart_with_their_state_but_no_history` |
| A-166 | Restore the last session at start (old web only; native always showed the intro) | A:1860 | done | `doc.open` stored autosave; first run shows the intro (†) | `crates/app/tests/app.rs::the_second_run_restores_the_workspace_instead_of_the_intro` (on p6-apps), `eng/cmd/doc_tests::opening_the_autosave_restores_the_active_tab_as_unsaved_work` |
| A-167 | Settings persistence (old: none) | inv §8 | done | `settings.*`, 25 keys in `docs/commands.md` §5, `Storage` port (†) | `eng/cmd/settings_tests::settings_survive_a_restart` |
| A-168 | Settings dialog (old: none; Excalidraw has Preferences) | 04 §1.6 | partial | Only the three snap toggles, grid, background, export scale and theme have controls. The other keys (`arrow.binding`, `arrow.midpoint_snap`, `input.box_select`, `input.nudge_step`, `history.depth`, `autosave.*`, `frame.*`, `clipboard.excalidraw_compat`) are reachable by the palette only. | `eng/cmd/settings_tests::set_applies_at_once_and_stores_only_what_differs_from_the_default` |
| A-169 | Canvas tabs: switch, close, per-tab state | A:1725 | done | `tab.switch`, `tab.close`; each tab keeps selection, camera, history; tabs persist | `eng/cmd/tab_tests::each_tab_keeps_its_own_selection_camera_and_history`, `S:tab/new_rename_switch_close.json` |
| A-170 | Tab rename, move, duplicate | new | partial | `tab.rename`, `tab.move`, `tab.duplicate` exist; **no UI control** (palette only) | `eng/cmd/tab_tests::duplicate_puts_the_copy_after_its_source_and_activates_it` |
| A-171 | Intro document | assets/intro.json | done | `doc.show_intro`, `doc::intro_document` | `eng/cmd/doc_tests::show_intro_replaces_the_document_with_the_welcome_document`, `S:fixtures/legacy_welcome_loads.json` |

### 1.14 Keyboard and mouse bindings of the old app (every row of `app-actions.md` §2.1 and §2.2)

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-175 | `V`, `1` Select | A:5245 | done | `tool.set` select | `ui/tests/keymap::every_registered_binding_dispatches_to_its_command_on_both_platforms` (covers A-175 to A-186) |
| A-176 | `H` Pan | A:5250 | done | `tool.set` hand | same |
| A-177 | `R`, `2` Rectangle | A:5255 | done | `tool.set` rectangle | same |
| A-178 | `O`, `4` Ellipse | A:5260 | done | `tool.set` ellipse | same |
| A-179 | `A`, `5` Arrow | A:5265 | done | `tool.set` arrow | same |
| A-180 | `L`, `6` Line | A:5270 | done | `tool.set` line | same |
| A-181 | `P`, `7` Freehand | A:5275 | done | `tool.set` freehand (also `X`) | same |
| A-182 | `T`, `8` Text | A:5280 | done | `tool.set` text | same |
| A-183 | `M`, `9` Math | A:5285 | done | `tool.set` math (`9` stays Math, decision 2) | same |
| A-184 | `E` Eraser | A:5290 | done | `tool.set` eraser (also `0`) | same |
| A-185 | `Z` Laser | A:5295 | done | `tool.set` laser (also `K`) | same |
| A-186 | Shift+letter used to pick tools (char match included uppercase) | A:5245 | done | Dropped on purpose: tool keys fire without Shift, `Shift+H` and `Shift+V` flip (§6.4) | `ui/tests/keymap::a_tool_key_picks_tool_set_with_the_tool` |
| A-187 | `Ctrl+A` select all | A:4777 | done | `selection.all` | `eng/cmd/selection_tests::select_all_is_bound_to_mod_a_on_the_canvas_and_is_in_the_palette` |
| A-188 | `Ctrl+S` save | A:4784 | done | `doc.save` | `eng/cmd/doc_tests::save_is_allowed_in_view_only_mode_and_has_the_key` |
| A-189 | `Ctrl+O` open (web: restored the autosave, native: picker) | A:4790 | done | always a picker (`ui.open_file`); autosave restore is `doc.open` stored | `ui/tests/keymap::verb_bindings_match_the_catalog` |
| A-190 | `Ctrl+E` export PNG | A:4870 | done | `doc.export` png | `eng/cmd/doc_tests::export_png_goes_through_the_renderer_with_the_scene_embedded` |
| A-191 | `Ctrl+Shift+E` and `Ctrl+Shift+C` copy PNG | A:4801, A:4992 | done | `clipboard.copy_as` (plus `Shift+Alt+C`) | `eng/cmd/clipboard_tests::copy_as_png_renders_the_selection_and_writes_the_image` |
| A-192 | `Ctrl+Z` undo, `Ctrl+Shift+Z` and `Ctrl+Y` redo | A:4944, A:4964 | done | `history.undo`, `history.redo` | `ui/tests/keymap::redo_has_two_chords` |
| A-193 | `Ctrl+G` group, `Ctrl+Shift+G` ungroup | A:4973 | done | `group.create`, `group.ungroup` | `eng/cmd/group_tests::create_has_a_shortcut_and_runs_in_view_only_mode_never` |
| A-194 | `Ctrl+C`, `Ctrl+X`, `Ctrl+V` | A:5061-5106 | done | `clipboard.*` | `ui/tests/context_menu::a_cursor_shortcut_pastes_at_the_pointer_with_the_real_command` |
| A-195 | `Ctrl+D` duplicate | A:5210 | done | `element.duplicate` | `eng/cmd/element_tests::duplicate_copies_offsets_and_selects_the_copies` |
| A-196 | `Delete`, `Backspace` | A:5299 | done | `element.delete` (in the point editor: `arrow.delete_point`) | `ui/tests/keymap::delete_is_arrow_delete_point_in_the_point_editor_and_element_delete_otherwise` |
| A-197 | `Esc` | A:5305 | done | `ui.close_overlays` then `tool.cancel` | `ui/tests/keymap::esc_closes_an_overlay_first_and_otherwise_cancels` |
| A-198 | Arrow keys nudge | A:5338 | done | `element.nudge` | `eng/cmd/element_tests::nudge_moves_by_the_setting_one_unit_when_fine_or_a_given_step` |
| A-199 | `Space` | A:4711 | done | see A-130 | see A-130 |
| A-200 | Text editor keys (`Ctrl+C/X/V/A`, word moves, `Esc`) | A:4548 | done | editor-local (†); `Mod+Enter` now ends the edit (it inserted a newline) | `ui/tests/keymap::during_a_text_edit_only_text_always_and_commit_gate_chords_apply`, `edit/src/text_edit/keys` |
| A-201 | Mouse: left press and drag, double-click (500 ms, 5 px) | input.rs:14 | done | recognizer defaults | `canvas/tests/recognizer::double_click_needs_time_distance_and_the_same_target` |
| A-202 | Mouse: right button (old: discarded) | A:4271 | done | `Intent::ContextMenu` opens the context menu; long press on touch | `canvas/tests/recognizer::secondary_button_gives_a_context_menu_and_middle_drag_pans`, `eng/int/interaction_tests::a_right_click_asks_for_a_context_menu` |
| A-203 | Modifiers: Shift toggle, additive marquee, keep aspect, 15 degree rotate; Alt duplicate | various | done | see A-060 to A-066 | see those rows |
| A-204 | Shortcut hint in tooltips and the `?` help (old: "?" label lied) | U:1998 | done | `ui.open_shortcuts` bound to `?` | `ui/tests/keymap::question_mark_opens_the_shortcuts_dialog_with_shift_implied` |

### 1.15 Claims in the old README

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-210 | Shapes and drawing: rectangles, ellipses, lines, arrows, freehand with pressure sensitivity | README | partial | All shapes done; pressure: web only, see A-014, A-015 | see those rows |
| A-211 | Smart guides, equal spacing, angle snapping | README | done | A-080 to A-085 | see those rows |
| A-212 | Text: GelPen, GelPen Serif, Vanilla Extract; per-character styling; inline LaTeX | README | done | Fonts; per-character colour in the on-canvas editor (A-053); inline LaTeX (A-056). Per-character styling means colour: the scene stores no other run style | see those rows |
| A-213 | Images: drag-drop, paste, embedded in the document | README | done | A-057 | see that row |
| A-214 | Real-time collaboration via Loro CRDT | README | done | A-155 to A-162 | see those rows |
| A-215 | Open formats: export PNG or JSON, import them back | README | done | A-100 to A-111, plus SVG and Excalidraw (section 2.10) | see those rows |
| A-216 | No telemetry | README | done | No analytics code; the web shell has none | none (absence of code; no test) |
| A-217 | Touch support: iPad and tablet friendly, gesture navigation | README | partial | Gestures done (A-122, A-127); no responsive layout (A-128); iPad Safari never tried | `canvas/tests/recognizer::two_fingers_pinch_and_pan_and_the_rest_is_ignored` |
| A-218 | Sketch style via roughr and fonts | README | done | A-034 to A-036 | see those rows |
| A-219 | Cross-platform (Linux, Windows, macOS, browser, mobile) | README | partial | Linux X11 native and Chromium web were run; Wayland, Windows, macOS, Safari, Firefox only compile or are untested; no mobile build | `docs/shell.md` §8, `docs/building.md` (p6-apps) |
| A-220 | Self-hostable with a single binary | README | done | `drafftink-server --web-dir` serves `/ws` and the web client (old server did not serve the client) | `apps/drafftink-server/tests/hostile.rs::static_files_are_served_from_the_web_dir_without_escaping_it` |

### 1.16 Numeric behaviour the inventory said to preserve (`inv §8` item 3)

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-225 | Grid 20, smart guide 8, equal-spacing radius 800, caps 50 and 200, angle step 15, rotation step 15 | inv §8 | done | `geom/src/snap.rs`, `geom/src/guides.rs` | `geom/tests/legacy/old_snap.rs` (the old module's tests, ported) |
| A-226 | Handle hit tolerance 24, rotate offset 25, handle size 16 | inv §8 | done | `geom/src/select.rs` | `geom/src/select.rs` unit test `assert_eq!(HANDLE_HIT_TOLERANCE, 24.0)` |
| A-227 | Double-click 500 ms and 5 px | inv §8 | done | `canvas/src/config.rs` | `canvas/tests/recognizer::double_click_needs_time_distance_and_the_same_target` |
| A-228 | Freehand simplify tolerance 2.0, highlighter 12 and alpha 128, image fit 800 | inv §8 | done | `edit/src/create.rs`, `cmd/image.rs` | `eng/cmd/element_create_tests::a_freehand_stroke_is_simplified_with_the_old_tolerance_and_keeps_its_pressures`, `eng/cmd/image_tests::a_big_picture_is_scaled_to_fit_800_and_centred_on_at` |
| A-229 | Export padding 20, autosave 5 s, peer display 8, nudge 20 | inv §8 | done | settings defaults | `eng/cmd/settings_tests::a_new_session_has_the_decided_defaults` |
| A-230 | Changed on purpose: undo depth 50 to 200, zoom max 10 to 30, eraser radius 10 world to 10 screen px, wheel and button zoom steps | commands.md §9 | done | decisions 3, 10; §7.5 item 10 | `eng/cmd/settings_tests::history_depth_is_validated_and_read_live` |

### 1.17 Old bugs the inventory said to fix, not port (`inv §8` item 4)

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| A-235 | Stuck drag when the release lands on a panel | A:2257 | done | gestures end cleanly on blur, pointer cancel or a release over a panel (`docs/exceptions.md` rule 3) | `eng/int/select_tests::tool_cancel_during_a_drag_leaves_no_stuck_state` |
| A-236 | Web drag-drop with a stale camera | A:1917 | done | `doc.drop` and `image.insert` take `at` from the live viewport (†) | `eng/cmd/clipboard_tests::the_default_place_is_the_viewport_centre_and_cursor_is_only_a_word` |
| A-237 | Web auto-join message dropped while the socket connects | A:1989 | done | the join is queued until the socket opens | `eng/cmd/collab_tests::connect_with_a_room_joins_it_and_returns_at_once` |
| A-238 | Seed collisions between peers; cursor throttle by event count with `static mut` | inv §8 | done | seeds come from the id source (`ids.seed()`); pointer and laser sends are throttled by the clock | `collab/src/awareness.rs` (`min_interval_ms`), `eng/int/draw_stroke_tests::laser_points_are_sent_at_most_every_33_ms_and_all_arrive` |
| A-239 | Tab switch leaves stale text-edit and drag state | A:1725 | done | `tab.switch` ends the edit and the drag | `eng/cmd/tab_tests::switch_commits_a_running_text_edit_first` |
| A-240 | Remote events not applied while idle; laser fade and caret blink not time-driven | A:2402 | done | `Waker` wakes the loop; fades by the clock (†) | `eng/int/draw_stroke_tests::the_trail_fades_by_the_clock_and_idle_means_idle` |
| A-241 | Renderer cost: texture per frame, no spatial index, math layout every frame | inv §5 | done | `docs/performance.md`, `cargo xtask perf` | `render/tests/grid.rs`, `render/benches/compose.rs` |

---

## 2. Excalidraw features (B)

Rows follow `research/parts/04-features-diff.md` §1.1 to §1.15 and §4. The AI features are `out of scope` by the user's decision. Features that `docs/commands.md` §8.3 lists as "not cataloged" (sticky note, embeds, bucket fill, autoshape, eyedropper, image crop, links, convert type, list markers, chart paste, follow mode UI, room encryption, pen mode, scrollbars) are `missing` here, because the requirement is to add everything except the AI features. Priorities are in section 4.

### 2.1 Tools

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-001 | Selection tool | X:Tools.tsx | done | A-001 | see A-001 |
| B-002 | Lasso selection | X:lasso/index.ts | done | `selection.select_lasso` (path in world units; overlap or contain, replace or add, groups, frames, locked, bound text as the marquee) over `geom::lasso` and `edit::group::selection::select_lasso`; the lasso tool (toolbar button, `tool.set` tool=lasso) and Alt-drag with the select tool draw the path (overlay `OverlayParts::lasso`) and run the command on release. Not adopted: live selection while the path grows (the marquee does not either); `contain` selects a group when any member is inside (Excalidraw: all members); a drag from an unselected element starts a lasso only with the lasso tool. | `geom/lasso::tests`, `edit/tests/selection_lasso`, `canvas/tests/hit::lasso_contain_versus_intersect_follows_the_marquee_rules`, `eng/cmd/selection_lasso_tests`, `eng/int/select_lasso_tests` (with `exception_E9_lasso_equals_plain_command`), `S:selection/lasso.json`, `render/tests/overlay::every_part_draws_something` |
| B-003 | Hand tool, Space-drag, wheel-button drag | X:App.pan.ts | done | `tool.set` hand, `tool.hold` (Space), middle drag | `eng/int/interaction_tests::space_hold_pans_instead_of_selecting_or_moving` |
| B-004 | Rectangle | X:Tools.tsx | done | A-003 | see A-003 |
| B-005 | Diamond (`D`, `3`) | X:Tools.tsx | done | `tool.set` diamond, `Kind::Diamond`, hit-test, binding geometry, import (old app turned diamonds into rectangles) | `eng/cmd/element_create_tests::diamond_ellipse_and_frame_are_boxes_of_their_kind`, `geom/src/binding.rs::orbit_on_ellipse_and_diamond` |
| B-006 | Ellipse | X:Tools.tsx | done | A-004 | see A-004 |
| B-007 | Arrow | X:Tools.tsx | done | bindings, heads, elbow, labels (see 2.4) | see 2.4 |
| B-008 | Line, closed polygon with fill | X:Tools.tsx | done | `Kind::Line` `closed` | `canvas/tests/hit::closed_lines_and_freehand_are_hit_inside_only_when_filled` |
| B-009 | Freedraw | X:Tools.tsx | done | A-007 | see A-007 |
| B-010 | Text tool (create on click, edit on click) | X:App.textTool.ts | done | A-010 | see A-010 |
| B-011 | Sticky note (`N`) | X:stickyNote.ts | done | `element.create_sticky` makes a rectangle with the Excalidraw preset (five paper colours, ink `#1e1e1e`, solid fill, corner radius a quarter of the short side up to 32) and a centred label whose font is fitted from 28 down to 16 before the note grows (`edit::sticky`). `N` places a note at the pointer and starts the text edit. Not done, by choice: no sticky element kind (an ordinary rectangle, so every command, format and Excalidraw export keeps working), no creation-date footer, no drop shadow (the renderer has none), no toolbar button, and the font is not re-fitted when the note is resized or its text edited later | `edit/sticky::tests`, `eng/cmd/element_sticky_tests::text_becomes_a_centred_label_with_the_largest_fitting_font`, `eng/cmd/element_sticky_tests::one_undo_step_takes_note_and_label_away_and_redo_brings_both_back`, `S:element/sticky_note_create_fit_undo.json` |
| B-012 | Image tool: file picker, paste, drop | X:image.ts | done | `I` opens the picker, `image.insert`, `doc.drop`, `clipboard.paste` (†). Formats: PNG, JPEG, WebP only (Excalidraw also GIF, SVG, BMP, ICO, AVIF); see B-103 | A-057, `eng/cmd/image_tests::a_damaged_picture_is_parse_failed` |
| B-013 | Eraser: trail, soft-delete preview, one undo step | X:eraser/index.ts | done | A-009 | see A-009 |
| B-014 | Frame tool (`F`) | X:Tools.tsx | done | `tool.set` frame, `element.create` kind frame | `eng/int/draw_tests::every_shape_tool_draws_one_element_with_one_command_and_one_undo_step` |
| B-015 | Magic frame (AI wireframe to HTML) | X:MagicButton.tsx | out of scope | AI feature | none |
| B-016 | Embeddable / iframe (YouTube, Figma, ...) | X:embeddable.ts | missing | Import warns and skips `embeddable` and `iframe` elements | `io/tests/excalidraw::images_come_with_their_picture_and_unsupported_embeds_are_reported` |
| B-017 | Laser pointer (`K`), shared with peers | X:laserTrails.ts | done | A-012, A-162; `Z` and `K` | see those rows |
| B-018 | Bucket fill (`B`) | X:bucketFill.ts | missing | none | none |
| B-019 | Autoshape (`Shift+X`) | X:convertToShape.ts | missing | none | none |
| B-020 | Eyedropper (`I`, `Shift+S`, `Shift+G`) | X:EyeDropper.tsx | missing | `I` is image insert here | none |
| B-021 | Tool lock (`Q`) | X:LockButton.tsx | done | `tool.lock`; toolbar lock button | `eng/int/draw_tests::with_the_lock_the_tool_stays_and_a_second_shape_can_be_drawn` |
| B-022 | Pen mode (stylus-only, palm rejection) | X:PenModeButton.tsx | missing | Pointer kind is known (`Pen`, `Touch`), but no mode ignores fingers | none |
| B-023 | Flowchart quick-create: `Mod+Arrow` creates a linked node, `Alt+Arrow` navigates | X:flowchart.ts | done | `diagram.extend`, `selection.navigate` | `eng/cmd/diagram_tests::extend_adds_a_connected_node_next_to_the_selection_and_selects_it`, `eng/cmd/selection_tests::navigate_selects_the_connected_element_in_the_direction` |
| B-024 | Hover "+" buttons around a selected node (element canvas buttons) | X:ElementCanvasButtons.tsx | missing | Keyboard only | none |
| B-025 | Highlighter, Math (ours, not in Excalidraw) | 04 §2 | done | A-008, A-011 | see those rows |

### 2.2 Element properties

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-030 | Stroke colour: palette, shades, hex input, custom colours, `S` opens the picker | X:ColorPicker | partial | Palette, shades, hex and `S` (verb) exist. No recent or custom colour list. | `ui/tests/color_popover.rs`, `ui/tests/keymap::verb_bindings_match_the_catalog` |
| B-031 | Background (fill) colour, `G` | X:actionChangeBackgroundColor | done | `style.set` `fill_color`; `G` verb | `ui/tests/surfaces::top_bar_swatches_and_widths_run_style_set` |
| B-032 | Fill styles hachure, cross-hatch, solid, zigzag | X:actionChangeFillStyle | done | A-034 (superset: also dots, dashed, zigzag line) | see A-034 |
| B-033 | Stroke width | X:actionChangeStrokeWidth | done | A-032 | see A-032 |
| B-034 | Stroke style on all shapes | X:actionChangeStrokeStyle | done | A-038 | see A-038 |
| B-035 | Sloppiness / roughness | X:actionChangeSloppiness | done | A-035 | see A-035 |
| B-036 | Edges sharp or round; proportional or adaptive radius | X:actionChangeRoundness | partial | `corner_radius` per shape (rectangle, diamond, frame, line, arrow); panel offers only Sharp and Round (32). No radius value control. | `eng/cmd/style_tests::get_corner_radius_ignores_elements_that_cannot_have_one` |
| B-037 | Opacity | X:actionChangeOpacity | done | A-033 | see A-033 |
| B-038 | Freedraw mode variable or constant | X:actionChangeFreedrawMode | done | `style.set` `pressure_mode`: `uniform` is constant, `simulated` and `device` are variable; see A-014 | see A-014 |
| B-039 | Arrow type sharp, round, elbow (existing arrow) | X:actionChangeArrowType | done | `arrow.set_type`; panel "Path" | `eng/cmd/arrow_tests::set_type_elbow_routes_around_the_bound_shapes_and_back_keeps_the_corners` |
| B-040 | Arrow type and heads chosen **before** drawing | X:Actions.tsx | done | `tool.set_arrow` sets the path type and the two heads that the arrow tool gives new arrows (tool defaults kept in the settings `arrow.type`, `arrow.start_head`, `arrow.end_head`); the arrow tool passes them to `element.create` and its preview shows the heads; the panel shows Path and Start and End head rows for the arrow tool when nothing is selected. Not adopted: Excalidraw's cycle of the type when the arrow key is pressed on the active arrow tool. | `eng/cmd/tool_arrow_tests` (all), `eng/int/draw_line_tests::the_arrow_tool_draws_with_the_type_and_heads_of_tool_set_arrow`, `scenarios tool::new_arrow_style`, `ui/surfaces::right_panel_of_the_arrow_tool_sets_the_type_and_heads_of_new_arrows`, `edit/arrow_defaults::tests` |
| B-041 | Arrowheads at both ends: arrow, bar, circle, triangle, diamond, outline variants, cardinality marks | X:arrowheads.ts | partial | `arrow.set_head` accepts all 13 kinds and the renderer draws them. The panel offers 6 per end (none, arrow, bar, dot, triangle, diamond); outline and cardinality heads only through the palette. | `eng/cmd/arrow_tests::set_head_sets_removes_and_undoes`, `sketch/tests/behavior::dashed_shaft_keeps_solid_heads` |
| B-042 | Swap arrowheads | new | done | `arrow.swap_heads`; panel button | `eng/cmd/arrow_tests::swap_heads_swaps_and_refuses_non_arrows` |
| B-043 | Font family picker (`Shift+F`) | X:FontPicker | done | font popover; 4 families | A-050 |
| B-044 | Font size, `Mod+Shift+<` and `>` | X:actionIncreaseFontSize | done | `text.step_size` | `eng/cmd/text_tests::step_size_walks_the_presets_and_undoes` |
| B-045 | Text align left, centre, right | X:actionChangeTextAlign | done | `style.set` `text_align`; panel "Align" | `typeset/tests/caret::alignment_moves_carets_with_the_text`, `ui/tests/surfaces.rs` |
| B-046 | Vertical align and line height | X:actionChangeVerticalAlign | partial | `style.set` `vertical_align`, `line_height` exist. No panel control. The renderer does not centre bound text vertically inside a taller box (`docs/render.md` §7). | `typeset/tests/bound.rs::vertical_alignment_top_middle_bottom` (layout only) |
| B-047 | Layers: forward, backward, front, back with `Mod+[` `Mod+]` | X:actionZindex | done | A-070 | see A-070 |
| B-048 | Align six ways, with keys | X:actionAlign | done | A-071 | see A-071 |
| B-049 | Distribute horizontally and vertically (`Alt+H`, `Alt+V`) | X:actionDistribute | done | `arrange.distribute` (needs 3 elements); panel buttons appear for 2 or more selected, context menu rows for 3 or more | `eng/cmd/arrange_tests::distribute_equalises_the_gaps_and_keeps_the_outer_elements` |
| B-050 | Flip with `Shift+H`, `Shift+V` | X:actionFlip | done | A-068 | see A-068 |
| B-051 | Duplicate, `Alt`-drag | X:actionDuplicateSelection | done | A-063, A-072 | see those rows |
| B-052 | Group, ungroup, enter group, deep select | X:actionGroup, groups.ts | done | `group.*`; `Mod`-click selects a member; double-click enters; `Esc` leaves | `eng/int/select_tests::ctrl_click_selects_a_member_of_a_group_not_the_group`, `S:group/group_click_enter_move_exit_ungroup.json` |
| B-053 | Group name | new (D4) | partial | `group.set_name` exists; no UI | `eng/cmd/group_tests::set_name_names_the_selected_group_and_undoes` |
| B-054 | Lock and unlock (`Mod+Shift+L`), unlock all | X:actionElementLock.ts | done | `element.lock` has a panel button, context menu row and key; locked elements are not hit, moved or erased. A right click on a locked element (the topmost thing under the pointer) clears the selection and opens the menu as `ContextTarget::Locked { element }` / `UiState::open_context_menu_on_locked`, whose Unlock row runs `element.unlock_all` with that id: the element (and its group) unlocks and is selected, as in Excalidraw. `element.unlock_all` with ids that are not locked refuses `nothing_to_do`. No lock badge or popup on a left click: the context menu row replaces Excalidraw's popup | `eng/int/interaction_tests::a_right_click_on_a_locked_element_asks_for_a_menu_about_it_and_selects_nothing`, `eng/int/interaction_tests::a_left_click_still_skips_a_locked_element`, `eng/cmd/element_tests::unlock_all_with_ids_that_are_not_locked_has_nothing_to_do`, `ui/tests/context_menu::the_menu_of_a_locked_element_offers_unlock_and_runs_it_on_that_element`, `S:element/unlock_from_the_canvas.json`, `S:element/lock_name_label.json` |
| B-055 | Hyperlink on an element (`Mod+K`), link icon, popup | X:actionLink.tsx | done | `element.set_link` (web link `http`, `https`, `mailto`, or element link `#id` / `@name`; other schemes refused with `invalid_link`), `element.open_link` (a web address goes back for the shell: `UiRequest::OpenUrl` from the pointer path, `HostRequest::OpenUrl` from the UI; an element link selects and shows the target), `Mod+K` dialog (`ui.open_link_editor`), context menu rows Link… and Open link, click on the link icon of an unselected element (the canvas picks it as `Target::LinkIcon`), `InteractionOverlay.canvas.link_icons` for the painter, drawn by `render::LinkMark` (†: the app maps them; there is no popup on a selected element, the dialog replaces it). `link` is a field of `element.query`; the link round-trips through native JSON and `.excalidraw` | `edit/src/link_tests`, `eng/cmd/element_link_tests`, `eng/int/interaction_tests::a_click_on_a_link_icon_asks_the_shell_to_open_the_address`, `eng/int/interaction_tests::the_icon_wins_over_the_element_under_it_and_a_drag_from_it_is_a_marquee_not_a_move`, `canvas/tests/canvas::the_link_icon_is_a_target_of_its_own_that_wins_over_what_lies_under_it`, `render/tests/overlay::a_link_icon_is_a_tile_with_a_glyph_and_keeps_to_its_rectangle`, `ui/tests/link_dialog`, `ui/tests/context_menu::an_element_with_a_link_has_an_open_link_row`, `io/tests/excalidraw::web_and_element_links_survive_an_export_and_an_import`, `S:element/links_set_open_roundtrip.json` |
| B-056 | Copy link to element, navigate to `#elementId` | X:actionElementLink.ts | partial | Navigation is done: an element link `#id` (or `@name`, stored as the id) set with `element.set_link` is followed by `element.open_link` or a click on its icon, which selects the target and zooms out to fit it (never in). Copying a link to the clipboard is not in the catalog: a link `#id` only means something inside its document, so a shareable URL needs a document address scheme first | `eng/cmd/element_link_tests::open_link_to_an_element_selects_it_and_brings_it_into_view`, `eng/int/interaction_tests::a_click_on_the_icon_of_an_element_link_goes_to_the_target`, `S:element/links_set_open_roundtrip.json` |
| B-057 | Crop image | X:actionCropEditor.tsx | partial | `ImageData.crop` is stored, imported and exported; **no command and no UI crop**; render of a crop: none found | `io/tests/excalidraw::image_scale_and_crop_and_bad_picture_data` (import only) |
| B-058 | Bound text in containers; `Enter` or double-click edits the label | X:textElement.ts | done | `text.bind`, `text.unbind`, `text.wrap_in_container`, `element.relabel`; pointer: click on a shape edits its label | `eng/cmd/text_tests::bind_makes_a_label_unbind_frees_it`, `eng/int/draw_stroke_tests::a_click_on_a_shape_edits_its_label`, `S:text/labels_bind_unbind_wrap.json` |
| B-059 | Bind, unbind, wrap text in a container from the UI | X:actionBoundText.tsx | partial | The three commands have no panel button or menu row (palette only) | `eng/cmd/text_tests::wrap_in_container_boxes_each_free_text_and_selects_the_shapes` |
| B-060 | Arrow labels, label position along the arc | X:arrowEndpointText.ts | done | `element.relabel` on an arrow; double-click on an arrow edits its label; `label_position` | `eng/int/select_tests::double_click_on_an_arrow_starts_a_label_edit`, `typeset/tests/bound.rs::arrow_labels_wrap_at_seventy_percent_or_eleven_em` |
| B-061 | Text auto-resize toggle and handle | X:actionTextAutoResize.ts | partial | `text.set_autoresize`; no control and no drag handle | `eng/cmd/text_tests::set_autoresize_toggles_between_following_the_content_and_wrapping` |
| B-062 | Wrap toggle `W` (wrap selection in a frame) | X:actionToggleWrap.ts | done | `frame.wrap` on `W` | `eng/cmd/frame_tests::wrap_has_the_w_key` |
| B-063 | List markers in text | X:listMarker.ts | missing | none | none |
| B-064 | Convert element type (`Tab`, `Shift+Tab` popup) | X:ConvertElementTypePopup.tsx | done | `element.convert` (`edit::convert`) turns rectangle, diamond and ellipse into one another, and line and sharp, round and elbow arrows into one another, in place: id, style, groups, frame and z-order stay, the bound label is laid out again (the shape grows when the new inner box is smaller), attached arrows are put back on the new outline. Without `to` it takes the next type of the family; `Tab` and `Shift+Tab` run it. Refused in plain words: text, math, image, frame and freehand, mixed shape and line selections, the other family as target, an attached or labelled arrow as a line, a closed line as an arrow, locked elements. Not done: the popup with a button per type (the command takes the type as an argument, and the palette lists it) | `edit/convert::tests`, `eng/cmd/element_convert_tests::the_label_stays_bound_and_the_shape_grows_when_the_new_inner_box_is_smaller`, `eng/cmd/element_convert_tests::arrows_bound_to_the_shape_are_put_back_on_the_new_outline`, `eng/cmd/element_convert_tests::tab_walks_line_sharp_round_elbow_and_wraps`, `S:element/convert_type_keeps_text_and_arrows.json` |
| B-065 | Rotation of all shapes with a handle, 15 degree snap | X:transformHandles.ts | done | `element.rotate`; lines and arrows are edited by points, as in Excalidraw; frames and elbow arrows refuse | `eng/cmd/element_tests::rotate_refuses_frames_and_elbow_arrows_with_the_catalog_text` |
| B-066 | Resize handles: edges, from centre (`Alt`), keep ratio (`Shift`) | X:resizeElements.ts | done | A-064, A-075; `from_center`, `keep_aspect` | `eng/cmd/element_tests::resize_edge_handles_stretch_one_axis_and_default_handle_is_se` |
| B-067 | Custom data per element | X:customData | done | `Element.custom` map, kept through import and export | `io/tests/excalidraw::native_documents_survive_a_trip_through_excalidraw` |

### 2.3 Selection and editing behaviours

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-070 | Click and Shift-click multi-select | X:selection.ts | done | A-060 | see A-060 |
| B-071 | Box select with contain or overlap mode | X:BoxSelectionMode | done | setting `input.box_select` (no control, palette only) | `eng/cmd/selection_tests::the_default_of_contain_is_the_setting_input_box_select` |
| B-072 | Deep select (`Mod`-click) and deep box select (`Mod`-drag) | X:HelpDialog | partial | `Mod`-click done; `select_region` has no `deep` argument, so `Mod`-drag selects whole groups | `eng/int/select_tests::ctrl_click_selects_a_member_of_a_group_not_the_group` |
| B-073 | Linear element point editor: `Mod+Enter`, add, move, delete points, midpoint insert | X:linearElementEditor.ts | done | `arrow.edit_points`, `arrow.add_point`, `arrow.delete_point`, `arrow.move_point` | `eng/cmd/arrow_tests::edit_points_toggles_the_editor_and_selects_a_point`, `eng/cmd/arrow_tests::delete_point_by_index_and_from_the_point_editor` |
| B-074 | Curved arrow or line by clicking several points | X:HelpDialog | done | click, click, double-click or `Enter` ends the draft | `eng/int/draw_line_tests::clicks_make_a_draft_that_a_double_click_ends` |
| B-075 | `Shift` constrains (square, circle, 45 degrees), `Alt` draws from the centre, `Mod` turns off snapping and binding | X:keys.ts | done | shapes: square and centre; lines: 15 degree lock; `Mod` off for grid and binding | `eng/int/draw_tests::shift_and_alt_draw_a_square_from_the_centre`, `eng/int/select_tests::grid_snap_follows_the_settings_and_ctrl_turns_it_off` |
| B-076 | Nudge with arrows, `Shift` for the small step | App.tsx | done | A-067 (the step is 20 for plain, 1 for `Shift`: the reverse of Excalidraw, by decision) | see A-067 |
| B-077 | Copy styles, paste styles | X:actionStyles.ts | done | `clipboard.copy_style`, `clipboard.paste_style` (`Mod+Alt+C`, `Mod+Alt+V`); context menu rows | `eng/cmd/clipboard_tests::copy_style_takes_the_first_selected_in_z_order_unless_told` |
| B-078 | Bottom hint bar and cursor hints | X:HintViewer.tsx | missing | Mouse cursors only | none |
| B-079 | Double-click on empty canvas creates text | X:App.text.ts | done | text draft at the point | `eng/int/select_tests::double_click_on_empty_canvas_starts_a_draft_at_the_point` |
| B-080 | Delta undo and redo, per user, collab-safe, selection-aware | X:history.ts | done | A-135 to A-137 | see those rows |
| B-081 | Stable z-order under concurrent edits (fractional index) | X:fractionalIndex.ts | done | `ZIndex` keys in scene | `scene/src/zindex.rs::excalidraw_style_indices_parse`, `eng/cmd/collab_tests::concurrent_edits_made_with_commands_converge` |

### 2.4 Arrows, bindings, elbow

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-085 | Arrow binding to shapes: inside and orbit modes, gap, focus point, highlight while dragging | X:binding.ts | done | `arrow.bind`, `arrow.unbind`, `arrow.move_point` (`bind_highlight` in the frame output), `element.create` bind, setting `arrow.binding` | `eng/int/select_tests::dragging_an_arrow_end_onto_a_rectangle_binds_it_and_shows_the_candidate`, `eng/int/draw_line_tests::an_arrow_from_one_rectangle_to_another_binds_both_ends_and_the_highlight_shows_while_placing`, `S:arrow/bind_unbind_rebind.json` |
| B-086 | Bound arrows follow moving, resizing, rotating and deleted shapes | X:updateBoundElements | done | `edit::binding`; delete frees the end in place; undo restores | `S:arrow/arrow_stays_bound_move_resize_rotate_delete.json`, `S:arrow/arrow_unbinds_when_its_target_is_deleted.json`, `eng/int/select_tests::moving_a_rectangle_keeps_the_arrow_attached` |
| B-087 | Unbind by dragging away; `Mod` prevents binding | X:actionToggleArrowBinding | done | `bind=off`, `Mod` held; setting `arrow.binding` has no control (palette only) | `eng/int/select_tests::ctrl_keeps_the_arrow_end_from_binding`, `eng/cmd/arrow_tests::bind_off_ignores_targets_and_dragging_away_unbinds` |
| B-088 | Midpoint snapping preference | X:actionToggleMidpointSnapping | partial | setting `arrow.midpoint_snap`, used by the binding code; no control (palette only) | `edit/tests/arrows_acquire::an_orbit_focus_snaps_to_a_side_midpoint` |
| B-089 | Elbow arrows: routed around bound shapes, fixed segments, single-corner preference | X:elbowArrow.ts | done | `arrow.set_type` elbow, `arrow.move_segment`, `arrow.release_segment`, `arrow.reroute`; A* router (old app: render-only fixed Z route) | `edit/tests/arrows_elbow.rs`, `eng/cmd/arrow_tests::scenario_an_elbow_arrow_stays_routed_and_attached`, `S:arrow/point_editing_and_elbow_segments.json` |
| B-090 | Elbow preview while drawing | X:elbowArrow.ts | partial | The preview is a straight line until release (`docs/backlog.md`) | `eng/int/draw_line_tests::the_elbow_arrow_type_sends_the_two_ends_and_the_engine_routes_it` |
| B-091 | Arrowheads fit short segments | X:arrowheads.ts | partial | All head kinds are drawn at both ends; I found no code or test that shrinks a head on a short segment | `sketch/tests/behavior::every_head_kind_draws_at_both_ends` |
| B-092 | Keep arrows attached through group, copy and paste | X:binding | done | groups, paste of a bound pair keep bindings | `eng/cmd/group_tests::old_weakness_arrows_bound_to_a_group_follow_it`, `S:clipboard/paste_bound_pair_keeps_binding.json` |

### 2.5 Text, fonts, languages

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-095 | Excalidraw font set (Excalifont, Virgil, Nunito, Lilita One, Comic Shanns, Liberation Sans, Assistant, Cascadia, Helvetica, Xiaolai, emoji) | X:fonts | partial | 4 families: GelPen, GelPen Serif, Vanilla Extract, Noto Sans. No CJK hand-drawn or emoji fallback. Importing a file maps unknown families to the nearest. | `io/tests/excalidraw::text_containers_labels_and_fonts` |
| B-096 | Font subsetting for SVG embedding | X:subset | partial | `io::svg` can embed subset fonts when given them; `doc.export` does not supply any yet (`docs/backlog.md`: "SVG export embeds no fonts yet") | `io/tests/formats::svg_fonts_are_embedded_only_when_used_and_supplied` |
| B-097 | Wrapping at a fixed width, auto-resize | X:textWrapping.ts | partial | `element.create` and `text.edit_begin` with `width` wrap the text and turn auto-resize off; a text-tool drag opens a draft of the dragged width; labels wrap in their container. The editor of the shell must show the box at `OpenTextEditor.width`. | `eng/cmd/element_create_tests::a_fixed_width_wraps_the_text_and_turns_auto_resize_off`, `eng/cmd/text_tests::a_draft_with_a_width_becomes_a_fixed_width_wrapping_text`, `eng/interact/draw_stroke_tests::a_text_tool_drag_on_empty_canvas_makes_a_fixed_width_box` |
| B-098 | Inline LaTeX in text | README | done | see A-056 | see A-056 |
| B-099 | Right-to-left and bidirectional text | X:locales | partial | Parley shapes bidi text; no test or fixture checks Arabic or Hebrew | none |
| B-100 | Localisation (58 locales) | X:i18n.ts | missing | All strings are English literals | none |

### 2.6 Canvas, view, preferences

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-105 | Zoom in, out, reset; range 0.1 to 30 | X:actionCanvas.tsx | done | A-124, A-126 | see those rows |
| B-106 | Zoom to fit all and to selection (`Shift+1`, `Shift+2`, `Shift+3`) | X:actionCanvas.tsx | done | `view.zoom_to_fit` | `eng/cmd/view_tests::fit_all_centres_the_content_and_uses_the_padding` |
| B-107 | `PgUp`, `PgDn` scrolling | X:HelpDialog | done | `view.scroll_page` | `eng/cmd/view_tests::scroll_page_moves_by_a_fraction_of_the_viewport` |
| B-108 | Scrollbars | X:scrollbars.ts | missing | none | none |
| B-109 | Input device preference (mouse or trackpad) | X:App.wheel.ts | missing | Wheel pans and `Mod`+wheel zooms; no setting | none |
| B-110 | Grid mode `Mod+'`, grid size 20 | X:actionToggleGridMode | done | `settings.cycle view.grid_style` | `eng/cmd/settings_tests::alt_s_toggles_smart_guides_and_mod_quote_cycles_the_grid` |
| B-111 | Objects snap mode `Alt+S` with gap and equal-spacing guides | X:snapping.ts | done | A-081, A-082 | see those rows |
| B-112 | Zen mode `Alt+Z` | X:actionToggleZenMode | done | UI verb `ui.toggle_zen` | `ui/tests/surfaces::zen_mode_hides_the_chrome` |
| B-113 | View mode `Alt+R` (read-only) | X:actionToggleViewMode | done | `view.set_view_only`; menu row; select, hand and laser still work | `S:view/view_only_mode.json`, `eng/cmd/tool_tests::in_view_only_mode_only_select_hand_and_laser_are_available` |
| B-114 | Stats panel `Alt+/` with x, y, w, h, angle, font size, scene counts | X:Stats | partial | Panel shows counts and the selection's x, y, w, h as editable fields (`element.set_bounds`). No angle and no font size fields; no multi-element min or max fields. | `ui/tests/small_dialogs::editing_a_bounds_field_runs_set_bounds_with_only_that_value` |
| B-115 | Theme light, dark, system; per-theme canvas colours; dark export | X:actionToggleTheme | partial | Setting `theme` (menu row), dark UI palette, dark canvas mapping in the renderer, dark PNG and SVG export (`export.dark`) and `copy_as` dark. **No keyboard toggle** (`Shift+Alt+D`: "not planned" in commands.md §8.1). | `render/tests/gpu::the_dark_theme_draws_light_ink_on_a_dark_background`, `eng/cmd/clipboard_tests::copy_as_dark_draws_with_the_dark_palette_without_a_warning`, `ui/tests/frame.rs` (theme follows the setting) |
| B-116 | Canvas background colour | X:actionChangeViewBackgroundColor | done | A-040 | see A-040 |
| B-117 | Welcome screen with hints | X:welcome-screen | done | the intro document (`doc.show_intro`, shown on first run †); no hint arrows | `eng/cmd/doc_tests::show_intro_replaces_the_document_with_the_welcome_document` |
| B-118 | Preferences menu | X:DefaultItems.tsx | partial | see A-168 | see A-168 |
| B-119 | Help dialog | X:HelpDialog.tsx | done | A-148 | see A-148 |
| B-120 | Command palette `Mod+/`, `Mod+Shift+P` | X:CommandPalette | done | lists every palette command with its key and refusal text; argument forms from the JSON Schema | `ui/tests/palette.rs::rows_cover_every_palette_command_and_every_titled_binding` |
| B-121 | Search in scene `Mod+F` over text, frame names | X:SearchMenu.tsx | done | `ui.open_search` over `element.query`; result list selects and centres | `ui/tests/small_dialogs::choosing_a_result_selects_the_element_centres_the_view_on_it_and_closes`, `eng/cmd/element_query_tests::query_filters_by_kind_text_name_and_style` |
| B-122 | Phone and tablet layout, mobile toolbar | X:MobileToolbar.tsx | missing | see A-128 | none |
| B-123 | Touch: pinch, two-finger pan, long-press context menu, double-tap | X:gesture.ts | partial | pinch, pan and long press are done; double-tap on touch is not tested (the recognizer's double-click rule is written for the mouse) | `canvas/tests/recognizer::touch_long_press_by_tick_and_by_event_time` |
| B-124 | Hardware pen pressure | X:freedraw | partial | see A-015 | see A-015 |
| B-125 | Scroll to content / viewport helpers | X:viewport.ts | done | `view.zoom_to_fit`, `view.center`, `view.set` | `eng/cmd/view_tests::center_moves_the_camera_and_keeps_the_zoom` |

### 2.7 Frames

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-130 | Frame element: name label, clip children, border | X:frame.ts | done | `Kind::Frame`; settings `frame.clip`, `frame.show_names`, `frame.outline` | `render/tests/painter::frames_draw_a_border_and_a_clipped_name`, `render/tests/compose::frame_children_are_clipped_or_culled` |
| B-131 | Add by drawing inside or dragging in; highlight while dragging | X:frame.ts | done | `frame.add`, `element.move` joins and leaves frames on the committed run | `eng/int/select_tests::dropping_over_a_frame_highlights_it_during_the_drag_and_joins_it_on_release`, `eng/cmd/element_create_tests::a_new_element_joins_the_frame_under_its_centre_and_sits_below_it` |
| B-132 | Wrap selection in a frame, unwrap | X:actionFrame.ts | partial | `frame.wrap` has `W`; `frame.unwrap` has no control (palette only) | `eng/cmd/frame_tests::unwrap_releases_the_children_deletes_the_frame_and_selects_them` |
| B-133 | Rename a frame, select all in frame, remove from frame | X:actionFrame.ts | partial | `frame.rename`, `frame.select_children`, `frame.remove` exist; palette only, no context menu rows, no double-click on the label | `eng/cmd/frame_tests::rename_sets_and_restores_the_default_label`, `eng/cmd/frame_tests::select_children_selects_them_without_bound_text_and_makes_no_undo_step` |
| B-134 | Export one frame | X:actionFrame.ts | done | `doc.export` scope `frame` | `eng/cmd/doc_tests::export_frame_takes_the_frame_with_its_children` |

### 2.8 Images and files

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-140 | Image formats PNG, JPEG, GIF, WebP, BMP, ICO, AVIF, JFIF, SVG | X:IMAGE_MIME_TYPES | partial | PNG, JPEG, WebP (`io::picture`); GIF and SVG images are skipped on import with a warning | `io/tests/excalidraw::images_come_with_their_picture_and_unsupported_embeds_are_reported` |
| B-141 | Downscale big images on insert | X:blob.ts | done | pictures above 2 MiB are re-encoded to at most 2048 px (setting-free; `downscale` argument) | `eng/cmd/image_tests::a_big_picture_is_stored_smaller_unless_downscale_is_off` |
| B-142 | Content-addressed file table | X:BinaryFiles | done | one copy per SHA-256 | `eng/cmd/image_tests::several_pictures_cascade_by_offset_and_the_same_bytes_are_stored_once` |

### 2.9 Clipboard

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-145 | Copy, cut, paste elements through the system clipboard, with the Excalidraw flavour | X:clipboard.ts | done | A-090; setting `clipboard.excalidraw_compat` | `eng/cmd/clipboard_tests::copy_keeps_the_buffer_and_writes_the_payload_with_the_excalidraw_flavour` |
| B-146 | Paste image | X:clipboard.ts | done | A-057 | see A-057 |
| B-147 | Paste plain text as a text element; `Mod+Shift+V` as plain text | X:clipboard.ts | done | `clipboard.paste` `plain_text` | `eng/cmd/clipboard_tests::plain_text_skips_the_sniffing` |
| B-148 | Paste SVG as an image | X:clipboard.ts | missing | paste routing has no SVG branch | none |
| B-149 | Paste spreadsheet data as a chart | X:charts | missing | none | none |
| B-150 | Paste Mermaid text | X:TTDDialog | done | A-109 | see A-109 |
| B-151 | Paste URL as embed or link | X:clipboard.ts | missing | none | none |
| B-152 | Copy as PNG (`Shift+Alt+C`) | X:actionCopyAsPng | done | A-092 | see A-092 |
| B-153 | Copy as SVG | X:actionCopyAsSvg | partial | `clipboard.copy_as` svg works; no menu row or key, palette only | `eng/cmd/clipboard_tests::copy_as_svg_and_text` |
| B-154 | Copy element text | App.tsx | partial | `clipboard.copy_as` `text` format; palette only | `eng/cmd/clipboard_tests::copy_as_svg_and_text` |
| B-155 | Drag and drop of scenes, libraries and images | X:blob.ts | done | `doc.drop` | `eng/cmd/doc_tests::drop_opens_documents_in_new_tabs_and_imports_libraries` |

### 2.10 Export, import, persistence

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-160 | Export dialog: PNG, SVG, copy PNG, background on or off, dark, scale, embed scene, selection only, padding | X:ImageExportDialog.tsx | partial | `doc.export` has every option as an argument (`scale`, `padding`, `background`, `dark`, `embed_scene`, `scope`). The UI has two menu rows (PNG, copy PNG) plus a scale row; **no export dialog**, so the other options need settings or the palette. | `eng/cmd/doc_tests::export_defaults_come_from_the_settings` |
| B-161 | PNG with the scene embedded; Excalidraw's own metadata | X:image.ts | partial | Our scene is embedded (and the old app's `zTXt` is read). Excalidraw's `application/vnd.excalidraw+json` PNG chunk is not read or written. | `io/tests/formats::png_scene_round_trip_with_unicode` |
| B-162 | SVG export with the scene embedded | X:export.ts | partial | `doc.export` svg embeds the scene and re-imports, embeds the font files of the texts it draws and honours `dark` | `eng/cmd/doc_tests::export_svg_inline_embeds_the_scene_and_changes_nothing`, `eng/cmd/doc_tests::export_svg_embeds_the_font_files_of_the_texts_it_draws_and_no_others`, `eng/cmd/doc_tests::export_svg_dark_uses_the_canvas_palette_and_no_longer_warns`, `io/tests/formats::svg_embedded_scene_round_trips_the_whole_document` |
| B-163 | `.excalidraw` save and load | X:json.ts | done | `doc.save`/`doc.export` format excalidraw; `doc.open` | `eng/cmd/doc_tests::save_as_excalidraw_writes_a_scene_and_lists_what_it_dropped`, `io/tests/excalidraw::every_fixture_survives_import_export_import` |
| B-164 | `.excalidrawlib` import and export | X:library.ts | partial | import: A-108. Export: `library.export`, palette only. | `eng/cmd/library_tests::export_inline_round_trips_through_import` |
| B-165 | Save to the active file (File System Access) | X:filesystem.ts | done | `doc.save` remembers its target; native dialog; web download or storage (†) | `eng/cmd/doc_tests::save_writes_the_document_marks_it_saved_and_remembers_the_target` |
| B-166 | Save as (`Mod+Shift+S`), open (`Mod+O`) | X:actionExport | partial | keys exist; naming problem of A-102 | see A-102 |
| B-167 | Clear canvas (`Mod+Delete`) | X:actionClearCanvas | done | A-104 | see A-104 |
| B-168 | Autosave and multi-tab sync | X:LocalData.ts | partial | Autosave of the whole workspace done (A-165). No sync between two browser tabs of the same origin. | `eng/cmd/tab_tests::all_tabs_come_back_after_a_restart_with_their_state_but_no_history` |
| B-169 | Project / scene name | X:ProjectName.tsx | partial | `doc.rename` exists; no control (see A-102) | `eng/cmd/doc_tests::rename_sets_the_name_in_one_undo_step` |
| B-170 | Shareable link with a key, Excalidraw+ export | X:ShareDialog.tsx | out of scope | Excalidraw+ is a hosted product. Room links exist (`#room=...&server=...`, no key). | none |
| B-171 | Schema versions and migration on load | X:restore.ts | done | `meta.schema`; newer files are read-only or refused | `eng/cmd/doc_tests::open_refuses_a_file_from_a_newer_version` |
| B-172 | `.excalidraw` field mapping: strokeStyle, arrowheads, bindings, fontFamily, textAlign, angle, frameId, images, frames | 04 §1.10 | done | all mapped (old importer dropped them); embeddable, iframe, selection skipped with a warning | `io/tests/excalidraw::basic_shapes_map_every_style_field`, `io/tests/excalidraw::frames_children_and_z_order` |

### 2.11 Diagrams from text

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-175 | Mermaid flowchart and sequence | X:mermaid-to-excalidraw | done | A-109 | see A-109 |
| B-176 | Mermaid class, state, ER, gantt, quadrant, timeline, pie; live-preview dialog with auto-fix | X:TTDDialog | missing | The parser refuses other diagram types with a hint; no dialog | `io/tests/mermaid::rejected_inputs_say_why` |
| B-177 | Text-to-diagram (AI chat) | X:TextToDiagram.tsx | out of scope | AI feature | none |
| B-178 | Diagram to code (AI) | X:DiagramToCodePlugin | out of scope | AI feature | none |
| B-179 | Build a diagram from structure (for agents) | plan §11 | done | `diagram.create`, `layout.apply` | `S:diagram/agent_builds_flowchart.json`, `S:layout/apply_layered_grid_and_refusals.json` |

### 2.12 Library

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-180 | Library store: add selection, insert, rename, remove, list, persist | X:library.ts | partial | `library.add`, `.insert`, `.rename`, `.remove`, `.list` and persistence exist and are tested; **no sidebar or panel**, so a user reaches them through the palette only | `eng/cmd/library_tests::add_stores_the_selection_as_an_item_at_the_origin`, `eng/cmd/library_tests::the_library_survives_a_new_session_over_the_same_storage` |
| B-181 | Library thumbnails, drag to insert, publish, browse, `#addLibrary=` URL | X:LibraryMenu | missing | none | none |
| B-182 | Library import merge or replace, skip duplicates | X:library.ts | done | `library.import` | `eng/cmd/library_tests::importing_the_same_file_again_skips_every_item` |

### 2.13 Menus, dialogs, panels

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-185 | Main menu (open, save, export, collaborate, command palette, search, help, reset, preferences, theme) | X:MainMenu.tsx | partial | Menu has new tab, open, recent, save, save as, export PNG, copy PNG, scale, insert image, library, Mermaid, clear, intro, view-only, theme, palette, shortcuts. **No Search row, no SVG or JSON export row, no preferences, no language.** | `ui/tests/surfaces::menu_opens_by_verb_and_its_rows_run_with_the_menu_origin` |
| B-186 | Canvas context menu: paste, select all, grid, snap, zen, view, stats, unlock all | X:getContextMenuItems | partial | Paste, select all, zoom to fit, copy and paste style. Missing: grid, snap, zen, view mode, stats toggles, unlock all. | `ui/tests/context_menu::opening_shows_the_rows` |
| B-187 | Element context menu: cut, copy, paste, duplicate, delete, group, ungroup, z-order, align, distribute, lock, copy style | X:getContextMenuItems | partial | Those rows exist. Missing: flip, copy as PNG or SVG, copy text, wrap in frame, remove from frame, select all in frame, bind or unbind text, wrap text in container, edit line points, add to library, link. | `ui/tests/context_menu.rs` (18 tests) |
| B-188 | Confirm dialogs, toasts | X:Dialog, Toast | done | confirm dialog, toasts with suggestion buttons | `ui/tests/routing.rs`, `ui/tests/small_dialogs::confirm_shows_the_refusal_text_unchanged` |
| B-189 | Collaborator list with avatars | X:UserList.tsx | partial | presence panel (dot and name, 8 shown); no avatars, no click action | `ui/tests/collab_dialog::presence_lists_the_peers_with_anonymous_for_empty_names` |
| B-190 | Footer: zoom, undo, redo, help | X:footer | done | bottom bar, `?` | A-144 |
| B-191 | Dockable sidebar (library, custom tabs) | X:Sidebar | missing | none | none |

### 2.14 Collaboration

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-195 | Rooms and links `#room=` | X:Collab.tsx | done | A-156, A-157; `collab.status` returns the link | `eng/cmd/collab_tests::room_links_follow_the_old_apps_forms` |
| B-196 | End-to-end encryption, room passwords | X:encryption.ts | missing | Frames are not encrypted by the protocol (`docs/protocol.md`); a bearer token protects the server, no per-room key | `collab/tests/server.rs::auth_off_ignores_the_token_and_auth_on_checks_it` (token only) |
| B-197 | Element-level merge | X:reconcile.ts | done | A-160 | see A-160 |
| B-198 | Cursor with user name label and colour | X:Collaborator | done | see A-159 (label in the peer's colour, `RemoteCursor::named`) | `render/tests/overlay::a_named_cursor_draws_a_label_with_glyphs_and_an_unnamed_one_does_not`, `render/tests/gpu::peers_are_drawn_in_their_own_colours` |
| B-199 | Remote selection outlines | X:Collaborator | done | Selection ids travel in awareness; `PeerView::to_remote_user` and `canvas::remote_parts` turn them into one box per selected element; `render::RemoteSelection` draws a solid outline in the peer's colour (†: the app maps it, `docs/render.md` §5). canvas-js: `stage.setRemoteUsers` draws them too | `render/tests/overlay::a_remote_selection_is_one_outline_per_box_and_a_turned_box_stays_turned`, `eng/cmd/collab_tests::a_peer_becomes_a_remote_user_with_pointer_name_selection_and_trail`, `canvas-js/tests/interaction::remote_users_get_a_pointer_a_selection_outline_and_a_trail_in_the_frame` |
| B-200 | Idle states active, away, idle | X:IDLE_THRESHOLD | missing | silent peers lose their cursor but nothing shows an away state | `collab/src/awareness.rs::silent_peers_lose_their_cursor_but_stay_listed` |
| B-201 | Follow mode (click an avatar, follow the viewport) | X:actionGoToCollaborator | partial | `collab.follow` works and stops when the user moves the camera; **no avatar or button** to start it (palette only) | `eng/cmd/collab_tests::following_moves_the_camera_until_the_user_moves_it` |
| B-202 | Remote laser pointers | X:laserTrails.ts | done | A-162 | see A-162 |
| B-203 | Remote pointer tool badge | X:Collaborator | missing | awareness has no tool field | none |
| B-204 | Reconnect, offline queue, room persistence | X:Collab.tsx | done | client backoff, offline edits kept, server stores rooms (filesystem, memory, sqlite, s3) | `eng/cmd/collab_tests::edits_made_while_not_connected_are_not_lost`, `apps/drafftink-server/tests/persistence.rs` |
| B-205 | Images in a room | X:FileManager | done | the file table is part of the document | `S:image/image_insert_fit_and_refusals.json`, `crates/doc` file table tests |
| B-206 | Self-hosted server | drafft-only | done | `apps/drafftink-server` | `apps/drafftink-server/tests/` |

### 2.15 Platform and misc

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-210 | PWA: manifest, service worker, installable | X:vite PWA | done | static assets in `apps/drafftink-web` on `p6-apps-web` (†) | none (not on main; `scripts/` in that branch) |
| B-211 | Sentry, analytics | X:sentry.ts | out of scope | telemetry, against the product's rules | none |
| B-212 | Embeddable React API | X:index.tsx | out of scope | Replaced by the `drafftink-canvas` npm package (`crates/canvas-js`, `packages/drafftink-canvas`) | `canvas-js/tests/shapes.rs`, `canvas-js/tests/interaction.rs` |
| B-213 | Sign-in, social links, Excalidraw+ | X:AppMainMenu | out of scope | hosted service | none |
| B-214 | Agent surfaces (new): MCP server, CLI, bridge, JS API | plan §11 | partial | `drafftink-mcp` and `crates/bridge` are on main with tests; `apps/drafftink-cli/src/main.rs` is `fn main() {}`; `window.drafftink` is on `p6-apps-web` | `apps/drafftink-mcp/tests/protocol.rs`, `crates/bridge/tests/rpc.rs` |

### 2.16 Every Excalidraw shortcut row (`04 §4.1` to `§4.3`)

Status here is about the key reaching a working action. Where the default key differs from Excalidraw, the Where column says which key does it.

| # | Feature | Source | Status | Where | Evidence |
|---|---|---|---|---|---|
| B-220 | `H` hand | 4.1 | done | `tool.set` hand | A-176 |
| B-221 | `V`, `1` selection (click again for lasso) | 4.1 | done | selection yes. Lasso is its own toolbar button after select and `tool.set` tool=lasso (palette), plus Alt-drag with the select tool; no key (Excalidraw's lasso has none, and `S` is the stroke colour picker); a click on the active select button does not switch to lasso | B-002; `ui/surfaces::every_tool_button_runs_tool_set_with_its_tool` |
| B-222 | `R`/`2`, `D`/`3`, `O`/`4`, `A`/`5`, `L`/`6`, `P`/`7`/`X`, `T`/`8` | 4.1 | done | `tool.set` | `ui/tests/keymap::every_registered_binding_dispatches_to_its_command_on_both_platforms` |
| B-223 | `N` sticky note | 4.1 | done | `N` runs `element.create_sticky` at the pointer with `edit=true` (registry binding) | B-011, `ui/tests/keymap::every_registered_binding_dispatches_to_its_command_on_both_platforms` |
| B-224 | `9` image | 4.1 | done | `I` (UI verb), `9` stays Math (decision 2) | `ui/tests/keymap::ui_verb_keys_do_not_collide_with_command_keys` |
| B-225 | `E`/`0` eraser, `F` frame, `K` laser | 4.1 | done | `tool.set` | same test |
| B-226 | `B` bucket fill, `Shift+X` autoshape, `I`/`Shift+S`/`Shift+G` eyedropper | 4.1 | missing | B-018 to B-020 | none |
| B-227 | `Q` tool lock | 4.1 | done | `tool.lock` | B-021 |
| B-228 | `Esc` | 4.1 | done | `tool.cancel` | A-197 |
| B-229 | `Enter` edits text or label, ends a multi-point line | 4.1 | done | `text.edit_begin`; draft `Enter` | `eng/int/draw_line_tests::enter_ends_the_draft_with_the_placed_points_only`, `eng/cmd/text_tests::begin_without_arguments_uses_the_single_selected_element` |
| B-230 | `Mod+Enter` edit line points / finish text | 4.1 | done | `arrow.edit_points`; editor-local commit (†) | `eng/cmd/arrow_tests::edit_points_has_its_shortcut` |
| B-231 | `Shift+Enter` newline in text | 4.1 | done | editor-local (†) | none |
| B-232 | `Tab`, `Shift+Tab` convert type | 4.1 | done | `Tab` and `Shift+Tab` run `element.convert` (next and previous type of the family) | B-064, `ui/tests/keymap::every_registered_binding_dispatches_to_its_command_on_both_platforms` |
| B-233 | `Mod` held: do not bind | 4.1 | done | B-087 | see B-087 |
| B-234 | Double-click or `Enter` on an image: crop | 4.1 | missing | B-057 (`Enter` edits nothing on images) | none |
| B-235 | `Mod+=`, `Mod++`, `Mod+-`, `Mod+0` | 4.2 | done | `view.zoom_in`, `view.zoom_out`, `view.zoom_reset` | `eng/cmd/view_tests::zoom_reset_goes_to_one_hundred_percent` |
| B-236 | `Shift+1`, `Shift+2`, `Shift+3` | 4.2 | done | `view.zoom_to_fit` | B-106 |
| B-237 | `PgUp`, `PgDn`, with `Shift` | 4.2 | done | `view.scroll_page` | B-107 |
| B-238 | Space-drag, wheel-button drag, wheel, `Shift`+wheel, `Mod`+wheel | 4.2 | partial | all but `Shift`+wheel horizontal scroll: the recognizer config has no horizontal-wheel rule | `canvas/tests/canvas::wheel_pans_the_view_and_ctrl_wheel_zooms_at_the_cursor` |
| B-239 | `Alt+Z` zen, `Alt+S` snap, `Mod+'` grid, `Alt+R` view mode | 4.2 | done | B-110 to B-113 | see those rows |
| B-240 | `Shift+Alt+D` theme | 4.2 | missing | Theme has a setting and a menu row but no key (commands.md §8.1: "not planned") | none |
| B-241 | `Alt+/` stats, `Mod+F` search, `Mod+/` and `Mod+Shift+P` palette, `?` help | 4.2 | done | UI verbs | `ui/tests/keymap::verb_bindings_match_the_catalog` |
| B-242 | `Mod+Arrow` flowchart create, `Alt+Arrow` navigate | 4.3 | done | B-023 | see B-023 |
| B-243 | Arrow, `Shift+Arrow` nudge | 4.3 | done | B-076 | see B-076 |
| B-244 | `Delete`, `Backspace`, `Mod+Delete` clear | 4.3 | done | A-196, A-104 | see those rows |
| B-245 | `Mod+X`, `Mod+C`, `Mod+V`, `Mod+Shift+V`, `Mod+A` | 4.3 | done | `clipboard.*`, `selection.all` | A-194 |
| B-246 | `Shift`-click, `Mod`-click, `Mod`-drag | 4.3 | partial | `Mod`-drag deep box select missing (B-072) | B-072 |
| B-247 | `Shift+Alt+C` copy as PNG | 4.3 | done | `clipboard.copy_as` | A-191 |
| B-248 | `Mod+Alt+C`, `Mod+Alt+V` copy and paste styles | 4.3 | done | B-077 | see B-077 |
| B-249 | `Mod+[`, `Mod+]`, with `Shift` (mac `Alt`) | 4.3 | done | `arrange.z_order` | `ui/tests/keymap.rs` (replays every binding on both platforms) |
| B-250 | `Mod+Shift+Arrows` align, `Alt+H`, `Alt+V` distribute | 4.3 | done | `arrange.align`, `arrange.distribute` | B-048, B-049 |
| B-251 | `Mod+D`, `Mod+Shift+L`, `Mod+Z`, `Mod+Shift+Z`, `Mod+Y`, `Mod+G`, `Mod+Shift+G`, `W`, `Shift+H`, `Shift+V` | 4.3 | done | registry bindings | `ui/tests/keymap::every_registered_binding_dispatches_to_its_command_on_both_platforms` |
| B-252 | `S`, `G` colour pickers, `Shift+F` font picker | 4.3 | done | UI verbs | `ui/tests/keymap::verb_bindings_match_the_catalog` |
| B-253 | `Mod+Shift+<`, `Mod+Shift+>` font size | 4.3 | done | `text.step_size` (`Mod+Shift+,` and `Mod+Shift+.`) | B-044 |
| B-254 | `Mod+K` link | 4.3 | done | `Mod+K` is the UI verb `ui.open_link_editor` (canvas scope), which opens the link dialog and ends with `element.set_link` | B-055, `ui/tests/keymap::verb_bindings_match_the_catalog`, `ui/tests/link_dialog::mod_k_is_bound_to_the_link_dialog_on_the_canvas` |
| B-255 | `Mod+O`, `Mod+S`, `Mod+Shift+S` | 4.3 | done | `ui.open_file`, `doc.save`, `ui.save_as` | A-188, A-189 |
| B-256 | `Mod+Shift+E` export dialog | 4.3 | partial | `Mod+Shift+E` copies a PNG (the old binding wins, §6.4); `Mod+E` exports a PNG; no export dialog (B-160) | A-191 |
| B-257 | `Shift`-drag constrain, `Alt`-drag centre / duplicate | 4.3 | done | B-075 | see B-075 |

---

## 3. Counts

Rows of section 2 that restate an old-app row (for example B-003 and A-002) count again, because the Excalidraw requirement lists them as separate features.

| Part | done | partial | missing | out of scope | rows | of the done rows, with † |
|---|---|---|---|---|---|---|
| A: old app (section 1) | 176 | 11 | 2 | 0 | 189 | 20 |
| B: Excalidraw (section 2) | 134 | 46 | 25 | 7 | 212 | 7 |
| Total | 310 | 57 | 27 | 7 | 401 | 27 |

What the numbers do not show:

* The † rows depend on the unmerged shell branches. Until `p6-apps` and `p6-apps-web` land, `cargo run -p drafftink` prints nothing. That is the largest single gap, and section 4.2 lists it first.
* Every test name in the Evidence column was read from source (a grep of `#[test]` functions in the 3,100 tests on main plus the p6-apps test file). I did not run the test suite for this audit. The one thing I ran was a throw-away example that printed the registry (147 commands, 23 families, all `Stable`).
* `cargo xtask harness` is red on main until branch `scenarios-2` lands (known; the `library` family has no scenario, so library evidence is unit tests only).
* Three Excalidraw rows are partial only because I could not verify the behaviour: B-091 (head fitting), B-099 (bidirectional text) and B-123 (double-tap).

---

## 4. Prioritised gap list

Priority: **P0** is a promise of the user's requirement ("keep all old features", plan §6) that does not work end to end. **P1** is an Excalidraw feature that users of the whiteboard meet daily. **P2** is useful but rarer. Sizes: S up to 1 day, M 2 to 5 days, L more. Each owner is the crate or command family that holds the missing piece; the UI crate is `ui`, the pointer paths are `engine/interact`, the shells are `app` / `apps/*`.

### 4.1 Missing

| Rank | Priority | Gap (rows) | What is needed | Owner | Size |
|---|---|---|---|---|---|
| 1 | P0 | Inline LaTeX inside text (A-056, B-098) | **Done**, the editor surface too (`text-editor` branch). `$...$` is plain content with a per-element switch (`font.inline_math`); typeset, render, measure, SVG and Excalidraw export, `text.set` and `text.edit_end` are in (`docs/typeset.md`, `docs/model.md`). The app's text editor shows formulas typeset when the caret is outside them and the source (`TextSpec::reveal`) when it is inside | `app` editor, `ui` | M |
| 2 | P0 | Phone and tablet layout (A-128, B-122) | A touch mode for the egui surfaces (bigger targets, a compact toolbar, no hover-only controls); iPad Safari run | `ui`, `app`, `apps/drafftink-web` | L |
| 3 | P1 | Hyperlinks and element links (B-055, B-056, B-254) | **Done** except two pieces. `element.set_link`, `element.open_link`, the `Mod+K` dialog, context menu rows, link icon click and the icon painter (`render::LinkMark`; the app maps `InteractionOverlay.canvas.link_icons`, †) exist. Left: "copy link to element" (B-056), which needs a document address scheme before a link can leave the document | `render` overlay (†), `engine` element family | S |
| 4 | P1 | Library panel (B-181, B-191, completes B-180) | A sidebar with thumbnails, add selection, click or drag to insert, delete, rename; the seven `library.*` commands already exist | `ui` (surface), `render` (thumbnails), `engine` library family | M |
| 5 | P1 | Lasso selection (B-002, B-221) | **Done** (branch `gaps-features-a`): `selection.select_lasso` (a path argument and the hit test of the path against element outlines in `geom::lasso`, used by `edit`, `canvas` and the engine), the lasso tool and Alt-drag in `engine/interact`, the path overlay through `canvas`, `render` | `geom`, `edit`, `canvas`, `render`, `engine`, `ui` | M |
| 6 | P1 | Sticky note (B-011, B-223) | **Done** (branch `gaps-features-c`) as a rectangle preset, not a new element kind: `element.create_sticky`, `edit::sticky`, key `N`. Left, by choice: a toolbar button, a drop shadow and fold (needs a shadow in `render`), the creation-date footer, and a re-fit of the font when the note is resized | `edit`, `engine` element family, `ui` (button), `render` (shadow) | S |
| 7 | P1 | Convert element type (B-064, B-232) | **Done** (branch `gaps-features-c`): `element.convert`, `edit::convert`, `Tab` and `Shift+Tab` cycle the type. Left: the popup with one button per type | `ui` (popup) | S |
| 8 | P1 | Paste SVG, URL, chart data (B-148, B-149, B-151) | A branch for each in the paste routing; SVG needs a rasteriser (for example resvg) in `io`; chart parse and dialog | `io`, `engine` clipboard family | M |
| 9 | P2 | More Mermaid diagram types and a preview dialog (B-176) | Class, state, ER, gantt, quadrant, timeline parsers and a layout for each; a dialog that shows the result before inserting | `io` mermaid, `edit` diagram, `engine` diagram family, `ui` | L |
| 10 | P2 | Localisation (B-100) | A string table behind every UI string; English first | `ui` | M to L |
| 11 | P2 | Hover "+" buttons for flowchart nodes (B-024) | Overlay buttons on a selected node that call `diagram.extend` | `canvas` overlay, `engine/interact`, `render` | M |
| 12 | P2 | Hint bar, scrollbars, input device setting (B-078, B-108, B-109) | Bottom hint line from the tool and gesture; drawn scrollbars; a mouse or trackpad wheel setting | `ui`, `canvas` config | S each |
| 13 | P2 | Idle state and tool badge of peers (B-200, B-203) | Two fields in `AwarenessState` and a draw rule | `protocol`, `collab`, `render` overlay | S |
| 14 | P2 | Room encryption and passwords (B-196) | Per-room key in the link fragment; encrypt update frames | `protocol`, `collab`, `apps/drafftink-server` (unchanged if frames stay opaque) | L |
| 15 | P2 | Eyedropper, bucket fill, autoshape (B-018 to B-020) | Pixel sample through the renderer port; planar region fill; stroke recognition | `render`/`gpu`, `geom`, `edit` | M each |
| 16 | P2 | Embeddables (B-016), pen mode (B-022), list markers (B-063), `Shift+Alt+D` key (B-240) | iframe overlay in the web shell and a placeholder elsewhere; ignore finger input while a pen is near; auto-continue list markers in the editor; bind `settings.cycle theme` | `scene`/`app` (embed), `canvas` (pen), `typeset`/app editor (lists), `engine` settings (key) | S to L |

### 4.2 Partial

| Rank | Priority | Gap (rows) | What is needed | Owner | Size |
|---|---|---|---|---|---|
| 1 | P0 | The shells are not on main (image drop, paste, file pickers, autosave timer, collab network, PNG render, IME, pointer pressure, PWA) | Merge `p6-apps` and `p6-apps-web` (26 `done` rows carry †; several `partial` rows do too), then run `cargo xtask ci`, the native check script and a macOS and a Windows build | `apps/drafftink`, `apps/drafftink-web`, `crates/app` | M |
| 2 | done | Calligraphy brush and simulated pressure (A-013, A-014, B-038) | Done in `gaps-brush`: `edit/src/brush.rs` (pure, time passed in), applied by `engine/interact/draw/stroke`; the painter honours `pressure_mode`. Possible improvement: the old outline is a plain offset polygon with no taper or streamline (as in perfect-freehand); not ported because the old app had none. | `edit`, `engine/interact`, `render` | M |
| 3 | P0 | Per-character colour has no user path (A-053, A-212) | **Done.** A run-aware text editor in the app (`edit::text_edit` model, `app/editor`): select a range, pick a colour, runs follow the edits and go with `text.edit_end` | `app` editor, `edit`, `render` | L |
| 4 | P0 | Arrow type and heads cannot be chosen before drawing (B-040) | **Done** (branch `gaps-features-a`): `tool.set_arrow` owns the default path type and heads (settings `arrow.type`, `arrow.start_head`, `arrow.end_head`); `interact::NewArrowPath` is gone; the panel shows the rows for the arrow tool | `engine` tool_arrow, `edit::arrow_defaults`, `ui` props | S |
| 5 | P0 | Documents and tabs cannot be named in the UI (A-102, A-170, B-169) | Name field in the save-as flow; double-click on a tab to rename; tab move and duplicate in a tab menu | `ui` (tabs, dialogs), `engine` doc and tab families | S |
| 6 | P1 | Menus and dialogs are thinner than the commands (A-168, B-118, B-160, B-185, B-186, B-187) | Export dialog with every `doc.export` option; preferences dialog for the settings with no control; Search, JSON, SVG rows in the menu; context menu rows for flip, copy as, wrap in frame, remove from frame, bind and unbind text, edit points, unlock all, grid, snap, zen, stats | `ui` | M |
| 7 | P1 | Lock cannot be undone from the canvas (B-054) | **Done.** A right click on a locked element opens the context menu with an Unlock row (`ContextTarget::Locked`, `element.unlock_all` with the id). Not done, by choice: a lock badge or popup on a left click | - | done |
| 8 | P1 | Arrowhead choices in the panel (B-041) | Add outline, hollow and cardinality heads to the panel rows | `ui` props | S |
| 9 | P1 | Peers on the canvas (A-159, B-198, B-199, B-201, B-189) | **Mostly done** (branch `render-overlays`): name label at the cursor, outlines of remote selections and peers' laser trails are in `render` and in canvas-js; the mapping from `collab_view()` is documented in `docs/render.md` §5 for the app. Left: a follow button per peer in the presence panel | `app` overlay (†), `ui` collab | S |
| 10 | P1 | Text boxes (B-046, B-059, B-061, B-097) | panel controls for vertical align, line height, auto-resize, bind and unbind; the renderer centres bound text vertically in a taller box | `engine/interact/draw/text`, `engine` text family, `ui`, `render` | M |
| 11 | P1 | SVG export is not self-contained (B-096, B-162) | `doc.export` supplies the used font bytes and the dark palette to `io::svg` | `engine` doc export, `typeset` (font bytes), `io` | M |
| 12 | P1 | Native pen pressure on Linux and macOS (A-015, B-124) | A platform backend for tablet input (XInput2, tablet-v2, NSEvent) | `gpu` shell | L |
| 13 | P1 | Settings with no control; recent and custom colours (A-042, B-030, B-036, B-071, B-087, B-088) | Rows in the preferences dialog (see rank 6); last-picked and custom colour list in the popover; a radius slider | `ui` | S |
| 14 | P2 | Image support (B-057, B-140, B-234) | `image.crop` command and handles, crop in the renderer; GIF, SVG and BMP decode | `engine` image family, `render`, `io::picture`, `ui` | M |
| 15 | P2 | Frames, groups and library from the UI (B-053, B-132, B-133, B-164, B-180, B-153, B-154) | Rename on double-click of a frame label; group name in the panel; unwrap, remove, select children in the context menu; library export and copy as SVG or text in the menu | `ui` | S |
| 16 | P2 | Stats and elbow preview (B-114, B-090) | Angle and font size fields; elbow preview while drawing | `ui`, `engine/interact/draw/linear` | S |
| 17 | P2 | Deep box select, wheel and touch details (B-072, B-238, B-123) | `deep` argument for `selection.select_region`; horizontal `Shift`+wheel; verify double-tap | `engine` selection, `canvas` | S |
| 18 | P2 | Fonts, text direction, head fitting (B-095, B-099, B-091) | Add Excalidraw-compatible faces (check licences) and a CJK and emoji fallback; Arabic and Hebrew fixtures; shrink heads on short segments | `typeset` assets, `sketch` | M |
| 19 | P2 | Interop details (B-161, B-168, B-214) | Read and write Excalidraw's PNG metadata; sync autosave between two browser tabs; fill `apps/drafftink-cli` (it is `fn main() {}`) | `io`, `app` web, `apps/drafftink-cli` | S to M |
| 20 | P2 | Untested platforms (A-219) | Run on Wayland, Windows, macOS, Safari, Firefox, a real IME and a phone | `gpu`, `app` | M |
