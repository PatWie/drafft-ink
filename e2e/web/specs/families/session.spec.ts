import { test, expect } from "../../support/app.ts";
import { box, camera, expectRefusal, undoDepth } from "../../support/build.ts";

test("family view: zoom, pan, fit and centre move only the camera and add no undo step", async ({ app }) => {
  await box(app, 1000, 1000, 100, 60);
  const depth = (await undoDepth(app)).depth;
  const v0 = await app.exec("view.get");
  expect(v0.viewport).toMatchObject({ w: 1280, h: 800 });
  await app.exec("view.set", { zoom: 1.68, scroll: [10, 20] });
  expect((await app.exec("view.get")).percent).toBeCloseTo(100, 6);
  const zoomed = await app.exec("view.zoom_at", { factor: 2 });
  expect(zoomed.zoom).toBeCloseTo(3.36, 6);
  expect((await app.exec("view.zoom_reset")).zoom).toBeCloseTo(1.68, 6);
  expectRefusal(await app.refusal("view.zoom_at", { factor: 2, zoom: 1 }), "bad_args", "Give exactly one of `factor` or `zoom`");
  const before = (await app.exec("view.get")).scroll as [number, number];
  const panned = await app.exec("view.pan", { dx: 100, dy: 0 });
  const after = panned.scroll as [number, number];
  expect(after[0], "the content moves right by dx screen pixels: the camera moves left by dx / zoom").toBeCloseTo(before[0] - 100 / 1.68, 6);
  expect(after[1]).toBeCloseTo(before[1], 6);
  expect((await app.exec("view.center", { on: "content" })).scroll).toEqual([1050, 1030]);
  expect((await app.exec("view.set", { zoom: 100000 })).zoom, "zoom is clamped").toBe(30);
  expectRefusal(await app.refusal("view.set", { zoom: "big" as never }), "bad_args", "zoom");
  await app.exec("view.zoom_to_fit", { padding: 50 });
  const view = await app.exec("view.get");
  const visible = view.visible as { x: number; y: number; w: number; h: number };
  expect(visible.x).toBeLessThanOrEqual(1000);
  expect(visible.x + visible.w).toBeGreaterThanOrEqual(1100);
  await app.exec("view.set", { zoom: 1.68, scroll: [0, 0] });
  await app.exec("selection.clear");
  await app.exec("view.set_view_only", { on: true });
  expectRefusal(await app.refusal("element.create", { kind: "rectangle", bounds: { x: 0, y: 0, w: 5, h: 5 } }), "view_only");
  await app.exec("view.set_view_only", { on: false });
  expectRefusal(await app.refusal("view.screenshot"), "port_unavailable", "renderer");
  expectRefusal(await app.refusal("view.set_viewport", { width: 10, height: 10 }), "not_for_agents");
  expect((await undoDepth(app)).depth, "camera commands are not history").toBe(depth);
});

test("family history: undo and redo walk one step at a time; a batch is one step; empty stacks refuse in words", async ({ app }) => {
  expectRefusal(await app.refusal("history.undo"), "nothing_to_undo", "nothing to undo");
  expectRefusal(await app.refusal("history.redo"), "nothing_to_redo", "nothing to redo");
  const a = await box(app, 0, 0);
  await app.exec("style.set", { ids: [a], props: { stroke_width: 4 } });
  expect(await app.exec("history.state")).toMatchObject({ undo_depth: 2, undo_label: "Set style" });
  const undone = await app.exec("history.undo");
  expect(undone.undone).toEqual(["Set style"]);
  expect(undone.can_redo).toBe(true);
  expect((await app.exec("history.redo")).redone).toEqual(["Set style"]);

  const b = await box(app, 200, 0);
  const batch = await app.batch(
    [
      { id: "style.set", args: { ids: [a], props: { stroke_width: 1 } } },
      { id: "style.set", args: { ids: [b], props: { stroke_width: 1 } } },
      { id: "element.move", args: { ids: [a], dx: 5, dy: 0, snap: "none" } },
    ],
    { label: "Restyle and move" },
  );
  expect(batch.completed).toBe(3);
  expect(batch.undo_label).toBe("Restyle and move");
  const depth = (await app.exec("history.state")).undo_depth as number;
  await app.exec("history.undo");
  expect((await app.exec("history.state")).undo_depth, "the whole batch is one step").toBe(depth - 1);
  expect((await app.element(a)).bounds.x).toBe(0);

  // A batch that fails in the middle rolls back when atomic.
  const failed = await app.batch(
    [
      { id: "element.move", args: { ids: [a], dx: 50, dy: 0, snap: "none" } },
      { id: "element.delete", args: { ids: ["nope"] } },
    ],
    { atomic: true },
  );
  expect(failed.failed?.index).toBe(1);
  expect(failed.failed?.refusal.code).toBe("not_found");
  expect(failed.rolled_back).toBe(true);
  expect((await app.element(a)).bounds.x, "the first step was rolled back").toBe(0);
  expect((await app.exec("history.clear")).changed).toBe(true);
  expectRefusal(await app.refusal("history.undo"), "nothing_to_undo");
});

