# Design: named versions (the "Versionsverlauf")

Status: built. Date: 2026-11-22. The commands are in `docs/commands.md` section 2.24, the CRDT layout in `docs/crdt.md` section 7a, the file format in `docs/model.md` section 7, the panel in `docs/ui.md`.

A user can name the current state of a document, see the list of names, open one as a copy and restore one. This page says where the data lives and why, what it costs, and what each path that moves a document (file, autosave, room, export) does with it.

## 1. Decision

**A named version is a snapshot of the scene, stored inside the document.** It is not a pointer into the Loro history.

| | (a) Loro frontiers | (b) scene snapshots |
|---|---|---|
| What is stored | a frontier or version vector and a name, about 20 to 100 bytes | the scene, deflated JSON, 77 bytes per element |
| Needs history | yes, the whole history from the oldest named version on | no |
| Survives the saved file | no: the file holds the JSON scene and no history | yes |
| Survives the workspace autosave | no: the same JSON | yes |
| Survives a room compaction | no: both servers trim history | yes: it is state |
| Opens in a build or tool that has no Loro | no | yes (JSON and zlib) |

### What is stored today

* **A saved `.drafftink` file** is the native JSON scene (`drafftink_scene::Document`: name, background, elements with tombstones, image blobs). It holds no Loro history, no peer ids and no operations (`docs/model.md` section 7). Opening one builds a new Loro document from it (`LoroStore::from_document`), as one commit with no history.
* **The workspace autosave** (`docs/commands.md`, "The workspace record") is the same JSON, one record per tab, plus an index. Nothing of a tab's CRDT history is written. (The encrypted-room design adds a Loro snapshot for room tabs only: `docs/design/collab-e2e.md` section 2.3.)
* **A room on the v1 server** keeps full history while peers are connected, then compacts and stores a shallow snapshot (`docs/crdt.md` section 13, `docs/server.md`). A joiner is sent a shallow or state-only snapshot.
* **An encrypted room** (planned, `docs/design/collab-e2e.md` section 8.4) compacts into one sealed snapshot by `export_snapshot()`, or the shallow form above 8 MiB. Records below the snapshot are deleted in the same transaction.

### Why not (a)

Frontiers point at operations, and every path above throws operations away. To make (a) work, the file would have to carry the Loro history (a binary format next to the open JSON one), the autosave would have to write it, every compaction would have to stop at the oldest named version, and a joiner would need the whole history instead of a state. The history of a document also keeps everything that was ever drawn and deleted, so a file that is shared would carry it. Loro can check out a frontier only inside the history it still holds, so a version older than a shallow snapshot would be gone without notice. And the file would grow with every edit since the first name, not with the number of names.

(b) costs space per version and nothing else. It needs no change to saving, autosave or compaction, because a version is plain state: a value in a map.

### What was measured

Release build, the intro document and the generated 1,000-element scene (`scenegen`). The tests that print and bound these numbers are `crates/io/tests/versions.rs` (`a_version_is_a_small_fraction_of_the_scene_it_holds`) and `crates/doc/tests/versions.rs` (`a_version_costs_its_stored_size_in_a_state_and_twice_that_in_a_full_snapshot`).

| | intro (50 elements) | 1,000 elements |
|---|---|---|
| Scene as native JSON (compact) | 23,542 B | 433,109 B |
| One version, stored (JSON, deflate level 6) | 3,812 B (6.2 : 1) | 76,586 B (5.7 : 1) |
| One version in a `.drafftink` file (base64 and metadata) | +5,187 B (+22 %) | +114,221 B (+26 %) |
| Five versions in a file | +25,883 B | +523,053 B |
| Loro full snapshot, without / with one version | 18,528 / 26,675 B (+8.1 KB) | 273,974 / 428,265 B (+154 KB) |
| Loro state only, without / with one version | 11,826 / 16,056 B (+4.2 KB) | 162,004 / 239,132 B (+77 KB) |
| Capture (`version.save`) | 0.4 ms | 8.6 ms |
| Open (`version.open`, `version.restore`) | 0.2 ms | 3.1 ms |

A version costs its stored size in a state and **twice** that in a full snapshot, because a full snapshot holds the state and the operation that wrote it. A trimmed snapshot (what a room keeps after compaction) holds it once. A deleted version leaves its operation in a full history until the next trim.

Images are never copied. An image element names its blob by content hash (`FileId`), the blob stays once in `files`, and a version lists the blob ids it needs (`NamedVersion::files`). The test `a_picture_is_in_the_file_once_however_many_versions_name_it` saves ten versions of a scene with a 300 KB picture: the picture's bytes occur once in the file, and the ten versions add less than 20 KB. Because blobs are never collected today, a version never loses its picture; `Document::unused_files` counts a version's blobs as used, so a later cleanup keeps them.

## 2. The format

`NamedVersion` (crate `scene`, `version.rs`):

| Field | Meaning |
|---|---|
| `id` | `VersionId`, `v` and 16 random characters, so two peers never collide. Commands address a version by id |
| `name` | 1 to 100 characters, trimmed, no control characters. Not unique |
| `created_ms` | the saving peer's clock, milliseconds since the Unix epoch |
| `author` | the setting `collab.user_name` at the time, when it was set (at most 64 characters) |
| `elements` | live elements in the scene |
| `files` | sorted blob ids the scene needs |
| `state` | the scene: one codec byte (1), then zlib of the JSON `{background, elements}` with every element, tombstones included |

Tombstones are in the state so that a restore brings a deleted element back exactly and every reference in it is valid. The state is read lazily: listing a version reads its metadata only. Opening one is bounded (2 MiB stored, 256 MiB inflated, at most 1,000,000 elements, every element passes `validate_element`) and returns an error, never a panic, for a hostile or damaged state.

