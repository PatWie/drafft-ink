// The entry point of the Worker: guard(), stripIdentity() and what gets logged.

import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { clearAccessCache, guard, stripIdentity, type AccessEnv } from "../src/access";
import {
  APP_HOST,
  AUD,
  CERTS_URL,
  CLIENT_ID,
  EMAIL,
  KeyServer,
  NOW_MS,
  TEAM,
  hs256WithPublicKey,
  loadCore,
  machineClaims,
  makeKey,
  personToken,
  requestWith,
  rs256Header,
  signRs256,
  type TestKey,
} from "./access-fixtures";

const core = loadCore();
let key: TestKey;

beforeAll(async () => {
  key = await makeKey("key-a");
});

beforeEach(() => clearAccessCache());

const ENV: AccessEnv = { ACCESS_REQUIRED: "true", ACCESS_TEAM_DOMAIN: TEAM, ACCESS_AUD: AUD, ACCESS_APP_HOSTNAME: APP_HOST };

/** A stand-in for the Durable Object (and R2): it counts the requests that reach it and keeps them. */
class StubObject {
  readonly seen: Request[] = [];
  fetch(request: Request): Promise<Response> {
    this.seen.push(request);
    return Promise.resolve(new Response("stub"));
  }
}

/** The Worker's route: guard first, the object only for a request that passed. */
async function route(request: Request, stub: StubObject, server: KeyServer, env: AccessEnv = ENV, nowMs = NOW_MS): Promise<Response> {
  const checked = await guard(request, env, nowMs, { core, fetch: server.fetch, log: (reason) => console.warn(`access refused: ${reason}`) });
  return checked instanceof Response ? checked : stub.fetch(checked);
}

describe("stripIdentity", () => {
  it("removes every Access header and the Access cookies, and keeps the other cookies and headers", async () => {
    const request = new Request("https://canvas.patwie.com/r/room1/ws?x=1", {
      method: "GET",
      headers: {
        "Cf-Access-Jwt-Assertion": "token",
        "Cf-Access-Authenticated-User-Email": EMAIL,
        "CF-Access-Client-Id": CLIENT_ID,
        "CF-Access-Client-Secret": "secret",
        "cf-access-token": "cloudflared-token",
        cookie: "theme=dark; CF_Authorization=abc; CF_AppSession=def; CF_Binding=ghi; lang=en",
        upgrade: "websocket",
        "x-room-token": "membership",
        "cf-connecting-ip": "203.0.113.7",
      },
    });
    const stripped = stripIdentity(request);
    expect([...stripped.headers.keys()].filter((name) => name.startsWith("cf-access-"))).toEqual([]);
    expect(stripped.headers.get("cookie")).toBe("theme=dark; lang=en");
    expect(stripped.headers.get("x-room-token")).toBe("membership");
    expect(stripped.headers.get("cf-connecting-ip")).toBe("203.0.113.7");
    expect(stripped.url).toBe(request.url);
    expect(stripped.method).toBe("GET");
    // The input is left alone.
    expect(request.headers.get("cf-access-jwt-assertion")).toBe("token");
  });

  it("drops the cookie header when only Access cookies were in it", () => {
    const stripped = stripIdentity(new Request("https://x.example/", { headers: { cookie: "CF_Authorization=abc" } }));
    expect(stripped.headers.has("cookie")).toBe(false);
  });

  it("does not touch a cookie whose name only starts like an Access cookie", () => {
    const stripped = stripIdentity(new Request("https://x.example/", { headers: { cookie: "CF_Authorization_extra=1; xCF_Authorization=2" } }));
    expect(stripped.headers.get("cookie")).toBe("CF_Authorization_extra=1; xCF_Authorization=2");
  });

  it("keeps the body of an upload", async () => {
    const request = new Request("https://x.example/r/room1/f/pic", { method: "PUT", body: "sealed bytes", headers: { "Cf-Access-Jwt-Assertion": "t" } });
    const stripped = stripIdentity(request);
    expect(stripped.method).toBe("PUT");
    expect(await stripped.text()).toBe("sealed bytes");
  });
});

