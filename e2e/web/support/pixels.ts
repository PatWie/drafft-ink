/**
 * Pixel checks: a screenshot of the page, sampled and counted with a tolerance. There is no golden image:
 * GPU output differs a little between drivers (docs/testing.md, "Pixel probes").
 */
import { expect } from "@playwright/test";
import { PNG } from "pngjs";
import type { App } from "./app.ts";

/** An RGB colour, 0 to 255. */
export type Rgb = readonly [number, number, number];

/** A rectangle of the page in CSS pixels. */
export interface Region {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A decoded screenshot. Coordinates are CSS pixels of the page (the device pixel ratio is 1 here). */
export class Shot {
  constructor(
    private readonly png: PNG,
    readonly clip: Region,
  ) {}

  /** The colour at a page point. */
  at(x: number, y: number): Rgb {
    const ix = Math.round(x - this.clip.x);
    const iy = Math.round(y - this.clip.y);
    if (ix < 0 || iy < 0 || ix >= this.png.width || iy >= this.png.height) throw new Error(`(${x}, ${y}) is outside the screenshot ${JSON.stringify(this.clip)}`);
    const o = (iy * this.png.width + ix) * 4;
    const d = this.png.data;
    return [d[o] ?? 0, d[o + 1] ?? 0, d[o + 2] ?? 0];
  }

  /** How many pixels of `region` are within `tol` of `color` in every channel. */
  count(region: Region, color: Rgb, tol: number): number {
    let n = 0;
    for (let y = Math.ceil(region.y); y < region.y + region.h; y++) {
      for (let x = Math.ceil(region.x); x < region.x + region.w; x++) {
        if (close(this.at(x, y), color, tol)) n++;
      }
    }
    return n;
  }

  /** The share (0 to 1) of the pixels of `region` that are within `tol` of `color`. */
  share(region: Region, color: Rgb, tol: number): number {
    return this.count(region, color, tol) / (Math.floor(region.w) * Math.floor(region.h));
  }

  /** How many pixels differ from `other` by more than `tol` in some channel (same clip required). */
  differing(other: Shot, tol: number): number {
    expect(other.png.width).toBe(this.png.width);
    expect(other.png.height).toBe(this.png.height);
    let n = 0;
    const a = this.png.data;
    const b = other.png.data;
    for (let i = 0; i < a.length; i += 4) {
      if (Math.abs((a[i] ?? 0) - (b[i] ?? 0)) > tol || Math.abs((a[i + 1] ?? 0) - (b[i + 1] ?? 0)) > tol || Math.abs((a[i + 2] ?? 0) - (b[i + 2] ?? 0)) > tol) n++;
    }
    return n;
  }
}

/** True when every channel of `a` is within `tol` of `b`. */
export function close(a: Rgb, b: Rgb, tol: number): boolean {
  return Math.abs(a[0] - b[0]) <= tol && Math.abs(a[1] - b[1]) <= tol && Math.abs(a[2] - b[2]) <= tol;
}

/** Parses `#rrggbb`. */
export function hex(color: string): Rgb {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color);
  if (!m) throw new Error(`not a #rrggbb colour: ${color}`);
  return [parseInt(m[1] ?? "0", 16), parseInt(m[2] ?? "0", 16), parseInt(m[3] ?? "0", 16)];
}

/** Waits until the app is idle (the last frame is on screen), then takes a screenshot of the page. */
export async function shot(app: App, region?: Region): Promise<Shot> {
  await app.settle();
  const buffer = await app.page.screenshot({ clip: region ? { x: region.x, y: region.y, width: region.w, height: region.h } : undefined, animations: "disabled" });
  const png = PNG.sync.read(buffer);
  return new Shot(png, region ?? { x: 0, y: 0, w: png.width, h: png.height });
}
