/** Real pointer and key input through Playwright (Chrome DevTools input events), not commands. */
import type { Page } from "@playwright/test";
import type { Pt } from "./app.ts";

/** Presses at `from`, moves in `steps` small moves to `to`, releases. */
export async function drag(page: Page, from: Pt, to: Pt, steps = 14): Promise<void> {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  for (let i = 1; i <= steps; i++) {
    await page.mouse.move(from.x + ((to.x - from.x) * i) / steps, from.y + ((to.y - from.y) * i) / steps);
    await page.waitForTimeout(16);
  }
  await page.mouse.up();
}
