#!/usr/bin/env python3
"""Writes the hand-authored Excalidraw inputs into ../excalidraw/ (deterministic, stdlib only).

Element field sets follow Excalidraw's own element base (packages/excalidraw/tests/fixtures/
elementFixture.ts, MIT) and the scene file envelope (type/version/source/elements/appState/files).
The content (which elements, positions, bindings) is authored for this repo. fixture_library.excalidrawlib
is copied unmodified from Excalidraw (see ../README.md).
"""
import json, os, shutil, sys

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "excalidraw")
EXCALIDRAW = os.environ.get("EXCALIDRAW_REPO", "/home/ubuntu/git/excalidraw")
_n = [0]


def base(**kw):
    _n[0] += 1
    e = dict(
        id="", type="", x=0, y=0, width=100, height=100, angle=0,
        strokeColor="#1e1e1e", backgroundColor="transparent", fillStyle="solid",
        strokeWidth=2, strokeStyle="solid", roughness=1, opacity=100,
        groupIds=[], frameId=None, index=None, roundness=None,
        seed=1000 + _n[0], version=1, versionNonce=2000 + _n[0], isDeleted=False,
        boundElements=None, updated=1, link=None, locked=False,
    )
    e.update(kw)
    return e


def rect(i, x, y, w, h, **kw): return base(id=i, type="rectangle", x=x, y=y, width=w, height=h, **kw)
def diamond(i, x, y, w, h, **kw): return base(id=i, type="diamond", x=x, y=y, width=w, height=h, **kw)
def ellipse(i, x, y, w, h, **kw): return base(id=i, type="ellipse", x=x, y=y, width=w, height=h, **kw)


def linear(t, i, x, y, pts, **kw):
    xs = [p[0] for p in pts]; ys = [p[1] for p in pts]
    d = dict(points=pts, width=max(xs) - min(xs), height=max(ys) - min(ys), lastCommittedPoint=None)
    if t == "arrow":
        d.update(startBinding=None, endBinding=None, startArrowhead=None, endArrowhead="arrow", elbowed=False)
    else:
        d.update(startBinding=None, endBinding=None, startArrowhead=None, endArrowhead=None)
    d.update(kw)
    return base(id=i, type=t, x=x, y=y, **d)


def text(i, x, y, s, **kw):
    lines = s.split("\n")
    d = dict(text=s, originalText=s, fontSize=20, fontFamily=5, textAlign="left", verticalAlign="top",
             containerId=None, lineHeight=1.25, autoResize=True,
             width=max(len(l) for l in lines) * 11, height=25 * len(lines))
    d.update(kw)
    return base(id=i, type="text", x=x, y=y, **d)


def scene(elements, files=None, source="https://excalidraw.com"):
    return {"type": "excalidraw", "version": 2, "source": source, "elements": elements,
            "appState": {"gridSize": None, "viewBackgroundColor": "#ffffff"}, "files": files or {}}


