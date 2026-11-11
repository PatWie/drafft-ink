import { test, expect } from "../../support/app.ts";
import { box, connect, expectRefusal, undoDepth } from "../../support/build.ts";

test("family arrange: z-order, align and distribute move to documented places; each is one undo step", async ({ app }) => {
  const a = await box(app, 0, 0, 100, 60);
  const b = await box(app, 150, 40, 80, 40);
  const c = await box(app, 400, 100, 60, 30);
  const order = async () => ((await app.exec("element.query", { fields: ["id"] })).items as { id: string }[]).map((i) => i.id);
  expect(await order()).toEqual([a, b, c]);
  await app.exec("arrange.z_order", { ids: [a], op: "front" });
  expect(await order()).toEqual([b, c, a]);
  expect((await app.exec("arrange.z_order", { ids: [a], op: "front" })).changed, "already in front").toBe(false);
  await app.exec("history.undo");
  expect(await order()).toEqual([a, b, c]);
  await app.exec("selection.clear");
  expectRefusal(await app.refusal("arrange.z_order", { op: "front" }), "no_target", "no elements are selected");

  await app.exec("arrange.align", { ids: [a, b, c], edge: "top" });
  const tops = await app.elements([a, b, c]);
  expect([tops[a]?.bounds.y, tops[b]?.bounds.y, tops[c]?.bounds.y]).toEqual([0, 0, 0]);
  await app.exec("history.undo");
  expectRefusal(await app.refusal("arrange.align", { ids: [a], edge: "left" }), "too_few", "Align needs at least 2 elements");

  await app.exec("arrange.distribute", { ids: [a, b, c], axis: "horizontal" });
  const spread = await app.elements([a, b, c]);
  expect(spread[a]?.bounds.x).toBe(0);
  expect(spread[c]?.bounds.x).toBe(400);
  expect(spread[b]?.bounds.x).toBeCloseTo(210, 1);
  await app.exec("history.undo");
  expectRefusal(await app.refusal("arrange.distribute", { ids: [a, b], axis: "horizontal" }), "too_few", "Distribute needs at least 3 elements");
  expect((await app.element(b)).bounds.x, "undo put it back").toBe(150);
});

test("family group: create, ungroup, rename; a group is one unit; refusals name the missing group", async ({ app }) => {
  const a = await box(app, 0, 0);
  const b = await box(app, 200, 0);
  expectRefusal(await app.refusal("group.create", { ids: [a] }), "too_few", "at least 2 elements");
  const group = (await app.exec("group.create", { ids: [a, b] })).group as string;
  expectRefusal(await app.refusal("group.create", { ids: [a, b] }), "already_grouped", "already form one group at this level");
  expect((await app.exec("doc.inspect")).groups).toBe(1);
  await app.exec("element.lock", { ids: [a] });
  const locked = await app.refusal("group.ungroup", { ids: [a] });
  expectRefusal(locked, "locked", "is locked", "element.unlock_all");
  expect(locked.message, "it names a member of the group").toMatch(new RegExp(`\`(${a}|${b})\``));
  await app.exec("element.unlock_all", { select: false });
  await app.exec("selection.select", { group });
  expectRefusal(await app.refusal("group.set_name", { name: "" }), "bad_name", "1 to 64 characters");
  expect((await app.exec("group.set_name", { name: "pair" })).name).toBe("pair");
  expect((await app.exec("group.ungroup", { group })).removed).toEqual([group]);
  expect((await app.exec("doc.inspect")).groups).toBe(0);
  await app.exec("history.undo");
  expect((await app.exec("doc.inspect")).groups, "undo of ungroup").toBe(1);
  // Deleting members until one is left dissolves the group.
  await app.exec("element.delete", { ids: [a] });
  expect((await app.exec("doc.inspect")).groups).toBe(0);
  await app.exec("history.undo");
  expect((await app.exec("doc.inspect")).groups).toBe(1);
  await app.exec("selection.clear");
  expectRefusal(await app.refusal("group.ungroup"), "no_group", "No group is selected");
  expectRefusal(await app.refusal("group.enter"), "no_group");
});

