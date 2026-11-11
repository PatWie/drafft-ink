import { readFileSync } from "node:fs";
import path from "node:path";
import { test, expect, fixtureBase64 } from "../../support/app.ts";
import { FIXTURES } from "../../support/env.ts";

/** What the old app's code computed about its own fixture (tests/fixtures/legacy/*.facts.json). */
interface Facts {
  doc: { counts_total: Record<string, number> };
}

test("story: the old app's all_shapes.json loads as a clean document with the facts' element counts", async ({ app }) => {
  const facts = JSON.parse(readFileSync(path.join(FIXTURES, "legacy/all_shapes.facts.json"), "utf8")) as Facts;
  const opened = await app.exec("doc.open", { source: { base64: fixtureBase64("legacy/all_shapes.json"), name: "all_shapes.json" } });
  expect(opened.format).toBe("drafftink");
  expect(opened.mode).toBe("replace");
  expect(opened.warnings).toEqual([]);

  const inspect = await app.exec("doc.inspect");
  const kinds = inspect.kinds as Record<string, number>;
  // The old code counted its group as a shape; the new model keeps groups as ids.
  const want = Object.fromEntries(Object.entries(facts.doc.counts_total).map(([kind, n]) => [kind.toLowerCase(), n]));
  const shapes = Object.entries(want).filter(([kind]) => kind !== "group");
  expect(inspect.elements, "elements by the facts file").toBe(shapes.reduce((n, [, v]) => n + v, 0));
  for (const [kind, n] of shapes) expect(kinds[kind] ?? 0, `${kind} count`).toBe(n);
  expect(inspect.groups, "groups").toBe(want["group"] ?? 0);
  expect(inspect.dirty, "a freshly opened document is clean").toBe(false);
  expect((await app.exec("history.state")).can_undo, "and has no history").toBe(false);
  const undo = await app.refusal("history.undo");
  expect(undo.code).toBe("nothing_to_undo");
});
