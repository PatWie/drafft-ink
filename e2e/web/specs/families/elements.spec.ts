import { test, expect } from "../../support/app.ts";
import { box, connect, expectRefusal, undoDepth } from "../../support/build.ts";

test("family element: create, transform, delete; each is one undo step and undo restores the exact box", async ({ app }) => {
  const made = await app.exec("element.create", { kind: "rectangle", bounds: { x: 0, y: 0, w: 100, h: 60 } });
  const id = made.id;
  expect(made.created).toEqual([id]);
  expect(made.bounds.w).toBe(100);
  expect((await app.exec("selection.get")).ids, "a new element is selected").toEqual([id]);
  expectRefusal(await app.refusal("element.create", { kind: "rectangle", bounds: { x: 0, y: 0, w: 0, h: 60 } }), "too_small", "smaller than 1 unit in width");
  expectRefusal(await app.refusal("element.create", { kind: "rectangle", id, bounds: { x: 0, y: 0, w: 5, h: 5 } }), "id_in_use", "already used by another element");
  expect((await undoDepth(app)).depth, "refusals add no step").toBe(1);

  const resized = await app.exec("element.resize", { ids: [id], handle: "se", dx: 20, dy: 10 });
  expect(resized.bounds).toMatchObject({ w: 120, h: 70, x: 0 });
  const rotated = await app.exec("element.rotate", { ids: [id], angle: 30 });
  expect((rotated.angle_badge as { degrees: number }).degrees).toBeCloseTo(30, 2);
  await app.exec("element.rotate", { ids: [id], angle: 0, mode: "to" });
  const set = await app.exec("element.set_bounds", { ids: [id], x: 10, w: 50 });
  expect(set.bounds).toEqual({ x: 10, y: 0, w: 50, h: 70 });
  const nudged = await app.exec("element.nudge", { ids: [id], dir: "left", step: 25 });
  expect(nudged.dx).toBe(-25);
  expect((await app.element(id)).bounds).toEqual({ x: -15, y: 0, w: 50, h: 70 });
  await app.exec("history.undo");
  expect((await app.element(id)).bounds).toEqual({ x: 10, y: 0, w: 50, h: 70 });
  await app.exec("history.undo", { steps: 3 });
  const back = await app.element(id);
  expect(back.bounds).toEqual({ x: 0, y: 0, w: 120, h: 70 });
  expect(back.angle ?? 0).toBeCloseTo(0, 3);
  await app.exec("history.redo", { steps: 3 });

  expect((await app.exec("element.delete", { ids: [id] })).deleted).toEqual([id]);
  expect(await app.count()).toBe(0);
  expectRefusal(await app.refusal("element.delete", { ids: [id] }), "not_found", `No element matches \`${id}\``);
  await app.exec("history.undo");
  expect((await app.element(id)).kind).toBe("rectangle");
  await app.exec("selection.clear");
  expectRefusal(await app.refusal("element.delete"), "no_target", "no elements are selected");
  expectRefusal(await app.refusal("element.set_bounds", { ids: [id], w: 0 }), "bad_args");
});

