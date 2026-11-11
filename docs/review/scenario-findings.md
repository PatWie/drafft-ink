# Scenario findings

Found while writing the scenarios of every registered command family (`tests/scenarios/`, task `scenarios-all`).
The headline stories (bound arrows, grouping, an agent-built flowchart, paste of a bound pair, the legacy fixtures)
all pass, so no scenario is failing and none is marked as skipped. What follows are places where the commands
differ from `docs/commands.md` or are rough. The scenarios avoid those cases or state what the docs say; no
assertion was weakened.

Every sequence below runs on a fresh `Harness::new()` (sequential ids `e1`, `e2`, ...).

## Differences from the docs

### F1. `element.relabel` on a bound label with empty text deletes the label

Docs (§2.4): for `element.relabel`, `empty_text` is refused "on a text element only: A text element cannot be
empty. Delete it with `element.delete`." A bound label is a text element.

```
element.create {kind: rectangle, bounds: {x:0,y:0,w:100,h:60}}      -> e1
element.relabel {id: e1, text: "Label"}                             -> label e2
element.relabel {id: e2, text: ""}
```

Expected (literal reading of the docs): refused `empty_text`. Actual: ok, `{action: "removed", deleted: ["e2"]}`.
`element.relabel` with `id: e1` and empty text is the documented way to remove a label, so the result is
defensible. Either the docs should say that a bound label counts as its container here, or the command should
refuse. Free text (`element.create kind=text`) refuses as documented; `tests/scenarios/element/lock_name_label.json`
tests that case.

### F2. A refused `text.edit_begin` still ends the running text edit

Docs (§1.5, §2.8): a refusal changes nothing; `text.edit_begin` "Ends any other text edit (committed)" as part of a
successful begin.

```
element.create {kind: rectangle, bounds: {x:200,y:0,w:100,h:60}}    -> e1
text.edit_begin {id: e1}                                            -> ok
text.edit_begin {id: "nope"}                                        -> refused not_found
engine.state                                                        -> text_edit: null
```

Expected: the first edit still runs. Actual: the executor commits the running edit before the command validates
its arguments (`crates/engine/src/executor.rs`, `commit_text_edit`). With nothing typed the commit changes
nothing, but an edit with typed text would be committed by a call that was refused. `text/edit_session.json` ends
the first edit itself before the refused call, so it depends on neither behaviour.

## Rough edges (wording, no behaviour change needed)

- **F3.** `selection.select` with a `filter` that matches nothing refuses `not_found` with the debug form of the
  filter in the message: ``No element matches `{"ids":null,"text":"nothing matches this","kind":null,"style…` ``.
  The `null` fields are noise for the reader.
- **F4.** `arrow.delete_point` `too_few` reads "A arrow needs at least 2 points" (should be "An arrow").
- **F5.** `group.create` allocates the next group id before it validates. `group.create {ids: [e3]}` refuses
  `too_few`, and the next successful group is `g2`, not `g1`. Harmless, but a scenario that hard-codes group ids
  breaks when a refusal is added before it, so the scenarios save `$.group` instead.
- **F6.** `doc.inspect` counts bound labels in `elements` (a rectangle with a label is 2), while `element.query`
  leaves bound labels out of `total` unless the filter asks for them (it is 1). Both follow the docs, but one word
  means two numbers.

## A flaky gate that is a real bug

### F7. `drafftink-geom` property test `hostile_shapes` fails on some seeds (not related to the scenarios)

`cargo xtask ci` failed once in the `test` gate, and `cargo xtask test` once more, on
`drafftink-geom::properties hostile_shapes` (`crates/geom/tests/properties.rs:407`, `!n.distance.is_nan() ||
!crate_finite(q)`). A second run of the same gate passed, because proptest draws new inputs each time. The shrunk
failing input:

```
kind = Ellipse
box  = OrientedBox { center: (2.2250738585072014e-308, 0.0), size: 1.4546800786768307e148 x 0.0, angle: -15.028060024101233 }
r = 0.0, off = 0.0, p = (0.0, 0.0), q = (0.0, 0.0), dir = (0.0, 0.0), rect = 0 x 0 at (0, 0), tol = -0.0, sw = 0.0
```

Expected: a finite query point gives a finite distance (the property). Actual: the nearest-point search for a
degenerate ellipse (height 0, width about 1e148, centre at the smallest normal double) returns NaN. The test has no
fixed seed, so the gate is red on some runs. It needs a fix in `drafftink-geom` and the shrunk input kept as a
regression case in `crates/geom/tests/properties.rs`.

## Not covered, because the command is not registered

- `doc.open`, `doc.save`, `doc.new` and the other `doc` commands: only `doc.inspect` and `doc.outline` exist. The
  legacy fixtures therefore still load through `Harness::load_fixture` (its `Reason:` says it becomes `doc.open`
  later). `tests/scenarios/fixtures/legacy_*_loads.json` (and `scale/legacy_large_10000_loads.json`) compare the
  element count, the count per kind and the group count with each `.facts.json`. `legacy/crdt/*.loro` has no loader.
- `library.*`: no command is registered.

## Harness note

`Harness::scene_probe` is documented as going away with its last caller once `element.query` and `doc.outline`
exist. They exist now, and the new scenarios use the commands. `scene_probe` is still called by seven older
scenarios (`arrow/arrow_stays_attached_when_a_shape_moves`, `arrow/arrow_unbinds_when_its_target_is_deleted`,
`arrow/bind_unbind_rebind`, `arrow/type_heads_reroute`, `scale/generated_10k_loads`, `view/zoom_pan_fit`,
`pixels/shapes_are_drawn`). It stays until those are migrated; then the method, its dispatcher entry and its row in
`docs/testing.md` go together.
