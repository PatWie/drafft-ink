// The Access verifier with the real Rust module (compiled to wasm for Node) and real RSA signatures from Web Crypto.

import { beforeAll, describe, expect, it } from "vitest";
import { AccessVerifier, FETCH_FAILURE_BACKOFF_MS, INTERNAL_REFUSAL, KEY_SET_TTL_MS, UNKNOWN_KID_REFETCH_MS, denied, type AccessCore, type AccessDenied, type AccessOutcome } from "../src/access";
import {
  APP_HOST,
  AUD,
  CERTS_URL,
  CLIENT_ID,
  KeyServer,
  NOW_MS,
  NOW_SECS,
  TEAM,
  b64url,
  hs256WithPublicKey,
  loadCore,
  machineClaims,
  makeKey,
  personClaims,
  personToken,
  requestWith,
  rs256Header,
  signRs256,
  type TestKey,
} from "./access-fixtures";

const core = loadCore();
let keyA: TestKey;
let keyB: TestKey;
let attacker: TestKey;

beforeAll(async () => {
  [keyA, keyB, attacker] = await Promise.all([makeKey("key-a"), makeKey("key-b"), makeKey("key-evil")]);
});

interface Rig {
  readonly server: KeyServer;
  readonly verifier: AccessVerifier;
  readonly logged: string[];
}

function rig(keys: readonly TestKey[], overrides: { core?: AccessCore; aud?: string; certsUrl?: string } = {}): Rig {
  const server = new KeyServer(keys);
  const logged: string[] = [];
  const verifier = new AccessVerifier({
    teamDomain: TEAM,
    aud: overrides.aud ?? AUD,
    appHostname: APP_HOST,
    certsUrl: overrides.certsUrl,
    core: overrides.core ?? core,
    fetch: server.fetch,
    log: (reason) => logged.push(reason),
  });
  return { server, verifier, logged };
}

function refusalOf(outcome: AccessOutcome): AccessDenied["refusal"] {
  if (outcome.ok) throw new Error("expected a refusal, got a grant");
  return outcome.refusal;
}

async function expectRefused(outcome: Promise<AccessOutcome>, code: string, reason: string): Promise<void> {
  const refusal = refusalOf(await outcome);
  expect({ code: refusal.code, reason: refusal.reason }).toEqual({ code, reason });
}

describe("a valid token", () => {
  it("passes for a person, and fetches the key set from the team's certs URL", async () => {
    const { verifier, server, logged } = rig([keyA]);
    const outcome = await verifier.verify(requestWith(await personToken(keyA)), NOW_MS);
    expect(outcome).toEqual({ ok: true, identity: "person" });
    expect(server.urls).toEqual([CERTS_URL]);
    expect(logged).toEqual([]);
  });

  it("passes for a service token with common_name and no email, as a machine", async () => {
    const { verifier } = rig([keyA]);
    const token = await signRs256(keyA, rs256Header(keyA), machineClaims());
    expect(await verifier.verify(requestWith(token), NOW_MS)).toEqual({ ok: true, identity: "machine" });
  });

  it("uses certsUrl when it is given", async () => {
    const { verifier, server } = rig([keyA], { certsUrl: "http://127.0.0.1:9999/certs" });
    await verifier.verify(requestWith(await personToken(keyA)), NOW_MS);
    expect(server.urls).toEqual(["http://127.0.0.1:9999/certs"]);
  });

  it("is read from the CF_Authorization cookie when there is no header", async () => {
    const { verifier } = rig([keyA]);
    const token = await personToken(keyA);
    const request = requestWith(undefined, { headers: { cookie: `theme=dark; CF_Authorization=${token}; other=1` } });
    expect(await verifier.verify(request, NOW_MS)).toEqual({ ok: true, identity: "person" });
  });

  it("prefers the header over the cookie", async () => {
    const { verifier } = rig([keyA]);
    const good = await personToken(keyA);
    const request = requestWith(good, { headers: { cookie: "CF_Authorization=garbage" } });
    expect(await verifier.verify(request, NOW_MS)).toEqual({ ok: true, identity: "person" });
  });

  it("passes with an nbf a few seconds ahead (clock skew) and refuses one far ahead", async () => {
    const { verifier } = rig([keyA]);
    const slight = await personToken(keyA, { nbf: NOW_SECS + 3 });
    expect((await verifier.verify(requestWith(slight), NOW_MS)).ok).toBe(true);
    const far = await personToken(keyA, { nbf: NOW_SECS + 60 });
    await expectRefused(verifier.verify(requestWith(far), NOW_MS), "access_invalid", "not_yet_valid");
  });
});

