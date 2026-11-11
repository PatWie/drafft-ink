/** Helpers for the X11 session the headed browser runs in. */
import { spawn } from "node:child_process";

/**
 * Puts `text` in the X11 primary selection (what a middle click pastes on Linux). Returns false when `xclip` is not
 * installed or there is no display, so the caller can still run its test without the seed.
 */
export function seedPrimarySelection(text: string): Promise<boolean> {
  return new Promise((resolve) => {
    if (process.platform !== "linux" || !process.env.DISPLAY) return resolve(false);
    const child = spawn("xclip", ["-selection", "primary", "-in"], { detached: true, stdio: ["pipe", "ignore", "ignore"] });
    child.once("error", () => resolve(false));
    child.once("spawn", () => {
      child.stdin.end(text, () => {
        // xclip stays in the background as the selection owner; do not wait for it.
        child.unref();
        resolve(true);
      });
    });
  });
}