test("family tool: set, lock, hold, release and cancel; unknown tools and view-only mode are refused in words", async ({ app }) => {
  expect(await app.state()).toMatchObject({ tool: "select", tool_lock: false });
  expect((await app.exec("tool.set", { tool: "rectangle" })).tool).toBe("rectangle");
  expect((await app.exec("tool.set", { tool: "rectangle" })).changed).toBe(false);
  expect((await app.exec("tool.lock", { on: true })).tool_lock).toBe(true);
  // Holding a tool (the space bar) belongs to the keyboard.
  expectRefusal(await app.refusal("tool.hold", { tool: "hand" }), "not_for_agents", "not available to agents");
  expect((await app.exec("tool.cancel")).rungs).toEqual(["tool"]);
  expect(await app.state()).toMatchObject({ tool: "select", tool_lock: false });
  expectRefusal(await app.refusal("tool.set", { tool: "banana" as never }), "bad_args", "banana", "rectangle");
  const set = await app.exec("tool.set_arrow", { arrow_type: "elbow", start_head: "circle", end_head: null });
  expect(set).toMatchObject({ arrow_type: "elbow", start_head: "circle", end_head: "none" });
  expectRefusal(await app.refusal("tool.set_arrow", {}), "bad_args", "arrow_type", "start_head", "end_head");
  await app.exec("settings.reset", { key: "arrow.type" });
  await app.exec("view.set_view_only", { on: true });
  expectRefusal(await app.refusal("tool.set", { tool: "rectangle" }), "view_only_tool", "select, hand and laser");
  expect((await app.exec("tool.set", { tool: "laser" })).tool).toBe("laser");
  await app.exec("view.set_view_only", { on: false });
  expect((await undoDepth(app)).depth, "tools are not history").toBe(0);
});

test("family gesture: begin, update and end are for the pointer, not for agents", async ({ app }) => {
  const a = await box(app, 0, 0);
  const begin = await app.refusal("gesture.begin", { command: "style.set", args: { ids: [a], props: { stroke_width: 3 } } });
  expectRefusal(begin, "not_for_agents", "not available to agents");
  expectRefusal(await app.refusal("gesture.update", { gesture: "g1", args: {} }), "not_for_agents");
  expectRefusal(await app.refusal("gesture.cancel"), "not_for_agents");
  expect((await app.state()).gesture ?? null, "nothing started").toBeNull();
  expect(((await app.state()).tabs as unknown[]).length).toBe(1);
  // As a user gets it: dragging a shape is one gesture of element.move, and it ends as one undo step.
  await camera(app, { x: 640, y: 400 }, 1);
  const shape = await box(app, 500, 300, 200, 100, { fill: "#ffd8a8" });
  await app.exec("selection.clear");
  const depth = (await undoDepth(app)).depth;
  const from = await app.toPage({ x: 600, y: 350 });
  await app.page.mouse.move(from.x, from.y);
  await app.page.mouse.move(from.x + 1, from.y + 1);
  await app.page.mouse.down();
  for (let i = 1; i <= 8; i++) {
    await app.page.mouse.move(from.x + i * 12, from.y + i * 9);
    await app.page.waitForTimeout(40);
  }
  await app.until("a gesture while the button is down", async () => Boolean((await app.state()).gesture));
  expect((await app.state()).gesture).toMatchObject({ command: "element.move" });
  expectRefusal(await app.refusal("history.undo"), "gesture_active");
  await app.page.mouse.up();
  await app.until("the gesture ends", async () => !(await app.state()).gesture);
  expect((await undoDepth(app)).depth, "the whole drag is one undo step").toBe(depth + 1);
  expect((await app.element(shape)).bounds.x).toBeGreaterThan(550);
  await app.exec("history.undo");
  expect((await app.element(shape)).bounds.x, "undo puts the shape back").toBe(500);
});

