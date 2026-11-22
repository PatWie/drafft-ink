import { describe, expect, it } from "vitest";

import { ROOM, request } from "./helpers";
import { PRIVATE_ORIGIN, mint, signedRequest } from "./private-helpers";
import { probe } from "./worker";

// This project requires Access but has no team domain and no AUD tag, and its switch is "yes" (a typo for "true").
// Failing closed means that nobody gets in: not without a token, and not with a token that is signed by the very key
// that the Worker could trust if it were configured.
describe("Access required but not configured", () => {
  it("refuses everything, with and without a token", async () => {
    const calls = probe().objectCalls;
    const token = await mint();
    for (const path of ["/api/status", `/r/${ROOM}/f`, `/r/${ROOM}/ws`]) {
      const without = await request(path, { headers: { upgrade: "websocket" } }, PRIVATE_ORIGIN);
      expect(without.status, path).toBe(401);
      const withToken = await signedRequest(path, { headers: { upgrade: "websocket" } }, token);
      expect(withToken.status, path).toBe(401);
      expect(((await withToken.json()) as { error: string }).error).toBe("access_invalid");
    }
    expect(probe().objectCalls, "no object was called").toBe(calls);
  });
});
