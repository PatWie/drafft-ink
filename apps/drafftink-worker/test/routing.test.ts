import { describe, expect, it } from "vitest";

import { Client, ERROR, MEMBER, ROOM, STRANGER, TAG, errorCode, fixtures, request, unhex } from "./helpers";
import { probe } from "./worker";

const NAME = fixtures.image.name;

/** Runs `call` and reports how many requests it sent to a Durable Object. */
async function objectCallsOf<T>(call: () => Promise<T>): Promise<{ result: T; calls: number }> {
  const before = probe().objectCalls;
  const result = await call();
  return { result, calls: probe().objectCalls - before };
}

async function body(response: Response): Promise<Record<string, unknown>> {
  return (await response.json()) as Record<string, unknown>;
}

describe("the Worker answers what it can without an object", () => {
  it("refuses a bad room id on every route", async () => {
    for (const path of ["/r/not-a-room/ws", "/r/not-a-room/f", `/r/not-a-room/f/${NAME}`, `/r/${ROOM}x/f`, "/r/AAAA/ws"]) {
      const { result, calls } = await objectCallsOf(() =>
        request(path, { headers: { upgrade: "websocket", authorization: MEMBER } }),
      );
      expect(result.status, path).toBe(404);
      expect((await body(result))["error"], path).toBe("room_missing");
      expect(calls, path).toBe(0);
    }
  });

  it("knows no other path under /r or /api", async () => {
    for (const path of [`/r/${ROOM}`, `/r/${ROOM}/zzz`, `/r/${ROOM}/f/a/b`, "/api/other", "/r/"]) {
      const { result, calls } = await objectCallsOf(() => request(path));
      expect(result.status, path).toBe(404);
      expect(calls, path).toBe(0);
    }
  });

  it("allows only the methods of a route", async () => {
    const status = await request("/api/status", { method: "POST" });
    expect(status.status).toBe(405);
    expect(status.headers.get("allow")).toBe("GET, HEAD");
    const list = await request(`/r/${ROOM}/f`, { method: "PUT", headers: { authorization: MEMBER } });
    expect(list.status).toBe(405);
    const image = await request(`/r/${ROOM}/f/${NAME}`, { method: "DELETE", headers: { authorization: MEMBER } });
    expect(image.status).toBe(405);
  });

  it("answers /api/status itself, with the next reset at 00:00 UTC", async () => {
    const { result, calls } = await objectCallsOf(() => request("/api/status"));
    expect(calls).toBe(0);
    expect(result.status).toBe(200);
    const status = await body(result);
    expect(status["status"]).toBe("ok");
    expect(status["env"]).toBe("demo");
    const now = status["now"] as number;
    const reset = status["reset_at_ms"] as number;
    expect(Math.abs(now - Date.now())).toBeLessThan(5_000);
    expect(reset).toBeGreaterThan(now);
    expect(reset - now).toBeLessThanOrEqual(86_400_000);
    expect(reset % 86_400_000).toBe(0);
  });
});

describe("picture requests are checked before any object is called", () => {
  it("needs a membership token of this room", async () => {
    for (const [what, headers] of [
      ["no header", {}],
      ["wrong scheme", { authorization: "Bearer abc" }],
      ["a token that is not base64url", { authorization: "Drafft-Member !!!" }],
      ["a token of another room", { authorization: STRANGER }],
    ] as const) {
      for (const [method, path] of [["GET", `/r/${ROOM}/f`], ["GET", `/r/${ROOM}/f/${NAME}`], ["PUT", `/r/${ROOM}/f/${NAME}`], ["HEAD", `/r/${ROOM}/f/${NAME}`]] as const) {
        const { result, calls } = await objectCallsOf(() => request(path, { method, headers, ...(method === "PUT" ? { body: "x" } : {}) }));
        expect(result.status, `${what} ${method} ${path}`).toBe(401);
        if (method !== "HEAD") expect((await body(result))["error"]).toBe("unauthorized");
        expect(calls, `${what} ${method}`).toBe(0);
      }
    }
  });

  it("refuses a name that is not 16 bytes of base64url", async () => {
    for (const name of ["x", "AAAA", "AAAAAAAAAAAAAAAAAAAAAA==", `${NAME}A`, "a%20b", "%00"]) {
      const { result, calls } = await objectCallsOf(() => request(`/r/${ROOM}/f/${name}`, { headers: { authorization: MEMBER } }));
      expect(result.status, name).toBe(400);
      expect(calls, name).toBe(0);
    }
  });

  it("never lets a dotted path segment reach a room", async () => {
    // The URL parser resolves `..` before the Worker sees the path.
    for (const path of [`/r/${ROOM}/f/..`, `/r/${ROOM}/f/%2e%2e`, `/r/${ROOM}/f/.`]) {
      const { result, calls } = await objectCallsOf(() => request(path, { headers: { authorization: MEMBER } }));
      expect([400, 404], path).toContain(result.status);
      expect(calls, path).toBe(0);
    }
  });

  it("answers 413 for a picture over the limit before the object, so no row is attempted", async () => {
    const limit = 4 * 1024 * 1024 + 78;
    const { result, calls } = await objectCallsOf(() =>
      request(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: new Uint8Array(limit + 1) }),
    );
    expect(result.status).toBe(413);
    expect((await body(result))["error"]).toBe("image_too_large");
    expect(calls).toBe(0);
  });

  it("lets a picture of exactly the limit reach the object, which refuses what is not a sealed picture", async () => {
    const limit = 4 * 1024 * 1024 + 78;
    const { result, calls } = await objectCallsOf(() =>
      request(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: new Uint8Array(limit) }),
    );
    // The room does not exist yet, which the core says before it looks at the body.
    expect([400, 404]).toContain(result.status);
    expect(calls).toBe(1);
  });
});

describe("WebSocket upgrades", () => {
  it("refuses a browser origin that is not allowed, before the object", async () => {
    const { result, calls } = await objectCallsOf(() => request(`/r/${ROOM}/ws`, { headers: { upgrade: "websocket", origin: "https://evil.example" } }));
    expect(result.status).toBe(403);
    expect((await body(result))["error"]).toBe("origin_not_allowed");
    expect(calls).toBe(0);
  });

  it("lets a native client (no Origin) and an allowed page through", async () => {
    const native = await Client.open(ROOM);
    native.close();
    const page = await Client.open(ROOM, { origin: "https://drafft.test" });
    page.close();
  });

  it("asks for an upgrade", async () => {
    const { result, calls } = await objectCallsOf(() => request(`/r/${ROOM}/ws`));
    expect(result.status).toBe(426);
    expect(calls).toBe(0);
  });

  it("costs a stranger one object call, and the core refuses him at Join", async () => {
    const before = probe().objectCalls;
    const stranger = await Client.open(ROOM);
    expect(probe().objectCalls - before).toBe(1);
    stranger.sendNamed("joinStranger");
    const refusal = await stranger.expect(TAG.error);
    expect(errorCode(refusal)).toBe(ERROR.unauthorized);
    await stranger.untilClosed();
  });

  it("closes hostile first frames without an exception", async () => {
    for (const hostile of fixtures.hostile) {
      const client = await Client.open(ROOM);
      client.send(unhex(hostile));
      const closed = await client.untilClosed();
      expect([1000, 1011], hostile).toContain(closed.code);
    }
  });
});
