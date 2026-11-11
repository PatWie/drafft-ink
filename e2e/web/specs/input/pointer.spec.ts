import { test, expect } from "../../support/app.ts";
import { box, camera } from "../../support/build.ts";
import { drag } from "../../support/mouse.ts";
import { PNG } from "pngjs";
import { Shot, type Region } from "../../support/pixels.ts";
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

  test("bold and italic: Mod+B and Mod+I in the editor style the typed text, and the result keeps the runs", async ({ app }) => {
    const spot = await app.toPage({ x: 500, y: 300 });
    await app.pressToolKey(spot, "t", "text");
    await app.page.mouse.click(spot.x, spot.y);
    await app.until("the editor", async () => (await app.state()).text_edit !== undefined && (await app.state()).text_edit !== null);
    const keys = app.page.keyboard;
    await keys.type("plain ", { delay: 20 });
    // A chord is a key event and the text is a text event: the app takes a key at its next frame, so give the
    // chord a frame before the next letter, as a person's fingers do.
    const chord = async (name: string) => {
      await keys.press(name);
      await app.page.waitForTimeout(100);
    };
    await chord("Control+b");
    await keys.type("bold", { delay: 20 });
    await chord("Control+b");
    await keys.type(" ", { delay: 20 });
    await chord("Control+i");
    await keys.type("slanted", { delay: 20 });
    await keys.press("Control+Enter");
    await app.until("the committed text", async () => (await app.exec("element.query", { filter: { kind: "text" } })).total === 1);

    const row = (await app.exec("element.query", { filter: { kind: "text" }, fields: ["id", "text", "bounds", "runs"] })).items[0] as {
      id: string;
      text: string;
      bounds: { w: number };
      runs: { start: number; end: number; bold?: boolean; italic?: boolean }[];
    };
    expect(row.text).toBe("plain bold slanted");
    expect(row.runs, "bold for `bold`, italic for `slanted`, nothing else").toEqual([
      { start: 6, end: 10, bold: true },
      { start: 11, end: 18, italic: true },
    ]);
    expect((await app.exec("history.state")).undo_depth, "typing and styling is one undo step").toBe(1);

    // The box is as wide as the styled letters are: wider than the same words in the regular face.
    const plain = await app.exec("element.create", { kind: "text", text: "plain bold slanted", at: [500, 500] });
    const plainWidth = ((await app.element(plain.id)).bounds as { w: number }).w;
    expect(row.bounds.w, "bold and italic letters are wider than regular ones").toBeGreaterThan(plainWidth + 1);

    // The same styling by command: a range of one text, colours stay, one undo step.
    await app.exec("text.format", { ids: [plain.id], range: [0, 5], bold: true });
    const formatted = (await app.exec("element.query", { filter: { ids: [plain.id] }, fields: ["runs"] })).items[0] as { runs: unknown[] };
    expect(formatted.runs).toEqual([{ start: 0, end: 5, bold: true }]);
    await app.exec("history.undo");
    const undone = (await app.exec("element.query", { filter: { ids: [plain.id] }, fields: ["runs"] })).items[0] as { runs: unknown[] };
    expect(undone.runs).toEqual([]);
  });

  test("the pointer is an I-beam over text and over an editable label, and an arrow elsewhere", async ({ app }) => {
    const cursor = async (at: { x: number; y: number }) => {
      await app.page.mouse.move(at.x - 8, at.y - 8);
      await app.page.mouse.move(at.x, at.y, { steps: 4 });
      await app.settle(300);
      return app.page.evaluate(() => getComputedStyle(document.querySelector("canvas") as HTMLCanvasElement).cursor);
    };
    const made = await app.exec("element.create", { kind: "text", text: "read me", at: [300, 300] });
    const text = await app.element(made.id);
    const inText = await app.toPage({ x: text.bounds.x + text.bounds.w / 2, y: text.bounds.y + text.bounds.h / 2 });
    const shape = await box(app, 700, 300, 160, 100, { fill: "#a5d8ff" });
    const inShape = await app.toPage({ x: 780, y: 350 });
    const empty = await app.toPage({ x: 1000, y: 600 });

    expect(await cursor(inText), "over text").toBe("text");
    expect(await cursor(inShape), "over a shape with the select tool: it moves").toBe("move");
    expect(await cursor(empty), "over nothing: the browser's arrow").toBe("auto");
    const labelled = await box(app, 700, 450, 160, 100, { fill: "#ffd8a8" });
    await app.exec("text.set", { id: labelled, text: "Label" });
    await app.exec("selection.clear");
    expect(await cursor(await app.toPage({ x: 780, y: 500 })), "over the label of a shape").toBe("text");
    expect(await cursor(await app.toPage({ x: 708, y: 458 })), "over the shape beside its label").toBe("move");
    await app.pressToolKey(empty, "t", "text");
    expect(await cursor(inShape), "with the text tool a shape takes a label").toBe("text");
    expect(await cursor(empty), "and empty canvas takes a new text").toBe("text");
    await app.exec("tool.set", { tool: "select" });
    expect(shape).toBeTruthy();
  });

  test("an input method's composition is drawn inline with an underline and commits as text", async ({ app }) => {
    const spot = await app.toPage({ x: 500, y: 300 });
    await app.pressToolKey(spot, "t", "text");
    await app.page.mouse.click(spot.x, spot.y);
    await app.until("the editor", async () => (await app.state()).text_edit !== undefined && (await app.state()).text_edit !== null);
    await app.page.keyboard.type("ab", { delay: 20 });
    const region: Region = { x: spot.x - 4, y: spot.y - 4, w: 260, h: 48 };
    // While a text is edited the caret blinks, so the page never goes quiet: take the pictures straight away.
    const shot = async (clip: Region): Promise<Shot> => {
      await app.page.waitForTimeout(300);
      const buffer = await app.page.screenshot({ clip: { x: clip.x, y: clip.y, width: clip.w, height: clip.h }, animations: "disabled" });
      return new Shot(PNG.sync.read(buffer), clip);
    };
    const before = await shot(region);
    // The text is drawn in the default stroke colour, a blue; the grid and the page are light.
    const ink = (shot: Shot): number => {
      let n = 0;
      for (let y = region.y; y < region.y + region.h; y++) for (let x = region.x; x < region.x + region.w; x++) {
        const [r, , b] = shot.at(x, y);
        if (b > 150 && r < 150) n++;
      }
      return n;
    };
    const inkBefore = ink(before);

    // The browser's input method: a preedit, then its commit (Chrome DevTools `Input.imeSetComposition`).
    const cdp = await app.page.context().newCDPSession(app.page);
    await cdp.send("Input.imeSetComposition", { text: "nihao", selectionStart: 5, selectionEnd: 5 });
    const composing = await shot(region);
    const inkComposing = ink(composing);
    expect(inkComposing, "the preedit is drawn in the text, with its underline").toBeGreaterThan(inkBefore + 60);
    expect(composing.differing(before, 40), "the picture changed where the text is").toBeGreaterThan(60);
    expect((await app.exec("element.query", { filter: { kind: "text" } })).total, "nothing is in the document until the edit ends").toBe(0);

    await cdp.send("Input.insertText", { text: "你好" });
    await app.page.keyboard.press("Control+Enter");
    await app.until("the committed text", async () => (await app.exec("element.query", { filter: { kind: "text" } })).total === 1);
    const row = (await app.exec("element.query", { filter: { kind: "text" }, fields: ["text"] })).items[0] as { text: string };
    expect(row.text, "the committed composition joined the text, the preedit is gone").toBe("ab你好");
    await cdp.detach();
  });
});
