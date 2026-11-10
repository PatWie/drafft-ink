#!/usr/bin/env python3
"""Browser test for the web host (crates/gpu/src/web).

Builds examples/web_host_probe.rs for wasm32, runs wasm-bindgen, serves web/probe.html and drives it with
headless Chromium (no WebGPU needed). The probe records every host event as JSON in `window.__events`;
this script dispatches real and synthetic browser input and asserts the exact events.

usage: web_host_test.py [--no-build] [--port 8131] [--shot /tmp/error-page.png]
Run with ~/.venvs/perf/bin/python. Exit code 0 only if every check passes.
Does not need a display: headless Chromium. Kills everything it starts.
"""
import base64
import functools
import http.server
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading

from playwright.sync_api import sync_playwright

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.normpath(os.path.join(HERE, ".."))
ARG = sys.argv[1:]
PORT = int(ARG[ARG.index("--port") + 1]) if "--port" in ARG else 8131
SHOT = ARG[ARG.index("--shot") + 1] if "--shot" in ARG else "/tmp/web_host_error_page.png"
TARGET_DIR = os.environ.get("CARGO_TARGET_DIR", os.path.join(CRATE, "..", "..", "target"))
WASM_BINDGEN = os.path.expanduser("~/.cargo/bin/wasm-bindgen")

# A real 1x1 PNG.
PNG_B64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="
PNG = base64.b64decode(PNG_B64)
PNG_HEAD = PNG[:8].hex()

results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok), detail))
    print(("PASS " if ok else "FAIL ") + name + ("" if ok else f"   -> {detail}"), flush=True)


def build():
    env = dict(os.environ)
    subprocess.run(
        ["cargo", "build", "-p", "drafftink-gpu", "--example", "web_host_probe", "--target", "wasm32-unknown-unknown", "--profile", "dev"],
        cwd=CRATE, env=env, check=True,
    )
    wasm = os.path.join(TARGET_DIR, "wasm32-unknown-unknown", "debug", "examples", "web_host_probe.wasm")
    out = tempfile.mkdtemp(prefix="web_host_probe_")
    pkg = os.path.join(out, "pkg")
    subprocess.run([WASM_BINDGEN, "--target", "web", "--out-dir", pkg, wasm], check=True)
    shutil.copy(os.path.join(CRATE, "web", "probe.html"), os.path.join(out, "probe.html"))
    return out


def serve(root):
    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), functools.partial(Quiet, directory=root))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def events(page, *types):
    got = [json.loads(s) for s in page.evaluate("window.__events")]
    return [e for e in got if not types or e["t"] in types]


def clear(page):
    page.evaluate("window.__events.length = 0")


def pointer_events(page):
    return events(page, "pointer")


def js_make_png_file(name="p.png"):
    return f"new File([Uint8Array.from(atob('{PNG_B64}'), c => c.charCodeAt(0))], '{name}', {{type: 'image/png'}})"


def dispatch_prevented(page, selector, js_event):
    """Dispatch a synthetic event on an element; True when a listener called preventDefault."""
    return page.evaluate(f"() => !document.querySelector('{selector}').dispatchEvent({js_event})")


def key_event(key, **kw):
    opts = {"key": key, "bubbles": True, "cancelable": True}
    opts.update(kw)
    return f"new KeyboardEvent('keydown', {json.dumps(opts)})"


# ---------------------------------------------------------------------------------------------------------


def test_canvas_setup(page):
    style = page.evaluate("() => { const s = getComputedStyle(document.getElementById('canvas')); return [s.touchAction, s.outlineStyle, s.userSelect]; }")
    check("canvas: touch-action none, no outline, no user-select", style[0] == "none" and style[1] == "none" and style[2] == "none", style)
    check("canvas: tabindex set", page.evaluate("document.getElementById('canvas').getAttribute('tabindex')") == "0")
    prevented = dispatch_prevented(page, "#canvas", "new MouseEvent('contextmenu', {bubbles: true, cancelable: true})")
    check("canvas: contextmenu is prevented", prevented)
    agent = page.evaluate(
        "() => { const t = document.querySelector('textarea[data-drafftink=text-agent]'); if (!t) return null; const s = getComputedStyle(t);"
        " return {pos: s.position, opacity: s.opacity, w: s.width, parent: t.parentElement.tagName, active: document.activeElement === t}; }"
    )
    check("text agent: hidden textarea exists in <body>, not focused at start",
          agent and agent["pos"] == "fixed" and agent["opacity"] == "0" and agent["w"] == "1px" and agent["parent"] == "BODY" and not agent["active"], agent)


def test_mouse(page, ctx):
    clear(page)
    page.evaluate("window.__stamps.length = 0")
    page.mouse.move(150, 140)  # canvas is at (50, 40): local (100, 100)
    page.mouse.down()
    page.mouse.move(170, 160, steps=2)
    page.mouse.move(700, 500)  # outside the canvas: the pointer capture must keep the drag coming
    captured = page.evaluate("document.getElementById('canvas').hasPointerCapture(1)")
    page.mouse.up()
    evs = pointer_events(page)
    phases = [(e["phase"], e["kind"]) for e in evs]
    check("mouse: Hover, Down, 3 x Move, Up, then Leave (released outside the canvas)",
          phases == [("Hover", "Mouse"), ("Down", "Mouse"), ("Move", "Mouse"), ("Move", "Mouse"), ("Move", "Mouse"), ("Up", "Mouse"), ("Leave", "Mouse")], phases)
    down = next((e for e in evs if e["phase"] == "Down"), None)
    check("mouse: down at canvas-relative CSS px, button 0, no pressure, id 1",
          down and down["point"]["x"] == 100 and down["point"]["y"] == 100 and down["button"] == 0 and down["point"]["pressure"] is None and down["id"] == 1 and down["tilt"] is None, down)
    check("mouse: pointer captured during the drag", captured)
    outside = [e for e in evs if e["phase"] == "Move" and e["point"]["x"] > 400]
    check("mouse: moves outside the canvas arrive while captured (x=650, y=460)", outside and outside[-1]["point"]["x"] == 650 and outside[-1]["point"]["y"] == 460, outside)
    up = next((e for e in evs if e["phase"] == "Up"), None)
    check("mouse: up carries button 0", up and up["button"] == 0, up)
    check("mouse: capture released after up", not page.evaluate("document.getElementById('canvas').hasPointerCapture(1)"))
    page.mouse.move(160, 150)
    check("mouse: moving back over the canvas after release is Hover", pointer_events(page)[-1]["phase"] == "Hover", pointer_events(page)[-1])
    # Time: time_ms is event.timeStamp plus one constant offset.
    stamps = page.evaluate("window.__stamps")[: len(evs)]
    host = list(evs)
    if len(stamps) == len(host) and len(host) > 2:
        offsets = [h["point"]["time_ms"] - s["t"] for h, s in zip(host, stamps)]
        check("time: time_ms - event.timeStamp is constant", max(offsets) - min(offsets) < 0.01, offsets[:4])
    else:
        check("time: one stamp per host event", False, (len(stamps), len(host)))
    check("time: monotonic", all(a["point"]["time_ms"] <= b["point"]["time_ms"] for a, b in zip(host, host[1:])))
    # Modifiers on a real mouse event.
    clear(page)
    page.keyboard.down("Shift")
    page.keyboard.down("Control")
    page.mouse.move(160, 150)
    page.keyboard.up("Control")
    page.keyboard.up("Shift")
    last = pointer_events(page)[-1]
    m = last["mods"]
    check("mouse: modifiers shift+ctrl, command = ctrl (non-mac)", m["shift"] and m["ctrl"] and m["command"] and not m["mac_cmd"] and not m["alt"], m)


