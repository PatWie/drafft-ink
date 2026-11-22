import type { Sockets } from "./sockets";
import { EffectKind, type Effects } from "./wasm";

/** Normal closure, and "try again later" for a used-up daily limit (the code of section 11.6 of the design). */
const CLOSE_NORMAL = 1000;
const CLOSE_TRY_AGAIN_LATER = 1013;

/** Runs `send` and ignores a socket that is already closing: the core hears about it from the close event. */
function attempt(work: () => void): void {
  try {
    work();
  } catch {
    // The peer went away between the core's decision and now.
  }
}

/**
 * Carries out the frames and closes of one call of the core, in order. Every close code is a legal one
 * (never 1005 or 1006, which `WebSocket.close` refuses to send).
 */
export function applyEffects(effects: Effects, sockets: Sockets): void {
  const code = effects.quotaHit ? CLOSE_TRY_AGAIN_LATER : CLOSE_NORMAL;
  // A frame for many sockets is made once.
  const frames = new Map<number, Uint8Array>();
  for (let i = 0; i < effects.count(); i++) {
    const conn = effects.conn(i);
    const ws = sockets.socket(conn);
    switch (effects.kind(i)) {
      case EffectKind.Send: {
        const id = effects.frameId(i);
        let frame = frames.get(id);
        if (frame === undefined) {
          frame = effects.frame(i);
          frames.set(id, frame);
        }
        const bytes = frame;
        if (ws !== undefined) attempt(() => ws.send(bytes));
        break;
      }
      case EffectKind.Text:
        if (ws !== undefined) {
          const text = effects.text(i);
          attempt(() => ws.send(text));
        }
        break;
      case EffectKind.Close:
        if (ws !== undefined) attempt(() => ws.close(code, ""));
        sockets.remove(conn);
        break;
      case undefined:
        break;
    }
  }
}
