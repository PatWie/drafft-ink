import type { Room } from "./room";

/** A variable of `wrangler.toml`: a string, or a number or boolean when written as one in TOML. */
export type Var = string | number | boolean | undefined;

/**
 * The bindings and variables of the Worker (docs/design/collab-e2e.md, section 19.8). The relay reads only
 * these. `ROOM_TTL_SECS` unset means that rooms never expire.
 */
export interface Env {
  /** The rooms: one Durable Object per room id. */
  ROOM: DurableObjectNamespace<Room>;
  /** The web app (static assets). Requests for it never reach the Worker. */
  ASSETS: Fetcher;
  /** `demo` or `private`: shown by `/api/status`. */
  ENVIRONMENT?: Var;
  ROOM_TTL_SECS?: Var;
  ROOM_LEAVE_GRACE_SECS?: Var;
  /** `do` (pictures in the room's database) or `r2` (phase 3P). */
  BLOB_STORE?: Var;
  ALLOW_DELETE_ROOM?: Var;
  MAX_ROOM_STATE_BYTES?: Var;
  MAX_IMAGE_BYTES?: Var;
  MAX_ROOM_IMAGE_BYTES?: Var;
  MAX_ROOM_IMAGES?: Var;
  MAX_PEERS_PER_ROOM?: Var;
  ROOM_PRESENCE_BUDGET?: Var;
  /** Comma separated origins that may open a WebSocket from a browser. Empty: the page's own origin only. */
  ALLOWED_ORIGINS?: Var;
  ACCESS_REQUIRED?: Var;
  ACCESS_TEAM_DOMAIN?: Var;
  ACCESS_AUD?: Var;
}

/** The variables that the Rust core reads (`src/config.rs`). */
const RELAY_VARS = [
  "ROOM_TTL_SECS",
  "ROOM_LEAVE_GRACE_SECS",
  "ALLOW_DELETE_ROOM",
  "MAX_ROOM_STATE_BYTES",
  "MAX_IMAGE_BYTES",
  "MAX_ROOM_IMAGE_BYTES",
  "MAX_ROOM_IMAGES",
  "MAX_PEERS_PER_ROOM",
  "ROOM_PRESENCE_BUDGET",
] as const satisfies readonly (keyof Env)[];

/** The relay variables as the JSON object that the core parses and validates. */
export function relaySettings(env: Env): string {
  const picked: Record<string, Var> = {};
  for (const name of RELAY_VARS) picked[name] = env[name];
  return JSON.stringify(picked);
}

/** Whether a flag variable is switched on (`"true"` or `"1"`; unset and empty are off). */
export function isOn(value: Var): boolean {
  return value === true || value === "true" || value === "1" || value === 1;
}

/** What is wrong with the deployment's own variables (those that the Rust core does not read), or `null`. */
export function deploymentProblem(env: Env): string | null {
  const store = env.BLOB_STORE === undefined || env.BLOB_STORE === "" ? "do" : String(env.BLOB_STORE);
  if (store === "r2") return "BLOB_STORE=r2 is not built into this version yet (phase 3P). Use BLOB_STORE=do.";
  if (store !== "do") return `BLOB_STORE must be "do" or "r2", not "${store}".`;
  return null;
}
