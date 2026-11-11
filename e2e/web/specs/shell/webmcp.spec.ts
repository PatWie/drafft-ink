import type { BrowserContext } from "@playwright/test";
import { openApp, serverInfo, test, expect, type App } from "../../support/app.ts";

/**
 * WebMCP (docs/agents.md, section 5.5): the page registers the generated tools with the browser's model context
 * when the browser has one. Chromium here has none, so the specs install a stand-in with the shape of the
 * November 2026 draft (`document.modelContext`) or of the first previews (`navigator.modelContext`) before the
 * page starts, call the tools the way a browser agent does, and read the document back through the commands.
 */

type Where = "document" | "navigator";

/** The stand-in's view of the world, read by the specs. */
interface StubState {
  tools: string[];
  /** The `readOnlyHint`, `untrustedContentHint` and `consequentialHint` each tool registered with. */
  annotations: Record<string, { readOnlyHint?: boolean; untrustedContentHint?: boolean; consequentialHint?: boolean }>;
  /** The descriptor of each tool: what the agent reads. */
  shape: Record<string, { description: string; schemaType: string; hasExecute: boolean; title: string }>;
}

/** What a tool call gives back, once parsed: an MCP-shaped result. */
interface ToolResult {
  content: { type: string; text?: string; data?: string; mimeType?: string }[];
  isError: boolean;
}

/**
 * Puts a stand-in for the browser's model context on `document` or `navigator`, before any page script runs.
 * It follows the draft: `registerTool(tool, {signal})` returns a promise, a tool is removed when the signal
 * aborts, a duplicate name rejects, and `executeTool(name, input)` resolves with the result as a JSON string.
 */
async function installModelContext(context: BrowserContext, where: Where): Promise<void> {
  await context.addInitScript((where: Where) => {
    interface Tool {
      name: string;
      title: string;
      description: string;
      inputSchema: { type: string };
      annotations: Record<string, boolean>;
      execute: (input: unknown, options: { signal: AbortSignal }) => Promise<unknown>;
    }
    const registered = new Map<string, Tool>();
    const context = {
      registerTool(tool: Tool, options?: { signal?: AbortSignal }): Promise<void> {
        if (registered.has(tool.name)) return Promise.reject(new DOMException(`${tool.name} is registered`, "InvalidStateError"));
        registered.set(tool.name, tool);
        options?.signal?.addEventListener("abort", () => registered.delete(tool.name));
        return Promise.resolve();
      },
      async executeTool(name: string, input: unknown): Promise<string> {
        const tool = registered.get(name);
        if (!tool) throw new DOMException(`no tool ${name}`, "NotFoundError");
        const result = await tool.execute(input, { signal: new AbortController().signal });
        return JSON.stringify(result);
      },
    };
    const owner: object = where === "document" ? document : navigator;
    Object.defineProperty(owner, "modelContext", { value: context, configurable: true });
    (window as unknown as { __modelContextStub: unknown }).__modelContextStub = {
      state: () => ({
        tools: [...registered.keys()].sort(),
        annotations: Object.fromEntries([...registered].map(([n, t]) => [n, t.annotations])),
        shape: Object.fromEntries(
          [...registered].map(([n, t]) => [n, { description: t.description, schemaType: t.inputSchema.type, hasExecute: typeof t.execute === "function", title: t.title }]),
        ),
      }),
      execute: (name: string, input: unknown) => context.executeTool(name, input),
    };
  }, where);
}

async function stubState(app: App): Promise<StubState> {
  return app.page.evaluate(() => (window as unknown as { __modelContextStub: { state(): StubState } }).__modelContextStub.state());
}

/** Calls a tool as a browser agent does and parses the JSON string it gets. */
async function callTool(app: App, name: string, input: unknown): Promise<ToolResult> {
  const text = await app.page.evaluate(
    ({ name, input }) => (window as unknown as { __modelContextStub: { execute(n: string, i: unknown): Promise<string> } }).__modelContextStub.execute(name, input),
    { name, input },
  );
  return JSON.parse(text) as ToolResult;
}

function textOf(result: ToolResult): string {
  return result.content.map((c) => c.text ?? "").join("\n");
}