def pointer_js(kind, ptype, x, y, buttons, pressure, extra=""):
    return (
        f"(() => {{ const e = new PointerEvent('{kind}', {{bubbles: true, cancelable: true, pointerId: 7, pointerType: '{ptype}', clientX: {x}, clientY: {y}, "
        f"buttons: {buttons}, button: {0 if buttons else -1}, pressure: {pressure}, tiltX: 20, tiltY: -10, isPrimary: true{extra}}}); "
        f"document.getElementById('canvas').dispatchEvent(e); return e; }})()"
    )


def test_pen_and_touch_synthetic(page):
    clear(page)
    page.evaluate(pointer_js("pointermove", "pen", 150.5, 140.25, 0, 0))
    page.evaluate(pointer_js("pointerdown", "pen", 150.5, 140.25, 1, 0.5))
    page.evaluate(pointer_js("pointermove", "pen", 160.5, 150.25, 1, 0.75))
    page.evaluate(pointer_js("pointerup", "pen", 160.5, 150.25, 0, 0))
    evs = pointer_events(page)
    got = [(e["phase"], e["kind"], e["point"]["pressure"]) for e in evs]
    check("pen (synthetic): phases and pressure (hover 0, down 0.5, move 0.75, up 0)",
          got == [("Hover", "Pen", 0.0), ("Down", "Pen", 0.5), ("Move", "Pen", 0.75), ("Up", "Pen", 0.0)], got)
    check("pen (synthetic): fractional position 100.5, 100.25", evs[1]["point"]["x"] == 100.5 and evs[1]["point"]["y"] == 100.25, evs[1]["point"])
    check("pen (synthetic): tilt [20, -10] and id 7", evs[1]["tilt"] == [20.0, -10.0] and evs[1]["id"] == 7, evs[1])
    # Touch: 0.5 is the value browsers give without pressure sensing -> reported as None.
    clear(page)
    page.evaluate(pointer_js("pointerdown", "touch", 150, 140, 1, 0.5))
    page.evaluate(pointer_js("pointermove", "touch", 151, 141, 1, 0.8))
    page.evaluate(pointer_js("pointercancel", "touch", 151, 141, 0, 0))
    page.evaluate(pointer_js("pointerleave", "touch", 151, 141, 0, 0))
    evs = pointer_events(page)
    got = [(e["phase"], e["kind"], e["point"]["pressure"], e["tilt"]) for e in evs]
    check("touch (synthetic): down(0.5 -> None), move 0.8, cancel, leave; no tilt",
          [(g[0], g[1], g[3]) for g in got] == [("Down", "Touch", None), ("Move", "Touch", None), ("Cancel", "Touch", None), ("Leave", "Touch", None)]
          and [g[2] for g in got][0] is None and abs(got[1][2] - 0.8) < 1e-6, got)
    # Coalesced events: stub getCoalescedEvents on the event instance.
    clear(page)
    page.evaluate(
        """() => {
            const mk = (x, y) => new PointerEvent('pointermove', {pointerId: 7, pointerType: 'pen', clientX: x, clientY: y, buttons: 1, pressure: 0.4});
            // Created oldest first: the coalesced events must not have a later time stamp than the main one.
            const a = mk(152, 142), b = mk(161, 151);
            const main = new PointerEvent('pointermove', {bubbles: true, pointerId: 7, pointerType: 'pen', clientX: 170, clientY: 160, buttons: 1, pressure: 0.6});
            Object.defineProperty(main, 'getCoalescedEvents', {value: () => [a, b, main]});
            document.getElementById('canvas').dispatchEvent(main);
        }"""
    )
    e = pointer_events(page)[0]
    co = e["coalesced"]
    check("coalesced: two intermediate points oldest first, dispatched event itself dropped",
          len(co) == 2 and co[0]["x"] == 102 and co[0]["y"] == 102 and co[1]["x"] == 111 and co[1]["y"] == 111 and abs(co[0]["pressure"] - 0.4) < 1e-6
          and co[0]["time_ms"] <= co[1]["time_ms"] <= e["point"]["time_ms"], e)
    clear(page)
    page.evaluate(
        """() => {
            const main = new PointerEvent('pointermove', {bubbles: true, pointerId: 7, pointerType: 'pen', clientX: 170, clientY: 160, buttons: 1, pressure: 0.6});
            Object.defineProperty(main, 'getCoalescedEvents', {value: () => { throw new TypeError('nope'); }});
            document.getElementById('canvas').dispatchEvent(main);
        }"""
    )
    check("coalesced: a throwing getCoalescedEvents gives an empty list, no crash", pointer_events(page)[0]["coalesced"] == [], pointer_events(page))
    clear(page)
    page.evaluate(
        """() => {
            const main = new PointerEvent('pointermove', {bubbles: true, pointerId: 7, pointerType: 'pen', clientX: 170, clientY: 160, buttons: 1, pressure: 0.6});
            Object.defineProperty(main, 'getCoalescedEvents', {value: undefined});
            document.getElementById('canvas').dispatchEvent(main);
        }"""
    )
    check("coalesced: browser without getCoalescedEvents -> empty list", pointer_events(page)[0]["coalesced"] == [], pointer_events(page))