def write(name, obj):
    path = os.path.join(OUT, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(obj, f, indent=2, sort_keys=False)
        f.write("\n")


# --- scenes -----------------------------------------------------------------------------------
write("basic_shapes.excalidraw", scene([
    rect("r1", 0, 0, 160, 100, backgroundColor="#ffc9c9", strokeColor="#e03131", fillStyle="hachure", roundness={"type": 3}),
    rect("r2", 200, 0, 160, 100, backgroundColor="#a5d8ff", fillStyle="cross-hatch", roughness=0, strokeWidth=4),
    rect("r3", 400, 0, 160, 100, backgroundColor="#b2f2bb", fillStyle="zigzag", roughness=2, opacity=50),
    rect("r4", 600, 0, 160, 100, backgroundColor="#ffec99", fillStyle="solid", strokeStyle="dashed", angle=0.4),
    rect("r5", 800, 0, 160, 100, strokeStyle="dotted", strokeColor="#fff", backgroundColor="#00f8"),
    rect("r6", 1000, 0, 50, 20, roundness={"type": 3}, backgroundColor="#abc"),
    diamond("d1", 0, 160, 140, 120, backgroundColor="#d0bfff", fillStyle="hachure"),
    ellipse("e1", 200, 160, 160, 100, backgroundColor="#99e9f2", roughness=2),
    ellipse("e2", 400, 160, 100, 100, strokeColor="red"),
    linear("line", "l1", 600, 160, [[0, 0], [80, 80], [160, 0]]),
    linear("line", "l2", 800, 160, [[0, 0], [80, 80], [160, 0]], roundness={"type": 2}),
    linear("arrow", "a1", 0, 340, [[0, 0], [200, 60]], startArrowhead="dot", endArrowhead="triangle"),
    linear("arrow", "a2", 260, 340, [[0, 0], [100, 80], [200, 0]], roundness={"type": 2}),
    base(id="f1", type="freedraw", x=500, y=340, points=[[0, 0], [10, 8], [24, 12], [40, 6], [60, 20], [80, 0]],
         pressures=[0.2, 0.4, 0.6, 0.8, 1.0, 0.5], simulatePressure=False, width=80, height=20, lastCommittedPoint=None),
    text("t1", 0, 460, "Hello Excalidraw", fontSize=28, fontFamily=1),
    text("t2", 300, 460, "multi\nline text", fontSize=16, fontFamily=2, strokeColor="#2f9e44"),
    text("t3", 560, 460, "mono", fontSize=20, fontFamily=3),
    rect("gone", 0, 600, 100, 100, isDeleted=True),
    base(id="img-ignored", type="embeddable", x=0, y=720, width=200, height=100, link="https://example.com"),
]))

write("bindings.excalidraw", scene([
    rect("A", 0, 0, 160, 80, backgroundColor="#a5d8ff", boundElements=[{"id": "arr1", "type": "arrow"}, {"id": "arr2", "type": "arrow"}], roundness={"type": 3}),
    rect("B", 400, 0, 160, 80, backgroundColor="#b2f2bb", boundElements=[{"id": "arr1", "type": "arrow"}]),
    ellipse("C", 200, 220, 160, 100, backgroundColor="#ffec99", boundElements=[{"id": "arr2", "type": "arrow"}, {"id": "lbl", "type": "text"}]),
    linear("arrow", "arr1", 160, 40, [[0, 0], [240, 0]],
           startBinding={"elementId": "A", "focus": 0, "gap": 1, "fixedPoint": [1.0, 0.5]},
           endBinding={"elementId": "B", "focus": 0, "gap": 1, "fixedPoint": [0.0, 0.5]},
           endArrowhead="triangle"),
    linear("arrow", "arr2", 80, 80, [[0, 0], [0, 100], [120, 100]],
           startBinding={"elementId": "A", "focus": 0, "gap": 1, "fixedPoint": [0.5, 1.0]},
           endBinding={"elementId": "C", "focus": 0, "gap": 1, "fixedPoint": [0.0, 0.5]},
           startArrowhead="bar", endArrowhead="arrow"),
    text("lbl", 230, 255, "bound", containerId="C", textAlign="center", verticalAlign="middle"),
    linear("arrow", "dangling", 600, 40, [[0, 0], [100, 0]], endBinding={"elementId": "missing", "focus": 0, "gap": 1}),
]))

write("groups.excalidraw", scene([
    rect("g1", 0, 0, 80, 80, groupIds=["G_inner", "G_outer"], backgroundColor="#ffc9c9"),
    ellipse("g2", 100, 0, 80, 80, groupIds=["G_inner", "G_outer"], backgroundColor="#a5d8ff"),
    rect("g3", 200, 0, 80, 80, groupIds=["G_outer"], backgroundColor="#b2f2bb"),
    rect("s1", 0, 120, 80, 80, groupIds=["G_other"]),
    text("s2", 100, 140, "grouped label", groupIds=["G_other"]),
    rect("loose", 300, 120, 80, 80),
    rect("single", 0, 240, 80, 80, groupIds=["G_single"]),
]))

write("frames.excalidraw", scene([
    base(id="frame1", type="frame", x=0, y=0, width=420, height=260, name="Frame 1"),
    rect("in1", 20, 40, 120, 80, frameId="frame1", backgroundColor="#a5d8ff"),
    linear("arrow", "in2", 160, 80, [[0, 0], [100, 0]], frameId="frame1"),
    ellipse("in3", 280, 40, 100, 80, frameId="frame1"),
    base(id="mframe", type="magicframe", x=500, y=0, width=300, height=200, name="Magic"),
    rect("in4", 520, 40, 100, 60, frameId="mframe"),
    rect("outside", 0, 320, 100, 60),
]))

write("text_containers.excalidraw", scene([
    rect("box1", 0, 0, 200, 100, backgroundColor="#ffec99", boundElements=[{"id": "txt1", "type": "text"}], roundness={"type": 3}),
    text("txt1", 20, 35, "Centered in box", containerId="box1", textAlign="center", verticalAlign="middle", width=160, height=25),
    ellipse("box2", 260, 0, 200, 120, boundElements=[{"id": "txt2", "type": "text"}]),
    text("txt2", 300, 40, "Ellipse\nlabel", containerId="box2", textAlign="center", verticalAlign="middle", fontFamily=1),
    diamond("box3", 520, 0, 160, 160, boundElements=[{"id": "txt3", "type": "text"}]),
    text("txt3", 570, 65, "Diamond", containerId="box3", textAlign="center", verticalAlign="middle", fontFamily=6),
    linear("arrow", "arrowL", 0, 220, [[0, 0], [200, 0]], boundElements=[{"id": "txt4", "type": "text"}]),
    text("txt4", 60, 195, "arrow label", containerId="arrowL", textAlign="center", verticalAlign="middle", fontFamily=8),
    text("free", 0, 280, "free text with fontSize 36", fontSize=36),
    text("wrapped", 0, 340, "A text with a fixed width that wraps\nover two lines", autoResize=False, width=240),
    text("noattr", 0, 420, "no explicit font fields"),
]))

write("elbow_arrows.excalidraw", scene([
    rect("E1", 0, 0, 120, 80, boundElements=[{"id": "el1", "type": "arrow"}]),
    rect("E2", 400, 240, 120, 80, boundElements=[{"id": "el1", "type": "arrow"}]),
    linear("arrow", "el1", 120, 40, [[0, 0], [140, 0], [140, 240], [280, 240]], elbowed=True, roundness=None,
           fixedSegments=[{"index": 2, "start": [140, 0], "end": [140, 240]}],
           startBinding={"elementId": "E1", "focus": 0, "gap": 0, "fixedPoint": [1.0, 0.5]},
           endBinding={"elementId": "E2", "focus": 0, "gap": 0, "fixedPoint": [0.0, 0.5]},
           startIsSpecial=False, endIsSpecial=False),
    linear("arrow", "el2", 0, 400, [[0, 0], [100, 0], [100, 80]], elbowed=True, roundness=None),
    linear("arrow", "round", 300, 400, [[0, 0], [100, 0], [100, 80]], elbowed=False, roundness={"type": 2}),
]))

write("closed_paths.excalidraw", scene([
    linear("line", "tri", 0, 0, [[0, 0], [100, 0], [50, 80], [0, 0]], backgroundColor="#ffc9c9", fillStyle="solid"),
    linear("line", "nearclosed", 200, 0, [[0, 0], [100, 0], [100, 80], [4, 6]], backgroundColor="#a5d8ff"),
    linear("line", "open", 400, 0, [[0, 0], [100, 0], [100, 80]]),
    linear("line", "two", 600, 0, [[0, 0], [100, 40]]),
    linear("line", "onepoint", 800, 0, [[0, 0]]),
    base(id="fd-closed", type="freedraw", x=0, y=200, points=[[0, 0], [60, 5], [60, 60], [5, 60], [2, 3]], pressures=[], simulatePressure=True, width=60, height=60, lastCommittedPoint=None),
    base(id="fd-empty", type="freedraw", x=200, y=200, points=[], pressures=[], simulatePressure=True),
    linear("arrow", "one-point-arrow", 400, 200, [[0, 0]]),
]))

write("images_and_unsupported.excalidraw", scene([
    base(id="img1", type="image", x=0, y=0, width=120, height=90, fileId="file-1", status="saved", scale=[1, 1], crop=None),
    base(id="iframe1", type="iframe", x=200, y=0, width=200, height=100),
    base(id="embed1", type="embeddable", x=450, y=0, width=200, height=100),
    rect("after", 0, 150, 100, 60),
], files={"file-1": {"mimeType": "image/png", "id": "file-1", "created": 1,
                    "dataURL": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=="}}))

write("minimal_fields.excalidraw", {
    "type": "excalidraw", "version": 2,
    "elements": [
        {"type": "rectangle"},
        {"type": "ellipse", "x": 10, "y": 20},
        {"type": "text", "x": 5, "y": 5},
        {"type": "text", "x": 5, "y": 50, "text": "fontSize as float", "fontSize": 16.5, "opacity": 0.5},
        {"type": "rectangle", "x": 300, "y": 0, "width": 40, "height": 40, "opacity": 100, "strokeColor": "#12345678", "backgroundColor": "#f00"},
        {"type": "rectangle", "x": 400, "y": 0, "width": 40, "height": 40, "opacity": 1, "strokeColor": "#xyz", "backgroundColor": "rgb(1,2,3)"},
        {"type": "unknown-type", "x": 0, "y": 0},
        {"x": 1, "y": 1},
    ],
})

write("no_elements.excalidraw", {"type": "excalidraw", "version": 2, "appState": {}})
with open(os.path.join(OUT, "not_json.excalidraw"), "w") as f:
    f.write("this is not json\n")

# --- libraries --------------------------------------------------------------------------------
shutil.copyfile(os.path.join(EXCALIDRAW, "packages/excalidraw/tests/fixtures/fixture_library.excalidrawlib"),
                os.path.join(OUT, "fixture_library.excalidrawlib"))

write("grouped_library.excalidrawlib", {
    "type": "excalidrawlib", "version": 2, "source": "https://excalidraw.com",
    "libraryItems": [
        {"id": "item-1", "status": "published", "created": 1, "name": "CloudSearch", "elements": [
            text("c-t", 0, 60, "CloudSearch", groupIds=["OUTER"]),
            rect("c-r", 0, 0, 40, 40, groupIds=["ICON_A", "ICON", "OUTER"], backgroundColor="#a5d8ff"),
            ellipse("c-e", 5, 5, 10, 10, groupIds=["ICON_B", "ICON", "OUTER"]),
            linear("line", "c-l", 0, 0, [[0, 0], [10, 10]], groupIds=["ICON_B", "ICON", "OUTER"]),
        ]},
        {"id": "item-2", "status": "unpublished", "created": 2, "name": "", "elements": [
            rect("u-r", 0, 0, 80, 30, backgroundColor="#ffc9c9"),
        ]},
        {"id": "item-3", "status": "unpublished", "created": 3, "name": "Arrow pair", "elements": [
            linear("arrow", "p-a", 0, 0, [[0, 0], [60, 0]]),
            linear("arrow", "p-b", 0, 20, [[0, 0], [60, 0]], elbowed=True),
        ]},
        {"id": "item-4", "status": "unpublished", "created": 4, "name": "Only unsupported", "elements": [
            base(id="img", type="image", width=10, height=10),
        ]},
        {"id": "item-5", "status": "unpublished", "created": 5, "name": "No elements key"},
    ]})

write("v1_library.excalidrawlib", {"type": "excalidrawlib", "version": 1, "library": [
    [rect("v-a", 0, 0, 50, 30, backgroundColor="#e64980")],
    [ellipse("v-b", 0, 0, 40, 40), text("v-c", 5, 10, "v1")],
    [],
]})

write("not_a_library.excalidrawlib", {"type": "excalidraw", "version": 2, "elements": []})

# --- clipboard --------------------------------------------------------------------------------
clip = scene([rect("c1", 0, 0, 100, 60, backgroundColor="#ffec99"), ellipse("c2", 140, 0, 60, 60),
              linear("arrow", "c3", 0, 100, [[0, 0], [200, 0]]), text("c4", 0, 130, "pasted")])
clip["type"] = "excalidraw/clipboard"
del clip["appState"]
write("clipboard/basic.json", clip)
notclip = dict(clip); notclip["type"] = "excalidraw"
write("clipboard/not_clipboard.json", notclip)
empty = dict(clip); empty["elements"] = []
write("clipboard/empty.json", empty)
