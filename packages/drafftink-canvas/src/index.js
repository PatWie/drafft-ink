// drafftink-canvas: the canvas library for JavaScript. See README.md and docs/canvas-api.md.
//
// The wasm module (dist/) holds the scene, the hit testing, the gesture recognizer and the renderer. This file is
// the browser half: it listens for pointer, wheel and resize events, draws a frame when one is due and not before,
// and turns what the module returns into the objects the TypeScript types describe.

import init, { Stage as RawStage, showErrorPage, webgpuAvailable } from "../dist/drafftink_canvas_js.js";
import { attachInput, cssCursor } from "./input.js";

let loading;

/**
 * Loads and starts the wasm module. `Stage.create` does it for you; call this early to hide the download behind
 * other work. `source` is a URL, a `Response`, bytes or a compiled module; the default is next to this package.
 */
export function loadWasm(source) {
  loading ??= init(source === undefined ? undefined : { module_or_path: source });
  return loading;
}

/** Whether the browser offers WebGPU. An adapter can still be missing; `Stage.create` rejects then. */
export { webgpuAvailable };

// Options that only this file reads; the rest go to the wasm module.
const SCRIPT_OPTIONS = ["wasm", "autoResize", "interactive", "cursor", "errorPage"];
const EVENTS = new Set([
  "pointerdown", "click", "dblclick", "longpress", "contextmenu", "dragstart", "dragmove", "dragend",
  "pointerenter", "pointerleave", "transformstart", "transform", "transformend",
]);
const OWN_EVENTS = new Set(["selectionchange", "change", "intent"]);

/** A canvas that shows a scene of shapes. */
export class Stage {
  #raw;
  #canvas;
  #frame = 0;
  #timer = 0;
  #closed = false;
  #detach = [];
  #cursor;
  #handles = new Map();
  #next = 0;
  #own = { selectionchange: new Map(), change: new Map(), intent: new Map() };
  #seen = { selection: 0, revision: 0 };
  #size = [0, 0, 0];

  /**
   * Mounts a stage on a `<canvas>`. Give the canvas a CSS size (`style="width: 100%; height: 400px"`); the stage
   * follows it, and the device pixel ratio, unless `autoResize` is `false`.
   * Rejects when the browser has no WebGPU.
   */
  static async create(canvas, options = {}) {
    await loadWasm(options.wasm);
    if (!webgpuAvailable()) {
      const why = "This browser has no WebGPU (navigator.gpu is missing).";
      if (options.errorPage) showErrorPage("WebGPU is not available", why);
      throw new Error(`${why} Use a current Chrome, Edge, Safari or Firefox.`);
    }
    freezeIntrinsicSize(canvas);
    const wasmOptions = Object.fromEntries(Object.entries(options).filter(([k]) => !SCRIPT_OPTIONS.includes(k)));
    let raw;
    try {
      raw = await RawStage.create(canvas, wasmOptions);
    } catch (error) {
      if (options.errorPage) showErrorPage("Drafft.ink could not start", String(error?.message ?? error));
      throw error;
    }
    return new Stage(raw, canvas, options);
  }

  constructor(raw, canvas, options) {
    this.#raw = raw;
    this.#canvas = canvas;
    this.#cursor = options.cursor !== false;
    if (options.interactive !== false) this.#detach.push(attachInput(canvas, (event) => this.handleInput(event)));
    if (options.autoResize !== false) this.#watchSize();
    this.#seen.revision = raw.revision();
    this.#kick();
  }

  // ---- scene ------------------------------------------------------------------------------------------------

  /** Adds a shape on top of everything and returns its id. See `ShapeInit`. */
  add(shape) {
    const id = this.#raw.add(shape);
    this.#kick();
    return id;
  }

  /** Merge-patches a shape: objects merge, `null` removes a field. */
  update(id, patch) {
    this.#raw.update(id, patch);
    this.#kick();
  }

  /** Removes shapes for good, with the text bound to them. */
  remove(...ids) {
    this.#raw.remove(ids.flat());
    this.#kick();
  }

  /** Removes everything. */
  clear() {
    this.#raw.clear();
    this.#kick();
  }