def test_pen_and_touch_cdp(page, ctx):
    cdp = ctx.new_cdp_session(page)
    clear(page)
    base = {"x": 150, "y": 140, "pointerType": "pen"}
    cdp.send("Input.dispatchMouseEvent", {**base, "type": "mouseMoved", "tiltX": 10, "tiltY": -5, "force": 0})
    cdp.send("Input.dispatchMouseEvent", {**base, "type": "mousePressed", "button": "left", "buttons": 1, "clickCount": 1, "force": 0.6, "tiltX": 10, "tiltY": -5})
    cdp.send("Input.dispatchMouseEvent", {**base, "x": 160, "y": 150, "type": "mouseMoved", "button": "left", "buttons": 1, "force": 0.9, "tiltX": 12, "tiltY": -6})
    cdp.send("Input.dispatchMouseEvent", {**base, "x": 160, "y": 150, "type": "mouseReleased", "button": "left", "buttons": 0, "clickCount": 1, "force": 0})
    evs = pointer_events(page)
    pen = [(e["phase"], e["kind"]) for e in evs if e["kind"] == "Pen"]
    check("pen (CDP Input.dispatchMouseEvent pointerType=pen): Hover, Down, Move, Up", pen[:4] == [("Hover", "Pen"), ("Down", "Pen"), ("Move", "Pen"), ("Up", "Pen")], [(e["phase"], e["kind"]) for e in evs])
    down = next((e for e in evs if e["phase"] == "Down" and e["kind"] == "Pen"), None)
    move = next((e for e in evs if e["phase"] == "Move" and e["kind"] == "Pen"), None)
    check("pen (CDP): pressure 0.6 on down, 0.9 on move", down and move and abs(down["point"]["pressure"] - 0.6) < 1e-3 and abs(move["point"]["pressure"] - 0.9) < 1e-3, (down, move))
    check("pen (CDP): tilt reported on down", down and down["tilt"] == [10.0, -5.0], down and down["tilt"])
    hover = evs[0]
    check("pen (CDP): hover has pressure 0 and phase Hover", hover["phase"] == "Hover" and hover["point"]["pressure"] == 0.0, hover)
    # Real touch through CDP.
    clear(page)
    cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": 150, "y": 140, "id": 3, "force": 0.8}]})
    cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": 155, "y": 145, "id": 3, "force": 0.8}]})
    cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
    evs = [e for e in pointer_events(page) if e["kind"] == "Touch"]
    got = [(e["phase"], e["point"]["pressure"]) for e in evs]
    check("touch (CDP): Down, Move, Up with pressure from force",
          [g[0] for g in got][:3] == ["Down", "Move", "Up"] and got[0][1] is not None and abs(got[0][1] - 0.8) < 1e-3, got)
    check("touch (CDP): positions are canvas-relative", evs and evs[0]["point"]["x"] == 100 and evs[0]["point"]["y"] == 100, evs[:1])


def test_visibility_focus(page):
    clear(page)
    page.evaluate("Object.defineProperty(document, 'hidden', {value: true, configurable: true}); document.dispatchEvent(new Event('visibilitychange'))")
    page.evaluate("Object.defineProperty(document, 'hidden', {value: false, configurable: true}); document.dispatchEvent(new Event('visibilitychange'))")
    page.evaluate("window.dispatchEvent(new Event('blur')); window.dispatchEvent(new Event('focus'))")
    got = [(e["t"], e.get("visible", e.get("focused"))) for e in events(page, "visibility", "focus")]
    check("visibilitychange -> Visibility(false), Visibility(true); window blur/focus -> Focus(false), Focus(true)",
          got == [("visibility", False), ("visibility", True), ("focus", False), ("focus", True)], got)


def test_paste(page):
    clear(page)
    html = f"""() => {{
        const dt = new DataTransfer();
        dt.setData('text/plain', 'hello\\r\\nworld');
        dt.items.add({js_make_png_file()});
        const ev = new ClipboardEvent('paste', {{clipboardData: dt, bubbles: true, cancelable: true}});
        return !document.getElementById('canvas').dispatchEvent(ev);
    }}"""
    prevented = page.evaluate(html)
    page.wait_for_function("window.__events.length >= 1")
    pastes = events(page, "paste")
    check("paste: one Paste with text (CRLF -> LF) and the PNG image", len(pastes) == 1 and pastes[0]["text"] == "hello\nworld"
          and len(pastes[0]["images"]) == 1 and pastes[0]["images"][0]["mime"] == "image/png"
          and pastes[0]["images"][0]["len"] == len(PNG) and pastes[0]["images"][0]["head"] == PNG_HEAD, pastes)
    check("paste: default prevented (the textarea must not insert the text)", prevented)
    clear(page)
    page.evaluate("""() => {
        const dt = new DataTransfer();
        dt.setData('text/plain', 'only text');
        document.getElementById('canvas').dispatchEvent(new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true}));
    }""")
    p = events(page, "paste")
    check("paste: text only is delivered synchronously, no images", len(p) == 1 and p[0]["text"] == "only text" and p[0]["images"] == [] and p[0]["custom"] is None, p)
    clear(page)
    page.evaluate("""() => {
        const dt = new DataTransfer();
        dt.setData('text/plain', 'x');
        dt.setData('web application/x-drafftink', '{"a":1}');
        document.getElementById('canvas').dispatchEvent(new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true}));
    }""")
    p = events(page, "paste")
    custom_in_types = page.evaluate("(() => { const dt = new DataTransfer(); dt.setData('web application/x-drafftink', '1'); return Array.from(dt.types); })()")
    if "web application/x-drafftink" in custom_in_types:
        check("paste: custom format 'web application/x-drafftink' delivered", len(p) == 1 and p[0]["custom"] and p[0]["custom"]["text"] == '{"a":1}', p)
    else:
        check("paste: custom format not representable in this DataTransfer (types=%s); text still delivered" % custom_in_types, len(p) == 1 and p[0]["text"] == "x", p)
    clear(page)
    page.evaluate("""() => {
        const dt = new DataTransfer();
        document.getElementById('canvas').dispatchEvent(new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true}));
    }""")
    check("paste: an empty clipboard sends nothing", events(page, "paste") == [])
    # Pasting into a form field of the page is not ours.
    clear(page)
    prevented = page.evaluate("""() => {
        const dt = new DataTransfer(); dt.setData('text/plain', 'for the field');
        return !document.getElementById('foreign').dispatchEvent(new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true}));
    }""")
    check("paste: into a foreign <input> is ignored and not prevented", events(page, "paste") == [] and not prevented)
    # Real Ctrl+V: the browser must fire `paste` (nothing cancelled the key press).
    page.evaluate("document.getElementById('canvas').focus()")
    clear(page)
    page.evaluate("navigator.clipboard.writeText('from the real clipboard')")
    page.keyboard.press("Control+V")
    page.wait_for_timeout(200)
    p = events(page, "paste")
    check("paste: real Ctrl+V on the canvas fires paste with the clipboard text", len(p) == 1 and p[0]["text"] == "from the real clipboard", p)


