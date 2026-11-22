import { DurableObject } from "cloudflare:workers";

import { DatabaseBlobs, R2Blobs, type BlobBackend } from "./blobs";
import { applyEffects } from "./effects";
import { blobStore, relaySettings, type Env } from "./env";
import { DAILY_LIMIT_HEADER, dailyLimit, failure, json } from "./http";
import { failureOf, HostFailure } from "./host-error";
import { imageResponse, readLimited } from "./image-http";
import { parseRoute } from "./routes";
import { ensureSchema, isDeleting, markDeleting, SqlRoomStore } from "./sql-store";
import { legalCloseCode, parseAttachment, Sockets, type Attachment } from "./sockets";
import { dailyLimitFrame, ImageCall, nextUtcMidnightMs, RoomCore, sealedImageLimit, type Effects, type ImageReply, type SqlHost } from "./wasm";
import { WakeScheduler } from "./wake";

/** A `webSocketMessage` rewrites the attachment at most this often, to keep `lastMessageAt` roughly current. */
const ATTACHMENT_REFRESH_MS = 5_000;
/** After a failed alarm, look again after this long. */
const ALARM_RETRY_MS = 5 * 60_000;
/** After a deletion that could not be finished (R2 did not answer), look again after this long. */
const DELETE_RETRY_MS = 30_000;
/** Pictures that one R2 listing returns while a room is purged. */
const R2_PAGE_SIZE = 1000;
/** Close code for a socket whose frame made the room fail. */
const CLOSE_INTERNAL_ERROR = 1011;
/** Close code for "try again later": the host's daily limit is used up. */
const CLOSE_TRY_AGAIN_LATER = 1013;

/**
 * One room: a SQLite-backed Durable Object that owns the sockets, the clock and the storage calls, and hands every
 * decision to the Rust core (`RoomCore`). The core is rebuilt on every wake, with the sockets that survived the
 * hibernation restored from their attachments. See `README.md` for the design notes.
 */
