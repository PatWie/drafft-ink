import type { Download } from "@playwright/test";
import { test, expect, fixtureBase64, probe, type App } from "../../support/app.ts";
import { box, camera } from "../../support/build.ts";
import { solidPng } from "../../support/png.ts";
import { close, hex, shot } from "../../support/pixels.ts";

test("drop: a PNG dropped on the canvas becomes one image element, one undo step", async ({ app }) => {
  await camera(app, { x: 640, y: 400 }, 1);
  const at = await app.toPage({ x: 640, y: 400 });
  await probe(app.page, "dropFiles", [{ name: "red.png", mime: "image/png", base64: solidPng(120, 80, [255, 0, 0]) }], at.x, at.y);
  await app.until("the image element", async () => (await app.exec("element.query", { filter: { kind: "image" } })).total === 1);
  const image = (await app.exec("element.query", { filter: { kind: "image" }, fields: ["bounds"] })).items[0] as { bounds: { x: number; y: number; w: number; h: number } };
  expect(image.bounds.w).toBe(120);
  expect(image.bounds.h).toBe(80);
  expect(image.bounds.x + image.bounds.w / 2, "centred on the drop point").toBeCloseTo(640, -1);
  const ring = await app.recent(10);
  expect(ring.some((e) => e.id === "doc.drop"), "the shell's drop handler ran doc.drop").toBe(true);

  await app.exec("selection.clear");
  const s = await shot(app);
  expect(close(s.at(640, 400), hex("#ff0000"), 24), `the picture is on screen: ${s.at(640, 400)}`).toBe(true);

  expect((await app.exec("history.state")).undo_depth).toBe(1);
  await app.exec("history.undo");
  expect(await app.count()).toBe(0);
});

test("drop: a document dropped on the canvas opens in a new tab", async ({ app }) => {
  await box(app, 0, 0, 50, 50);
  await probe(app.page, "dropFiles", [{ name: "all_shapes.json", mime: "application/json", base64: fixtureBase64("legacy/all_shapes.json") }], 640, 400);
  await app.until("a second tab", async () => ((await app.exec("tab.list")).tabs as unknown[]).length === 2);
  expect((await app.exec("doc.inspect")).elements, "the dropped document is the active tab").toBe(11);
  const tabs = (await app.exec("tab.list")).tabs as { active: boolean }[];
  expect(tabs.filter((t) => t.active)).toHaveLength(1);
});

test("drop: something that is not an image or a document is refused in words and nothing changes", async ({ app }) => {
  await probe(app.page, "dropFiles", [{ name: "notes.txt", mime: "text/plain", base64: Buffer.from("hello").toString("base64") }], 640, 400);
  await app.page.waitForTimeout(800);
  expect(await app.count()).toBe(0);
  expect(((await app.exec("tab.list")).tabs as unknown[]).length).toBe(1);
  // doc.drop is for the shell's drop handler, not for agents.
  const refusal = await app.refusal("doc.drop", { items: [{ name: "notes.txt", bytes: { base64: Buffer.from("hello").toString("base64") } }] });
  expect(refusal.code).toBe("not_for_agents");
});

test("paste: text pasted into the page becomes a text element, and Mermaid text becomes a diagram", async ({ app }) => {
  await probe(app.page, "pasteText", "plain words");
  await app.until("the text element", async () => (await app.exec("element.query", { filter: { kind: "text" }, fields: ["text"] })).total === 1);
  expect(((await app.exec("element.query", { filter: { kind: "text" }, fields: ["text"] })).items[0] as { text: string }).text).toBe("plain words");

  await probe(app.page, "pasteText", "graph TD\n A[Start] --> B{Choice}\n B -->|yes| C[Go]");
  await app.until("the diagram", async () => (await app.exec("element.query", { filter: { kind: "arrow" } })).total === 2);
  const outline = (await app.exec("doc.outline", { format: "json" })).outline as { nodes: unknown[]; edges: unknown[] };
  expect(outline.edges).toHaveLength(2);
  expect(outline.nodes, "A, B and C").toHaveLength(3);
  await app.exec("history.undo");
  expect(await app.exec("element.query", { filter: { kind: "arrow" } }).then((r) => r.total), "the diagram is one undo step").toBe(0);
});