test("family element: duplicate, flip, erase, lock, names and labels", async ({ app }) => {
  const a = await box(app, 0, 0, 100, 60, { fill: "#ffcc00" });
  const b = await box(app, 300, 100, 100, 60, { kind: "ellipse", fill: "#00ccff" });
  const dup = await app.exec("element.duplicate", { ids: [a], dx: 5, dy: 6 });
  const copy = (dup.created as string[])[0] ?? "";
  expect((await app.element(copy)).bounds).toEqual({ x: 5, y: 6, w: 100, h: 60 });
  await app.exec("history.undo");
  expect(await app.count()).toBe(2);
  await app.exec("selection.clear");
  expectRefusal(await app.refusal("element.duplicate"), "no_target", "no elements are selected");

  await app.exec("element.flip", { ids: [a, b], axis: "horizontal" });
  expect((await app.element(a)).bounds.x, "the rectangle is on the right after the flip").toBe(300);
  await app.exec("history.undo");
  expect((await app.element(a)).bounds.x).toBe(0);

  const connected = await app.exec("element.connect", { from: a, to: b, label: "go" });
  expect(connected.created as string[]).toHaveLength(2);
  expectRefusal(await app.refusal("element.connect", { from: a, to: a }), "same_element", "Pass two different elements");
  await app.exec("history.undo");

  expect((await app.exec("element.lock", { ids: [a] })).locked).toBe(true);
  expectRefusal(await app.refusal("element.move", { ids: [a], dx: 1 }), "locked", `\`${a}\` is locked`, "element.unlock_all");
  expect((await app.exec("element.unlock_all")).ids).toEqual([a]);
  expectRefusal(await app.refusal("element.unlock_all"), "nothing_to_do", "no element is locked");

  expect((await app.exec("element.set_name", { id: a, name: "box" })).name).toBe("box");
  expectRefusal(await app.refusal("element.set_name", { id: b, name: "box" }), "name_taken", `already used by element \`${a}\``, "@box");
  expectRefusal(await app.refusal("element.set_name", { id: b, name: "@x" }), "bad_name", "must not start with `@` or `$`");

  const labelled = await app.exec("element.relabel", { id: a, text: "Label" });
  expect(labelled.action).toBe("created");
  expect((await app.element(a)).text).toBe("Label");
  const cleared = await app.exec("element.relabel", { id: a, text: "" });
  expect(cleared.action).toBe("removed");
  await app.exec("history.undo");
  expect((await app.element(a)).text, "undo brings the label back").toBe("Label");
  expectRefusal(await app.refusal("element.relabel", { id: "nope", text: "L" }), "not_found");
});

test("family element: query finds by kind, text and @name, and at hit-tests a point", async ({ app }) => {
  const r = await box(app, 0, 0, 100, 60, { name: "start", fill: "#ffcc00" });
  await box(app, 300, 100, 100, 60, { kind: "ellipse" });
  await app.exec("element.create", { kind: "text", text: "hello", at: [0, 200] });
  const kinds = await app.exec("element.query", { filter: { kind: ["rectangle", "text"] }, fields: ["id", "kind", "text"] });
  expect(kinds.total).toBe(2);
  expect(kinds.truncated).toBe(false);
  expect((await app.exec("element.query", { filter: { text: "HEL" }, fields: ["id"] })).total, "text search ignores case").toBe(1);
  expect(((await app.exec("element.query", { filter: { ids: ["@start"] }, fields: ["id", "name"] })).items[0] as { id: string }).id).toBe(r);
  expectRefusal(await app.refusal("element.query", { filter: { ids: ["@nope"] } }), "not_found", "No element matches `@nope`", "element.query");
  expectRefusal(await app.refusal("element.query", { limit: 0 }), "bad_args", "from 1 to 1000");
  const page = await app.exec("element.query", { limit: 2, offset: 2, fields: ["id"] });
  expect(page.total).toBe(3);
  expect(page.items).toHaveLength(1);

  expect((await app.exec("element.at", { point: [50, 30] })).count).toBe(1);
  expect((await app.exec("element.at", { point: [350, 600] })).count).toBe(0);
  await app.exec("element.lock", { ids: [r] });
  expect((await app.exec("element.at", { point: [50, 30] })).count, "a locked element is not hit").toBe(0);
  expect(((await app.exec("element.at", { point: [50, 30], include_locked: true })).hits as { id: string }[])[0]?.id).toBe(r);
});