test("family frame: wrap, rename, add, remove and unwrap; frames hold elements and cannot nest", async ({ app }) => {
  const a = await box(app, 0, 0);
  const b = await box(app, 200, 0, 100, 60, { kind: "ellipse" });
  const c = await box(app, 600, 0);
  const wrapped = await app.exec("frame.wrap", { ids: [a, b], name: "Box" });
  const frame = wrapped.frame as string;
  expect(wrapped.added).toEqual([a, b]);
  expectRefusal(await app.refusal("frame.wrap", { ids: [frame] }), "nested_frame", "Frames cannot be nested");
  await app.exec("element.move", { ids: [frame], dx: 0, dy: 100, snap: "none" });
  expect((await app.element(a)).bounds.y, "children move with the frame").toBe(100);
  await app.exec("history.undo");
  expect((await app.exec("frame.rename", { frame, name: "Renamed" })).name).toBe("Renamed");
  expectRefusal(await app.refusal("frame.rename", { frame: a, name: "x" }), "wrong_kind", "`frame.rename` works on frames");
  expect((await app.exec("frame.add", { frame, ids: [c] })).added).toEqual([c]);
  expect([...((await app.exec("frame.select_children", { frame })).selected as string[])].sort()).toEqual([a, b, c].sort());
  expect((await app.exec("frame.remove", { ids: [c] })).removed).toEqual([c]);
  expectRefusal(await app.refusal("frame.remove", { ids: [c] }), "nothing_to_do", "none of the given elements is in a frame");
  await app.exec("history.undo");
  expect((await app.element(c)).frame).toBe(frame);
  expectRefusal(await app.refusal("frame.unwrap", { frame: a }), "wrong_kind", "`frame.unwrap` works on frames");
  const unwrapped = await app.exec("frame.unwrap", { frame });
  expect([...(unwrapped.children as string[])].sort()).toEqual([a, b, c].sort());
  expect(await app.count()).toBe(3);
  await app.exec("history.undo");
  expect(await app.count(), "undo brings the frame back").toBe(4);
});

test("family arrow: bind, unbind, heads, type, points and segments; refusals name the reason", async ({ app }) => {
  const a = await box(app, 0, 0);
  const b = await box(app, 400, 0);
  const c = await box(app, 800, 0);
  const arrow = await connect(app, a, b);
  expect((await app.exec("arrow.unbind", { arrow, end: "both" })).freed).toEqual(["start", "end"]);
  expect((await app.element(arrow)).bindings).toEqual({ start: null, end: null });
  await app.exec("history.undo");
  expect((await app.element(arrow)).bindings).toEqual({ start: a, end: b });
  const rebound = await app.exec("arrow.bind", { arrow, end: "end", target: c });
  expect((rebound.binding as { target: string }).target).toBe(c);
  expectRefusal(await app.refusal("arrow.bind", { arrow: a, end: "end", target: b }), "wrong_kind", "arrows and lines");
  expectRefusal(await app.refusal("arrow.bind", { arrow, end: "end", target: "nope" }), "not_found", "nope");
  await app.exec("history.undo");

  expect((await app.exec("arrow.reroute", { ids: [arrow] })).rerouted, "an arrow that is already on its route is left alone").toEqual([]);
  expect((await app.exec("arrow.set_type", { ids: [arrow], type: "elbow" })).undo_label).toBe("Set arrow type");
  expect((await app.element(arrow)).bindings, "an elbow arrow keeps its bindings").toEqual({ start: a, end: b });
  expectRefusal(await app.refusal("arrow.set_type", { ids: [a], type: "elbow" }), "not_applicable", "arrows and lines");
  expectRefusal(await app.refusal("arrow.set_type", { ids: [arrow], type: "zigzag" as "elbow" }), "bad_args", "zigzag");
  expect((await app.exec("arrow.set_head", { ids: [arrow], end: "start", head: "triangle" })).changed).toBe(true);
  expect((await app.exec("arrow.swap_heads", { ids: [arrow] })).undo_label).toBe("Swap arrowheads");

  const free = (await app.exec("element.create", { kind: "arrow", points: [[0, 300], [200, 300]], bind: "off" })).id;
  expect((await app.exec("arrow.add_point", { arrow: free, segment: 1 })).index).toBe(1);
  expectRefusal(await app.refusal("arrow.add_point", { arrow: free, segment: 9 }), "bad_args", "segments 1 to 2");
  const moved = await app.exec("arrow.move_point", { arrow: free, index: 1, to: [100, 380] });
  expect(moved.point).toEqual([100, 380]);
  expect((await app.element(free)).bounds.h).toBe(80);
  await app.exec("history.undo");
  expect((await app.element(free)).bounds.h, "undo of the point move").toBe(0);
  expect((await app.exec("arrow.delete_point", { arrow: free, index: 1 })).removed, "three points become two").toBe(1);
  expectRefusal(await app.refusal("arrow.delete_point", { arrow: free, index: 0 }), "too_few", "An arrow needs at least 2 points");
  const elbow = await connect(app, a, c);
  await app.exec("arrow.set_type", { ids: [elbow], type: "elbow" });
  expectRefusal(await app.refusal("arrow.add_point", { arrow: elbow, segment: 1 }), "not_movable", "Elbow arrows only allow moving their two ends");
});

