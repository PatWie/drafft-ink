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
  /** Cloudflare Access (src/access.ts). Anything but empty, `false` or `0` turns the check on: it fails closed. */
  ACCESS_REQUIRED?: Var;
  ACCESS_TEAM_DOMAIN?: Var;
  ACCESS_AUD?: Var;
  /** The host people open, for example `canvas.patwie.com`. A request on another host (workers.dev) needs a token too. */
  ACCESS_APP_HOSTNAME?: Var;
  /** Replaces the Access key set URL. For tests and local runs only: it is empty in a deployment. */
  ACCESS_CERTS_URL?: Var;
  /** The R2 bucket of the private environment (`BLOB_STORE=r2`). */
  BLOBS?: R2Bucket;
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

/**
 * Whether the Access check is required. It fails closed: only an unset or empty value, `false` and `0` switch it
 * off, so a typo (`"yes"`, `"True "`) keeps the instance locked and does not open it.
 */
export function accessRequired(value: Var): boolean {
  if (value === undefined || value === false || value === 0) return false;
  const text = String(value).trim().toLowerCase();
  return !(text === "" || text === "false" || text === "0");
}

/** A variable as text: unset is the empty string. */
export function text(value: Var): string {
  return value === undefined ? "" : String(value).trim();
}

/** Where the sealed pictures are kept. */
export type BlobStoreKind = "do" | "r2";

/** The configured picture store, or the plain-words problem with the setting. */
export function blobStore(env: Env): BlobStoreKind | { problem: string } {
  const value = text(env.BLOB_STORE) || "do";
  if (value === "do") return "do";
  if (value !== "r2") return { problem: `BLOB_STORE must be "do" or "r2", not "${value}".` };
  if (env.BLOBS === undefined) return { problem: 'BLOB_STORE=r2 needs an R2 binding named BLOBS (the "private" environment declares it).' };
  return "r2";
}

/** What is wrong with the deployment's own variables (those that the Rust core does not read), or `null`. */
export function deploymentProblem(env: Env): string | null {
  const store = blobStore(env);
  return typeof store === "string" ? null : store.problem;
}