describe("a token that is no good", () => {
  it("is refused for the wrong audience", async () => {
    const { verifier } = rig([keyA]);
    const token = await personToken(keyA, { aud: ["someone-elses-app"] });
    await expectRefused(verifier.verify(requestWith(token), NOW_MS), "access_invalid", "wrong_aud");
  });

  it("is refused for the wrong issuer", async () => {
    const { verifier } = rig([keyA]);
    const token = await personToken(keyA, { iss: "https://evil.cloudflareaccess.com" });
    await expectRefused(verifier.verify(requestWith(token), NOW_MS), "access_invalid", "wrong_iss");
  });

  it("is refused when it has expired", async () => {
    const { verifier } = rig([keyA]);
    const token = await personToken(keyA, { exp: NOW_SECS - 1 });
    await expectRefused(verifier.verify(requestWith(token), NOW_MS), "access_invalid", "expired");
  });

  it("is refused once the clock passes its expiry, with the key still cached", async () => {
    const { verifier, server } = rig([keyA]);
    const token = await personToken(keyA);
    expect((await verifier.verify(requestWith(token), NOW_MS)).ok).toBe(true);
    await expectRefused(verifier.verify(requestWith(token), NOW_MS + 601_000), "access_invalid", "expired");
    expect(server.calls).toBe(1);
  });

  it("is refused when neither email nor common_name is present", async () => {
    const { verifier } = rig([keyA]);
    const claims = personClaims();
    delete claims.email;
    const token = await signRs256(keyA, rs256Header(keyA), claims);
    await expectRefused(verifier.verify(requestWith(token), NOW_MS), "access_invalid", "no_identity");
  });

  it("is refused when the payload was changed after signing (bad signature)", async () => {
    const { verifier } = rig([keyA]);
    const [header, , signature] = (await personToken(keyA)).split(".");
    const forgedPayload = b64url(JSON.stringify(personClaims({ email: "mallory@example.com" })));
    await expectRefused(verifier.verify(requestWith(`${header}.${forgedPayload}.${signature}`), NOW_MS), "access_invalid", "bad_signature");
  });

  it("is refused when it is signed by another key that claims the same kid", async () => {
    const { verifier } = rig([keyA]);
    const impostor = { ...attacker, kid: keyA.kid };
    const token = await signRs256(impostor, rs256Header(impostor), personClaims());
    await expectRefused(verifier.verify(requestWith(token), NOW_MS), "access_invalid", "bad_signature");
  });

  it("is refused when the signature is cut short or empty", async () => {
    const { verifier } = rig([keyA]);
    const token = await personToken(keyA);
    // 6 characters off a 342 character signature leave valid base64url of a shorter signature.
    await expectRefused(verifier.verify(requestWith(token.slice(0, -6)), NOW_MS), "access_invalid", "bad_signature");
    for (let cut = 1; cut <= 60; cut += 1) {
      const outcome = await verifier.verify(requestWith(token.slice(0, -cut)), NOW_MS);
      expect(["bad_signature", "malformed"], `cut ${cut}`).toContain(refusalOf(outcome).reason);
    }
    await expectRefused(verifier.verify(requestWith(token.slice(0, token.lastIndexOf(".") + 1)), NOW_MS), "access_invalid", "malformed");
  });

  it("is refused for alg none, before any key is fetched", async () => {
    const { verifier, server } = rig([keyA]);
    const unsigned = `${b64url(JSON.stringify({ alg: "none", kid: keyA.kid }))}.${b64url(JSON.stringify(personClaims()))}.`;
    await expectRefused(verifier.verify(requestWith(unsigned), NOW_MS), "access_invalid", "bad_alg");
    expect(server.calls).toBe(0);
  });

  it("is refused for HS256 signed with the public key (algorithm confusion), before any key is fetched", async () => {
    const { verifier, server } = rig([keyA]);
    await expectRefused(verifier.verify(requestWith(await hs256WithPublicKey(keyA)), NOW_MS), "access_invalid", "bad_alg");
    expect(server.calls).toBe(0);
  });

  it("is refused when garbage is sent in the header, oversize, or truncated", async () => {
    const { verifier, server } = rig([keyA]);
    await expectRefused(verifier.verify(requestWith("not a token"), NOW_MS), "access_invalid", "malformed");
    await expectRefused(verifier.verify(requestWith("a.b.c"), NOW_MS), "access_invalid", "malformed");
    await expectRefused(verifier.verify(requestWith("x".repeat(9000)), NOW_MS), "access_invalid", "too_large");
    const token = await personToken(keyA);
    await expectRefused(verifier.verify(requestWith(token.slice(0, 40)), NOW_MS), "access_invalid", "malformed");
    expect(server.calls).toBe(0);
  });
});

