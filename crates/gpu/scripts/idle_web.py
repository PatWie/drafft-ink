#!/usr/bin/env python3
"""Idle and input check of the web demo in headed Chromium under a private Xvfb with real X input (XTEST).

Asserts: 0 frames in 10 s idle, 0 frames while the pointer moves over empty canvas, frames only where
the hovered shape changes, 0 frames after pan / wheel, ~10 fps with a 100 ms timer and 0 Vello runs,
no frames while the tab is hidden, and recovery after a destroyed GPU device (F9).
Reports the CPU of all Chromium processes (1 core = 100%) next to a blank-page baseline.
usage: ~/.venvs/perf/bin/python idle_web.py [--display :96] [--port 8132] [--shot out.png] [--json out.json]
Needs crates/gpu/web/pkg (scripts/build_web_demo.sh). Exit code 0 only if every assertion holds."""
import functools, http.server, json, math, os, subprocess, sys, threading, time
from Xlib import X, XK, display
from Xlib.ext import xtest
from playwright.sync_api import sync_playwright

HERE = os.path.dirname(os.path.abspath(__file__)); WEB = os.path.join(HERE, "..", "web")
def arg(name, default=None): return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default
DISP = arg("--display", ":96"); PORT = int(arg("--port", "8132")); shot = arg("--shot"); out_json = arg("--json")
PROF = f"/tmp/drafftink-gpu-chrome-{PORT}"; HZ = os.sysconf("SC_CLK_TCK")
# Headed Chromium with WebGPU on the NVIDIA adapter through ANGLE/Vulkan (docs/stack.md §8).
NV = ["--enable-unsafe-webgpu", "--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan", "--use-angle=vulkan", "--use-vulkan=native",
      "--ignore-gpu-blocklist", "--disable-vulkan-surface", "--no-sandbox", "--disable-gpu-sandbox"]
SW = ["--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--use-webgpu-adapter=swiftshader", "--ignore-gpu-blocklist"]
args = SW if arg("--adapter", "nvidia") == "swiftshader" else NV

