// The Worker as the tests run it: the real Worker and the real Room, with a probe in the room that the tests read.
// It is a subclass so that the production code carries no test hooks except the one clock method.
import type { Env } from "../src/env";
import { failureOf } from "../src/host-error";
import worker, { Room as ProductionRoom } from "../src/index";
import type { SqlHost } from "../src/wasm";

/** What the tests can see and set, shared through `globalThis` because tests and Worker run in one isolate. */
export interface Probe {
  /** Added to the clock of every room. Time jumps without waiting. */
  clockOffsetMs: number;
  /** Requests that reached an object (`fetch`). */
  objectCalls: number;
  /** Frames and texts that reached `webSocketMessage` (a `ping` must not). */
  socketMessages: number;
  /** Alarms that ran. */
  alarms: number;
  /** When set, every request to an object throws this (a platform quota error seen by the Worker). */
  failFetchWith: Error | null;
  /** When set, every storage call of a room throws this (a quota error inside the object). */
  failStorageWith: Error | null;
  /** When set, creating the tables throws this (a quota error while a room wakes). */
  failSchemaWith: Error | null;
  /** Room instances built so far: a hibernation shows as a new one. */
  constructed: number;
}

const KEY = Symbol.for("drafftink.test.probe");

export function probe(): Probe {
  const holder = globalThis as unknown as Record<symbol, Probe | undefined>;
  const existing = holder[KEY];
  if (existing !== undefined) return existing;
  const fresh: Probe = { clockOffsetMs: 0, objectCalls: 0, socketMessages: 0, alarms: 0, failFetchWith: null, failStorageWith: null, failSchemaWith: null, constructed: 0 };
  holder[KEY] = fresh;
  return fresh;
}

export default worker;

/** Wraps the rows of a room so that a test can make every call fail. */
function failing(inner: SqlHost): SqlHost {
  const guard = <A extends unknown[], R>(call: (...args: A) => R) => (...args: A): R => {
    const failure = probe().failStorageWith;
    // The production store maps every exception of `sql.exec` the same way (`guard`).
    if (failure !== null) throw failureOf(failure);
    return call(...args);
  };
  return {
    loadMeta: guard(() => inner.loadMeta()),
    saveMeta: guard((meta) => inner.saveMeta(meta)),
    appendSegment: guard((first, last, data, meta) => inner.appendSegment(first, last, data, meta)),
    readSegments: guard((after, max) => inner.readSegments(after, max)),
    readSnapshot: guard(() => inner.readSnapshot()),
    putSnapshot: guard((covers, record, meta) => inner.putSnapshot(covers, record, meta)),
    deleteAll: () => inner.deleteAll(),
  };
}

export class Room extends ProductionRoom {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    probe().constructed++;
  }

  protected override heardAt(ws: WebSocket): number | null {
    const real = super.heardAt(ws);
    return real === null ? null : real + probe().clockOffsetMs;
  }

  protected override prepareStorage(): void {
    const failure = probe().failSchemaWith;
    if (failure !== null) throw failureOf(failure);
    super.prepareStorage();
  }

  protected override sqlHost(): SqlHost {
    return failing(super.sqlHost());
  }

  protected override now(): number {
    return Date.now() + probe().clockOffsetMs;
  }

  override async fetch(request: Request): Promise<Response> {
    probe().objectCalls++;
    const failure = probe().failFetchWith;
    if (failure !== null) throw failure;
    return super.fetch(request);
  }

  override async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    probe().socketMessages++;
    return super.webSocketMessage(ws, message);
  }

  override async alarm(): Promise<void> {
    probe().alarms++;
    return super.alarm();
  }
}
