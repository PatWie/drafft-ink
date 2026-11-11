/** Starts `drafftink-server` on a free port, serving `dist/` and `/ws` from one origin. */
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import net from "node:net";
import { DIST, SERVER_BIN } from "./env.ts";

/** A running server. */
export interface RunningServer {
  port: number;
  /** `http://127.0.0.1:<port>` */
  origin: string;
  /** The same server by another host name, so that the browser keeps a separate IndexedDB for it. */
  altOrigin: string;
  stop(): Promise<void>;
}

/** A free TCP port on the loopback interface. */
export function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.once("error", reject);
    s.listen(0, "127.0.0.1", () => {
      const addr = s.address();
      s.close(() => (addr && typeof addr === "object" ? resolve(addr.port) : reject(new Error("no port"))));
    });
  });
}

async function waitHealthy(origin: string, child: ChildProcess): Promise<void> {
  for (let i = 0; i < 200; i++) {
    if (child.exitCode !== null) throw new Error(`drafftink-server exited with ${child.exitCode}`);
    try {
      const r = await fetch(`${origin}/health`);
      if (r.ok) return;
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error("drafftink-server did not answer /health within 10 s");
}

/** Starts the server with an in-memory room store. `port` is chosen when omitted. With `authToken` a join must carry it. */
export async function startServer(port?: number, opts: { authToken?: string } = {}): Promise<RunningServer> {
  if (!existsSync(SERVER_BIN)) throw new Error(`${SERVER_BIN} not found. Build it: cargo build --release -p drafftink-server`);
  if (!existsSync(`${DIST}/index.html`)) throw new Error(`${DIST}/index.html not found. Build the web app: scripts/build-web.sh`);
  const chosen = port ?? (await freePort());
  // The token goes by the environment, not the command line, which other users can read.
  const env = opts.authToken ? { ...process.env, DRAFFTINK_AUTH_TOKENS: opts.authToken } : process.env;
  const child = spawn(SERVER_BIN, ["--web-dir", DIST, "--bind", `127.0.0.1:${chosen}`, "--storage", "memory"], { stdio: "ignore", env });
  const origin = `http://127.0.0.1:${chosen}`;
  await waitHealthy(origin, child);
  return {
    port: chosen,
    origin,
    altOrigin: `http://localhost:${chosen}`,
    async stop() {
      if (child.exitCode !== null) return;
      const exited = new Promise<void>((r) => child.once("exit", () => r()));
      child.kill("SIGTERM");
      await exited;
    },
  };
}
