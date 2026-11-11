import { test, expect, type App } from "../../support/app.ts";

/** Brings another tab of the same window to the front, so that `app.page` is hidden, and waits until it reports it. */
async function hide(app: App): Promise<() => Promise<void>> {
  const other = await app.page.context().newPage();
  await other.goto("about:blank");
  await other.bringToFront();
  await app.until("the page to report that it is hidden", async () => (await app.page.evaluate(() => document.visibilityState)) === "hidden");
  return async () => {
    await app.page.bringToFront();
    await other.close();
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
  const made = await within("element.create", app.exec("element.create", { kind: "rectangle", bounds: [10, 20, 120, 80] }));
  expect(made.changed, "the command changed the document").toBe(true);
  expect(((await within("state", app.state())).revision as number), "a read sees the new revision").toBeGreaterThan(revision);
  const batch = await within(
    "batch",
    app.batch([
      { id: "element.create", args: { kind: "ellipse", bounds: [200, 20, 60, 60] } },
      { id: "element.create", args: { kind: "rectangle", bounds: [300, 20, 60, 60] } },
    ]),
  );
  expect(batch.failed ?? null, "the batch ran").toBeNull();
  expect(await within("element count", app.count())).toBe(3);
  const refusal = await within("a refused command", app.refusal("history.redo"));
  expect(refusal.message.length, "a refusal answers with its text").toBeGreaterThan(0);

  expect((await app.frames()).frames, "a hidden page draws nothing for these calls").toBe(before.frames);
  expect(await app.page.evaluate(() => document.visibilityState)).toBe("hidden");

  await show();
  await app.until("the shape on screen after the page is shown again", async () => (await app.frames()).frames > before.frames);
  expect(await app.count()).toBe(3);
});
