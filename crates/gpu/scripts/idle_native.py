#!/usr/bin/env python3
"""Idle and input check of the native demo under a private Xvfb with real X input (XTEST).

Asserts: 0 frames in 10 s of idle, 0 frames while the pointer moves over empty canvas, frames only
where something changed for hover over shapes, 0 frames after a pan, a wheel zoom and a device loss,
and that the shell recovers from a destroyed GPU device. Prints one JSON line per window and a summary.
usage: ~/.venvs/perf/bin/python idle_native.py <demo binary> [--display :95] [--json out.json]
Exit code 0 only if every assertion holds."""
import json, math, os, re, subprocess, sys, tempfile, time
from Xlib import X, XK, display
from Xlib.ext import xtest

binary = sys.argv[1]
DISP = sys.argv[sys.argv.index("--display") + 1] if "--display" in sys.argv else ":95"
out_json = sys.argv[sys.argv.index("--json") + 1] if "--json" in sys.argv else None
HZ = os.sysconf("SC_CLK_TCK")
fails = []

xvfb = subprocess.Popen(["Xvfb", DISP, "-screen", "0", "1400x900x24", "+extension", "XTEST"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.5)
log = tempfile.NamedTemporaryFile("w+", suffix=".log", delete=False)
env = dict(os.environ, DISPLAY=DISP, RUST_LOG="drafftink_gpu::frame=debug,drafftink_gpu=info,warn")
proc = subprocess.Popen([binary], env=env, stderr=log, stdout=subprocess.DEVNULL)
d = display.Display(DISP)
FRAME = re.compile(r"frame (\d+) reason=(\w+)")

def frames():
    log.flush()
    return FRAME.findall(open(log.name).read())

def ticks():
    f = open(f"/proc/{proc.pid}/stat").read().rsplit(")", 1)[1].split()
    return int(f[11]) + int(f[12])

def move(x, y):
    xtest.fake_input(d, X.MotionNotify, x=x, y=y); d.sync()

def focus_window():
    """No window manager under Xvfb: give the demo window the keyboard focus ourselves."""
    for w in d.screen().root.query_tree().children:
        try:
            if w.get_wm_name() and "demo" in str(w.get_wm_name()):
                w.set_input_focus(X.RevertToParent, X.CurrentTime); d.sync(); return True
        except Exception: pass
    return False

def key(name):
    code = d.keysym_to_keycode(XK.string_to_keysym(name))
    xtest.fake_input(d, X.KeyPress, code); xtest.fake_input(d, X.KeyRelease, code); d.sync()

results = []
def window(label, secs, fn=None, rate=125, expect=None):
    """expect: ('==', n) | ('>=', n) | ('<=', n) on the number of frames drawn in the window."""
    f0 = len(frames()); c0 = ticks(); t0 = time.time(); n = 0
    while time.time() - t0 < secs:
        if fn: fn(n)
        n += 1
        dl = t0 + n / rate - time.time()
        time.sleep(max(dl, 0) if fn else 0.05)
    el = time.time() - t0
    fr = frames()[f0:]
    reasons = {}
    for _, r in fr: reasons[r] = reasons.get(r, 0) + 1
    r = dict(window=label, secs=round(el, 2), cpu_pct_of_one_core=round((ticks() - c0) / HZ / el * 100, 2), frames=len(fr), reasons=reasons, events=n if fn else 0)
    ok = True
    if expect:
        op, v = expect
        ok = {"==": len(fr) == v, ">=": len(fr) >= v, "<=": len(fr) <= v}[op]
    r["ok"] = ok
    if not ok: fails.append(f"{label}: frames={len(fr)} expected {expect}")
    print(json.dumps(r), flush=True); results.append(r)

try:
    for _ in range(100):  # first frame (shader compile can take seconds)
        if frames(): break
        if proc.poll() is not None: raise SystemExit("demo exited early:\n" + open(log.name).read()[-2000:])
        time.sleep(0.2)
    else:
        raise SystemExit("no first frame in 20 s:\n" + open(log.name).read()[-2000:])
    adapter = re.findall(r"adapter: (.*)", open(log.name).read())
    print("adapter:", adapter[:1], flush=True)
    time.sleep(2)
    move(900, 120); time.sleep(1)   # y=120 is the gap between cell rows: empty canvas
    window("idle 10s", 10, expect=("==", 0))
    # Hover (right of the egui window) along the gap between two rows of rectangles: nothing under the pointer ever changes.
    window("hover over empty canvas 10s (125 moves/s)", 10, lambda n: move(int(700 + 500 * (1 + math.sin(n / 30)) / 2), 120), expect=("==", 0))
    # Hover across shapes (row 2): frames only where the hovered shape changes.
    window("hover across shapes 5s", 5, lambda n: move(int(700 + 500 * (1 + math.sin(n / 40)) / 2), 250), expect=(">=", 1))
    move(900, 120); time.sleep(1)
    window("idle after hover", 3, expect=("==", 0))
    xtest.fake_input(d, X.ButtonPress, 1); d.sync()
    window("drag-pan 5s", 5, lambda n: move(int(600 + 200 * math.sin(n / 40)), int(400 + 150 * math.cos(n / 55))), expect=(">=", 1))
    xtest.fake_input(d, X.ButtonRelease, 1); d.sync(); time.sleep(1)
    window("idle after pan 10s", 10, expect=("==", 0))
    def wheel(n):
        b = 4 if (n // 50) % 2 == 0 else 5
        xtest.fake_input(d, X.ButtonPress, b); xtest.fake_input(d, X.ButtonRelease, b); d.flush()
    window("wheel zoom 5s (100 ticks/s)", 5, wheel, rate=100, expect=(">=", 1))
    time.sleep(1.5)
    window("idle after wheel 10s", 10, expect=("==", 0))
    # Device loss: F9 destroys the device; the shell must recreate it and draw again.
    move(900, 120); time.sleep(0.5)
    if not focus_window(): fails.append("demo window not found for keyboard focus")
    key("F9")
    window("after F9 (device destroyed), 6s", 6, expect=(">=", 1))
    window("idle after recovery 10s", 10, expect=("==", 0))
    xtest.fake_input(d, X.ButtonPress, 1); d.sync()
    window("drag-pan after recovery 3s", 3, lambda n: move(int(600 + 200 * math.sin(n / 40)), int(400 + 150 * math.cos(n / 55))), expect=(">=", 1))
    xtest.fake_input(d, X.ButtonRelease, 1); d.sync(); time.sleep(1)
    if proc.poll() is not None: fails.append("demo process died")
    text = open(log.name).read()
    if "device lost" not in text.lower(): fails.append("no 'device lost' log line: F9 did not reach the app")
finally:
    proc.terminate()
    try: proc.wait(5)
    except Exception: proc.kill()
    xvfb.terminate()

if out_json: json.dump(results, open(out_json, "w"), indent=1)
if fails:
    print("--- demo log tail ---"); print(open(log.name).read()[-3000:])
print("RESULT:", "PASS" if not fails else "FAIL")
for f in fails: print("  FAIL", f)
sys.exit(1 if fails else 0)
