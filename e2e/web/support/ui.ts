/**
 * Where the app's own widgets (egui chrome drawn on the canvas) are on the page, for the few specs that must click
 * them with the real mouse. The app publishes no widget rectangles, so each position is derived here from the layout
 * of `crates/ui` at the suite's window (1280 x 800, desktop layout, zoom 1), in one place and with its derivation.
 * `crates/ui/tests/top_strip.rs` pins the strip and menu numbers; when a surface changes, change the constants here.
 * A click on a stale position is not silent: each helper checks what the click did (the tool, the open menu).
 */
import { expect } from "@playwright/test";
import type { App, Pt } from "./app.ts";
import { Shot, shot, type Region } from "./pixels.ts";

/** The window of the suite (`playwright.config.ts`). The positions below hold for exactly this height. */
const WINDOW_HEIGHT = 800;

/** Buttons of the top strip, which is 72 points high (two rows of 28, a gap, and 6 above and below). */
export const STRIP = {
  /** The hamburger button: first of the first row, `MARGIN` (12) from the left, 28 square, so x 12..40, y 6..34. */
  menuButton: { x: 26, y: 20 },
  /** The collaboration (rooms) button: right of the menu button, x 44..72, y 6..34. */
  collabButton: { x: 58, y: 20 },
} as const satisfies Record<string, Pt>;

/** The tools of the column, top to bottom: `TOOLS` in `crates/ui/src/surfaces/toolbar.rs`, by `Tool::name`. */
const TOOL_COLUMN = [
  "select", "lasso", "hand", "rectangle", "diamond", "ellipse", "arrow", "line", "freehand", "highlighter", "text", "math", "frame", "eraser", "laser",
] as const;
export type ColumnTool = (typeof TOOL_COLUMN)[number];

/**
 * The centre of a tool button. The column is a panel anchored left and centred on the window, and with its 15 tools,
 * the sticky note, the lock and the library button it is taller than the room under the strip, so it is clamped to
 * `2 * (800 / 2 - 72 - 8) = 640` points (`ChromeRects::centred_panel_height`) and scrolls: it spans y 80..720.
 * The buttons start 8 points inside it (`PANEL_INSET_Y`), are 34 square (`Control::Tool`) and 3 apart (`space::XS`);
 * the panel is `MARGIN` (12) from the left edge and its buttons are inset by 11, so the centres are at x 12 + 11 + 17 = 40.
 */
export function toolButton(tool: ColumnTool): Pt {
  const column = { top: 72 + 8, inset: 8, side: 34, gap: 3 };
  return { x: 40, y: column.top + column.inset + column.side / 2 + TOOL_COLUMN.indexOf(tool) * (column.side + column.gap) };
}

/** The first row of the main menu, "New tab" (under the "File" heading): x 23..255, y 110..138. */
export const MENU_NEW_TAB: Pt = { x: 50, y: 124 };

/** "Export as PNG", the first row under the "Export" heading: 28 points high, centred at y 354, `top_strip.rs`. */
export const MENU_EXPORT_AS_PNG: Pt = { x: 50, y: 354 };

/** The area the open main menu covers, under the menu button: its rows are at least this wide and 400 points tall. */
const MENU_PANEL: Region = { x: 23, y: 110, w: 232, h: 400 };

function expectWindow(app: App): void {
  expect(app.page.viewportSize()?.height, "the positions of the widgets are for a window 800 points high").toBe(WINDOW_HEIGHT);
}

/**
 * One real click on the app's own UI, and a wait until the app has drawn the frame that handles it. The pointer goes
 * to the point and presses and releases once: nothing else is sent, so a held-back hover cannot be what the click
 * rides on. Then it waits on the frame counter (`window.drafftink.frames()`), then until the app is quiet.
 */
export async function clickUi(app: App, at: Pt): Promise<void> {
  const before = (await app.frames()).frames;
  await app.page.mouse.click(at.x, at.y);
  await app.until(`a frame that handles the click at (${at.x}, ${at.y})`, async () => (await app.frames()).frames > before);
  await app.settle();
}

/** Moves the pointer over the canvas and waits until the app has drawn what the hover changes. */
export async function hoverCanvas(app: App): Promise<void> {
  await app.page.mouse.move(900, 500);
  await app.settle();
  await app.page.mouse.move(920, 520);
  await app.settle();
}

/** Clicks the button of `tool` in the tool column and waits until the engine reports the tool; names the tool it got when it is another. */
export async function clickTool(app: App, tool: ColumnTool): Promise<void> {
  expectWindow(app);
  const at = toolButton(tool);
  await clickUi(app, at);
  const got = (await app.state()).tool;
  expect(got, `the click at (${at.x}, ${at.y}) on the ${tool} button set the tool ${String(got)}: has the tool column moved (support/ui.ts)?`).toBe(tool);
}

/**
 * Clicks the menu button and checks that the menu is open: the picture under the button changes where the menu panel is
 * drawn (the open state is UI state, which no command reports). Fails with the position when it did not open.
 */
export async function openMainMenu(app: App): Promise<void> {
  expectWindow(app);
  const closed: Shot = await shot(app, MENU_PANEL);
  await clickUi(app, STRIP.menuButton);
  const open: Shot = await shot(app, MENU_PANEL);
  // The open menu changes about 9% of the panel's area (text, icons, its edge and shadow); a click on another widget changes under 1%.
  const changed = open.differing(closed, 24);
  expect(changed, `the click at (${STRIP.menuButton.x}, ${STRIP.menuButton.y}) opened no menu: has the menu button moved (support/ui.ts)?`).toBeGreaterThan((MENU_PANEL.w * MENU_PANEL.h) / 20);
}
