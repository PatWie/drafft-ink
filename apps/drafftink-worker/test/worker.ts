// The Worker as the tests run it: the real Worker and the real Room, with a probe in the room that the tests read.
// It is a subclass so that the production code carries no test hooks except the one clock method.
import worker, { Room as ProductionRoom } from "../src/index";

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
  /** When set, the next object requests throw this (a platform quota error). */
  failFetchWith: Error | null;
}

const KEY = Symbol.for("drafftink.test.probe");

export function probe(): Probe {
  const holder = globalThis as unknown as Record<symbol, Probe | undefined>;
  const existing = holder[KEY];
  if (existing !== undefined) return existing;
  const fresh: Probe = { clockOffsetMs: 0, objectCalls: 0, socketMessages: 0, alarms: 0, failFetchWith: null };
  holder[KEY] = fresh;
  return fresh;
}

export default worker;

export class Room extends ProductionRoom {
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
