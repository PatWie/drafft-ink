#!/usr/bin/env python3
"""Browser smoke test of drafftink-canvas: headed Chromium on a private Xvfb, WebGPU on the NVIDIA adapter.

Mounts a Stage, adds 1000 shapes, clicks one and receives the event, drags it, pans, checks that an idle stage
draws no frame and schedules no animation frame for 5 s (also while the pointer moves over empty space), exports
JSON, SVG and PNG and loads the PNG back, checks that a listener can change the stage, that a missing WebGPU is a
clear error, and takes a screenshot. Headless Chromium cannot show a WebGPU canvas (docs/stack.md section 8), so it
is not used.

usage: ~/.venvs/perf/bin/python test/smoke.py [--display :97] [--port 8137] [--shot test/smoke.png] [--adapter nvidia|swiftshader]
Needs dist/ (scripts/build.sh), python with playwright (and PIL), and Xvfb. Exit code 0 only if every check passes.
"""
import functools
import http.server
import io
import json
import os
import subprocess
import sys
import threading
import time

from PIL import Image
from playwright.sync_api import sync_playwright

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)


def arg(name, default=None):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default


DISPLAY = arg("--display", ":97")
PORT = int(arg("--port", "8137"))
SHOT = arg("--shot", os.path.join(HERE, "smoke.png"))
PROFILE = f"/tmp/drafftink-canvas-chrome-{PORT}"
# WebGPU on the NVIDIA adapter through ANGLE/Vulkan, or on SwiftShader (docs/stack.md section 8).
NVIDIA = ["--enable-unsafe-webgpu", "--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan", "--use-angle=vulkan",
          "--use-vulkan=native", "--ignore-gpu-blocklist", "--disable-vulkan-surface", "--no-sandbox", "--disable-gpu-sandbox"]
SWIFTSHADER = ["--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
               "--use-webgpu-adapter=swiftshader", "--ignore-gpu-blocklist"]
FLAGS = SWIFTSHADER if arg("--adapter", "nvidia") == "swiftshader" else NVIDIA

# Counts what an idle page must not do: schedule animation frames, take a canvas texture, submit GPU work.
COUNTERS = """
window.__counts = { raf: 0, texture: 0, submit: 0 };
const raf = window.requestAnimationFrame.bind(window);
window.requestAnimationFrame = (cb) => { window.__counts.raf++; return raf(cb); };
if (window.GPUCanvasContext) {
  const g = GPUCanvasContext.prototype.getCurrentTexture;
  GPUCanvasContext.prototype.getCurrentTexture = function () { window.__counts.texture++; return g.call(this); };
}
if (window.GPUQueue) {
  const s = GPUQueue.prototype.submit;
  GPUQueue.prototype.submit = function (...a) { window.__counts.submit++; return s.apply(this, a); };
}
"""
NO_WEBGPU = "Object.defineProperty(navigator, 'gpu', { value: undefined, configurable: true });"

failures, report = [], {}


def check(name, ok, detail=""):
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  ({detail})" if detail else ""), flush=True)
    if not ok:
        failures.append(name)


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


server = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), functools.partial(Quiet, directory=ROOT))
threading.Thread(target=server.serve_forever, daemon=True).start()
xvfb = subprocess.Popen(["Xvfb", DISPLAY, "-screen", "0", "1500x1000x24"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.5)
os.environ["DISPLAY"] = DISPLAY
BASE = f"http://127.0.0.1:{PORT}"


def js(page, code, *args):
    return page.evaluate(code, *args) if args else page.evaluate(code)


def counts(page):
    return js(page, "() => ({ ...window.__counts, frames: window.stage.framesDrawn })")


def screen_of(page, world):
    """Viewport pixel of a world point."""
    return js(page, "(p) => { const r = stage.canvas.getBoundingClientRect(); const s = stage.worldToScreen(p); return [r.left + s[0], r.top + s[1]]; }", world)


try:
    with sync_playwright() as pw:
        ctx = pw.chromium.launch_persistent_context(PROFILE, headless=False, viewport={"width": 1400, "height": 860}, args=FLAGS)
        ctx.add_init_script(COUNTERS)
        page = ctx.pages[0] if ctx.pages else ctx.new_page()
        logs = []
        page.on("console", lambda m: logs.append((m.type, m.text)))
        page.on("pageerror", lambda e: logs.append(("pageerror", str(e))))

        # ---- start, 1000 shapes -------------------------------------------------------------------------
        t0 = time.time()
        page.goto(f"{BASE}/examples/index.html?n=1000")
        page.wait_for_function("() => window.__ready || window.__error", timeout=60000)
        error = js(page, "() => window.__error || null")
        check("the stage starts on WebGPU", error is None, error or f"{time.time() - t0:.2f} s from navigation to ready")
        if error:
            raise SystemExit(1)
        time.sleep(1.0)
        stats = js(page, "() => stage.stats()")
        report["adapter"] = stats["adapter"]
        check("1000 shapes are in the scene", js(page, "() => stage.count") == 1000)
        check("a frame was drawn", stats["framesDrawn"] >= 1, f"{stats['framesDrawn']} frames; adapter {stats['adapter']!r}")
        last = stats["lastFrame"]
        check("culling draws only what is visible", 0 < last["drawn"] <= 1000, f"{last}")
        cam = js(page, "() => stage.camera")
        report["zoom"] = cam["zoom"]

        # ---- a screenshot that is not blank -----------------------------------------------------------
        png = page.screenshot()
        img = Image.open(io.BytesIO(png)).convert("RGB")
        colors = len({img.getpixel((x, y)) for x in range(0, img.width, 7) for y in range(60, img.height, 7)})
        check("the screenshot shows the drawing", colors > 20, f"{colors} distinct colours")
        os.makedirs(os.path.dirname(SHOT), exist_ok=True)
        img.save(SHOT)

        # ---- click: the listener gets the event, the shape is selected ----------------------------------
        js(page, """() => {
          window.__clicks = [];
          stage.on('click', '#s500', (e) => window.__clicks.push(['s500', e.target, e.currentTarget.kind]));
          stage.on('click', (e) => window.__clicks.push(['stage', e.target, e.currentTarget.kind]));
        }""")
        # s500 is the rectangle at world (1800, 1350), 70 x 50.
        cx, cy = screen_of(page, [1800 + 35, 1350 + 25])
        page.mouse.move(cx, cy)
        page.mouse.down()
        page.mouse.up()
        time.sleep(0.3)
        clicks = js(page, "() => window.__clicks")
        check("the click reaches the shape's listener, then the stage's", clicks == [["s500", "s500", "element"], ["stage", "s500", "stage"]], str(clicks))
        check("the click selected the shape", js(page, "() => stage.selection") == ["s500"])
        check("a hit test at the point finds it", js(page, "([x, y]) => { const r = stage.canvas.getBoundingClientRect(); return stage.hit(x - r.left, y - r.top)?.id; }", [cx, cy]) == "s500")

        # ---- drag moves the shape by the pointer's distance in world units ---------------------------
        # (A selected shape this small on screen is covered by its own handles, so the drag starts from nothing selected.)
        js(page, "() => stage.setSelection([])")
        before = js(page, "() => stage.get('s500').transform")
        zoom = js(page, "() => stage.camera.zoom")
        page.mouse.move(cx, cy)
        page.mouse.down()
        page.mouse.move(cx + 60, cy + 30, steps=6)
        page.mouse.up()
        time.sleep(0.3)
        after = js(page, "() => stage.get('s500').transform")
        dx, dy = after["x"] - before["x"], after["y"] - before["y"]
        check("a drag moves the shape", abs(dx - 60 / zoom) < 1e-6 and abs(dy - 30 / zoom) < 1e-6, f"moved ({dx:.3f}, {dy:.3f}), expected ({60 / zoom:.3f}, {30 / zoom:.3f})")
        js(page, "() => stage.setSelection([])")

        # ---- pan with the hand tool and with the wheel -------------------------------------------------
        js(page, "() => stage.setPointerMode('hand')")
        c0 = js(page, "() => stage.camera")
        page.mouse.move(300, 400)
        page.mouse.down()
        page.mouse.move(420, 460, steps=8)
        page.mouse.up()
        time.sleep(0.3)
        c1 = js(page, "() => stage.camera")
        check("the hand tool pans", abs((c1["scroll"][0] - c0["scroll"][0]) + 120 / zoom) < 1e-6 and abs((c1["scroll"][1] - c0["scroll"][1]) + 60 / zoom) < 1e-6,
              f"{c0['scroll']} -> {c1['scroll']}")
        js(page, "() => stage.setPointerMode('select')")
        page.mouse.move(700, 400)
        page.mouse.wheel(0, 100)
        time.sleep(0.4)
        c2 = js(page, "() => stage.camera")
        check("the wheel pans", c2["scroll"][1] > c1["scroll"][1], f"{c1['scroll'][1]:.2f} -> {c2['scroll'][1]:.2f}")

        # ---- a listener may change the stage ------------------------------------------------------------
        js(page, """() => {
          window.__busy = null;
          stage.on('click', '#s4', (e) => {
            stage.update('s4', { style: { fill_color: '#ff0000' } });
            try { stage.get('s4'); window.__busy = 'no error'; } catch (err) { window.__busy = err.message; }
          });
        }""")
        sx, sy = screen_of(page, [4 % 32 * 90 + 35, 0 + 25])
        # s4 may be off screen after the pan: bring it to the middle first.
        js(page, "() => stage.fitToContent({ ids: ['s4'], padding: 400 })")
        time.sleep(0.3)
        sx, sy = screen_of(page, [4 % 32 * 90 + 35, 25])
        page.mouse.click(sx, sy)
        time.sleep(0.4)
        check("a listener's change is applied", js(page, "() => stage.get('s4').style.fill_color") == "#ff0000")
        busy = js(page, "() => window.__busy")
        check("a read inside a listener says why it cannot answer", busy and "busy running a listener" in busy, str(busy))

        # ---- idle: no frame, no animation frame, no GPU work -------------------------------------------
        js(page, "() => stage.fitToContent({ padding: 40 })")
        time.sleep(1.5)
        js(page, "() => stage.setSelection([])")
        time.sleep(1.0)
        c_before = counts(page)
        time.sleep(5.0)
        c_after = counts(page)
        diff = {k: c_after[k] - c_before[k] for k in c_after}
        check("an idle stage draws 0 frames in 5 s", diff["frames"] == 0, str(diff))
        check("an idle stage schedules no animation frame", diff["raf"] == 0, str(diff))
        check("an idle stage touches no canvas texture and submits no GPU work", diff["texture"] == 0 and diff["submit"] == 0, str(diff))
        # Hovering over empty space (the margin left of the content) changes nothing either.
        page.mouse.move(8, 100)
        time.sleep(0.5)
        c_before = counts(page)
        for i in range(150):
            page.mouse.move(8 + (i % 20), 100 + i)
        time.sleep(1.0)
        c_after = counts(page)
        diff = {k: c_after[k] - c_before[k] for k in c_after}
        check("150 pointer moves over empty space draw 0 frames", diff["frames"] == 0 and diff["raf"] == 0, str(diff))
        # One change is exactly one frame.
        c_before = counts(page)
        js(page, "() => stage.update('s0', { x: 5 })")
        time.sleep(0.6)
        c_after = counts(page)
        check("one change draws exactly one frame", c_after["frames"] - c_before["frames"] == 1, f"{c_after['frames'] - c_before['frames']} frames, {c_after['raf'] - c_before['raf']} animation frames")
        report["idle"] = diff

        # ---- timings ---------------------------------------------------------------------------------
        timing = js(page, """() => {
          const t = (f, n) => { const s = performance.now(); for (let i = 0; i < n; i++) f(i); return (performance.now() - s) / n; };
          const r = stage.canvas.getBoundingClientRect();
          return { hit_ms: t((i) => stage.hit(200 + (i % 500), 300 + (i % 300)), 2000),
                   update_ms: t((i) => stage.update('s' + (i % 1000), { x: i }), 500) };
        }""")
        report["timing"] = timing
        print(f"      per call: hit {timing['hit_ms'] * 1000:.1f} us, update {timing['update_ms'] * 1000:.1f} us (1000 shapes)")

        # ---- export and import -------------------------------------------------------------------------
        info = js(page, """async () => {
          const json = stage.exportJSON();
          const svg = stage.toSVG({ embedScene: false });
          const png = await stage.toPNG({ scale: 0.25 });
          return { doc: JSON.parse(json).elements.length, svg: svg.slice(0, 40), svgPaths: svg.split('<path').length - 1,
                   png: Array.from(png) };
        }""")
        check("JSON export holds the 1000 shapes", info["doc"] == 1000)
        check("SVG export is an SVG with paths", info["svg"].startswith("<svg") and info["svgPaths"] > 500, f"{info['svgPaths']} paths")
        png_bytes = bytes(info["png"])
        check("PNG export is a PNG", png_bytes[:8] == b"\x89PNG\r\n\x1a\n", f"{len(png_bytes)} bytes")
        exported = Image.open(io.BytesIO(png_bytes)).convert("RGB")
        ecolors = len({exported.getpixel((x, y)) for x in range(0, exported.width, 3) for y in range(0, exported.height, 3)})
        check("the PNG shows the drawing", exported.width > 100 and ecolors > 10, f"{exported.width}x{exported.height}, {ecolors} colours")
        count_back = js(page, """async (bytes) => { stage.clear(); const empty = stage.count; stage.load(new Uint8Array(bytes)); return [empty, stage.count]; }""", info["png"])
        check("loading the PNG brings the scene back", count_back == [0, 1000], str(count_back))
        time.sleep(0.5)
        c_before = counts(page)
        time.sleep(2.0)
        check("the stage is idle again after the export and the load", counts(page)["frames"] == c_before["frames"])

        # ---- errors the page would see --------------------------------------------------------------
        err = js(page, """() => { try { stage.add({ type: 'rectangle', colour: 'red' }); return null; } catch (e) { return e.message; } }""")
        check("a typo in a shape is a plain-words error", err and "colour" in err, str(err))
        check("a refused shape leaves the scene alone", js(page, "() => stage.count") == 1000)

        # ---- no WebGPU ------------------------------------------------------------------------------
        bare = ctx.new_page()
        bare.add_init_script(NO_WEBGPU)
        bare.goto(f"{BASE}/examples/index.html?n=3")
        bare.wait_for_function("() => window.__ready || window.__error", timeout=30000)
        message = js(bare, "() => window.__error || ''")
        check("without WebGPU the stage says so", "no WebGPU" in message, message)
        shown = bare.evaluate("""async () => {
          const { Stage } = await import('/src/index.js');
          try { await Stage.create(document.createElement('canvas'), { errorPage: true }); } catch (e) {}
          return document.body.innerText;
        }""")
        check("the optional error page explains which browsers work", "Browsers that work" in shown, shown[:60].replace("\n", " "))
        bare.close()

        bad = [l for l in logs if l[0] in ("error", "pageerror") and "listener" not in l[1] and "colour" not in l[1]]
        check("no console errors", not bad, str(bad[:3]))
        warnings = [l for l in logs if l[0] == "warning"]
        if warnings:
            print("      warnings:", [w[1][:100] for w in warnings[:5]])
        ctx.close()
finally:
    server.shutdown()
    xvfb.terminate()
    xvfb.wait()

print(json.dumps(report))
print(f"\n{'FAILED: ' + ', '.join(failures) if failures else 'all checks passed'}  (screenshot: {SHOT})")
sys.exit(1 if failures else 0)
