import { test, expect, type App } from "../../support/app.ts";
import { box, connect } from "../../support/build.ts";

/** The arrow's row, with the facts the story checks. */
async function arrowRow(app: App, id: string) {
  const row = await app.element(id);
  return { bounds: row.bounds, start: row.bindings?.start ?? null, end: row.bindings?.end ?? null };
}

test("story: an arrow stays attached through move, resize, rotate, delete and undo", async ({ app }) => {
  const rect = await box(app, 0, 0, 100, 60);
  const oval = await box(app, 400, 0, 100, 60, { kind: "ellipse" });
  const arrow = await connect(app, rect, oval);
  const base = await arrowRow(app, arrow);
  expect(base.start, "the arrow starts on the rectangle").toBe(rect);
  expect(base.end, "the arrow ends on the ellipse").toBe(oval);
  expect(base.bounds.w).toBeGreaterThan(250);

  /** Runs `change`, checks the arrow with `after`, undoes once and checks that the arrow is exactly as before. */
  async function step(what: string, change: () => Promise<unknown>, after: (row: Awaited<ReturnType<typeof arrowRow>>) => void) {
    await change();
    after(await arrowRow(app, arrow));
    await app.exec("history.undo");
    const back = await arrowRow(app, arrow);
    expect(back.bounds.x, `${what}: undo restores x`).toBeCloseTo(base.bounds.x, 2);
    expect(back.bounds.y, `${what}: undo restores y`).toBeCloseTo(base.bounds.y, 2);
    expect(back.bounds.w, `${what}: undo restores w`).toBeCloseTo(base.bounds.w, 2);
    expect(back.bounds.h, `${what}: undo restores h`).toBeCloseTo(base.bounds.h, 2);
    expect(back.start, `${what}: undo keeps the start binding`).toBe(rect);
    expect(back.end, `${what}: undo keeps the end binding`).toBe(oval);
  }

  await step(
    "move the rectangle left",
    () => app.exec("element.move", { ids: [rect], dx: -50, dy: 0, snap: "none" }),
    (row) => {
      expect(row.start).toBe(rect);
      expect(row.end).toBe(oval);
      expect(row.bounds.x).toBeCloseTo(base.bounds.x - 50, 0);
      expect(row.bounds.w).toBeCloseTo(base.bounds.w + 50, 0);
    },
  );
  await step(
    "move the ellipse right",
    () => app.exec("element.move", { ids: [oval], dx: 80, dy: 0, snap: "none" }),
    (row) => {
      expect(row.bounds.x).toBeCloseTo(base.bounds.x, 0);
      expect(row.bounds.w).toBeCloseTo(base.bounds.w + 80, 0);
      expect(row.end).toBe(oval);
    },
  );
  await step(
    "resize the rectangle wider",
    () => app.exec("element.resize", { ids: [rect], handle: "e", dx: 60, dy: 0, snap: "none" }),
    (row) => {
      expect(row.bounds.x).toBeCloseTo(base.bounds.x + 60, 0);
      expect(row.bounds.w).toBeCloseTo(base.bounds.w - 60, 0);
    },
  );
  await step(
    "rotate the ellipse by 90 degrees",
    () => app.exec("element.rotate", { ids: [oval], angle: 90 }),
    (row) => {
      expect(row.end, "still bound").toBe(oval);
      expect(row.bounds.h, "the end left the flat line").toBeGreaterThan(20);
      expect(row.bounds.y, "and went up, to the top of the turned ellipse").toBeLessThan(0);
    },
  );
  await step(
    "rotate both shapes together",
    () => app.exec("element.rotate", { ids: [rect, oval], angle: 90 }),
    (row) => {
      expect(row.bounds.w, "the arrow turned rigidly: it is vertical now").toBeCloseTo(0, 0);
      expect(row.bounds.h).toBeCloseTo(base.bounds.w, 0);
    },
  );
  await step(
    "move both shapes",
    () => app.exec("element.move", { ids: [rect, oval], dx: 100, dy: 100, snap: "none" }),
    (row) => {
      expect(row.bounds.x).toBeCloseTo(base.bounds.x + 100, 0);
      expect(row.bounds.y).toBeCloseTo(base.bounds.y + 100, 0);
      expect(row.bounds.w).toBeCloseTo(base.bounds.w, 0);
    },
  );
  await step(
    "move the arrow alone",
    () => app.exec("element.move", { ids: [arrow], dx: 0, dy: 100, snap: "none" }),
    (row) => {
      expect(row.start, "an arrow moved alone comes off the start").toBeNull();
      expect(row.end, "and off the end").toBeNull();
      expect(row.bounds.y).toBeCloseTo(base.bounds.y + 100, 0);
    },
  );

  // Delete the ellipse: the end is freed and the arrow stays where it was. Undo brings the binding back.
  await app.exec("element.delete", { ids: [oval] });
  const freed = await arrowRow(app, arrow);
  expect(freed.start).toBe(rect);
  expect(freed.end, "the end of the arrow is free").toBeNull();
  expect(freed.bounds.w).toBeCloseTo(base.bounds.w, 1);
  await app.exec("history.undo");
  const restored = await arrowRow(app, arrow);
  expect(restored.end, "undo of the delete binds the end again").toBe(oval);
  const ellipses = await app.exec("element.query", { filter: { kind: "ellipse" }, fields: ["id"] });
  expect(ellipses.total).toBe(1);
  expect((ellipses.items[0] as { id: string }).id, "and the ellipse is back under its own id").toBe(oval);
});

test("story: a move shows in the recent ring with origin agent and drags the bound arrow", async ({ app }) => {
  const a = await box(app, 0, 0);
  const b = await box(app, 300, 120);
  const arrow = await connect(app, a, b);
  await app.exec("element.move", { ids: [a], dx: 0, dy: -60, snap: "none" });
  const entries = await app.recent(5);
  expect(entries.at(-1)?.id).toBe("element.move");
  expect(entries.at(-1)?.source, "the web page runs commands with origin agent").toBe("agent");
  const row = await app.element(arrow);
  expect(row.points?.[0]?.[1], "the start went up with the rectangle").toBeLessThan(30);
});
