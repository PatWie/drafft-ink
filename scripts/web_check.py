#!/usr/bin/env python3
"""End-to-end check of the web app in headed Chromium on a private Xvfb. Builds nothing.

Needs dist/ (scripts/build-web.sh) and target/release/drafftink-headless (cargo build --release -p drafftink-headless).
Starts `drafftink-headless serve` on an ephemeral port with --web-dir dist and --storage memory, then checks:
  load       the page starts, window.drafftink exists, perf()/frames() answer, WebGPU adapter works
  api        agent.pair refuses a non-loopback address with the loopback message; status() is idle
  idle       0 new frames in a 10 s idle window, CPU of all Chromium processes (1 core = 100 %)
  --app      (needs the App core) execute()/state()/commands(), a rectangle and an arrow, a screenshot,
             persistence in IndexedDB, a refusal as a rejected promise
  --app      hover: 125 mouse moves per second over empty canvas for 5 s draw 0 frames (the CPU is printed)
  --app      the on-canvas text editor: draft, typing, a coloured word, Esc, one undo step
  --app      collab: two pages in the same room (?room=...) see each other's element
  offline    server stopped, reload: the service worker serves the page and the app starts
usage: ~/.venvs/perf/bin/python scripts/web_check.py [--app] [--display :97] [--shot docs/img/web-app.png] [--json out.json]
Exit code 0 only if every assertion holds."""
import argparse, json, math, os, random, shutil, signal, socket, subprocess, sys, tempfile, time, urllib.request
from playwright.sync_api import sync_playwright

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# cargo's target folder, which CARGO_TARGET_DIR moves (relative to the repository root, where xtask runs cargo)
TARGET = os.path.join(ROOT, os.environ.get("CARGO_TARGET_DIR") or "target")
ap = argparse.ArgumentParser()
ap.add_argument("--app", action="store_true", help="also run the checks that need the real App core")
ap.add_argument("--display", default=":97")
ap.add_argument("--dist", default=os.path.join(ROOT, "dist"))
ap.add_argument("--headless", default=os.path.join(TARGET, "release/drafftink-headless"), help="the drafftink-headless binary; its `serve` runs the server")
ap.add_argument("--shot", default=None, help="screenshot path (1400x860), only with --app")
ap.add_argument("--json", default=None)
ap.add_argument("--adapter", default="nvidia", choices=["nvidia", "swiftshader"])
ap.add_argument("--idle-secs", type=float, default=10)
args = ap.parse_args()

HZ = os.sysconf("SC_CLK_TCK")
PROF = tempfile.mkdtemp(prefix="drafftink-web-check-")
NV = ["--enable-unsafe-webgpu", "--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan", "--use-angle=vulkan", "--use-vulkan=native",
      "--ignore-gpu-blocklist", "--disable-vulkan-surface", "--no-sandbox", "--disable-gpu-sandbox"]
SW = ["--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
      "--use-webgpu-adapter=swiftshader", "--ignore-gpu-blocklist"]
FLAGS = SW if args.adapter == "swiftshader" else NV

fails, results, logs = [], [], []

def check(ok, what, detail=""):
    print(("ok   " if ok else "FAIL ") + what + (f"  [{detail}]" if detail else ""), flush=True)
    if not ok: fails.append(what + (f" [{detail}]" if detail else ""))
    return ok

def free_port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p

def cpu_ticks():
    t = 0
    for p in os.listdir("/proc"):
        if not p.isdigit(): continue
        try:
            if PROF not in open(f"/proc/{p}/cmdline").read(): continue
            f = open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split(); t += int(f[11]) + int(f[12])
        except Exception: pass
    return t

