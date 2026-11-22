import { abortAllDurableObjects } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";

import { Client, MEMBER, ROOM, request } from "./helpers";
import { probe } from "./worker";

// Its own file: the Worker remembers a used-up limit for the rest of the isolate, which would change the other tests.
afterEach(async () => {
  probe().failSchemaWith = null;
  await abortAllDurableObjects();
});

describe("a room that cannot wake because the daily limit is used up", () => {
  it("is a 503 daily_limit for an upgrade (no socket exists yet) and for a picture request", async () => {
    probe().failSchemaWith = new Error("Exceeded the account's spending cap for Durable Objects.");
    const upgrade = await request(`/r/${ROOM}/ws`, { headers: { upgrade: "websocket" } });
    expect(upgrade.status).toBe(503);
    const body = (await upgrade.json()) as { error: string; retry_after_s: number };
    expect(body.error).toBe("daily_limit");
    expect(body.retry_after_s).toBeGreaterThan(0);
    probe().failSchemaWith = null;

    // The Worker noted it: the next upgrade is accepted to be told, with a frame and 1013.
    const next = await Client.open();
    expect((await next.untilClosed()).code).toBe(1013);
    const list = await request(`/r/${ROOM}/f`, { headers: { authorization: MEMBER } });
    expect(list.status).toBe(503);
  });
});
