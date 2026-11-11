import { test, expect } from "../../support/app.ts";
import { box, expectRefusal, undoDepth } from "../../support/build.ts";
import { solidPng } from "../../support/png.ts";

// One test per command family: a command that works, a refusal with its words, and the undo (or the fact that there is none).

test("family doc: rename, background and clear undo; refusals say how to go on; reads change nothing", async ({ app }) => {
  expectRefusal(await app.refusal("doc.clear"), "empty_document", "The document is already empty, so there is nothing to clear.");
  const renamed = await app.exec("doc.rename", { name: "  Plan  " });
  expect(renamed.name).toBe("Plan");
  expect(renamed.undo_label).toBe("Rename document");
  expectRefusal(await app.refusal("doc.rename", { name: "   " }), "empty_name", "The name is empty. Give a name of 1 to 256 characters.");
  await app.exec("history.undo");
  expect((await app.exec("doc.inspect")).name).toBe("Untitled");

  const bg = await app.exec("doc.set_background", { color: "#fafafa" });
  expect(bg.background).toBe("#fafafa");
  expect((await app.exec("doc.set_background", { color: "#fafafa" })).changed, "the same colour is not a change").toBe(false);
  expectRefusal(await app.refusal("doc.set_background", { color: "not-a-colour" }), "bad_args", "color");
  await app.exec("history.undo");
  expect((await app.exec("doc.inspect")).background).toBe("#ffffff");

  await box(app, 0, 0);
  const second = await box(app, 200, 0, 100, 60, { kind: "ellipse" });
  await app.exec("element.lock", { ids: [second] });
  const cleared = await app.exec("doc.clear");
  expect(cleared.removed).toBe(2);
  expect((await app.exec("doc.inspect")).elements).toBe(0);
  await app.exec("history.undo");
  const back = await app.exec("doc.inspect");
  expect(back.elements).toBe(2);
  expect(back.locked).toBe(1);

  const before = await undoDepth(app);
  await app.exec("doc.outline");
  await app.exec("doc.inspect", { detail: "full" });
  expect(await undoDepth(app), "reading adds no undo step").toEqual(before);
  expectRefusal(await app.refusal("doc.inspect", { ids: [second] }), "bad_args", 'only work with detail "full"');

  expectRefusal(
    await app.refusal("doc.new"),
    "unsaved_changes",
    "This document has unsaved changes. Save it with `doc.save`, or pass on_unsaved=\"discard\" to drop them",
  );
  const fresh = await app.exec("doc.new", { on_unsaved: "discard", name: "Fresh" });
  expect(fresh.elements).toBe(0);
  expect(fresh.name).toBe("Fresh");
  expectRefusal(await app.refusal("history.undo"), "nothing_to_undo", "nothing to undo");
});

test("family doc: export gives JSON and SVG inline, and refuses an empty document", async ({ app }) => {
  expectRefusal(await app.refusal("doc.export", { format: "json" }), "nothing_to_do", "Nothing to export: the document has no elements.");
  await box(app, 0, 0);
  await box(app, 200, 0, 100, 60, { kind: "ellipse" });
  const json = await app.exec("doc.export", { format: "json", scope: "all" });
  expect(json.elements).toBe(2);
  const bytes = json.bytes as { base64: string };
  expect(JSON.parse(Buffer.from(bytes.base64, "base64").toString("utf8"))).toBeTruthy();
  const svg = await app.exec("doc.export", { format: "svg", scope: "all" });
  expect(svg.mime).toBe("image/svg+xml");
  expect(Buffer.from((svg.bytes as { base64: string }).base64, "base64").toString("utf8")).toContain("<svg");
  await app.exec("selection.clear");
  expectRefusal(await app.refusal("doc.export", { format: "json", scope: "selection" }), "no_target", "Nothing to act on: no elements are selected.");
  expectRefusal(await app.refusal("doc.export", { format: "json", scale: 100 }), "bad_args", "scale");
  // The exported JSON opens again as a new tab.
  const reopened = await app.exec("doc.open", { source: { base64: bytes.base64, name: "out.json" }, mode: "new_tab" });
  expect(reopened.elements).toBe(2);
  expect(reopened.format).toBe("drafftink");
});

