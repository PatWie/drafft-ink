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
  expect(keys).toEqual(["agent", "batch", "commands", "describe", "execute", "frames", "perf", "perf_enable", "perf_reset", "recent", "runScenario", "scenarioReport", "state", "version", "webmcp"]);
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

test("startup: the loading page counts the wasm download as a percentage and ends at 100%", async ({ browser }) => {
  // No service worker, so that the route below sees the request.
  const context = await browser.newContext({ serviceWorkers: "block" });
  const seen: string[] = [];
  let wasmBytes = 0;
  await context.route("**/*.wasm*", async (route) => {
    const response = await route.fetch();
    const body = await response.body();
    wasmBytes = body.length;
    await route.fulfill({ response, body });
  });
  await context.addInitScript(() => {
    const log: string[] = ((window as unknown as { __progressLog: string[] }).__progressLog = []);
    const watch = () => {
      const el = document.getElementById("loading-progress");
      if (!el) return false;
      new MutationObserver(() => log.push(el.textContent ?? "")).observe(el, { childList: true, characterData: true, subtree: true });
      return true;
    };
    if (!watch()) document.addEventListener("DOMContentLoaded", watch);
  });
  const app = await openApp(context, serverInfo().origin);
  const log = await app.page.evaluate(() => (window as unknown as { __progressLog: string[] }).__progressLog);
  seen.push(...log);
  expect(wasmBytes, "the wasm went through the route").toBeGreaterThan(100_000);
  const progress = await app.page.evaluate(() => window.__loadProgress);
  expect(progress, "every byte of the module was counted").toEqual({ loaded: wasmBytes, total: wasmBytes });
  expect(seen.length, "the number was written while loading").toBeGreaterThan(0);
  for (const text of seen) expect(text, "each reading is a whole percentage").toMatch(/^\d{1,3}%$/);
  expect(seen[seen.length - 1], "and the last one is complete").toBe("100%");
  expect(app.consoleErrors).toEqual([]);
  await context.close();
});

test("startup: a wasm that cannot be fetched shows the failure page with the reason", async ({ browser }) => {
  const context = await browser.newContext({ serviceWorkers: "block" });
  await context.route("**/*.wasm*", (route) => route.fulfill({ status: 404, body: "gone" }));
  const page = await context.newPage();
  await page.goto(serverInfo().origin);
  await page.waitForFunction(() => typeof window.__error === "string", undefined, { timeout: 30_000 });
  await expect(page.locator("#failure")).toBeVisible();
  await expect(page.locator("#failure-detail")).toContainText("404");
  await context.close();
});