### Where a version lives

| Place | Form |
|---|---|
| The working store of a tab (`MemoryStore`) and so every command | `SceneRead::versions`, `SceneTx::put_version`, `rename_version`, `remove_version`; the change set lists the changed ids, so a version is journaled and rolled back like a blob |
| The Loro document | root map `versions`, one mergeable child map per version (`docs/crdt.md` section 7a) |
| A `.drafftink` file, the workspace autosave, a tab copy | the field `versions` of `Document` |

### Merge

Two peers that name a version at the same moment add two children and both stay. A rename is one key (last writer wins). A delete removes the child, and wins over a rename of the same version. Names may repeat: addressing is by id, and a merge can create a duplicate however the command refuses, so `version.save` allows one. Past the cap of the saver, the merge keeps all of them (section 3).

A rename writes the name and not the scene again (about 100 bytes on the wire for a 200-element scene, tested), because `state` is a separate key that is written once and never changes.

## 3. Limits

Every limit is a named constant in `drafftink_scene::limits`.

| Constant | Value | Why |
|---|---|---|
| `MAX_VERSIONS` | 50 | what `version.save` allows. Past it the command refuses and says to delete one |
| `MAX_VERSIONS_STORED` | 500 | what a store and a file accept. Ten peers that each saved 50 must still be able to open the file |
| `MAX_VERSION_NAME_CHARS` | 100 | one line of the panel |
| `MAX_VERSION_STATE_BYTES` | 2 MiB | half of the largest live update of the protocol (4 MiB), so that a save is one update on the v1 server. About 27,000 elements at 77 bytes. A bigger scene refuses with its size and the limit |
| `MAX_VERSIONS_TOTAL_BYTES` | 32 MiB | what all versions of a document may take. 41 versions of a 10,000-element scene |
| `MAX_VERSION_INFLATED_BYTES` | 256 MiB | stops a decompression bomb |

## 4. What each path does

| Path | Versions |
|---|---|
| `doc.save` to a file or a stored name | written, as format version 2 (below) |
| Open of a file | read. A file without versions loads as it did |
| Workspace autosave and restart | written and read with the document (`SharedDocument` and `JsonJob` carry them, byte for byte like `Document::to_json`) |
| `tab.duplicate`, `tab.new` from the active tab | copied with the document |
| `version.open` | the copy has **no** versions: it is a new line of work |
| A room on the v1 server | ordinary Loro updates, at most one per save. They survive the server's compaction, a restart and a late joiner (`crates/collab/tests/server.rs`) |
| An encrypted room | the same: the relay never parses them, and compaction by `export_snapshot()` or a shallow snapshot keeps state. A save fits one sealed record |
| `version.restore` in a room | a normal edit of elements, so it syncs and merges like one |
| Excalidraw export (`doc.save` format `excalidraw`) | **dropped, with a warning**: "N named versions were left out: an Excalidraw file has no named versions." |
| PNG with scene, SVG with scene, clipboard, export of a selection, `doc.export` JSON | **excluded**: every one goes through `native::subset`, which has no versions, so the embedded scene stays small. `doc.save` is the way to keep them in a JSON file |

### Schema bump

* The **native file format** goes to version 2, **only for a document that has versions**. A document without any is written as version 1, byte for byte as before (the golden file did not change), so a build from before versions opens it. A build from before versions that meets a file with versions reads `"version": 2` first and refuses it with the existing message "Update the app to open it" instead of a parse error. `SCHEMA_VERSION` (newest readable) is 2; `BASE_SCHEMA_VERSION` (what a plain document says) is 1; `Document::with_versions` keeps the field consistent, and reading a file sets it from the content.
* The **Loro schema** `meta.schema` stays 1. `versions` is a new root map, and docs/crdt.md already says that unknown roots are ignored and kept. An older build in a room does not show the versions and does not lose them.

### Old files

No `versions` key, version 1: they load with an empty list. There is nothing to migrate.

## 5. Automatic entries later

Not built. The room left for them:

* The Loro layout reserves the key `kind`. Absent means a version the user named. This build hides any entry that has it, so a later build can add entries of its own kind (on save, hourly) without an older build listing them as names.
* A later `NamedVersion.kind` in the file would bump the file format to 3, again only for files that have such an entry.
* An automatic entry has its own cap, evicts the oldest automatic one first and never evicts a named one, and the panel groups it apart. `version.save` and the cap of 50 stay about names.

## 6. Open points

* **Autosave rewrites versions.** Each write of a tab's record includes every version. For the intro with ten versions that is 75 KB. For a 1,000-element scene with 50 versions it is 6 MB per write. The fix is a second record per tab for versions, written only when they change.
* **No thumbnails.** The list shows name, age and author. A picture needs a read command for a version (`library.thumbnail` takes a library item); it is not cheap enough to add here.
* **Largest scene.** About 27,000 elements. A bigger one cannot be named on the v1 server's limits. Chunking the state over several keys would lift it.
* **Room quotas.** The demo room of the encrypted design holds 32 MiB of sealed state (`MAX_ROOM_STATE_BYTES`, `docs/design/collab-e2e.md` section 7.4), and a version counts twice in a full snapshot until the room is compacted. Fifty versions of a 1,000-element scene are about 4 MB stored and 8 MB before compaction, which fits; fifty versions of a 10,000-element scene do not, and the limit of 32 MiB of versions per document (`MAX_VERSIONS_TOTAL_BYTES`) is above what a demo room keeps. The self-host default of 256 MiB has room.
* **A deleted version leaves its operation** in a full Loro history until the room is compacted or the history trimmed.
* **A restore does not restore pictures that were removed from `files`.** Nothing removes them today. If that changes, a restore warns, and the element shows as a missing picture (`version.restore` already reports `missing_pictures`).
