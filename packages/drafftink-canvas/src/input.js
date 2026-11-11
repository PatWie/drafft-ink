// DOM events of a canvas, turned into the input events of the canvas library (docs/canvas-api.md §3).
//
// Positions are CSS pixels relative to the canvas, time is `event.timeStamp` (the clock of performance.now()), so a
// recorded stream replays to the same intents.

const KIND = { mouse: "Mouse", pen: "Pen", touch: "Touch" };
// DOM `button`: 0 main, 1 auxiliary (wheel), 2 secondary, 3 and 4 browser back and forward.
const BUTTON = ["Primary", "Middle", "Secondary", "Other", "Other"];
const WHEEL_UNIT = ["Pixel", "Line", "Page"];
const CURSOR = {
  Default: "default",
  Move: "move",
  Grab: "grab",
  Grabbing: "grabbing",
  Crosshair: "crosshair",
  ResizeNs: "ns-resize",
  ResizeEw: "ew-resize",
  ResizeNesw: "nesw-resize",
  ResizeNwse: "nwse-resize",
  Rotate: "alias",
};

const modifiers = (e) => ({ shift: e.shiftKey, ctrl: e.ctrlKey, alt: e.altKey, meta: e.metaKey });

/** The input event for a DOM pointer event. */
export function pointerInput(e, phase, canvas) {
  const rect = canvas.getBoundingClientRect();
  const changed = phase === "Down" || phase === "Up" ? (BUTTON[e.button] ?? "Other") : "None";
  return {
    Pointer: {
      id: e.pointerId,
      kind: KIND[e.pointerType] ?? "Mouse",
      phase,
      pos: { x: e.clientX - rect.left, y: e.clientY - rect.top },
      button: changed,
      buttons: e.buttons,
      modifiers: modifiers(e),
      pressure: e.pressure,
      tilt: { x: e.tiltX ?? 0, y: e.tiltY ?? 0 },
      time_ms: e.timeStamp,
    },
  };
}

/** The input event for a DOM wheel event. A trackpad pinch arrives with `ctrlKey` set. */
export function wheelInput(e, canvas) {
  const rect = canvas.getBoundingClientRect();
  return {
    Wheel: {
      pos: { x: e.clientX - rect.left, y: e.clientY - rect.top },
      delta: { x: e.deltaX, y: e.deltaY },
      unit: WHEEL_UNIT[e.deltaMode] ?? "Pixel",
      modifiers: modifiers(e),
      time_ms: e.timeStamp,
    },
  };
}

/** The CSS cursor for a cursor of the canvas library. */
export const cssCursor = (cursor) => CURSOR[cursor] ?? "default";

/**
 * Listens on the canvas and calls `send(inputEvent)` for each. Returns a function that removes the listeners.
 * The canvas gets `touch-action: none` (the page must not scroll under a finger or pen) and a tab stop.
 */
export function attachInput(canvas, send) {
  canvas.style.touchAction = "none";
  canvas.style.userSelect = "none";
  canvas.style.webkitUserSelect = "none";
  canvas.style.outline = "none";
  if (!canvas.hasAttribute("tabindex")) canvas.tabIndex = 0;

  const on = (type, handler, options) => {
    canvas.addEventListener(type, handler, options);
    return () => canvas.removeEventListener(type, handler, options);
  };
  const off = [
    on("pointerdown", (e) => {
      canvas.focus({ preventScroll: true });
      // The drag keeps reporting when the pointer leaves the canvas.
      try {
        canvas.setPointerCapture(e.pointerId);
      } catch {
        /* a pointer that is already gone */
      }
      send(pointerInput(e, "Down", canvas));
    }),
    on("pointermove", (e) => send(pointerInput(e, "Move", canvas))),
    on("pointerup", (e) => send(pointerInput(e, "Up", canvas))),
    on("pointercancel", (e) => send(pointerInput(e, "Cancel", canvas))),
    on("pointerleave", (e) => {
      // With a button down the pointer is captured and the drag goes on outside.
      if (e.buttons === 0) send({ PointerLeave: { id: e.pointerId } });
    }),
    // Not passive: a wheel over the canvas must not scroll the page, and ctrl+wheel must not zoom it.
    on("wheel", (e) => {
      e.preventDefault();
      send(wheelInput(e, canvas));
    }, { passive: false }),
    on("contextmenu", (e) => e.preventDefault()),
    on("keydown", (e) => {
      if (e.key === "Escape") send("CancelAll");
    }),
    on("blur", () => send("CancelAll")),
  ];
  return () => off.forEach((remove) => remove());
}