export class Room extends DurableObject<Env> {
  #core: RoomCore | null = null;
  readonly #sockets = new Sockets();
  /** Sockets that the room closed itself when it was deleted: their close events must not wake the room again. */
  readonly #closedByDeletion = new WeakSet<WebSocket>();
  readonly #scheduler: WakeScheduler;
  readonly #blobs: BlobBackend;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // The runtime answers the client's keepalive without waking the object (and without a request charge).
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    this.#blobs = this.#makeBlobs();
    this.#scheduler = new WakeScheduler(
      ctx.storage,
      () => this.now(),
      () => void this.#tick().catch((error: unknown) => console.error("room timer failed", error)),
    );
  }

  /** The time in milliseconds since the epoch. The one clock of the room; tests replace it. */
  protected now(): number {
    return Date.now();
  }

  /**
   * When the runtime last answered a `ping` of `ws`, in the room's clock, or `null` before the first one. A client's
   * `ping` never reaches the room, so this is how the core learns that a socket is alive. Tests move it with `now()`.
   */
  protected heardAt(ws: WebSocket): number | null {
    return this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() ?? null;
  }

  /** Creates the tables that are missing. A seam for tests, which make it fail. */
  protected prepareStorage(): void {
    ensureSchema(this.ctx.storage.sql);
  }

  /** The R2 bucket of the private environment. A seam for tests, which wrap it to make R2 fail. */
  protected bucket(): R2Bucket {
    const bucket = this.env.BLOBS;
    if (bucket === undefined) throw new HostFailure("storage", "the R2 binding BLOBS is missing");
    return bucket;
  }

  /** How many objects one R2 listing returns while a room is purged. Tests make it small to see the paging. */
  protected r2PageSize(): number {
    return R2_PAGE_SIZE;
  }

  #makeBlobs(): BlobBackend {
    if (blobStore(this.env) !== "r2") return new DatabaseBlobs(() => this.ctx.storage.sql);
    const room = this.ctx.id.name;
    if (room === undefined) throw new Error("the room has no name: it must be reached with idFromName");
    return new R2Blobs(() => this.ctx.storage.sql, () => this.bucket(), room, this.r2PageSize());
  }

  /** The rows of this room. A seam for tests, which wrap it to make the storage fail. */
  protected sqlHost(): SqlHost {
    return new SqlRoomStore(this.ctx.storage);
  }

  // ---- requests ---------------------------------------------------------------------------------------------

  override async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const route = parseRoute(url.pathname);
    try {
      switch (route.kind) {
        case "socket":
          return await this.#upgrade(request);
        case "images":
          return await this.#image(request, ImageCall.List, null);
        case "image":
          return await this.#image(request, request.method === "PUT" ? ImageCall.Put : ImageCall.Get, route.name);
        case "status":
        case "unknown":
          return failure(404, "not_found", "There is nothing at this address.");
      }
    } catch (error) {
      return this.#failed(error);
    }
  }

  /** What a request that threw gets: a used-up daily limit and a storage failure are `503`, a fault is `500`. */
  #failed(error: unknown): Response {
    console.error("room request failed", error);
    const failed = failureOf(error);
    if (failed.kind === "daily_limit") {
      const seconds = Math.ceil((nextUtcMidnightMs(this.now()) - this.now()) / 1000);
      const reply = dailyLimit(seconds);
      reply.headers.set(DAILY_LIMIT_HEADER, String(seconds));
      return reply;
    }
    return failure(503, "unavailable", "The server cannot read or write this room right now. Trying again soon.", {
      "retry-after": "5",
    });
  }

  async #upgrade(request: Request): Promise<Response> {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return failure(426, "upgrade_required", "This address speaks WebSocket. Open it with a room link.");
    }
    const core = await this.#wake();
    const now = this.now();
    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    const conn = this.#sockets.next();
    this.ctx.acceptWebSocket(server);
    const attachment: Attachment = { v: 1, conn, connectedAt: now, lastMessageAt: now };
    server.serializeAttachment(attachment);
    this.#sockets.add(conn, server);
    await this.#apply(core.connect(conn, now));
    return new Response(null, { status: 101, webSocket: client });
  }

  async #image(request: Request, call: ImageCall, name: string | null): Promise<Response> {
    const head = request.method === "HEAD";
    const core = await this.#wake();
    const now = this.now();
    let body = new Uint8Array(0);
    if (call === ImageCall.Put) {
      const limit = sealedImageLimit(relaySettings(this.env));
      const read = await readLimited(request, limit);
      if (read === null) {
        return failure(413, "image_too_large", "This picture is larger than a shared room accepts.");
      }
      body = read;
    }
    // A HEAD is answered from the manifest in both modes (`List`): no bytes are read, and in R2 mode there are none here.
    const asked = head ? ImageCall.List : call;
    const reply = core.image(now, asked, request.headers.get("authorization"), head ? undefined : (name ?? undefined), body);
    try {
      // No await between the core's answer and `settle`: the pending write is taken synchronously.
      const settled = await this.#blobs.settle();
      await this.#scheduler.follow(reply.wakeAt);
      if (!settled.ok) return this.#writeFailed(settled.reason);
      return head && name !== null ? headOf(reply, name) : imageResponse(reply, head);
    } finally {
      reply.free();
    }
  }

  /** A picture that the core accepted but R2 refused: the manifest is rolled back, and the client is told when to retry. */
  #writeFailed(reason: "rate_limited" | "unavailable"): Response {
    if (reason === "rate_limited") {
      return json(429, { error: "rate_limited", message: "Too many pictures at once. Trying again in a moment." }, { "retry-after": "1" });
    }
    return failure(503, "unavailable", "The server cannot store pictures right now. Trying again soon.", { "retry-after": "5" });
  }

  // ---- sockets ----------------------------------------------------------------------------------------------

  override async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    await this.#guarded(ws, async (core, conn, now) => {
      const effects =
        typeof message === "string" ? core.onText(conn, now, message) : core.onFrame(conn, now, new Uint8Array(message));
      this.#rememberMember(core, ws, conn, now);
      await this.#apply(effects);
    });
  }

  override async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    await this.#guarded(ws, async (core, conn, now) => {
      this.#sockets.remove(conn);
      await this.#apply(core.onClosed(conn, now));
    });
    // Completes the closing handshake. Only a legal code may be sent: the peer may have reported 1005 or 1006.
    try {
      ws.close(legalCloseCode(code) ? code : 1000, "");
    } catch {
      // Already closed.
    }
  }

  override async webSocketError(ws: WebSocket, error: unknown): Promise<void> {
    console.error("room socket error", error);
    await this.webSocketClose(ws, 1011);
  }

  /** Runs `work` for the connection of `ws`. A failure closes that socket and forgets the core; it never escapes. */
  async #guarded(ws: WebSocket, work: (core: RoomCore, conn: number, now: number) => Promise<void>): Promise<void> {
    if (this.#closedByDeletion.has(ws)) return;
    try {
      const core = await this.#wake();
      const conn = this.#sockets.conn(ws);
      if (conn === undefined) return;
      await work(core, conn, this.now());
    } catch (error) {
      console.error("room handler failed", error);
      // The core may be in an unknown state: rebuild it from storage and the sockets on the next call.
      this.#core = null;
      this.#refuse(ws, error);
    }
  }

  /** Ends a socket whose handler failed: a used-up daily limit is said with a frame and 1013, anything else is 1011. */
  #refuse(ws: WebSocket, error: unknown): void {
    const quota = failureOf(error).kind === "daily_limit";
    try {
      if (quota) ws.send(dailyLimitFrame(this.now()));
      ws.close(quota ? CLOSE_TRY_AGAIN_LATER : CLOSE_INTERNAL_ERROR, "");
    } catch {
      // Already closed.
    }
  }

  /** After a frame: keep the client id and join order of a member, and the time of the last frame, in the attachment. */
  #rememberMember(core: RoomCore, ws: WebSocket, conn: number, now: number): void {
    const attachment = parseAttachment(ws.deserializeAttachment());
    if (attachment === null) return;
    let changed = false;
    if (attachment.client === undefined) {
      const client = core.memberClient(conn);
      const order = core.memberOrder(conn);
      if (client !== undefined && order !== undefined) {
        attachment.client = client;
        attachment.order = order;
        changed = true;
      }
    }
    if (now - attachment.lastMessageAt > ATTACHMENT_REFRESH_MS) {
      attachment.lastMessageAt = now;
      changed = true;
    }
    if (changed) ws.serializeAttachment(attachment);
  }

  // ---- time -------------------------------------------------------------------------------------------------

  override async alarm(): Promise<void> {
    this.#scheduler.alarmFired();
    try {
      await this.#tick();
    } catch (error) {
      // The alarm must not throw: the platform would retry it six times. Look again later instead.
      console.error("room alarm failed", error);
      this.#core = null;
      try {
        await this.#scheduler.retryLater(ALARM_RETRY_MS);
      } catch (retryError) {
        console.error("room alarm could not be set again", retryError);
      }
    }
  }

  /** The time of a wake has come (timer or alarm): feed the liveness of the sockets, then let the core look. */
  async #tick(): Promise<void> {
    const core = await this.#wake();
    this.#feedLiveness(core);
    await this.#apply(core.onWake(this.now()));
  }

  /** Tells the core when the runtime last answered a `ping` of each socket: it never reaches the core itself. */
  #feedLiveness(core: RoomCore): void {
    for (const [conn, ws] of this.#sockets.entries()) {
      const answered = this.heardAt(ws);
      if (answered !== null) core.heard(conn, answered);
    }
  }

  // ---- the core ---------------------------------------------------------------------------------------------

  /** The core of this room, built from storage and the open sockets when there is none (after a wake). */
  async #wake(): Promise<RoomCore> {
    if (this.#core !== null) return this.#core;
    const roomId = this.ctx.id.name;
    if (roomId === undefined) throw new Error("the room has no name: it must be reached with idFromName");
    // The tables are made on every wake: the database may have been wiped since the last one.
    this.prepareStorage();
    if (isDeleting(this.ctx.storage.sql)) {
      // A deletion was started and R2 did not finish it. Nothing else may happen to this room before it does.
      if (!(await this.#finishDeletion())) throw new HostFailure("storage", "the room is being deleted");
      this.prepareStorage();
    }
    const core = new RoomCore(roomId, relaySettings(this.env), this.sqlHost(), this.#blobs);
    this.#core = core;
    this.#sockets.clear();
    const now = this.now();
    for (const ws of this.ctx.getWebSockets()) {
      const attachment = parseAttachment(ws.deserializeAttachment());
      if (attachment === null) {
        ws.close(CLOSE_INTERNAL_ERROR, "");
        continue;
      }
      this.#sockets.add(attachment.conn, ws);
      const answered = this.heardAt(ws) ?? 0;
      const lastHeard = Math.max(attachment.lastMessageAt, attachment.connectedAt, answered);
      const restored =
        attachment.client !== undefined && attachment.order !== undefined
          ? core.restoreMember(attachment.conn, attachment.client, attachment.order, lastHeard, now)
          : core.restoreWaiting(attachment.conn, attachment.connectedAt, now);
      restored.free();
    }
    // Timers do not survive a hibernation: the core says which are needed.
    await this.#apply(core.onWake(now));
    return core;
  }

  /** Carries out what the core decided: frames and closes now, then the timer or alarm, or the deletion. */
  async #apply(effects: Effects): Promise<void> {
    try {
      // The core closes every socket of a deleted room. Their close events must not rebuild the room.
      if (effects.deleted) for (const [, ws] of this.#sockets.entries()) this.#closedByDeletion.add(ws);
      applyEffects(effects, this.#sockets);
      if (effects.deleted) {
        await this.#destroy();
      } else {
        await this.#scheduler.follow(effects.wakeAt);
      }
    } finally {
      effects.free();
    }
  }

  /** The room is gone: no sockets, no memory of it, then the R2 objects, the alarm and the rows. */
  async #destroy(): Promise<void> {
    this.#scheduler.reset();
    this.#core = null;
    for (const [conn, ws] of [...this.#sockets.entries()]) {
      try {
        ws.close(1000, "");
      } catch {
        // Already closed.
      }
      this.#sockets.remove(conn);
    }
    await this.#finishDeletion();
  }

  /**
   * Cleans out what a deleted room left: first R2 (listing the prefix and deleting in pages), then the alarm, then every
   * row. The order matters: when R2 fails the rows and the alarm stay, a tombstone says why, and an alarm shortly after
   * tries again, so nothing in R2 is left without a room that remembers it. `deleteAlarm()` comes before
   * `deleteAll()` although `deleteAll()` also clears the alarm from compatibility date 2026-02-24 on.
   */
  async #finishDeletion(): Promise<boolean> {
    try {
      await this.#blobs.purge();
      await this.ctx.storage.deleteAlarm();
      await this.ctx.storage.deleteAll();
      this.#scheduler.reset();
      return true;
    } catch (error) {
      console.error("room could not be deleted yet", error);
      try {
        ensureSchema(this.ctx.storage.sql);
        markDeleting(this.ctx.storage.sql);
        await this.#scheduler.retryLater(DELETE_RETRY_MS);
      } catch (markError) {
        console.error("the unfinished deletion could not be noted", markError);
      }
      return false;
    }
  }
}

/** The reply to a `HEAD`: the size from the manifest listing, without a body. */
function headOf(reply: ImageReply, name: string): Response {
  if (reply.status !== 200) return imageResponse(reply, true);
  const listing = JSON.parse(new TextDecoder().decode(reply.body())) as { name: string; size: number }[];
  const found = listing.find((entry) => entry.name === name);
  if (found === undefined) return failure(404, "image_missing", "The room has no picture with this name.");
  return new Response(null, { status: 200, headers: { "content-type": "application/octet-stream", "content-length": String(found.size), "cache-control": "no-store" } });
}
