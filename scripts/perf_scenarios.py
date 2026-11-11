#!/usr/bin/env python3
"""Scenario driver (docs/performance.md section 5.3): runs a perf scenario against the real app and writes a ScenarioReport.

  native  starts `drafftink --perf-scenario KEY` under a private Xvfb and delivers real X11 input (XTEST).
  web     opens dist/ in headed Chromium under Xvfb, calls window.drafftink.runScenario(KEY) and delivers real browser
          input (Chromium DevTools input events through Playwright).

The input scripts are the same for both. The app resets its profiler when the scene is on screen and quiet, counts frames,
phases and redraw reasons, and the driver adds meta (adapter, commit) and, for the web, the CPU of the Chromium process tree.

usage: ~/.venvs/perf/bin/python scripts/perf_scenarios.py {native,web} KEY... [--out DIR] [--secs N] [--display :95]
                [--binary target/release/drafftink] [--dist dist] [--server target/release/drafftink-server]
Reports go to DIR/<key>.json (default target/perf/app-<platform>). Exit 0 when every scenario ran and produced a report;
checking against perf/budgets.json is `cargo run -p drafftink-profile --example perf_check -- check ... --timing`."""
import argparse, json, math, os, shutil, socket, subprocess, sys, tempfile, threading, time, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HZ = os.sysconf("SC_CLK_TCK")

# Seconds of each window (docs/performance.md 4.1; the same as ScenarioKind::default_secs in drafftink-profile).
SECS = {"idle": 10, "pan": 10, "wheel": 10, "zoom": 10, "drag": 10, "hover": 8}
# Where the canvas is free of toolbars, in window points: x 100..1250, y 80..720 in a 1280x800 window.
CENTRE = (640, 400)
# `drag` scene: world point (20, 20) of the filled rectangle e0 is at (190, 190) (SceneLook::Corner margin 150, zoom 0.5).
DRAG_GRAB = (190, 190)


def git_commit():
    try:
        return subprocess.check_output(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, text=True).strip()
    except Exception:
        return "unknown"


class Pace:
    """Calls `step(n)` at `rate` per second for `secs`, without drifting."""
    def __init__(self, rate, secs):
        self.rate, self.secs = rate, secs

    def run(self, step):
        t0, n = time.time(), 0
        while time.time() - t0 < self.secs:
            step(n)
            n += 1
            time.sleep(max(t0 + n / self.rate - time.time(), 0))
        return n