test("family doc: open refuses what it cannot read and unsaved work; merge is one undo step", async ({ app }) => {
  await box(app, 0, 0, 100, 60, { name: "mine" });
  const nonsense = Buffer.from("not a document").toString("base64");
  expectRefusal(
    await app.refusal("doc.open", { source: { base64: nonsense } }),
    "parse_failed",
    "The file is not a drafft.ink or Excalidraw document",
    "Open a .json, .drafftink or .excalidraw file",
  );
  const mermaid = Buffer.from("flowchart TD; A-->B").toString("base64");
  expectRefusal(await app.refusal("doc.open", { source: { base64: mermaid, name: "plan.mmd" } }), "parse_failed", "diagram.import_mermaid");
  expect(await app.count(), "a refused open leaves the document alone").toBe(1);

  const exported = (await app.exec("doc.export", { format: "json", scope: "all" })).bytes as { base64: string };
  expectRefusal(
    await app.refusal("doc.open", { source: { base64: exported.base64, name: "mine.json" } }),
    "unsaved_changes",
    "This document has unsaved changes. Save it with `doc.save`, or pass on_unsaved=\"discard\" to drop them",
  );
  const merged = await app.exec("doc.open", { source: { base64: exported.base64, name: "mine.json" }, mode: "merge", at: [2000, 0] });
  expect(merged.mode).toBe("merge");
  expect(merged.selected).toBe(1);
  expect(merged.undo_label).toBe("Open document");
  expect(await app.count()).toBe(2);
  await app.exec("history.undo");
  expect(await app.count(), "the merge was one undo step").toBe(1);
});

test("family doc: the web build renders a png export (the renderer port is there)", async ({ app }) => {
  await box(app, 0, 0);
  const out = await app.exec("doc.export", { format: "png" });
  const bytes = out.bytes as { base64: string; mime: string };
  expect(bytes.mime).toBe("image/png");
  expect(Buffer.from(bytes.base64, "base64").subarray(0, 8).toString("hex"), "a PNG signature").toBe("89504e470d0a1a0a");
});

function pngWidth(base64: string): number {
  return Buffer.from(base64, "base64").readUInt32BE(16);
}

test("a batch with pictures in it completes as a whole: applied once, every picture in the answer, outside steps kept", async ({ app }) => {
  const before = await undoDepth(app);
  const scales = [0.5, 1, 1.5, 2, 3];
  const result = await app.batch([
    { id: "settings.set", args: { key: "export.padding", value: 0 } },
    { id: "element.create", args: { kind: "rectangle", bounds: { x: 0, y: 0, w: 100, h: 60 } } },
    ...scales.map((scale) => ({ id: "doc.export" as const, args: { format: "png" as const, scope: "all" as const, scale } })),
  ]);
  expect(result.failed ?? null, "nothing failed").toBeNull();
  expect(result.rolled_back).toBe(false);
  expect(result.pending, "the answer comes after the pictures: no placeholder is left").toBeUndefined();
  expect(result.pending_render, "the batch was not taken back").toBeUndefined();
  expect(result.completed).toBe(2 + scales.length);
  const widths = result.results.slice(2).map((r) => pngWidth((r as { bytes: { base64: string } }).bytes.base64));
  expect(widths, "each picture is of its own size, 100 units wide at its scale").toEqual(scales.map((s) => Math.round(100 * s)));
  // Applied once: one rectangle, one undo step, and the setting stays.
  expect(await app.count()).toBe(1);
  const after = await undoDepth(app);
  expect(after.depth, "the batch is one undo step").toBe(before.depth + 1);
  expect(((await app.exec("settings.get", { key: "export.padding" })).settings as Record<string, unknown>)["export.padding"]).toBe(0);
});

