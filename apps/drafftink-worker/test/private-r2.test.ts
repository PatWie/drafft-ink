import { env } from "cloudflare:workers";
import { runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";

import { Client, ERROR, MEMBER, ROOM, TAG, errorCode, fixtures, joined, limitsOf, roomStub, sleep, unhex } from "./helpers";
import { r2Keys, resetAll, signedClient, signedRequest, waitFor } from "./private-helpers";
import { probe } from "./worker";

const NAME = fixtures.image.name;
const SEALED = unhex(fixtures.image.sealedHex);
const auth = { authorization: MEMBER };

/** The key of the fixture room's picture in R2: `r/<room id hex>/<name hex>`. */
function keyOf(name: string): string {
  return `r/${hexOfBase64Url(ROOM)}/${hexOfBase64Url(name)}`;
}

function hexOfBase64Url(text: string): string {
  const raw = atob(text.replaceAll("-", "+").replaceAll("_", "/"));
  return [...raw].map((c) => c.charCodeAt(0).toString(16).padStart(2, "0")).join("");
}

/** A picture name that is valid and not the fixture's, number `n`. */
function nameN(n: number): string {
  const bytes = new Uint8Array(16).fill(n);
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function manifest(): Promise<{ name: string; size: number; data: unknown }[]> {
  return runInDurableObject(roomStub(), (_instance, state) =>
    state.storage.sql
      .exec("SELECT hex(name) AS name, size, data FROM room_images ORDER BY name")
      .toArray()
      .map((row) => ({ name: String(row["name"]).toLowerCase(), size: Number(row["size"]), data: row["data"] })),
  );
}

function tableRows(): Promise<number> {
  return runInDurableObject(roomStub(), (_instance, state) => {
    const sql = state.storage.sql;
    const tables = sql.exec("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_cf_%'").toArray();
    return tables.reduce((sum, row) => sum + Number(sql.exec(`SELECT count(*) AS n FROM "${String(row["name"])}"`).one()["n"]), 0);
  });
}

async function createRoom(): Promise<Client> {
  const client = await signedClient();
  client.sendNamed("joinCreateA");
  await client.expect(TAG.joined);
  return client;
}

afterEach(resetAll);

describe("pictures in R2", () => {
  it("an upload writes a manifest row with the name and the size only, and the sealed bytes to R2", async () => {
    const a = await createRoom();
    const put = await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: auth, body: SEALED });
    expect(put.status).toBe(201);
    expect(await put.json()).toEqual({ room_images: 1, room_image_bytes: SEALED.byteLength });

    expect(await manifest()).toEqual([{ name: hexOfBase64Url(NAME), size: SEALED.byteLength, data: null }]);
    expect(await r2Keys()).toEqual([keyOf(NAME)]);
    const stored = await env.BLOBS.get(keyOf(NAME));
    expect(new Uint8Array((await stored?.arrayBuffer()) ?? new ArrayBuffer(0))).toEqual(SEALED);
    a.close();
  });

  it("the list and a HEAD come from the manifest and read no R2 object", async () => {
    const a = await createRoom();
    await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: auth, body: SEALED });
    // Take the bytes away: the manifest alone must still answer.
    await env.BLOBS.delete(keyOf(NAME));
    const list = await signedRequest(`/r/${ROOM}/f`, { headers: auth });
    expect(await list.json()).toEqual([{ name: NAME, size: SEALED.byteLength }]);
    const head = await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "HEAD", headers: auth });
    expect(head.status).toBe(200);
    expect(head.headers.get("content-length")).toBe(String(SEALED.byteLength));
    const nothing = await signedRequest(`/r/${ROOM}/f/${nameN(9)}`, { method: "HEAD", headers: auth });
    expect(nothing.status).toBe(404);
    a.close();
  });

  it("a download is the Worker's own: it reads R2 and wakes no object", async () => {
    const a = await createRoom();
    await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: auth, body: SEALED });
    a.close();
    await a.untilClosed();
    await sleep(50);
    const calls = probe().objectCalls;
    const get = await signedRequest(`/r/${ROOM}/f/${NAME}`, { headers: auth });
    expect(get.status).toBe(200);
    expect(get.headers.get("content-type")).toBe("application/octet-stream");
    expect(get.headers.get("content-length")).toBe(String(SEALED.byteLength));
    expect(new Uint8Array(await get.arrayBuffer())).toEqual(SEALED);
    const missing = await signedRequest(`/r/${ROOM}/f/${nameN(3)}`, { headers: auth });
    expect(missing.status).toBe(404);
    expect(probe().objectCalls, "no object call for two downloads").toBe(calls);
    // The membership check comes first: a wrong token reads nothing.
    const stranger = await signedRequest(`/r/${ROOM}/f/${NAME}`, { headers: { authorization: "Drafft-Member AAAA" } });
    expect(stranger.status).toBe(401);
    expect(probe().objectCalls).toBe(calls);
  });

  it("a failed R2 write leaves no manifest row, answers 503, and a failed overwrite keeps the old row", async () => {
    const a = await createRoom();
    probe().failR2.put = new Error("put: Internal error (10001)");
    const failed = await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: auth, body: SEALED });
    expect(failed.status).toBe(503);
    expect(((await failed.json()) as { error: string }).error).toBe("unavailable");
    expect(await manifest(), "no manifest row without the bytes").toEqual([]);
    expect(await r2Keys()).toEqual([]);

    probe().failR2.put = null;
    expect((await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: auth, body: SEALED })).status).toBe(201);
    const longer = new Uint8Array([...SEALED, ...new Uint8Array(40)]);
    probe().failR2.put = new Error("put: Internal error (10001)");
    expect((await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: auth, body: longer })).status).toBe(503);
    expect((await manifest()).map((row) => row.size), "the old size is back").toEqual([SEALED.byteLength]);
    const list = await signedRequest(`/r/${ROOM}/f`, { headers: auth });
    expect(await list.json()).toEqual([{ name: NAME, size: SEALED.byteLength }]);
    a.close();
  });

  it("R2's limit of one write a second per key becomes a 429 rate_limited with Retry-After", async () => {
    const a = await createRoom();
    probe().failR2.put = new Error("put: Reduce your concurrent request rate for the same object. (10058)");
    const response = await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: auth, body: SEALED });
    expect(response.status).toBe(429);
    expect(response.headers.get("retry-after")).toBe("1");
    expect(((await response.json()) as { error: string }).error).toBe("rate_limited");
    expect(await manifest()).toEqual([]);
    a.close();
  });

  it("the quotas come from the manifest: the eighth picture of seven is refused, and nothing is written", async () => {
    const a = await createRoom();
    for (let n = 1; n <= 7; n++) {
      expect((await signedRequest(`/r/${ROOM}/f/${nameN(n)}`, { method: "PUT", headers: auth, body: SEALED })).status, `picture ${n}`).toBe(201);
    }
    const eighth = await signedRequest(`/r/${ROOM}/f/${nameN(8)}`, { method: "PUT", headers: auth, body: SEALED });
    expect(eighth.status).toBe(507);
    expect(((await eighth.json()) as { error: string }).error).toBe("room_images_full");
    expect((await r2Keys()).length).toBe(7);
    // The same name again is a replacement, which is allowed.
    expect((await signedRequest(`/r/${ROOM}/f/${nameN(7)}`, { method: "PUT", headers: auth, body: SEALED })).status).toBe(201);
    a.close();
  });

  it("a picture over the environment's own limit (2 MiB here) is refused before the object", async () => {
    const calls = probe().objectCalls;
    const response = await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: auth, body: new Uint8Array(2 * 1024 * 1024 + 79) });
    expect(response.status).toBe(413);
    expect(probe().objectCalls).toBe(calls);
  });
});

