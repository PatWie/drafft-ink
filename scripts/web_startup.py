#!/usr/bin/env python3
"""Start-up time and frame time of the web app in headed Chromium on a private Xvfb. Builds nothing.

usage: ~/.venvs/perf/bin/python scripts/web_startup.py [--dist dist] [--runs 5] [--frames 150] [--json out.json]

Needs dist/ (scripts/build-web.sh) and target/release/drafftink-server. Measured in the page, in milliseconds
from the start of the navigation (performance.now()):
  response_end   the wasm download is done (localhost, so this is almost the start of the compile)
  first_frame    window.drafftink.frames().frames first reads above 0 (polled every animation frame)
  warm_first_frame   the same after a reload, with the service worker and the HTTP cache warm
Frame time: 1000 rectangles, then `--frames` view.pan commands, one per animation frame, with spans on. It reports
the app's own frame time (PerfStats frame_ms) and the encode, ui and gpu_submit phases, which do not depend on the
GPU of the machine as much as the wall-clock frame rate does.
"""
import argparse, json, os, socket, statistics, subprocess, tempfile, time, urllib.request
from playwright.sync_api import sync_playwright

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ap = argparse.ArgumentParser()
ap.add_argument("--dist", default=os.path.join(ROOT, "dist"))
ap.add_argument("--server", default=os.path.join(ROOT, "target/release/drafftink-server"))
ap.add_argument("--display", default=":96")
ap.add_argument("--runs", type=int, default=5)
ap.add_argument("--frames", type=int, default=150)
ap.add_argument("--json", default=None)
args = ap.parse_args()

NV = ["--enable-unsafe-webgpu", "--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan", "--use-angle=vulkan", "--use-vulkan=native",
      "--ignore-gpu-blocklist", "--disable-vulkan-surface", "--no-sandbox", "--disable-gpu-sandbox"]

# Records the moment the first frame has been drawn, polled once per animation frame.
INIT = """
window.__tff = null;
(function poll() {
  if (window.drafftink && window.drafftink.frames().frames > 0) { window.__tff = performance.now(); return; }
  requestAnimationFrame(poll);
})();
"""


def free_port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p


def wait_started(pg, secs=60):
    for _ in range(int(secs * 20)):
        if pg.evaluate("()=>window.__started||window.__error||false"):
            return
        time.sleep(0.05)


def startup(pw, base):
    """One cold load in a fresh profile, then one warm reload. Returns the two timelines."""
    prof = tempfile.mkdtemp(prefix="drafftink-startup-")
    ctx = pw.chromium.launch_persistent_context(prof, headless=False, viewport={"width": 1400, "height": 860}, args=NV)
    out = {}
    try:
        pg = ctx.new_page()
        pg.add_init_script(INIT)
        pg.goto(base + "/")
        wait_started(pg)
        time.sleep(1.0)
        info = pg.evaluate("""()=>{
          const wasm = performance.getEntriesByType('resource').find(e => e.name.includes('.wasm'));
          return {first_frame: window.__tff, response_end: wasm ? wasm.responseEnd : null, error: window.__error || null};}""")
        out["cold"] = info
        # Let the service worker finish storing the shell, then reload with everything warm.
        time.sleep(2.0)
        pg.reload()
        wait_started(pg)
        time.sleep(1.0)
        out["warm"] = pg.evaluate("""()=>{
          const wasm = performance.getEntriesByType('resource').find(e => e.name.includes('.wasm'));
          return {first_frame: window.__tff, response_end: wasm ? wasm.responseEnd : null, error: window.__error || null};}""")
        out["page"] = pg
        out["ctx"] = ctx
        return out
    except Exception:
        ctx.close()
        raise


def frame_time(pg, frames):
    """1000 rectangles and a pan of `frames` animation frames."""
    pg.evaluate("""async()=>{
      const steps = [];
      for (let i = 0; i < 1000; i++) steps.push({id: 'element.create', args: {kind: 'rectangle', bounds: {x: (i % 40) * 90, y: Math.floor(i / 40) * 70, w: 70, h: 50}}});
      await drafftink.batch(steps, {label: 'bench', atomic: false});
      await drafftink.execute('view.zoom_to_fit', {scope: 'all'});
      drafftink.perf_enable(true);
      await new Promise(r => setTimeout(r, 500));
      drafftink.perf_reset();
    }""")
    pg.evaluate("""async(n)=>{
      for (let i = 0; i < n; i++) {
        await drafftink.execute('view.pan', {dx: 6 * Math.sin(i / 8), dy: 4 * Math.cos(i / 8)});
        await new Promise(r => requestAnimationFrame(r));
      }
    }""", frames)
    f = pg.evaluate("()=>drafftink.perf()")["frames"]
    keep = {k: v for k, v in f["phase_ms"].items() if k in ("encode", "ui", "gpu_submit", "commands", "layout", "other")}
    pick = lambda d, keys: {k: round(d[k], 2) for k in keys if k in d}
    return {"frames": f["total"],
            "frame_ms": pick(f["frame_ms"], ("count", "mean_ms", "p50_ms", "p95_ms", "max_ms")),
            "phase_p50_ms": {k: round(v.get("p50_ms", 0), 2) for k, v in keep.items()},
            "phase_mean_ms": {k: round(v.get("mean_ms", 0), 2) for k, v in keep.items()}}


def main():
    xvfb = subprocess.Popen(["Xvfb", args.display, "-screen", "0", "1600x1100x24"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.5)
    os.environ["DISPLAY"] = args.display
    port = free_port()
    srv = subprocess.Popen([args.server, "--web-dir", args.dist, "--bind", f"127.0.0.1:{port}", "--storage", "memory"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(100):
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=1); break
        except Exception:
            time.sleep(0.1)
    base = f"http://127.0.0.1:{port}"
    result = {"dist": args.dist, "cold_first_frame_ms": [], "warm_first_frame_ms": [], "response_end_ms": []}
    try:
        with sync_playwright() as pw:
            last = None
            for i in range(args.runs):
                r = startup(pw, base)
                print(json.dumps({k: r[k] for k in ("cold", "warm")}), flush=True)
                if r["cold"]["error"]:
                    raise SystemExit("the app did not start: " + r["cold"]["error"])
                result["cold_first_frame_ms"].append(round(r["cold"]["first_frame"]))
                result["response_end_ms"].append(round(r["cold"]["response_end"] or 0))
                result["warm_first_frame_ms"].append(round(r["warm"]["first_frame"]))
                if i == args.runs - 1:
                    result["frame"] = frame_time(r["page"], args.frames)
                r["ctx"].close()
        med = statistics.median
        result["median"] = {"cold_first_frame_ms": med(result["cold_first_frame_ms"]), "warm_first_frame_ms": med(result["warm_first_frame_ms"]),
                            "response_end_ms": med(result["response_end_ms"])}
        print(json.dumps(result, indent=1))
        if args.json:
            json.dump(result, open(args.json, "w"), indent=1)
    finally:
        srv.terminate(); xvfb.terminate()


main()