def test_copy_cut(page):
    clear(page)
    prevented = dispatch_prevented(page, "#canvas", "new ClipboardEvent('copy', {bubbles: true, cancelable: true, clipboardData: new DataTransfer()})")
    cut_prevented = dispatch_prevented(page, "#canvas", "new ClipboardEvent('cut', {bubbles: true, cancelable: true, clipboardData: new DataTransfer()})")
    got = [(e["t"], e["cut"]) for e in events(page, "copy")]
    check("copy/cut events -> CopyRequested{cut:false}, CopyRequested{cut:true}", got == [("copy", False), ("copy", True)], got)
    check("copy/cut: not prevented when the app writes nothing", not prevented and not cut_prevented)
    # The app answers inside the event: text goes through event.clipboardData.
    clear(page)
    page.evaluate("() => { window.__hook = (json) => { if (JSON.parse(json).t === 'copy') window.probe.probe_write_clipboard('written in the event'); }; }")
    r = page.evaluate("""() => {
        const dt = new DataTransfer();
        const ev = new ClipboardEvent('copy', {bubbles: true, cancelable: true, clipboardData: dt});
        const notPrevented = document.getElementById('canvas').dispatchEvent(ev);
        return {prevented: !notPrevented, data: dt.getData('text/plain')};
    }""")
    page.evaluate("() => { window.__hook = null; }")
    check("copy: write_clipboard inside the event fills event.clipboardData and prevents the default", r["prevented"] and r["data"] == "written in the event", r)
    foreign = page.evaluate("""() => { const f = document.getElementById('foreign');
        return !f.dispatchEvent(new ClipboardEvent('copy', {bubbles: true, cancelable: true, clipboardData: new DataTransfer()})); }""")
    check("copy: in a foreign <input> is ignored", len(events(page, "copy")) == 1 and not foreign, events(page, "copy"))
    # Real Ctrl+C on the focused canvas.
    clear(page)
    page.evaluate("document.getElementById('canvas').focus()")
    page.keyboard.press("Control+C")
    page.wait_for_timeout(150)
    got = [(e["t"], e["cut"]) for e in events(page, "copy")]
    check("copy: real Ctrl+C on the canvas fires copy (nothing cancelled the key)", got == [("copy", False)], got)
    clear(page)
    page.keyboard.press("Control+X")
    page.wait_for_timeout(150)
    got = [(e["t"], e["cut"]) for e in events(page, "copy")]
    check("cut: real Ctrl+X on the canvas fires cut", got == [("copy", True)], got)


def is_png_1x1(data):
    """Chromium re-encodes images it puts on the clipboard, so compare the signature and the size."""
    data = bytes(data)
    return data[:8] == PNG[:8] and data[16:24] == (1).to_bytes(4, "big") * 2


def clipboard_state(page):
    return page.evaluate(
        """async () => {
            const items = await navigator.clipboard.read();
            const out = [];
            for (const item of items) {
                const o = {types: item.types};
                if (item.types.includes('text/plain')) o.text = await (await item.getType('text/plain')).text();
                if (item.types.includes('image/png')) o.png = Array.from(new Uint8Array(await (await item.getType('image/png')).arrayBuffer()));
                if (item.types.includes('web application/x-drafftink')) o.custom = await (await item.getType('web application/x-drafftink')).text();
                out.push(o);
            }
            return out;
        }"""
    )


def test_clipboard_write_read(page):
    page.evaluate("document.getElementById('canvas').focus()")
    page.evaluate("window.probe.probe_write_clipboard('round trip: ünïcode ✓')")
    page.wait_for_timeout(300)
    text = page.evaluate("navigator.clipboard.readText()")
    check("clipboard write: text round-trips through navigator.clipboard.readText()", text == "round trip: ünïcode ✓", text)

    page.evaluate(f"window.probe.probe_write_clipboard_rich('rich text', Uint8Array.from(atob('{PNG_B64}'), c => c.charCodeAt(0)), '{{\"elements\":[1,2]}}')")
    page.wait_for_timeout(400)
    state = clipboard_state(page)
    item = state[0] if state else {}
    check("clipboard write: text + PNG + custom type in one ClipboardItem",
          item.get("text") == "rich text" and is_png_1x1(item.get("png", [])) and item.get("custom") == '{"elements":[1,2]}', item.get("types"))

    clear(page)
    page.evaluate("window.probe.probe_read_clipboard()")
    page.wait_for_function("window.__events.length >= 1", timeout=5000)
    p = events(page, "paste")
    check("read_clipboard: sends Paste with text, image and custom payload",
          len(p) == 1 and p[0]["text"] == "rich text" and p[0]["images"] and p[0]["images"][0]["head"] == PNG_HEAD and p[0]["custom"] and p[0]["custom"]["text"] == '{"elements":[1,2]}', p)

    # Fallback 1: the ClipboardItem constructor rejects the custom type (Safari, Firefox).
    page.evaluate("""() => {
        window.__OrigItem = window.ClipboardItem;
        window.ClipboardItem = class extends window.__OrigItem {
            constructor(record) { if (Object.keys(record).some(k => k.startsWith('web '))) throw new TypeError('unsupported type'); super(record); }
        };
    }""")
    page.evaluate(f"window.probe.probe_write_clipboard_rich('no custom', Uint8Array.from(atob('{PNG_B64}'), c => c.charCodeAt(0)), 'private')")
    page.wait_for_timeout(400)
    item = clipboard_state(page)[0]
    check("clipboard write fallback: constructor rejects the custom type -> text + PNG without it",
          item.get("text") == "no custom" and is_png_1x1(item.get("png", [])) and "custom" not in item, item.get("types"))
    # Fallback 2: no ClipboardItem at all -> writeText.
    page.evaluate("() => { window.ClipboardItem = undefined; }")
    page.evaluate(f"window.probe.probe_write_clipboard_rich('text only fallback', Uint8Array.from(atob('{PNG_B64}'), c => c.charCodeAt(0)), 'private')")
    page.wait_for_timeout(400)
    page.evaluate("() => { window.ClipboardItem = window.__OrigItem; }")
    check("clipboard write fallback: no ClipboardItem -> writeText", page.evaluate("navigator.clipboard.readText()") == "text only fallback")
    # Fallback 3: the rich write is rejected by write() -> text only.
    page.evaluate("""() => {
        window.__origWrite = navigator.clipboard.write;
        navigator.clipboard.write = () => Promise.reject(new DOMException('denied', 'NotAllowedError'));
    }""")
    page.evaluate(f"window.probe.probe_write_clipboard_rich('after rejection', Uint8Array.from(atob('{PNG_B64}'), c => c.charCodeAt(0)), 'private')")
    page.wait_for_timeout(400)
    page.evaluate("() => { navigator.clipboard.write = window.__origWrite; }")
    check("clipboard write fallback: write() rejected (NotAllowedError) -> writeText", page.evaluate("navigator.clipboard.readText()") == "after rejection")


def test_clipboard_unavailable(page, logs):
    # Insecure context / old browser: navigator.clipboard is missing. Must warn, never throw.
    page.evaluate("Object.defineProperty(navigator, 'clipboard', {value: undefined, configurable: true})")
    n = len(logs)
    clear(page)
    page.evaluate("window.probe.probe_write_clipboard('nowhere')")
    page.evaluate("window.probe.probe_read_clipboard()")
    page.wait_for_timeout(300)
    new = logs[n:]
    check("clipboard unavailable: write and read log a warning, send nothing, no exception",
          any("clipboard write failed" in m for m in new) and any("clipboard read failed" in m for m in new) and events(page, "paste") == []
          and not any(m.startswith("PAGEERROR") for m in new), new)
    page.evaluate("delete navigator.clipboard")  # remove the instance override: the prototype getter is back