  /**
   * Runs `fn(tx)` and applies everything it did as one change that commits once, or not at all when a step is
   * refused. Returns what each step produced (`tx.add` and `tx.group` return their id at once).
   */
  transact(fn) {
    const ops = [];
    const tx = {
      add: (shape) => {
        const id = shape.id ?? this.#raw.newId();
        ops.push({ op: "add", shape: { ...shape, id } });
        return id;
      },
      update: (id, patch) => ops.push({ op: "update", id, patch }),
      remove: (...ids) => ops.push({ op: "remove", ids: ids.flat() }),
      order: (ids, to) => ops.push({ op: "order", ids: [ids].flat(), to }),
      group: (ids, id) => {
        ops.push({ op: "group", ids, id });
        return id;
      },
      ungroup: (ids) => ops.push({ op: "ungroup", ids: [ids].flat() }),
      lock: (ids, locked = true) => ops.push({ op: "lock", ids: [ids].flat(), locked }),
      move: (ids, dx, dy) => ops.push({ op: "move", ids: [ids].flat(), dx, dy }),
    };
    fn(tx);
    const results = this.#raw.transact(ops);
    this.#kick();
    return results;
  }

  /** Stacking: `"front"`, `"back"`, `"forward"` or `"backward"`. */
  order(ids, to) {
    this.transact((tx) => tx.order(ids, to));
  }

  /** Groups shapes; returns the group id. */
  group(ids, id) {
    return this.transact((tx) => tx.group(ids, id))[0];
  }

  /** Dissolves the groups the shapes stand for. */
  ungroup(ids) {
    this.transact((tx) => tx.ungroup(ids));
  }

  /** One shape, or throws when there is none with this id. */
  get(id) {
    return this.#raw.get(id);
  }

  /** All shapes, back to front. */
  all() {
    return this.#raw.elements();
  }

  /** The ids a selector matches: `#id`, `.name`, `rectangle`, `group:g1`, `*`. */
  find(selector) {
    return this.#raw.find(selector);
  }

  /** Number of shapes. */
  get count() {
    return this.#raw.count();
  }

  // ---- documents --------------------------------------------------------------------------------------------

