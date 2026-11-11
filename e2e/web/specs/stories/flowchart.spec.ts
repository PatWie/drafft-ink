import { test, expect } from "../../support/app.ts";

test("story: an agent builds a flowchart with diagram.create and reads it back with doc.outline", async ({ app }) => {
  const made = await app.exec("diagram.create", {
    nodes: [
      { id: "start", label: "Start", shape: "ellipse", name: "start" },
      { id: "check", label: "Valid?", shape: "diamond", name: "check" },
      { id: "ok", label: "Process", name: "ok" },
      { id: "bad", label: "Reject", name: "bad" },
    ],
    edges: [
      { from: "start", to: "check" },
      { from: "check", to: "ok", label: "yes" },
      { from: "check", to: "bad", label: "no" },
    ],
    layout: {},
  });
  expect(Object.keys(made.nodes as object)).toHaveLength(4);
  expect(made.edges as unknown[]).toHaveLength(3);
  const ids = made.nodes as Record<string, string>;

  const outline = await app.exec("doc.outline", { format: "json" });
  const json = outline.outline as {
    nodes: { id: string; kind: string; label?: string }[];
    edges: { from: string; to: string; label?: string }[];
  };
  expect(json.nodes).toHaveLength(4);
  expect(json.edges).toHaveLength(3);
  expect(json.nodes.find((n) => n.id === ids["check"])).toMatchObject({ kind: "diamond", label: "Valid?" });
  expect(json.nodes.find((n) => n.id === ids["start"])?.kind).toBe("ellipse");
  expect(json.edges.map((e) => e.label).filter(Boolean).sort()).toEqual(["no", "yes"]);

  const text = (await app.exec("doc.outline", { format: "text" })).text as string;
  expect(text).toContain('"Valid?" diamond');
  expect(text).toContain("nodes (4):");

  // The layout put the nodes in layers from the top: start above check above process.
  const rows = await app.exec("element.query", { filter: { ids: ["@start", "@check", "@ok", "@bad"] }, fields: ["id", "name", "bounds"] });
  const y = (name: string) => (rows.items as { name: string; bounds: { y: number } }[]).find((r) => r.name === name)?.bounds.y ?? Number.NaN;
  expect(y("start")).toBeLessThan(y("check"));
  expect(y("check")).toBeLessThan(y("ok"));

  // One undo step for the whole diagram.
  expect((await app.exec("history.state")).undo_depth).toBe(1);

  // Edit by meaning: relabel a node, add one and connect it by name, extend from a node.
  await app.exec("element.relabel", { id: "@bad", text: "Reject request" });
  const after = (await app.exec("doc.outline", { format: "json" })).outline as typeof json;
  expect(after.nodes.find((n) => n.id === ids["bad"])?.label).toBe("Reject request");

  await app.exec("element.create", { kind: "rectangle", bounds: { x: 120, y: 480, w: 120, h: 60 }, name: "log" });
  await app.exec("element.relabel", { id: "@log", text: "Write log" });
  const linked = await app.exec("element.connect", { from: "@bad", to: "@log", label: "then" });
  expect(linked.created).toHaveLength(2);
  await app.exec("diagram.extend", { from: "@ok", direction: "down", label: "Done" });
  const grown = (await app.exec("doc.outline", { format: "json" })).outline as typeof json;
  expect(grown.nodes).toHaveLength(6);
  expect(grown.edges).toHaveLength(5);

  const sideways = await app.refusal("diagram.extend", { from: "@start", direction: "sideways" as "down" });
  expect(sideways.code).toBe("bad_args");

  // Undo walks back one step at a time; everything the agent did is in the ring with origin agent.
  await app.exec("history.undo");
  expect(((await app.exec("doc.outline", { format: "json" })).outline as typeof json).nodes).toHaveLength(5);
  // Four edits are left on top of the diagram: relabel, create, relabel, connect.
  await app.exec("history.undo", { steps: 4 });
  const reverted = (await app.exec("doc.outline", { format: "json" })).outline as typeof json;
  expect(reverted.nodes, "back to the four nodes of the diagram").toHaveLength(4);
  expect(reverted.nodes.find((n) => n.id === ids["bad"])?.label, "with the old label").toBe("Reject");
  await app.exec("history.undo");
  const empty = (await app.exec("doc.outline", { format: "json" })).outline as typeof json;
  expect(empty.nodes).toHaveLength(0);
  await app.exec("history.redo");
  expect(((await app.exec("doc.outline", { format: "json" })).outline as typeof json).nodes).toHaveLength(4);
  const ring = await app.recent(50);
  expect(ring.filter((e) => e.id === "diagram.create").every((e) => e.source === "agent")).toBe(true);

  const missing = await app.refusal("diagram.create", { nodes: [{ id: "a" }], edges: [{ from: "a", to: "zz" }], layout: {} });
  expect(missing.code).toBe("unknown_node");
  expect(missing.message).toContain("Edge 0 refers to node `zz`");
  expect(missing.message).toContain("Known nodes: a");
});
