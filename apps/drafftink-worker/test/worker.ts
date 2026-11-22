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
  /** When set, the R2 call of that name throws this (R2 is down, or answers 429). */
  failR2: { put: Error | null; list: Error | null; delete: Error | null };
  /** When set, a purge of R2 lists this many objects at a time (the default is 1000). */
  r2PageSize: number | null;
  /** Every request that reached an object: its path and the headers it carried. */
  objectRequests: { path: string; headers: [string, string][] }[];
  /** Room instances built so far: a hibernation shows as a new one. */
  constructed: number;
}

const KEY = Symbol.for("drafftink.test.probe");

export function probe(): Probe {
  const holder = globalThis as unknown as Record<symbol, Probe | undefined>;
  const existing = holder[KEY];
  if (existing !== undefined) return existing;
  const fresh: Probe = { clockOffsetMs: 0, objectCalls: 0, socketMessages: 0, alarms: 0, failFetchWith: null, failStorageWith: null, failSchemaWith: null, failR2: { put: null, list: null, delete: null }, r2PageSize: null, objectRequests: [], constructed: 0 };
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

/** Wraps the R2 bucket so that a test can make `put`, `list` or `delete` fail. */
function flaky(inner: R2Bucket): R2Bucket {
  return new Proxy(inner, {
    get(target, property) {
      const original: unknown = Reflect.get(target, property);
      if (typeof original !== "function") return original;
      return (...args: unknown[]): unknown => {
        const failure = property === "put" || property === "list" || property === "delete" ? probe().failR2[property] : null;
        if (failure !== null) return Promise.reject(failure);
        return Reflect.apply(original, target, args);
      };
    },
  });
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

  protected override bucket(): R2Bucket {
    return flaky(super.bucket());
  }

  protected override r2PageSize(): number {
    return probe().r2PageSize ?? super.r2PageSize();
  }

  protected override sqlHost(): SqlHost {
    return failing(super.sqlHost());
  }

  protected override now(): number {
    return Date.now() + probe().clockOffsetMs;
  }

  override async fetch(request: Request): Promise<Response> {
    probe().objectCalls++;
    probe().objectRequests.push({ path: new URL(request.url).pathname, headers: [...request.headers] });
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
