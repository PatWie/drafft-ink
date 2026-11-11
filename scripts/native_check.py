#!/usr/bin/env python3
"""End-to-end check of the desktop app under a private Xvfb with real X input (XTEST) and the agent bridge.

Launches `drafftink --bridge`, waits for the intro document, draws a rectangle and an arrow with the mouse
through the toolbar, checks through the bridge that both elements exist, takes screenshots (X screen and
`view.screenshot`), measures idle (10 s), hover, drag-pan and wheel (frames and CPU), then closes the window
and checks the perf dump.

usage: ~/.venvs/perf/bin/python scripts/native_check.py <drafftink binary> [--img-dir docs/img] [--display :97]
Exit code 0 only if every assertion holds."""
import base64, hashlib, json, math, os, re, socket, struct, subprocess, sys, tempfile, time
from Xlib import X, XK, Xatom, display
from Xlib.ext import xtest
from Xlib.protocol import event
from PIL import Image

binary = sys.argv[1]
arg = lambda name, default: sys.argv[sys.argv.index(name) + 1] if name in sys.argv else default
DISP = arg("--display", ":97")
IMG = arg("--img-dir", "docs/img")
HZ = os.sysconf("SC_CLK_TCK")
fails, results = [], []
work = tempfile.mkdtemp(prefix="drafftink-native-")


class Ws:
    """A just-enough WebSocket client for the bridge (text frames, no extensions)."""
    def __init__(self, port):
        self.s = socket.create_connection(("127.0.0.1", port), timeout=30)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall((f"GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        head = b""
        while b"\r\n\r\n" not in head:
            head += self.s.recv(1)
        assert b" 101 " in head.split(b"\r\n")[0], head
        self.n = 0

    def _read(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.s.recv(n - len(buf))
            if not chunk: raise EOFError
            buf += chunk
        return buf

    def _frame(self):
        b1, b2 = self._read(2)
        ln = b2 & 0x7F
        if ln == 126: ln = struct.unpack(">H", self._read(2))[0]
        elif ln == 127: ln = struct.unpack(">Q", self._read(8))[0]
        return b1 & 0x0F, self._read(ln)

    def call(self, method, params=None):
        self.n += 1
        body = json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params or {}}).encode()
        mask = os.urandom(4)
        n = len(body)
        head = bytes([0x81]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n) if n < 65536 else bytes([0x80 | 127]) + struct.pack(">Q", n))
        self.s.sendall(head + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(body)))
        while True:
            op, data = self._frame()
            if op == 1:
                msg = json.loads(data)
                if msg.get("id") == self.n:
                    return msg
            elif op == 8:
                raise EOFError

    def ok(self, method, params=None):
        msg = self.call(method, params)
        if "error" in msg: raise RuntimeError(f"{method}: {msg['error']}")
        return msg["result"]