class Q(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), functools.partial(Q, directory=WEB))
threading.Thread(target=srv.serve_forever, daemon=True).start()
xvfb = subprocess.Popen(["Xvfb", DISP, "-screen", "0", "1400x900x24", "+extension", "XTEST"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.5); os.environ["DISPLAY"] = DISP
d = display.Display(DISP)
fails = []; results = []; logs = []

def move(x, y): xtest.fake_input(d, X.MotionNotify, x=x, y=y); d.sync()
def key(name):
    code = d.keysym_to_keycode(XK.string_to_keysym(name))
    xtest.fake_input(d, X.KeyPress, code); xtest.fake_input(d, X.KeyRelease, code); d.sync()
def click(x, y):
    move(x, y); xtest.fake_input(d, X.ButtonPress, 1); xtest.fake_input(d, X.ButtonRelease, 1); d.sync()
def cpu_ticks():
    t = 0
    for p in os.listdir("/proc"):
        if not p.isdigit(): continue
        try:
            if PROF not in open(f"/proc/{p}/cmdline").read(): continue
            f = open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split(); t += int(f[11]) + int(f[12])
        except Exception: pass
    return t

def run_page(ctx, query, body):
    pg = ctx.new_page()
    pg.on("console", lambda m: logs.append(f"{m.type}: {m.text[:300]}")); pg.on("pageerror", lambda e: logs.append("PAGEERROR " + str(e)[:300]))
    t0 = time.time(); pg.goto(f"http://127.0.0.1:{PORT}/index.html{query}")
    for _ in range(400):
        if pg.evaluate("()=>window.__started||window.__error||false"): break
        time.sleep(0.1)
    err = pg.evaluate("()=>window.__error||null")
    print("startup_s", round(time.time() - t0, 2), "err", err, flush=True)
    if err: fails.append(f"startup error: {err}"); return pg
    time.sleep(3)
    body(pg)
    return pg

def make_window(pg, ox, oy):
    def stats(): return pg.evaluate("()=>Array.from(window.__stats())")
    def reasons():
        return {k: int(v) for k, v in (x.split("=") for x in pg.evaluate("()=>Array.from(window.__reasons())"))}
    def window(label, secs, fn=None, rate=125, expect=None):
        s0 = stats(); r0 = reasons(); c0 = cpu_ticks(); t = time.time(); n = 0
        while time.time() - t < secs:
            if fn: fn(n)
            n += 1
            if fn:
                dl = t + n / rate - time.time()
                if dl > 0: time.sleep(dl)
            else: time.sleep(0.05)
        el = time.time() - t; s1 = stats(); r1 = reasons()
        by_reason = {k: v - r0.get(k, 0) for k, v in r1.items() if v - r0.get(k, 0)}
        fr = int(s1[0] - s0[0]); vr = int(s1[1] - s0[1])
        r = dict(window=label, secs=round(el, 2), cpu_pct_of_one_core_all_chromium=round((cpu_ticks() - c0) / HZ / el * 100, 1), redraw_requests=by_reason, frames=fr, vello_runs=vr, fps=round(fr / el, 1), events=n if fn else 0)
        ok = True
        if expect:
            op, v = expect[0], expect[1]
            ok = {"==": lambda: fr == v, ">=": lambda: fr >= v, "<=": lambda: fr <= v, "between": lambda: v <= fr <= expect[2]}[op]()
        r["ok"] = ok
        if not ok: fails.append(f"{label}: frames={fr} expected {expect}")
        print(json.dumps(r), flush=True); results.append(r)
    return window

try:
    with sync_playwright() as pw:
        ctx = pw.chromium.launch_persistent_context(PROF, headless=False, viewport={"width": 1400, "height": 860}, args=args)
        # --- blank page baseline
        blank = ctx.pages[0] if ctx.pages else ctx.new_page()
        blank.goto("about:blank"); time.sleep(2)
        c0 = cpu_ticks(); time.sleep(5); base = (cpu_ticks() - c0) / HZ / 5 * 100
        print(json.dumps(dict(window="blank page baseline", cpu_pct_of_one_core_all_chromium=round(base, 1))), flush=True)
        results.append(dict(window="blank page baseline", cpu_pct_of_one_core_all_chromium=round(base, 1)))

        def main_body(pg):
            # Where is the page on the X screen? Move the real pointer and read where the page sees it.
            pg.evaluate("()=>{window.__last=null;window.addEventListener('pointermove',e=>{window.__last=[e.clientX,e.clientY]},true)}")
            move(500, 500); time.sleep(0.3); move(520, 520); time.sleep(0.5)
            last = pg.evaluate("()=>window.__last")
            if not last: raise SystemExit("the page saw no pointer movement; is the window mapped?")
            ox, oy = 520 - last[0], 520 - last[1]
            print("page origin on screen:", ox, oy, flush=True)
            window = make_window(pg, ox, oy)
            adapter = [l for l in logs if "adapter:" in l]
            print("adapter:", adapter[:1], flush=True)
            if shot: pg.screenshot(path=shot)
            move(ox + 900, oy + 120); time.sleep(1)
            window("idle 10s", 10, expect=("==", 0))
            window("hover over empty canvas 10s (125 moves/s)", 10, lambda n: move(int(ox + 700 + 500 * (1 + math.sin(n / 30)) / 2), oy + 120), expect=("==", 0))
            window("hover across shapes 5s", 5, lambda n: move(int(ox + 700 + 500 * (1 + math.sin(n / 40)) / 2), oy + 250), expect=(">=", 1))
            move(ox + 900, oy + 120); time.sleep(1)
            window("idle after hover 3s", 3, expect=("==", 0))
            xtest.fake_input(d, X.ButtonPress, 1); d.sync()
            window("drag-pan 5s", 5, lambda n: move(int(ox + 600 + 200 * math.sin(n / 40)), int(oy + 400 + 150 * math.cos(n / 55))), expect=(">=", 1))
            xtest.fake_input(d, X.ButtonRelease, 1); d.sync(); time.sleep(1)
            window("idle after pan 10s", 10, expect=("==", 0))
            def wheel(n):
                b = 4 if (n // 50) % 2 == 0 else 5
                xtest.fake_input(d, X.ButtonPress, b); xtest.fake_input(d, X.ButtonRelease, b); d.flush()
            window("wheel zoom 5s (100 ticks/s)", 5, wheel, rate=100, expect=(">=", 1))
            time.sleep(1.5)
            window("idle after wheel 10s", 10, expect=("==", 0))
            if shot: pg.screenshot(path=shot.replace(".png", "-after.png"))
            # Device loss: click the canvas for keyboard focus, F9 destroys the device, the shell recovers.
            click(ox + 900, oy + 120); time.sleep(0.5)
            key("F9")
            window("after F9 (device destroyed) 8s", 8, expect=(">=", 1))
            window("idle after recovery 10s", 10, expect=("==", 0))
            xtest.fake_input(d, X.ButtonPress, 1); d.sync()
            window("drag-pan after recovery 3s", 3, lambda n: move(int(ox + 600 + 200 * math.sin(n / 40)), int(oy + 400 + 150 * math.cos(n / 55))), expect=(">=", 1))
            xtest.fake_input(d, X.ButtonRelease, 1); d.sync(); time.sleep(1)
        pg = run_page(ctx, "", main_body); pg.close()

        # --- timer path: the app asks for a repaint every 100 ms
        def tick_body(pg):
            window = make_window(pg, 0, 0)
            window("timer 100 ms for 10s", 10, expect=("between", 80, 120))
            r = results[-1]
            if r["vello_runs"] != 0: fails.append(f"timer frames ran Vello {r['vello_runs']} times")
            # Hide the tab: a second page comes to the front. No frames while hidden.
            other = ctx.new_page(); other.goto("about:blank"); other.bring_to_front(); time.sleep(1)
            real = pg.evaluate("()=>document.hidden")
            print("document.hidden after opening another tab:", real, flush=True)
            if not real:
                # Without a window manager the browser keeps both tabs visible. Fire the same DOM signal
                # the browser would: visibilitychange with document.hidden true.
                print("(simulating visibilitychange: no window manager under Xvfb)", flush=True)
                pg.evaluate("""()=>{Object.defineProperty(document,'hidden',{configurable:true,get:()=>true});
                  Object.defineProperty(document,'visibilityState',{configurable:true,get:()=>'hidden'});
                  document.dispatchEvent(new Event('visibilitychange'))}""")
            time.sleep(0.5)
            window("tab hidden, timer still requested, 5s", 5, expect=("<=", 2))
            if not real:
                pg.evaluate("""()=>{delete document.hidden;delete document.visibilityState;
                  document.dispatchEvent(new Event('visibilitychange'))}""")
            other.close(); pg.bring_to_front(); time.sleep(1.5)
            window("tab visible again, timer 3s", 3, expect=("between", 20, 40))
        pg2 = run_page(ctx, "?tick=100", tick_body); pg2.close()
        ctx.close()
finally:
    xvfb.terminate(); srv.shutdown()

errs = [l for l in logs if l.startswith(("error", "PAGEERROR"))]
if errs: print("console errors:\n" + "\n".join(errs[:15]))
if fails: print("console log tail:\n" + "\n".join(logs[-15:]))
if out_json: json.dump(results, open(out_json, "w"), indent=1)
print("RESULT:", "PASS" if not fails else "FAIL")
for f in fails: print("  FAIL", f)
sys.exit(1 if fails else 0)