def test_files(page):
    clear(page)
    make_dt = f"""const dt = new DataTransfer();
        dt.items.add({js_make_png_file('pic.png')});
        dt.items.add(new File(['{{"a":1}}'], 'doc.json', {{type: ''}}));"""
    def fire(kind, extra=""):
        return page.evaluate(f"""() => {{ {make_dt}
            const ev = new DragEvent('{kind}', {{bubbles: true, cancelable: true, dataTransfer: dt}});
            return !document.getElementById('canvas').dispatchEvent(ev); }}""")
    over_prevented = fire("dragenter")
    over2 = fire("dragover")
    check("drag: dragenter/dragover with files are prevented (so the drop is allowed)", over_prevented and over2)
    check("drag: FileHover(true) once (not per dragover)", [e["on"] for e in events(page, "file_hover")] == [True], events(page, "file_hover"))
    fire("dragleave")
    check("drag: dragleave -> FileHover(false)", [e["on"] for e in events(page, "file_hover")] == [True, False])
    clear(page)
    fire("dragenter")
    drop_prevented = fire("drop")
    page.wait_for_function("window.__events.some(s => s.includes('files_dropped'))", timeout=5000)
    ev = events(page)
    dropped = [e for e in ev if e["t"] == "files_dropped"]
    check("drop: FileHover(true), FileHover(false), then FilesDropped", [e["t"] for e in ev] == ["file_hover", "file_hover", "files_dropped"] and not ev[1]["on"], ev)
    files = dropped[0]["files"] if dropped else []
    check("drop: both files read completely, in order, with mime (empty type guessed from the extension)",
          [(f["name"], f["mime"], f["len"]) for f in files] == [("pic.png", "image/png", len(PNG)), ("doc.json", "application/json", 7)] and files[0]["head"] == PNG_HEAD, files)
    check("drop: default prevented (the browser must not open the file)", drop_prevented)
    # A text drag is not ours.
    clear(page)
    text_prevented = page.evaluate("""() => { const dt = new DataTransfer(); dt.setData('text/plain', 'x');
        return !document.getElementById('canvas').dispatchEvent(new DragEvent('dragover', {bubbles: true, cancelable: true, dataTransfer: dt})); }""")
    check("drag: text drags are neither prevented nor reported", not text_prevented and events(page) == [])
    # A drop beside the canvas must not navigate.
    beside = page.evaluate(f"""() => {{ {make_dt}
        const a = !document.body.dispatchEvent(new DragEvent('dragover', {{bubbles: true, cancelable: true, dataTransfer: dt}}));
        const b = !document.body.dispatchEvent(new DragEvent('drop', {{bubbles: true, cancelable: true, dataTransfer: dt}}));
        return [a, b]; }}""")
    check("drop beside the canvas: dragover and drop are prevented (no navigation to the file)", beside == [True, True], beside)
    # The 256 MB cap.
    clear(page)
    page.evaluate("""() => { const dt = new DataTransfer();
        dt.items.add(new File([new Uint8Array(257 * 1024 * 1024)], 'huge.bin', {type: 'application/octet-stream'}));
        dt.items.add(new File(['ok'], 'small.txt', {type: 'text/plain'}));
        document.getElementById('canvas').dispatchEvent(new DragEvent('drop', {bubbles: true, cancelable: true, dataTransfer: dt})); }""")
    page.wait_for_function("window.__events.some(s => s.includes('files_dropped'))", timeout=15000)
    files = [e for e in events(page, "files_dropped")][0]["files"]
    check("drop: a 257 MB file is skipped (limit 256 MB), the other file still arrives", [f["name"] for f in files] == ["small.txt"], files)


def tick(page, ms=60):
    page.wait_for_timeout(ms)


