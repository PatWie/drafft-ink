# drafftink-canvas

An infinite-canvas library for the browser. Shapes, a spatial index with exact hit testing, Konva-style events with bubbling, a transformer, and export to JSON, SVG and PNG. Rust compiled to WebAssembly, drawn with Vello on WebGPU. It draws only when something changed: an idle canvas costs no animation frames.

It is the reusable tier of [Drafft.ink](https://drafft.ink). It has no editor, no undo and no collaboration; those are in the app. The design and the full API are in `docs/canvas-api.md` of the repository.

Needs a browser with WebGPU (Chrome or Edge 113+, Safari 26+, Firefox 141+ on Windows). `Stage.create` rejects with a plain message otherwise, or shows an explanation page with `errorPage: true`.

## Quick start

```html
<canvas id="c" style="width: 100%; height: 400px"></canvas>
<script type="module">
  import { Stage } from "drafftink-canvas";

  const stage = await Stage.create(document.getElementById("c"), {
    defaultStyle: { sloppiness: "architect" }, // precise lines; the default is hand-drawn
  });

  stage.add({ id: "box", type: "rectangle", x: 0, y: 0, w: 160, h: 90, name: "card", style: { fill_color: "#a5d8ff" } });
  stage.add({ type: "text", x: 20, y: 30, content: "Hello" });
  stage.add({ type: "arrow", x: 160, y: 45, points: [[0, 0], [120, 0]] });

  stage.on("click", ".card", (e) => console.log("clicked", e.target, e.world));
  stage.fitToContent({ padding: 60 });
</script>
```

Click a shape to select it, drag it, drag a handle to resize or rotate, drag on empty space for a rubber band, scroll to pan, Ctrl+scroll or pinch to zoom. Turn any of that off with `interaction` options.

Serve the page over HTTP (`npm run serve`, then open `examples/minimal.html` and `examples/index.html`). The wasm file is found next to the package; pass `wasm: url` to load it from elsewhere.

## Shapes

A shape is the scene's element, with the field names of the document file. The flat shorthand puts `x`, `y`, `w`, `h`, `angle` into `transform` and `type` plus the kind's fields into `kind`:

| type | fields |
|---|---|
| `rectangle`, `diamond`, `ellipse` | box |
| `line`, `arrow` | `points` (relative to `x`, `y`), `path: "sharp" \| "round" \| "elbow"`, `start_head`, `end_head`, `closed` |
| `freehand` | `points`, `pressures` |
| `text` | `content`, `font: { family, size, weight, align }`; the box follows the text |
| `math` | `latex`, `font_size` |
| `frame` | `name`, `clip` |

`style` has `stroke_color`, `stroke_width`, `stroke_style`, `fill_color`, `fill_pattern`, `sloppiness`, `opacity`, `corner_radius`. Colours are `#rrggbb` or `#rrggbbaa`. `name` is a list of words like CSS classes. `custom` holds your data.

```js
stage.update("box", { w: 200, style: { fill_color: "#ffd8a8" }, name: null }); // merge patch: null removes
stage.transact((tx) => { const a = tx.add({ ... }); tx.group([a, "box"]); });   // one commit, or none
stage.find(".card");   // ids; also "#id", "rectangle", "group:g1", "*"
stage.hit(x, y);       // { id, part } at a screen point; also hitAll, marquee, inRect
```

## Events

`stage.on(event, selector, callback)`; events bubble from the shape through its groups to the stage, and `e.stopPropagation()` ends the bubble. Events: `pointerdown click dblclick longpress contextmenu dragstart dragmove dragend pointerenter pointerleave transformstart transform transformend`, and `selectionchange`, `change`, `intent`. A listener may change the stage (`stage.update(...)` inside a click handler works); the change is applied when the event has been delivered, and reads wait until then.

## Export

```js
const doc = stage.toJSON();                  // load it again with stage.load(doc)
const svg = stage.toSVG({ look: "clean" });
const png = await stage.toPNG({ scale: 2 }); // Uint8Array; the scene is inside, stage.load(png) opens it
```

## Build

Needs Rust with the `wasm32-unknown-unknown` target, `wasm-bindgen-cli` 0.2.130 and binaryen (`wasm-opt`).

```
npm run build   # scripts/build.sh: cargo build --profile release-wasm, wasm-bindgen --target web, wasm-opt -Os
npm run sizes   # raw, gzip and brotli size of dist/drafftink_canvas_js_bg.wasm
```

`npm test` runs `test/smoke.py`: headed Chromium on Xvfb with WebGPU (headless Chromium draws nothing for WebGPU), with the python venv of `research/web-perf.md` (`~/.venvs/perf/bin/python`). It mounts a stage, adds 1000 shapes, clicks one, drags, pans, checks 0 frames in 5 s idle, exports and loads, and saves `test/smoke.png`.

Size of the wasm file: 7.4 MB, 2.3 MB with brotli. About 1 MB of that, compressed, is the bundled fonts (GelPen, Noto Sans, the STIX math font).

## Licence

AGPL-3.0-only.
