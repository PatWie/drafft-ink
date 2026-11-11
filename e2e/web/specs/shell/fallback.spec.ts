import { test as base, expect, openApp, serverInfo, type App } from "../../support/app.ts";
import { box, camera } from "../../support/build.ts";
import { close, hex, shot, type Rgb } from "../../support/pixels.ts";

// The browser has no WebGPU: `navigator.gpu` is removed before the page loads, so the page must pick the WebGL2 module
// (docs/web-app.md, "Browsers without WebGPU"). Everything else is the same app, so these specs draw with the same
// commands as pixels.spec.ts and read the pixels.
const WHITE: Rgb = [255, 255, 255];
const TOL = 24;

/** A page opened without `navigator.gpu`, and the paths it and its service worker requested. */
interface Fallback {
  app: App;
  requests: string[];
}

const test = base.extend<{ fallback: Fallback }>({
  fallback: async ({ context }, use) => {
    await context.addInitScript(() => {
      Object.defineProperty(Navigator.prototype, "gpu", { get: () => undefined, configurable: true });
    });
    const requests: string[] = [];
    context.on("request", (r) => requests.push(new URL(r.url()).pathname));
    const app = await openApp(context, serverInfo().origin);
    await app.exec("doc.new");
    await use({ app, requests });
    expect(app.consoleErrors, "the page logged errors").toEqual([]);
  },
});