xvfb = subprocess.Popen(["Xvfb", DISP, "-screen", "0", "1400x900x24", "+extension", "XTEST"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.5)
log = open(os.path.join(work, "app.log"), "w+")
perf_json = os.path.join(work, "perf.json")
env = dict(os.environ, DISPLAY=DISP, DRAFFTINK_BRIDGE_DIR=os.path.join(work, "bridge"),
           RUST_LOG="drafftink_gpu::frame=debug,drafftink_gpu=info,warn")
proc = subprocess.Popen([binary, "--bridge", "--data-dir", os.path.join(work, "data"), "--perf-dump", perf_json], env=env, stderr=log, stdout=subprocess.DEVNULL)
d = display.Display(DISP)
FRAME = re.compile(r"frame (\d+) reason=(\w+)")


def frames():
    log.flush()
    log.seek(0)
    return FRAME.findall(log.read())


def ticks():
    f = open(f"/proc/{proc.pid}/stat").read().rsplit(")", 1)[1].split()
    return int(f[11]) + int(f[12])


def move(x, y): xtest.fake_input(d, X.MotionNotify, x=x, y=y); d.sync()
def button(b, down): xtest.fake_input(d, X.ButtonPress if down else X.ButtonRelease, b); d.sync()
def click(x, y): move(x, y); time.sleep(0.15); button(1, True); button(1, False); time.sleep(0.4)


def drag(x0, y0, x1, y1, steps=20):
    move(x0, y0); time.sleep(0.15); button(1, True); time.sleep(0.1)
    for i in range(1, steps + 1):
        move(x0 + (x1 - x0) * i // steps, y0 + (y1 - y0) * i // steps); time.sleep(0.02)
    button(1, False); time.sleep(0.5)



def focus_app():
    for w in d.screen().root.query_tree().children:
        try:
            if w.get_wm_name() and "Drafft" in str(w.get_wm_name()): w.set_input_focus(X.RevertToParent, X.CurrentTime)
        except Exception: pass
    d.sync()


def press(*names):
    codes = [d.keysym_to_keycode(XK.string_to_keysym(n)) for n in names]
    for c in codes: xtest.fake_input(d, X.KeyPress, c)
    for c in reversed(codes): xtest.fake_input(d, X.KeyRelease, c)
    d.sync(); time.sleep(0.4)


def shot(name):
    os.makedirs(IMG, exist_ok=True)
    g = d.screen().root.get_geometry()
    raw = d.screen().root.get_image(0, 0, g.width, g.height, X.ZPixmap, 0xFFFFFFFF)
    img = Image.frombytes("RGB", (g.width, g.height), raw.data, "raw", "BGRX").crop((0, 0, 1280, 800))
    path = os.path.join(IMG, name)
    img.save(path)
    return img


def window(label, secs, fn=None, rate=125, expect=None):
    f0, c0, t0, n = len(frames()), ticks(), time.time(), 0
    while time.time() - t0 < secs:
        if fn: fn(n)
        n += 1
        time.sleep(max(t0 + n / rate - time.time(), 0) if fn else 0.05)
    el = time.time() - t0
    fr = frames()[f0:]
    reasons = {}
    for _, r in fr: reasons[r] = reasons.get(r, 0) + 1
    r = dict(window=label, secs=round(el, 2), cpu_pct_of_one_core=round((ticks() - c0) / HZ / el * 100, 2), frames=len(fr), reasons=reasons)
    ok = True
    if expect:
        op, v = expect
        ok = {"==": len(fr) == v, ">=": len(fr) >= v, "<=": len(fr) <= v}[op]
    r["ok"] = ok
    if not ok: fails.append(f"{label}: frames={len(fr)} expected {expect}")
    print(json.dumps(r), flush=True); results.append(r)


def counts(ws):
    out = ws.ok("execute", {"id": "doc.inspect", "args": {}})
    return out


def check(cond, what):
    if not cond: fails.append(what)
    print(("ok   " if cond else "FAIL ") + what, flush=True)


def close_window():
    root = d.screen().root
    for w in root.query_tree().children:
        try:
            name = w.get_wm_name()
        except Exception:
            continue
        if name and "Drafft" in str(name):
            wm_protocols = d.intern_atom("WM_PROTOCOLS")
            delete = d.intern_atom("WM_DELETE_WINDOW")
            ev = event.ClientMessage(window=w, client_type=wm_protocols, data=(32, [delete, X.CurrentTime, 0, 0, 0]))
            w.send_event(ev, event_mask=X.NoEventMask); d.sync()
            return True
    return False


try:
    for _ in range(150):
        if frames(): break
        if proc.poll() is not None: raise SystemExit("app exited early:\n" + open(log.name).read()[-2000:])
        time.sleep(0.2)
    else:
        raise SystemExit("no first frame in 30 s:\n" + open(log.name).read()[-2000:])
    print("adapter:", re.findall(r"adapter: (.*)", open(log.name).read())[:1], flush=True)
    time.sleep(2.5)
    port = int(open(os.path.join(work, "bridge", "current.port")).read())
    token = open(os.path.join(work, "bridge", f"{port}.token")).read().strip()
    ws = Ws(port)
    ws.ok("hello", {"token": token})
    before = ws.ok("execute", {"id": "doc.inspect", "args": {}})
    n_before = sum(before.get("counts", {}).values()) if isinstance(before.get("counts"), dict) else None
    print("intro:", json.dumps(before)[:300], flush=True)
    img = shot("native-intro.png")
    bg = img.getpixel((640, 600)); ink = sum(1 for p in img.get_flattened_data() if p != bg)
    check(ink > 20000, f"the intro document is on screen ({ink} non-background pixels)")

    # Draw through the bridge: two rectangles, an arrow bound to both, then move one and see the arrow follow.
    f0 = len(frames())
    ex = lambda i, a: ws.ok("execute", {"id": i, "args": a})
    ra = ex("element.create", {"kind": "rectangle", "bounds": {"x": 2000, "y": 100, "w": 160, "h": 90}})["id"]
    rb = ex("element.create", {"kind": "rectangle", "bounds": {"x": 2400, "y": 200, "w": 160, "h": 90}})["id"]
    arrow = ex("element.create", {"kind": "arrow", "points": [[2160, 145], [2400, 245]], "start": {"target": ra}, "end": {"target": rb}})
    check(arrow.get("start_target") == ra and arrow.get("end_target") == rb, f"the bridge made an arrow bound to both shapes ({arrow.get('start_target')}, {arrow.get('end_target')})")
    pts = lambda: ex("doc.inspect", {"ids": [arrow["id"]], "detail": "full"})["items"][0]["points"]
    p0 = pts()
    ex("element.move", {"ids": [ra], "dx": 0, "dy": 150})
    p1 = pts()
    check(p0 != p1, f"element.move dragged the bound arrow along ({p0} -> {p1})")
    time.sleep(1.0)
    check(len(frames()) > f0, f"the bridge commands drew frames ({len(frames()) - f0})")
    cam = ex("view.get", {})
    ex("selection.clear", {})
    ex("view.zoom_to_fit", {"scope": "ids", "ids": [ra, rb, arrow["id"]]})
    time.sleep(0.8)
    shot("native-bridge-drawn.png")
    ex("view.set", {"scroll": cam["scroll"], "zoom": cam["zoom"]})
    time.sleep(0.5)

    # Draw with the toolbar and the mouse: rectangle tool, then arrow tool.
    f0 = len(frames())
    click(40, 232)
    drag(640, 610, 790, 700)
    click(40, 343)
    drag(830, 700, 930, 620)
    # Text: the text tool, a click, typing, Esc.
    click(40, 491)
    focus_app()
    print("after T click:", [(e["id"], e["source"], e["outcome"]) for e in ws.ok("recent", {"limit": 3})["entries"]], flush=True)
    click(700, 740)
    time.sleep(0.5)
    for ch in ("h", "i"): press(ch)
    shot("native-text-edit.png")
    press("Escape"); time.sleep(0.5)
    print("after text:", [(e["id"], e["source"], e["outcome"].get("ok")) for e in ws.ok("recent", {"limit": 4})["entries"]], flush=True)
    shot("native-drawn.png")
    outline = ws.ok("execute", {"id": "doc.outline", "args": {"format": "text"}})
    check('"hi"' in json.dumps(outline).replace('\\"', '"'), "the typed text became an element")
    after = ws.ok("execute", {"id": "doc.inspect", "args": {}})
    print("after:", json.dumps(after)[:300], flush=True)
    recent = ws.ok("recent", {"limit": 30, "mutating_only": True})
    creates = [e for e in recent["entries"] if e["id"] == "element.create"]
    creates = [e for e in creates if e["source"] != "agent"]  # the bridge made three earlier
    check(len(creates) >= 2, f"two element.create calls came from the mouse ({[ (e['id'], e['source']) for e in recent['entries']]})")
    kinds = [json.loads(e["args"]).get("kind") if e["args"].startswith("{") else None for e in creates]
    check("rectangle" in kinds and "arrow" in kinds, f"a rectangle and an arrow were created ({kinds})")
    check(all(e["source"] in ("ui", "script") for e in creates), "created by the interaction layer, not the bridge")

    # view.screenshot through the offscreen renderer
    try:
        shotres = ws.ok("execute", {"id": "view.screenshot", "args": {"region": "all", "max_side_px": 1400}})
        png = base64.b64decode(shotres["bytes"]["base64"])
        os.makedirs(IMG, exist_ok=True)
        open(os.path.join(IMG, "native-view-screenshot.png"), "wb").write(png)
        check(png[:4] == b"\x89PNG", f"view.screenshot returned a PNG ({len(png)} bytes, {shotres['width']}x{shotres['height']})")
    except Exception as e:
        check(False, f"view.screenshot: {e}")

    # Idle and input windows.
    move(1000, 700); time.sleep(2)
    window("idle 10s", 10, expect=("==", 0))
    window("hover over empty canvas 10s (125 moves/s)", 10, lambda n: move(int(900 + 300 * (1 + math.sin(n / 30)) / 2), 720), expect=("==", 0))
    move(1000, 700); time.sleep(1)
    window("idle after hover 3s", 3, expect=("==", 0))
    # Pan with the middle button (a view gesture).
    button(2, True)
    window("middle-button pan 5s (125 moves/s)", 5, lambda n: move(int(640 + 200 * math.sin(n / 40)), int(400 + 150 * math.cos(n / 55))), expect=(">=", 1))
    button(2, False); time.sleep(1.5)
    window("idle after pan 10s", 10, expect=("==", 0))

    def wheel(n):
        b = 4 if (n // 50) % 2 == 0 else 5
        xtest.fake_input(d, X.ButtonPress, b); xtest.fake_input(d, X.ButtonRelease, b); d.flush()
    move(700, 650)
    window("wheel 5s (100 ticks/s)", 5, wheel, rate=100, expect=(">=", 1))
    time.sleep(2)
    window("idle after wheel 10s", 10, expect=("==", 0))
    # Undo the drawing with Ctrl+Z from the keyboard.
    focus_app()
    kc = lambda name: d.keysym_to_keycode(XK.string_to_keysym(name))
    for k in ("Control_L", "z"):
        xtest.fake_input(d, X.KeyPress, kc(k))
    for k in ("z", "Control_L"):
        xtest.fake_input(d, X.KeyRelease, kc(k))
    d.sync(); time.sleep(1)
    undone = ws.ok("execute", {"id": "doc.inspect", "args": {}})
    check(undone != after, "Ctrl+Z undid the last drawing")
    time.sleep(3)  # the autosave of the undone state comes a second after the change
    window("idle after undo 5s", 5, expect=("==", 0))
    check(close_window(), "window found to close")
    for _ in range(50):
        if proc.poll() is not None: break
        time.sleep(0.2)
    check(proc.poll() == 0, f"the app exited cleanly after the window close (exit {proc.poll()})")
    stats = json.load(open(perf_json)) if os.path.exists(perf_json) else None
    check(stats is not None and stats.get("schema", "").startswith("drafftink"), "the perf dump was written")
    log.flush(); log.seek(0)
    print("exit line:", re.findall(r"frames=\d+ vello_runs=\d+", log.read()), flush=True)
finally:
    if proc.poll() is None:
        proc.terminate()
        try: proc.wait(5)
        except Exception: proc.kill()
    xvfb.terminate()

json.dump(results, open(os.path.join(work, "results.json"), "w"), indent=1)
print("work dir:", work)
if fails:
    log.flush(); log.seek(0)
    print("--- app log tail ---"); print(log.read()[-3000:])
print("RESULT:", "PASS" if not fails else "FAIL")
for f in fails: print("  FAIL", f)
sys.exit(1 if fails else 0)