def drive(kind, inp, secs):
    """The input of one scenario through `inp` (move, button, wheel, key). Returns the number of events sent."""
    cx, cy = CENTRE
    if kind == "idle":
        time.sleep(secs)
        return 0
    if kind == "pan":
        inp.move(cx, cy); time.sleep(0.2); inp.button(2, True)
        n = Pace(125, secs).run(lambda n: inp.move(int(cx + 200 * math.sin(n / 40)), int(cy + 150 * math.cos(n / 55))))
        inp.button(2, False)
        return n
    if kind == "wheel":
        inp.move(cx, cy); time.sleep(0.2)
        return Pace(100, secs).run(lambda n: inp.wheel(-1 if (n // 50) % 2 == 0 else 1))
    if kind == "zoom":
        inp.move(cx, cy); time.sleep(0.2); inp.key("Control", True)
        n = Pace(60, secs).run(lambda n: inp.wheel(-1 if (n // 30) % 2 == 0 else 1))
        inp.key("Control", False)
        return n
    if kind == "drag":
        gx, gy = DRAG_GRAB
        inp.move(gx, gy); time.sleep(0.3); inp.button(0, True); time.sleep(0.1)
        n = Pace(125, secs).run(lambda n: inp.move(int(gx + 150 * math.sin(n / 40)), int(gy + 100 * math.sin(n / 29))))
        inp.button(0, False)
        return n
    if kind == "hover":
        # A Lissajous curve over the free part of the canvas, 125 moves per second. It crosses shapes now and then,
        # so a few moves change the hover state and most do not.
        inp.move(cx, cy); time.sleep(0.3)
        return Pace(125, secs).run(lambda n: inp.move(int(cx + 480 * math.sin(n / 61)), int(cy + 260 * math.sin(n / 43))))
    raise SystemExit(f"unknown scenario kind {kind}")


# ------------------------------------------------------------------------------------------------ native

class XInput:
    """Real operating system input: X11 XTEST events on a private Xvfb."""
    def __init__(self, display_name):
        from Xlib import X, XK, display
        from Xlib.ext import xtest
        self.X, self.XK, self.xtest = X, XK, xtest
        self.d = display.Display(display_name)
        self.origin = (0, 0)

    def move(self, x, y):
        self.xtest.fake_input(self.d, self.X.MotionNotify, x=self.origin[0] + x, y=self.origin[1] + y); self.d.flush()

    def button(self, b, down):
        # X buttons: 1 left, 2 middle, 3 right. Our scripts say 0 left, 2 middle (DOM numbering).
        xb = {0: 1, 1: 2, 2: 2}.get(b, b)
        self.xtest.fake_input(self.d, self.X.ButtonPress if down else self.X.ButtonRelease, xb); self.d.sync()

    def wheel(self, direction):
        b = 4 if direction < 0 else 5   # a negative delta scrolls the content up: X button 4
        self.xtest.fake_input(self.d, self.X.ButtonPress, b); self.xtest.fake_input(self.d, self.X.ButtonRelease, b); self.d.flush()

    def key(self, name, down):
        code = self.d.keysym_to_keycode(self.XK.string_to_keysym(name + "_L"))
        self.xtest.fake_input(self.d, self.X.KeyPress if down else self.X.KeyRelease, code); self.d.sync()


def start_xvfb(display_name, size="1400x900x24"):
    proc = subprocess.Popen(["Xvfb", display_name, "-screen", "0", size, "+extension", "XTEST"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.5)
    return proc


def run_native(args, keys):
    binary = os.path.abspath(args.binary)
    out_dir = args.out or os.path.join(ROOT, "target/perf/app-native")
    os.makedirs(out_dir, exist_ok=True)
    xvfb = start_xvfb(args.display)
    results = []
    try:
        for key in keys:
            kind = key.split("@")[0]
            secs = args.secs or SECS[kind]
            work = tempfile.mkdtemp(prefix="drafftink-perf-")
            out = os.path.join(out_dir, key + ".json")
            log = open(os.path.join(work, "app.log"), "w+")
            env = dict(os.environ, DISPLAY=args.display, RUST_LOG="drafftink_gpu=info,warn")
            proc = subprocess.Popen([binary, "--data-dir", os.path.join(work, "data"), "--perf-scenario", key, "--perf-out", out],
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, env=env, text=True)
            ready = threading.Event()
            threading.Thread(target=lambda: [ready.set() for l in proc.stdout if "PERF-READY" in l], daemon=True).start()
            t_start = time.time()
            if not ready.wait(300):
                proc.kill(); raise SystemExit(f"{key}: the app did not become ready in 300 s\n" + open(log.name).read()[-2000:])
            startup = time.time() - t_start
            inp = XInput(args.display)
            events = drive(kind, inp, secs)
            proc.stdin.write("stop\n"); proc.stdin.flush()
            try:
                proc.wait(60)
            except subprocess.TimeoutExpired:
                proc.kill(); raise SystemExit(f"{key}: the app did not exit")
            log.flush(); log.seek(0)
            text = log.read()
            adapter = next((l.split("adapter:", 1)[1].strip() for l in text.splitlines() if "adapter:" in l), "unknown")
            finish_report(out, {"adapter": adapter, "commit": git_commit(), "display": "xvfb"},
                          {"input_events": events, "ready_s": round(startup, 2)})
            results.append(out)
            print_summary(out)
            shutil.rmtree(work, ignore_errors=True)
    finally:
        xvfb.terminate()
    return results


# ------------------------------------------------------------------------------------------------ web

class PwInput:
    """Real browser input: Chromium DevTools mouse and key events, which the page receives as trusted events."""
    def __init__(self, page):
        self.page = page

    def move(self, x, y): self.page.mouse.move(x, y)

    def button(self, b, down):
        name = {0: "left", 1: "middle", 2: "middle"}.get(b, "left")
        (self.page.mouse.down if down else self.page.mouse.up)(button=name)

    def wheel(self, direction): self.page.mouse.wheel(0, 100 * direction)

    def key(self, name, down): (self.page.keyboard.down if down else self.page.keyboard.up)(name)


NV = ["--enable-unsafe-webgpu", "--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan", "--use-angle=vulkan", "--use-vulkan=native",
      "--ignore-gpu-blocklist", "--disable-vulkan-surface", "--no-sandbox", "--disable-gpu-sandbox"]
SW = ["--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
      "--use-webgpu-adapter=swiftshader", "--ignore-gpu-blocklist"]


def free_port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p


def tree_ticks(profile_dir):
    total = 0
    for p in os.listdir("/proc"):
        if not p.isdigit(): continue
        try:
            if profile_dir not in open(f"/proc/{p}/cmdline").read(): continue
            f = open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split(); total += int(f[11]) + int(f[12])
        except Exception:
            pass
    return total


def run_web(args, keys):
    from playwright.sync_api import sync_playwright
    out_dir = args.out or os.path.join(ROOT, "target/perf/app-web")
    os.makedirs(out_dir, exist_ok=True)
    dist, server_bin = os.path.abspath(args.dist), os.path.abspath(args.server)
    port = free_port()
    server = subprocess.Popen([server_bin, "--web-dir", dist, "--bind", f"127.0.0.1:{port}", "--storage", "memory"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(100):
        try: urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=1); break
        except Exception: time.sleep(0.1)
    xvfb = start_xvfb(args.display, "1600x1100x24")
    os.environ["DISPLAY"] = args.display
    results = []
    try:
        for key in keys:
            kind = key.split("@")[0]
            secs = args.secs or SECS[kind]
            prof = tempfile.mkdtemp(prefix="drafftink-perf-web-")
            out = os.path.join(out_dir, key + ".json")
            with sync_playwright() as pw:
                ctx = pw.chromium.launch_persistent_context(prof, headless=False, viewport={"width": 1280, "height": 800},
                                                            args=SW if args.adapter == "swiftshader" else NV)
                page = ctx.new_page()
                logs = []
                page.on("console", lambda m: logs.append(f"{m.type}: {m.text[:300]}"))
                page.on("pageerror", lambda e: logs.append("PAGEERROR " + str(e)[:300]))
                page.set_viewport_size({"width": 1280, "height": 800})
                t0 = time.time()
                page.goto(f"http://127.0.0.1:{port}/?perf=1")
                for _ in range(900):
                    if page.evaluate("()=>window.__started||window.__error||false"): break
                    time.sleep(0.1)
                startup = time.time() - t0
                marks = page.evaluate("""()=>({...(window.__marks||{}), ...Object.fromEntries(performance.getEntriesByType('mark')
                    .map(m=>[m.name.replace('drafftink:',''), Math.round(m.startTime)]))})""")
                if page.evaluate("()=>window.__error||null"): raise SystemExit(f"{key}: page error {page.evaluate('()=>window.__error')}")
                info = page.evaluate("async(k)=>await window.drafftink.runScenario(k)", key)
                c0, w0 = tree_ticks(prof), time.time()
                events = drive(kind, PwInput(page), secs)
                elapsed = time.time() - w0
                cpu = (tree_ticks(prof) - c0) / HZ / elapsed * 100
                report = page.evaluate("async()=>await window.drafftink.scenarioReport()")
                adapter = page.evaluate("async()=>{try{const a=await navigator.gpu.requestAdapter();return a.info?`${a.info.vendor}/${a.info.architecture}`:'unknown'}catch(e){return 'unknown'}}")
                browser = ctx.browser.version if ctx.browser else "chromium"
                ctx.close()
            report.setdefault("external", {}).update({"cpu_pct": round(cpu, 2), "window_s": round(elapsed, 2), "input_events": events,
                                                       "startup_s": round(startup, 2), "install_ms": info.get("install_ms", 0)})
            # Start-up timeline in ms since navigation: glue loaded, wasm instantiated, app:start (Rust entered), platform,
            # app (session built), started (run returned), first_frame (polled every 50 ms by the page).
            report["external"].update({f"startup_{k}_ms": v for k, v in marks.items()})
            report.setdefault("meta", {}).update({"adapter": adapter, "commit": git_commit(), "browser": f"chromium {browser}"})
            json.dump(report, open(out, "w"), indent=2)
            results.append(out)
            print_summary(out)
            shutil.rmtree(prof, ignore_errors=True)
    finally:
        xvfb.terminate(); server.terminate()
    return results


# ------------------------------------------------------------------------------------------------ common

def finish_report(path, meta, external):
    r = json.load(open(path))
    r["meta"].update(meta); r["external"].update(external)
    # Under Xvfb the NVIDIA driver spends most of the process CPU inside the present call (it copies and waits for every
    # frame); the same app is far cheaper on a real display. The span `present` is that wait, so what is left is
    # an estimate of the app's own CPU (a wall-clock span against CPU time, hence "estimate").
    present_ms = next((sp["total_ms"] for sp in r["stats"]["spans"] if sp["name"] == "present"), None)
    window_ms = r["external"].get("window_s", 0) * 1000
    if present_ms is not None and window_ms > 0 and "cpu_pct" in r["external"]:
        r["external"]["cpu_pct_ex_present"] = round(max(r["external"]["cpu_pct"] - present_ms / window_ms * 100, 0.0), 2)
    json.dump(r, open(path, "w"), indent=2)


def print_summary(path):
    r = json.load(open(path)); f = r["stats"]["frames"]
    ph = {k: v for k, v in f["phase_ms"].items() if v and v.get("count")}
    print(f"{os.path.basename(path)[:-5]:<10} cpu {r['external'].get('cpu_pct', float('nan')):6.1f}% ({r['external'].get('cpu_pct_ex_present', float('nan')):4.1f} ex-present)  frames {f['total']:5d} "
          f"p50 {f['frame_ms'].get('p50_ms', 0):6.2f} p99 {f['frame_ms'].get('p99_ms', 0):6.2f} ms  unexplained {f['unexplained']}  "
          f"redraws {r['stats']['redraws']['total']}  by_reason {json.dumps({k: v for k, v in f['by_reason'].items() if v})}  "
          f"encoded {r['stats']['counters'].get('elements_encoded')}", flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("platform", choices=["native", "web"])
    ap.add_argument("keys", nargs="+", help="scenario keys: idle, pan@10k, wheel@1k, zoom@100k, drag@10k, hover@1k ...")
    ap.add_argument("--out"); ap.add_argument("--secs", type=float); ap.add_argument("--display", default=":95")
    ap.add_argument("--binary", default=os.path.join(ROOT, "target/release/drafftink"))
    ap.add_argument("--dist", default=os.path.join(ROOT, "dist"))
    ap.add_argument("--server", default=os.path.join(ROOT, "target/release/drafftink-server"))
    ap.add_argument("--adapter", default="nvidia", choices=["nvidia", "swiftshader"])
    args = ap.parse_args()
    (run_native if args.platform == "native" else run_web)(args, args.keys)


if __name__ == "__main__":
    main()