test("keys: shortcuts run the same commands (origin shortcut), and undo, delete, select-all and group work from the keyboard", async ({ app }) => {
  await camera(app, { x: 640, y: 400 }, 1);
  const a = await box(app, 300, 300, 100, 60, { fill: "#ffd8a8" });
  const b = await box(app, 500, 300, 100, 60, { fill: "#a5d8ff" });
  const centre = await app.toPage({ x: 640, y: 600 });
  await app.page.mouse.move(centre.x, centre.y);

  await app.page.keyboard.press("Control+a");
  await app.until("both selected", async () => ((await app.exec("selection.get")).ids as string[]).length === 2);
  await app.page.keyboard.press("Control+g");
  await app.until("a group", async () => (await app.exec("doc.inspect")).groups === 1);
  await app.page.keyboard.press("Control+Shift+g");
  await app.until("no group", async () => (await app.exec("doc.inspect")).groups === 0);
  await app.page.keyboard.press("Control+z");
  await app.until("group back after undo", async () => (await app.exec("doc.inspect")).groups === 1);
  await app.page.keyboard.press("Control+Shift+z");
  await app.until("no group after redo", async () => (await app.exec("doc.inspect")).groups === 0);

  await app.exec("selection.select", { ids: [a] });
  await app.page.keyboard.press("Delete");
  await app.until("a deleted", async () => (await app.count()) === 1);
  expect((await app.elements([b]))[b]).toBeTruthy();
  await app.page.keyboard.press("Control+z");
  await app.until("a back", async () => (await app.count()) === 2);

  const ring = await app.recent(30);
  const fromKeys = ring.filter((e) => e.source === "shortcut").map((e) => e.id);
  for (const id of ["selection.all", "group.create", "group.ungroup", "history.undo", "history.redo", "element.delete"]) {
    expect(fromKeys, `${id} came from a shortcut`).toContain(id);
  }
});

test("keys: Ctrl+C and Ctrl+V paste a copy of the selected shape", async ({ app }) => {
  await camera(app, { x: 640, y: 400 }, 1);
  const a = await box(app, 300, 300, 100, 60, { fill: "#ffd8a8", name: "orig" });
  await app.exec("selection.select", { ids: [a] });
  await app.page.keyboard.press("Control+c");
  // The browser writes the system clipboard asynchronously; a person takes longer than a millisecond to press V.
  await app.page.waitForTimeout(300);
  await app.page.keyboard.press("Control+v");
  await app.until("a pasted copy", async () => (await app.count()) >= 2);
  await app.settle(800);
  const rows = (await app.exec("element.query", { fields: ["id", "bounds", "name"] })).items as { id: string; bounds: { w: number; h: number }; name?: string }[];
  const copies = rows.filter((r) => r.id !== a);
  expect(copies, "one Ctrl+V makes one copy").toHaveLength(1);
  for (const c of copies) expect(c.bounds, "a copy has the size of the original").toMatchObject({ w: 100, h: 60 });
  const selected = (await app.exec("selection.get")).ids as string[];
  expect(selected.length).toBeGreaterThan(0);
  expect(selected.every((id) => copies.some((c) => c.id === id)), "the selection is the pasted copy").toBe(true);
  await app.exec("history.undo");
  expect(await app.count(), "one undo removes the copy").toBe(1);
});

test("keys: one Ctrl+C copies once", async ({ app }) => {
  await camera(app, { x: 640, y: 400 }, 1);
  const a = await box(app, 300, 300, 100, 60, { fill: "#ffd8a8" });
  await app.exec("selection.select", { ids: [a] });
  await app.page.keyboard.press("Control+c");
  await app.settle(600);
  const copies = (await app.recent(30)).filter((e) => e.id === "clipboard.copy" && e.source === "shortcut");
  expect(copies, "one Ctrl+C is one clipboard.copy").toHaveLength(1);
});

test("keys: one Ctrl+V pastes once", async ({ app }) => {
  await camera(app, { x: 640, y: 400 }, 1);
  const a = await box(app, 300, 300, 100, 60, { fill: "#ffd8a8" });
  await app.exec("selection.select", { ids: [a] });
  await app.page.keyboard.press("Control+c");
  await app.page.waitForTimeout(300);
  await app.page.keyboard.press("Control+v");
  await app.settle(1000);
  expect(await app.count(), "one original and one copy").toBe(2);
  const pastes = (await app.recent(30)).filter((e) => e.id === "clipboard.paste" && e.source === "shortcut");
  expect(pastes, "one Ctrl+V is one clipboard.paste").toHaveLength(1);
});

test("keys: Ctrl+V pastes the copy centred on the pointer", async ({ app }) => {
  await camera(app, { x: 640, y: 400 }, 1);
  const a = await box(app, 300, 300, 100, 60, { fill: "#ffd8a8", name: "orig" });
  await app.exec("selection.select", { ids: [a] });
  const where = await app.toPage({ x: 700, y: 520 });
  await app.page.mouse.move(where.x - 1, where.y);
  await app.page.waitForTimeout(250);
  await app.page.mouse.move(where.x, where.y);
  await app.page.waitForTimeout(250);
  await app.page.keyboard.press("Control+c");
  await app.page.keyboard.press("Control+v");
  await app.until("a pasted copy", async () => (await app.count()) === 2);
  const rows = (await app.exec("element.query", { fields: ["id", "bounds"] })).items as { id: string; bounds: { x: number; y: number } }[];
  const copy = rows.find((r) => r.id !== a);
  expect(copy?.bounds.x, "centred on the pointer (x)").toBeCloseTo(700 - 50, -1);
  expect(copy?.bounds.y, "centred on the pointer (y)").toBeCloseTo(520 - 30, -1);
});

