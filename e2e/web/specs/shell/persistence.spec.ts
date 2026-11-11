import { test, expect, openApp } from "../../support/app.ts";
import { startServer } from "../../support/server.ts";
import { box, connect } from "../../support/build.ts";

test("persistence: the workspace is saved and comes back after a reload", async ({ app }) => {
  const since = Date.now();
  const a = await box(app, 100, 100, 120, 80, { name: "keep-a" });
  const b = await box(app, 400, 100, 120, 80, { kind: "ellipse", name: "keep-b" });
  await connect(app, a, b, "kept");
  await app.exec("doc.rename", { name: "Persisted drawing" });
  await app.untilSaved(since);

  await app.reload();
  const inspect = await app.exec("doc.inspect");
  expect(inspect.name).toBe("Persisted drawing");
  expect(inspect.elements).toBe(3);
  expect(inspect.bound_arrows, "the arrow is still bound").toBe(1);
  const names = ((await app.exec("element.query", { fields: ["name"] })).items as { name?: string }[]).map((r) => r.name).filter(Boolean);
  expect(names.sort()).toEqual(["keep-a", "keep-b"]);
});

test("offline: with the server stopped, a reload still starts the app and restores the drawing", async ({ context }) => {
  const server = await startServer();
  try {
    const app = await openApp(context, server.origin);
    await app.exec("doc.new");
    const since = Date.now();
    await box(app, 10, 10, 100, 60, { name: "offline-box" });
    await app.untilSaved(since);
    // The service worker caches the shell on its first visit; wait until it controls the page.
    await app.page.evaluate(async () => {
      await navigator.serviceWorker.ready;
    });
    await app.page.reload();
    await app.page.waitForFunction(() => navigator.serviceWorker.controller !== null);
    await app.page.waitForFunction(() => window.__started === true);

    await server.stop();
    await app.page.reload();
    await app.page.waitForFunction(() => window.__started === true || typeof window.__error === "string", undefined, { timeout: 60_000 });
    expect(await app.page.evaluate(() => window.__error ?? null), "the app starts without the server").toBeNull();
    const rows = await app.exec("element.query", { filter: { ids: ["@offline-box"] }, fields: ["name"] });
    expect(rows.total, "and the workspace is back").toBe(1);

    // A room link also loads from the cache (the room itself needs the network).
    await app.page.goto(`${server.origin}/?room=offline-room`);
    await app.page.waitForFunction(() => window.__started === true || typeof window.__error === "string", undefined, { timeout: 60_000 });
    expect(await app.page.evaluate(() => document.title)).not.toBe("");
  } finally {
    await server.stop();
  }
});