test("family diagram and layout: build from structure, lay out, extend; one undo step each", async ({ app }) => {
  const made = await app.exec("diagram.create", {
    nodes: [
      { id: "a", label: "A", x: 0, y: 0 },
      { id: "b", label: "B", x: 0, y: 40 },
      { id: "c", label: "C", x: 0, y: 80 },
    ],
    edges: [
      { from: "a", to: "b" },
      { from: "b", to: "c" },
    ],
    layout: { algorithm: "none" },
  });
  expect(Object.keys(made.nodes as object)).toHaveLength(3);
  const xs = async () => ((await app.exec("element.query", { filter: { kind: "rectangle" }, fields: ["bounds"] })).items as { bounds: { x: number } }[]).map((i) => i.bounds.x);
  expect((await xs()).every((x) => x === (0 as number))).toBe(true);
  await app.exec("selection.all");
  const laid = await app.exec("layout.apply", { algorithm: "layered", direction: "right", node_gap: 40, layer_gap: 80 });
  expect(laid.moved as unknown[], "the first node keeps its place, the other two move").toHaveLength(2);
  expect((await app.exec("element.query", { filter: { kind: "arrow" }, fields: ["bindings"] })).items, "bindings survive the layout").toHaveLength(2);
  const after = await xs();
  expect(new Set(after).size, "three layers, three columns").toBe(3);
  await app.exec("history.undo");
  expect((await xs()).every((x) => x === (0 as number))).toBe(true);
  expectRefusal(await app.refusal("layout.apply", { ids: [Object.values(made.nodes as Record<string, string>)[0] ?? ""] }), "too_few", "needs at least 2 elements");

  const grown = await app.exec("diagram.extend", { from: (made.nodes as Record<string, string>)["c"] ?? "", direction: "down", label: "D" });
  expect(grown.changed).toBe(true);
  expect(await app.count(), "a node, its arrow and the earlier three nodes with two arrows").toBe(7);
  await app.exec("history.undo");
  expectRefusal(await app.refusal("diagram.create", { nodes: [{ id: "a" }, { id: "a" }], layout: {} }), "duplicate_node", "Node ids must be unique");
  const mermaid = await app.exec("diagram.import_mermaid", { source: { text: "graph TD\n A[Start] --> B{Choice}\n B -->|yes| C[Go]" }, at: [0, 600] });
  expect(Object.keys(mermaid.nodes as object)).toHaveLength(3);
  expect(mermaid.groups as unknown[]).toHaveLength(1);
  expectRefusal(await app.refusal("diagram.import_mermaid", { source: { text: "pie\n \"a\": 1" } }), "mermaid_unsupported", "`pie`");
  expect((await undoDepth(app)).label).toContain("Mermaid");
});