describe("a request without a token", () => {
  it("is access_required, and no key is fetched", async () => {
    const { verifier, server, logged } = rig([keyA]);
    await expectRefused(verifier.verify(requestWith(undefined), NOW_MS), "access_required", "missing_token");
    await expectRefused(verifier.verify(requestWith("   "), NOW_MS), "access_required", "missing_token");
    expect(server.calls).toBe(0);
    expect(logged).toEqual(["missing_token", "missing_token"]);
  });

  it("is refused before parsing when the cookie comes from a page that is not ours", async () => {
    const { verifier, server } = rig([keyA]);
    const token = await personToken(keyA);
    const foreign = requestWith(undefined, { headers: { origin: "https://evil.example", cookie: `CF_Authorization=${token}` } });
    await expectRefused(verifier.verify(foreign, NOW_MS), "access_required", "foreign_origin");
    for (const origin of ["null", "not a url", `https://${APP_HOST}.evil.example`, `https://evil.example/${APP_HOST}`]) {
      const request = requestWith(undefined, { headers: { origin, cookie: `CF_Authorization=${token}` } });
      await expectRefused(verifier.verify(request, NOW_MS), "access_required", "foreign_origin");
    }
    expect(server.calls).toBe(0);
  });

  it("accepts the cookie from our own page", async () => {
    const { verifier } = rig([keyA]);
    const token = await personToken(keyA);
    const own = requestWith(undefined, { headers: { origin: `https://${APP_HOST}`, cookie: `CF_Authorization=${token}` } });
    expect((await verifier.verify(own, NOW_MS)).ok).toBe(true);
  });
});

