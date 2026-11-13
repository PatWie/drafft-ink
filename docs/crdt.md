# CRDT schema and the Loro store (crate `drafftink-doc`, L3)

`LoroStore` is the Loro-backed implementation of the `SceneStore` trait from `drafftink-scene` (docs/model.md section 9). This page is the schema (what is stored where, and why), the rules of the store, and the trade-offs of history trimming. The wire protocol around it is `docs/protocol.md`. Builds for wasm32; no `unsafe`, `unwrap`, `expect` or `panic!` outside tests.

Contents: 1 What the old code did and what replaces it, 2 Roots, 3 Element fields, 4 Choices that matter, 5 Z-order, 6 Deleting, 7 Meta and files, 8 Bad data from peers, 9 Transactions, 10 Remote updates, 10a Repairing a merge, 11 Export, 12 Undo, 13 History and trimming, 14 Schema version and migrations, 15 Measurements, 16 Known limits.

## 1. What the old code did and what replaces it

| Old (`drafft-ink` 8ce40ab, `formats.md` section 2) | New |
|---|---|
| Every local edit cleared the Loro doc and re-added every shape (`sync_to_crdt`: 64 ms at 1k shapes, 427 ms at 10k), then sent a full snapshot. | A commit writes only the fields that changed, into one Loro commit, and yields an update of 110 to 130 bytes (an inserted rectangle: 200 bytes). 12 us at 1k, 10k and 100k. |
| Every received message rebuilt the whole document from Loro (`sync_from_crdt`: 2.05 s at 1k, over 80 s at 10k). | Loro's diff events name the touched elements; only those are decoded and patched into the view. 47 us at 1k and 10k, 52 us at 100k. |
| Shapes were maps, but points, colours and groups were nested lists of lists; a `z_order` list was shared by everyone; the name was a `LoroText` that was deleted and re-inserted. Groups owned their children by value. | Flat elements, flat fields, z-order is a field of the element, no shared list. |
| History grew without bound because each edit rewrote the document. | History is small per edit and can be trimmed (section 13). |
| A Loro `UndoManager` existed but undo was a 50-snapshot stack. | Undo is the `UndoManager`, one step per commit, local only (section 12). |
| No schema version. | `meta.schema`, a migration hook, and a clear refusal of newer documents (section 14). |

## 2. Roots

A Loro document has three root maps. Nothing else is read; unknown roots are ignored, never deleted.

| Root | Type | Content |
|---|---|---|
| `elements` | `LoroMap` | Key: element id. Value: a child `LoroMap` (an *element map*). |
| `meta` | `LoroMap` | `schema` (integer), `name` (string), `background` (string). |
| `files` | `LoroMap` | Key: SHA-256 of the bytes (64 hex digits). Value: `{mime: string, bytes: binary}`. |

The legacy collaboration format had roots `name`, `shapes` and `z_order`. A snapshot with those roots and no `meta.schema` is refused (`DocError::NotADocument`). It is never converted: the old app never persisted Loro bytes (`formats.md` section 2), so the only such bytes are old rooms on the old server.

## 3. Element fields

An element map holds **flat fields**. Nested model structures are flattened with dots. The flat form is produced from the model's own serde output (`codec.rs`), so a new field in `scene` appears here without a code change, and a stored layout cannot drift from the JSON schema.

Absent means default, in this schema version. A writer deletes a key when the value returns to its default. This keeps a plain rectangle at 6 keys (`kind`, `x`, `y`, `w`, `h`, `index`) instead of about 25, and makes "recolor, then undo" delete the key again.

| Key | Loro type | Group (`Fields`) | Default when absent | Merge |
|---|---|---|---|---|
| `kind` | string (`rectangle`, `diamond`, `ellipse`, `line`, `arrow`, `freehand`, `text`, `math`, `image`, `frame`) | KIND | (required) | last writer wins |
| `x`, `y` | double | POSITION | (required) | each key on its own |
| `w`, `h` | double | SIZE | (required) | each key on its own |
| `angle` | double | ANGLE | 0 | |
| `flip_x`, `flip_y` | bool | FLIP | false | |
| `style.stroke_color`, `style.stroke_width`, `style.stroke_style`, `style.fill_color`, `style.fill_pattern`, `style.sloppiness`, `style.opacity`, `style.corner_radius`, `style.seed` | string (colour `#rrggbb[aa]`, enums as snake_case names), double, integer | STYLE | the `Style` default | each key on its own |
| `index` | string (fractional index) | INDEX | (required) | last writer wins |
| `group_ids` | list of strings, innermost first | GROUPS | empty | whole list is one value |
| `frame_id` | string | FRAME | none | |
| `locked`, `deleted` | bool | LOCKED, DELETED | false | |
| `name`, `link` | string | NAME, LINK | none | |
| `custom` | map (JSON as Loro values) | CUSTOM | none | whole map is one value |
| `kind.*` | kind data, see below | KIND | the kind's default | each key on its own |

