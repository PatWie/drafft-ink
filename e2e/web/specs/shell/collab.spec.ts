import { test as base, expect, openApp, serverInfo, type App } from "../../support/app.ts";
import { box } from "../../support/build.ts";
import { startServer } from "../../support/server.ts";
import { close, shot } from "../../support/pixels.ts";

/** Two pages with separate storage, in one room of the server that serves the page. */
const test = base.extend<{ room: { a: App; b: App; name: string } }>({
  room: async ({ browser }, use) => {
    const name = `e2e-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    const [ca, cb] = [await browser.newContext(), await browser.newContext()];
    const a = await openApp(ca, serverInfo().origin, `/?room=${name}`);
    const b = await openApp(cb, serverInfo().origin, `/?room=${name}`);
    for (const app of [a, b]) {
      await app.until("the room is joined", async () => {
        const st = await app.exec("collab.status");
        return st.state === "connected" && st.room === name;
      });
    }
    await use({ a, b, name });
    expect(a.consoleErrors, "page A logged errors").toEqual([]);
    expect(b.consoleErrors, "page B logged errors").toEqual([]);
    await ca.close();
    await cb.close();
  },
});

async function names(app: App): Promise<string[]> {
  const rows = (await app.exec("element.query", { fields: ["name"], limit: 500 })).items as { name?: string }[];
  return rows.map((r) => r.name).filter((n): n is string => Boolean(n)).sort();
}

test("collab: an element drawn on one page appears on the other, both ways", async ({ room }) => {
  const { a, b } = room;
  await box(a, 0, 0, 80, 50, { name: "from-a" });
  await b.until("page B sees from-a", async () => (await names(b)).includes("from-a"));
  await box(b, 200, 0, 80, 50, { kind: "ellipse", name: "from-b" });
  await a.until("page A sees from-b", async () => (await names(a)).includes("from-b"));
  expect(await names(a)).toEqual(["from-a", "from-b"]);
  expect(await names(b)).toEqual(["from-a", "from-b"]);
});

test("collab: presence shows the peer, and edits on both sides converge", async ({ room }) => {
  const { a, b } = room;
  await a.exec("collab.set_user", { name: "Ada", color: "#ef4444" });
  await b.exec("collab.set_user", { name: "Bo", color: "#3b82f6" });
  await a.until("A lists Bo", async () => {
    const st = await a.exec("collab.status");
    return (st.peers as { name?: string }[]).some((p) => p.name === "Bo");
  });
  await b.until("B lists Ada", async () => {
    const st = await b.exec("collab.status");
    return (st.peers as { name?: string }[]).some((p) => p.name === "Ada");
  });

  const left = await box(a, 0, 0, 100, 60, { name: "left" });
  await b.until("B has left", async () => (await names(b)).includes("left"));
  const right = await box(b, 400, 0, 100, 60, { name: "right" });
  await a.until("A has right", async () => (await names(a)).includes("right"));

  // A real mouse move on page A is published as A's pointer, in world coordinates.
  await a.exec("view.set", { scroll: [0, 0], zoom: 1 });
  await b.exec("view.set", { scroll: [0, 0], zoom: 1 });
  const over = await a.toPage({ x: 300, y: 250 });
  await a.page.mouse.move(over.x - 20, over.y - 20);
  await a.page.mouse.move(over.x, over.y, { steps: 6 });
  await b.until("B sees A's pointer near (300, 250)", async () => {
    const peers = (await b.exec("collab.status")).peers as { name?: string; pointer?: [number, number] | { x: number; y: number } | null }[];
    const p = peers.find((x) => x.name === "Ada")?.pointer;
    if (!p) return false;
    const [px, py] = Array.isArray(p) ? p : [p.x, p.y];
    return Math.abs(px - 300) < 40 && Math.abs(py - 250) < 40;
  });

  // Each side moves its own shape at the same time; both end up with both moves.
  await Promise.all([a.exec("element.move", { ids: [left], dx: 0, dy: 100, snap: "none" }), b.exec("element.move", { ids: [right], dx: 0, dy: 200, snap: "none" })]);
  const settled = async (app: App) => {
    const rows = await app.elements([left, right]);
    return rows[left]?.bounds.y === 100 && rows[right]?.bounds.y === 200;
  };
  await a.until("A has both moves", () => settled(a));
  await b.until("B has both moves", () => settled(b));

  // Undo is local: A undoes its own move and B's move stays.
  await a.exec("history.undo");
  await b.until("B sees A's undo", async () => (await b.element(left)).bounds.y === 0);
  expect((await b.element(right)).bounds.y, "B's move is untouched").toBe(200);
});

test("collab: leaving the room keeps the drawing and stops sync", async ({ room }) => {
  const { a, b } = room;
  await box(a, 0, 0, 60, 40, { name: "before-leave" });
  await b.until("B sees it", async () => (await names(b)).includes("before-leave"));
  await b.exec("collab.leave_room");
  expect((await b.exec("collab.status")).room, "B is out of the room").toBeUndefined();
  await box(a, 100, 0, 60, 40, { name: "after-leave" });
  await a.page.waitForTimeout(1500);
  expect(await names(b), "B does not get later edits").not.toContain("after-leave");
  expect(await names(b), "but keeps what it had").toContain("before-leave");
});

test("collab: 0 frames while alone in a room and with a peer (server ping and presence heartbeat included)", async ({ room }) => {
  const { a, b } = room;
  await box(a, 0, 0, 60, 40, { name: "sync" });
  await b.until("B has the shape", async () => (await names(b)).includes("sync"));
  await a.settle(3000, 30_000);
  await b.settle(3000, 30_000);
  const [a0, b0] = [await a.frames(), await b.frames()];
  // The server pings every 20 s and a peer sends presence every 10 s: 22 s covers both.
  await a.page.waitForTimeout(22_000);
  const [a1, b1] = [await a.frames(), await b.frames()];
  expect(a1.frames - a0.frames, `page A redraw reasons ${JSON.stringify((await a.perf()).redraws.by_reason)}`).toBe(0);
  expect(b1.frames - b0.frames, `page B redraw reasons ${JSON.stringify((await b.perf()).redraws.by_reason)}`).toBe(0);
});

test("collab: the dialog offers the access token field, and the token reaches collab.join_room and is kept nowhere", async ({ browser }) => {
  const token = "e2e-room-token-0123456789";
  const server = await startServer(undefined, { authToken: token });
  const room = `e2e-token-${Date.now().toString(36)}`;
  const context = await browser.newContext();
  try {
    const app = await openApp(context, server.origin);
    await app.exec("doc.new");
    // Without the token this server turns a join away, and the app says so.
    await app.exec("collab.connect", { url: "/ws" });
    await app.exec("collab.join_room", { room: `${room}-no-token` });
    await app.until("the server refuses the join", async () => (await app.exec("collab.status")).state === "error");
    await app.exec("collab.leave_room");
    expect((await app.exec("collab.status")).state).toBe("ready");

    // The dialog: the top bar's collaboration button. With the server chosen it offers a room field and, below it,
    // the access token field, then the button. These are the pixels of the fields (what they do with text is in
    // the egui tests: `ui/tests/collab_dialog.rs`).
    await app.page.mouse.click(74, 37, { delay: 120 });
    await app.page.waitForTimeout(600);
    const dialog = await shot(app);
    const field = (y: number) => dialog.at(780, y);
    expect(close(field(386), field(434), 1), "the token field looks like the room field (both at the right end, clear of the hint)").toBe(true);
    expect(close(field(434), dialog.at(470, 434), 1), "and not like the dialog around it").toBe(false);

    // The call the dialog's button makes with a room and a token.
    await app.exec("collab.join_room", { room, token });
    await app.until("the room is joined", async () => {
      const st = await app.exec("collab.status");
      return st.state === "connected" && st.room === room;
    });

    // The ring holds the call without the token; the settings and the page's storage do not hold it either.
    const joins = (await app.recent(50)).filter((e) => e.id === "collab.join_room");
    expect(joins.length, "collab.join_room is in the ring").toBeGreaterThan(0);
    expect(JSON.stringify(await app.recent(200)), "the ring never holds the token").not.toContain(token);
    expect(JSON.stringify(await app.exec("settings.get")), "no setting holds the token").not.toContain(token);
    const stored = await app.page.evaluate(() => {
      const out: string[] = [];
      for (let i = 0; i < localStorage.length; i++) out.push(localStorage.getItem(localStorage.key(i) ?? "") ?? "");
      return out.join("\n");
    });
    expect(stored, "localStorage does not hold the token").not.toContain(token);
    expect(app.consoleErrors, "the page logged errors").toEqual([]);
  } finally {
    await context.close();
    await server.stop();
  }
});
