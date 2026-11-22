import { abortAllDurableObjects } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";

import { Client, ERROR, MEMBER, ROOM, TAG, errorCode, fixtures, request, retryAfterMs, unhex } from "./helpers";
import { probe } from "./worker";

const NAME = fixtures.image.name;
const DAY_MS = 86_400_000;
const QUOTA = new Error("Exceeded the account's spending cap for Durable Objects.");

afterEach(async () => {
  probe().failStorageWith = null;
  probe().failFetchWith = null;
  await abortAllDurableObjects();
});

/** The tests of this file run in order: the Worker remembers a used-up limit for the rest of the isolate. */
describe("a used-up daily limit inside a room", () => {
  it("a join gets Error{DailyLimit} with the time to 00:00 UTC, and the socket closes with 1013", async () => {
    probe().failStorageWith = QUOTA;
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    const refusal = await a.expect(TAG.error);
    expect(errorCode(refusal)).toBe(ERROR.dailyLimit);
    const wait = retryAfterMs(refusal);
    expect(wait).not.toBeNull();
    expect(wait ?? 0).toBeGreaterThan(0);
    expect(wait ?? 0).toBeLessThanOrEqual(DAY_MS);
    expect(((Date.now() + (wait ?? 0)) % DAY_MS)).toBeLessThan(10_000);
    expect((await a.untilClosed()).code).toBe(1013);
  });

  it("other storage failures stay Unavailable and close normally", async () => {
    probe().failStorageWith = new Error("disk I/O error");
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    expect(errorCode(await a.expect(TAG.error))).toBe(ERROR.unavailable);
    expect((await a.untilClosed()).code).toBe(1000);
  });

  it("a failed write sheds everybody with 1013 instead of retrying every second for hours", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    const b = await Client.open();
    b.sendNamed("joinOpenB");
    await b.expect(TAG.joined);
    probe().failStorageWith = QUOTA;
    a.sendNamed("appendA1");
    // The record was fanned out at once; the write at the flush fails.
    expect(errorCode(await a.expect(TAG.error))).toBe(ERROR.dailyLimit);
    expect(errorCode(await b.expect(TAG.error))).toBe(ERROR.dailyLimit);
    expect((await a.untilClosed()).code).toBe(1013);
    expect((await b.untilClosed()).code).toBe(1013);
    expect(a.tags(), "no Ack for a record that was not stored").not.toContain(TAG.ack);
  });
});

describe("the Worker", () => {
  it("answers 503 unavailable when an object call fails for another reason, and does not give up the object", async () => {
    probe().failFetchWith = new Error("the object is overloaded");
    const response = await request(`/r/${ROOM}/f`, { headers: { authorization: MEMBER } });
    expect(response.status).toBe(503);
    expect(((await response.json()) as { error: string }).error).toBe("unavailable");
    probe().failFetchWith = null;
    const status = await request("/api/status");
    expect(status.status, "one failure is not a used-up limit").toBe(200);
  });

  it("notes a quota error that an object reports on a picture request, then answers at the edge until 00:00 UTC", async () => {
    probe().failStorageWith = QUOTA;
    const first = await request(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: unhex(fixtures.image.sealedHex) });
    expect(first.status).toBe(503);
    const body = (await first.json()) as { error: string; retry_after_s: number };
    expect(body.error).toBe("daily_limit");
    expect(body.retry_after_s).toBeGreaterThan(0);
    expect(body.retry_after_s).toBeLessThanOrEqual(86_400);
    expect(first.headers.get("retry-after")).toBe(String(body.retry_after_s));
    probe().failStorageWith = null;

    // The Worker now answers by itself: no object is called.
    const before = probe().objectCalls;
    const list = await request(`/r/${ROOM}/f`, { headers: { authorization: MEMBER } });
    expect(list.status).toBe(503);
    expect(((await list.json()) as { error: string }).error).toBe("daily_limit");
    const status = await request("/api/status");
    expect(status.status).toBe(503);
    const probeBody = (await status.json()) as { error: string; retry_after_s: number };
    expect(probeBody.error).toBe("daily_limit");
    expect(probeBody.retry_after_s).toBeGreaterThan(0);
    // A request that fails the membership check is still a 401, and a bad id is still a 404.
    expect((await request(`/r/${ROOM}/f`)).status).toBe(401);
    expect((await request("/r/nope/f")).status).toBe(404);
    expect(probe().objectCalls - before, "not one object call").toBe(0);
  });

  it("accepts a WebSocket upgrade only to say why: an Error frame, then close 1013, without an object", async () => {
    const before = probe().objectCalls;
    const a = await Client.open();
    const refusal = await a.expect(TAG.error);
    expect(errorCode(refusal)).toBe(ERROR.dailyLimit);
    expect(retryAfterMs(refusal) ?? 0).toBeGreaterThan(0);
    expect((await a.untilClosed()).code).toBe(1013);
    expect(probe().objectCalls - before).toBe(0);
  });
});