Kind data keys (`kind.` plus the field name of `docs/model.md` section 4):

| Kind | Keys |
|---|---|
| `line`, `arrow` | `kind.points` (binary), `kind.path`, `kind.start_binding`, `kind.end_binding` (maps, whole), `kind.start_head`, `kind.end_head`, `kind.head_size`, `kind.fixed_segments` (list of maps, whole), `kind.start_is_special`, `kind.end_is_special`, `kind.closed` |
| `freehand` | `kind.points` (binary), `kind.pressures` (binary), `kind.pressure_mode`, `kind.brush`, `kind.highlighter`, `kind.closed` |
| `text` | `kind.content` (**LoroText** with `color` marks), `kind.font.family`, `kind.font.size`, `kind.font.weight`, `kind.font.align`, `kind.font.vertical_align`, `kind.font.line_height`, `kind.font.inline_math` (only when `off`), `kind.auto_resize`, `kind.container_id`, `kind.label_position` |
| `math` | `kind.latex`, `kind.font_size` |
| `image` | `kind.file_id`, `kind.natural_width`, `kind.natural_height`, `kind.crop` (map, whole) |
| `frame` | `kind.name`, `kind.clip` |

`Element.version` is not stored. It is a local counter, raised by one per reported change.

The store only ever writes the keys of the groups a commit changed (the `Fields` of the `ChangeSet`) and only deletes keys of those groups. Keys it does not know are left alone, so a field added by a newer minor version survives edits by an older build.

## 4. Choices that matter

**Fields, not a document blob.** Two people editing different fields of one element both win: one moves while another recolors, one resizes while another locks. Last-writer-wins applies per key, so only two edits of the same key conflict, and then everyone picks the same winner. `x`/`y` are separate keys, so a horizontal move and a vertical move also merge.

**Points are one atomic value, not a `LoroList`.** `kind.points` is a binary of little-endian `f64` pairs (16 bytes per point). Reasons:
- Points are in the element's local frame, so moving, rotating and flipping never rewrite them. Moving a 20,000-point stroke writes one position key and grows the snapshot by under 200 bytes (tested).
- A line's points, its box (`w`, `h`), its bindings' fixed points and its `fixed_segments` indices only make sense together. A list CRDT would merge two people's vertex edits into a shape nobody drew, and would violate the point-box rule of the model.
- A list of two-element lists costs about 10 bytes per number in Loro's encoding and one container per point. The binary is a third of that.
- The price: two people editing different vertices of the same line at the same time conflict, and one edit is lost. Point edits are rare, short gestures that commit once, so this is the right side of the trade.

When a move and a point edit merge (position from one, points and box from the other) and the box no longer matches the points, the decoder calls `Element::refit_points()`, which depends only on the merged data, so every replica repairs the same way. If that does not fix it, the element is hidden (section 8).

**Text content is a `LoroText`.** Two people typing in one text element keep both. Edits use the scalar-value positions the model uses. The store computes one splice (common prefix and suffix) per commit. A change of font or box is an ordinary key and merges separately.

**Colour runs are marks on the text.** A run is a `color` mark (`#rrggbb[aa]`, expand type `none`) over the characters it colours, not a stored range. When a peer inserts in front of a coloured word, the colour stays on the word. Stored ranges would drift (the old `char_colors` had to be padded and truncated for exactly this reason). The model sees ranges again on read. Normal form of runs, applied on every write so that all replicas read the same: sorted, non-empty, adjacent equal colours merged, runs without a colour dropped (no colour already means "use the stroke colour"). Inline math adds nothing to the content: `$E=mc^2$` is characters of the same `LoroText`, so two people typing a formula, or one typing inside a formula another is coloring, merge like any text. `kind.font.inline_math` is an ordinary key (last writer wins). Marks are not part of the synced data: every `LoroDoc` the store creates is configured with the `color` style (`text::configure`).