describe("guard", () => {
  it("forwards a valid request to the object with the identity stripped", async () => {
    const server = new KeyServer([key]);
    const stub = new StubObject();
    const token = await personToken(key);
    const request = requestWith(token, { headers: { "CF-Access-Client-Id": "id", cookie: `a=1; CF_Authorization=${token}`, upgrade: "websocket" } });
    const response = await route(request, stub, server);
    expect(response.status).toBe(200);
    expect(stub.seen).toHaveLength(1);
    const seen = stub.seen[0];
    expect(seen?.headers.get("cf-access-jwt-assertion")).toBeNull();
    expect(seen?.headers.get("cf-access-client-id")).toBeNull();
    expect(seen?.headers.get("cookie")).toBe("a=1");
    expect(seen?.headers.get("upgrade")).toBe("websocket");
  });

  it("forwards a valid service-token request", async () => {
    const stub = new StubObject();
    const token = await signRs256(key, rs256Header(key), machineClaims());
    expect((await route(requestWith(token), stub, new KeyServer([key]))).status).toBe(200);
    expect(stub.seen).toHaveLength(1);
  });

  it("answers 401 and never calls the object for a request that is refused", async () => {
    const server = new KeyServer([key]);
    const stub = new StubObject();
    const valid = await personToken(key);
    const requests: [string, Request][] = [
      ["no header", requestWith(undefined)],
      ["garbage", requestWith("garbage")],
      ["expired", requestWith(await personToken(key, { exp: NOW_MS / 1000 - 5 }))],
      ["wrong aud", requestWith(await personToken(key, { aud: ["x"] }))],
      ["wrong iss", requestWith(await personToken(key, { iss: "https://x.cloudflareaccess.com" }))],
      ["alg none", requestWith(`${valid.split(".")[0] ?? ""}.${valid.split(".")[1] ?? ""}.`)],
      ["hs256", requestWith(await hs256WithPublicKey(key))],
      ["cookie from a foreign page", requestWith(undefined, { headers: { origin: "https://evil.example", cookie: `CF_Authorization=${valid}` } })],
      ["api path", requestWith(undefined, { url: `https://${APP_HOST}/api/status` })],
    ];
    for (const [label, request] of requests) {
      const response = await route(request, stub, server);
      expect(response.status, label).toBe(401);
      expect(response.headers.get("location"), label).toBeNull();
      const body = (await response.json()) as { error: string; message: string };
      expect(["access_required", "access_invalid"], label).toContain(body.error);
      expect(body.message.length, label).toBeGreaterThan(20);
    }
    expect(stub.seen).toHaveLength(0);
  });

  it("refuses a request on a workers.dev style host that has no assertion (the backstop)", async () => {
    const stub = new StubObject();
    const env: AccessEnv = { ACCESS_REQUIRED: true, ACCESS_TEAM_DOMAIN: TEAM, ACCESS_AUD: AUD };
    const response = await route(requestWith(undefined, { url: "https://drafftink-relay-private.someone.workers.dev/r/room1/ws" }), stub, new KeyServer([key]), env);
    expect(response.status).toBe(401);
    expect(((await response.json()) as { error: string }).error).toBe("access_required");
    expect(stub.seen).toHaveLength(0);
  });

  it("refuses everything when ACCESS_REQUIRED is on and the AUD tag is not set", async () => {
    const stub = new StubObject();
    const env: AccessEnv = { ACCESS_REQUIRED: "true", ACCESS_TEAM_DOMAIN: TEAM };
    const response = await route(requestWith(await personToken(key)), stub, new KeyServer([key]), env);
    expect(response.status).toBe(401);
    expect(stub.seen).toHaveLength(0);
  });

  it("checks nothing when ACCESS_REQUIRED is off, and still strips the identity", async () => {
    const server = new KeyServer([key]);
    const stub = new StubObject();
    for (const env of [{}, { ACCESS_REQUIRED: "false" }, { ACCESS_REQUIRED: false }, { ACCESS_REQUIRED: "yes" }]) {
      const response = await route(requestWith("garbage", { headers: { "CF-Access-Client-Id": "id" } }), stub, server, env);
      expect(response.status).toBe(200);
    }
    expect(stub.seen).toHaveLength(4);
    expect(stub.seen.every((seen) => seen.headers.get("cf-access-jwt-assertion") === null && seen.headers.get("cf-access-client-id") === null)).toBe(true);
    expect(server.calls).toBe(0);
  });

  it("caches the key set across requests, and ACCESS_CERTS_URL overrides the address", async () => {
    const server = new KeyServer([key]);
    const stub = new StubObject();
    const env: AccessEnv = { ...ENV, ACCESS_CERTS_URL: "http://127.0.0.1:8788/certs" };
    for (let i = 0; i < 5; i += 1) await route(requestWith(await personToken(key)), stub, server, env);
    expect(server.calls).toBe(1);
    expect(server.urls).toEqual(["http://127.0.0.1:8788/certs"]);
    expect(stub.seen).toHaveLength(5);
  });

  it("uses the certs endpoint of the team domain by default", async () => {
    const server = new KeyServer([key]);
    await route(requestWith(await personToken(key)), new StubObject(), server);
    expect(server.urls).toEqual([CERTS_URL]);
  });
});

describe("logging", () => {
  const spies = (["log", "info", "warn", "error", "debug"] as const).map((method) => vi.spyOn(console, method).mockImplementation(() => undefined));

  afterEach(() => {
    for (const spy of spies) spy.mockClear();
  });

  it("contains the reason words and never a token, a part of one, an email or a client id", async () => {
    const server = new KeyServer([key]);
    const stub = new StubObject();
    const person = await personToken(key);
    const service = await signRs256(key, rs256Header(key), machineClaims());
    const forged = `${person.split(".")[0] ?? ""}.${Buffer.from(JSON.stringify({ email: "mallory@example.com" })).toString("base64url")}.${person.split(".")[2] ?? ""}`;
    const tokens = [person, service, forged, await personToken(key, { exp: 1 }), await personToken(key, { aud: ["x"] }), await hs256WithPublicKey(key)];
    for (const token of tokens) {
      await route(requestWith(token), stub, server);
      await route(requestWith(undefined, { headers: { cookie: `CF_Authorization=${token}`, origin: "https://evil.example" } }), stub, server);
    }
    await route(requestWith(undefined), stub, server);
    server.mode = "network_error";
    clearAccessCache();
    await route(requestWith(person), stub, server);

    const output = spies.flatMap((spy) => spy.mock.calls).map((args) => args.map(String).join(" ")).join("\n");
    expect(output).toContain("expired");
    expect(output).toContain("wrong_aud");
    expect(output).toContain("bad_signature");
    expect(output).toContain("bad_alg");
    expect(output).toContain("foreign_origin");
    expect(output).toContain("missing_token");
    expect(output).toContain("keys_unavailable");
    for (const token of tokens) {
      for (const part of token.split(".")) {
        if (part.length > 0) expect(output).not.toContain(part);
      }
    }
    for (const secret of [EMAIL, "alice", "mallory", CLIENT_ID, "0123456789abcdef", "eyJ"]) {
      expect(output).not.toContain(secret);
    }
  });

  it("logs nothing for a request that passes", async () => {
    await route(requestWith(await personToken(key)), new StubObject(), new KeyServer([key]));
    expect(spies.flatMap((spy) => spy.mock.calls)).toEqual([]);
  });
});
