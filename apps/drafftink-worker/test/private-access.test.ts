import { env } from "cloudflare:workers";
import { afterEach, describe, expect, it } from "vitest";

import { Client, ERROR, MEMBER, ROOM, TAG, errorCode, fixtures, joined, request, unhex } from "./helpers";
import { ASSERTION, PRIVATE_ORIGIN, machine, mint, person, resetAll, signedClient, signedRequest } from "./private-helpers";
import { probe } from "./worker";

const NAME = fixtures.image.name;

afterEach(resetAll);

async function error(response: Response): Promise<string> {
  return ((await response.json()) as { error: string }).error;
}

describe("without a sign-in nothing gets through", () => {
  it("answers 401 access_required on every route and calls neither an object nor R2", async () => {
    // Something is in R2 under the right key: it must not be served without a sign-in.
    const key = `r/${"00".repeat(16)}/${"11".repeat(16)}`;
    await env.BLOBS.put(key, new Uint8Array([1, 2, 3]));
    const calls = probe().objectCalls;
    for (const [method, path] of [
      ["GET", "/api/status"],
      ["GET", `/r/${ROOM}/f`],
      ["GET", `/r/${ROOM}/f/${NAME}`],
      ["PUT", `/r/${ROOM}/f/${NAME}`],
      ["HEAD", `/r/${ROOM}/f/${NAME}`],
      ["GET", "/api/anything"],
      ["GET", "/r/not-a-room/ws"],
    ] as const) {
      const response = await request(path, { method, headers: { authorization: MEMBER, ...(method === "PUT" ? {} : {}) } }, PRIVATE_ORIGIN);
      expect(response.status, `${method} ${path}`).toBe(401);
      expect(response.headers.get("content-type")).toContain("application/json");
      if (method !== "HEAD") expect(await error(response), path).toBe("access_required");
    }
    const upgrade = await request(`/r/${ROOM}/ws`, { headers: { upgrade: "websocket" } }, PRIVATE_ORIGIN);
    expect(upgrade.status).toBe(401);
    expect(upgrade.webSocket).toBeNull();
    expect(probe().objectCalls, "no object was called").toBe(calls);
    expect((await env.BLOBS.head(key))?.size, "and R2 was not touched").toBe(3);
  });

  it("refuses a request on a workers.dev style host that carries no assertion", async () => {
    const host = "https://drafftink-relay-private.someone.workers.dev";
    for (const path of ["/api/status", `/r/${ROOM}/f`]) {
      const response = await request(path, { headers: { authorization: MEMBER } }, host);
      expect(response.status, path).toBe(401);
    }
    // Not even with a cookie of the application from a foreign page.
    const forged = await request("/api/status", { headers: { cookie: "CF_Authorization=abc", origin: "https://evil.example" } }, host);
    expect(forged.status).toBe(401);
    expect(await error(forged)).toBe("access_required");
  });

  it("refuses a token that is not valid, with access_invalid", async () => {
    const good = await person();
    const [h, p, s] = good.split(".");
    const forged = `${h}.${p}.${s?.slice(0, -4)}AAAA`;
    for (const [what, token] of [
      ["a broken signature", forged],
      ["another audience", await mint({ aud: ["someone-else"] })],
      ["another issuer", await mint({ iss: "https://evil.cloudflareaccess.com" })],
      ["an expired token", await mint({ exp: Math.floor(Date.now() / 1000) - 10 })],
      ["no identity", await mint({ email: undefined })],
      ["algorithm none", `${btoa(JSON.stringify({ alg: "none", typ: "JWT" })).replaceAll("=", "")}.${p}.`],
      ["garbage", "not a token"],
    ] as const) {
      const response = await signedRequest("/api/status", {}, token);
      expect(response.status, what).toBe(401);
      expect(await error(response), what).toBe("access_invalid");
    }
  });
});

describe("with a valid sign-in", () => {
  it("lets a person and a service token through", async () => {
    for (const token of [await person(), await machine()]) {
      const response = await signedRequest("/api/status", {}, token);
      expect(response.status).toBe(200);
      expect(((await response.json()) as { env: string }).env).toBe("private");
    }
  });

  it("keeps a WebSocket upgrade working after the request was rebuilt without the Access headers", async () => {
    for (const token of [await person(), await machine()]) {
      const client = await signedClient(ROOM, token);
      client.sendNamed("joinCreateA");
      const frame = await client.expect(TAG.joined);
      expect(joined(frame).roomState).toBeGreaterThanOrEqual(0);
      client.sendNamed("appendA1");
      await client.expect(TAG.ack);
      client.close();
      await client.untilClosed();
      await resetAll();
    }
  });

  it("still lets the room refuse a stranger who has a sign-in but no membership token", async () => {
    const client = await signedClient(ROOM);
    client.sendNamed("joinStranger");
    expect(errorCode(await client.expect(TAG.error))).toBe(ERROR.unauthorized);
  });

  it("strips every Access header and cookie before the object sees the request, upgrade and picture alike", async () => {
    const token = await person();
    const extra = {
      "cf-access-authenticated-user-email": "alice@example.com",
      "cf-access-client-id": "id.access",
      "cf-access-client-secret": "secret",
      cookie: `theme=dark; CF_Authorization=${token}; CF_AppSession=x; lang=en`,
    };
    const socket = await Client.open(ROOM, { [ASSERTION]: token, ...extra });
    socket.sendNamed("joinCreateA");
    await socket.expect(TAG.joined);
    const put = await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER, ...extra }, body: unhex(fixtures.image.sealedHex) }, token);
    expect(put.status).toBe(201);

    expect(probe().objectRequests.length).toBeGreaterThanOrEqual(2);
    for (const seen of probe().objectRequests) {
      const names = seen.headers.map(([name]) => name.toLowerCase());
      expect(names.filter((n) => n.startsWith("cf-access-")), seen.path).toEqual([]);
      const cookie = seen.headers.find(([name]) => name.toLowerCase() === "cookie")?.[1];
      if (cookie !== undefined) expect(cookie, seen.path).toBe("theme=dark; lang=en");
      expect(names, "the membership header goes on").toEqual(expect.arrayContaining(seen.path.includes("/f/") ? ["authorization"] : []));
    }
    socket.close();
  });
});
