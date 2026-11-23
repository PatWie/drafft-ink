import { coreFromWasm, guard, type AccessEnv } from "./access";
import { accessRequired, blobStore, deploymentProblem, relaySettings, text, type Env } from "./env";
import { dailyLimit, DAILY_LIMIT_HEADER, failure, json } from "./http";
import { isQuotaError } from "./host-error";
import { originAllowed } from "./origin";
import { QuotaGate } from "./quota";
import { parseRoute, type Route } from "./routes";
import {
  access_check_claims,
  access_parse,
  access_refusal,
  checkMember,
  checkSettings,
  dailyLimitFrame,
  encryptedOnlyFrame,
  isImageName,
  isRoomId,
  MemberCheck,
  nextUtcMidnightMs,
  r2Key,
  sealedImageLimit,
} from "./wasm";

export { Room } from "./room";

/** A Durable Object call failed with a used-up daily limit: remembered per isolate until 00:00 UTC. */
const quota = new QuotaGate();

/** Settings that were checked already, by their JSON text. */
const checkedSettings = new Set<string>();

const CLOSE_TRY_AGAIN_LATER = 1013;
const CLOSE_NORMAL = 1000;

/** The Rust side of the Access check (claims and refusal texts); the signature is checked with Web Crypto. */
const accessCore = coreFromWasm({ access_parse, access_check_claims, access_refusal });

/** The settings the Access guard reads, with the fail-closed reading of `ACCESS_REQUIRED`. */
function accessEnv(env: Env): AccessEnv {
  return {
    ACCESS_REQUIRED: accessRequired(env.ACCESS_REQUIRED),
    ACCESS_TEAM_DOMAIN: text(env.ACCESS_TEAM_DOMAIN),
    ACCESS_AUD: text(env.ACCESS_AUD),
    ACCESS_CERTS_URL: text(env.ACCESS_CERTS_URL),
    ...(text(env.ACCESS_APP_HOSTNAME) === "" ? {} : { ACCESS_APP_HOSTNAME: text(env.ACCESS_APP_HOSTNAME) }),
  };
}

/**
 * The Worker in front of the rooms. It routes `/r/<id>/ws`, `/r/<id>/f` and `/r/<id>/f/<name>` to the Durable Object
 * of the room, and answers `/api/status` itself. What it can decide without an object it decides here, so a
 * request that would fail costs a Worker request and no object request: a bad room id or picture name, a missing
 * or foreign membership token, a picture that is too large, a disallowed origin, a spent daily limit. It hashes 32
 * bytes of token at most and never reads a body.
 *
 * A WebSocket upgrade cannot be membership-checked here: the proof is in the first frame, which only the object
 * receives, and a socket cannot be handed over once the Worker has accepted it. A stranger's upgrade therefore costs
 * one object request, and the core refuses him at `Join`. The picture routes do carry the proof in a header, and
 * those are checked here before any object is called.
 */
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    try {
      return await handle(request, env);
    } catch (error) {
      console.error("worker failed", error);
      return failure(500, "internal", "The server failed on this request. Trying again may help.");
    }
  },
} satisfies ExportedHandler<Env>;

async function handle(incoming: Request, env: Env): Promise<Response> {
  // Cloudflare Access is the first lock and this is the second (design 19.5). It runs before anything else, so a
  // request without a valid sign-in learns nothing, and neither a Durable Object nor R2 is ever called for it. What
  // goes on is the same request without Access headers and cookies: the relay never learns who connected.
  const checkedRequest = await guard(incoming, accessEnv(env), Date.now(), { core: accessCore });
  if (checkedRequest instanceof Response) return checkedRequest;
  const request = checkedRequest;
  const url = new URL(request.url);
  const route = parseRoute(url.pathname);
  const settings = relaySettings(env);
  const configProblem = checked(settings);
  if (configProblem !== null) return failure(500, "bad_config", configProblem);
  const deployProblem = deploymentProblem(env);
  if (deployProblem !== null) return failure(500, "bad_config", deployProblem);
  switch (route.kind) {
    case "status":
      return status(request, env);
    case "unknown":
      return failure(404, "not_found", "There is nothing at this address.");
    case "legacy_socket":
      return legacySocket(request, env, url);
    case "socket":
      return socket(request, env, url, route.room);
    case "images":
    case "image":
      return pictures(request, env, route, settings);
  }
}

/** `null` when the variables are fine, else the plain-words problem. Checked once per distinct set of values. */
function checked(settings: string): string | null {
  if (checkedSettings.has(settings)) return null;
  try {
    checkSettings(settings);
  } catch (error) {
    return error instanceof Error ? error.message : "The settings of this server are not valid.";
  }
  checkedSettings.add(settings);
  return null;
}

// ---- /api/status ------------------------------------------------------------------------------------------------

function status(request: Request, env: Env): Response {
  if (request.method !== "GET" && request.method !== "HEAD") return methodNotAllowed("GET, HEAD");
  const now = Date.now();
  const wait = quota.retryAfterSeconds(now);
  if (wait !== null) return dailyLimit(wait);
  return json(200, {
    status: "ok",
    env: typeof env.ENVIRONMENT === "string" ? env.ENVIRONMENT : "unknown",
    now,
    reset_at_ms: nextUtcMidnightMs(now),
  });
}

function methodNotAllowed(allow: string): Response {
  return failure(405, "method_not_allowed", `This address accepts ${allow}.`, { allow });
}

// ---- WebSocket ------------------------------------------------------------------------------------------------

