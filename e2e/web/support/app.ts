/**
 * The app under test: a page with `window.drafftink`, wrapped so that specs read like the commands they run.
 *
 * Everything a spec does to the document goes through `exec` and `batch` (the path a user's button and an
 * agent share), or through real pointer and key events on the page. The harness (`harness.page.ts`) is for
 * what neither can do.
 */
import { test as base, expect, type BrowserContext, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";
import type { DrafftinkTest } from "../harness/harness.types.ts";
import { installHarness } from "../harness/harness.page.ts";
import type { ArgsParam, BatchOpts, BatchResult, BatchStep, CommandId, DrafftinkApi, Output, PerfStats, RecentEntry, RefusalData } from "./api.ts";
import { FIXTURES, SERVER_INFO } from "./env.ts";

/** JSON-RPC code of a command refusal (docs/agents.md 5.1). */
export const REFUSED = -32010;
/** JSON-RPC code of invalid parameters. */
export const INVALID_PARAMS = -32602;

/** A rejected call of `window.drafftink`. */
export class Rejected extends Error {
  constructor(
    readonly rpcCode: number,
    message: string,
    readonly data: RefusalData | undefined,
    readonly call: string,
  ) {
    super(`${call} was rejected (${rpcCode}): ${message}`);
  }
}

interface Settled<T> {
  ok: boolean;
  value?: T;
  error?: { code: number; message: string; data?: RefusalData };
}

/** Where the server of the run listens (written by the global setup). */
export function serverInfo(): { port: number; origin: string; altOrigin: string; skip: string | null } {
  return JSON.parse(readFileSync(SERVER_INFO, "utf8")) as { port: number; origin: string; altOrigin: string; skip: string | null };
}

/** A point in the world or on the screen. */
export interface Pt {
  x: number;
  y: number;
}

/** Names the harness methods of a spec call; `probe(app.page, "canvasInfo")`. */
export type HarnessName = keyof DrafftinkTest;

/** Calls a method of `window.__drafftinkTest`. */
export async function probe<K extends HarnessName>(page: Page, name: K, ...args: Parameters<DrafftinkTest[K]>): Promise<Awaited<ReturnType<DrafftinkTest[K]>>> {
  return (await page.evaluate(
    ({ name, args }) => {
      const h = (window as unknown as { __drafftinkTest: Record<string, (...a: unknown[]) => unknown> }).__drafftinkTest;
      const method = h[name];
      if (!method) throw new Error(`window.__drafftinkTest.${name} is not installed`);
      return method(...args);
    },
    { name: name as string, args: args as unknown[] },
  )) as Awaited<ReturnType<DrafftinkTest[K]>>;
}

/** One open page of the app. */
export class App {
  readonly consoleErrors: string[] = [];

  constructor(
    readonly page: Page,
    readonly origin: string,
  ) {
    page.on("pageerror", (e) => this.consoleErrors.push(`pageerror: ${e.message}`));
    page.on("console", (m) => {
      if (m.type() === "error") this.consoleErrors.push(`console.error: ${m.text()}`);
    });
  }

  /** Runs `window.drafftink[method](...args)` and returns what it resolves or how it rejects. */
  private async call<T>(method: string, args: unknown[]): Promise<Settled<T>> {
    return this.page.evaluate(
      async ({ method, args }) => {
        const api = window.drafftink as unknown as Record<string, (...a: unknown[]) => unknown>;
        const fn = api[method];
        if (typeof fn !== "function") return { ok: false, error: { code: 0, message: `window.drafftink.${method} is not a function` } };
        try {
          return { ok: true, value: await fn.apply(api, args) } as Settled<never>;
        } catch (e) {
          const err = e as { code?: number; message?: string; data?: RefusalData };
          return { ok: false, error: { code: err.code ?? 0, message: err.message ?? String(e), data: err.data } };
        }
      },
      { method, args },
    );
  }

  private async resolve<T>(method: string, args: unknown[], label: string): Promise<T> {
    const r = await this.call<T>(method, args);
    if (!r.ok) throw new Rejected(r.error?.code ?? 0, r.error?.message ?? "", r.error?.data, label);
    return r.value as T;
  }

  /** Runs one command. Throws `Rejected` with the refusal when the command is refused. */
  exec<K extends CommandId>(id: K, ...args: ArgsParam<K>): Promise<Output<K>> {
    return this.resolve<Output<K>>("execute", [id, args[0]], id);
  }

  /** Runs one command that must be refused, and returns the refusal. Fails when it succeeds. */
  async refusal<K extends CommandId>(id: K, ...args: ArgsParam<K>): Promise<RefusalData> {
    const r = await this.call<unknown>("execute", [id, args[0]]);
    expect(r.ok, `${id} should have been refused but it ran: ${JSON.stringify(r.value)?.slice(0, 200)}`).toBe(false);
    expect(r.error?.code, `${id} rejected, but not as a refusal: ${r.error?.message}`).toBe(REFUSED);
    const data = r.error?.data;
    if (!data) throw new Error(`${id}: the refusal carries no data`);
    return data;
  }

  /** Runs a call that must reject with another code than a refusal (bad parameters), and returns it. */
  async rejection(method: "execute" | "batch" | "describe" | "recent", ...args: unknown[]): Promise<{ code: number; message: string }> {
    const r = await this.call<unknown>(method, args);
    expect(r.ok, `${method} should have been rejected`).toBe(false);
    return { code: r.error?.code ?? 0, message: r.error?.message ?? "" };
  }

  /** Runs several commands as one undo step. */
  batch(steps: BatchStep[], opts?: BatchOpts): Promise<BatchResult> {
    return this.resolve<BatchResult>("batch", [steps, opts], "batch");
  }

  /** The registry with live availability. */
  commands(opts?: Parameters<DrafftinkApi["commands"]>[0]): Promise<Output<"engine.commands">> {
    return this.resolve("commands", [opts ?? {}], "commands");
  }

  /** Schemas, facts, example and refusals of one command. */
  describe(id: string): Promise<Record<string, unknown>> {
    return this.resolve("describe", [id], "describe");
  }

  /** The newest entries of the ring, oldest first. */
  async recent(limit = 20): Promise<RecentEntry[]> {
    return (await this.resolve<{ entries: RecentEntry[] }>("recent", [{ limit }], "recent")).entries;
  }

  /** `engine.state`. */
  state(): Promise<Output<"engine.state">> {
    return this.resolve("state", [], "state");
  }

  /** Counters and redraw reasons. */
  perf(): Promise<PerfStats> {
    return this.page.evaluate(() => window.drafftink.perf());
  }

  /** Frames drawn and frames in which Vello ran since start. */
  frames(): Promise<{ frames: number; vello_runs: number }> {
    return this.page.evaluate(() => window.drafftink.frames());
  }

  /** Waits until no frame has been drawn for `quietMs`. Returns the frame counters. */
  async settle(quietMs = 600, limitMs = 20_000): Promise<{ frames: number; vello_runs: number }> {
    const start = Date.now();
    let last = await this.frames();
    let since = Date.now();
    while (Date.now() - since < quietMs) {
      if (Date.now() - start > limitMs) throw new Error(`the app kept drawing for ${limitMs} ms (frames ${last.frames})`);
      await this.page.waitForTimeout(50);
      const now = await this.frames();
      if (now.frames !== last.frames) {
        last = now;
        since = Date.now();
      }
    }
    return last;
  }

  /** Waits for a command-read condition to hold; the polling is the only wait. */
  async until(what: string, check: () => Promise<boolean>, limitMs = 15_000): Promise<void> {
    const start = Date.now();
    while (Date.now() - start < limitMs) {
      if (await check()) return;
      await this.page.waitForTimeout(100);
    }
    throw new Error(`timed out after ${limitMs} ms waiting for ${what}`);
  }

  /**
   * Waits until the autosave record (`doc.list {from: "autosave"}`) was written at or after `sinceMs`
   * (a `Date.now()` taken before the edit). The app writes it on its own timer; `doc.autosave` is not for agents.
   */
  async untilSaved(sinceMs: number, limitMs = 30_000): Promise<void> {
    await this.until("the autosave record", async () => {
      const docs = (await this.exec("doc.list", { from: "autosave" })).documents as { modified_ms: number | null }[];
      return docs.some((d) => (d.modified_ms ?? 0) >= sinceMs);
    }, limitMs);
  }

  /** Presses a tool shortcut with the pointer at `at`, and waits until the engine reports the tool. */
  async pressToolKey(at: Pt, name: string, tool: string): Promise<void> {
    await this.page.mouse.move(at.x, at.y);
    await this.page.keyboard.press(name);
    await this.until(`tool ${tool} after the key ${name}`, async () => (await this.state()).tool === tool);
  }

  /** Reloads the page and waits for the app. */
  async reload(): Promise<void> {
    await this.page.reload();
    await waitStarted(this.page);
  }

  // ---- geometry ---------------------------------------------------------------------------------------

  /** Turns a world point into the page coordinates a mouse event uses, from `view.get` and the canvas rectangle. */
  async toPage(world: Pt): Promise<Pt> {
    const [view, info] = await Promise.all([this.exec("view.get"), probe(this.page, "canvasInfo")]);
    const zoom = view.zoom as number;
    const scroll = view.scroll as Pt | [number, number];
    const [sx, sy] = Array.isArray(scroll) ? scroll : [scroll.x, scroll.y];
    const viewport = view.viewport as { w: number; h: number };
    return { x: info.left + viewport.w / 2 + (world.x - (sx as number)) * zoom, y: info.top + viewport.h / 2 + (world.y - (sy as number)) * zoom };
  }

  /** Turns page coordinates into a world point. */
  async toWorld(screen: Pt): Promise<Pt> {
    const [view, info] = await Promise.all([this.exec("view.get"), probe(this.page, "canvasInfo")]);
    const zoom = view.zoom as number;
    const scroll = view.scroll as Pt | [number, number];
    const [sx, sy] = Array.isArray(scroll) ? scroll : [scroll.x, scroll.y];
    const viewport = view.viewport as { w: number; h: number };
    return { x: (sx as number) + (screen.x - info.left - viewport.w / 2) / zoom, y: (sy as number) + (screen.y - info.top - viewport.h / 2) / zoom };
  }

  // ---- reading the document ---------------------------------------------------------------------------

  /** The bounds, bindings and kind of elements by id. */
  async elements(ids: string[]): Promise<Record<string, ElementRow>> {
    const r = await this.exec("element.query", { filter: { ids }, fields: ["id", "kind", "bounds", "bindings", "points", "angle", "group_ids", "frame", "name", "locked", "text", "bound_text"], limit: 500 });
    const rows = r.items as ElementRow[];
    return Object.fromEntries(rows.map((row) => [row.id, row]));
  }

  /** One element row; fails when the element is missing. */
  async element(id: string): Promise<ElementRow> {
    const row = (await this.elements([id]))[id];
    if (!row) throw new Error(`element ${id} is not in the document`);
    return row;
  }

  /** How many elements the document has (what `element.query` counts). */
  async count(): Promise<number> {
    return (await this.exec("element.query", { limit: 1 })).total;
  }
}

/** The fields `elements()` asks for. */
export interface ElementRow {
  id: string;
  kind: string;
  bounds: { x: number; y: number; w: number; h: number };
  bindings?: { start: string | null; end: string | null };
  points?: [number, number][];
  angle?: number;
  group_ids?: string[];
  frame?: string | null;
  text?: string | null;
  bound_text?: string | null;
  name?: string;
  locked?: boolean;
}

/** Waits until `index.html` says the app started; throws with the page's error when it did not. */
export async function waitStarted(page: Page, timeoutMs = 90_000): Promise<void> {
  await page.waitForFunction(() => window.__started === true || typeof window.__error === "string", undefined, { timeout: timeoutMs });
  const error = await page.evaluate(() => window.__error ?? null);
  if (error !== null) throw new Error(`the app failed to start: ${error}`);
  // The WebMCP tools are built from the command schemas, which arrive as a file after the first frame.
  await page.evaluate(() => window.drafftink.webmcp.ready);
}

/** Opens the app on a new page of `context`. `path` may carry a query such as `?room=x`. */
export async function openApp(context: BrowserContext, origin: string, urlPath = "/"): Promise<App> {
  // No WebGPU adapter on this machine: every spec that needs the app is skipped, with the reason (the global setup
  // fails the run instead when DRAFFTINK_REQUIRE_GPU=1).
  const skip = serverInfo().skip;
  base.skip(skip !== null, skip ?? "");
  const page = await context.newPage();
  await page.addInitScript(installHarness);
  const app = new App(page, origin);
  await page.goto(origin + urlPath);
  await waitStarted(page);
  return app;
}

/** A fixture file of `tests/fixtures`, as base64 for `doc.open {source: {base64}}` or the harness drop. */
export function fixtureBase64(relative: string): string {
  return readFileSync(path.join(FIXTURES, relative)).toString("base64");
}

/** The test with an `app` fixture: a fresh browser context and the app opened on `/`. */
export const test = base.extend<{ app: App }>({
  app: async ({ context }, use) => {
    const app = await openApp(context, serverInfo().origin);
    // A blank document: the app starts on the intro document.
    await app.exec("doc.new");
    await use(app);
    // A page error or a console error is a bug of the app, whatever the spec was checking.
    expect(app.consoleErrors, "the page logged errors").toEqual([]);
  },
});

export { expect };