def test_text_agent(page, ctx, logs):
    cdp = ctx.new_cdp_session(page)
    clear(page)
    page.evaluate("window.probe.probe_set_text_input(50, 40, 2, 20)")
    st = page.evaluate("""() => { const t = document.querySelector('textarea[data-drafftink=text-agent]'); const r = t.getBoundingClientRect();
        return {active: document.activeElement === t, left: r.left, top: r.top, h: r.height, focused: window.probe.probe_text_input_focused()}; }""")
    check("set_text_input(Some): textarea focused and placed at the caret (canvas origin 50,40 + caret 50,40)",
          st["active"] and st["focused"] and st["left"] == 100 and st["top"] == 80 and st["h"] == 20, st)
    check("set_text_input(Some): egui WindowFocused(true) sent for the textarea", {"t": "egui_window_focused", "focused": True} in events(page), events(page))

    # Typing.
    clear(page)
    page.keyboard.type("ab")
    got = [(e["t"], e.get("key", e.get("text")), e.get("pressed")) for e in events(page)]
    check("typing 'ab': Key down, Text, Key up per character, no doubled characters",
          got == [("egui_key", "A", True), ("egui_text", "a", None), ("text_input", "a", None), ("egui_key", "A", False),
                  ("egui_key", "B", True), ("egui_text", "b", None), ("text_input", "b", None), ("egui_key", "B", False)], got)
    check("typing: the textarea stays empty", page.evaluate("document.querySelector('textarea[data-drafftink=text-agent]').value") == "")

    # Editing keys: one press + one release each, no text.
    for key, egui_key in [("Backspace", "Backspace"), ("ArrowLeft", "ArrowLeft"), ("ArrowRight", "ArrowRight"), ("ArrowUp", "ArrowUp"), ("ArrowDown", "ArrowDown"),
                          ("Enter", "Enter"), ("Tab", "Tab"), ("Escape", "Escape"), ("Delete", "Delete"), ("Home", "Home"), ("End", "End"), ("PageUp", "PageUp")]:
        clear(page)
        page.keyboard.press(key)
        got = [(e["t"], e.get("key"), e.get("pressed")) for e in events(page)]
        check(f"key {key} in the textarea -> exactly Key({egui_key}) down and up, no text", got == [("egui_key", egui_key, True), ("egui_key", egui_key, False)], got)
    check("Tab did not move the focus away from the textarea", page.evaluate("document.activeElement.tagName") == "TEXTAREA")
    clear(page)
    page.keyboard.press("Space")
    got = [(e["t"], e.get("key", e.get("text"))) for e in events(page)]
    check("Space types a space: Key(Space) and Text(' ')", ("egui_key", "Space") in got and ("egui_text", " ") in got, got)
    for chord, egui_key in [("Control+A", "A"), ("Control+Z", "Z"), ("Control+Y", "Y"), ("Control+Shift+Z", "Z")]:
        clear(page)
        page.keyboard.press(chord)
        evs = events(page)
        downs = [e for e in evs if e["t"] == "egui_key" and e["pressed"] and e["key"] == egui_key]
        ups = [e for e in evs if e["t"] == "egui_key" and not e["pressed"] and e["key"] == egui_key]
        texts = [e for e in evs if e["t"] in ("egui_text", "text_input")]
        check(f"{chord}: Key({egui_key}) with ctrl+command set, released, no text, no stuck key",
              len(downs) == 1 and downs[0]["mods"]["ctrl"] and downs[0]["mods"]["command"] and len(ups) >= 1 and not texts, evs)
    clear(page)
    page.keyboard.down("Shift")
    page.keyboard.press("ArrowLeft")
    page.keyboard.up("Shift")
    d = [e for e in events(page) if e["t"] == "egui_key" and e["key"] == "ArrowLeft" and e["pressed"]]
    check("Shift+ArrowLeft carries shift", d and d[0]["mods"]["shift"], d)
    clear(page)
    page.keyboard.down("a")
    page.keyboard.down("a")  # auto repeat
    page.keyboard.up("a")
    d = [e for e in events(page) if e["t"] == "egui_key" and e["key"] == "A" and e["pressed"]]
    check("auto repeat: second keydown has repeat=true", len(d) == 2 and not d[0]["repeat"] and d[1]["repeat"], d)

    # IME through CDP (the real composition pipeline of Chromium).
    clear(page)
    cdp.send("Input.imeSetComposition", {"text": "にほ", "selectionStart": 2, "selectionEnd": 2})
    cdp.send("Input.imeSetComposition", {"text": "にほん", "selectionStart": 3, "selectionEnd": 3})
    mid = events(page)
    cdp.send("Input.insertText", {"text": "日本"})
    tick(page)
    evs = events(page)
    kinds = [(e["t"], e.get("kind"), e.get("text")) for e in evs]
    check("IME (CDP): composition start enables egui IME, updates give Preedit",
          ("egui_ime", "enabled", None) in kinds and ("egui_ime", "preedit", "にほ") in kinds and ("egui_ime", "preedit", "にほん") in kinds
          and ("text_input", "preedit", "にほ") in kinds and ("text_input", "preedit", "にほん") in kinds, [(e["t"], e.get("kind"), e.get("text")) for e in mid])
    commits = [k for k in kinds if k[1] == "commit"]
    check("IME (CDP): commit '日本' sent once to egui and once to the app, then Disabled, and no extra Text",
          commits.count(("egui_ime", "commit", "日本")) == 1 and commits.count(("text_input", "commit", "日本")) == 1
          and ("egui_ime", "disabled", None) in kinds and not [e for e in evs if e["t"] == "egui_text"], kinds)
    check("IME: textarea is empty after the commit", page.evaluate("document.querySelector('textarea[data-drafftink=text-agent]').value") == "")
    # Cancel: composition cleared without commit.
    clear(page)
    cdp.send("Input.imeSetComposition", {"text": "か", "selectionStart": 1, "selectionEnd": 1})
    cdp.send("Input.imeSetComposition", {"text": "", "selectionStart": 0, "selectionEnd": 0})
    tick(page)
    kinds = [(e["t"], e.get("kind"), e.get("text")) for e in events(page)]
    check("IME (CDP): composition cleared without commit -> Preedit(\"\") for egui and Cancel for the app",
          ("egui_ime", "preedit", "") in kinds and ("text_input", "cancel", None) in kinds and not [k for k in kinds if k[1] == "commit"], kinds)
    # Synthetic composition events (Firefox/Safari order: compositionend, then insertText).
    clear(page)
    page.evaluate("""() => { const t = document.querySelector('textarea[data-drafftink=text-agent]');
        t.dispatchEvent(new CompositionEvent('compositionstart', {data: '', bubbles: true}));
        t.dispatchEvent(new CompositionEvent('compositionupdate', {data: 'ni', bubbles: true}));
        t.dispatchEvent(new CompositionEvent('compositionend', {data: 'ni', bubbles: true}));
        t.dispatchEvent(new InputEvent('input', {data: 'ni', inputType: 'insertText', isComposing: false, bubbles: true})); }""")
    kinds = [(e["t"], e.get("kind"), e.get("text")) for e in events(page)]
    check("IME (synthetic): start/update/end give Enabled, Preedit, Commit, Disabled; the browser's follow-up insertText is not sent twice",
          kinds == [("egui_ime", "enabled", None), ("egui_ime", "preedit", "ni"), ("text_input", "preedit", "ni"), ("egui_ime", "commit", "ni"),
                    ("text_input", "commit", "ni"), ("egui_ime", "disabled", None)], kinds)
    # Plain insertText with no key events (a soft keyboard, a dictation result, CDP insertText).
    clear(page)
    cdp.send("Input.insertText", {"text": "é"})
    got = [(e["t"], e.get("text")) for e in events(page)]
    check("insertText without composition (soft keyboard): Text + TextInput commit", got == [("egui_text", "é"), ("text_input", "é")], got)
    # Keys during composition are the IME's.
    clear(page)
    page.evaluate("""() => document.querySelector('textarea[data-drafftink=text-agent]').dispatchEvent(
        new KeyboardEvent('keydown', {key: 'Process', keyCode: 229, isComposing: true, bubbles: true, cancelable: true}))""")
    check("keydown during composition (keyCode 229) is ignored", events(page) == [], events(page))
    # Android: Backspace arrives as keyCode 229 + beforeinput(deleteContentBackward).
    clear(page)
    page.evaluate("""() => { const t = document.querySelector('textarea[data-drafftink=text-agent]');
        t.dispatchEvent(new KeyboardEvent('keydown', {key: 'Unidentified', keyCode: 229, bubbles: true, cancelable: true}));
        t.dispatchEvent(new InputEvent('beforeinput', {inputType: 'deleteContentBackward', bubbles: true, cancelable: true})); }""")
    got = [(e["t"], e.get("key"), e.get("pressed")) for e in events(page)]
    check("soft keyboard Backspace (keyCode 229 + beforeinput deleteContentBackward) -> Key(Backspace) down and up",
          got == [("egui_key", "Backspace", True), ("egui_key", "Backspace", False)], got)

    # preventDefault policy, on the textarea.
    def prevented_in_textarea(js):
        return page.evaluate(f"() => !document.querySelector('textarea[data-drafftink=text-agent]').dispatchEvent({js})")
    check("textarea keydown Space not prevented (it must type), Tab prevented", not prevented_in_textarea(key_event(" ")) and prevented_in_textarea(key_event("Tab")))

    # set_text_input(None).
    clear(page)
    page.evaluate("window.probe.probe_clear_text_input()")
    st = page.evaluate("() => ({active: document.activeElement.id, ta: document.activeElement.tagName})")
    check("set_text_input(None): textarea blurred, focus is back on the canvas", st["active"] == "canvas", st)
    check("set_text_input(None): no WindowFocused(false) for the focus hand-over", not [e for e in events(page, "egui_window_focused") if not e["focused"]], events(page))
    # Typing now goes to the canvas (winit's job); the host sees no key.
    clear(page)
    page.keyboard.type("q")
    check("after None, key presses are not handled by the textarea (no events from the host)", events(page) == [], events(page))

    # Edge-triggered focus, tap refocus.
    clear(page)
    page.evaluate("window.probe.probe_set_text_input(10, 10, 2, 20)")
    page.evaluate("document.activeElement.blur()")
    ev = events(page, "egui_window_focused")
    check("user dismisses the keyboard (blur): WindowFocused(false) sent", ev and ev[-1]["focused"] is False, ev)
    page.evaluate("window.probe.probe_set_text_input(30, 30, 2, 20)")
    st = page.evaluate("() => { const t = document.querySelector('textarea[data-drafftink=text-agent]'); return {active: document.activeElement === t, left: t.getBoundingClientRect().left}; }")
    check("a later set_text_input(Some) only moves the textarea; it does not bring the keyboard back", not st["active"] and st["left"] == 80, st)
    page.mouse.click(200, 150)
    st = page.evaluate("document.activeElement.tagName")
    check("a tap on the canvas while text input is wanted refocuses the textarea (inside the gesture)", st == "TEXTAREA", st)
    page.evaluate("window.probe.probe_clear_text_input()")
    page.mouse.click(200, 150)
    check("a click on the canvas without text input focuses the canvas", page.evaluate("document.activeElement.id") == "canvas")
    # Clamping to the canvas.
    page.evaluate("window.probe.probe_set_text_input(-500, 99999, 2, 20)")
    r = page.evaluate("() => { const r = document.querySelector('textarea[data-drafftink=text-agent]').getBoundingClientRect(); return [r.left, r.top]; }")
    check("caret far outside the canvas is clamped inside it (left 50, top 340)", r == [50, 340], r)
    page.evaluate("window.probe.probe_clear_text_input()")


