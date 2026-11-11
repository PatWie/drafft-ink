import { test, expect, type App } from "../../support/app.ts";
import { box } from "../../support/build.ts";

/** Frames drawn in a window of `ms` milliseconds, and the redraw reasons that were requested. */
async function frameWindow(app: App, ms: number) {
  const f0 = await app.frames();
  const r0 = (await app.perf()).redraws.by_reason;
  await app.page.waitForTimeout(ms);
  const f1 = await app.frames();
  const r1 = (await app.perf()).redraws.by_reason;
  const reasons = Object.fromEntries(Object.entries(r1).map(([k, v]) => [k, v - (r0[k] ?? 0)]).filter(([, v]) => v !== 0));
  return { frames: f1.frames - f0.frames, vello: f1.vello_runs - f0.vello_runs, reasons };
}

test("idle: an open document draws 0 frames in 10 seconds", async ({ app }) => {
  await box(app, 100, 100);
  await app.settle(1500);
  const w = await frameWindow(app, 10_000);
  expect(w.frames, `reasons: ${JSON.stringify(w.reasons)}`).toBe(0);
  expect(w.vello).toBe(0);
});

test("idle: a pointer resting over the canvas draws no frame", async ({ app }) => {
  await box(app, 100, 100);
  await app.settle(1500);
  const p = await app.toPage({ x: 500, y: 500 });
  await app.page.mouse.move(p.x, p.y);
  await app.settle(1500);
  const w = await frameWindow(app, 5_000);
  expect(w.frames, `reasons: ${JSON.stringify(w.reasons)}`).toBe(0);
});

test("idle: commands wake the app, then it goes back to 0 frames", async ({ app }) => {
  await app.settle(1000);
  const before = await app.frames();
  await box(app, 50, 50);
  await app.until("a frame for the new shape", async () => (await app.frames()).frames > before.frames);
  await app.settle(1500);
  const w = await frameWindow(app, 5_000);
  expect(w.frames, `reasons: ${JSON.stringify(w.reasons)}`).toBe(0);
  const reasons = (await app.perf()).redraws.by_reason;
  expect(Object.values(reasons).some((n) => n > 0), "redraws carry a reason").toBe(true);
});
