/**
 * What a hibernated socket carries with it (`serializeAttachment`, at most 16 KiB; this is about 100 bytes). The
 * core is rebuilt on every wake, so everything it must know about a connection is here.
 */
export interface Attachment {
  v: 1;
  /** The number the core knows the connection by. */
  conn: number;
  /** When the socket opened, in milliseconds since the epoch. */
  connectedAt: number;
  /** The last frame that reached the room (a `ping` does not: the runtime answers it). */
  lastMessageAt: number;
  /** The client id (16 bytes) and the join order, once the socket has joined. */
  client?: Uint8Array;
  order?: number;
}

/** Reads an attachment back. `null` for anything that this build did not write. */
export function parseAttachment(value: unknown): Attachment | null {
  if (typeof value !== "object" || value === null) return null;
  const v = value as Record<string, unknown>;
  if (v["v"] !== 1 || typeof v["conn"] !== "number" || typeof v["connectedAt"] !== "number") return null;
  if (typeof v["lastMessageAt"] !== "number") return null;
  const attachment: Attachment = { v: 1, conn: v["conn"], connectedAt: v["connectedAt"], lastMessageAt: v["lastMessageAt"] };
  if (v["client"] instanceof Uint8Array && v["client"].byteLength === 16 && typeof v["order"] === "number") {
    attachment.client = v["client"];
    attachment.order = v["order"];
  }
  return attachment;
}

/** The sockets of a room by the number the core knows them by. */
export class Sockets {
  readonly #byConn = new Map<number, WebSocket>();
  readonly #byWs = new Map<WebSocket, number>();

  add(conn: number, ws: WebSocket): void {
    this.#byConn.set(conn, ws);
    this.#byWs.set(ws, conn);
  }

  remove(conn: number): void {
    const ws = this.#byConn.get(conn);
    this.#byConn.delete(conn);
    if (ws !== undefined) this.#byWs.delete(ws);
  }

  socket(conn: number): WebSocket | undefined {
    return this.#byConn.get(conn);
  }

  conn(ws: WebSocket): number | undefined {
    return this.#byWs.get(ws);
  }

  entries(): IterableIterator<[number, WebSocket]> {
    return this.#byConn.entries();
  }

  /** The next free connection number. */
  next(): number {
    let n = 1;
    for (const conn of this.#byConn.keys()) n = Math.max(n, conn + 1);
    return n;
  }

  clear(): void {
    this.#byConn.clear();
    this.#byWs.clear();
  }
}

/** Close codes that may be sent (RFC 6455, section 7.4): 1005, 1006 and 1015 are for local reports only. */
export function legalCloseCode(code: number): boolean {
  return (code >= 1000 && code <= 1003) || (code >= 1007 && code <= 1014) || (code >= 3000 && code <= 4999);
}