test("family selection: select modes, region, lasso and navigate add no undo step; refusals name what is wrong", async ({ app }) => {
  const a = await box(app, 0, 0, 100, 60, { fill: "#ffcc00" });
  const b = await box(app, 300, 0, 100, 60, { kind: "ellipse", fill: "#ffcc00" });
  const c = await box(app, 600, 0, 100, 60, { kind: "diamond", fill: "#ffcc00" });
  await connect(app, a, b);
  const depth = (await undoDepth(app)).depth;
  await app.exec("selection.clear");
  expect((await app.exec("selection.get")).count).toBe(0);
  expect((await app.exec("selection.select", { ids: [a] })).selected).toEqual([a]);
  expect((await app.exec("selection.select", { ids: [b], mode: "add" })).selected).toHaveLength(2);
  expect((await app.exec("selection.select", { ids: [a], mode: "toggle" })).selected).toEqual([b]);
  expect((await app.exec("selection.select", { ids: [b], mode: "remove" })).selected).toEqual([]);
  expect((await app.exec("selection.select", { filter: { kind: "ellipse" } })).selected).toEqual([b]);
  expectRefusal(await app.refusal("selection.select", { ids: [a], filter: { kind: "ellipse" } }), "bad_args", "exactly one of `ids`, `group` or `filter`");
  expectRefusal(await app.refusal("selection.select", { ids: ["nope"] }), "not_found", "nope");
  await app.exec("selection.all");
  expect((await app.exec("selection.get", { detail: "ids" })).count).toBe(4);
  await app.exec("selection.clear");
  expect((await app.exec("selection.select_at", { point: [650, 30] })).selected).toEqual([c]);
  expect((await app.exec("selection.select_region", { rect: { x: -10, y: -10, w: 105, h: 90 } })).selected).toEqual([a]);
  const lasso = await app.exec("selection.select_lasso", { points: [[-50, -50], [130, -50], [130, 110], [-50, 110]], contain: "contain" });
  expect(lasso.selected).toEqual([a]);
  expectRefusal(await app.refusal("selection.select_lasso", { points: [[0, 0], [100, 100]] }), "bad_args", "a lasso needs 3", "Draw a loop");
  await app.exec("selection.select", { ids: [a] });
  expect((await app.exec("selection.navigate", { direction: "right" })).selected).toEqual([b]);
  expectRefusal(await app.refusal("selection.navigate", { direction: "up" }), "nothing_to_do", "has no connected element to the up");
  expect((await undoDepth(app)).depth, "selecting never adds an undo step").toBe(depth);
});

test("family clipboard: cut and paste in one undo step; style copies; refusals say what is missing", async ({ app }) => {
  const a = await box(app, 0, 0, 100, 60, { fill: "#ffcc00" });
  const b = await box(app, 300, 0, 100, 60, { kind: "ellipse" });
  expectRefusal(await app.refusal("clipboard.paste_style", { ids: [b] }), "nothing_to_do", "No style has been copied yet");
  const style = await app.exec("clipboard.copy_style", { from: a });
  expect((style.props as Record<string, unknown>)["fill_color"]).toBe("#ffcc00");
  expect((await app.exec("clipboard.paste_style", { ids: [b] })).changed).toBe(true);
  expect(((await app.exec("style.get", { ids: [b], props: ["fill_color"] })).props as unknown as { fill_color: { value: string } }).fill_color.value).toBe("#ffcc00");
  await app.exec("history.undo");
  expect(((await app.exec("style.get", { ids: [b], props: ["fill_color"] })).props as unknown as { fill_color: { value: string } }).fill_color.value).not.toBe("#ffcc00");

  await app.exec("selection.select", { ids: [a] });
  expect((await app.exec("clipboard.cut")).deleted).toEqual([a]);
  expect(await app.count()).toBe(1);
  expect((await app.exec("clipboard.paste", { source: "internal", at: [200, 200] })).kind).toBe("elements");
  expect(await app.count()).toBe(2);
  await app.exec("history.undo", { steps: 2 });
  expect(await app.count(), "cut and paste are two steps; both undone").toBe(2);
  expect((await app.element(a)).kind).toBe("rectangle");

  await app.exec("selection.clear");
  expectRefusal(await app.refusal("clipboard.cut"), "no_target", "no elements are selected");
  expectRefusal(await app.refusal("clipboard.copy"), "no_target");
  const text = await app.exec("clipboard.paste", { source: { text: "plain words" } });
  expect(text.kind).toBe("text");
  expectRefusal(await app.refusal("clipboard.paste", { source: "internal", plain_text: true }), "bad_args");
  expectRefusal(await app.refusal("clipboard.paste", { source: { elements: { nonsense: true } } }), "parse_failed");
  expect((await app.exec("clipboard.copy_as", { format: "text", scope: "all" })).format).toBe("text");
});

