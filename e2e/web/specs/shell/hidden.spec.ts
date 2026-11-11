import { test, expect, type App } from "../../support/app.ts";

/**
 * Makes the page report that it is hidden, and returns the function that shows it again.
 *
 * Xvfb has no window manager, so another tab or window never occludes the page and the browser never reports
 * `hidden`. The page is told the way the shell's own idle checks do it (`crates/gpu/scripts/idle_web.py`): the
 * `hidden` and `visibilityState` properties are overridden and `visibilitychange` is fired. The app reads
 * `document.hidden` in that event, so everything after it runs the code path of a hidden tab: the shell draws no
 * frame and a wake-up has to do its work without one.
 */
async function hide(app: App): Promise<() => Promise<void>> {
  await app.page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  return async () => {
    await app.page.evaluate(() => {
      delete (document as unknown as Record<string, unknown>)["hidden"];
      delete (document as unknown as Record<string, unknown>)["visibilityState"];
      document.dispatchEvent(new Event("visibilitychange"));
    });
  };
}

/** The call, or a rejection after `ms` that names it: a hung call must fail fast, not at the 120 s test limit. */
async function within<T>(what: string, call: Promise<T>, ms = 10_000): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what} did not answer within ${ms} ms while the page was hidden`)), ms);
  });
  try {
    return await Promise.race([call, late]);
  } finally {
    clearTimeout(timer);
  }
}

test("hidden page: window.drafftink commands run and answer, and no frame is drawn", async ({ app }) => {
  await app.settle(1000);
  const show = await hide(app);
  await app.settle(1000);
  const before = await app.frames();

  const revision = (await within("state", app.state())).revision as number;
  const made = await within("element.create", app.exec("element.create", { kind: "rectangle", bounds: { x: 10, y: 20, w: 120, h: 80 } }));
  expect(made.changed, "the command changed the document").toBe(true);
  expect(((await within("state", app.state())).revision as number), "a read sees the new revision").toBeGreaterThan(revision);
  const batch = await within(
    "batch",
    app.batch([
      { id: "element.create", args: { kind: "ellipse", bounds: { x: 200, y: 20, w: 60, h: 60 } } },
      { id: "element.create", args: { kind: "rectangle", bounds: { x: 300, y: 20, w: 60, h: 60 } } },
    ]),
  );
  expect(batch.failed ?? null, "the batch ran").toBeNull();
  expect(await within("element count", app.count())).toBe(3);
  const refusal = await within("a refused command", app.refusal("history.redo"));
  expect(refusal.message.length, "a refusal answers with its text").toBeGreaterThan(0);

  expect((await app.frames()).frames, "a hidden page draws nothing for these calls").toBe(before.frames);
  expect(await app.page.evaluate(() => document.visibilityState), "the page is still hidden").toBe("hidden");

  await show();
  await app.until("the shape on screen after the page is shown again", async () => (await app.frames()).frames > before.frames);
  expect(await app.count()).toBe(3);
});
