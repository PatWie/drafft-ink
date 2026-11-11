/**
 * Fingers and a stylus through the DevTools protocol. Playwright's own `touchscreen` can only tap, and its mouse is
 * always a mouse; these send `pointerType` "touch" and "pen" events, the ones an iPad sends to Safari.
 */
import type { CDPSession, Page } from "@playwright/test";
import type { Pt } from "./app.ts";

type TouchType = "touchStart" | "touchMove" | "touchEnd" | "touchCancel";

export class Hands {
  private constructor(private readonly cdp: CDPSession) {}

  static async on(page: Page): Promise<Hands> {
    return new Hands(await page.context().newCDPSession(page));
  }

  /**
   * One touch event. `fingers` are all the fingers on the screen after the event, in the order they landed; a finger
   * keeps its place in the list (its id is the place + 1) until it lifts, and the last one goes with an empty list.
   */
  private touch(type: TouchType, fingers: Pt[]): Promise<unknown> {
    const touchPoints = fingers.map((p, i) => ({ x: p.x, y: p.y, id: i + 1, force: 0.5 }));
    return this.cdp.send("Input.dispatchTouchEvent", { type, touchPoints });
  }

  /** One finger from `from` to `to` in `steps` moves. */
  async drag(from: Pt, to: Pt, steps = 12): Promise<void> {
    await this.touch("touchStart", [from]);
    for (let i = 1; i <= steps; i++) {
      await this.touch("touchMove", [lerp(from, to, i / steps)]);
      await sleep(16);
    }
    await this.touch("touchEnd", []);
  }

  /** A finger that lands and lifts again. */
  async tap(at: Pt): Promise<void> {
    await this.touch("touchStart", [at]);
    await sleep(40);
    await this.touch("touchEnd", []);
  }

  /** A finger that stays down for `ms` and lifts. */
  async hold(at: Pt, ms: number): Promise<void> {
    await this.touch("touchStart", [at]);
    await sleep(ms);
    await this.touch("touchEnd", []);
  }

  /** Two fingers that move from `a` and `b` to `a2` and `b2` together. */
  async pinch(a: Pt, b: Pt, a2: Pt, b2: Pt, steps = 14): Promise<void> {
    await this.touch("touchStart", [a]);
    await this.touch("touchStart", [a, b]);
    for (let i = 1; i <= steps; i++) {
      const t = i / steps;
      await this.touch("touchMove", [lerp(a, a2, t), lerp(b, b2, t)]);
      await sleep(16);
    }
    await this.touch("touchEnd", [a2]);
    await this.touch("touchEnd", []);
  }

  /** The stylus moves in the air above the screen (hover). */
  async hover(at: Pt): Promise<void> {
    await this.cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: at.x, y: at.y, button: "none", buttons: 0, pointerType: "pen" });
  }

  /** The stylus draws along `points` (at least two) with the pressure of each. */
  async stroke(points: { at: Pt; pressure: number }[]): Promise<void> {
    const [first, ...rest] = points;
    if (!first || rest.length === 0) throw new Error("a stroke needs two points");
    const pen = (type: "mousePressed" | "mouseMoved" | "mouseReleased", at: Pt, pressure: number) =>
      this.cdp.send("Input.dispatchMouseEvent", {
        type,
        x: at.x,
        y: at.y,
        button: "left",
        buttons: type === "mouseReleased" ? 0 : 1,
        clickCount: type === "mouseMoved" ? 0 : 1,
        force: pressure,
        tiltX: 20,
        tiltY: -10,
        pointerType: "pen",
      });
    await pen("mousePressed", first.at, first.pressure);
    for (const p of rest) {
      await pen("mouseMoved", p.at, p.pressure);
      await sleep(8);
    }
    const last = rest[rest.length - 1] ?? first;
    await pen("mouseReleased", last.at, 0);
  }
}

function lerp(a: Pt, b: Pt, t: number): Pt {
  return { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