test("keys: Ctrl+V with the pointer over a panel pastes at the middle of the view", async ({ app }) => {
  // The ribbon card hangs under the top strip when something is selected. A pointer over it is not on the
  // canvas, so `$cursor` is omitted and the paste lands at the viewport centre (docs/commands.md section 6.1).
  // The first version of the pointer spec above hovered a panel and read the result as a lagging cursor (F2).
  await camera(app, { x: 640, y: 400 }, 1);
  const a = await box(app, 300, 300, 100, 60, { fill: "#ffd8a8" });
  await app.exec("selection.select", { ids: [a] });
  await app.settle();
  // The card is centred under the 40 point strip: (640, 68) is inside it for a rectangle (about 270 to 1010 wide,
  // 46 to 90 high at 1280 x 800). Page points, not world points: the card does not move with the camera.
  const over = { x: 640, y: 68 };
  await app.page.mouse.move(over.x - 1, over.y);
  await app.page.waitForTimeout(250);
  await app.page.mouse.move(over.x, over.y);
  await app.page.waitForTimeout(250);
  await app.page.keyboard.press("Control+c");
  await app.page.keyboard.press("Control+v");
  await app.until("a pasted copy", async () => (await app.count()) === 2);
  const rows = (await app.exec("element.query", { fields: ["id", "bounds"] })).items as { id: string; bounds: { x: number; y: number } }[];
  const copy = rows.find((r) => r.id !== a);
  expect(copy?.bounds.x, "centred on the view (x)").toBeCloseTo(640 - 50, -1);
  expect(copy?.bounds.y, "centred on the view (y)").toBeCloseTo(400 - 30, -1);
});

test.describe("export as PNG without the dialog", () => {
  // The menu button and the row, in page points at zoom 1 (`crates/ui/tests/top_strip.rs` pins the numbers): the button
  // is at (26, 20) and "Export as PNG" is centred at y = 322, the first row under the "Export" heading.
  const MENU_BUTTON = { x: 26, y: 20 };
  const EXPORT_AS_PNG = { x: 50, y: 322 };

  const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

  async function activeName(app: App): Promise<string> {
    const tabs = (await app.exec("tab.list")).tabs as { active: boolean; name: string }[];
    return tabs.find((t) => t.active)?.name ?? "";
  }

  async function readAll(download: Download): Promise<Buffer> {
    const chunks: Buffer[] = [];
    for await (const chunk of (await download.createReadStream()) as AsyncIterable<Buffer>) chunks.push(chunk);
    return Buffer.concat(chunks);
  }

  test("a real click on the menu row 'Export as PNG' downloads a PNG named after the document", async ({ app }) => {
    await camera(app, { x: 640, y: 400 }, 1);
    await box(app, 600, 360, 120, 80, { fill: "#ffd43b" });
    await app.exec("selection.clear");
    const name = await activeName(app);

    await app.page.mouse.move(MENU_BUTTON.x, MENU_BUTTON.y);
    await app.page.mouse.click(MENU_BUTTON.x, MENU_BUTTON.y);
    await app.page.waitForTimeout(400);
    const downloaded = app.page.waitForEvent("download", { timeout: 20_000 });
    await app.page.mouse.click(EXPORT_AS_PNG.x, EXPORT_AS_PNG.y);
    const download = await downloaded;

    expect(download.suggestedFilename()).toBe(`${name}.png`);
    const bytes = await readAll(download);
    expect(bytes.subarray(0, 8).equals(PNG_SIGNATURE), "the file is a PNG").toBe(true);
    const ring = (await app.recent(50)).filter((e) => e.id === "doc.export");
    expect(ring.at(-1)?.source, "the click ran doc.export from the menu").toBe("menu");
  });

  test("Ctrl+E downloads the same PNG as the click", async ({ app }) => {
    await camera(app, { x: 640, y: 400 }, 1);
    await box(app, 600, 360, 120, 80, { fill: "#ffd43b" });
    await app.exec("selection.clear");
    const name = await activeName(app);

    await app.page.mouse.move(900, 600);
    const downloaded = app.page.waitForEvent("download", { timeout: 20_000 });
    await app.page.keyboard.press("Control+e");
    const download = await downloaded;
    expect(download.suggestedFilename()).toBe(`${name}.png`);
    expect((await readAll(download)).subarray(0, 8).equals(PNG_SIGNATURE)).toBe(true);
  });
});
