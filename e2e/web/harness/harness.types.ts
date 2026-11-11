/** Types shared by the page harness and the specs. */

/** The canvas element as the DOM sees it. */
export interface CanvasInfo {
  left: number;
  top: number;
  cssWidth: number;
  cssHeight: number;
  bufferWidth: number;
  bufferHeight: number;
  devicePixelRatio: number;
}

/** A file to drop: its bytes are base64 because they cross from Node into the page. */
export interface DropFile {
  name: string;
  mime: string;
  base64: string;
}

/** The methods of `window.__drafftinkTest`, as the specs see them. Keep in step with `harness.page.ts`. */
export interface DrafftinkTest {
  canvasInfo(): CanvasInfo;
  dropFiles(files: DropFile[], x: number, y: number): void;
  pasteText(text: string): void;
}
