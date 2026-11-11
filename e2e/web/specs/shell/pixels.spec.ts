import { test, expect } from "../../support/app.ts";
import { box, camera } from "../../support/build.ts";
import { close, hex, shot, type Rgb } from "../../support/pixels.ts";

const WHITE: Rgb = [255, 255, 255];
const TOL = 24;

test.describe("pixels: the screen shows what the commands built", () => {
  test.beforeEach(async ({ app }) => {
    // The world point (640, 400) is the middle of the 1280 x 800 canvas, at 100 %: world and page coordinates match.
    await camera(app, { x: 640, y: 400 }, 1);
  });

  test("a filled rectangle is the right colour inside, background outside, and the background changes with doc.set_background", async ({ app }) => {
    await box(app, 540, 340, 200, 120, { fill: "#ff0000" });
    await app.exec("selection.clear");
    const s = await shot(app);
    expect(close(s.at(640, 400), hex("#ff0000"), TOL), `centre is ${s.at(640, 400)}`).toBe(true);
    expect(s.share({ x: 560, y: 360, w: 160, h: 80 }, hex("#ff0000"), TOL), "the inside is red").toBeGreaterThan(0.95);
    expect(s.share({ x: 800, y: 500, w: 200, h: 150 }, WHITE, TOL), "away from the shape the page is white").toBeGreaterThan(0.95);
    expect(s.share({ x: 100, y: 600, w: 200, h: 100 }, WHITE, TOL), "and white elsewhere").toBeGreaterThan(0.95);

    await app.exec("doc.set_background", { color: "#202020" });
    const dark = await shot(app);
    // The grid lines are lighter than a dark page and take about one pixel in ten of a region.
    expect(dark.share({ x: 800, y: 500, w: 200, h: 150 }, hex("#202020"), TOL), "the background colour is drawn").toBeGreaterThan(0.85);
    expect(close(dark.at(805, 505), hex("#202020"), TOL) || close(dark.at(810, 515), hex("#202020"), TOL), "and cells of the grid hold it").toBe(true);
    expect(close(dark.at(640, 400), hex("#ff0000"), TOL), "the shape is unchanged").toBe(true);
    await app.exec("history.undo");
    expect((await shot(app)).share({ x: 800, y: 500, w: 200, h: 150 }, WHITE, TOL), "undo brings the white back").toBeGreaterThan(0.95);
  });

  test("an ellipse is filled at its centre and empty at the corners of its box", async ({ app }) => {
    await box(app, 540, 340, 200, 120, { kind: "ellipse", fill: "#00aa00" });
    await app.exec("selection.clear");
    const s = await shot(app);
    expect(close(s.at(640, 400), hex("#00aa00"), TOL), `centre is ${s.at(640, 400)}`).toBe(true);
    for (const [x, y] of [
      [548, 348],
      [732, 348],
      [548, 452],
      [732, 452],
    ] as const) {
      expect(close(s.at(x, y), WHITE, TOL), `corner (${x}, ${y}) is ${s.at(x, y)}`).toBe(true);
    }
  });

  test("deleting a shape clears its pixels and undo draws them again", async ({ app }) => {
    const id = await box(app, 540, 340, 200, 120, { fill: "#3366ff" });
    await app.exec("selection.clear");
    const region = { x: 560, y: 360, w: 160, h: 80 };
    expect((await shot(app)).share(region, hex("#3366ff"), TOL)).toBeGreaterThan(0.95);
    await app.exec("element.delete", { ids: [id] });
    expect((await shot(app)).share(region, WHITE, TOL), "gone").toBeGreaterThan(0.95);
    await app.exec("history.undo");
    expect((await shot(app)).share(region, hex("#3366ff"), TOL), "back").toBeGreaterThan(0.95);
  });

  test("zooming in grows the shape on screen by the zoom factor", async ({ app }) => {
    await box(app, 590, 370, 100, 60, { fill: "#ff0000" });
    await app.exec("selection.clear");
    const small = await shot(app);
    const red = (s: typeof small) => s.count({ x: 300, y: 200, w: 680, h: 400 }, hex("#ff0000"), TOL);
    const before = red(small);
    await app.exec("view.zoom_at", { factor: 2, at: "viewport_center" });
    const big = red(await shot(app));
    expect(before, "the shape has pixels at 100 %").toBeGreaterThan(4000);
    expect(big / before, "twice the zoom is about four times the area").toBeGreaterThan(3.4);
    expect(big / before).toBeLessThan(4.6);
  });

  test("a shape drawn with the mouse shows in its colour where the drag was", async ({ app }) => {
    await app.exec("tool.set", { tool: "rectangle" });
    const from = await app.toPage({ x: 500, y: 300 });
    const to = await app.toPage({ x: 700, y: 420 });
    await app.page.mouse.move(from.x, from.y);
    await app.page.mouse.down();
    for (let i = 1; i <= 12; i++) await app.page.mouse.move(from.x + ((to.x - from.x) * i) / 12, from.y + ((to.y - from.y) * i) / 12);
    await app.page.mouse.up();
    const id = (await app.exec("element.query", { fields: ["id"] })).items[0] as { id: string };
    await app.exec("style.set", { ids: [id.id], props: { fill_color: "#ff00ff", fill_pattern: "solid" } });
    await app.exec("selection.clear");
    const s = await shot(app);
    expect(s.share({ x: 520, y: 320, w: 160, h: 80 }, hex("#ff00ff"), TOL)).toBeGreaterThan(0.95);
  });
  test("dark theme: the colour a pick of White stores draws a white line on screen, and the document keeps the stored colour", async ({ app }) => {
    // What the colour popover stores for its White cell in the dark theme: the inverse of the display mapping of
    // white (`Color::WHITE.from_dark_theme()`, pinned by a test in drafftink-scene and by the ui test
    // `in_the_dark_theme_a_palette_pick_stores_...`). The canvas draws it through the same mapping.
    const STORED = "#010001";
    await app.exec("settings.set", { key: "theme", value: "dark" });
    await app.settle();
    await app.exec("tool.set", { tool: "line" });
    const from = await app.toPage({ x: 500, y: 400 });
    const to = await app.toPage({ x: 700, y: 400 });
    await app.page.mouse.move(from.x, from.y);
    await app.page.mouse.down();
    for (let i = 1; i <= 12; i++) await app.page.mouse.move(from.x + ((to.x - from.x) * i) / 12, from.y);
    await app.page.mouse.up();
    const id = (await app.exec("element.query", { fields: ["id"] })).items[0] as { id: string };
    await app.exec("style.set", { ids: [id.id], props: { stroke_color: STORED, stroke_width: 8, sloppiness: "architect" } });
    await app.exec("selection.clear");
    await app.settle();
    const onDark = await shot(app);
    expect(onDark.share({ x: 800, y: 500, w: 200, h: 150 }, hex("#121212"), TOL), "the page is drawn dark in the dark theme").toBeGreaterThan(0.85);
    const line = onDark.at(600, 400);
    expect(close(line, [237, 237, 237], TOL), `the line is white on screen: ${line}`).toBe(true);
    const held = await app.exec("style.get", { ids: [id.id], props: ["stroke_color"] });
    expect(held.props.stroke_color?.value, "the document holds the stored colour").toBe(STORED);

    await app.exec("settings.set", { key: "theme", value: "light" });
    await app.settle();
    const onLight = await shot(app);
    expect(close(onLight.at(600, 400), [0, 0, 0], TOL), `and the same element is black in the light theme: ${onLight.at(600, 400)}`).toBe(true);
  });
});
