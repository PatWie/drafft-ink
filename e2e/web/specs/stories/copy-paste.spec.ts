import { test, expect } from "../../support/app.ts";
import { box, connect } from "../../support/build.ts";

test("story: copy and paste a bound pair keeps the binding on the copies", async ({ app }) => {
  const a = await box(app, 0, 0, 100, 60);
  const b = await box(app, 300, 0, 100, 60, { kind: "ellipse" });
  const arrow = await connect(app, a, b, "go");

  await app.exec("selection.select", { ids: [a, b, arrow] });
  const copied = await app.exec("clipboard.copy");
  expect(copied.count, "two shapes, an arrow and its label").toBe(4);
  const pasted = await app.exec("clipboard.paste", { source: "internal", at: [0, 500] });
  expect(pasted.kind).toBe("elements");
  expect(pasted.created as string[]).toHaveLength(4);
  expect((await app.exec("selection.get")).count, "the pasted shapes are selected, the label follows its arrow").toBe(3);

  const all = await app.exec("element.query", { fields: ["id", "kind", "bindings"] });
  expect(all.total).toBe(6);
  const created = new Set(pasted.created as string[]);
  const copies = (all.items as { id: string; kind: string; bindings?: { start: string | null; end: string | null } }[]).filter((r) => created.has(r.id));
  const newRect = copies.find((r) => r.kind === "rectangle");
  const newOval = copies.find((r) => r.kind === "ellipse");
  const newArrow = copies.find((r) => r.kind === "arrow");
  expect(newRect && newOval && newArrow, "a rectangle, an ellipse and an arrow were pasted").toBeTruthy();
  expect(new Set([newRect?.id, newOval?.id, newArrow?.id]).size).toBe(3);
  expect([a, b, arrow]).not.toContain(newRect?.id);
  expect(newArrow?.bindings, "the new arrow is bound to the new shapes, not to the originals").toEqual({ start: newRect?.id, end: newOval?.id });

  // Moving the copied ellipse drags only the copied arrow.
  const origArrow = await app.element(arrow);
  await app.exec("element.move", { ids: [newOval?.id ?? ""], dx: 0, dy: 150, snap: "none" });
  expect((await app.element(arrow)).bounds, "the original arrow did not move").toEqual(origArrow.bounds);
  expect((await app.element(newArrow?.id ?? "")).bounds.h, "the copied arrow followed").toBeGreaterThan(50);

  // Undo the move and the paste in two steps; redo brings the pair back.
  await app.exec("history.undo", { steps: 2 });
  expect(await app.count()).toBe(3);
  await app.exec("history.redo");
  expect(await app.count()).toBe(6);

  // A lone copied arrow does not stay bound to anything.
  await app.exec("selection.select", { ids: [arrow] });
  await app.exec("clipboard.copy");
  const lone = await app.exec("clipboard.paste", { source: "internal", at: [0, 900] });
  const rows = await app.exec("element.query", { filter: { ids: lone.created as string[] }, fields: ["id", "kind", "bindings"] });
  const loneRow = (rows.items as { kind: string; bindings?: { start: string | null; end: string | null } }[]).find((r) => r.kind === "arrow");
  expect(loneRow, "the arrow was pasted (with its label)").toBeTruthy();
  expect(loneRow?.bindings).toEqual({ start: null, end: null });

  const empty = await app.refusal("clipboard.paste", { source: { text: "" } });
  expect(empty.code).toBe("clipboard_empty");
  expect(empty.message).toContain("nothing drafft.ink can paste");
});
