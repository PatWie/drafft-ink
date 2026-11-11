import { test, expect } from "../../support/app.ts";
import { box } from "../../support/build.ts";

test("story: grouping, entering a group to move one member, and leaving it", async ({ app }) => {
  const a = await box(app, 0, 0, 100, 60, { fill: "#ffcc00" });
  const b = await box(app, 200, 0, 100, 60, { kind: "ellipse", fill: "#ffcc00" });
  const c = await box(app, 400, 0, 100, 60, { kind: "diamond", fill: "#ffcc00" });
  const d = await box(app, 600, 0, 100, 60, { fill: "#ffcc00" });

  const tooFew = await app.refusal("group.create", { ids: [c] });
  expect(tooFew.code).toBe("too_few");
  expect(tooFew.message).toContain("at least 2 elements");

  // Two shapes that are not neighbours in z-order: the members become contiguous.
  const made = await app.exec("group.create", { ids: [a, c] });
  const group = made.group as string;
  expect(made.members).toEqual([a, c]);
  const again = await app.refusal("group.create", { ids: [a, c] });
  expect(again.code).toBe("already_grouped");
  expect(again.message).toContain("already form one group at this level");

  const order = (await app.exec("element.query", { fields: ["id", "group_ids"] })).items as { id: string; group_ids: string[] }[];
  expect(order.map((o) => o.id), "members are next to each other, in order").toEqual([b, a, c, d]);
  expect(order.find((o) => o.id === a)?.group_ids).toEqual([group]);

  // A click selects the whole group; entering it lets one member be selected and moved alone.
  await app.exec("selection.clear");
  const clicked = await app.exec("selection.select_at", { point: [50, 30] });
  expect(clicked.selected).toEqual([a, c]);
  expect(clicked.groups).toEqual([group]);
  const deep = await app.exec("element.at", { point: [50, 30], deep: true });
  expect((deep.hits as { id: string }[])[0]?.id).toBe(a);

  const entered = await app.exec("group.enter", { select: c });
  expect(entered.editing_group).toBe(group);
  expect(entered.selected).toEqual([c]);
  await app.exec("element.move", { dx: 0, dy: 100 });
  const moved = await app.elements([a, c]);
  expect(moved[a]?.bounds.y, "the other member stayed").toBe(0);
  expect(moved[c]?.bounds.y, "the entered member moved").toBe(100);

  const left = await app.exec("group.exit");
  expect(left.editing_group).toBeNull();
  const sel = await app.exec("selection.get");
  expect(sel.groups, "the whole group is selected again").toEqual([group]);
  const notIn = await app.refusal("group.exit");
  expect(notIn.code).toBe("not_in_group");
  expect(notIn.message).toContain("not inside a group");

  // Ungroup, then undo the ungroup and the move.
  const named = await app.exec("group.set_name", { name: "pair" });
  expect(named.name).toBe("pair");
  const ungrouped = await app.exec("group.ungroup");
  expect(ungrouped.removed).toEqual([group]);
  const none = await app.refusal("group.ungroup");
  expect(none.code).toBe("no_group");
  expect(none.message).toContain("No group is selected");
  expect((await app.exec("doc.inspect")).groups).toBe(0);
  await app.exec("history.undo");
  expect((await app.exec("doc.inspect")).groups, "undo of ungroup").toBe(1);
  await app.exec("history.undo");
  await app.exec("history.undo");
  expect((await app.element(c)).bounds.y, "undo of the member move").toBe(0);
  await app.exec("history.redo", { steps: 3 });
  expect((await app.element(c)).bounds.y, "redo puts the move back").toBe(100);
});
