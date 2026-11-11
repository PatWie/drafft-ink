import { test, expect } from "../../support/app.ts";
import { box, camera } from "../../support/build.ts";
import { drag } from "../../support/mouse.ts";
import { seedPrimarySelection } from "../../support/x11.ts";

test.describe("real pointer input", () => {
  test.beforeEach(async ({ app }) => {
    await camera(app, { x: 640, y: 400 }, 1);
  });

  test("a mouse drag with the rectangle tool draws a rectangle, one undo step, origin ui", async ({ app }) => {
    const from = await app.toPage({ x: 500, y: 300 });
    const to = await app.toPage({ x: 760, y: 450 });
    await app.pressToolKey(from, "r", "rectangle");
    await drag(app.page, from, to);
    await app.settle();
    expect(await app.count()).toBe(1);
    const rows = (await app.exec("element.query", { fields: ["id", "kind", "bounds"] })).items as { kind: string; bounds: { x: number; y: number; w: number; h: number } }[];
    const r = rows[0];
    expect(r?.kind).toBe("rectangle");
    expect(r?.bounds.x).toBeCloseTo(500, -1);
    expect(r?.bounds.y).toBeCloseTo(300, -1);
    expect(r?.bounds.w).toBeCloseTo(260, -1);
    expect(r?.bounds.h).toBeCloseTo(150, -1);
    const ring = await app.recent(6);
    expect(ring.some((e) => e.id === "element.create" && e.source === "ui"), "the drag is a ui element.create in the ring").toBe(true);
    expect((await app.exec("history.state")).undo_depth, "the whole drag is one undo step").toBe(1);
    await app.exec("history.undo");
    expect(await app.count()).toBe(0);
  });

  test("dragging an arrow from one shape onto another binds both ends", async ({ app }) => {
    // Filled, so that a click inside grabs the shape (an empty shape is grabbed by its outline).
    const left = await box(app, 400, 350, 120, 80, { fill: "#ffd8a8" });
    const right = await box(app, 800, 350, 120, 80, { fill: "#a5d8ff" });
    const from = await app.toPage({ x: 460, y: 390 });
    const to = await app.toPage({ x: 860, y: 390 });
    await app.pressToolKey(from, "a", "arrow");
    await drag(app.page, from, to);
    await app.settle();
    const arrows = (await app.exec("element.query", { filter: { kind: "arrow" }, fields: ["id", "bindings"] })).items as { id: string; bindings: { start: string | null; end: string | null } }[];
    expect(arrows).toHaveLength(1);
    expect(arrows[0]?.bindings, "the arrow is bound to the shape under each end").toEqual({ start: left, end: right });

    // Moving the right shape with the mouse drags the arrow's end along.
    await app.exec("tool.set", { tool: "select" });
    const before = await app.element(arrows[0]?.id ?? "");
    const grab = await app.toPage({ x: 880, y: 410 });
    await drag(app.page, grab, { x: grab.x, y: grab.y + 120 });
    await app.settle();
    const after = await app.element(arrows[0]?.id ?? "");
    expect((await app.element(right)).bounds.y, "the shape moved down").toBeGreaterThan(450);
    expect(after.bindings, "the binding survived the drag").toEqual({ start: left, end: right });
    expect(after.bounds.h, "and the arrow bent down with it").toBeGreaterThan(before.bounds.h + 60);
  });

  test("a marquee drag on empty canvas selects the shapes it covers", async ({ app }) => {
    const a = await box(app, 300, 300, 100, 60);
    const b = await box(app, 450, 300, 100, 60);
    const far = await box(app, 900, 600, 100, 60, { fill: "#a5d8ff" });
    await app.exec("selection.clear");
    await app.exec("tool.set", { tool: "select" });
    const from = await app.toPage({ x: 250, y: 250 });
    const to = await app.toPage({ x: 620, y: 420 });
    await drag(app.page, from, to);
    await app.settle();
    const sel = await app.exec("selection.get");
    expect([...(sel.ids as string[])].sort()).toEqual([a, b].sort());
    expect(sel.ids as string[]).not.toContain(far);
    // A click on a shape selects just that shape.
    const click = await app.toPage({ x: 950, y: 630 });
    await app.page.mouse.click(click.x, click.y);
    await app.settle();
    expect((await app.exec("selection.get")).ids).toEqual([far]);
    expect((await app.exec("history.state")).undo_depth, "selecting adds no undo step").toBe(3);
  });

  test("the wheel pans and ctrl+wheel zooms about the pointer", async ({ app }) => {
    await box(app, 600, 380, 80, 40);
    const centre = await app.toPage({ x: 640, y: 400 });
    await app.page.mouse.move(centre.x, centre.y);
    const v0 = await app.exec("view.get");
    const s0 = v0.scroll as [number, number];

    await app.page.mouse.wheel(0, 200);
    await app.settle();
    const s1 = (await app.exec("view.get")).scroll as [number, number];
    expect(s1[1], "wheel down scrolls the content up: the camera moves down").toBeGreaterThan(s0[1]);
    expect(s1[0]).toBeCloseTo(s0[0], 3);

    const world = await app.toWorld(centre);
    await app.page.keyboard.down("Control");
    await app.page.mouse.wheel(0, -300);
    await app.page.keyboard.up("Control");
    await app.settle();
    const v2 = await app.exec("view.get");
    expect(v2.zoom as number, "ctrl+wheel up zooms in").toBeGreaterThan(v0.zoom as number);
    const worldAfter = await app.toWorld(centre);
    expect(worldAfter.x, "the point under the pointer stays put (x)").toBeCloseTo(world.x, 0);
    expect(worldAfter.y, "the point under the pointer stays put (y)").toBeCloseTo(world.y, 0);

    expect((await app.exec("history.state")).undo_depth, "camera moves are not undo steps").toBe(1);
  });

  test("a middle-button drag pans the content and pastes nothing", async ({ app }) => {
    // Linux browsers paste the primary selection on a middle click. Give them something to paste (F4).
    const seeded = await seedPrimarySelection("text from the primary selection");
    if (seeded) await app.page.waitForTimeout(200);
    await box(app, 600, 380, 80, 40);
    const centre = await app.toPage({ x: 640, y: 400 });
    const s2 = (await app.exec("view.get")).scroll as [number, number];
    await app.page.mouse.move(centre.x, centre.y);
    await app.page.mouse.down({ button: "middle" });
    for (let i = 1; i <= 10; i++) await app.page.mouse.move(centre.x + i * 10, centre.y);
    await app.page.mouse.up({ button: "middle" });
    await app.settle();
    const s3 = (await app.exec("view.get")).scroll as [number, number];
    expect(s3[0], "dragging the content right moves the camera left").toBeLessThan(s2[0]);
    const pastes = (await app.recent(30)).filter((e) => e.id === "clipboard.paste");
    expect(pastes, "a pan is not a paste").toHaveLength(0);
    expect((await app.exec("history.state")).undo_depth, "camera moves are not undo steps").toBe(1);
  });

  test("typing text: the text tool, a click, the keyboard and Ctrl+Enter make one text element", async ({ app }) => {
    const spot = await app.toPage({ x: 500, y: 300 });
    await app.pressToolKey(spot, "t", "text");
    await app.page.mouse.click(spot.x, spot.y);
    await app.until("the editor", async () => (await app.state()).text_edit !== undefined && (await app.state()).text_edit !== null);
    await app.page.keyboard.type("hello canvas", { delay: 20 });
    await app.page.keyboard.press("Control+Enter");
    await app.until("the committed text", async () => (await app.exec("element.query", { filter: { kind: "text" } })).total === 1);
    const row = (await app.exec("element.query", { filter: { kind: "text" }, fields: ["text", "bounds"] })).items[0] as { text: string; bounds: { x: number; y: number } };
    expect(row.text).toBe("hello canvas");
    expect(row.bounds.x).toBeCloseTo(500, -1);
    expect(row.bounds.y).toBeCloseTo(300, -1);
    await app.exec("history.undo");
    expect(await app.count(), "typing and committing is one undo step").toBe(0);
  });
});
