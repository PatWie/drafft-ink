# Regression audit: old app (`8ce40ab`) against the rewrite (`main` at `b7f8011`)

Run on 2026-11-08 on the research box (Ubuntu x86_64, 64 cores, NVIDIA L40S, driver 580.178.04, Rust 1.97.1, load average 5 to 13 from other agents). The old app was built into `/tmp/old-target`, the new one into `/tmp/new-target`; neither source tree was modified. Raw logs are in `/tmp/audit/*.log` (not committed).

## 1. Counts

### 1.1 Baseline: `cargo test --workspace` in `drafft-ink` at `8ce40ab`

Same result as `research/baseline-tests.log` (158 tests, the same 158 names).

| Crate | passed | failed | ignored |
|---|---|---|---|
| drafftink-core | 155 | 0 | 0 |
| drafftink-render | 3 | 0 | 0 |
| drafftink, -app, -server, -widgets | 0 | 0 | 0 |
| doc tests (all crates) | 0 | 0 | 0 |
| **Total** | **158** | **0** | **0** |

### 1.2 HEAD

| Suite | Command | passed | failed | skipped / ignored |
|---|---|---|---|---|
| Unit and integration | `cargo nextest run --workspace --all-features --no-fail-fast` | **4072** | 0 | 0 skipped |
| Doc tests | `cargo test --workspace --all-features --doc` | 6 | 0 | 2 ignored |
| Harness | `cargo xtask harness` | 94 (82 scenarios, 11 accounting, 1 scenario guard) | 0 | 0 |
| Legacy fixtures | `cargo xtask fixtures` | 1 test; 20 fixtures, 53,420 checks | 0 failures | 0 |
| Browser e2e | `cargo xtask e2e` | 73 | 0 | 0 |
| Native e2e | `cargo xtask e2e` | 15 | 0 | 0 |
| Perf work counters | `cargo xtask perf` | 42 tests in 5 binaries; 13 scenario reports within `perf/budgets.json` | 0 | 0 |
| Perf timing, criterion | `cargo xtask perf --bench` | within the limits | 0 | - |
| Perf, real app, native | `cargo xtask perf --app native` | 16 of 16 scenarios within budget | 0 | - |
| Perf, real app, web | `cargo xtask perf --app web` (needs `build-web.sh --features scenarios`) | 15 of 16 within budget on the first run; the 16th (`wheel@10k`, p99 135.9 ms, limit 100) passed 3 of 3 reruns (p99 2.5, 6.6, 3.3 ms) | 1 flaky under load | - |
| Size | `cargo xtask size` | within the budgets | 0 | - |