**Group stacks, bindings and `custom` are whole values.** A group stack is consistent only as a unit (validation checks it). A binding is `{target, mode, fixed_point}` and a half-merged one would point at one element with the position meant for another. Start and end bindings are separate keys, so one person can rebind the start while another rebinds the end (tested).

**Enums are strings.** Readable in `debug_json()` and in tools, forward compatible with new variants through the schema version rather than through numeric codes. Loro compresses repeated keys and strings in snapshots.

**Element identity is a mergeable container.** The element map is created with `ensure_mergeable_map(id)`. Its container id is derived from (parent, key, type), so two peers that create the same id get the same container and their fields merge (tested), and undoing a hard removal brings back the same container, with edits that peers made meanwhile (Loro's `UndoManager` documentation, "Undoing deletion of a map child"). A regular child container would be reconstructed under a new id and lose concurrent edits. The text content uses `ensure_mergeable_text` for the same reason.

## 5. Z-order

There is no shared order list. The z-order of an element is its `index` field, a fractional index (`ZIndex`, docs/model.md section 6). Consequences:
- A reorder writes one key on the moved element, not a list splice. Concurrent reorders of different elements merge.
- Two peers that insert at the same spot make the same key. The model orders by `(index, id)`, so everyone sees the same order (tested, also for inserts between the same two neighbours). The store does not rewrite keys during a merge; the engine may run `zindex::repair` as an ordinary command if it wants distinct keys.
- A reorder that races with a restyle of the same element keeps both (tested).

## 6. Deleting

- **Soft delete** (`deleted: true`) is what commands do. It is a normal field, so a delete that races with a move keeps both: the element is deleted at its new position, and undeleting it later restores the moved element (tested). Tombstones stay in the document and in the order.
- **Hard removal** (`SceneTx::remove`) deletes the element map's key. A peer's concurrent edit of that element is hidden with it. It exists for undoing a creation and for garbage collection. Because the container is mergeable, re-creating the same id writes over what remains; the store deletes stale keys of the groups it writes, and all keys when the element had been hidden as invalid.
- Garbage collection of old tombstones is not done by the store. If a policy wants it, it removes tombstones in one commit that all peers have seen.

## 7. Meta and files

- `meta.name` and `meta.background` are strings, absent while default ("Untitled", white). Last writer wins; name and background are independent keys. The name is a string, not a `LoroText`: nobody edits a document title concurrently character by character, and the old code's delete-all-then-insert was a bug source.
- `meta.schema` is written once when a document is created (section 14).
- `files[hash]` is written once per distinct picture and is immutable, since the key is the content hash. Two peers adding the same picture write the same value. On read, an entry whose bytes do not hash to its key, whose media type is not `image/*`, or whose size is over the limit, is ignored (section 8). Removing a blob is the caller's decision (`SceneTx::remove_file`); the store does not collect unused blobs.

## 8. Bad data from peers

Every peer is hostile. Rules, all tested in `tests/hostile.rs`:
- Bytes go through `LoroDoc::import` only, after a size check (`LoroConfig::max_import_bytes`, default 256 MiB) and inside `catch_unwind`. **Loro 1.17 asserts on some corrupted inputs** (for example `block_meta_encode.rs:229`, found by the mutation test with a recomputed checksum). Every call that parses outside bytes, and every later call that reads the library's state, is guarded:
  - A panic while *reading a blob* (opening a snapshot, checking its header) is an error (`DocError::Import`) and the half-built document is leaked on purpose, because dropping a `LoroDoc` whose lock was poisoned panics again.
  - A panic inside the *live* document **poisons the store** (`is_poisoned()`). Reads of the scene keep working. Edits, `apply_remote`, undo and exports return `DocError::Poisoned` ("Reload the document from the server or from a saved copy"). `replace_from_snapshot` replaces the document and clears the state. Dropping a poisoned store does not panic.
  - `cargo test` runs 6,000 random and 12,000 mutated inputs (half of them with a valid checksum) with the guard on; `PROPTEST_CASES=6000` ran clean.
  - **On wasm the release profile uses `panic = "abort"`** (`Cargo.toml`, `release-wasm`), where `catch_unwind` cannot catch. A hostile blob that makes Loro assert would then end the page. docs/protocol.md ("Opaque payloads") already has the server import update bytes through Loro's validating import under a task that catches failure. To make that protect web clients, the server should relay only bytes it imported successfully (or re-export from its own document); this page asks for that, and `drafftink-server` should do it.
- Valid Loro data with nonsense content is handled per element. An element that fails to decode (wrong type, NaN, unknown kind or enum, out-of-range number, point list of the wrong length or over `MAX_POINTS`, value nested deeper than 24 levels, a key that is both a value and a group) or that fails `validate_element` after the box repair is **hidden**: not in the view, listed by `invalid_elements()`, other elements untouched. If its author fixes it, the next update makes it appear as `Added`. A local insert over a hidden id replaces it completely.
- Keys that are not valid element ids, and element-map entries that are not containers, are ignored.
- Unknown fields inside `style.*` or `kind.*` hide the element (the model decodes strictly); unknown top-level fields are ignored and kept. A format change that adds fields to the model therefore needs a schema version bump.
- A document with `meta.schema` above this build's is refused. On `apply_remote` the update has already been merged into the CRDT (Loro offers no way to peek first), so the store goes **read-only** instead: `apply_remote` returns `NewerSchema`, `blocked_by_schema()` reports the version, the view keeps the last scene this build understood, and edits, undo and further updates return "Update the app". `replace_from_snapshot` (or reopening the document) clears it. A mutated or hostile update can trigger this, so the server does not relay or store updates that raise the schema beyond what it serves: it checks `meta.schema` after the import, rebuilds the room from before the update and answers `InvalidUpdate`.
- Trimmed snapshots that do not fit this replica's history are never dropped silently (section 13).

## 9. Transactions

`LoroStore` holds two copies of the scene that always agree: the Loro document (the replicated truth) and a `MemoryStore` view (reads, ordering, derived indexes, revisions, net change sets).

- A transaction edits the view. Reads inside it see its writes, rollback and drop are free, and nothing touches Loro.
- `commit()` first lets the view compute the net `ChangeSet` (so a no-op commit is a no-op: no Loro operation, no revision, no notification), then writes into Loro **only the groups named by each change's `Fields`**, then makes **one** Loro commit. `ChangeKind::Added` writes all groups, `Updated` only the changed ones (a move writes `x` and `y`), `Removed` deletes the key. Equal values are not rewritten. Stats: `WorkStats { elements_written, fields_written, commits, .. }`.
- Loro operations are recorded in one change per transaction with an origin and a message:
  - origin `local`: undoable. Origin `sys:local`: not undoable (`LoroTx::not_undoable()`), for loading and migrations. Origins starting with `sys:` are excluded from undo.
  - message: `LoroTx::set_label` or `describe`; `"edit"` by default. Peers and history tools see it.
  - `LoroTx::describe(label, before, after)` also records the observed editor state with the undo step (section 12).
- If writing into Loro fails halfway (it should not), the commit returns an error and the view is rebuilt from Loro before the next transaction.
- Subscribers run once per commit, after both copies are consistent.

## 10. Remote updates

`apply_remote(bytes) -> Result<ChangeSet, DocError>`:

1. Size check; refuse if the document is blocked by a newer schema.
2. A trimmed snapshot that starts after this replica's history is not mergeable: a replica that holds nothing of its own takes it (`replace_from_snapshot`), any other returns `DocError::SnapshotStartsLater` and changes nothing. A full snapshot or any update merges normally.
3. Import with a `subscribe_root` subscription active. Loro reports the touched containers; a path `elements / <id>` (also from a nested text) names an element, a diff of the `elements` root names added and removed ids, `files` names blobs, `meta` sets the meta flag.
4. Only the named elements are read from Loro and decoded. One view transaction patches the view: `insert` for new, `update` for changed, `remove` for gone. The view yields the net `ChangeSet` (a remote edit that changes nothing reports nothing), raises versions by one, and updates the derived indexes for those ids only. No rebuild, no scan.
5. `has_pending_remote()` is true when operations wait for missing predecessors (an update that arrived out of order). The client then asks the server with `export_updates_since(version_vector())`.

Work is proportional to the change. `tests/remote.rs` asserts it on a 3,000-element scene: one changed element means one decoded element (`WorkStats::elements_decoded == 1`, `full_rebuilds == 0`, and with the `profile` feature `Counter::SyncOpsApplied == 1`), five changed elements mean five, applying the same bytes twice decodes nothing, and the other 2,999 versions do not move.

## 10a. Repairing a merge

The store converges, but it does not know what the fields mean together. Two edits that are valid one after the other can merge into a scene that is not:

| Race | Merged state | `validate_scene` |
|---|---|---|
| Peer A moves target 1 of an arrow, peer B moves target 2 | the arrow's points are A's or B's (one key), so one end is off its target | none (the arrow is just wrong) |
| A binds an arrow end to a shape, B deletes the shape | binding to a tombstone | warning |
| A hard-removes a shape (undo of its creation), B binds to it | binding to nothing | error |
| A binds a text to a shape, B binds another text to it | two labels for one container | error |
| A binds a text to a shape, B groups the text with something | a label in a group its container is not in | none, but `group.create` then fails |
| A wraps a shape in a frame, B deletes the frame | child of a deleted frame | warning |
| A and B nest groups differently | members disagree about the groups around a group | error |
| A and B insert or move to the same spot | equal z-order keys, or a frame child above its frame, or a label below its container | warning |

The doc crate leaves these alone on purpose: it is below the editor rules (L3), and a store that "fixed" fields would write on every import. The engine (L5) repairs them after a merge: `Session::apply_remote` and the collab glue (`mirror_remote`) call `Session::repair_merge` (`crates/engine/src/remote_repair.rs`), which calls the rules in `drafftink_edit::repair` and the solver of `drafftink_edit::binding`. It is not the command invariant pass (`invariants.rs`), which reads a command's journal; it reads the change set of the merge.

**What it does**, in this order, each step in its own commit, on the elements the merge changed and the elements that refer to them (found through the derived index, so the cost follows the change):

1. *References* (`dangling_references`). An arrow end bound to a missing, deleted or unattachable element is freed, and stays where it is (what `element.delete` does when one peer works alone). A label whose container is missing, deleted or cannot hold a label becomes free text; when several texts claim one container the smallest id keeps it (the rule `DerivedIndex::label_of` already applies when reading). A frame child whose frame is missing, deleted or not a frame leaves the frame. A label takes the frame and the groups of its container.
2. *Groups* (`inconsistent_groups`). A group whose live members disagree about the groups around it is dissolved, smallest id first, and the check runs again until it holds. Dissolving removes the one id from the stacks. It never needs a tie-break by arrival.
3. *Z-order* (`tied_order_keys`, `children_below_frame`, `labels_above_container`). Equal keys get distinct ones: the smallest id keeps the key and the others get keys between it and the next larger key, in id order, so nothing moves on screen. A frame child above its frame gets a key just below the frame, a label below its container a key just above it.
4. *Arrows*. `binding::update_bound_arrows` solves the arrows bound to what changed, so an arrow whose targets two peers moved ends on both. Elbow arrows are routed again, and so are the unpinned elbow arrows whose box touches a shape that changed: the route goes around the other shapes, is stored as points, and depends on the scene alone, so peers that merge to the same scene compute the same route. A merge that moves a shape away from an arrow that another peer routed around it leaves that detour until the arrow or a shape near it changes again (the pass after a merge does not know where the shape was).
5. *Labels* of the arrows step 4 moved are placed again, and a container that has to grow to hold its label is followed by its arrows (at most four rounds, as in `invariants.rs`).

**Cost.** A step starts only when a changed field can matter to it (a restyle or a rename starts none), and finds its elements through the derived index. A remote move of one element in a 3,000-element scene took 55 us to apply without the repair and 57 us with it (release build, measured once by hand).

**Why peers do not answer each other for ever.** Each rule is a function of the merged scene alone: no clock, peer id, randomness or arrival order. So two peers that hold the same scene compute the same fix and write the same values. A Loro map key written twice with the same value from two peers is one value, and the store reports no change for it, so the second peer's write is not even an event for a third. A rule writes only what differs (arrow points are compared with a tolerance of 1e-9), so on a repaired scene every rule writes nothing: the pass is idempotent. A repair that loses a last-writer race to an older, concurrent value restores the violation the peer that lost sees, and that peer repairs again with a newer timestamp; each such round needs an operation the other side had not seen, and there are finitely many. The random test (below) asserts that after any number of concurrent commands and partial syncs, a full round of delivery makes nobody write.

**Undo.** A repair is one Loro commit per step with a system origin (`sys:`), the same as loading: it syncs like any local write (`TabDoc::local_writes` counts it, the collab glue sends it), and is on no undo stack. Loro's undo manager rebases the peer's own steps over it, as over a collaborator's edit. What it cannot do is bring a binding back: if A deletes a shape while B binds an arrow to it, and A then undoes the delete, the shape returns and the arrow stays free. `TabDoc::repair` writes into the replicated store and copies the result into the working store outside the journal, so a running gesture or batch keeps its journal; a repair that touches an element the gesture is changing is overwritten in the working store like any remote edit (docs/engine.md, "Known limits").

**Ring.** The merge is one `remote.apply` entry as before. A pass that wrote something adds a `remote.repair` entry with origin `remote` and the counts (`elements`, `ends_freed`, `labels_released`, `frame_children_released`, `label_groups_aligned`, `groups_dissolved`, `arrows_solved`); a pass that failed adds a refused entry. A repair never fails the update: the update is already in the document.

**Who repairs.** A view-only session never writes, so it does not repair; the peers that can edit do, and the result reaches it. A document blocked by a newer schema is read-only and not repaired. A joiner repairs what it receives in one piece. The slices of a staged import (docs/collab.md, "Big rooms") are not repaired, because a slice shows only part of the room; the room it joins was repaired by its peers.

**Not repaired**, because readers resolve it and the commands leave it too: a group with fewer than two live members (the stale id goes the next time a group command edits the element, and deleting one member and undoing must bring the group back) and duplicate names (`@name` queries report the ambiguity).

**Gestures.** A running gesture that was changing an element the merge deleted or removed is cancelled with a notice (docs/protocol.md section 9). Ending it would write the copy from before the gesture over the collaborator's delete.

**Tests.** `crates/edit/src/repair.rs` (group rules), `crates/engine/src/remote_repair_tests.rs`: the races of the table above as two-peer tests, an elbow arrow, ring entries, undo stack, view-only, and `random_concurrent_commands_on_three_peers_converge_to_a_valid_scene`: three sessions run random commands (create, connect, bind, unbind, move, resize, delete, group, ungroup, frame wrap/add/remove/unwrap, text bind/unbind, undo, redo) with random partial syncs, then settle, and the test asserts equal scenes, a working store equal to the CRDT, `validate_scene` with no error and no warning but the two above, arrows on their targets (`stale_arrows`), and no writes in the last round. `one_peer_alone_is_valid_after_any_commands` is its baseline without a merge. `crates/doc/tests/converge.rs` has the same operations on the bare store (groups, bindings, frames, arrow points, undo, redo) and asserts convergence and valid elements; the relations are the engine's.

## 11. Export

| Call | Content | Use |
|---|---|---|
| `version_vector()` | encoded Loro version vector | `Join.version` in the protocol |
| `export_updates_since(vv)` | operations the peer lacks; empty `vv` means all history held | `SyncResponse` of kind `Updates`; fails with `DocError::HistoryTrimmed` when the peer is older than the kept history (send a snapshot) |
| `take_unsent_update()` | this replica's own operations committed since the last call (operations that arrived through `apply_remote` count as sent) | the update the sync client sends after each commit; a new store starts with nothing sent, a store opened from a snapshot with everything in it sent |
| `export_snapshot()` | state and the whole history | saving a room, a joiner that wants full history |
| `export_state_only()` | state, no history | smallest; late joiners who will never merge old versions |
| `export_shallow_snapshot(keep_commits)` | state and the history of the last `keep_commits` local commits | late joiners; see section 13 |

All of them are opaque bytes to `protocol`.

## 12. Undo

Loro's `UndoManager`, configured with merge interval 0 (one step per Loro commit, so one per transaction, so one per command, batch or gesture end), `sys:` origins excluded, depth `LoroConfig::undo_depth` (default **200**; the engine passes its `history.depth`, whose default is 50) and `set_undo_depth`.

- **Local only.** The manager records this peer's operations. A collaborator's edits are never on this stack, and undo rebases the stored inverse over them (tested: A moves, B recolors, locks and adds an element; A's undo reverts only the move).
- **An undo is an edit.** It is a new local commit; `take_unsent_update()` and `export_updates_since` carry it to peers like any other, and `undo()` returns the `ChangeSet` it caused.
- **Metadata slot.** Each step stores `UndoInfo { label, before: ObservedState, after: ObservedState }` in the manager's item metadata. `ObservedState` is `{selection, selected_groups, editing_group, point_editor}` (docs/commands.md section 1.1). The engine passes it in `LoroTx::describe` when it commits. `undo()` returns `UndoStep { label, restore: before, changes }`, `redo()` returns `restore: after`. `undo_label()`, `redo_label()`, `undo_depth()`, `redo_depth()`, `can_undo()`, `can_redo()` feed `history.state`. A step with no description gets label `"edit"` and empty states.
- **Selection alone is not a step**: only commits are. The engine records the selection before and after the document step.
- **Redo** is cleared by the next new commit (Loro's rule).
- **Merging.** `LoroTx::merge_with_previous()` folds one commit into the step below it (nudging with the arrow keys is one step, not forty). Loro merges by system time only, so the store raises the manager's merge interval for that single commit and sets it back; `LoroStore::last_commit_merged()` says whether it worked (each step is stamped with the commit that pushed it, so the answer is exact even at the depth limit). The step keeps its label and its `before` state; undo hands the newest commit's `after` state to the redo step. It does nothing when the stack is empty, when the commit is not undoable, or when a collaborator's edit arrived since the step (Loro keeps those apart). The engine decides when to ask (`docs/engine.md` section 5).
- **A step that has become a no-op** (a collaborator already reverted the same things) is consumed and returns an empty `ChangeSet`; the engine may undo once more, as `history.undo` specifies.
- **Loading is not a step**: `from_document` and `from_snapshot` clear history; `clear_history()` is for `doc.open`, `doc.new`, `collab.join_room` with discard.

## 13. History and trimming

Each commit adds its operations to Loro's log. With field-level writes this is small (a move is about 110 bytes), but it is never zero, so a long session grows. Measured: 300 single-field edits on 100 elements make the full snapshot 10.8 KiB against 5.8 KiB for the same state with no history; periodic trimming keeps the snapshot flat over 2,000 edits (`tests/history.rs`).

| Form | Contains | Peers can merge | Typical use |
|---|---|---|---|
| full snapshot | state, all history | any version | archive, server of a room with long-lived offline peers |
| shallow snapshot (`keep_commits = N`) | state, history from N local commits ago | peers no older than that point | late joiners, server storage |
| state only | state, a few ops | only peers that join from this snapshot | smallest download |

**What trimming costs**
- **Undo.** Loro cannot trim a document in place. `trim_history` continues on a new document built from a shallow snapshot, so **the undo and redo stacks are cleared**. Run it when that is acceptable: after a save, after a join, when the stack is empty, or on an idle timer with the engine's agreement. `keep_commits` keeps the recent history for *other peers*; it does not keep local undo.
- **Late joiners.** A joiner takes a shallow or state-only snapshot and then ordinary updates. They never see the trimmed history (no "time travel" to before the cut).
- **Stale peers.** A peer whose version vector is older than the cut cannot be served updates (`DocError::HistoryTrimmed`). It must take a snapshot with `replace_from_snapshot`, which clears its undo history and **loses any local edits it never sent**. This is why the server should trim by age, not by size alone.
- **Unsent edits at the trimming peer.** `trim_history` takes `TrimOptions { synced_to }` (the version vector the server acknowledged) and refuses with `DocError::UnsyncedChanges` when the replica holds operations beyond it, because trimmed operations cannot be exported anymore.

**Policy** (used by `collab` and the server)
- Clients: never trim during a gesture, a text edit or while unsent edits exist. Call `should_trim(max_bytes)` (true when the full snapshot is above `max_bytes` and the state alone is under half of it) at save time and on join. Default `max_bytes`: 4 MiB. Then `trim_history` with `keep_commits: 0`, `synced_to: Some(server_vv)`.
- Server: keep full history in memory for a room while it has connected clients. When storing, write a shallow snapshot with `keep_commits` large enough to cover the offline window it promises (for example 1,000 commits), and serve joiners `export_shallow_snapshot(0)`. A client that reconnects after the window gets `HistoryTrimmed` and takes the snapshot.
- Tombstones are not collected by this; they are state, not history.

`history_stats()` reports `ops`, `snapshot_bytes`, `state_only_bytes` and whether the document is shallow.

## 14. Schema version and migrations

- `meta.schema` holds the version (`SCHEMA_VERSION = 1`). `from_snapshot` reads it:
  - none and the document is empty: a new document, version written;
  - none and the document has data: refused (`NotADocument`);
  - greater than this build's: refused (`NewerSchema`, "Update the app to open it");
  - smaller: migrated with the steps in `schema::MIGRATIONS`, in order, in one `sys:open` commit that is not undoable; a missing step is `DocError::Migration` with the version it stopped at, and the document is not opened.
- A step is `Migration { from, apply: fn(&LoroDoc) -> Result<(), DocError> }`: upgrade from `from` to `from + 1` with plain Loro operations. Rules: derive every value from the data (never from the clock, the peer id or randomness) and prefer adding keys to rewriting them, because two peers that open the same old document both migrate it, and the CRDT merges their identical writes (tested). Steps are idempotent.
- A change to the stored layout needs a version bump when it adds a key to `style.*` or `kind.*`, changes the meaning or type of a key, or changes a default. Adding a top-level element key does not.
- The built-in table is empty because version 1 is the first schema. `LoroStore::open_with_migrations` takes an explicit table for tests and tools.
- The native file format has its own version (docs/model.md section 7). The two move independently.

## 15. Measurements

Release build, AMD EPYC 7R13, criterion means (`cargo bench -p drafftink-doc --bench store`; scene: 60% rectangles, 20% text, 10% arrows, 10% 30-point strokes). Old numbers are `docs/performance.md` section 7.2.

| Operation | 1,000 | 10,000 | 100,000 | Old |
|---|---|---|---|---|
| Commit: move one element | 12 us | 12 us | 13 us | `sync_to_crdt`: 64 ms at 1k, 427 ms at 10k |
| Commit: recolor one element | 14 us | 14 us | 14 us | same |
| `apply_remote` of a one-element update | 47 us | 46 us | 52 us | `sync_from_crdt`: 2.05 s at 1k, over 80 s at 10k; applying one update to the raw CRDT, without the rebuild: 103 us |
| Edit, undo, redo | 95 us | 97 us | 98 us | undo: 50 document clones, 52 us at 1k, 17 ms at 100k per push |
| Export full snapshot | 0.14 ms | 1.4 ms | 21 ms | 2.5 ms at 1k, 33 ms at 10k |
| Export state only | 3.4 ms | 46 ms | 731 ms | |
| Import full snapshot (decodes every element into the view) | 9.2 ms | 107 ms | 1.48 s | 15 ms at 1k, 207 ms at 10k |
| Import state-only snapshot | 8.6 ms | 126 ms | 1.35 s | |
| Bulk load of N elements in one commit | 25 ms | 327 ms | | |
| Full snapshot size | 150 KiB | 1.5 MiB | 15.6 MiB | |
| State-only snapshot size | 96 KiB | 1.0 MiB | 10.3 MiB | |

**Staged import** (`LoroConfig::stage_import_bytes`, `LoroStore::import_step`, `docs/collab.md` "Big rooms"). The CRDT import itself is lazy: 8 ms for the 100,000-element snapshot into a bare document. Into the document in use it takes 1.7 s, because the undo manager asks Loro for the diff of everything the import touches, so a staged import switches the store to a fresh document (the replica must hold nothing of the user's). The call then lists the element ids with `get_value` (80 ms at 100,000) and the view is decoded in slices: 256 items take 4.7 ms at 10,000 and at most 17 ms at 100,000, 1.1 s in total.

A local edit and a remote edit cost the same at 1k and 100k: the work is independent of the scene size. Import is linear (about 14 us per element at 100k, 9 us at 1k), mostly Loro's lazy state load plus a JSON-based decode; a typed decoder could halve it if opening 100k-element files matters.

## 16. Known limits

- **Undo and a later overwrite of the same field.** If a peer writes the same key after your edit and you then undo, your undo writes the old value back, because it is a newer write. Other keys and other elements are never touched. A "skip fields that a peer changed since" rule would need per-key authorship (`get_last_editor`) and is not implemented.
- **Same-key concurrent edits** (two people drag the same element at once, edit the same vertex, or retype the same style field) keep one of the two. This is true of every last-writer-wins field.
- **Hard removal races with edits** and removal wins (section 6). Commands use tombstones.
- **A repair does not restore what it freed.** A binding freed because its target was deleted is not bound again when the delete is undone (section 10a).
- **Label geometry after concurrent edits of a shape and its label text** is not laid out again until the next local edit of either; arrows and their labels are (section 10a).
- **A trimmed document cannot keep local undo** (section 13).
- **No collection of unused blobs or old tombstones.**
- **Read cost**: opening a very large document decodes every element once.
