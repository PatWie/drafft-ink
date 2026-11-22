import { env, exports } from "cloudflare:workers";

import fixtureFile from "./fixtures/frames.json";

/** The frames and keys that `tests/fixtures.rs` makes. */
export const fixtures = fixtureFile;

export function unhex(text: string): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(2 * i, 2 * i + 2), 16);
  return out;
}

export function hex(bytes: Uint8Array): string {
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export type FrameName = keyof typeof fixtures.frames;

export function frame(name: FrameName): Uint8Array<ArrayBuffer> {
  return unhex(fixtures.frames[name]);
}

export const ROOM = fixtures.room.id;
export const MEMBER = `Drafft-Member ${fixtures.room.token.base64urlNoPad}`;
export const STRANGER = `Drafft-Member ${fixtures.stranger.token.base64urlNoPad}`;

const ORIGIN = "https://drafft.test";

/** A request to the Worker, as a client would send it. `origin` is the scheme and host the request is addressed to. */
export function request(path: string, init: RequestInit = {}, origin: string = ORIGIN): Promise<Response> {
  return exports.default.fetch(new Request(`${origin}${path}`, init));
}

export function roomStub(room: string = ROOM): DurableObjectStub {
  return env.ROOM.get(env.ROOM.idFromName(room));
}

// ---- decoding what the relay sends ----------------------------------------------------------------------------

export const TAG = {
  joined: 0x81,
  catchup: 0x82,
  record: 0x83,
  ack: 0x84,
  peerJoined: 0x85,
  peerLeft: 0x86,
  presenceBundle: 0x87,
  error: 0x88,
  snapshotResult: 0x89,
  compactAdvice: 0x8a,
  direct: 0x8b,
  retention: 0x8c,
} as const;

export const ERROR = { unauthorized: 3, roomMissing: 15, unavailable: 13, dailyLimit: 18, malformed: 1, notJoined: 10 } as const;

function view(frame: Uint8Array): DataView {
  return new DataView(frame.buffer, frame.byteOffset, frame.byteLength);
}

export function tagOf(frame: Uint8Array): number {
  return frame[0] ?? 0;
}

/** The code of an `Error` frame. */
export function errorCode(frame: Uint8Array): number {
  return frame[1] ?? 0;
}

/** `Ack`: the counter of the record and the sequence number it got. */
export function ack(frame: Uint8Array): { ctr: bigint; seq: bigint } {
  return { ctr: view(frame).getBigUint64(1, true), seq: view(frame).getBigUint64(9, true) };
}

/** `Joined`: the numbers at its start. */
export function joined(frame: Uint8Array): { roomState: number; headSeq: bigint; snapshotSeq: bigint; expiresAtMs: bigint } {
  const v = view(frame);
  return { roomState: frame[3] ?? 0, headSeq: v.getBigUint64(4, true), snapshotSeq: v.getBigUint64(12, true), expiresAtMs: v.getBigUint64(28, true) };
}

/** `Record`: the sequence number. */
export function recordSeq(frame: Uint8Array): bigint {
  return view(frame).getBigUint64(1, true);
}

/** `Error`: the milliseconds to wait, when the frame carries them (the last five bytes: a flag and a u32). */
export function retryAfterMs(frame: Uint8Array): number | null {
  if (frame.byteLength < 6 || frame[frame.byteLength - 5] !== 1) return null;
  return view(frame).getUint32(frame.byteLength - 4, true);
}

/** `Retention`: the deadline and whether the room is past it. */
export function retention(frame: Uint8Array): { expiresAtMs: bigint; expiring: boolean } {
  return { expiresAtMs: view(frame).getBigUint64(1, true), expiring: frame[9] === 1 };
}

/** `Catchup`: how many records and the head, read from the tail of the frame. */
export function snapshotResult(frame: Uint8Array): { ok: boolean; coversSeq: bigint } {
  return { ok: frame[1] === 1, coversSeq: view(frame).getBigUint64(2, true) };
}

/** `Joined.limits`, read from the end of the frame: what the environment announces to a client. */
export function limitsOf(frame: Uint8Array): {
  maxFrameBytes: number;
  maxRecordBytes: number;
  maxUpdatesPerSec: number;
  maxPresencePerSec: number;
  maxImageBytes: number;
  maxRoomImageBytes: bigint;
  maxRoomImages: number;
} {
  const v = view(frame);
  const at = frame.byteLength - 28;
  return {
    maxFrameBytes: v.getUint32(at, true),
    maxRecordBytes: v.getUint32(at + 4, true),
    maxUpdatesPerSec: v.getUint16(at + 8, true),
    maxPresencePerSec: v.getUint16(at + 10, true),
    maxImageBytes: v.getUint32(at + 12, true),
    maxRoomImageBytes: v.getBigUint64(at + 16, true),
    maxRoomImages: v.getUint32(at + 24, true),
  };
}

// ---- a client socket ---------------------------------------------------------------------------------------------

export interface Closed {
  code: number;
}

/** A WebSocket to a room through the Worker, with everything it received kept for the test. */
export class Client {
  /** Everything this client sent as binary frames, in order. */
  readonly sent: Uint8Array[] = [];
  readonly frames: Uint8Array[] = [];
  readonly texts: string[] = [];
  closed: Closed | null = null;
  readonly #waiters: (() => void)[] = [];

  private constructor(readonly ws: WebSocket) {
    ws.accept();
    ws.binaryType = "arraybuffer";
    ws.addEventListener("message", (event) => {
      if (typeof event.data === "string") this.texts.push(event.data);
      else this.frames.push(new Uint8Array(event.data));
      this.#wake();
    });
    ws.addEventListener("close", (event) => {
      this.closed = { code: event.code };
      this.#wake();
    });
  }

  /** Opens `/r/<room>/ws`. Throws when the upgrade is refused. */
  static async open(room: string = ROOM, headers: Record<string, string> = {}): Promise<Client> {
    const response = await request(`/r/${room}/ws`, { headers: { upgrade: "websocket", ...headers } });
    if (response.webSocket === null) throw new Error(`no WebSocket: HTTP ${response.status} ${await response.text()}`);
    return new Client(response.webSocket);
  }

  send(frameBytes: Uint8Array): void {
    this.sent.push(frameBytes);
    this.ws.send(frameBytes);
  }

  sendNamed(name: FrameName): void {
    this.send(frame(name));
  }

  #wake(): void {
    for (const resolve of this.#waiters.splice(0)) resolve();
  }

  /** Waits until `ready()` holds. */
  async until(ready: () => boolean, what: string, timeoutMs = 5_000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while (!ready()) {
      const left = deadline - Date.now();
      if (left <= 0) throw new Error(`timed out waiting for ${what}; got tags [${this.frames.map((f) => tagOf(f).toString(16))}]`);
      await new Promise<void>((resolve) => {
        this.#waiters.push(resolve);
        setTimeout(resolve, Math.min(left, 100));
      });
    }
  }

  /** Waits for the next frame with `tag` after the `from`-th one and returns it. */
  async expect(tag: number, from = 0, timeoutMs = 5_000): Promise<Uint8Array> {
    await this.until(() => this.frames.slice(from).some((f) => tagOf(f) === tag), `a frame with tag 0x${tag.toString(16)}`, timeoutMs);
    const found = this.frames.slice(from).find((f) => tagOf(f) === tag);
    if (found === undefined) throw new Error("unreachable");
    return found;
  }

  tags(): number[] {
    return this.frames.map(tagOf);
  }

  async untilClosed(timeoutMs = 5_000): Promise<Closed> {
    await this.until(() => this.closed !== null, "the socket to close", timeoutMs);
    return this.closed ?? { code: 0 };
  }

  close(): void {
    try {
      this.ws.close(1000, "");
    } catch {
      // Already closed.
    }
  }
}

export const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));
