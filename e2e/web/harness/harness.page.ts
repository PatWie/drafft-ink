/**
 * The page side of the test harness: `window.__drafftinkTest`.
 *
 * Specs drive the app through `window.drafftink.execute` and real pointer input. This object holds only
 * what neither can do. Every method has a `Reason:` line that says what a command cannot do here. The
 * accounting check (`scripts/accounting.ts`, run by `npm run accounting`) fails when a method here has no
 * `Reason:`, when no spec calls it, or when a spec calls a name that is not declared here.
 *
 * `installHarness` runs inside the page (Playwright serialises the function), so it must not use anything
 * from this file or from imports. It is installed by `support/app.ts` with `page.addInitScript`. It is not
 * part of the app build: a production page has no such object.
 *
 * Add a method only when a command and a mouse or key event cannot do the job.
 */
import type { CanvasInfo, DrafftinkTest, DropFile } from "./harness.types.ts";

export function installHarness(): void {
  const canvas = (): HTMLCanvasElement => {
    const c = document.getElementById("canvas");
    if (!(c instanceof HTMLCanvasElement)) throw new Error("the page has no #canvas");
    return c;
  };
  const bytes = (base64: string): Uint8Array => Uint8Array.from(atob(base64), (ch) => ch.charCodeAt(0));

  const harness: DrafftinkTest = {
    /**
     * Where the canvas is on the page and how big its drawing buffer is.
     *
     * Reason: the element's position, CSS size and backing-store size live in the DOM. No command reads the
     * DOM, and a mouse event needs page coordinates, so specs turn a world point (from `view.get`) into
     * a screen point with this rectangle, and check that the buffer matches the device pixel ratio.
     */
    canvasInfo(): CanvasInfo {
      const c = canvas();
      const r = c.getBoundingClientRect();
      return {
        left: r.left,
        top: r.top,
        cssWidth: r.width,
        cssHeight: r.height,
        bufferWidth: c.width,
        bufferHeight: c.height,
        devicePixelRatio: window.devicePixelRatio,
      };
    },

    /**
     * Drops files on the canvas as the browser does when a user drags them from the desktop.
     *
     * Reason: a real drag from the file manager cannot be scripted in a browser, and `doc.drop` is the
     * command behind the drop handler, not the handler. The `DataTransfer` and its `File` objects can only
     * be made inside the page, so this builds them and dispatches `dragenter`, `dragover` and `drop` at a
     * point of the canvas. That exercises the shell's listeners, the file reading and the route into the command.
     */
    dropFiles(files: DropFile[], x: number, y: number): void {
      const transfer = new DataTransfer();
      for (const f of files) transfer.items.add(new File([bytes(f.base64) as BlobPart], f.name, { type: f.mime }));
      const target = canvas();
      for (const type of ["dragenter", "dragover", "drop"]) {
        target.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, dataTransfer: transfer }));
      }
    },

    /**
     * Pastes text as the browser does after Ctrl+V.
     *
     * Reason: the system clipboard belongs to the operating system. Chromium asks the user before a page
     * may read it, and a script cannot answer that prompt. The `paste` event carries the data to the page,
     * so this dispatches one on the document with a `DataTransfer`. That exercises the shell's paste
     * listener and the routing of `clipboard.paste` with source `system`, which no command call reaches.
     */
    pasteText(text: string): void {
      const transfer = new DataTransfer();
      transfer.setData("text/plain", text);
      document.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: transfer }));
    },
  };

  (window as unknown as { __drafftinkTest: DrafftinkTest }).__drafftinkTest = harness;
}