def test_key_policy(page):
    cases = [
        # (description, key, modifiers, expected prevented)
        ("Tab", "Tab", {}, True), ("Backspace", "Backspace", {}, True), ("ArrowDown", "ArrowDown", {}, True), ("Space", " ", {}, True),
        ("PageDown", "PageDown", {}, True), ("Delete", "Delete", {}, True),
        ("plain letter", "a", {}, False), ("Enter", "Enter", {}, False), ("Escape", "Escape", {}, False),
        ("F5 reload", "F5", {}, False), ("F11 fullscreen", "F11", {}, False), ("F12 devtools", "F12", {}, False),
        ("Ctrl+C", "c", {"ctrlKey": True}, False), ("Ctrl+V", "v", {"ctrlKey": True}, False), ("Ctrl+X", "x", {"ctrlKey": True}, False),
        ("Ctrl+R reload", "r", {"ctrlKey": True}, False), ("Ctrl+Shift+R", "R", {"ctrlKey": True, "shiftKey": True}, False),
        ("Ctrl+Shift+I devtools", "I", {"ctrlKey": True, "shiftKey": True}, False), ("Ctrl+Shift+C inspect", "C", {"ctrlKey": True, "shiftKey": True}, False),
        ("Ctrl+Shift+J console", "J", {"ctrlKey": True, "shiftKey": True}, False),
        ("Ctrl+T new tab", "t", {"ctrlKey": True}, False), ("Ctrl+W", "w", {"ctrlKey": True}, False), ("Ctrl+L", "l", {"ctrlKey": True}, False),
        ("Ctrl+1 tab switch", "1", {"ctrlKey": True}, False),
        ("Ctrl+S save page", "s", {"ctrlKey": True}, True), ("Ctrl+O open", "o", {"ctrlKey": True}, True), ("Ctrl+P print", "p", {"ctrlKey": True}, True),
        ("Ctrl+A select all", "a", {"ctrlKey": True}, True), ("Ctrl+D bookmark", "d", {"ctrlKey": True}, True), ("Ctrl+G", "g", {"ctrlKey": True}, True),
        ("Ctrl+Z", "z", {"ctrlKey": True}, True), ("Ctrl+, ", ",", {"ctrlKey": True}, True), ("Ctrl+= zoom", "=", {"ctrlKey": True}, True),
        ("Ctrl+- zoom", "-", {"ctrlKey": True}, True),
        ("AltGr+Q ('@' on German layouts)", "@", {"ctrlKey": True, "altKey": True}, False),
        ("Alt+ArrowLeft (browser back)", "ArrowLeft", {"altKey": True}, True),
    ]
    bad = []
    for name, key, mods, want in cases:
        got = page.evaluate(f"() => !document.getElementById('canvas').dispatchEvent({key_event(key, **mods)})")
        if got != want:
            bad.append((name, got, want))
    check(f"canvas keydown policy: {len(cases)} keys (F5/F11/F12/Ctrl+R/devtools/clipboard keys kept; Tab, arrows, Space, Ctrl+S/O/P/A/D/Z/zoom cancelled)", not bad, bad)
    wheel = page.evaluate("() => !document.getElementById('canvas').dispatchEvent(new WheelEvent('wheel', {ctrlKey: true, deltaY: 10, bubbles: true, cancelable: true}))")
    plain = page.evaluate("() => !document.getElementById('canvas').dispatchEvent(new WheelEvent('wheel', {deltaY: 10, bubbles: true, cancelable: true}))")
    check("canvas: ctrl+wheel (page zoom, trackpad pinch) and plain wheel are cancelled (wheel listener is non-passive)", wheel and plain, (wheel, plain))
    g = page.evaluate("() => !document.getElementById('canvas').dispatchEvent(new Event('gesturestart', {bubbles: true, cancelable: true}))")
    check("canvas: Safari gesturestart is cancelled", g)


def test_open_url(page):
    page.evaluate("window.__opened = []; window.open = (...a) => { window.__opened.push(a); return null; }; window.__xss = 0;")
    url_before = page.url
    page.evaluate("window.probe.probe_open_url('https://example.com/a?b=1', true)")
    page.evaluate("window.probe.probe_open_url('http://example.com/', true)")
    for bad in ["javascript:window.__xss=1", "  JaVaScRiPt:window.__xss=1", "data:text/html,<script>window.__xss=1</script>", "file:///etc/passwd",
                "vbscript:x", "blob:http://x/1", "/relative/path", "example.com", ""]:
        page.evaluate(f"window.probe.probe_open_url({json.dumps(bad)}, true)")
        page.evaluate(f"window.probe.probe_open_url({json.dumps(bad)}, false)")
    opened = page.evaluate("window.__opened")
    check("open_url: https and http open in a new tab with noopener,noreferrer",
          opened == [["https://example.com/a?b=1", "_blank", "noopener,noreferrer"], ["http://example.com/", "_blank", "noopener,noreferrer"]], opened)
    check("open_url: javascript:, data:, file:, vbscript:, blob:, relative and empty URLs are ignored (nothing opened, no navigation, no script)",
          len(opened) == 2 and page.url == url_before and page.evaluate("window.__xss") == 0, (opened, page.url))