test("a batch whose picture comes before a change is still run once and shows the document before that change", async ({ app }) => {
  const id = await box(app, 0, 0, 100, 60);
  const result = await app.batch([
    { id: "doc.export", args: { format: "png", scope: "all", scale: 1, padding: 0 } },
    { id: "element.move", args: { ids: [id], dx: 500, dy: 0, snap: "none" } },
  ]);
  expect(result.failed ?? null).toBeNull();
  expect(result.pending_render, "the answer is the finished one").toBeUndefined();
  expect(pngWidth(((result.results[0] as { bytes: { base64: string } }).bytes.base64))).toBe(100);
  expect((await app.element(id)).bounds.x).toBe(500);
});

test("family tab: new, rename, switch, duplicate, move and close; closing the last or a dirty tab is refused", async ({ app }) => {
  const first = (await app.exec("tab.list")).tabs as { id: string }[];
  expect(first).toHaveLength(1);
  const t1 = first[0]?.id ?? "";
  const created = await app.exec("tab.new", { name: "Sketch" });
  const t2 = created.id as string;
  expect(created.active).toBe(t2);
  const renamed = await app.exec("tab.rename", { name: "Plans" });
  expect(renamed.undo_label).toBe("Rename tab");
  expectRefusal(await app.refusal("tab.rename", { name: "" }), "empty_name", "1 to 256 characters");
  await app.exec("history.undo");
  const tabs = async () => (await app.exec("tab.list")).tabs as { id: string; name: string; active: boolean }[];
  expect((await tabs()).find((t) => t.id === t2)?.name).toBe("Sketch");
  await app.exec("history.redo");
  expect((await tabs()).find((t) => t.id === t2)?.name).toBe("Plans");

  expect((await app.exec("tab.switch", { tab: t1 })).active).toBe(t1);
  expectRefusal(await app.refusal("tab.switch", { tab: "t9" }), "no_such_tab", "t9", "tab.list");
  const copy = await app.exec("tab.duplicate", { name: "Copy" });
  const t3 = copy.id as string;
  await app.exec("tab.move", { tab: t3, to_index: 0 });
  expect((await tabs()).map((t) => t.id)).toEqual([t3, t1, t2]);

  await box(app, 0, 0);
  expectRefusal(await app.refusal("tab.close", { tab: t3 }), "unsaved_changes", "unsaved");
  await app.exec("tab.close", { tab: t3, on_unsaved: "discard" });
  await app.exec("tab.close", { tab: t1, on_unsaved: "discard" });
  expectRefusal(await app.refusal("tab.close", { tab: t2, on_unsaved: "discard" }), "last_tab", "tab.new");
  expect(await tabs()).toHaveLength(1);
});

test("family settings: set, toggle, cycle and reset; wrong keys and values are refused with the fix", async ({ app }) => {
  expect((await app.exec("settings.get", { key: "snap.grid" })).default).toBe(false);
  const set = await app.exec("settings.set", { key: "snap.grid", value: true });
  expect(set).toMatchObject({ changed: true, previous: false, value: true });
  expect((await app.exec("settings.set", { key: "snap.grid", value: true })).changed).toBe(false);
  expect((await app.exec("settings.toggle", { key: "snap.grid" })).value).toBe(false);
  const cycled = await app.exec("settings.cycle", { key: "view.grid_style", step: 1 });
  expect(cycled.previous).toBe("lines");
  expect(cycled.value).toBe("horizontal_lines");
  expect((await app.exec("settings.reset", { key: "view.grid_style" })).reset).toEqual(["view.grid_style"]);
  expect(((await app.exec("settings.get", { key: "view.grid_style" })).settings as Record<string, unknown>)["view.grid_style"]).toBe("lines");
  expectRefusal(await app.refusal("settings.set", { key: "snap.gird", value: true }), "unknown_setting", "Did you mean `snap.grid`");
  expectRefusal(await app.refusal("settings.set", { key: "snap.grid", value: "sure" }), "bad_args", "true or false");
  expectRefusal(await app.refusal("settings.set", { key: "doc.recent", value: [] }), "bad_args", "read only");
  expect((await undoDepth(app)).depth, "settings are not document history").toBe(0);
  expectRefusal(await app.refusal("history.undo"), "nothing_to_undo");
});