class Server:
    def __init__(self, port):
        self.port = port; self.proc = None
    def start(self):
        if not os.path.exists(self.server_bin()): raise SystemExit(f"{args.headless} not found: cargo build --release -p drafftink-headless")
        self.proc = subprocess.Popen([args.headless, "serve", "--web-dir", args.dist, "--bind", f"127.0.0.1:{self.port}", "--storage", "memory"],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try: urllib.request.urlopen(f"http://127.0.0.1:{self.port}/health", timeout=1); return
            except Exception: time.sleep(0.1)
        raise SystemExit("the server did not start")
    def server_bin(self): return args.headless
    def stop(self):
        if self.proc: self.proc.terminate(); self.proc.wait(10); self.proc = None

def open_app(ctx, url, wait=60):
    pg = ctx.new_page()
    pg.on("console", lambda m: logs.append(f"{m.type}: {m.text[:300]}"))
    pg.on("pageerror", lambda e: logs.append("PAGEERROR " + str(e)[:300]))
    pg.set_viewport_size({"width": 1400, "height": 860})
    t0 = time.time(); pg.goto(url)
    for _ in range(int(wait * 10)):
        if pg.evaluate("()=>window.__started||window.__error||false"): break
        time.sleep(0.1)
    return pg, round(time.time() - t0, 2)

def call(pg, expr):
    """Runs an async JS expression in the page and returns {ok, value} or {ok: false, error}."""
    return pg.evaluate("async()=>{try{return {ok:true,value:await (" + expr + ")}}catch(e){return {ok:false,error:(e&&e.message)?{code:e.code,message:e.message,data:e.data}:String(e)}}}")

def frames(pg): return pg.evaluate("()=>window.drafftink.frames()")

def reasons(pg):
    return pg.evaluate("()=>window.drafftink.perf().redraws.by_reason")

def settle(pg, quiet=1.5, limit=20):
    """Waits until no frame was drawn for `quiet` seconds. A page that starts is busy for a moment: fonts and the
    first documents arrive after the first frame (the big fonts are separate files, docs/web-app.md). An app that
    never settles still fails the idle window that follows, because that window counts frames."""
    try: pg.wait_for_load_state("networkidle", timeout=limit * 1000)
    except Exception: pass
    t0 = time.time(); last = frames(pg)["frames"]; since = time.time()
    while time.time() - t0 < limit and time.time() - since < quiet:
        time.sleep(0.1)
        now = frames(pg)["frames"]
        if now != last: last, since = now, time.time()

def window(pg, label, secs):
    settle(pg)
    f0 = frames(pg); r0 = reasons(pg); c0 = cpu_ticks(); t = time.time(); seen = []
    while time.time() - t < secs:
        time.sleep(min(0.5, secs - (time.time() - t)))
        n = frames(pg)["frames"] - f0["frames"]
        if not seen or n != seen[-1][1]: seen.append((round(time.time() - t, 1), n))
    el = time.time() - t
    f1 = frames(pg); r1 = reasons(pg)
    r = dict(window=label, secs=round(el, 2), frames=f1["frames"] - f0["frames"], vello_runs=f1["vello_runs"] - f0["vello_runs"],
             redraw_requests={k: v - r0.get(k, 0) for k, v in r1.items() if v - r0.get(k, 0)},
             cpu_pct_of_one_core_all_chromium=round((cpu_ticks() - c0) / HZ / el * 100, 1),
             frame_times=[x for x in seen if x[1]])
    print(json.dumps(r), flush=True); results.append(r); return r

def hover_window(pg, label, secs, rate=125, centre=(1100, 250)):
    """`rate` pointer moves per second on a small circle over empty canvas (no shape, no panel): the app has nothing to
    redraw, so no frame may be drawn. Reports the CPU of all Chromium processes while it runs."""
    settle(pg)
    pg.mouse.move(*centre); time.sleep(0.3)
    f0 = frames(pg); c0 = cpu_ticks(); t = time.time(); n = 0
    while time.time() - t < secs:
        pg.mouse.move(int(centre[0] + 40 * math.sin(n / 20)), int(centre[1] + 25 * math.cos(n / 20))); n += 1
        time.sleep(max(t + n / rate - time.time(), 0))
    el = time.time() - t
    r = dict(window=label, secs=round(el, 2), moves=n, frames=frames(pg)["frames"] - f0["frames"],
             cpu_pct_of_one_core_all_chromium=round((cpu_ticks() - c0) / HZ / el * 100, 1))
    print(json.dumps(r), flush=True); results.append(r); return r

xvfb = subprocess.Popen(["Xvfb", args.display, "-screen", "0", "1600x1100x24"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.5); os.environ["DISPLAY"] = args.display
port = free_port(); server = Server(port); server.start()
base = f"http://127.0.0.1:{port}"
TOKEN = "0123456789abcdef" * 4
try:
    with sync_playwright() as pw:
        ctx = pw.chromium.launch_persistent_context(PROF, headless=False, viewport={"width": 1400, "height": 860}, args=FLAGS)
        # ---------------------------------------------------------------- load
        pg, secs = open_app(ctx, base + "/")
        err = pg.evaluate("()=>window.__error||null")
        check(err is None, "page starts without an error", str(err))
        check(pg.evaluate("()=>window.__started===true"), "first frame drawn", f"{secs}s from navigation")
        print("startup_s", secs, flush=True)
        api = pg.evaluate("()=>Object.keys(window.drafftink).sort()")
        want = ["agent", "batch", "commands", "describe", "execute", "frames", "perf", "perf_enable", "perf_reset", "recent", "runScenario", "scenarioReport", "state", "version"]
        check(api == sorted(want), "window.drafftink has the documented methods", str(api))
        perf = pg.evaluate("()=>window.drafftink.perf()")
        check(isinstance(perf, dict) and "redraws" in perf, "perf() returns PerfStats", str(list(perf)[:6]) if isinstance(perf, dict) else str(perf))
        check(pg.evaluate("()=>window.drafftink.frames().frames")>0, "frames() counts frames")
        # ---------------------------------------------------------------- api (no document needed)
        r = call(pg, "window.drafftink.agent.pair('ws://example.com:4555/', '" + TOKEN + "')")
        check((not r["ok"]) and "only listens on this machine" in r["error"]["message"], "agent.pair refuses a non-loopback address", str(r))
        r = call(pg, "window.drafftink.agent.pair('ws://127.0.0.1:4555/', 'short')")
        check((not r["ok"]) and "64 hexadecimal" in r["error"]["message"], "agent.pair refuses a bad token", str(r)[:120])
        st = pg.evaluate("()=>window.drafftink.agent.status()")
        check(st["state"] == "idle", "agent.status() is idle", str(st))
        r = call(pg, "window.drafftink.execute(42)")
        check((not r["ok"]) and r["error"]["code"] == -32602, "execute with a bad id rejects with invalid params", str(r)[:120])
        # the pairing in the address is removed at start
        pg2, _ = open_app(ctx, f"{base}/?x=1#room=r1&drafftink-agent-port=1&drafftink-agent-token={TOKEN}", wait=60)
        h = pg2.evaluate("()=>location.hash+'|'+location.search")
        check("drafftink-agent" not in h and "room=r1" in h and "x=1" in h, "pairing keys are scrubbed from the address, other parts stay", h)
        pg2.close()
        # ---------------------------------------------------------------- idle
        time.sleep(2)
        r = window(pg, f"idle {args.idle_secs:g}s", args.idle_secs)
        check(r["frames"] == 0, "0 frames in the idle window", str(r["frames"]))
        # ---------------------------------------------------------------- app
        if args.app:
            r = call(pg, "window.drafftink.state()")
            check(r["ok"] and "tabs" in r["value"], "state() resolves with engine.state", str(r)[:200])
            r = call(pg, "window.drafftink.commands({search:'element.create'})")
            check(r["ok"], "commands() resolves", str(r)[:120])
            r = call(pg, "window.drafftink.describe('element.create')")
            check(r["ok"], "describe() resolves", str(r)[:120])
            r = call(pg, "window.drafftink.execute('nope.nothing', {})")
            check((not r["ok"]) and r["error"]["code"] == -32010, "an unknown command rejects with a refusal", str(r)[:200])
            r = call(pg, "window.drafftink.execute('element.create', {kind:'rectangle', bounds:{x:120,y:120,w:260,h:150}})")
            check(r["ok"] and r["value"].get("changed") is True, "element.create rectangle", str(r)[:200])
            id_a = r["value"].get("id")
            r = call(pg, "window.drafftink.execute('element.create', {kind:'rectangle', bounds:{x:620,y:260,w:200,h:120}})")
            id_b = r["value"].get("id")
            r = call(pg, "window.drafftink.execute('element.create', {kind:'arrow', points:[[380,195],[620,320]], start:{target:'" + id_a + "'}, end:{target:'" + id_b + "'}})")
            check(r["ok"] and r["value"].get("start_target") == id_a and r["value"].get("end_target") == id_b, "element.create arrow bound to both rectangles", str(r)[:200])
            arrow = r["value"].get("id")
            r = call(pg, "window.drafftink.execute('element.move', {ids:['" + id_a + "'], dx:0, dy:-60})")
            moved = call(pg, "window.drafftink.execute('doc.inspect', {ids:['" + arrow + "'], detail:'full'})")
            check(r["ok"] and moved["ok"] and moved["value"]["items"][0]["points"][0][1] < 195, "element.move dragged the bound arrow along", str(moved)[:200])
            r = call(pg, "window.drafftink.commands({})")
            cmds = r["value"] if r["ok"] else None
            items = cmds.get("commands", cmds.get("items", [])) if isinstance(cmds, dict) else (cmds or [])
            check(r["ok"] and len(items) > 100 and any("available" in str(c) for c in items[:3]), "commands() lists the registry with availability", f"{len(items)} commands; first: {str(items[:1])[:200]}")
            r = call(pg, "window.drafftink.batch([{id:'element.create', args:{kind:'ellipse', bounds:{x:470,y:80,w:140,h:90}}}], {label:'ellipse'})")
            check(r["ok"], "batch() resolves", str(r)[:200])
            r = call(pg, "window.drafftink.recent({limit:5})")
            check(r["ok"] and any(e.get("source") == "agent" for e in r["value"].get("entries", [])), "recent() shows origin agent", str(r)[:200])
            time.sleep(1.5)
            q = call(pg, "window.drafftink.execute('element.query', {})")
            check(q["ok"] and q["value"].get("total", 0) >= 4, "element.query sees 4 elements", str(q)[:160])
            # PNG export: the renderer port answers "not ready" while the GPU reads the picture back, and the bridge
            # runs the command again, so the caller just sees the picture.
            import base64, io
            from PIL import Image
            ex = call(pg, "window.drafftink.execute('doc.export', {format: 'png', scope: 'all', scale: 1})")
            png = base64.b64decode(ex["value"]["bytes"]["base64"]) if ex["ok"] else b""
            check(ex["ok"] and png[:8] == b"\x89PNG\r\n\x1a\n", "doc.export png returns a PNG on the web", str(ex)[:200])
            if png:
                im = Image.open(io.BytesIO(png)).convert("RGB")
                check(im.size == (ex["value"]["width"], ex["value"]["height"]) and len(set(im.resize((64, 64)).getdata())) > 3,
                      "the exported picture has the reported size and is not blank", f"{im.size}")
                check(ex["value"].get("scene_embedded") is True, "the exported PNG embeds the scene", str(ex["value"].get("scene_embedded")))
            sh = call(pg, "window.drafftink.execute('view.screenshot', {region: 'all', scale: 0.5})")
            check(sh["ok"] and base64.b64decode(sh["value"]["bytes"]["base64"])[:4] == b"\x89PNG", "view.screenshot returns a PNG on the web", str(sh)[:160])
            ex2 = call(pg, "window.drafftink.execute('doc.export', {format: 'png', scope: 'all', scale: 2})")
            check(ex2["ok"] and ex2["value"]["width"] == 2 * ex["value"]["width"], "a second export at another scale is drawn too", str(ex2)[:160])
            # real browser input: the rectangle tool from the toolbar, then a drag on the canvas
            n0 = call(pg, "window.drafftink.execute('element.query', {})")["value"]["total"]
            call(pg, "window.drafftink.execute('selection.clear', {})")
            pg.mouse.move(700, 300); time.sleep(0.3); pg.keyboard.press("r"); time.sleep(0.3)
            pg.mouse.move(700, 100); pg.mouse.down()
            for i in range(1, 16): pg.mouse.move(700 + i * 10, 100 + i * 8); time.sleep(0.02)
            pg.mouse.up(); time.sleep(0.6)
            n1 = call(pg, "window.drafftink.execute('element.query', {})")["value"]["total"]
            check(n1 == n0 + 1, "a mouse drag with the rectangle tool drew a rectangle", f"{n0} -> {n1}")
            rec = call(pg, "window.drafftink.recent({limit:3})")
            check(rec["ok"] and any(e.get("id") == "element.create" and e.get("source") == "ui" for e in rec["value"]["entries"]), "the mouse drag is a ui element.create in the ring", str(rec)[:300])
            if args.shot:
                os.makedirs(os.path.dirname(os.path.abspath(args.shot)), exist_ok=True)
                pg.screenshot(path=args.shot); print("screenshot", args.shot, flush=True)
            time.sleep(2)
            r = window(pg, f"idle after commands {args.idle_secs:g}s", args.idle_secs)
            check(r["frames"] == 0, "0 frames idle after commands", str(r["frames"]))
            call(pg, "window.drafftink.execute('selection.clear', {})")   # a selection opens the style panel
            r = hover_window(pg, "hover over empty canvas 5s", 5)
            check(r["frames"] == 0, "0 frames while the mouse moves over empty canvas", f'{r["frames"]} frames in {r["moves"]} moves')
            perf_now = pg.evaluate("()=>window.drafftink.perf()")
            check(isinstance(perf_now, dict) and perf_now["redraws"]["by_reason"].get("autosave", 0) >= 0 and frames(pg)["frames"] > 0, "perf() reports redraw reasons and frames() counts frames", str(perf_now["redraws"])[:200])
            # persistence: the workspace reaches IndexedDB
            time.sleep(3)
            keys = pg.evaluate("""()=>new Promise((res,rej)=>{const o=indexedDB.open('drafftink');o.onerror=()=>rej('open');o.onsuccess=()=>{const db=o.result;
                const r=db.transaction('kv').objectStore('kv').getAllKeys();r.onsuccess=()=>res(r.result);r.onerror=()=>rej('keys')}})""")
            check(len(keys) > 0, "IndexedDB holds the workspace after edits", str(keys)[:200])
            pg.reload(); time.sleep(0.5)
            for _ in range(600):
                if pg.evaluate("()=>window.__started||window.__error||false"): break
                time.sleep(0.1)
            q = call(pg, "window.drafftink.execute('element.query', {})")
            check(q["ok"] and q["value"].get("total", 0) >= 4, "the drawing is back after a reload", str(q)[:160])
            # the on-canvas text editor in a real browser: a draft by double click, typing into the hidden textarea,
            # one word selected with the keyboard and coloured through the style panel, Esc ends the edit
            import io as _io
            from PIL import Image as _Image
            r = call(pg, "window.drafftink.execute('doc.new', {on_unsaved:'discard'})"); time.sleep(0.8)
            check(r["ok"], "text editor: a blank document to type into", str(r)[:200])
            pg.mouse.move(700, 400); time.sleep(0.2); pg.mouse.dblclick(700, 400); time.sleep(0.8)
            te = call(pg, "window.drafftink.state()")["value"]["text_edit"]
            check(bool(te) and te.get("draft") is True, "text editor: a double click on empty canvas starts a draft", str(te))
            check(pg.evaluate("()=>document.activeElement&&document.activeElement.tagName") == "TEXTAREA", "text editor: the hidden textarea has the focus (IME, soft keyboard)")
            pg.keyboard.type("say red now", delay=40); time.sleep(0.4)
            pg.keyboard.press("Home")
            for _ in range(4): pg.keyboard.press("ArrowRight")
            for _ in range(3): pg.keyboard.press("Shift+ArrowRight")
            time.sleep(0.4)
            pg.mouse.click(456, 32); time.sleep(0.6)  # the red swatch of the stroke row
            check(bool(call(pg, "window.drafftink.state()")["value"]["text_edit"]), "text editor: a colour from the style panel does not end the edit")
            n_text0 = call(pg, "window.drafftink.execute('element.query', {})")["value"]["total"]
            check(n_text0 == 0, "text editor: nothing is in the document while the person types", str(n_text0))
            pg.keyboard.press("Escape"); time.sleep(0.8)
            check(call(pg, "window.drafftink.state()")["value"]["text_edit"] is None, "text editor: Esc ends the edit")
            ins = call(pg, "window.drafftink.execute('doc.inspect', {detail:'full'})")
            texts = [i.get("text") for i in ins["value"]["items"]] if ins["ok"] else []
            check(texts == ["say red now"], "text editor: the typed text is one text element", str(texts))
            # red ink where the word is, the text colour (indigo) elsewhere; no selection tint, no caret left behind
            png = _Image.open(_io.BytesIO(pg.screenshot(clip={"x": 690, "y": 395, "width": 200, "height": 50}))).convert("RGB")
            px = list(png.get_flattened_data()) if hasattr(png, 'get_flattened_data') else list(png.getdata())
            red = sum(1 for r, g, b in px if r > 200 and g < 110 and b < 110)
            indigo = sum(1 for r, g, b in px if b > 200 and r < 140 and g < 140)
            tint = sum(1 for r, g, b in px if (r, g, b) == (197, 213, 247))
            check(red > 30 and indigo > 30, "text editor: one word is red and the rest keeps the text colour", f"red {red}, indigo {indigo}")
            check(tint == 0, "text editor: the selection tint is gone after the edit", str(tint))
            r = call(pg, "window.drafftink.execute('history.undo', {})")
            check(r["ok"] and r["value"]["undone"] == ["Finish editing text"], "text editor: the whole edit is one undo step", str(r)[:200])
            # collab: page A on 127.0.0.1, page B on localhost (separate IndexedDB), same room
            room = "web-check-" + "".join(random.choice("abcdefghij") for _ in range(8))
            a, _ = open_app(ctx, f"{base}/?room={room}")
            time.sleep(8)  # join, first sync and the status settling
            # The server pings every 20 s and a peer sends presence every 10 s; 22 s covers both.
            r = window(a, "alone in a room, idle 22s", 22)
            check(r["frames"] == 0, "0 frames idle, alone in a room (server ping included)", f'{r["frames"]} frames, {r["redraw_requests"]}')
            b, _ = open_app(ctx, f"http://localhost:{port}/?room={room}")
            time.sleep(2.5)
            call(a, "window.drafftink.execute('element.create', {kind:'rectangle', name:'from-a', bounds:{x:0,y:0,w:80,h:50}})")
            seen = False
            for _ in range(80):
                q = call(b, "window.drafftink.execute('element.query', {fields:['id','name']})")
                if q["ok"] and any(i.get("name") == "from-a" for i in q["value"].get("items", [])): seen = True; break
                time.sleep(0.25)
            check(seen, "collab: page B sees the element page A drew", room)
            call(b, "window.drafftink.execute('element.create', {kind:'ellipse', name:'from-b', bounds:{x:200,y:0,w:80,h:50}})")
            seen = False
            for _ in range(80):
                q = call(a, "window.drafftink.execute('element.query', {fields:['id','name']})")
                if q["ok"] and any(i.get("name") == "from-b" for i in q["value"].get("items", [])): seen = True; break
                time.sleep(0.25)
            check(seen, "collab: page A sees the element page B drew")
            if args.shot:
                a.screenshot(path=args.shot.replace(".png", "-collab.png"))
            time.sleep(8)  # the edits are acknowledged and the peers' first presence has arrived
            r = window(a, "with a peer in the room, idle 22s", 22)
            check(r["frames"] == 0, "0 frames idle with a peer in the room (presence heartbeat included)", f'{r["frames"]} frames, {r["redraw_requests"]}')
            a.close(); b.close()
        # ---------------------------------------------------------------- offline
        pg.close()
        pg = ctx.new_page()
        pg.goto(base + "/"); time.sleep(1)
        sw = pg.evaluate("async()=>{const r=await navigator.serviceWorker.ready;return {state:r.active&&r.active.state, keys:(await caches.keys())}}")
        check(sw["state"] == "activated" and len(sw["keys"]) == 1, "service worker is active with one cache", str(sw))
        pg.close()
        server.stop()
        pg = ctx.new_page(); pg.on("console", lambda m: logs.append(f"{m.type}: {m.text[:300]}"))
        pg.goto(base + "/")
        for _ in range(600):
            if pg.evaluate("()=>window.__started||window.__error||false"): break
            time.sleep(0.1)
        check(pg.evaluate("()=>window.__started===true"), "offline: reload with the server stopped still starts the app", str(pg.evaluate("()=>window.__error||null")))
        check(pg.evaluate("()=>!!navigator.serviceWorker.controller"), "offline: the service worker controls the page")
        pg.goto(base + "/?room=offline-room")
        time.sleep(1)
        check(pg.evaluate("()=>document.title") != "", "offline: a room link also loads from the cache")
        ctx.close()
finally:
    server.stop(); xvfb.terminate(); shutil.rmtree(PROF, ignore_errors=True)

errs = [l for l in logs if l.startswith(("error", "PAGEERROR"))]
if errs: print("console errors:\n" + "\n".join(errs[:15]))
if fails: print("console tail:\n" + "\n".join(logs[-15:]))
if args.json: json.dump(results, open(args.json, "w"), indent=1)
print("RESULT:", "PASS" if not fails else "FAIL")
for f in fails: print("  FAIL", f)
sys.exit(1 if fails else 0)