Notes on reading these numbers.
- nextest reports 0 skipped because a scenario or test that needs a GPU adapter passes silently without one. I ran the two `requires.gpu` scenarios with `DRAFFTINK_REQUIRE_GPU=1` (2 passed) and `cargo test -p drafftink-render --features gpu-tests --test gpu` (18 passed on the L40S; they compare pixels with the old renderer's PNGs). So no GPU test was a silent skip on this run.
- `cargo xtask perf --bench` and `--app` read `target/criterion` and `target/release` of the worktree. I had set `CARGO_TARGET_DIR=/tmp/new-target`, so the first attempt of each failed on a missing path; I symlinked both and reran, then removed the links. This is a property of my setup, not a defect (`xtask` could honour `CARGO_TARGET_DIR`; see section 5).
- The old-app crates and the new crates do not match one to one, so the per-crate table below is for orientation only.

HEAD per crate (nextest, all binaries of the crate summed):

| Crate | tests | Crate | tests | Crate | tests |
|---|---|---|---|---|---|
| engine | 1311 | ui | 370 | edit | 596 |
| geom | 202 | io | 197 | render | 131 |
| canvas | 114 | typeset | 129 | testkit | 136 |
| server | 125 | doc | 99 | collab | 79 |
| bridge | 73 | scene | 102 | profile | 64 |
| xtask | 59 | app | 55 | sketch | 55 |
| canvas-js | 45 | protocol | 30 | web | 30 |
| gpu | 29 | mcp | 26 | e2e | 15 |

(sum 4072)

### 1.3 Cross-version check (`cargo xtask fixtures`)

The 20 legacy fixtures and their `*.facts.json` were produced by the OLD code (`tests/fixtures/README.md`). `check_fixture_facts` loads each one with the NEW code and compares 53,420 facts (counts, bounds, z order, groups, colours, CRDT container trees): 0 failures. The fixtures are: `all_shapes`, `groups_nested`, `images`, `large_1000`, `large_10000`, `math`, `styles_arrow`, `styles_ellipse`, `styles_freehand`, `styles_line`, `styles_rectangle`, `text_char_colors`, `text_fonts`, `welcome`, `z_order`, and the CRDT snapshots `crdt/{all_shapes, groups_nested, images, styles_line, text_char_colors}`.

Other old-versus-new evidence that passes at HEAD: `crates/geom/tests/legacy_snap.rs` (proptest against the old `snap.rs` copied unchanged), `crates/sketch/tests/old_app.rs` (old sketch paths), `crates/edit/tests/brush_old_app.rs` (old brush), `crates/typeset/tests/legacy.rs` and `crates/render/tests/gpu.rs` (old renderer PNGs), `crates/io/tests/{legacy,excalidraw,mermaid}.rs` (old importers' output).

## 2. The 158 old tests against the new suite

The old tests are unit tests of `drafftink-core` and `drafftink-render` types (`Shape`, `CanvasDocument`, `Camera`, `CrdtDocument`, `ToolManager`, ...) that no longer exist, so the same test cannot run on both. Each old test is mapped by behaviour to new tests. I checked that every cited test function exists in the file named and that the whole suite passes (section 1.2); I did not mutate code to prove that a cited test fails when the behaviour breaks.

Status: **covered, passing** (the behaviour is asserted by a passing new test), **covered differently (deliberate)** (the old assertion cannot hold, and the change is written down; the citation is in the row), **not covered**.

| Old group | Old tests | Covered, passing | Covered differently | Not covered |
|---|---|---|---|---|
| `camera` | 7 | 3 | 4 | 0 |
| `canvas` | 11 | 11 | 0 | 0 |
| `collaboration` | 6 | 4 | 2 | 0 |
| `crdt` | 9 | 9 | 0 | 0 |
| `excalidraw` | 9 | 6 | 3 | 0 |
| `mermaid` | 32 | 28 | 1 | 3 |
| `selection` | 8 | 7 | 1 | 0 |
| `shapes` | 36 | 30 | 6 | 0 |
| `snap` | 13 | 13 | 0 | 0 |
| `storage` | 15 | 8 | 7 | 0 |
| `sync` | 5 | 0 | 5 | 0 |
| `tools` | 4 | 4 | 0 | 0 |
| `vello_impl` | 3 | 3 | 0 | 0 |
| **Total** | **158** | **126** | **29** | **3** |

Two things the counts hide.
- Where the 29 "covered differently" rows get their justification. `docs/commands.md` §7.5 or `docs/deviations.md`: 7 rows (the five group-shape tests, item 11 and D4; two autosave tests, item 18). Another design document only: 22 rows (camera formulas, 3: `docs/canvas-api.md`; the sync and collaboration mirror, 7: `docs/protocol.md`, `docs/crdt.md`; Excalidraw library wrappers, 3: `docs/model.md`; autosave manager creation, 1; the byte `Storage` port replacing the typed document storage, 4: `docs/commands.md` §1; the 8-handle transformer, 1: `docs/canvas-api.md`; image format by bytes, 1: `docs/formats.md`; max zoom 30 instead of 10, 1: `docs/commands.md` §3 view; layered layout replacing the old rank code, 1: `docs/commands.md` §2.19). Items outside §7.5 and `deviations.md` could be added there if the coordinator wants one list of every behaviour change.
- Mermaid subgraph clusters are the only real loss; see section 3.

### Table: old test -> new evidence -> status

| Old test (at `8ce40ab`) | New evidence | Status |
|---|---|---|
| `camera::tests::test_default_camera` | `tests/scenarios/view/zoom_at_center_scroll_screenshot.json`<br>`crates/engine/src/cmd/doc_tests.rs` `new_replaces_the_document_and_resets_everything_around_it` | covered, passing - default is 100% = zoom 1.68, as before |
| `camera::tests::test_pan` | `crates/canvas/src/viewport.rs` `pan_moves_the_content_with_the_pointer`<br>`crates/engine/src/cmd/view_tests.rs` `pan_moves_the_content_with_the_pointer` | covered, passing |
| `camera::tests::test_roundtrip_conversion` | `crates/canvas/src/viewport.rs` `round_trip_and_centre` | covered, passing |
| `camera::tests::test_screen_to_world_identity` | `crates/canvas/src/viewport.rs` `round_trip_and_centre`<br>`crates/canvas/tests/canvas.rs` `the_viewport_centres_zooms_and_tells_what_is_visible` | covered differently (deliberate) - camera is now a centre point `scroll` plus `zoom`, not a screen offset (docs/canvas-api.md Viewport, docs/commands.md section 3 view); the old formula has no equivalent |
| `camera::tests::test_screen_to_world_with_offset` | `crates/canvas/src/viewport.rs` `round_trip_and_centre`<br>`crates/canvas/tests/canvas.rs` `the_viewport_centres_zooms_and_tells_what_is_visible` | covered differently (deliberate) - camera is now a centre point `scroll` plus `zoom`, not a screen offset (docs/canvas-api.md Viewport, docs/commands.md section 3 view); the old formula has no equivalent |
| `camera::tests::test_screen_to_world_with_zoom` | `crates/canvas/src/viewport.rs` `round_trip_and_centre`<br>`crates/canvas/tests/canvas.rs` `the_viewport_centres_zooms_and_tells_what_is_visible` | covered differently (deliberate) - camera is now a centre point `scroll` plus `zoom`, not a screen offset (docs/canvas-api.md Viewport, docs/commands.md section 3 view); the old formula has no equivalent |
| `camera::tests::test_zoom_clamp` | `crates/canvas/src/viewport.rs` `zoom_is_clamped_and_stops_at_the_limit`<br>`crates/engine/src/cmd/view_tests.rs` `set_moves_the_camera_and_clamps_zoom` | covered differently (deliberate) - limits are 0.1..30 (old 0.1..10); stated in docs/commands.md section 3 (view), but not in 7.5 or deviations.md |
| `canvas::tests::test_add_shape` | `crates/engine/src/cmd/element_create_tests.rs` `undo_takes_the_element_and_the_selection_back_and_redo_restores_both`<br>`crates/doc/tests/schema.rs` `every_kind_and_field_survives_the_crdt` | covered, passing |
| `canvas::tests::test_canvas_selection` | `crates/engine/src/cmd/selection_tests.rs` `select_replaces_adds_removes_and_toggles`<br>`crates/edit/tests/structure_selection.rs` `add_remove_and_toggle_work_on_units` | covered, passing |
| `canvas::tests::test_delete_selected` | `crates/engine/src/cmd/element_tests.rs` `delete_removes_the_selection_and_undo_brings_back_both_elements_and_selection`<br>`crates/ui/tests/context_menu.rs` `delete_removes_the_selected_element_from_the_document` | covered, passing |
| `canvas::tests::test_document_creation` | `crates/engine/src/cmd/doc_tests.rs` `new_replaces_the_document_and_resets_everything_around_it` | covered, passing |
| `canvas::tests::test_remove_shape` | `crates/engine/src/cmd/element_tests.rs` `delete_removes_the_selection_and_undo_brings_back_both_elements_and_selection`<br>`crates/edit/tests/structure_lifecycle.rs` `delete_then_restore_returns_to_the_start` | covered, passing - delete is a soft delete (tombstone), docs/commands.md element.delete |
| `canvas::tests::test_shapes_at_point` | `crates/canvas/tests/hit.rs` `z_order_decides_and_hit_all_lists_topmost_first`<br>`crates/canvas/tests/hit.rs` `filled_shapes_are_hit_inside`<br>`crates/edit/tests/diagram_query.rs` `element_at_all_orders_top_down_and_skips_locked` | covered, passing |
| `canvas::tests::test_undo_add_shape` | `crates/doc/tests/undo.rs` `each_commit_is_one_step_and_undo_redo_round_trip`<br>`crates/engine/src/cmd/element_create_tests.rs` `undo_takes_the_element_and_the_selection_back_and_redo_restores_both` | covered, passing |
| `canvas::tests::test_undo_clears_redo` | `crates/doc/tests/undo.rs` `a_new_commit_clears_the_redo_stack`<br>`crates/engine/src/cmd/history_tests.rs` `a_new_step_clears_what_could_be_redone` | covered, passing |
| `canvas::tests::test_undo_empty_stack` | `crates/engine/src/cmd/history_tests.rs` `undo_and_redo_with_nothing_to_do_refuse_in_the_catalog_words` | covered, passing - now a refusal with words instead of `false` (docs/commands.md section 1.5) |
| `canvas::tests::test_undo_remove_shape` | `crates/engine/src/cmd/element_tests.rs` `delete_removes_the_selection_and_undo_brings_back_both_elements_and_selection`<br>`crates/engine/src/cmd/history_tests.rs` `undo_takes_back_the_last_step_and_restores_the_selection` | covered, passing |
| `canvas::tests::test_z_order` | `crates/edit/tests/structure_zorder.rs` `old_front_and_back`<br>`crates/engine/src/cmd/arrange_tests.rs` `z_order_front_back_forward_backward_and_undo` | covered, passing |
| `collaboration::tests::test_add_shape_with_sync` | `crates/engine/src/cmd/collab_tests.rs` `concurrent_edits_made_with_commands_converge`<br>`crates/collab/tests/server.rs` `concurrent_edits_of_three_clients_converge` | covered, passing |
| `collaboration::tests::test_collaboration_disabled_by_default` | `crates/engine/src/cmd/collab_tests.rs` `status_lists_the_people_and_reads_only` | covered, passing - a fresh session reports state disconnected, room null |
| `collaboration::tests::test_export_import_snapshot` | `crates/doc/tests/converge.rs` `a_late_joiner_gets_the_state_from_a_snapshot_then_updates`<br>`crates/collab/tests/state_machine.rs` `joining_imports_the_servers_data_and_uploads_what_the_server_lacks` | covered, passing |
| `collaboration::tests::test_remove_shape_with_sync` | `crates/doc/tests/converge.rs` `delete_against_edit_keeps_both_when_deleting_is_a_tombstone`<br>`crates/engine/src/cmd/collab_tests.rs` `concurrent_edits_made_with_commands_converge` | covered, passing |
| `collaboration::tests::test_sync_from_crdt` | `crates/doc/tests/remote.rs` `a_full_snapshot_applied_to_an_empty_replica_adds_everything`<br>`crates/doc/tests/remote.rs` `a_single_element_update_touches_a_single_element` | covered differently (deliberate) - the mirror (`sync_to_crdt` clear and re-add, `sync_from_crdt` rebuild) is gone: commits write Loro fields directly and remote updates are patches (docs/protocol.md intro, docs/crdt.md, docs/plan.md) |
| `collaboration::tests::test_sync_to_crdt` | `crates/doc/tests/schema.rs` `a_local_edit_writes_only_the_changed_fields`<br>`crates/doc/tests/remote.rs` `take_unsent_update_sends_each_commit_once` | covered differently (deliberate) - the mirror (`sync_to_crdt` clear and re-add, `sync_from_crdt` rebuild) is gone: commits write Loro fields directly and remote updates are patches (docs/protocol.md intro, docs/crdt.md, docs/plan.md) |
| `crdt::tests::test_add_shape_to_crdt` | `crates/doc/tests/conformance.rs` `loro_store_passes_the_scene_conformance_suite`<br>`crates/doc/tests/schema.rs` `the_layout_is_flat_fields_and_omits_defaults` | covered, passing |
| `crdt::tests::test_crdt_document_creation` | `crates/doc/tests/conformance.rs` `loro_store_passes_the_scene_conformance_suite` | covered, passing |
| `crdt::tests::test_crdt_undo_add_shape` | `crates/doc/tests/undo.rs` `undoing_a_creation_leaves_a_peers_element_alone_and_redo_brings_it_back`<br>`crates/doc/tests/undo.rs` `each_commit_is_one_step_and_undo_redo_round_trip` | covered, passing |
| `crdt::tests::test_crdt_undo_count` | `crates/doc/tests/undo.rs` `depth_is_configurable_and_defaults_to_200`<br>`crates/engine/src/cmd/history_tests.rs` `state_reports_labels_and_depths_in_every_mode` | covered, passing |
| `crdt::tests::test_crdt_undo_remove_shape` | `crates/doc/tests/undo.rs` `undo_covers_text_z_order_meta_and_files`<br>`crates/doc/tests/undo.rs` `undo_never_reverts_a_collaborators_edit` | covered, passing |
| `crdt::tests::test_export_import` | `crates/doc/tests/converge.rs` `a_late_joiner_gets_the_state_from_a_snapshot_then_updates`<br>`crates/doc/tests/remote.rs` `applying_the_same_update_twice_changes_nothing` | covered, passing |
| `crdt::tests::test_remove_shape` | `crates/doc/tests/converge.rs` `hard_remove_against_edit_removes_the_element_everywhere`<br>`crates/doc/tests/converge.rs` `delete_against_edit_keeps_both_when_deleting_is_a_tombstone` | covered, passing |
| `crdt::tests::test_roundtrip_rectangle` | `crates/doc/tests/schema.rs` `every_kind_and_field_survives_the_crdt`<br>`crates/doc/tests/schema.rs` `reloading_a_snapshot_gives_the_same_scene` | covered, passing |
| `crdt::tests::test_z_order_manipulation` | `crates/doc/tests/converge.rs` `reordering_while_another_peer_edits_style_merges`<br>`crates/edit/tests/structure_zorder.rs` `only_the_moved_elements_get_new_keys` | covered, passing - z order is now a per-element fractional key, not a list |
| `excalidraw::tests::element_converter_maps_core_types` | `crates/io/tests/excalidraw.rs` `basic_shapes_map_every_style_field`<br>`crates/io/tests/excalidraw.rs` `shape_counts_match_the_old_importer_where_behaviour_is_the_same` | covered, passing - images and deleted elements are handled in images_and_unsupported fixtures |
| `excalidraw::tests::empty_grid_is_empty` | `crates/io/tests/excalidraw.rs` `grid_cells_are_uniform_with_padding_and_ten_columns` | covered, passing - line 1086 of the file asserts an empty library gives no elements |
| `excalidraw::tests::grid_layout_separates_items_without_overlap` | `crates/io/tests/excalidraw.rs` `grid_layout_matches_the_old_positions`<br>`crates/io/tests/excalidraw.rs` `grid_cells_are_uniform_with_padding_and_ten_columns` | covered, passing |
| `excalidraw::tests::parses_item_name_and_wraps_in_one_group` | `crates/io/tests/excalidraw.rs` `libraries_have_the_items_and_names_the_old_importer_found`<br>`crates/io/tests/excalidraw.rs` `library_items_share_group_ids_as_in_the_file` | covered differently (deliberate) - an item is a list of elements with `group_ids`, no wrapper Group shape (docs/model.md Group row, deviations D4) |
| `excalidraw::tests::preserves_icon_and_text_as_separate_groups` | `crates/io/tests/excalidraw.rs` `library_items_share_group_ids_as_in_the_file`<br>`crates/io/tests/excalidraw.rs` `grid_groups_each_item_and_keeps_ids_unique` | covered differently (deliberate) - nested groups are group-id stacks (docs/model.md) |
| `excalidraw::tests::rejects_non_library_json` | `crates/io/tests/excalidraw.rs` `not_a_library_is_rejected_for_the_right_reason` | covered, passing |
| `excalidraw::tests::single_shape_item_is_still_wrapped_in_a_group` | `crates/io/tests/excalidraw.rs` `grid_groups_each_item_and_keeps_ids_unique` | covered differently (deliberate) - a one-element item gets no group (a group needs two members; docs/model.md Group row) |
| `excalidraw::tests::unnamed_items_get_a_fallback_name` | `crates/io/tests/excalidraw.rs` `libraries_have_the_items_and_names_the_old_importer_found` | covered, passing |
| `excalidraw::tests::v1_array_of_arrays_layout_is_supported` | `crates/io/tests/excalidraw.rs` `v1_and_v2_libraries_are_both_read` | covered, passing |
| `mermaid::flowchart::tests::back_edges_are_detected` | `crates/edit/tests/diagram_layout.rs` `cycles_are_handled`<br>`crates/edit/tests/diagram_layout.rs` `layered_flows_forward_on_acyclic_graphs` | covered differently (deliberate) - old layout internals replaced by the `edit` Sugiyama layout (docs/commands.md section 2.19); the behaviour (cycles lay out) is tested |
| `mermaid::flowchart::tests::build_produces_two_nodes_and_three_connectors` | `crates/engine/src/cmd/diagram_mermaid.rs` `a_flowchart_keeps_ids_labels_shapes_heads_and_direction`<br>`crates/io/tests/mermaid.rs` `flowchart_fixtures_match_the_old_output` | covered, passing |
| `mermaid::flowchart::tests::cycle_does_not_hang_and_ranks_are_finite` | `crates/edit/tests/diagram_layout.rs` `cycles_are_handled`<br>`crates/io/tests/mermaid.rs` `flowchart_fixtures_match_the_old_output` | covered, passing - fixture flowchart_cycle.mmd |
| `mermaid::flowchart::tests::nested_diagram_imports_and_draws_cluster_boxes` | `crates/engine/src/cmd/diagram_tests.rs` `mermaid_subgraphs_are_nested_groups_and_the_loss_is_reported` | not covered - the new import groups subgraph members and warns `subgraph title(s) are not drawn`; no cluster box is drawn and no test or doc names this as a decision |
| `mermaid::flowchart::tests::nested_layout_places_cluster_children_inside_the_cluster_box` | `crates/edit/tests/diagram_create.rs` `groups_nest_and_arrows_between_members_join_them` | not covered - no cluster box exists any more, so nothing asserts children sit inside one |
| `mermaid::flowchart::tests::parses_direction` | `crates/io/tests/mermaid.rs` `header_token_forms`<br>`crates/io/tests/mermaid.rs` `thick_stays_thick_and_direction_is_kept` | covered, passing |
| `mermaid::flowchart::tests::parses_dotted_and_thick_and_plain_links` | `crates/io/tests/mermaid.rs` `chained_links_and_all_link_forms`<br>`crates/io/tests/mermaid.rs` `thick_stays_thick_and_direction_is_kept` | covered, passing |
| `mermaid::flowchart::tests::parses_edges_with_quoted_labels` | `crates/io/tests/mermaid.rs` `label_cleaning`<br>`crates/io/tests/mermaid.rs` `flowchart_fixture_details` | covered, passing - fixture flowchart_quoted_labels.mmd |
| `mermaid::flowchart::tests::parses_nodes_with_labels_and_shapes` | `crates/io/tests/mermaid.rs` `all_node_shapes`<br>`crates/io/tests/mermaid.rs` `flowchart_fixture_details` | covered, passing |
| `mermaid::flowchart::tests::parses_various_node_shapes` | `crates/io/tests/mermaid.rs` `all_node_shapes`<br>`crates/io/tests/mermaid.rs` `node_attribute_shape_syntax` | covered, passing |
| `mermaid::flowchart::tests::pipe_label_syntax_is_parsed` | `crates/io/tests/mermaid.rs` `chained_links_and_all_link_forms`<br>`crates/io/tests/mermaid.rs` `flowchart_fixture_details` | covered, passing |
| `mermaid::flowchart::tests::plain_link_does_not_swallow_target_node` | `crates/io/tests/mermaid.rs` `old_bug_fixtures_are_read_correctly` | covered, passing |
| `mermaid::flowchart::tests::plain_link_into_shaped_node_is_not_a_label` | `crates/io/tests/mermaid.rs` `old_bug_fixtures_are_read_correctly` | covered, passing |
| `mermaid::flowchart::tests::subgraphs_capture_membership_and_are_clusters` | `crates/io/tests/mermaid.rs` `subgraph_structure`<br>`crates/io/tests/mermaid.rs` `subgraph_title_forms` | covered, passing - parser level; the drawn cluster is the N rows above |
| `mermaid::flowchart::tests::user_diagram_builds_without_panic_as_single_group` | `crates/engine/src/cmd/diagram_mermaid.rs` `a_flowchart_keeps_ids_labels_shapes_heads_and_direction` | not covered - single group and no panic are covered; the second assertion (three cluster boxes for KITCHEN, REAL, SIM) is not |
| `mermaid::flowchart::tests::user_diagram_parses_chains_clusters_and_hexagon` | `crates/io/tests/mermaid.rs` `old_bug_fixtures_are_read_correctly`<br>`crates/io/tests/mermaid.rs` `chained_links_and_all_link_forms` | covered, passing |
| `mermaid::sequence::tests::build_produces_boxes_lifelines_and_arrows` | `crates/engine/src/cmd/diagram_tests.rs` `a_mermaid_sequence_diagram_gets_lifelines_and_messages`<br>`crates/io/tests/mermaid.rs` `sequence_fixtures_match_the_old_output` | covered, passing |
| `mermaid::sequence::tests::detects_dashed_reply_arrows` | `crates/io/tests/mermaid.rs` `sequence_fixture_details`<br>`crates/io/src/mermaid/sequence.rs` `arrows_are_found_leftmost_longest` | covered, passing |
| `mermaid::sequence::tests::find_arrow_prefers_longer_dashed_token` | `crates/io/src/mermaid/sequence.rs` `arrows_are_found_leftmost_longest` | covered, passing |
| `mermaid::sequence::tests::parses_all_participants_in_order` | `crates/io/tests/mermaid.rs` `participant_declarations`<br>`crates/io/tests/mermaid.rs` `notes_create_participants_and_keep_order` | covered, passing |
| `mermaid::sequence::tests::parses_messages_and_notes` | `crates/io/tests/mermaid.rs` `sequence_fixture_details`<br>`crates/io/tests/mermaid.rs` `notes_create_participants_and_keep_order` | covered, passing |
| `mermaid::sequence::tests::self_message_is_supported` | `crates/io/tests/mermaid.rs` `sequence_fixtures_match_the_old_output`<br>`crates/engine/src/cmd/diagram_mermaid.rs` `a_sequence_diagram_has_lifelines_and_messages_below_the_participants` | covered, passing - fixture sequence_notes_self.mmd; the engine test has B-->>B |
| `mermaid::tests::detect_kind_recognizes_both_diagram_types` | `crates/io/tests/mermaid.rs` `detection_is_by_whole_word`<br>`crates/io/tests/mermaid.rs` `every_fixture_is_classified_like_the_old_importer` | covered, passing |
| `mermaid::tests::detect_kind_skips_comments_and_blanks` | `crates/io/tests/mermaid.rs` `comments_and_blank_lines_before_the_header` | covered, passing |
| `mermaid::tests::fenced_diagram_is_recognized` | `crates/io/tests/mermaid.rs` `fence_and_front_matter`<br>`crates/engine/src/cmd/clipboard_tests.rs` `mermaid_text_goes_to_the_diagram_command_when_it_exists` | covered, passing |
| `mermaid::tests::flowchart_imports_as_one_editable_group` | `crates/engine/src/cmd/diagram_mermaid.rs` `a_flowchart_keeps_ids_labels_shapes_heads_and_direction`<br>`crates/engine/src/cmd/diagram_tests.rs` `a_mermaid_flowchart_becomes_a_grouped_diagram_with_bound_arrows` | covered, passing - `group` defaults to true (docs/commands.md section 2.19) |
| `mermaid::tests::measure_text_handles_multiline` | `crates/edit/tests/diagram_create.rs` `node_size_has_a_minimum_and_follows_the_label`<br>`crates/typeset/tests/legacy.rs` `multiline_text_height_is_lines_times_line_height` | covered, passing |
| `mermaid::tests::normalize_label_converts_breaks_and_quotes` | `crates/io/tests/mermaid.rs` `label_cleaning`<br>`crates/io/src/mermaid/text.rs` `labels_are_cleaned` | covered, passing |
| `mermaid::tests::sequence_imports_as_one_editable_group` | `crates/engine/src/cmd/diagram_tests.rs` `a_mermaid_sequence_diagram_gets_lifelines_and_messages`<br>`crates/engine/src/cmd/diagram_mermaid.rs` `a_sequence_diagram_has_lifelines_and_messages_below_the_participants` | covered, passing |
| `mermaid::tests::strip_code_fence_leaves_plain_text` | `crates/io/tests/mermaid.rs` `fence_and_front_matter` | covered, passing |
| `mermaid::tests::strip_code_fence_removes_mermaid_fence` | `crates/io/tests/mermaid.rs` `fence_and_front_matter` | covered, passing |
| `mermaid::tests::unrecognized_input_returns_none` | `crates/io/tests/mermaid.rs` `rejected_inputs_say_why`<br>`crates/engine/src/cmd/clipboard_tests.rs` `text_that_only_looks_like_mermaid_is_text` | covered, passing |
| `selection::tests::test_apply_corner_manipulation` | `crates/edit/tests/structure_transform.rs` `old_rectangle_corner_resize`<br>`crates/geom/src/select.rs` `resize_corner_keeps_the_opposite_corner` | covered, passing |
| `selection::tests::test_apply_endpoint_manipulation` | `crates/engine/src/interact/select_tests.rs` `a_drag_of_a_vertex_handle_still_moves_the_point_and_a_click_on_a_midpoint_selects_nothing`<br>`crates/canvas-js/tests/interaction.rs` `point_and_midpoint_handles_reshape_a_line` | covered, passing |
| `selection::tests::test_aspect_ratio_resize` | `crates/edit/tests/structure_transform.rs` `old_aspect_ratio_is_kept`<br>`crates/geom/src/select.rs` `resize_aspect_keeps_the_anchor_for_every_corner` | covered, passing |
| `selection::tests::test_freehand_resize` | `crates/edit/tests/structure_transform.rs` `old_freehand_resize_scales_the_points` | covered, passing |
| `selection::tests::test_handle_hit_test` | `crates/geom/src/select.rs` `handle_hit_test_priority_and_tolerance` | covered, passing |
| `selection::tests::test_image_resize` | `crates/edit/tests/structure_transform.rs` `old_image_resize_keeps_the_aspect_ratio` | covered, passing - images keep their aspect ratio on corner resize; the old test dragged a square image |
| `selection::tests::test_line_handles` | `crates/canvas/src/transformer.rs` `line_has_point_and_midpoint_handles_and_no_box`<br>`crates/canvas-js/tests/interaction.rs` `point_and_midpoint_handles_reshape_a_line` | covered, passing |
| `selection::tests::test_rectangle_handles` | `crates/geom/src/select.rs` `handles_of_an_unrotated_box`<br>`crates/canvas/src/transformer.rs` `single_rotated_box_has_nine_handles_that_turn_with_it` | covered differently (deliberate) - eight resize handles plus a rotate handle (old: 4 corners plus rotate); Excalidraw-class transformer, docs/canvas-api.md |
| `shapes::arrow::tests::test_arrow_creation` | `crates/engine/src/cmd/element_create_tests.rs` `an_arrow_has_the_old_defaults_no_start_head_and_an_arrow_end_head`<br>`crates/edit/tests/arrows_linear.rs` `adding_a_new_end_frees_the_binding_of_the_old_end` | covered, passing - old test only checked the length of a two-point arrow |
| `shapes::arrow::tests::test_direction` | `crates/sketch/tests/behavior.rs` `old_arrow_has_the_old_head_geometry`<br>`crates/sketch/src/arrowhead.rs` `old_arrow_head_is_two_wings` | covered, passing - the head follows the last segment; there is no public `direction()` helper |
| `shapes::arrow::tests::test_hit_test_head` | `crates/canvas/tests/hit.rs` `lines_are_hit_near_the_stroke_and_the_tolerance_is_in_screen_pixels`<br>`crates/canvas/tests/hit.rs` `a_thick_stroke_is_hit_over_its_full_width` | covered, passing - hit at the end point of the arrow |
| `shapes::arrow::tests::test_hit_test_shaft` | `crates/canvas/tests/hit.rs` `lines_are_hit_near_the_stroke_and_the_tolerance_is_in_screen_pixels` | covered, passing |
| `shapes::ellipse::tests::test_bounds` | `crates/geom/src/shapes.rs` `ellipse_basics`<br>`crates/geom/tests/properties.rs` `bounds_are_tight` | covered, passing |
| `shapes::ellipse::tests::test_circle` | `crates/engine/src/interact/draw_tests.rs` `shift_makes_a_square` | covered, passing - Shift gives a circle; `Ellipse::circle` is gone with the shape structs |
| `shapes::ellipse::tests::test_ellipse_creation` | `crates/engine/src/interact/draw_tests.rs` `every_shape_tool_draws_one_element_with_one_command_and_one_undo_step`<br>`crates/geom/src/shapes.rs` `ellipse_basics` | covered, passing |
| `shapes::ellipse::tests::test_hit_test_center` | `crates/canvas/tests/hit.rs` `filled_shapes_are_hit_inside` | covered, passing |
| `shapes::ellipse::tests::test_hit_test_edge` | `crates/canvas/tests/hit.rs` `unfilled_shapes_are_hit_on_the_outline_only`<br>`crates/geom/tests/properties.rs` `signed_distance_agrees_with_containment` | covered, passing |
| `shapes::freehand::tests::test_add_points` | `crates/engine/src/interact/draw_stroke_tests.rs` `a_recorded_style_stroke_of_five_hundred_samples_is_one_simplified_create`<br>`crates/edit/tests/brush_old_app.rs` `the_calligraphy_brush_keeps_the_points_the_old_app_kept` | covered, passing |
| `shapes::freehand::tests::test_bounds` | `crates/geom/tests/properties.rs` `bounds_contain_every_outline_point_and_every_interior_point`<br>`crates/geom/src/shapes.rs` `polyline_open_and_closed` | covered, passing |
| `shapes::freehand::tests::test_freehand_creation` | `crates/engine/src/interact/draw_stroke_tests.rs` `the_uniform_mouse_stroke_still_has_no_pressures_and_the_old_points` | covered, passing |
| `shapes::freehand::tests::test_hit_test` | `crates/canvas/tests/hit.rs` `lines_are_hit_near_the_stroke_and_the_tolerance_is_in_screen_pixels`<br>`crates/canvas/tests/hit.rs` `closed_lines_and_freehand_are_hit_inside_only_when_filled` | covered, passing |
| `shapes::freehand::tests::test_simplify` | `crates/engine/src/cmd/element_create_tests.rs` `a_freehand_stroke_is_simplified_with_the_old_tolerance_and_keeps_its_pressures` | covered, passing |
| `shapes::group::tests::test_group_bounds` | `crates/edit/tests/structure_group.rs` `old_group_bounds_and_children_stay_individually_addressable`<br>`crates/edit/tests/structure_common_bounds.rs` `several_elements_get_their_common_box_at_the_requested_place` | covered differently (deliberate) - same as group creation (7.5 item 11) |
| `shapes::group::tests::test_group_creation` | `crates/engine/src/cmd/group_tests.rs` `create_groups_selects_the_group_and_undoes`<br>`crates/edit/tests/structure_group.rs` `old_group_bounds_and_children_stay_individually_addressable` | covered differently (deliberate) - a group is a `group_ids` stack on its members, not a container shape; ids and order of members do not change (docs/commands.md 7.5 item 11, docs/model.md, deviations D4) |
| `shapes::group::tests::test_group_hit_test` | `crates/canvas/tests/hit.rs` `unit_group_resolves_the_outermost_group_or_the_next_one_inside_the_entered_group`<br>`crates/edit/tests/structure_selection.rs` `click_selects_the_outermost_group` | covered differently (deliberate) - a click on a member selects the group unit; a gap between members hits nothing |
| `shapes::group::tests::test_nested_groups` | `crates/edit/tests/structure_group.rs` `nested_groups_wrap_the_old_one`<br>`crates/edit/tests/structure_selection.rs` `inside_an_editing_group_a_click_selects_one_level_down` | covered differently (deliberate) - 7.5 item 11 |
| `shapes::group::tests::test_ungroup` | `crates/edit/tests/structure_group.rs` `old_ungroup_keeps_member_geometry`<br>`crates/engine/src/cmd/group_tests.rs` `ungroup_removes_the_group_keeps_ids_and_order_and_selects_the_members` | covered differently (deliberate) - 7.5 item 11 |
| `shapes::image::tests::test_bounds` | `crates/engine/src/cmd/image_tests.rs` `a_small_picture_keeps_its_size`<br>`crates/canvas/tests/hit.rs` `text_is_hit_anywhere_in_its_box` | covered, passing - image box = position and size |
| `shapes::image::tests::test_fit_within` | `crates/engine/src/cmd/image_tests.rs` `a_big_picture_is_scaled_to_fit_800_and_centred_on_at`<br>`crates/engine/src/cmd/image_tests.rs` `fit_changes_the_limit` | covered, passing |
| `shapes::image::tests::test_format_detection` | `crates/engine/src/cmd/image_tests.rs` `other_formats_are_unsupported_and_the_name_is_in_the_message`<br>`crates/engine/src/cmd/image_tests.rs` `random_bytes_never_panic` | covered differently (deliberate) - the format is read from the bytes, not from the file extension; PNG, JPEG, WebP only, GIF gets a refusal in words (docs/formats.md, docs/commands.md image.insert) |
| `shapes::image::tests::test_image_creation` | `crates/engine/src/cmd/image_tests.rs` `a_small_picture_keeps_its_size`<br>`crates/engine/src/cmd/image_tests.rs` `a_damaged_picture_is_parse_failed` | covered, passing - the old test only checked PNG magic bytes |
| `shapes::line::tests::test_bounds` | `crates/geom/tests/properties.rs` `bounds_are_tight_for_thin_long_shapes`<br>`crates/geom/src/shapes.rs` `polyline_open_and_closed` | covered, passing |
| `shapes::line::tests::test_hit_test_endpoints` | `crates/canvas/tests/hit.rs` `lines_are_hit_near_the_stroke_and_the_tolerance_is_in_screen_pixels` | covered, passing |
| `shapes::line::tests::test_hit_test_on_line` | `crates/canvas/tests/hit.rs` `lines_are_hit_near_the_stroke_and_the_tolerance_is_in_screen_pixels` | covered, passing |
| `shapes::line::tests::test_line_creation` | `crates/engine/src/interact/draw_line_tests.rs` `the_arrow_tool_draws_with_the_type_and_heads_of_tool_set_arrow`<br>`crates/geom/src/shapes.rs` `polyline_open_and_closed` | covered, passing - length is derived from the points |
| `shapes::line::tests::test_midpoint` | `crates/canvas/src/transformer.rs` `line_has_point_and_midpoint_handles_and_no_box` | covered, passing - the midpoint is the segment-midpoint handle |
| `shapes::rectangle::tests::test_bounds` | `crates/geom/src/shapes.rs` `rect_bounds_under_rotation`<br>`crates/geom/tests/properties.rs` `bounds_are_tight` | covered, passing |
| `shapes::rectangle::tests::test_hit_test` | `crates/canvas/tests/hit.rs` `filled_shapes_are_hit_inside`<br>`crates/geom/src/shapes.rs` `rect_filled_and_stroke_hits` | covered, passing |
| `shapes::rectangle::tests::test_rectangle_creation` | `crates/engine/src/interact/draw_tests.rs` `every_shape_tool_draws_one_element_with_one_command_and_one_undo_step`<br>`crates/engine/src/interact/draw_tests.rs` `a_drag_in_any_direction_makes_the_same_box` | covered, passing |
| `shapes::rectangle::tests::test_rectangle_from_corners` | `crates/engine/src/interact/draw_tests.rs` `a_drag_in_any_direction_makes_the_same_box` | covered, passing |
| `shapes::text::tests::test_bounds` | `crates/typeset/tests/legacy.rs` `old_estimated_bounds_were_rough_but_in_the_same_range`<br>`crates/typeset/tests/legacy.rs` `text_fonts_ink_matches_the_old_render` | covered, passing - old bounds were estimates; the layout is now measured (fixture facts say `bounds_estimated`) |
| `shapes::text::tests::test_hit_test` | `crates/canvas/tests/hit.rs` `text_is_hit_anywhere_in_its_box` | covered, passing |
| `shapes::text::tests::test_text_creation` | `crates/engine/src/cmd/text_tests.rs` `new_text_from_edit_end_uses_the_text_defaults_of_the_tool`<br>`crates/engine/src/cmd/tool_tests.rs` `new_elements_start_from_the_old_defaults` | covered, passing |
| `shapes::text::tests::test_text_with_font_size` | `crates/engine/src/cmd/text_tests.rs` `step_size_walks_the_presets_and_undoes`<br>`crates/typeset/tests/legacy.rs` `text_fonts_ink_matches_the_old_render` | covered, passing |
| `snap::tests::test_snap_angle` | `crates/geom/src/snap.rs` `test_snap_angle`<br>`crates/geom/tests/legacy_snap.rs` `grid_and_angle_match` | covered, passing |
| `snap::tests::test_snap_line_endpoint_45_degrees` | `crates/geom/src/snap.rs` `test_snap_line_endpoint_45_degrees` | covered, passing |
| `snap::tests::test_snap_line_endpoint_disabled` | `crates/geom/src/snap.rs` `test_snap_line_endpoint_disabled` | covered, passing |
| `snap::tests::test_snap_line_endpoint_horizontal` | `crates/geom/src/snap.rs` `test_snap_line_endpoint_horizontal`<br>`crates/geom/tests/legacy_snap.rs` `grid_and_angle_match` | covered, passing |
| `snap::tests::test_snap_line_endpoint_isometric` | `crates/geom/src/snap.rs` `test_snap_endpoint_both_horizontal` | covered, passing - the old isometric function is `snap_endpoint` with both angle and grid on |
| `snap::tests::test_snap_line_endpoint_isometric_45_degrees` | `crates/geom/src/snap.rs` `test_snap_endpoint_both_45_degrees` | covered, passing |
| `snap::tests::test_snap_line_preserves_distance` | `crates/geom/src/snap.rs` `test_snap_line_preserves_distance` | covered, passing |
| `snap::tests::test_snap_ray_to_grid_lines_30_degrees` | `crates/geom/src/snap.rs` `test_snap_ray_to_grid_lines_30_degrees` | covered, passing |
| `snap::tests::test_snap_ray_to_grid_lines_45_degrees` | `crates/geom/src/snap.rs` `test_snap_ray_to_grid_lines_45_degrees` | covered, passing |
| `snap::tests::test_snap_ray_to_grid_lines_horizontal` | `crates/geom/src/snap.rs` `test_snap_ray_to_grid_lines_horizontal`<br>`crates/geom/tests/legacy_snap.rs` `ray_guides_match` | covered, passing |
| `snap::tests::test_snap_to_grid` | `crates/geom/src/snap.rs` `test_snap_to_grid`<br>`crates/geom/tests/legacy_snap.rs` `grid_and_angle_match` | covered, passing |
| `snap::tests::test_snap_to_grid_exact` | `crates/geom/src/snap.rs` `test_snap_to_grid_exact`<br>`crates/geom/tests/legacy_snap.rs` `grid_and_angle_match` | covered, passing |
| `snap::tests::test_snap_to_grid_round_up` | `crates/geom/src/snap.rs` `test_snap_to_grid_round_up`<br>`crates/geom/src/snap.rs` `rounding_is_half_away_from_zero_like_the_old_code` | covered, passing |
| `storage::autosave::tests::test_autosave_dirty_flag` | `crates/engine/src/cmd/doc_tests.rs` `autosave_writes_the_workspace_only_for_documents_that_changed`<br>`crates/engine/src/cmd/history_tests.rs` `a_remote_change_between_an_edit_and_its_undo_keeps_the_document_dirty` | covered differently (deliberate) - 7.5 item 18: autosave watches the document revision, not a flag set by the caller |
| `storage::autosave::tests::test_autosave_list_excludes_special_key` | `crates/engine/src/cmd/doc_tests.rs` `list_shows_recent_stored_and_autosaved_documents` | covered, passing - the autosave is not a recent file (asserted at line 474) |
| `storage::autosave::tests::test_autosave_load_last` | `crates/engine/src/cmd/doc_tests.rs` `opening_the_autosave_restores_the_active_tab_as_unsaved_work`<br>`crates/engine/src/cmd/library_tests.rs` `the_library_survives_a_new_session_over_the_same_storage` | covered, passing |
| `storage::autosave::tests::test_autosave_manager_creation` | `crates/engine/src/cmd/tab_tests.rs` `autosave_off_writes_nothing_and_asks_for_no_timer`<br>`crates/engine/src/cmd/doc_tests.rs` `autosave_needs_storage` | covered differently (deliberate) - `AutoSaveManager` is gone; autosave is the `doc.autosave` command with a timer request (docs/commands.md, 7.5 item 18) |
| `storage::autosave::tests::test_autosave_save_clears_dirty` | `crates/engine/src/cmd/doc_tests.rs` `autosave_writes_the_workspace_only_for_documents_that_changed`<br>`crates/engine/src/cmd/history_tests.rs` `undoing_back_to_the_saved_content_leaves_the_document_clean` | covered differently (deliberate) - 7.5 item 18 |
| `storage::file::tests::test_file_storage_delete` | `crates/app/src/native/storage.rs` `put_get_list_delete_round_trip` | covered, passing |
| `storage::file::tests::test_file_storage_list` | `crates/app/src/native/storage.rs` `put_get_list_delete_round_trip`<br>`crates/engine/src/ports.rs` `storage_round_trip_and_list` | covered, passing |
| `storage::file::tests::test_file_storage_not_found` | `crates/app/src/native/storage.rs` `put_get_list_delete_round_trip` | covered differently (deliberate) - a missing key is `Ok(None)`, not `NotFound` (asserted first in the test) |
| `storage::file::tests::test_file_storage_sanitizes_id` | `crates/app/src/native/storage.rs` `any_key_is_a_safe_file_name_and_comes_back_unchanged`<br>`crates/app/src/native/storage.rs` `a_leftover_temp_file_is_not_listed` | covered, passing - stronger: `../../etc/passwd` round trips and cannot escape |
| `storage::file::tests::test_file_storage_save_load` | `crates/app/src/native/storage.rs` `put_get_list_delete_round_trip` | covered differently (deliberate) - documents go through the key-value `Storage` port (namespaces, bytes), not a typed document store (docs/commands.md section 1 ports) |
| `storage::memory::tests::test_delete` | `crates/engine/src/ports.rs` `storage_round_trip_and_list` | covered, passing |
| `storage::memory::tests::test_exists` | `crates/engine/src/ports.rs` `storage_round_trip_and_list` | covered differently (deliberate) - typed `DocumentStorage` replaced by the byte `Storage` port; no `exists`; `get` returning `None` is the check |
| `storage::memory::tests::test_list` | `crates/engine/src/ports.rs` `storage_round_trip_and_list` | covered, passing |
| `storage::memory::tests::test_not_found` | `crates/engine/src/ports.rs` `storage_round_trip_and_list` | covered differently (deliberate) - typed `DocumentStorage` replaced by the byte `Storage` port; missing key is `None` |
| `storage::memory::tests::test_save_and_load` | `crates/engine/src/ports.rs` `storage_round_trip_and_list` | covered, passing |
| `sync::tests::test_base64_empty` | `crates/protocol/tests/hostile.rs` `empty_frame` | covered differently (deliberate) - same; raw payloads |
| `sync::tests::test_base64_padding` | `crates/protocol/tests/roundtrip.rs` `binary_payloads_are_raw_not_base64` | covered differently (deliberate) - same; base64 still exists for images in files: scene/tests/document.rs::blob_bytes_survive_as_base64 |
| `sync::tests::test_base64_roundtrip` | `crates/protocol/tests/roundtrip.rs` `binary_payloads_are_raw_not_base64` | covered differently (deliberate) - payloads are raw bytes; base64 is gone from the wire (docs/protocol.md frame format, 33% less) |
| `sync::tests::test_client_message_serialize` | `crates/protocol/tests/roundtrip.rs` `client_messages_round_trip_json`<br>`crates/protocol/tests/roundtrip.rs` `json_form_is_readable` | covered differently (deliberate) - new message set and binary framing (docs/protocol.md); the JSON form is kept for tools |
| `sync::tests::test_server_message_deserialize` | `crates/protocol/tests/roundtrip.rs` `server_messages_round_trip_json`<br>`crates/protocol/tests/version.rs` `json_join_is_version_checked_too` | covered differently (deliberate) - the old `Joined{peer_count}` message is replaced (docs/protocol.md) |
| `tools::tests::test_cancel_interaction` | `crates/engine/src/interact/draw_tests.rs` `escape_mid_drag_leaves_document_ring_and_undo_untouched`<br>`crates/engine/src/interact/draw_tests.rs` `a_new_drag_works_after_a_cancelled_one` | covered, passing |
| `tools::tests::test_select_tool_no_shape` | `crates/engine/src/interact/draw_tests.rs` `a_click_with_a_shape_tool_creates_nothing`<br>`crates/engine/src/interact/select_tests.rs` `cancelling_keeps_the_original` | covered, passing - a drag with the select tool is a marquee or a move and creates nothing (edit/tests/structure_selection.rs::marquee_touch_and_contain) |
| `tools::tests::test_tool_interaction` | `crates/engine/src/interact/draw_tests.rs` `the_preview_shows_mid_drag_and_the_document_is_untouched`<br>`crates/engine/src/interact/draw_tests.rs` `every_shape_tool_draws_one_element_with_one_command_and_one_undo_step` | covered, passing |
| `tools::tests::test_tool_selection` | `crates/engine/src/cmd/tool_tests.rs` `set_switches_tool_and_is_not_undoable`<br>`crates/engine/src/cmd/view_tests.rs` `get_reports_the_camera_the_visible_rectangle_and_modes` | covered, passing - default tool is select |
| `vello_impl::tests::test_build_empty_scene` | `crates/render/tests/compose.rs` `culling_matches_brute_force`<br>`crates/render/tests/compose.rs` `hostile_views_and_elements_never_panic` | covered, passing |
| `vello_impl::tests::test_build_scene_with_shapes` | `crates/render/tests/gpu.rs` `rectangles_match_the_old_renderer`<br>`crates/render/tests/compose.rs` `encoded_elements_follow_the_view_not_the_document` | covered, passing - gpu.rs compares the pixels with the old renderer's PNG when an adapter exists |
| `vello_impl::tests::test_renderer_creation` | `crates/render/tests/compose.rs` `hostile_views_and_elements_never_panic`<br>`crates/render/tests/overlay.rs` `an_empty_overlay_draws_nothing` | covered, passing |
## 3. Old behaviour with no new test (for the coordinator; not implemented)

Three old tests, one behaviour: the old Mermaid import drew a filled, titled rectangle ("cluster box", `CLUSTER_FILL`, `docs/formats.md` §Mermaid layout constants) around each `subgraph`, nested boxes for nested subgraphs, and placed the members inside. The new `diagram.import_mermaid` groups the members and warns `N subgraph title(s) are not drawn; the subgraph members are grouped` (`crates/engine/src/cmd/diagram_mermaid.rs:292`). The warning is tested, but no document lists the dropped box as a decision (not in §7.5, not in `deviations.md`, and `docs/parity.md` A-109 says "nested groups for subgraphs" only). Either draw the boxes or write the decision down. Tests to write if the boxes come back:

1. `crates/engine/src/cmd/diagram_tests.rs`: `mermaid_subgraphs_are_drawn_as_titled_boxes_around_their_members`. Import `graph TD\nsubgraph outer\nsubgraph inner\nA-->B\nend\nC\nend`. Expect one box element per subgraph with the subgraph title as its label, the inner box inside the outer box, and no `subgraph title` warning. Covers `nested_diagram_imports_and_draws_cluster_boxes`.
2. `crates/edit/tests/diagram_create.rs`: `children_of_a_nested_group_with_a_box_sit_inside_the_box_with_padding` (property test over nested group specs: every child's bounds are inside the bounds of its box minus the title band, and sibling boxes do not overlap). Covers `nested_layout_places_cluster_children_inside_the_cluster_box`.
3. `crates/engine/src/cmd/diagram_tests.rs`: `the_kitchen_diagram_has_three_boxes_and_one_outer_group`. Use the old test's `USER_DIAGRAM` (clusters KITCHEN, REAL, SIM) as a fixture under `tests/fixtures/mermaid/`; expect 3 boxes and one outer group. Covers the second assertion of `user_diagram_builds_without_panic_as_single_group`.

If the boxes are dropped on purpose: add the item to `docs/commands.md` §7.5, and the three rows become "covered differently".

**Number of old tests with no new coverage: 3 of 158** (all three: the Mermaid cluster box).

## 4. Performance: baseline against HEAD

Baseline numbers are from `research/baseline-perf.md` (same box, old app at `8ce40ab`). HEAD numbers are from this run unless the source column says `docs/performance.md`. The two sides do not always measure the same operation, because the old code had no spatial index and no gesture scenarios; the note says what is compared. Scenario windows are 10 s (hover 8 s). The box had a load average of 5 to 13, so treat CPU figures as indicative.

| Metric | Baseline | HEAD | Source / note |
|---|---|---|---|
| Native idle CPU | 0.3 - 0.6% | **0.5%, 0 frames, 0 redraws** | `xtask perf --app native` `idle`; same range |
| Web idle CPU (Chromium tree) | 0.6% (NVIDIA) | **1.5%, 0 frames** | `--app web` `idle`; baseline 0.6% to 0.9%; `docs/performance.md` 7.8 gives 1.0% to 2.5% for HEAD |
| Native hover, nothing changing, ~125 moves/s | 53 - 68% CPU, one frame per move | **6.6% CPU at 10k elements, 78 frames for 1000 moves** (one frame per hover change; 4.7% at 1k, 7.5% at 100k) | `hover@N`; the app's own work is 1.4 to 2.6% of it, the rest is the X11 present call |
| Native wheel pan, ~113 events/s (welcome document) | 52 - 65% CPU, falls behind input | **34.3% CPU, 664 frames in 10 s (at most one per tick), p99 8.0 ms** at 10k elements; 42.6% at 100k | `wheel@10k`, `wheel@100k`; scene is 10k elements, not the welcome document |
| Native pan, 10k / 100k elements | not measured (no scene-size scaling) | 34.0% / 44.1% CPU; frame p50 7.3 / 7.7 ms, p99 8.4 / 13.0 ms | `pan@10k`, `pan@100k` |
| Native zoom, 10k / 100k | not measured | 31.8% / 39.0% CPU; p99 7.9 / 8.1 ms; 1.0 frame per tick | `zoom@10k`, `zoom@100k` |
| Native drag 100 elements, 100k scene | not measured | 61.5% CPU; frame p50 9.1 ms, p99 10.2 ms | `drag@100k` |
| Frame time scales with scene size | `build_scene` 1.0 ms at 10k, 10.4 ms at 100k even when culled | p50 7.3 / 7.3 / 7.7 ms at 1k / 10k / 100k (pan); `encode` flat | `pan@N`, `docs/performance.md` 7.8 |
| Web pan, 100 events/s | 94.6% CPU, 33 fps (Xvfb present ceiling) | 109.9% CPU, 582 frames in 10 s (~58 fps), frame p50 1.4 ms, p99 2.1 ms (10k) | `--app web` `pan@10k`; about 1.9 ms CPU per frame against 2.9 ms before |
| Web ctrl+wheel zoom, 60 events/s | 106% CPU, 40 fps | 68.5% CPU, 310 frames in 10 s, p50 1.7 ms (10k); 89.1% and p99 41 ms at 100k | `--app web` `zoom@10k`, `zoom@100k` |
| Web hover | not measured | 17.6 - 21.4% CPU, one frame per hover change | `hover@N` |
| Hit test (point), 10k / 100k elements | 0.70 ms / 9.1 ms (linear scan) | **270 ns / 205 ns** (index) | criterion `canvas/hit/10000`, `canvas/hit/100000` |
| Marquee 800x600, 10k | 0.77 ms | **520 ns** | `geom/index_query_rect_800x600/10000` |
| Hover move through the canvas, 10k | part of a full frame | 498 ns | `canvas/canvas_hover_move/10000` |
| Scene build, warm, mostly culled / panned, 10k / 100k | 1.0 ms / 10.4 ms (`build_scene`) | **6.4 us / 6.8 us** (`compose` zoomed in, warm); pan warm at 100k 6.9 us | `compose/zoomed_in_warm/*`, `compose/pan_warm/100k`; different function, same job |
| Scene build, everything visible, 10k | 15.1 ms | 3.8 ms (level of detail, warm) | `docs/performance.md` 7.7, not rerun here |
| Smart-guide input, 10k | 1.67 ms (collect all bounds) | 0 elements scanned | work counter `moving_with_smart_guides_scans_nothing_once_the_index_exists` (passes in `cargo xtask perf`) |
| Undo point / end of a drag, 1k / 10k / 100k | 52 us / 1.2 ms / 17 ms (`push_undo` clones the whole document) | `gesture.end` of a move: 80 / 84 / 81 us | `docs/performance.md` 7.7, not rerun here |
| One drag frame, 1k / 10k / 100k | not measured | 15 / 16 / 14 us | `docs/performance.md` 7.7, not rerun here |
| Local edit commit in a room, 10k | 427 ms (`sync_to_crdt`, every edit) | **12.6 us** (`local_edit_commit/move_one_element/10000`) | criterion |
| Apply one remote single-element update | 103 us; whole-document rebuild 2.05 s at 1k | **49.4 us at 10k** (a patch) | `apply_remote_single_element/move/10000` |
| Redraw reasons | none recorded | every redraw has a reason; `unexplained` = 0 in all 16 native and 16 web scenarios | `by_reason` in the reports |
| Frame statistics | none | `PerfStats`, spans, counters, reports | `docs/performance.md` 2 to 4 |
| Wasm after wasm-opt (shipped) | 10,328,928 B | **12,508,700 B (+21%)** | `cargo xtask size` (limit 12.75 MB in `perf/budgets.json`) |
| Wasm gzip -9 | 4,658,290 B | 4,636,077 B (-0.5%) | `cargo xtask size` |
| Wasm brotli -11 | 3,462,376 B | **3,255,227 B (-6%)**; fonts 1,505,581 B more, loaded only when used | `cargo xtask size`; `docs/performance.md` 8.1 puts the first visit with fonts at 3.57 MiB, about 8% above the old 3.46 MB |
| JS glue | 118,642 B | 153,179 B (+29%) | `dist/pkg/drafftink_web.js` of the e2e build |
| Native release binary | 24.4 MB | **53.8 MB (+120%)** | `/tmp/new-target/release/drafftink`; release profile is `opt-level=3`, thin LTO (old: `opt-level=z`, full LTO) |

Reading. Every interaction path that was O(scene) in the old app is now flat in scene size, idle and no-change hover cost nothing, and a local edit or a remote update costs microseconds instead of 0.4 s to 2 s. The prices are size: the wasm module is 21% bigger before compression (brotli is 6% smaller), the JS glue 29% bigger, and the native binary 2.2 times bigger. None of these is gated except the wasm sizes.

## 5. Other findings

- **Native binary size** (24.4 MB to 53.8 MB) has no budget in `perf/budgets.json` and no line in `docs/performance.md` §8. The cause is in `Cargo.toml` (`[profile.release] opt-level = 3, lto = "thin"`, old `z` and fat LTO), plus new dependencies (rmcp, axum, egui-wgpu, Loro). Decide whether to budget it or accept it in writing.
- **First-visit transfer** (module plus the fonts the welcome page needs) is about 8% above the old module alone, per `docs/performance.md` 8.1, although the module itself is 6% smaller. The 3.5 MB target of §7.5 is met for the module only.
- **`wheel@10k` on the web** failed its p99 limit (135.9 ms against 100 ms) once, under load average 13, and passed three reruns. The limit is tight for a shared box; treat a single failure with a load average above 8 as noise.
- **`xtask perf --bench` and `--app` ignore `CARGO_TARGET_DIR`** and read `target/criterion` and `target/release/drafftink`. A private target directory (as agents and CI caches use) breaks them with a path error that does not name the cause for `--bench` ("The bench did not run"). Honour `CARGO_TARGET_DIR` in `xtask/src/perf.rs` and `scripts/perf_scenarios.py`.
- **`cargo xtask perf --app web` needs a build with `--features scenarios`**; the `dist/` that `cargo xtask size` leaves behind lacks it and the driver fails with a Playwright error. The message names the fix, but `xtask` could build it.
- **A dead-code warning** in a release build: `crates/io/src/legacy/mod.rs:111`, `arrow_bounds` is never used.
- nextest cannot tell a GPU test that ran from one that skipped; see section 1.2. `DRAFFTINK_REQUIRE_GPU=1` is the way to check on a machine that should have one.
