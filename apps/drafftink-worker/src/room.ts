import { DurableObject } from "cloudflare:workers";

import { DatabaseBlobs, type BlobBackend } from "./blobs";
import { applyEffects } from "./effects";
import type { Env } from "./env";
import { relaySettings } from "./env";
import { DAILY_LIMIT_HEADER, dailyLimit, failure } from "./http";
import { failureOf } from "./host-error";
import { imageResponse, readLimited } from "./image-http";
import { parseRoute } from "./routes";
import { ensureSchema, SqlRoomStore } from "./sql-store";
import { legalCloseCode, parseAttachment, Sockets, type Attachment } from "./sockets";
import { ImageCall, RoomCore, sealedImageLimit, nextUtcMidnightMs, type Effects } from "./wasm";
import { WakeScheduler } from "./wake";

/** A `webSocketMessage` rewrites the attachment at most this often, to keep `lastMessageAt` roughly current. */
const ATTACHMENT_REFRESH_MS = 5_000;
/** After a failed alarm, look again after this long. */
const ALARM_RETRY_MS = 5 * 60_000;
/** Close code for a socket whose frame made the room fail. */
const CLOSE_INTERNAL_ERROR = 1011;

/**
 * One room: a SQLite-backed Durable Object that owns the sockets, the clock and the storage calls, and hands every
 * decision to the Rust core (`RoomCore`). The core is rebuilt on every wake, with the sockets that survived the
 * hibernation restored from their attachments. See `README.md` for the design notes.
 */
export class Room extends DurableObject<Env> {
  #core: RoomCore | null = null;
  readonly #sockets = new Sockets();
  readonly #scheduler: WakeScheduler;
  readonly #blobs: BlobBackend;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // The runtime answers the client's keepalive without waking the object (and without a request charge).
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    this.#blobs = new DatabaseBlobs(() => ctx.storage.sql);
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
    const reply = core.image(now, call, request.headers.get("authorization"), name ?? undefined, body);
    try {
      if (reply.status === 201) await this.#blobs.settle();
      await this.#scheduler.follow(reply.wakeAt);
      return imageResponse(reply, head);
    } finally {
      reply.free();
    }
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
    try {
      const core = await this.#wake();
      const conn = this.#sockets.conn(ws);
      if (conn === undefined) return;
      await work(core, conn, this.now());
    } catch (error) {
      console.error("room handler failed", error);
      // The core may be in an unknown state: rebuild it from storage and the sockets on the next call.
      this.#core = null;
      try {
        ws.close(CLOSE_INTERNAL_ERROR, "");
      } catch {
        // Already closed.
      }
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
      const answered = this.ctx.getWebSocketAutoResponseTimestamp(ws);
      if (answered !== null) core.heard(conn, answered.getTime());
    }
  }

  // ---- the core ---------------------------------------------------------------------------------------------

  /** The core of this room, built from storage and the open sockets when there is none (after a wake). */
  async #wake(): Promise<RoomCore> {
    if (this.#core !== null) return this.#core;
    const roomId = this.ctx.id.name;
    if (roomId === undefined) throw new Error("the room has no name: it must be reached with idFromName");
    // The tables are made on every wake: the database may have been wiped since the last one.
    ensureSchema(this.ctx.storage.sql);
    const core = new RoomCore(roomId, relaySettings(this.env), new SqlRoomStore(this.ctx.storage), this.#blobs);
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
      const answered = this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() ?? 0;
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

  /** The room is gone: no alarm, no rows, no sockets, and no memory of it. */
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
    try {
      await this.#blobs.purge();
      // `deleteAll` also clears the alarm on a compatibility date of 2026-02-24 or later. The explicit call keeps this
      // code independent of the date.
      await this.ctx.storage.deleteAlarm();
      await this.ctx.storage.deleteAll();
    } catch (error) {
      console.error("room could not be deleted", error);
      // A fresh core finds the room still past its deadline with nobody in it, and deletes it again.
      await this.#scheduler.retryLater(ALARM_RETRY_MS);
    }
  }
}