describe("the key set", () => {
  it("verifies tokens of both keys of a rotation", async () => {
    const { verifier, server } = rig([keyA, keyB]);
    expect((await verifier.verify(requestWith(await personToken(keyA)), NOW_MS)).ok).toBe(true);
    expect((await verifier.verify(requestWith(await personToken(keyB)), NOW_MS)).ok).toBe(true);
    expect(server.calls).toBe(1);
  });

  it("is cached for an hour and fetched again after that", async () => {
    const { verifier, server } = rig([keyA]);
    const token = await personToken(keyA, { exp: NOW_SECS + 7200 });
    for (let minute = 0; minute < 60; minute += 1) {
      expect((await verifier.verify(requestWith(token), NOW_MS + minute * 60_000)).ok).toBe(true);
    }
    expect(server.calls).toBe(1);
    expect((await verifier.verify(requestWith(token), NOW_MS + KEY_SET_TTL_MS + 1)).ok).toBe(true);
    expect(server.calls).toBe(2);
  });

  it("is fetched once for requests that arrive together", async () => {
    const { verifier, server } = rig([keyA]);
    const token = await personToken(keyA);
    const outcomes = await Promise.all(Array.from({ length: 8 }, () => verifier.verify(requestWith(token), NOW_MS)));
    expect(outcomes.every((outcome) => outcome.ok)).toBe(true);
    expect(server.calls).toBe(1);
  });

  it("refetches once for an unknown kid, then not again within a minute, then refuses", async () => {
    const { verifier, server, logged } = rig([keyA]);
    expect((await verifier.verify(requestWith(await personToken(keyA)), NOW_MS)).ok).toBe(true);
    expect(server.calls).toBe(1);

    const unknown = await personToken(keyB);
    const later = NOW_MS + UNKNOWN_KID_REFETCH_MS + 1;
    await expectRefused(verifier.verify(requestWith(unknown), later), "access_invalid", "unknown_kid");
    expect(server.calls).toBe(2); // exactly one refetch

    await expectRefused(verifier.verify(requestWith(unknown), later + 1000), "access_invalid", "unknown_kid");
    await expectRefused(verifier.verify(requestWith(unknown), later + UNKNOWN_KID_REFETCH_MS - 1), "access_invalid", "unknown_kid");
    expect(server.calls).toBe(2); // none inside the minute

    await expectRefused(verifier.verify(requestWith(unknown), later + UNKNOWN_KID_REFETCH_MS), "access_invalid", "unknown_kid");
    expect(server.calls).toBe(3); // one more once the minute has passed
    expect(logged).toEqual(["unknown_kid", "unknown_kid", "unknown_kid", "unknown_kid"]);
  });

  it("picks up a rotated-in key through the one refetch", async () => {
    const { verifier, server } = rig([keyA]);
    expect((await verifier.verify(requestWith(await personToken(keyA)), NOW_MS)).ok).toBe(true);
    server.keys = [keyA, keyB]; // Access publishes the new key
    const later = NOW_MS + UNKNOWN_KID_REFETCH_MS + 1;
    expect((await verifier.verify(requestWith(await personToken(keyB)), later)).ok).toBe(true);
    expect(server.calls).toBe(2);
  });

  it("ignores keys of the wrong type and the certificate field", async () => {
    const odd = new KeyServer([keyA]).fetch;
    // A document with an EC key, a key without kid, and the real key: only the real key counts.
    const patched: typeof fetch = async (input, init) => {
      const body = (await (await odd(input, init)).json()) as { keys: unknown[] };
      return Response.json({ keys: [{ kty: "EC", kid: "ec", crv: "P-256", x: "AA", y: "AA" }, { kty: "RSA", n: "AQAB", e: "AQAB" }, ...body.keys] });
    };
    const patchedVerifier = new AccessVerifier({ teamDomain: TEAM, aud: AUD, appHostname: APP_HOST, core, fetch: patched, log: () => undefined });
    expect((await patchedVerifier.verify(requestWith(await personToken(keyA)), NOW_MS)).ok).toBe(true);
  });
});

describe("when the key set cannot be fetched", () => {
  for (const mode of ["network_error", "http_error", "garbage", "no_keys"] as const) {
    it(`fails closed with keys_unavailable (${mode}), and recovers`, async () => {
      const { verifier, server, logged } = rig([keyA]);
      server.mode = mode;
      const token = await personToken(keyA);
      await expectRefused(verifier.verify(requestWith(token), NOW_MS), "access_invalid", "keys_unavailable");
      expect(logged).toEqual(["keys_unavailable"]);

      // The failure is remembered for a few seconds only.
      await expectRefused(verifier.verify(requestWith(token), NOW_MS + 1000), "access_invalid", "keys_unavailable");
      expect(server.calls).toBe(1);

      server.mode = "ok";
      expect((await verifier.verify(requestWith(token), NOW_MS + FETCH_FAILURE_BACKOFF_MS)).ok).toBe(true);
      expect(server.calls).toBe(2);
    });
  }

  it("keeps verifying known keys when only the refetch for an unknown kid fails", async () => {
    const { verifier, server } = rig([keyA]);
    expect((await verifier.verify(requestWith(await personToken(keyA)), NOW_MS)).ok).toBe(true);
    server.mode = "network_error";
    const later = NOW_MS + UNKNOWN_KID_REFETCH_MS + 1;
    await expectRefused(verifier.verify(requestWith(await personToken(keyB)), later), "access_invalid", "keys_unavailable");
    expect((await verifier.verify(requestWith(await personToken(keyA)), later + 1)).ok).toBe(true);
  });

  it("does not trust an expired key set when the refresh fails", async () => {
    const { verifier, server } = rig([keyA]);
    const token = await personToken(keyA, { exp: NOW_SECS + 7200 });
    expect((await verifier.verify(requestWith(token), NOW_MS)).ok).toBe(true);
    server.mode = "http_error";
    await expectRefused(verifier.verify(requestWith(token), NOW_MS + KEY_SET_TTL_MS + 1), "access_invalid", "keys_unavailable");
  });
});

