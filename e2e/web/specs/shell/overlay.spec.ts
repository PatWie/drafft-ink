import { test, expect, openApp, serverInfo, type App } from "../../support/app.ts";
import { box, camera } from "../../support/build.ts";
import { hex, shot, type Region, type Rgb } from "../../support/pixels.ts";

/** The selection blue of the overlay (`OverlayStyle::accent`). */
const ACCENT: Rgb = [59, 130, 246];
const TOL = 40;
/** A 1 px line that falls between two pixel rows is a blend of the accent and the page; this reaches it. */
const LINE_TOL = 110;

/** Two pages with separate storage, in one room of the server that serves the page. */
const twoPages = test.extend<{ room: { a: App; b: App } }>({
  room: async ({ browser }, use) => {
    const name = `e2e-overlay-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    const [ca, cb] = [await browser.newContext(), await browser.newContext()];
    const a = await openApp(ca, serverInfo().origin, `/?room=${name}`);
    const b = await openApp(cb, serverInfo().origin, `/?room=${name}`);
    for (const app of [a, b]) {
      await app.until("the room is joined", async () => {
        const st = await app.exec("collab.status");
        return st.state === "connected" && st.room === name;
      });
    }
    await use({ a, b });
    expect(a.consoleErrors, "page A logged errors").toEqual([]);
    expect(b.consoleErrors, "page B logged errors").toEqual([]);
    await ca.close();
    await cb.close();
  },
});

type Peer = { name?: string; pointer?: [number, number] | { x: number; y: number } | null; selection?: string[] };

async function peers(app: App): Promise<Peer[]> {
  return (await app.exec("collab.status")).peers as Peer[];
}

twoPages("overlay: a peer's selection outline and cursor label are drawn in the peer's colour", async ({ room }) => {
  const { a, b } = room;
  const red = "#ef4444";
  await a.exec("collab.set_user", { name: "Ada", color: red });
  await b.exec("collab.set_user", { name: "Bo", color: "#3b82f6" });
  await b.until("B lists Ada", async () => (await peers(b)).some((p) => p.name === "Ada"));
  for (const app of [a, b]) await camera(app, { x: 640, y: 400 }, 1);

  const target = await box(a, 540, 340, 200, 120, { fill: "#ffffff" });
  await b.until("B has the shape", async () => (await b.count()) === 1);
  await a.exec("selection.clear");
  await b.exec("selection.clear");

  // A box 4 px outside the shape on the left, 2 px wide, in the peer's colour.
  const strip: Region = { x: 530, y: 350, w: 8, h: 100 };
  const before = (await shot(b)).count(strip, hex(red), TOL);
  expect(before, "no outline before A selects the shape").toBe(0);

  await a.exec("selection.select", { ids: [target] });
  await b.until("B sees A's selection", async () => (await peers(b)).some((p) => p.name === "Ada" && p.selection?.includes(target)));
  const outlined = (await shot(b)).count(strip, hex(red), TOL);
  expect(outlined, "the outline of A's selection is drawn on page B").toBeGreaterThan(100);

  // The outline goes when A lets go.
  await a.exec("selection.clear");
  await b.until("B sees the selection gone", async () => (await peers(b)).every((p) => !p.selection?.length));
  expect((await shot(b)).count(strip, hex(red), TOL), "and is gone").toBe(0);

  // A's pointer is a red arrow with a red chip that carries the name, beside it.
  const at = await a.toPage({ x: 900, y: 600 });
  await a.page.mouse.move(at.x - 30, at.y - 30);
  await a.page.mouse.move(at.x, at.y, { steps: 6 });
  await b.until("B sees A's pointer", async () => {
    const p = (await peers(b)).find((x) => x.name === "Ada")?.pointer;
    if (!p) return false;
    const [px, py] = Array.isArray(p) ? p : [p.x, p.y];
    return Math.abs(px - 900) < 40 && Math.abs(py - 600) < 40;
  });
  // The name chip sits at (+14, +16) from the tip of the pointer and is about 32 x 19 px.
  const chip: Region = { x: 910, y: 612, w: 30, h: 17 };
  const tip: Region = { x: 890, y: 590, w: 40, h: 40 };
  await b.until("the chip is on page B's screen", async () => (await shot(b)).count(chip, hex(red), TOL) > 150);
  // The laser trail of the pointer fades within 1.5 s; the pointer and its chip stay, drawn by a frame of their own.
  await b.page.waitForTimeout(2000);
  const s = await shot(b);
  expect(s.count(chip, hex(red), TOL), "the name chip is filled in A's colour").toBeGreaterThan(150);
  expect(s.count(tip, hex(red), TOL), "and the pointer is there").toBeGreaterThan(40);
  // The chip is not the pointer alone: on A itself nothing is drawn at A's own pointer.
  expect((await shot(a)).count(chip, hex(red), TOL), "page A does not draw its own pointer").toBe(0);
});

test("overlay: the lasso path shows while dragging and goes when the button is released", async ({ app }) => {
  await camera(app, { x: 640, y: 400 }, 1);
  await app.exec("tool.set", { tool: "lasso" });
  const region: Region = { x: 300, y: 200, w: 680, h: 400 };
  const idle = (await shot(app)).count(region, ACCENT, LINE_TOL);

  const corners = [
    { x: 500, y: 300 },
    { x: 780, y: 300 },
    { x: 780, y: 500 },
    { x: 500, y: 500 },
  ];
  const [first, ...rest] = await Promise.all(corners.map((c) => app.toPage(c)));
  if (!first) throw new Error("no corner");
  await app.page.mouse.move(first.x, first.y);
  await app.page.mouse.down();
  for (const to of [...rest, first]) await app.page.mouse.move(to.x, to.y, { steps: 10 });
  // Still held: the path is on screen. Its outline is a 1 px accent line round a rectangle 280 x 200.
  await app.page.mouse.move(first.x + 2, first.y + 2, { steps: 2 });
  const during = (await shot(app)).count(region, ACCENT, LINE_TOL);
  expect(during - idle, "the lasso outline is drawn while the button is down").toBeGreaterThan(500);
  // Along the top edge, away from the corners.
  const top = (await shot(app)).count({ x: 560, y: 297, w: 160, h: 7 }, ACCENT, LINE_TOL);
  expect(top, "the path runs along the top edge").toBeGreaterThan(100);

  await app.page.mouse.up();
  await app.settle();
  const after = (await shot(app)).count(region, ACCENT, LINE_TOL);
  expect(after, "the path is gone after the release").toBeLessThan(idle + 50);
});

test("overlay: a link icon is drawn on an unselected element and a click on it asks the browser to open the address", async ({ app, context }) => {
  await camera(app, { x: 640, y: 400 }, 1);
  const url = "https://example.com/drafftink-link";
  await context.route("https://example.com/**", (route) => route.fulfill({ contentType: "text/html", body: "<title>linked</title>" }));
  const id = await box(app, 540, 340, 200, 120, { fill: "#ffffff" });
  await app.exec("element.set_link", { id, link: url });
  await app.exec("selection.clear");

  // The icon sits beside the top-right corner: a square of 12 px centred about (748, 332).
  const icon: Region = { x: 740, y: 324, w: 16, h: 16 };
  const shown = (await shot(app)).count(icon, ACCENT, TOL);
  expect(shown, "the chain glyph is drawn in the accent colour").toBeGreaterThan(5);
  await app.exec("selection.select", { ids: [id] });
  const hidden = (await shot(app)).count(icon, ACCENT, TOL);
  expect(hidden, "a selected element shows its handles there instead").toBeLessThan(shown);
  await app.exec("selection.clear");

  const click = await app.toPage({ x: 748, y: 332 });
  const popup = context.waitForEvent("page", { timeout: 10_000 });
  await app.page.mouse.click(click.x, click.y);
  const opened = await popup;
  await opened.waitForLoadState("domcontentloaded");
  expect(opened.url()).toBe(url);
  expect(await app.exec("selection.get"), "the click opened the link and selected nothing").toMatchObject({ ids: [] });
  await opened.close();
});