test.describe("fallback: no WebGPU in the browser", () => {
  test("the page loads the WebGL2 module only, draws with WebGL2 and shows the banner", async ({ fallback }) => {
    const gl = fallback.app;
    expect(await gl.page.evaluate(() => (window as unknown as { __renderer?: string }).__renderer)).toBe("webgl");
    const canvas = await gl.page.evaluate(() => {
      const c = document.getElementById("canvas") as HTMLCanvasElement;
      return { webgl2: c.getContext("webgl2") !== null, webgpu: c.getContext("webgpu") !== null };
    });
    expect(canvas, "the canvas has a WebGL2 context and no WebGPU one").toEqual({ webgl2: true, webgpu: false });
    const paths = fallback.requests;
    // The page and the service worker (which stores the shell) each ask for the fallback module; nobody asks for the other.
    expect([...new Set(paths.filter((p) => p.endsWith(".wasm")))], "only the fallback module is fetched").toEqual(["/pkg-gl/drafftink_web_bg.wasm"]);
    expect(paths.some((p) => p.startsWith("/pkg/")), "nothing of the WebGPU module is fetched").toBe(false);
    const banner = gl.page.locator("#compat");
    await expect(banner).toContainText("Compatibility mode");
    await banner.getByRole("button", { name: "Dismiss" }).click();
    await expect(banner).toHaveCount(0);
  });

  test("a filled rectangle and an ellipse are drawn in their colours, undo and delete clear them", async ({ fallback }) => {
    const gl = fallback.app;
    await camera(gl, { x: 640, y: 400 }, 1);
    const rect = await box(gl, 340, 340, 200, 120, { fill: "#ff0000" });
    await box(gl, 740, 340, 200, 120, { kind: "ellipse", fill: "#00aa00" });
    await gl.exec("selection.clear");
    const s = await shot(gl);
    expect(s.share({ x: 360, y: 360, w: 160, h: 80 }, hex("#ff0000"), TOL), "the rectangle is red inside").toBeGreaterThan(0.95);
    expect(close(s.at(840, 400), hex("#00aa00"), TOL), `the ellipse centre is ${s.at(840, 400)}`).toBe(true);
    for (const [x, y] of [[748, 348], [932, 348], [748, 452], [932, 452]] as const) {
      expect(close(s.at(x, y), WHITE, TOL), `corner (${x}, ${y}) of the ellipse box is ${s.at(x, y)}`).toBe(true);
    }
    expect(s.share({ x: 100, y: 600, w: 200, h: 100 }, WHITE, TOL), "away from the shapes the page is white").toBeGreaterThan(0.95);

    await gl.exec("element.delete", { ids: [rect] });
    expect((await shot(gl)).share({ x: 360, y: 360, w: 160, h: 80 }, WHITE, TOL), "deleted").toBeGreaterThan(0.95);
    await gl.exec("history.undo");
    expect((await shot(gl)).share({ x: 360, y: 360, w: 160, h: 80 }, hex("#ff0000"), TOL), "undone").toBeGreaterThan(0.95);
  });

  test("zooming grows a shape by the zoom factor and the background colour follows", async ({ fallback }) => {
    const gl = fallback.app;
    await camera(gl, { x: 640, y: 400 }, 1);
    await box(gl, 590, 370, 100, 60, { fill: "#ff0000" });
    await gl.exec("selection.clear");
    const red = (s: Awaited<ReturnType<typeof shot>>) => s.count({ x: 300, y: 200, w: 680, h: 400 }, hex("#ff0000"), TOL);
    const before = red(await shot(gl));
    await gl.exec("view.zoom_at", { factor: 2, at: "viewport_center" });
    const after = red(await shot(gl));
    expect(before).toBeGreaterThan(4000);
    expect(after / before, "twice the zoom is four times the area").toBeGreaterThan(3.4);
    expect(after / before).toBeLessThan(4.6);

    await gl.exec("doc.set_background", { color: "#202020" });
    const dark = await shot(gl);
    expect(dark.share({ x: 800, y: 500, w: 200, h: 150 }, hex("#202020"), TOL), "the background colour is drawn").toBeGreaterThan(0.85);
  });

  test("text is drawn as glyphs: dark pixels in its box, none outside", async ({ fallback }) => {
    const gl = fallback.app;
    await camera(gl, { x: 640, y: 400 }, 1);
    await gl.exec("element.create", { kind: "text", at: [400, 380], text: "Hello WebGL", style: { stroke_color: "#000000" } });
    await gl.exec("selection.clear");
    const s = await shot(gl);
    const dark = (r: { x: number; y: number; w: number; h: number }) => s.count(r, [0, 0, 0], 90);
    expect(dark({ x: 400, y: 380, w: 200, h: 40 }), "glyph pixels inside the text box").toBeGreaterThan(150);
    expect(dark({ x: 400, y: 520, w: 200, h: 60 }), "none below it").toBe(0);
  });

  test("a screenshot command returns a PNG drawn without WebGPU", async ({ fallback }) => {
    const gl = fallback.app;
    await camera(gl, { x: 640, y: 400 }, 1);
    await box(gl, 540, 340, 200, 120, { fill: "#3366ff" });
    await gl.exec("selection.clear");
    const picture = await gl.exec("view.screenshot", { background: true, max_side_px: 640 });
    const png = Buffer.from(picture.bytes.base64, "base64");
    expect([picture.width, picture.height], "half the viewport").toEqual([640, 400]);
    expect(png.subarray(1, 4).toString("latin1"), "a PNG signature").toBe("PNG");
    const { PNG } = await import("pngjs");
    const image = PNG.sync.read(png);
    const at = (x: number, y: number): Rgb => {
      const o = (Math.floor(y) * image.width + Math.floor(x)) * 4;
      return [image.data[o] ?? 0, image.data[o + 1] ?? 0, image.data[o + 2] ?? 0];
    };
    // The picture is the viewport at half size: the rectangle is at x 270..370, y 170..230.
    expect(close(at(320, 200), hex("#3366ff"), TOL), `inside is ${at(320, 200)}`).toBe(true);
    expect(close(at(40, 40), WHITE, TOL), `outside is ${at(40, 40)}`).toBe(true);
  });

  test("a busy drawing still pans: frame time with 600 shapes and 200 labels", async ({ fallback }, testInfo) => {
    const gl = fallback.app;
    const steps = [];
    for (let i = 0; i < 600; i++) {
      const x = (i % 30) * 120;
      const y = Math.floor(i / 30) * 90;
      steps.push({ id: "element.create" as const, args: { kind: i % 3 === 0 ? ("ellipse" as const) : ("rectangle" as const), bounds: { x, y, w: 100, h: 70 }, select: false } });
    }
    for (let i = 0; i < 200; i++) {
      steps.push({ id: "element.create" as const, args: { kind: "text" as const, at: [(i % 20) * 180 + 5, Math.floor(i / 20) * 150 + 20] as [number, number], text: `label ${i} the quick brown fox`, select: false } });
    }
    await gl.batch(steps);
    await gl.exec("view.set", { scroll: [1700, 700], zoom: 0.55 });
    await gl.settle(1500, 60_000);

    // A pan is a command here; each one draws one frame while it moves. Time from the command to the frame.
    const times: number[] = await gl.page.evaluate(async () => {
      const api = window.drafftink;
      const out: number[] = [];
      for (let i = 0; i < 40; i++) {
        const before = api.frames().frames;
        const t0 = performance.now();
        await api.execute("view.pan", { dx: i % 2 === 0 ? 60 : -60, dy: 0 });
        while (api.frames().frames === before) await new Promise((r) => setTimeout(r, 0));
        out.push(performance.now() - t0);
      }
      return out;
    });
    const sorted = [...times].sort((a, b) => a - b);
    const median = sorted[Math.floor(sorted.length / 2)] ?? 0;
    const p95 = sorted[Math.floor(sorted.length * 0.95)] ?? 0;
    testInfo.annotations.push({ type: "frame time", description: `median ${median.toFixed(1)} ms, p95 ${p95.toFixed(1)} ms, over ${times.length} pans of 800 elements` });
    console.log(`fallback frame time: median ${median.toFixed(1)} ms, p95 ${p95.toFixed(1)} ms (800 elements, 40 pans)`);
    // A regression guard, not a target: the measured median is far below it.
    expect(median, "median frame time of a pan in the WebGL2 renderer").toBeLessThan(250);
    const s = await shot(gl);
    expect(s.share({ x: 0, y: 0, w: 1280, h: 800 }, WHITE, 6), "the settled frame is drawn in full, not left as bars").toBeGreaterThan(0.2);
  });
});
