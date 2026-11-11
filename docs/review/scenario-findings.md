# Scenario findings

Found while writing the scenarios of every registered command family (`tests/scenarios/`, tasks `scenarios-all`
and `scenarios-2`). All of them are fixed; each fix has a test that failed before it. This page keeps the record.

| # | Finding | Fix | Where it is tested |
|---|---|---|---|
| F1 | `element.relabel` with empty text on a bound label removed the label, while the docs said `empty_text` for "a text element" | Kept the behaviour (it matches `text.set` and the way to empty a label), made the docs say so: a bound label given as `id` counts as its container; only free text refuses `empty_text` | `relabel_with_empty_text_on_a_bound_label_removes_the_label_but_refuses_on_free_text`, `element/lock_name_label.json` |
| F2 | A refused `text.edit_begin` (and any refused command with a `Commit` gate) committed and ended the running text edit, because the executor committed before the command validated | The gate commit is now settled after the command: a refusal puts the text edit and selection back when the commit changed no document content, and the commit leaves no ring entry. A commit that did change the document stays (it is a step of its own) | `a_refused_command_leaves_the_running_text_edit_alone`, `a_commit_that_changed_the_document_stays_when_the_command_then_refuses`, `text/refused_begin_keeps_the_running_edit.json` |
| F2b | Why `refusal_leaves_state_unchanged` did not catch F2 | It excused every `Commit`-gate command whose fingerprint lost `text=Some`. The excuse is gone; the test now compares the fingerprint for every command | `registry_tests::refusal_leaves_state_unchanged` |
| F3 | `selection.select` with a filter that matches nothing printed the debug form of the filter | The message names the set fields in plain words: ``No element matches the filter (kind: freehand). Loosen the filter, ...`` (`RefQuery::describe`, `Refusal::no_match`) | `select_refuses_unknown_targets_with_not_found`, `ref_query_describe_names_only_the_fields_that_are_set`, `selection/select_modes_region_navigate.json` |
| F4 | "A arrow needs at least 2 points" | One helper, `drafftink_scene::words::Article`, builds "a/an" for every message that names a kind. It also fixed "do not apply to an rectangle" and the same in the `edit`, `scene` and `canvas-js` messages | `words::tests`, `arrow_tests`, `arrow/point_editing_and_elbow_segments.json` |
| F5 | A refused `group.create` used up a group id | `group::group_with_ids` draws the id after every rule has passed | `a_refused_create_does_not_use_up_a_group_id`, `group_with_ids_takes_an_id_only_when_the_grouping_succeeds`, `group/group_click_enter_move_exit_ungroup.json` (expects `g1` after a refusal) |
| F6 | `doc.inspect` counted bound labels in `elements`; `element.query` leaves them out of `total` | `doc.inspect` counts them apart: `elements` is what `element.query` counts, `labels` is the number of bound labels, and `kinds`, `groups`, `frames`, `locked`, `bounds` leave them out. Full detail lists both, so `total = elements + labels` | `inspect_counts_a_shape_with_a_label_once_like_element_query`, `doc/inspect_counts_a_labelled_shape_once.json` |
| F7 | `drafftink-geom` property test `hostile_shapes` failed on some seeds: the nearest point of an ellipse of height 0 and width about 1e148 gave a NaN distance | `closest_on_ellipse` no longer squares the axes (overflow to infinity); an axis ratio beyond double range is a segment | `degenerate_ellipse_gives_a_finite_distance_regression` (the shrunk input), `hostile_shapes` |

## Found while fixing these

- `doc.inspect` always reported `save_target: null`. It now reports the target of the tab.
- `bad_args` doubled the full stop when the detail ended with one ("all=true.. See").
- `doc.save` without a Files port advised passing the data inline, which only works for reading. It now points to `doc.export` with `to="inline"`.
- `doc.open` of an `.excalidraw` or other non-native file makes that file the save target, so the next `doc.save` writes drafft.ink JSON over it. Not changed here; worth a decision (`would_overwrite` does not fire because the target is "this document's file").
- `cmd::perf::tests::last_call_reports_only_what_happened_since_the_previous_call` reads process-wide counters and can fail when other tests run in parallel. Not changed here.
