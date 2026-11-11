/** Small builders on top of the commands, so that specs say what they draw and not how. */
import { expect } from "@playwright/test";
import type { App, Pt } from "./app.ts";
import type { RefusalData } from "./api.ts";

/** Creates a rectangle (or another box kind) and returns its id. */
export async function box(app: App, x: number, y: number, w = 100, h = 60, extra: { kind?: "rectangle" | "ellipse" | "diamond"; name?: string; fill?: string } = {}): Promise<string> {
  const made = await app.exec("element.create", {
    kind: extra.kind ?? "rectangle",
    bounds: { x, y, w, h },
    ...(extra.name ? { name: extra.name } : {}),
    ...(extra.fill ? { style: { fill_color: extra.fill, fill_pattern: "solid" as const } } : {}),
  });
  return made.id;
}

/** Joins two elements with an arrow bound to both and returns the arrow's id. */
export async function connect(app: App, from: string, to: string, label?: string): Promise<string> {
  const made = await app.exec("element.connect", { from, to, ...(label ? { label } : {}) });
  return made.id as string;
}

/** Puts the camera at a known place: 100 % zoom, `center` in the middle of the canvas. */
export async function camera(app: App, center: Pt = { x: 0, y: 0 }, zoom = 1): Promise<void> {
  await app.exec("view.set", { scroll: [center.x, center.y], zoom });
}

/** The number of undo steps and the label of the next one. */
export async function undoDepth(app: App): Promise<{ depth: number; label: string | null }> {
  const h = await app.exec("history.state");
  return { depth: h.undo_depth as number, label: (h.undo_label as string | null) ?? null };
}

/** Distance between two points. */
export function dist(a: Pt, b: Pt): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

/** Checks a refusal's code and that each text appears in its message (the words the user and the agent read). */
export function expectRefusal(refusal: RefusalData, code: string, ...texts: string[]): void {
  expect(refusal.code, `refusal code (message: ${refusal.message})`).toBe(code);
  for (const t of texts) expect(refusal.message).toContain(t);
}
