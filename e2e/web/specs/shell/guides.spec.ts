import { test, expect, type App } from "../../support/app.ts";
import { box, camera } from "../../support/build.ts";
import { PNG } from "pngjs";
import { shot, Shot, type Region, type Rgb } from "../../support/pixels.ts";

// With the camera at (640, 400) and zoom 1, a page point is the world point.

/** The smart-guide pink of the overlay (`OverlayStyle::guide`) over the white page. */
const GUIDE_ON_WHITE: Rgb = [241, 126, 183];
/** The fully opaque chip fill: `OverlayStyle::guide` without alpha. */
const CHIP: Rgb = [236, 72, 153];
/** The red of the eraser ring's outline over white (200, 50, 50 at 200/255). */
const ERASER: Rgb = [211, 88, 88];
/** The green of snapped rotation guides. */
const GREEN: Rgb = [34, 197, 94];

async function prepare(app: App): Promise<void> {
  await camera(app, { x: 640, y: 400 }, 1);
  await app.exec("selection.clear");
}

/**
 * A screenshot while a move or rotation is held. `shot` waits for a quiet page, and a gesture that has changed the
 * document keeps the autosave looking again every 500 ms (`GESTURE_RECHECK_MS`), so the page is never quiet then.
 */
async function heldShot(app: App): Promise<Shot> {
  await app.page.waitForTimeout(250);
  const png = PNG.sync.read(await app.page.screenshot({ animations: "disabled" }));
  return new Shot(png, { x: 0, y: 0, w: png.width, h: png.height });
}

async function press(app: App, at: { x: number; y: number }): Promise<void> {
  await app.page.mouse.move(at.x, at.y);
  await app.page.mouse.down();
}

async function moveTo(app: App, from: { x: number; y: number }, to: { x: number; y: number }, steps = 8): Promise<void> {
  for (let i = 1; i <= steps; i++) {
    await app.page.mouse.move(from.x + ((to.x - from.x) * i) / steps, from.y + ((to.y - from.y) * i) / steps);
    await app.page.waitForTimeout(16);
  }
}

test("guides: the eraser shows its radius ring at the pointer while it sweeps and the ring goes at the release", async ({ app }) => {
  await prepare(app);
  await app.exec("tool.set", { tool: "eraser" });
  const ring: Region = { x: 590, y: 350, w: 100, h: 100 };
  expect((await shot(app)).count(ring, ERASER, 40), "no ring before the press").toBe(0);

  await press(app, { x: 500, y: 400 });
  await moveTo(app, { x: 500, y: 400 }, { x: 640, y: 400 });
  const during = (await shot(app)).count(ring, ERASER, 40);
  expect(during, "a 2 px red circle of radius 10 is about 120 pixels").toBeGreaterThan(40);
  // The ring is round and sits on the pointer: nothing at 30 px from it.
  expect((await shot(app)).count({ x: 665, y: 350, w: 25, h: 100 }, ERASER, 40), "and nothing further out").toBe(0);

  await app.page.mouse.up();
  expect((await shot(app)).count(ring, ERASER, 40), "the ring is gone").toBe(0);
});

test("guides: a line end that the grid moved gets a crosshair across the view, and it goes with the drag", async ({ app }) => {
  await prepare(app);
  await app.exec("settings.set", { key: "snap.grid", value: true });
  await app.exec("tool.set", { tool: "line" });
  // The end at (743, 389) lands on the grid point (740, 380).
  await press(app, { x: 500, y: 300 });
  await moveTo(app, { x: 500, y: 300 }, { x: 743, y: 389 });
  const s = await shot(app);
  const across: Region = { x: 100, y: 379, w: 300, h: 3 };
  expect(s.count(across, GUIDE_ON_WHITE, 100), "the horizontal line runs far to the left of the drawing").toBeGreaterThan(200);
  const down: Region = { x: 739, y: 500, w: 3, h: 250 };
  expect(s.count(down, GUIDE_ON_WHITE, 100), "and the vertical one far below it").toBeGreaterThan(150);
  await app.page.mouse.up();
  const after = await shot(app);
  expect(after.count(across, GUIDE_ON_WHITE, 100), "no crosshair after the release").toBe(0);
});

test("guides: Shift while drawing an arrow shows the polar rays: the locked ray and a chip with the angle", async ({ app }) => {
  await prepare(app);
  await app.exec("tool.set", { tool: "arrow" });
  await app.page.keyboard.down("Shift");
  // 3 degrees off the horizontal: the lock takes it to 0.
  await press(app, { x: 500, y: 400 });
  await moveTo(app, { x: 500, y: 400 }, { x: 700, y: 410 });
  const s = await shot(app);
  expect(s.count({ x: 760, y: 398, w: 170, h: 5 }, GUIDE_ON_WHITE, 100), "the locked ray goes on past the arrow").toBeGreaterThan(200);
  expect(s.count({ x: 520, y: 380, w: 90, h: 60 }, CHIP, 25), "the angle sits in an opaque chip beside the arc").toBeGreaterThan(80);
  await app.page.mouse.up();
  await app.page.keyboard.up("Shift");
  const after = await shot(app);
  expect(after.count({ x: 760, y: 398, w: 170, h: 5 }, GUIDE_ON_WHITE, 100), "the rays go with the drag").toBe(0);
});

test("guides: an alignment snap shows the distance to the box it lines up with", async ({ app }) => {
  await prepare(app);
  await app.exec("settings.set", { key: "snap.smart_guides", value: true });
  await box(app, 300, 300, 100, 60, { fill: "#ffffff" });
  await box(app, 700, 400, 100, 60, { fill: "#ffffff" });
  await app.exec("selection.clear");
  // Drag the right box up until its top is 5 units below the other's: it snaps to it, and the gap between them is 300.
  await press(app, { x: 750, y: 430 });
  await moveTo(app, { x: 750, y: 430 }, { x: 790, y: 380 });
  await moveTo(app, { x: 790, y: 380 }, { x: 750, y: 335 });
  const s = await heldShot(app);
  // The chip sits above the guide, in the middle of the gap (x 400 to 700, y 300).
  expect(s.count({ x: 500, y: 262, w: 100, h: 34 }, CHIP, 25), "an opaque chip with the 300 above the guide").toBeGreaterThan(60);
  await app.page.mouse.up();
  const after = await shot(app);
  expect(after.count({ x: 500, y: 262, w: 100, h: 34 }, CHIP, 25), "and none after the release").toBe(0);
});

test("guides: rotating with Shift shows green guides at the centre, and they go at the release", async ({ app }) => {
  await prepare(app);
  const id = await box(app, 540, 340, 200, 120, { fill: "#ffffff" });
  await app.exec("selection.select", { ids: [id] });
  const centre: Region = { x: 625, y: 385, w: 30, h: 30 };
  expect((await shot(app)).count(centre, GREEN, 40)).toBe(0);
  await app.page.keyboard.down("Shift");
  // The rotate handle is 25 px above the top edge.
  await press(app, { x: 640, y: 315 });
  await moveTo(app, { x: 640, y: 315 }, { x: 760, y: 400 });
  const s = await heldShot(app);
  expect(s.count({ x: 580, y: 340, w: 120, h: 120 }, GREEN, 40), "the cross, the indicator line and the arc are green").toBeGreaterThan(60);
  await app.page.mouse.up();
  await app.page.keyboard.up("Shift");
  expect((await shot(app)).count({ x: 580, y: 340, w: 120, h: 120 }, GREEN, 40), "gone after the release").toBe(0);
});