test("family engine and perf: the registry describes itself and counts what ran", async ({ app }) => {
  const tab = await app.commands({ family: "tab" });
  const byId = Object.fromEntries((tab.commands as { id: string; family: string; undo: string }[]).map((c) => [c.id, c]));
  expect(byId["tab.new"]).toMatchObject({ family: "tab", undo: "NoUndo" });
  expect(byId["tab.rename"]?.undo).toBe("Undoable");
  expect(byId["view.pan"]).toBeUndefined();
  const zoom = await app.commands({ search: "zoom", available_only: true });
  expect((zoom.commands as { id: string; available: unknown }[]).find((c) => c.id === "view.zoom_in")?.available).toBe(true);
  const described = await app.describe("view.pan");
  expect((described["args_schema"] as { properties: object }).properties).toHaveProperty("dx");
  expect(typeof (described["example"] as { args: { dx: unknown } }).args.dx).toBe("number");
  expectRefusal(await app.refusal("engine.describe", { id: "view.pann" }), "unknown_command", "Did you mean `view.pan`");
  expectRefusal(await app.refusal("engine.commands", { family: "nope" }), "bad_args", "Families:");

  await app.exec("view.zoom_in");
  const stats = await app.exec("perf.stats");
  expect((stats.counters as { commands: number }).commands).toBeGreaterThan(0);
  expect(stats.since).toBe("start");
  expect((await app.exec("perf.stats", { since: "last_call" })).since).toBe("last_call");
  expectRefusal(await app.refusal("perf.stats", { since: "never" as never }), "bad_args", "last_call");
  const ring = await app.recent(5);
  expect(ring.at(-1)?.source).toBe("agent");
  expect((await undoDepth(app)).depth, "introspection is not history").toBe(0);
});

test("family collab: the steps are checked in order and each refusal says the next one", async ({ app }) => {
  const status = await app.exec("collab.status");
  expect(status.state).toBe("disconnected");
  expect(status.peers).toEqual([]);
  expectRefusal(await app.refusal("collab.disconnect"), "not_connected", "nothing to disconnect");
  expectRefusal(await app.refusal("collab.leave_room"), "not_in_room", "nothing to leave");
  expectRefusal(await app.refusal("collab.follow", { peer: "nobody" }), "not_in_room");
  expectRefusal(await app.refusal("collab.connect", { url: "http://example.com/ws" }), "invalid_url", "is not a valid server address", "ws:// or wss://");
  expectRefusal(await app.refusal("collab.join_room", { room: "abc" }), "not_connected", "Connect first with `collab.connect`");
  // In the web app a bare /ws means the server that delivered this page.
  expect((await app.exec("collab.connect", { url: "/ws" })).state).toBe("ready");
  expectRefusal(await app.refusal("collab.join_room", { room: "bad room!" }), "bad_args", "letters, digits, `-` and `_`");
  expectRefusal(await app.refusal("collab.connect", { url: "/ws" }), "already_connected", "Disconnect first with `collab.disconnect`");
  expect((await app.exec("collab.disconnect")).changed).toBe(true);
  expect((await app.exec("collab.status")).state).toBe("disconnected");
  const user = await app.exec("collab.set_user", { name: "Pat", color: "#ef4444" });
  expect(user).toMatchObject({ name: "Pat", color: "#ef4444" });
  expectRefusal(await app.refusal("collab.set_user", { color: "red" }), "bad_args", "is not a colour");
  expect(((await app.exec("settings.get", { key: "collab.user_name" })).settings as Record<string, unknown>)["collab.user_name"]).toBe("Pat");
  expectRefusal(await app.refusal("collab.set_pointer", { point: [1, 2] }), "not_for_agents");
  const laser = await app.exec("collab.laser", { points: [[0, 0], [10, 10]] });
  expect(laser.points).toBe(2);
  expectRefusal(await app.refusal("collab.laser", { points: [[0, 0]], fade_ms: 0 }), "bad_args");
  expect((await undoDepth(app)).depth, "presence is not history").toBe(0);
});