describe("the limits of the environment", () => {
  it("Joined announces the private environment's numbers", async () => {
    const a = await createRoom();
    const frame = a.frames.find((f) => f[0] === TAG.joined);
    expect(frame).toBeDefined();
    const limits = limitsOf(frame ?? new Uint8Array(40));
    expect(limits.maxImageBytes).toBe(2 * 1024 * 1024);
    expect(limits.maxRoomImageBytes).toBe(10n * 1024n * 1024n);
    expect(limits.maxRoomImages).toBe(7);
    expect(joined(frame ?? new Uint8Array(40)).expiresAtMs, "the private room never expires").toBe(0n);
    a.close();
  });
});

describe("deleting a room", () => {
  /** The objects of another room that must survive, put straight into R2. */
  async function otherRoomsObjects(): Promise<string[]> {
    const keys = [`r/${"ab".repeat(16)}/${"01".repeat(16)}`, `r/${"ab".repeat(16)}/${"02".repeat(16)}`, `r/${"cd".repeat(16)}/${"01".repeat(16)}`];
    for (const key of keys) await env.BLOBS.put(key, new Uint8Array([7]));
    return keys;
  }

  it("DeleteRoom removes the rows, the alarm and every object under the prefix, in pages, and nothing else", async () => {
    probe().r2PageSize = 3;
    const survivors = await otherRoomsObjects();
    const a = await createRoom();
    const b = await signedClient();
    b.sendNamed("joinOpenB");
    await b.expect(TAG.joined);
    for (let n = 1; n <= 7; n++) {
      expect((await signedRequest(`/r/${ROOM}/f/${nameN(n)}`, { method: "PUT", headers: auth, body: SEALED })).status).toBe(201);
    }
    expect((await r2Keys()).length).toBe(7 + 3);

    a.sendNamed("deleteRoom");
    await a.untilClosed();
    await b.untilClosed();
    await waitFor(async () => (await r2Keys()).length === survivors.length, "the purge");
    await waitFor(async () => (await tableRows()) === 0, "the rows to go");
    expect(await r2Keys(), "only the other rooms' objects are left").toEqual(survivors.sort());
    const dump = await runInDurableObject(roomStub(), async (_instance, state) => ({
      kv: (await state.storage.list()).size,
      alarm: await state.storage.getAlarm(),
      tables: state.storage.sql.exec("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'room_%'").toArray().length,
    }));
    expect(dump).toEqual({ kv: 0, alarm: null, tables: 0 });

    const again = await signedClient();
    again.sendNamed("joinOpenA");
    expect(errorCode(await again.expect(TAG.error))).toBe(ERROR.roomMissing);
  });

  it("when R2 cannot be purged the rows and an alarm stay, and the next alarm finishes the job", async () => {
    const survivors = await otherRoomsObjects();
    const a = await createRoom();
    await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: auth, body: SEALED });
    probe().failR2.list = new Error("list: Internal error (10001)");

    a.sendNamed("deleteRoom");
    await a.untilClosed();
    await sleep(100);
    // Nothing was orphaned: the object is still known to the room that stays behind, with an alarm to retry.
    expect(await r2Keys()).toContain(keyOf(NAME));
    expect(await tableRows(), "the rows are still there").toBeGreaterThan(0);
    const alarm = await runInDurableObject(roomStub(), (_instance, state) => state.storage.getAlarm());
    expect(alarm).not.toBeNull();
    expect(Math.abs((alarm ?? 0) - (Date.now() + 30_000))).toBeLessThan(5_000);
    // Meanwhile the room is not served as if nothing had happened.
    const meanwhile = await signedRequest(`/r/${ROOM}/ws`, { headers: { upgrade: "websocket" } });
    expect(meanwhile.status, "no socket is accepted for a room that is being deleted").toBe(503);

    probe().failR2.list = null;
    expect(await runDurableObjectAlarm(roomStub())).toBe(true);
    expect(await r2Keys()).toEqual(survivors.sort());
    const after = await runInDurableObject(roomStub(), async (_instance, state) => ({
      alarm: await state.storage.getAlarm(),
      kv: (await state.storage.list()).size,
    }));
    expect(after).toEqual({ alarm: null, kv: 0 });
    expect(await tableRows()).toBe(0);
  });

  it("when the delete call fails half way the next try removes the rest", async () => {
    probe().r2PageSize = 2;
    const a = await createRoom();
    for (let n = 1; n <= 5; n++) await signedRequest(`/r/${ROOM}/f/${nameN(n)}`, { method: "PUT", headers: auth, body: SEALED });
    probe().failR2.delete = new Error("delete: Internal error (10001)");
    a.sendNamed("deleteRoom");
    await a.untilClosed();
    await sleep(100);
    expect((await r2Keys()).length).toBe(5);
    probe().failR2.delete = null;
    await runDurableObjectAlarm(roomStub());
    expect(await r2Keys()).toEqual([]);
  });
});
