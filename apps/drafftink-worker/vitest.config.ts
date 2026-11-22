import { execFileSync } from "node:child_process";
import { generateKeyPairSync } from "node:crypto";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

const app = dirname(fileURLToPath(import.meta.url));

/** The test Access team: the key set that the Worker fetches is served by the outbound handler below. */
const TEAM = "acme.cloudflareaccess.com";
const CERTS_URL = "https://certs.test/certs";
const KEY_ID = "test-key-1";
const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const privateJwk = { ...privateKey.export({ format: "jwk" }), kid: KEY_ID, alg: "RS256", use: "sig" };
const publicJwk = { ...publicKey.export({ format: "jwk" }), kid: KEY_ID, alg: "RS256", use: "sig" };

/**
 * Renders the wrangler config of the Worker for the tests: the test entry (`test/worker.ts`), and the top level
 * mirrors `environment`. `env` sets the variables of `render-config.sh`.
 */
function config(name: string, environment: "demo" | "private", env: Record<string, string>): string {
  const out = join(app, ".wrangler", "test", `${name}.toml`);
  mkdirSync(dirname(out), { recursive: true });
  execFileSync("bash", [join(app, "scripts", "render-config.sh"), environment, out], {
    env: { ...process.env, MAIN: join(app, "test", "worker.ts"), ...env },
    stdio: "pipe",
  });
  return out;
}

/** Answers the Worker's request for the Access key set. Every other outbound request is refused: nothing leaves. */
function outbound(request: Request): Response {
  if (request.url === CERTS_URL) return Response.json({ keys: [publicJwk] });
  return new Response("no network in tests", { status: 502 });
}

/** One project is one set of variables: the room reads them when it wakes. */
function project(
  name: string,
  include: string[],
  vars: Record<string, string>,
  environment: "demo" | "private" = "demo",
) {
  return {
    extends: true as const,
    plugins: [
      cloudflareTest({
        wrangler: { configPath: config(name, environment, vars) },
        // A real Worker with real bindings; nothing leaves the machine.
        remoteBindings: false,
        miniflare: {
          outboundService: outbound,
          bindings: { TEST_ACCESS_PRIVATE_JWK: JSON.stringify(privateJwk), TEST_ACCESS_KEY_ID: KEY_ID },
        },
      }),
    ],
    test: { name, include, testTimeout: 20_000 },
  };
}

/** The Access settings of the private tests. */
const ACCESS = {
  PRIVATE_ACCESS_REQUIRED: "true",
  PRIVATE_ACCESS_TEAM_DOMAIN: TEAM,
  PRIVATE_ACCESS_AUD: "aud-tag-1234",
  PRIVATE_DOMAIN: "canvas.test",
  PRIVATE_ACCESS_APP_HOSTNAME: "canvas.test",
  PRIVATE_ACCESS_CERTS_URL: CERTS_URL,
  PRIVATE_ALLOWED_ORIGINS: "https://canvas.test",
};

export default defineConfig({
  test: {
    projects: [
      // Rooms as the demo runs them: a 48 hour deadline and a one second grace, so a test can wait for it.
      project(
        "demo",
        ["test/routing.test.ts", "test/room.test.ts", "test/retention.test.ts", "test/canary.test.ts", "test/units.test.ts", "test/quota.test.ts", "test/quota-wake.test.ts"],
        { DEMO_ROOM_LEAVE_GRACE_SECS: "1", DEMO_ALLOWED_ORIGINS: "https://drafft.test" },
      ),
      // No ROOM_TTL_SECS: no deadline and no alarm. Deleting a room is allowed, as on the private instance.
      project("no-expiry", ["test/no-expiry.test.ts"], { DEMO_ROOM_TTL_SECS: "", DEMO_ALLOW_DELETE_ROOM: "true" }),
      // The private environment: Access on, pictures in R2, rooms never expire, a member may delete the room, and
      // limits that differ from the defaults so that a test can see the environment's own numbers.
      project(
        "private",
        ["test/private-*.test.ts"],
        {
          ...ACCESS,
          PRIVATE_MAX_ROOM_IMAGES: "7",
          PRIVATE_MAX_ROOM_IMAGE_BYTES: "10485760",
          PRIVATE_MAX_IMAGE_BYTES: "2097152",
        },
        "private",
      ),
      // Access required, but the team domain and the AUD tag are missing, and the switch is a typo: all is refused.
      project(
        "private-misconfigured",
        ["test/misconfigured-*.test.ts"],
        { ...ACCESS, PRIVATE_ACCESS_REQUIRED: "yes", PRIVATE_ACCESS_TEAM_DOMAIN: "", PRIVATE_ACCESS_AUD: "" },
        "private",
      ),
      // The Access check and its helpers, in plain Node against the wasm module built for Node (`npm run build:wasm`).
      {
        extends: true as const,
        test: { name: "access", environment: "node", include: ["test/access*.test.ts"], testTimeout: 20_000 },
      },
    ],
  },
});
