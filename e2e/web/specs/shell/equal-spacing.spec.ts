import { test, expect, type App } from "../../support/app.ts";
import { box, camera } from "../../support/build.ts";

// With the camera at (640, 400) and zoom 1, a page point is the world point.

/** Squares of 140 with 140 between them: wide enough that the far square of a pair is outside the 400 unit square of alignment candidates around the dragged one. */
const SIZE = 140;
const STEP = 280;
const START = { x: 300, y: 60 };

type Axis = "x" | "y";

/** Three squares in a row along `axis`, with equal gaps. Returns their ids, first to last. */
async function row(app: App, axis: Axis): Promise<string[]> {
  await camera(app, { x: 640, y: 400 }, 1);
  await app.exec("settings.set", { key: "snap.smart_guides", value: true });
  const ids: string[] = [];
  for (let k = 0; k < 3; k++) {
    const x = axis === "x" ? START.x + k * STEP : START.x;
    const y = axis === "x" ? 300 : START.y + k * STEP;
    ids.push(await box(app, x, y, SIZE, SIZE, { fill: "#ffffff" }));
  }
  await app.exec("selection.clear");
  return ids;
}

/** A real mouse drag of the square `index`: away along the row by `away`, then to `off` units from its place, released there. */
async function dragOut(app: App, axis: Axis, index: number, away: number, off: number): Promise<void> {
  const place = (axis === "x" ? START.x : START.y) + index * STEP;
  const centre = (along: number) => (axis === "x" ? { x: along + SIZE / 2, y: 300 + SIZE / 2 } : { x: START.x + SIZE / 2, y: along + SIZE / 2 });
  const from = centre(place);
  await app.page.mouse.move(from.x, from.y);
  await app.page.mouse.down();
  for (const along of [place + away / 2, place + away, place + (away + off) / 2, place + off]) {
    const at = centre(along);
    await app.page.mouse.move(at.x, at.y, { steps: 4 });
    await app.page.waitForTimeout(16);
  }
  await app.page.mouse.up();
}

for (const axis of ["x", "y"] as const) {
  for (const [index, away, off] of [[2, 60, 6], [0, -60, -6], [1, 50, 5]] as const) {
    const where = axis === "x" ? ["left", "middle", "right"][index] : ["top", "middle", "bottom"][index];
    test(`equal spacing: dragging the ${where} of three squares in a ${axis === "x" ? "row" : "column"} snaps it to the equal gap`, async ({ app }) => {
      const ids = await row(app, axis);
      await dragOut(app, axis, index, away, off);
      const moved = (await app.element(ids[index]!)).bounds;
      const want = (axis === "x" ? START.x : START.y) + index * STEP;
      expect(moved[axis], "the square is back at the equal gap, not 5 or 6 units off").toBe(want);
    });
  }
}