def test_uninstall(page):
    page.evaluate("window.probe.probe_set_text_input(10, 10, 2, 20)")
    clear(page)
    page.evaluate("window.probe.probe_uninstall()")
    check("uninstall: the hidden textarea is removed", page.evaluate("document.querySelector('textarea[data-drafftink=text-agent]')") is None)
    page.evaluate(pointer_js("pointerdown", "pen", 150, 140, 1, 0.5))
    page.evaluate("""() => { const dt = new DataTransfer(); dt.setData('text/plain', 'x');
        document.getElementById('canvas').dispatchEvent(new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true}));
        document.dispatchEvent(new Event('visibilitychange')); window.dispatchEvent(new Event('blur'));
        document.getElementById('canvas').dispatchEvent(new DragEvent('dragenter', {bubbles: true, cancelable: true, dataTransfer: (() => { const d = new DataTransfer(); d.items.add(new File(['a'], 'a.txt')); return d; })()})); }""")
    tick(page, 100)
    check("uninstall: every listener is gone (pointer, paste, visibility, focus, drag produce no events)", events(page) == [], events(page))
    prevented = page.evaluate(f"() => !document.getElementById('canvas').dispatchEvent({key_event('Tab')})")
    check("uninstall: the key policy listener is gone too (Tab no longer prevented)", not prevented)


def test_error_page(page, requests):
    check("webgpu_available() agrees with 'gpu' in navigator", page.evaluate("window.probe.probe_webgpu_available()") == page.evaluate("'gpu' in navigator && navigator.gpu !== undefined"))
    page.evaluate("Object.defineProperty(navigator, 'gpu', {value: undefined, configurable: true})")
    check("webgpu_available() is false without navigator.gpu", page.evaluate("window.probe.probe_webgpu_available()") is False)
    page.evaluate("delete navigator.gpu")
    n = len(requests)
    page.evaluate("""window.probe.probe_show_error_page('WebGPU is not available', 'adapter request failed: <img src=x onerror="window.__xss=1"> & "quotes"\\nline 2')""")
    page.wait_for_timeout(300)
    info = page.evaluate(
        """() => ({title: document.title, h1: document.querySelector('main h1')?.textContent, role: document.querySelector('main')?.getAttribute('role'),
                  pre: document.querySelector('main pre')?.textContent, imgs: document.querySelectorAll('img').length, xss: window.__xss ?? 0,
                  canvas: document.getElementById('canvas') !== null, li: document.querySelectorAll('main li').length,
                  scripts: document.querySelectorAll('script, link, iframe').length, text: document.body.innerText})"""
    )
    check("error page: title, main landmark, details shown verbatim in <pre>, body replaced",
          info["h1"] == "WebGPU is not available" and info["role"] == "main" and info["title"].startswith("WebGPU is not available")
          and info["pre"] == 'adapter request failed: <img src=x onerror="window.__xss=1"> & "quotes"\nline 2' and not info["canvas"], info)
    check("error page: details cannot inject markup or run script (no <img>, no script, __xss unset)", info["imgs"] == 0 and info["xss"] == 0 and info["scripts"] == 0, info)
    check("error page: names the browsers that work (Chrome/Edge 113, Firefox 141, Safari 26)",
          all(s in info["text"] for s in ["Chrome or Edge 113", "Firefox 141", "Safari 26"]) and info["li"] == 4, info["text"])
    new_requests = [r for r in requests[n:] if not r.startswith(f"http://127.0.0.1:{PORT}/")]
    check("error page: self-contained, no request leaves the page", new_requests == [], new_requests)
    page.screenshot(path=SHOT)


def main():
    root = None
    if "--no-build" in ARG:
        root = os.environ.get("WEB_HOST_PROBE_DIR")
    if not root:
        root = build()
    srv = serve(root)
    logs = []
    requests = []
    ok = False
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            ctx = browser.new_context(viewport={"width": 800, "height": 600}, has_touch=True, permissions=["clipboard-read", "clipboard-write"])
            page = ctx.new_page()
            page.on("console", lambda m: logs.append(f"{m.type}: {m.text}"))
            page.on("pageerror", lambda e: logs.append(f"PAGEERROR {e}"))
            page.on("request", lambda r: requests.append(r.url))
            page.goto(f"http://127.0.0.1:{PORT}/probe.html")
            page.wait_for_function("window.__probe_ready === true || window.__probe_error", timeout=20000)
            err = page.evaluate("window.__probe_error || null")
            if err:
                check("probe starts", False, err)
            else:
                check("probe starts: host installed on <canvas id=canvas>", True)
                for name, fn in [
                    ("canvas setup", lambda: test_canvas_setup(page)),
                    ("mouse", lambda: test_mouse(page, ctx)),
                    ("pen/touch synthetic", lambda: test_pen_and_touch_synthetic(page)),
                    ("pen/touch cdp", lambda: test_pen_and_touch_cdp(page, ctx)),
                    ("visibility", lambda: test_visibility_focus(page)),
                    ("paste", lambda: test_paste(page)),
                    ("copy/cut", lambda: test_copy_cut(page)),
                    ("clipboard write/read", lambda: test_clipboard_write_read(page)),
                    ("clipboard unavailable", lambda: test_clipboard_unavailable(page, logs)),
                    ("files", lambda: test_files(page)),
                    ("text agent", lambda: test_text_agent(page, ctx, logs)),
                    ("key policy", lambda: test_key_policy(page)),
                    ("open_url", lambda: test_open_url(page)),
                    ("uninstall", lambda: test_uninstall(page)),
                    ("error page", lambda: test_error_page(page, requests)),
                ]:
                    try:
                        fn()
                    except Exception as e:  # a crashed section is a failure, the others still run
                        check(f"section '{name}' ran without a harness error", False, repr(e)[:400])
                errors = [m for m in logs if m.startswith("PAGEERROR") or m.startswith("error:")]
                check("no page errors or console errors during the whole run", not errors, errors[:5])
            browser.close()
    finally:
        srv.shutdown()
        srv.server_close()
    failed = [r for r in results if not r[1]]
    print(f"\n{len(results) - len(failed)} passed, {len(failed)} failed, {len(results)} checks")
    for name, _, detail in failed:
        print("FAILED:", name, "->", detail)
    print("RESULT:", "PASS" if not failed else "FAIL")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