for (const where of ["document", "navigator"] as const) {
  test(`webmcp: the tools are registered with ${where}.modelContext and a call changes the document`, async ({ context }) => {
    await installModelContext(context, where);
    const app = await openApp(context, serverInfo().origin);
    await app.exec("doc.new");

    const status = await app.page.evaluate(() => window.drafftink.webmcp.status());
    expect(status.supported).toBe(true);
    expect(status.api).toBe(`${where}.modelContext`);
    expect(status.registered).toBe(true);

    // The browser holds the tools the page says it registered: one per family, and the fixed ones.
    const stub = await stubState(app);
    expect(stub.tools).toEqual([...status.tools].sort());
    for (const want of ["inspect", "element", "style", "diagram", "arrange", "batch", "commands", "describe", "recent"]) {
      expect(stub.tools, `the tool ${want}`).toContain(want);
    }
    for (const name of stub.tools) {
      const shape = stub.shape[name];
      expect(shape?.hasExecute, `${name} can be executed`).toBe(true);
      expect(shape?.schemaType, `${name} has an object schema`).toBe("object");
      expect(shape?.description.length ?? 0, `${name} says what it does`).toBeGreaterThan(20);
      expect(shape?.title, `${name} has a title`).not.toBe("");
    }
    expect(stub.annotations["inspect"]).toMatchObject({ readOnlyHint: true, untrustedContentHint: true, consequentialHint: false });
    expect(stub.annotations["diagram"]).toMatchObject({ readOnlyHint: false, untrustedContentHint: false });

    // A flowchart through the tool, as the browser's agent would call it.
    expect(await app.count()).toBe(0);
    const made = await callTool(app, "diagram", {
      command: "diagram.create",
      args: {
        nodes: [
          { id: "a", label: "Start", shape: "ellipse" },
          { id: "b", label: "Check" },
        ],
        edges: [{ from: "a", to: "b", label: "go" }],
      },
    });
    expect(made.isError, textOf(made)).toBe(false);
    const outline = (await app.exec("doc.outline", { format: "json" })).outline as { nodes: { label?: string }[]; edges: unknown[] };
    expect(outline.nodes.map((n) => n.label).sort()).toEqual(["Check", "Start"]);
    expect(outline.edges).toHaveLength(1);

    // It was an agent's call: the ring says so, and it is one undo step.
    const ring = await app.recent(10);
    expect(ring.filter((e) => e.id === "diagram.create").map((e) => e.source)).toEqual(["agent"]);
    expect((await app.exec("history.state")).undo_depth).toBe(1);

    // Reading goes through the read-only tool and returns the document's words.
    const read = await callTool(app, "inspect", { command: "doc.outline" });
    expect(read.isError, textOf(read)).toBe(false);
    expect(textOf(read)).toContain("Start");

    // A command of another family is pointed to its own tool, in words.
    const wrongTool = await callTool(app, "element", { command: "view.pan" });
    expect(wrongTool.isError).toBe(true);
    expect(textOf(wrongTool)).toContain("`view.pan` is called with the `view` tool");

    // One undo through the tool removes the diagram.
    const undone = await callTool(app, "history", { command: "history.undo" });
    expect(undone.isError, textOf(undone)).toBe(false);
    expect(await app.count()).toBe(0);

    // A refusal comes back as a result with the words and the way out, not as a thrown error.
    const refused = await callTool(app, "view", { command: "view.zoom_to_fit" });
    expect(refused.isError).toBe(true);
    expect(textOf(refused)).toContain("view.center");

    expect(app.consoleErrors).toEqual([]);
  });
}

test("webmcp: a batch through the tool is one atomic undo step and a bad call is an error result", async ({ context }) => {
  await installModelContext(context, "document");
  const app = await openApp(context, serverInfo().origin);
  await app.exec("doc.new");

  const batch = await callTool(app, "batch", {
    label: "Two boxes",
    steps: [
      { command: "element.create", args: { kind: "rectangle", name: "one", bounds: { x: 0, y: 0, w: 100, h: 60 } } },
      { command: "element.create", args: { kind: "rectangle", name: "two", bounds: { x: 200, y: 0, w: 100, h: 60 } } },
    ],
  });
  expect(batch.isError, textOf(batch)).toBe(false);
  expect(await app.count()).toBe(2);
  expect((await app.exec("history.state")).undo_depth).toBe(1);

  const failing = await callTool(app, "batch", {
    steps: [
      { command: "element.create", args: { kind: "rectangle", bounds: { x: 0, y: 200, w: 100, h: 60 } } },
      { command: "element.nope", args: {} },
    ],
  });
  expect(failing.isError).toBe(true);
  expect(textOf(failing)).toContain("Step 1");
  expect(await app.count(), "the failed batch changed nothing").toBe(2);

  const notAnObject = await app.page.evaluate(() =>
    (window as unknown as { __modelContextStub: { execute(n: string, i: unknown): Promise<string> } }).__modelContextStub.execute("inspect", "doc.outline"),
  );
  const parsed = JSON.parse(notAnObject) as ToolResult;
  expect(parsed.isError).toBe(true);
  expect(textOf(parsed)).toContain("must be an object");
  expect(app.consoleErrors).toEqual([]);
});

test("webmcp: the tools can be taken back and put again, and the address can turn them off", async ({ context }) => {
  await installModelContext(context, "document");
  const app = await openApp(context, serverInfo().origin);
  expect((await stubState(app)).tools.length).toBeGreaterThan(10);

  await app.page.evaluate(() => window.drafftink.webmcp.unregister());
  expect((await stubState(app)).tools, "an aborted registration removes every tool").toEqual([]);
  expect(await app.page.evaluate(() => window.drafftink.webmcp.status().registered)).toBe(false);

  expect(await app.page.evaluate(() => window.drafftink.webmcp.register())).toBe(true);
  expect((await stubState(app)).tools.length).toBeGreaterThan(10);
  expect(await app.page.evaluate(() => window.drafftink.webmcp.register()), "registering twice is harmless").toBe(true);

  const off = await openApp(context, serverInfo().origin, "/?webmcp=off");
  expect((await stubState(off)).tools, "?webmcp=off registers nothing").toEqual([]);
  expect(await off.page.evaluate(() => window.drafftink.webmcp.status().supported)).toBe(true);
  expect(app.consoleErrors).toEqual([]);
  expect(off.consoleErrors).toEqual([]);
});

test("webmcp: a browser without WebMCP is not affected", async ({ context }) => {
  const app = await openApp(context, serverInfo().origin);
  await app.exec("doc.new");
  const status = await app.page.evaluate(() => window.drafftink.webmcp.status());
  expect(status).toMatchObject({ supported: false, api: null, registered: false });
  expect(status.tools.length, "the page knows its tools even when nothing takes them").toBeGreaterThan(10);
  expect(await app.page.evaluate(() => window.drafftink.webmcp.register())).toBe(false);
  await app.exec("element.create", { kind: "rectangle", bounds: { x: 0, y: 0, w: 50, h: 50 } });
  expect(await app.count()).toBe(1);
  expect(app.consoleErrors).toEqual([]);
});
