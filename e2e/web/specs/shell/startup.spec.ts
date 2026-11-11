import registry from "../../generated/registry.json" with { type: "json" };
import { openApp, probe, serverInfo, test as base, expect, waitStarted } from "../../support/app.ts";

// These specs look at the app as it starts, so they do not use the blank-document fixture.
const test = base;

test("startup: the page starts, draws, and shows the intro document", async ({ context }) => {
  const app = await openApp(context, serverInfo().origin);
  expect(await app.page.evaluate(() => window.__error ?? null)).toBeNull();
  expect((await app.frames()).frames, "a first frame was drawn").toBeGreaterThan(0);
  expect(await app.count(), "the app opens on the intro document").toBeGreaterThan(0);
  expect((await app.exec("doc.inspect")).dirty, "which is not unsaved work").toBe(false);
  expect((await app.exec("history.state")).can_undo).toBe(false);
  expect(app.consoleErrors).toEqual([]);
});

test("startup: window.drafftink has the documented methods", async ({ context }) => {
  const app = await openApp(context, serverInfo().origin);
  const keys = await app.page.evaluate(() => Object.keys(window.drafftink).sort());
  expect(keys).toEqual(["agent", "batch", "commands", "describe", "execute", "frames", "perf", "perf_enable", "perf_reset", "recent", "state", "version"]);
  expect(await app.page.evaluate(() => typeof window.drafftink.version)).toBe("string");
  const perf = await app.perf();
  expect(typeof perf.redraws.by_reason).toBe("object");
  expect(app.consoleErrors).toEqual([]);
});

test("startup: the registry in the wasm build is the registry the specs are typed from", async ({ context }) => {
  const app = await openApp(context, serverInfo().origin);
  const live = await app.commands();
  const ids = (live.commands as { id: string }[]).map((c) => c.id).sort();
  expect(ids, "run `cargo xtask e2e --types` after changing the registry").toEqual(registry.ids);
  expect(live.count).toBe(registry.count);
  const available = (live.commands as { id: string; available: unknown }[]).filter((c) => c.available === true);
  expect(available.length, "commands() carries live availability").toBeGreaterThan(50);
});

test("startup: the canvas fills the viewport and the drawing buffer matches the device pixel ratio", async ({ context }) => {
  const app = await openApp(context, serverInfo().origin);
  const info = await probe(app.page, "canvasInfo");
  expect(info.left).toBe(0);
  expect(info.top).toBe(0);
  expect(info.cssWidth).toBe(1280);
  expect(info.cssHeight).toBe(800);
  expect(info.bufferWidth).toBe(Math.round(info.cssWidth * info.devicePixelRatio));
  expect(info.bufferHeight).toBe(Math.round(info.cssHeight * info.devicePixelRatio));
  const view = await app.exec("view.get");
  expect(view.viewport).toMatchObject({ w: 1280, h: 800 });
});

test("startup: bad calls to window.drafftink reject with the documented codes", async ({ context }) => {
  const app = await openApp(context, serverInfo().origin);
  const notAString = await app.rejection("execute", 42);
  expect(notAString.code).toBe(-32602);
  expect(notAString.message).toContain("command id as a string");
  const unknown = await app.refusal("nope.nothing" as "doc.new");
  expect(unknown.code).toBe("unknown_command");
  expect(unknown.message).toContain("engine.commands");
  const badOptions = await app.rejection("recent", 5);
  expect(badOptions.code).toBe(-32602);
  expect(badOptions.message).toContain("options must be an object");
});

test("startup: the agent pairing refuses what it should and scrubs the address", async ({ context }) => {
  const app = await openApp(context, serverInfo().origin);
  const token = "0123456789abcdef".repeat(4);
  const remote = await app.page.evaluate(async (t) => window.drafftink.agent.pair("ws://example.com:4555/", t).then(() => "paired", (e: Error) => e.message), token);
  expect(remote).toContain("only listens on this machine");
  const shortToken = await app.page.evaluate(async () => window.drafftink.agent.pair("ws://127.0.0.1:4555/", "short").then(() => "paired", (e: Error) => e.message));
  expect(shortToken).toContain("64 hexadecimal");
  expect((await app.page.evaluate(() => window.drafftink.agent.status())).state).toBe("idle");

  const page = await context.newPage();
  await page.goto(`${serverInfo().origin}/?x=1#room=r1&drafftink-agent-port=1&drafftink-agent-token=${token}`);
  await waitStarted(page);
  const where = await page.evaluate(() => location.hash + "|" + location.search);
  expect(where, "the token leaves the address bar, the rest stays").toBe("#room=r1|?x=1");
});