describe("configuration and failures of the core", () => {
  it("refuses everything when the AUD tag or the team domain is not set", async () => {
    const empty = rig([keyA], { aud: "" });
    await expectRefused(empty.verifier.verify(requestWith(await personToken(keyA, { aud: [""] })), NOW_MS), "access_invalid", "not_configured");
    expect(empty.server.calls).toBe(0);
  });

  it("fails closed when the wasm module throws", async () => {
    const broken: AccessCore = {
      parse: () => {
        throw new Error("wasm trap");
      },
      checkClaims: core.checkClaims,
      refusal: core.refusal,
    };
    const { verifier, logged } = rig([keyA], { core: broken });
    const refusal = refusalOf(await verifier.verify(requestWith(await personToken(keyA)), NOW_MS));
    expect(refusal).toEqual(INTERNAL_REFUSAL);
    expect(logged).toEqual(["internal"]);
  });

  it("keeps the fallback text equal to the text of the Rust module", () => {
    expect(INTERNAL_REFUSAL.message).toBe(core.refusal("bad_signature", APP_HOST).message);
    expect(INTERNAL_REFUSAL.code).toBe("access_invalid");
  });
});

describe("the 401 response", () => {
  it("has the access_required body of section 19.5 with the app's hostname", async () => {
    const { verifier } = rig([keyA]);
    const response = denied(refusalOnly(await verifier.verify(requestWith(undefined), NOW_MS)));
    expect(response.status).toBe(401);
    expect(response.headers.get("content-type")).toContain("application/json");
    expect(response.headers.get("location")).toBeNull();
    expect(await response.json()).toEqual({
      error: "access_required",
      message:
        "This server is only open to signed-in, approved users. Open https://canvas.patwie.com in your browser and sign in with Google. In the desktop app, enter the access token for this server in Preferences.",
    });
  });

  it("has the access_invalid body for every other refusal", async () => {
    const { verifier } = rig([keyA]);
    const response = denied(refusalOnly(await verifier.verify(requestWith("garbage"), NOW_MS)));
    expect(response.status).toBe(401);
    expect(await response.json()).toEqual({
      error: "access_invalid",
      message: "Your sign-in is not valid for this server or has expired. Reload the page and sign in again.",
    });
  });

  it("names the hostname that was configured", async () => {
    const other = new AccessVerifier({ teamDomain: TEAM, aud: AUD, appHostname: "draw.example.org", core, fetch: new KeyServer([keyA]).fetch, log: () => undefined });
    const body = (await denied(refusalOnly(await other.verify(requestWith(undefined), NOW_MS))).json()) as { message: string };
    expect(body.message).toContain("https://draw.example.org");
  });
});

function refusalOnly(outcome: AccessOutcome): AccessDenied {
  if (outcome.ok) throw new Error("expected a refusal");
  return outcome;
}

describe("identity is never exposed", () => {
  it("returns only the kind of identity", async () => {
    const { verifier } = rig([keyA]);
    const outcome = await verifier.verify(requestWith(await signRs256(keyA, rs256Header(keyA), machineClaims())), NOW_MS);
    expect(JSON.stringify(outcome)).not.toContain(CLIENT_ID);
    expect(JSON.stringify(outcome)).toBe('{"ok":true,"identity":"machine"}');
  });
});