async function socket(request: Request, env: Env, url: URL, room: string): Promise<Response> {
  if (request.method !== "GET") return methodNotAllowed("GET");
  if (!isRoomId(room)) return failure(404, "room_missing", "This room does not exist (any more).");
  if (!originAllowed(request.headers.get("origin"), url, typeof env.ALLOWED_ORIGINS === "string" ? env.ALLOWED_ORIGINS : "")) {
    return failure(403, "origin_not_allowed", "This page may not open rooms on this server.");
  }
  if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
    return failure(426, "upgrade_required", "This address speaks WebSocket. Open it with a room link.");
  }
  const wait = quota.retryAfterSeconds(Date.now());
  if (wait !== null) return refuseSocket();
  return callRoom(env, room, request);
}

/**
 * `/ws` is the plaintext (v1) route of the app. This server has no plaintext rooms, so the Worker answers the first
 * frame (the v1 `Join`) with the `UnsupportedVersion` error whose text says so, and closes. It is here and not in the
 * static assets, because the assets would answer the upgrade with the app's page and the app would wait for a
 * handshake that never comes. No object is called: it costs one Worker request.
 */
function legacySocket(request: Request, env: Env, url: URL): Response {
  if (request.method !== "GET") return methodNotAllowed("GET");
  if (!originAllowed(request.headers.get("origin"), url, typeof env.ALLOWED_ORIGINS === "string" ? env.ALLOWED_ORIGINS : "")) {
    return failure(403, "origin_not_allowed", "This page may not open rooms on this server.");
  }
  if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
    return failure(426, "upgrade_required", "This address speaks WebSocket. Open it with a room link.");
  }
  const pair = new WebSocketPair();
  const [client, server] = [pair[0], pair[1]];
  server.accept();
  server.addEventListener("message", () => {
    server.send(encryptedOnlyFrame());
    server.close(CLOSE_NORMAL, "encrypted rooms only");
  });
  return new Response(null, { status: 101, webSocket: client });
}

/** Accepts the upgrade only to say why and close with 1013 ("try again later"): the client shows the reset time. */
function refuseSocket(): Response {
  const pair = new WebSocketPair();
  const [client, server] = [pair[0], pair[1]];
  server.accept();
  server.send(dailyLimitFrame(Date.now()));
  server.close(CLOSE_TRY_AGAIN_LATER, "daily limit");
  return new Response(null, { status: 101, webSocket: client });
}

// ---- pictures ------------------------------------------------------------------------------------------------

async function pictures(request: Request, env: Env, route: Extract<Route, { kind: "images" | "image" }>, settings: string): Promise<Response> {
  const methods = route.kind === "images" ? ["GET"] : ["GET", "HEAD", "PUT"];
  if (!methods.includes(request.method)) return methodNotAllowed(methods.join(", "));
  // The same order as the other relay hosts: the room, the token, the name, the size.
  const member = checkMember(route.room, request.headers.get("authorization"));
  if (member === MemberCheck.BadRoom) return failure(404, "room_missing", "This room does not exist (any more).");
  if (member !== MemberCheck.Member) {
    return failure(401, "unauthorized", "This request does not carry the room's membership token.");
  }
  if (route.kind === "image" && !isImageName(route.name)) {
    return failure(400, "bad_request", "The request is not a sealed picture with a valid name.");
  }
  if (request.method === "PUT" && Number(request.headers.get("content-length") ?? "0") > sealedImageLimit(settings)) {
    return failure(413, "image_too_large", "This picture is larger than a shared room accepts.");
  }
  // In R2 mode a download is the Worker's own: it reads R2 after the membership check above and wakes no object.
  if (route.kind === "image" && request.method === "GET" && blobStore(env) === "r2") {
    return downloadFromR2(env, route.room, route.name);
  }
  const wait = quota.retryAfterSeconds(Date.now());
  if (wait !== null) return dailyLimit(wait);
  return callRoom(env, route.room, request);
}

/** Streams a sealed picture out of R2. The membership token was checked by the caller. */
async function downloadFromR2(env: Env, room: string, name: string): Promise<Response> {
  const key = r2Key(room, name);
  if (key === undefined || env.BLOBS === undefined) return failure(400, "bad_request", "The request is not a sealed picture with a valid name.");
  try {
    const object = await env.BLOBS.get(key);
    if (object === null) return failure(404, "image_missing", "The room has no picture with this name.");
    return new Response(object.body, {
      status: 200,
      headers: { "content-type": "application/octet-stream", "content-length": String(object.size), "cache-control": "no-store" },
    });
  } catch (error) {
    console.error("r2 read failed", error);
    return failure(503, "unavailable", "The server cannot read pictures right now. Trying again soon.", { "retry-after": "5" });
  }
}

// ---- the room ------------------------------------------------------------------------------------------------

/** Forwards the request to the room's object and notes a used-up daily limit that the object reports or throws. */
async function callRoom(env: Env, room: string, request: Request): Promise<Response> {
  try {
    const response = await env.ROOM.get(env.ROOM.idFromName(room)).fetch(request);
    if (response.headers.has(DAILY_LIMIT_HEADER)) noteQuota();
    return response;
  } catch (error) {
    console.error("room call failed", error);
    if (isQuotaError(error)) {
      noteQuota();
      return dailyLimit(quota.retryAfterSeconds(Date.now()) ?? 1);
    }
    return failure(503, "unavailable", "The server cannot reach this room right now. Trying again soon.", {
      "retry-after": "5",
    });
  }
}

function noteQuota(): void {
  quota.trip(nextUtcMidnightMs(Date.now()));
}