  /** The scene as a document object (`JSON.stringify(stage)` works). */
  toJSON() {
    return JSON.parse(this.#raw.exportJson(false));
  }

  /** The scene as native JSON text. */
  exportJSON(pretty = false) {
    return this.#raw.exportJson(pretty);
  }

  /** Replaces the scene with a document (object or text), or the bytes of a file; see the README for formats. */
  load(source) {
    const bytes =
      source instanceof Uint8Array ? source
        : typeof source === "string" ? new TextEncoder().encode(source)
          : new TextEncoder().encode(JSON.stringify(source));
    this.#raw.loadBytes(bytes);
    this.#kick();
  }

  /** The scene as SVG text. */
  toSVG(options = {}) {
    return this.#raw.toSVG(options);
  }

  /** The scene as PNG bytes. The scene is stored in the PNG, so `load` brings it back. */
  async toPNG(options = {}) {
    const bytes = await this.#raw.toPNG(options);
    this.#kick();
    return bytes;
  }

  // ---- camera -----------------------------------------------------------------------------------------------

  /** `{ scroll: [x, y], zoom }`: the world point at the centre of the canvas, and pixels per world unit. */
  get camera() {
    return this.#raw.camera();
  }

  set camera({ scroll, zoom }) {
    this.#raw.setCamera(scroll, zoom);
    this.#kick();
  }

  /** Moves the content by `dx`, `dy` pixels. */
  panBy(dx, dy) {
    this.#raw.panBy(dx, dy);
    this.#kick();
  }

  /** Zooms by `factor` keeping the world point under the screen point `(x, y)` in place. */
  zoomAt(x, y, factor) {
    this.#raw.zoomAt(x, y, factor);
    this.#kick();
  }

  /** Shows all shapes (or `ids`). */
  fitToContent({ padding = 40, maxZoom, ids } = {}) {
    if (ids) this.#raw.fitTo(ids, padding, maxZoom);
    else this.#raw.fitToContent(padding, maxZoom);
    this.#kick();
  }

  /** `[x, y]` on screen for a world point. */
  worldToScreen([x, y]) {
    return this.#raw.worldToScreen(x, y);
  }

  /** `[x, y]` in the world for a point on screen. */
  screenToWorld([x, y]) {
    return this.#raw.screenToWorld(x, y);
  }

  /** Sets the size in CSS pixels and the device pixel ratio by hand (`autoResize: false`). */
  resize(width, height, devicePixelRatio = window.devicePixelRatio) {
    this.#raw.resize(width, height, devicePixelRatio);
    this.#size = [width, height, devicePixelRatio];
    this.#kick();
  }

  // ---- queries ----------------------------------------------------------------------------------------------

  /** The topmost shape at a screen point: `{ id, part }` or `null`. */
  hit(x, y, { tolerancePx } = {}) {
    return this.#raw.hit(x, y, tolerancePx) ?? null;
  }

  /** Every shape at a screen point, topmost first. */
  hitAll(x, y) {
    return this.#raw.hitAll(x, y);
  }

  /** Ids of the shapes a world rectangle selects: `"contain"` (default) or `"intersect"`. */
  marquee({ x, y, w, h }, mode = "contain") {
    return this.#raw.marquee([x, y, w, h], mode);
  }

  /** Ids of the shapes whose bounds touch a world rectangle. */
  inRect({ x, y, w, h }) {
    return this.#raw.inRect([x, y, w, h]);
  }

  /** What a pointer at a screen point would act on: a transformer handle, a shape or nothing. */
  pick(x, y) {
    return this.#raw.pick(x, y);
  }

  // ---- selection and transformer ---------------------------------------------------------------------------

  /** The transformer wraps these shapes. Clicking and dragging on the canvas changes the selection too. */
  setSelection(ids) {
    this.#raw.setSelection(ids);
    this.#kick();
    this.#notify();
  }

  /** The selected ids. */
  get selection() {
    return this.#raw.selection();
  }

  /** The group whose members a click selects one by one, or `null`. */
  setEnteredGroup(group) {
    this.#raw.setEnteredGroup(group ?? undefined);
  }

  /** What a drag with the primary button does: `"select"` (default), `"hand"` (pan) or `"draw"`. */
  setPointerMode(mode) {
    this.#raw.setPointerMode(mode);
  }

  /** Outlines shapes as targets (for example while something is dragged over them). */
  setHighlights(ids) {
    this.#raw.setHighlights(ids);
    this.#kick();
  }

  /** What is drawn above the scene, as data. */
  overlay() {
    return this.#raw.overlay();
  }

  // ---- appearance -------------------------------------------------------------------------------------------

  setTheme(theme) {
    this.#raw.setTheme(theme);
    this.#kick();
  }

  setGrid(grid) {
    this.#raw.setGrid(grid);
    this.#kick();
  }

  setBackground(color) {
    this.#raw.setBackground(color);
    this.#kick();
  }

  // ---- events -----------------------------------------------------------------------------------------------

  /**
   * Listens for an event. `on("click", "#id" | ".name" | "rectangle" | "group:g1" | "stage" | "*", callback)`.
   * Events bubble from the shape through its groups to the stage; `event.stopPropagation()` ends the bubble. With
   * no selector the listener is on the stage, so it sees every event that bubbles up and the ones on empty space.
   * `"selectionchange"`, `"change"` and `"intent"` are the stage's own events. Returns a handle for `off`.
   */
  on(event, selector, callback) {
    if (typeof selector === "function") [selector, callback] = ["stage", selector];
    const handle = ++this.#next;
    if (OWN_EVENTS.has(event)) {
      this.#own[event].set(handle, callback);
      this.#handles.set(handle, { own: event });
      return handle;
    }
    if (!EVENTS.has(event)) throw new Error(`${JSON.stringify(event)} is not an event; use one of ${[...EVENTS, ...OWN_EVENTS].join(", ")}`);
    const raw = this.#raw.on(event, selector, (data) => {
      let stopped = false;
      data.stopPropagation = () => {
        stopped = true;
      };
      callback(data);
      return stopped;
    });
    this.#handles.set(handle, { raw });
    return handle;
  }

  /** Removes a listener. */
  off(handle) {
    const entry = this.#handles.get(handle);
    if (!entry) return;
    this.#handles.delete(handle);
    if (entry.own) this.#own[entry.own].delete(handle);
    else this.#raw.off(entry.raw);
  }

  /**
   * Feeds an input event of the canvas library (`{ Pointer }`, `{ Wheel }`, `"CancelAll"`). The DOM listeners call
   * this; call it yourself with `interactive: false`, or to replay recorded input.
   */
  handleInput(event) {
    const intents = this.#raw.handleInput(event);
    for (const intent of intents) {
      const hover = intent.Hover;
      if (hover && this.#cursor) this.#canvas.style.cursor = cssCursor(hover.cursor);
      for (const cb of this.#own.intent.values()) cb(intent);
    }
    this.#kick();
    this.#arm();
    this.#notify();
    return intents;
  }

  // ---- drawing and lifecycle -------------------------------------------------------------------------------

  /** Draws a frame now if one is due. A stage draws by itself; this is for tests and exports. */
  render() {
    return this.#raw.render();
  }

  /** Counters for tests and bug reports: the adapter, shapes, frames drawn, what the last frame did. */
  stats() {
    return this.#raw.stats();
  }

  /** Frames drawn since the stage started. Stays put while nothing changes. */
  get framesDrawn() {
    return this.#raw.framesDrawn();
  }

  /** The canvas element. */
  get canvas() {
    return this.#canvas;
  }

  /** Stops listening and drawing and releases the GPU. */
  destroy() {
    if (this.#closed) return;
    this.#closed = true;
    cancelAnimationFrame(this.#frame);
    clearTimeout(this.#timer);
    this.#detach.forEach((f) => f());
    this.#detach = [];
    this.#raw.destroy();
    this.#raw.free();
  }

  // ---- internals --------------------------------------------------------------------------------------------

  /** Asks for a frame, once, when one is due. Nothing is scheduled while the stage is idle. */
  #kick() {
    if (this.#closed || this.#frame !== 0 || !this.#raw.needsFrame()) return;
    this.#frame = requestAnimationFrame(() => {
      this.#frame = 0;
      if (this.#closed) return;
      try {
        this.#raw.render();
      } catch (error) {
        console.error("drafftink-canvas: drawing failed", error);
        return;
      }
      if (this.#raw.isLost()) {
        this.#recover();
        return;
      }
      this.#kick();
    });
  }

  async #recover() {
    try {
      await this.#raw.recover();
      this.#kick();
    } catch (error) {
      console.error("drafftink-canvas: the GPU device was lost and could not be restored", error);
    }
  }

  /** One timer, for the next long press; none when nothing is pending. */
  #arm() {
    clearTimeout(this.#timer);
    this.#timer = 0;
    const deadline = this.#raw.nextDeadline();
    if (deadline === undefined || this.#closed) return;
    this.#timer = setTimeout(() => {
      this.#timer = 0;
      const intents = this.#raw.tick(performance.now());
      for (const intent of intents) for (const cb of this.#own.intent.values()) cb(intent);
      this.#kick();
      this.#arm();
      this.#notify();
    }, Math.max(0, deadline - performance.now()));
  }

  /** Tells `selectionchange` and `change` listeners what the last input changed. */
  #notify() {
    const selection = this.#raw.selectionVersion();
    if (selection !== this.#seen.selection) {
      this.#seen.selection = selection;
      for (const cb of this.#own.selectionchange.values()) cb(this.#raw.selection());
    }
    const revision = this.#raw.revision();
    if (revision !== this.#seen.revision) {
      this.#seen.revision = revision;
      for (const cb of this.#own.change.values()) cb();
    }
  }

  #watchSize() {
    const sync = () => {
      const { clientWidth: w, clientHeight: h } = this.#canvas;
      const dpr = window.devicePixelRatio || 1;
      if (w === 0 || h === 0) return;
      if (w === this.#size[0] && h === this.#size[1] && dpr === this.#size[2]) return;
      this.resize(w, h, dpr);
    };
    const observer = new ResizeObserver(sync);
    observer.observe(this.#canvas);
    this.#detach.push(() => observer.disconnect());
    // The ratio changes when the page is zoomed or the window moves to another screen.
    let query;
    const onRatio = () => {
      sync();
      query?.removeEventListener("change", onRatio);
      query = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      query.addEventListener("change", onRatio);
    };
    onRatio();
    this.#detach.push(() => query?.removeEventListener("change", onRatio));
    this.#size = [this.#canvas.clientWidth, this.#canvas.clientHeight, window.devicePixelRatio || 1];
  }
}

/**
 * A canvas without a CSS size is as large as its pixel buffer, and the buffer follows the size: on a screen with
 * a ratio of 2 every resize would double the canvas. When the CSS says nothing, the current size is fixed.
 */
function freezeIntrinsicSize(canvas) {
  if (canvas.style.width || canvas.style.height) return;
  const style = getComputedStyle(canvas);
  if (style.width === `${canvas.width}px` && style.height === `${canvas.height}px`) {
    canvas.style.width = `${canvas.width}px`;
    canvas.style.height = `${canvas.height}px`;
  }
}

export { RawStage, showErrorPage };
