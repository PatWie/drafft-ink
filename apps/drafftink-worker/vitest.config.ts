import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

const app = dirname(fileURLToPath(import.meta.url));

/** Renders the wrangler config of the Worker for the tests: the test entry, the demo environment's defaults. */
function config(name: string, env: Record<string, string>): string {
  const out = join(app, ".wrangler", "test", `${name}.toml`);
  mkdirSync(dirname(out), { recursive: true });
  execFileSync("bash", [join(app, "scripts", "render-config.sh"), "demo", out], {
    env: { ...process.env, MAIN: join(app, "test", "worker.ts"), ...env },
    stdio: "pipe",
  });
  return out;
}

/** One project is one set of variables: the room reads them when it wakes. */
function project(name: string, include: string[], vars: Record<string, string>) {
  return {
    extends: true as const,
    plugins: [
      cloudflareTest({
        wrangler: { configPath: config(name, vars) },
        // A real Worker with real bindings; nothing leaves the machine.
        remoteBindings: false,
      }),
    ],
    test: { name, include, testTimeout: 20_000 },
  };
}

export default defineConfig({
  test: {
    projects: [
      // Rooms as the demo runs them: a 48 hour deadline and a one second grace, so a test can wait for it.
      project("demo", ["test/routing.test.ts", "test/room.test.ts", "test/retention.test.ts", "test/canary.test.ts", "test/units.test.ts", "test/quota.test.ts", "test/quota-wake.test.ts"], {
        DEMO_ROOM_LEAVE_GRACE_SECS: "1",
        DEMO_ALLOWED_ORIGINS: "https://drafft.test",
      }),
      // No ROOM_TTL_SECS: no deadline and no alarm. Deleting a room is allowed, as on the private instance.
      project("no-expiry", ["test/no-expiry.test.ts"], {
        DEMO_ROOM_TTL_SECS: "",
        DEMO_ALLOW_DELETE_ROOM: "true",
      }),
    ],
  },
});