test("family style: set changes only the named properties; themes restyle in one step; undo gives the old look back", async ({ app }) => {
  const a = await box(app, 0, 0, 100, 60);
  const b = await box(app, 200, 0, 100, 60);
  const get = async (id: string, prop: string) => ((await app.exec("style.get", { ids: [id], props: [prop as "stroke_width"] })).props as Record<string, { value: unknown }>)[prop]?.value;
  expect(await get(a, "stroke_width")).toBe(2);
  const set = await app.exec("style.set", { ids: [a], props: { stroke_width: 4 } });
  expect(set.undo_label).toBe("Set style");
  expect(set.applied).toEqual({ [a]: ["stroke_width"] });
  expect(await get(a, "stroke_width")).toBe(4);
  expect(await get(b, "stroke_width"), "other elements are untouched").toBe(2);
  expectRefusal(await app.refusal("style.set", { ids: [a], props: { no_such_property: 1 } as never }), "bad_args", "no_such_property");
  expectRefusal(await app.refusal("style.set", { ids: ["zzz"], props: { stroke_width: 4 } }), "not_found", "zzz");
  const themes = (await app.exec("style.list_themes")).themes as { name: string }[];
  expect(themes.some((t) => t.name === "pastel")).toBe(true);
  expect((await app.exec("style.apply_theme", { name: "pastel", ids: [a, b] })).theme).toBe("pastel");
  expectRefusal(await app.refusal("style.apply_theme", { name: "no-such-theme", ids: [a] }), "unknown_theme");
  const undone = await app.exec("history.undo", { steps: 2 });
  expect(undone.undone).toEqual(["Apply theme", "Set style"]);
  expect(await get(a, "stroke_width")).toBe(2);
});

test("family text: create, set, latex, runs, size and labels undo; the editor session is not for agents", async ({ app }) => {
  const rect = await box(app, 200, 0, 100, 60);
  // The edit session belongs to the editor (a person typing); an agent sets text with `text.set`.
  expectRefusal(await app.refusal("text.edit_begin", { at: [0, 300] }), "not_for_agents", "not available to agents");
  const made = await app.exec("element.create", { kind: "text", text: "new draft", at: [0, 300] });
  const textId = made.id;
  expect((await app.element(textId)).text).toBe("new draft");
  await app.exec("history.undo");
  expect(await app.count(), "creating the text is one undo step").toBe(1);
  await app.exec("history.redo");

  expect((await app.exec("text.set", { id: textId, text: "hello there" })).changed).toBe(true);
  expectRefusal(await app.refusal("text.set", { id: textId, text: "" }), "empty_text", "Delete it with `element.delete`");

  const math = await app.exec("element.create", { kind: "math", latex: "x^2", at: [0, 200] });
  expectRefusal(await app.refusal("text.set", { id: math.id, text: "x" }), "wrong_kind", "text.set_latex");
  await app.exec("text.set_latex", { id: math.id, latex: "\\frac{a}{b}" });
  expectRefusal(await app.refusal("text.set_latex", { id: math.id, latex: "\\frac{" }), "invalid_latex", "could not be parsed", "\\frac{a}{b}");
  expect((await app.element(math.id)).text, "a refused formula keeps the old one").toBe("\\frac{a}{b}");

  const runs = await app.exec("text.style_range", { id: textId, start: 0, end: 5, color: "#ff0000" });
  expect(runs.runs).toEqual([[0, 5, "#ff0000"]]);
  expectRefusal(await app.refusal("text.style_range", { id: textId, start: 0, end: 500, color: "#ff0000" }), "bad_range", "outside the text");
  const bigger = await app.exec("text.step_size", { direction: "increase", ids: [textId] });
  expect(bigger.resized).toEqual([textId]);
  expectRefusal(await app.refusal("text.step_size", { direction: "increase", ids: [rect] }), "not_applicable", "support");

  await app.exec("text.bind", { text: textId, container: rect });
  expect((await app.element(rect)).bound_text).toBe(textId);
  expect((await app.exec("text.bind", { text: textId, container: rect })).changed, "binding the same label again changes nothing").toBe(false);
  const other = await app.exec("element.create", { kind: "text", text: "Other", at: [0, 500] });
  expectRefusal(await app.refusal("text.bind", { text: other.id, container: rect }), "already_bound", "Remove it first with `text.unbind`");
  await app.exec("text.unbind", { text: textId });
  expect((await app.element(rect)).bound_text ?? null).toBeNull();
  expectRefusal(await app.refusal("text.unbind", { text: textId }), "nothing_to_do", "is not a label of any element");
  await app.exec("history.undo");
  expect((await app.element(rect)).bound_text, "undo of unbind").toBe(textId);
  expect((await app.exec("text.set_autoresize", { ids: [other.id], on: false })).changed).toBe(true);
  expect((await app.exec("text.set_autoresize", { ids: [other.id], on: false })).changed).toBe(false);
  expectRefusal(await app.refusal("text.set_autoresize", { ids: [rect], on: true }), "not_applicable", "apply to text elements");
});