test("family library: add a selection, insert it back as new elements, rename, remove", async ({ app }) => {
  expect((await app.exec("library.list")).items).toEqual([]);
  expectRefusal(await app.refusal("library.add"), "no_target", "Nothing to act on: no elements are selected.");
  const a = await box(app, 100, 100, 100, 60);
  await app.exec("element.relabel", { id: a, text: "Start" });
  const b = await box(app, 300, 100, 100, 60, { kind: "ellipse" });
  const added = await app.exec("library.add", { ids: [a, b], name: "Pair" });
  expect((added.item as { name: string; element_count: number }).element_count).toBe(3);
  expectRefusal(await app.refusal("library.add", { ids: [a], name: "   " }), "empty_name", "The name is empty.");
  const depth = (await undoDepth(app)).depth;
  const inserted = await app.exec("library.insert", { item: "Pair", at: [1000, 500] });
  expect(inserted.ids as string[]).toHaveLength(3);
  expect(inserted.undo_label).toBe("Insert from library");
  expect(await app.count()).toBe(4);
  await app.exec("history.undo");
  expect(await app.count()).toBe(2);
  expect((await undoDepth(app)).depth, "adding to the library is not a document step").toBe(depth);
  expectRefusal(await app.refusal("library.insert", { item: "nope" }), "no_such_item", "There is no library item `nope`. List items with `library.list`.");
  expect(((await app.exec("library.rename", { item: "Pair", name: "Two shapes" })).item as { name: string }).name).toBe("Two shapes");
  expectRefusal(await app.refusal("library.remove"), "bad_args", "Give either `items` or all=true.");
  const removed = await app.exec("library.remove", { items: ["Two shapes"] });
  expect(removed.removed as string[]).toHaveLength(1);
  expect((await app.exec("library.list")).items).toEqual([]);
});

test("family image: insert from bytes scales to fit; bad bytes are refused with the supported formats; one undo step", async ({ app }) => {
  const small = solidPng(40, 20, [10, 120, 200]);
  const wide = solidPng(1600, 800, [200, 50, 50]);
  const one = await app.exec("image.insert", { images: [{ base64: small, name: "small.png" }], at: [100, 100] });
  expect(one.created as string[]).toHaveLength(1);
  const rows = (await app.exec("element.query", { filter: { kind: "image" }, fields: ["bounds"] })).items as { bounds: { x: number; w: number; h: number } }[];
  expect(rows[0]?.bounds).toMatchObject({ w: 40, h: 20, x: 80 });
  const two = await app.exec("image.insert", { images: [{ base64: wide, name: "wide.png" }] });
  const wideRow = await app.element(two.created?.[0] ?? "");
  expect(wideRow.bounds.w, "a large picture is scaled to the fit width").toBe(800);
  expect(wideRow.bounds.h).toBe(400);
  await app.exec("history.undo");
  expect((await app.exec("element.query", { filter: { kind: "image" } })).total).toBe(1);
  expectRefusal(
    await app.refusal("image.insert", { images: [{ base64: Buffer.from("not an image").toString("base64"), name: "notes.txt" }] }),
    "unsupported_format",
    "`notes.txt` is not a supported image",
    "PNG, JPEG or WebP",
  );
  expectRefusal(await app.refusal("element.create", { kind: "image" }), "wrong_kind", "image.insert");
});
