/**
 * An iPad: fingers, the Apple Pencil, the visible viewport. Chromium emulates the device (touch events, a mobile
 * viewport, the user agent), so this checks the page and the app's input path; Safari itself (the toolbar that
 * `100vh` ignores, the home indicator, Scribble) is only checked by hand on a real iPad, see docs/web-app.md,
 * "Touch devices".
 */
import { devices } from "@playwright/test";
import { test, expect, probe } from "../../support/app.ts";
import { box, camera } from "../../support/build.ts";
import { Hands } from "../../support/touch.ts";

// The browser of the project stays Chromium: only the device's screen, pixel ratio, user agent and touch flags apply.
const { defaultBrowserType: _browser, ...ipad } = devices["iPad Pro 11"];
test.use(ipad);

test.describe("an iPad", () => {
  test.beforeEach(async ({ app }) => {
    await camera(app, { x: 0, y: 0 }, 1);
  });

  test("the canvas is exactly the visible page and the page never scrolls", async ({ app }) => {
    const [info, page] = await Promise.all([
      probe(app.page, "canvasInfo"),
      app.page.evaluate(() => ({
        w: window.visualViewport?.width ?? window.innerWidth,
        h: window.visualViewport?.height ?? window.innerHeight,
        coarse: matchMedia("(pointer: coarse)").matches,
        position: getComputedStyle(document.getElementById("canvas") as HTMLElement).position,
      })),
    ]);
    expect(page.coarse, "the emulated device has a coarse pointer, so the app starts in its touch layout").toBe(true);
    expect(page.position, "pinned to the visible viewport, not sized with 100vh").toBe("fixed");
    expect([info.left, info.top]).toEqual([0, 0]);
    expect(info.cssWidth).toBeCloseTo(page.w, 0);
    expect(info.cssHeight, "the bottom of the canvas (and the bottom bar) is on the screen").toBeCloseTo(page.h, 0);

    const hands = await Hands.on(app.page);
    await app.exec("tool.set", { tool: "rectangle" });
    await hands.drag({ x: 300, y: 300 }, { x: 500, y: 420 });
    const scrolled = await app.page.evaluate(() => [window.scrollX, window.scrollY, document.scrollingElement?.scrollTop ?? 0]);
    expect(scrolled, "dragging on the canvas does not move the page").toEqual([0, 0, 0]);
  });

  test("a one-finger drag draws with the active tool", async ({ app }) => {
    const hands = await Hands.on(app.page);
    await app.exec("tool.set", { tool: "rectangle" });
    await hands.drag({ x: 300, y: 300 }, { x: 520, y: 420 });
    await app.until("the rectangle", async () => (await app.count()) === 1);
    const row = (await app.exec("element.query", { fields: ["kind", "bounds"] })).items[0] as { kind: string; bounds: { w: number; h: number } };
    expect(row.kind).toBe("rectangle");
    expect(row.bounds.w).toBeGreaterThan(150);
    expect(row.bounds.h).toBeGreaterThan(80);
  });

  test("a tap on an element selects it and a drag with the select tool moves it", async ({ app }) => {
    const id = await box(app, -100, -60, 200, 120, { fill: "#ffd8a8" });
    const centre = await app.toPage({ x: 0, y: 0 });
    const hands = await Hands.on(app.page);
    await hands.tap(centre);
    await app.until("the selection", async () => (await app.exec("selection.get")).count === 1);
    const before = await app.element(id);
    await hands.drag(centre, { x: centre.x + 80, y: centre.y + 50 });
    await app.until("the move", async () => (await app.element(id)).bounds.x > before.bounds.x + 40);
  });

  test("the Apple Pencil draws a stroke", async ({ app }) => {
    const hands = await Hands.on(app.page);
    await app.exec("tool.set", { tool: "freehand" });
    const at = (i: number) => ({ x: 300 + i * 30, y: 400 + 25 * Math.sin(i / 2) });
    await hands.stroke(Array.from({ length: 12 }, (_, i) => ({ at: at(i), pressure: 0.2 + 0.07 * i })));
    await app.until("the stroke", async () => (await app.count()) === 1);
    const row = (await app.exec("element.query", { fields: ["kind", "points"] })).items[0] as { kind: string; points: unknown[] };
    expect(row.kind).toBe("freehand");
    expect(row.points.length).toBeGreaterThan(5);
  });

  test("two fingers pinch and pan the view and draw nothing", async ({ app }) => {
    const hands = await Hands.on(app.page);
    await app.exec("tool.set", { tool: "freehand" });
    const before = await app.exec("view.get");
    await hands.pinch({ x: 500, y: 500 }, { x: 600, y: 500 }, { x: 440, y: 500 }, { x: 700, y: 520 });
    await app.until("the zoom", async () => ((await app.exec("view.get")).zoom as number) > (before.zoom as number) * 1.5);
    expect(await app.count(), "no stroke from the fingers").toBe(0);
    // And the next finger works again (the recognizer is not left waiting for a lifted finger).
    await hands.drag({ x: 300, y: 300 }, { x: 420, y: 340 });
    await app.until("a stroke", async () => (await app.count()) === 1);
  });

  test("a finger next to the pen only pans (palm rejection); alone it draws again later", async ({ app }) => {
    const hands = await Hands.on(app.page);
    await app.exec("tool.set", { tool: "freehand" });
    const before = await app.exec("view.get");
    await hands.hover({ x: 700, y: 300 });
    await hands.drag({ x: 400, y: 600 }, { x: 330, y: 560 });
    await app.page.waitForTimeout(300);
    expect(await app.count(), "the palm leaves no mark").toBe(0);
    expect((await app.exec("view.get")).scroll, "it moved the view").not.toEqual(before.scroll);

    await app.page.waitForTimeout(2_500);
    await hands.drag({ x: 300, y: 300 }, { x: 420, y: 340 });
    await app.until("the finger's stroke", async () => (await app.count()) === 1);
  });
});
