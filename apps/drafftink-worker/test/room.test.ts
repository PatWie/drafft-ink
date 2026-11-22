import { abortAllDurableObjects, evictDurableObject, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";

import { Client, ERROR, MEMBER, ROOM, TAG, ack, errorCode, fixtures, joined, recordSeq, request, roomStub, sleep, unhex } from "./helpers";
import { probe } from "./worker";

const NAME = fixtures.image.name;
const SEALED = unhex(fixtures.image.sealedHex);

async function rowCounts(): Promise<Record<string, number>> {
  return runInDurableObject(roomStub(), (_instance, state) => {
    const counts: Record<string, number> = {};
    for (const table of ["room_meta", "room_snapshot", "room_segments", "room_images"]) {
      counts[table] = Number(state.storage.sql.exec(`SELECT count(*) AS n FROM ${table}`).one()["n"]);
    }
    return counts;
  });
}

/**
 * Empties the room's storage and drops the instance, so that the next request wakes a new one (as after a deletion).
 * `abortAllDurableObjects` is used and not a graceful eviction, which waits for the room's timers.
 */
async function wipe(): Promise<void> {
  await runInDurableObject(roomStub(), async (_instance, state) => {
    await state.storage.deleteAlarm();
    await state.storage.deleteAll();
  });
  await abortAllDurableObjects();
}

describe("one room, two people", () => {
  it("creates a room, sequences a record, fans it out and acknowledges it once it is stored", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    const first = joined(await a.expect(TAG.joined));
    expect(first.roomState).toBe(1);
    expect(first.headSeq).toBe(0n);
    expect(first.expiresAtMs).toBeGreaterThan(BigInt(Date.now()));

    const b = await Client.open();
    b.sendNamed("joinOpenB");
    expect(joined(await b.expect(TAG.joined)).roomState).toBe(0);
    await a.expect(TAG.peerJoined);

    a.sendNamed("appendA1");
    expect(recordSeq(await b.expect(TAG.record))).toBe(1n);
    expect(a.tags(), "the sender gets no echo").not.toContain(TAG.record);
    // The Ack waits for the flush: the record is stored when it arrives.
    const done = ack(await a.expect(TAG.ack));
    expect(done).toEqual({ ctr: 1n, seq: 1n });
    expect((await rowCounts())["room_segments"]).toBe(1);
    a.close();
    b.close();
    await b.expect(TAG.peerLeft).catch(() => undefined);
    await wipe();
  });

  it("answers ping with pong and does not run the room for it", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    const before = probe().socketMessages;
    for (let i = 0; i < 3; i++) a.ws.send("ping");
    await a.until(() => a.texts.length === 3, "three pongs");
    expect(a.texts).toEqual(["pong", "pong", "pong"]);
    expect(probe().socketMessages, "the runtime answers; the object's handler never runs").toBe(before);
    a.close();
    await wipe();
  });

  it("refuses a record from a room that was never joined, and text that is not a ping", async () => {
    const early = await Client.open();
    early.sendNamed("appendA1");
    expect(errorCode(await early.expect(TAG.error))).toBe(ERROR.notJoined);
    await early.untilClosed();

    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    a.ws.send("hello");
    expect(errorCode(await a.expect(TAG.error))).toBe(ERROR.malformed);
    await a.untilClosed();
    await wipe();
  });

  it("says RoomMissing for an open join of a room that does not exist", async () => {
    const a = await Client.open();
    a.sendNamed("joinOpenA");
    expect(errorCode(await a.expect(TAG.error))).toBe(ERROR.roomMissing);
    await a.untilClosed();
    expect(await rowCounts()).toEqual({ room_meta: 0, room_snapshot: 0, room_segments: 0, room_images: 0 });
  });

  it("closes a socket that the peer closes without a code, and tells the others", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    const b = await Client.open();
    b.sendNamed("joinOpenB");
    await b.expect(TAG.joined);
    // `close()` without a code reaches the room as 1005, which must not be echoed.
    a.ws.close();
    await b.expect(TAG.peerLeft);
    b.close();
    await wipe();
  });
});

describe("snapshots", () => {
  it("seeds an empty room, then swaps a snapshot in and drops the segments it covers", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    a.sendNamed("seedSnapshot");
    const seeded = await a.expect(TAG.snapshotResult);
    expect(seeded[1], "ok").toBe(1);
    expect((await rowCounts())["room_snapshot"]).toBe(1);
    a.close();
    await wipe();
  });

  it("compacts: after two records, a snapshot that covers them leaves no segment row", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    a.sendNamed("appendA1");
    a.sendNamed("appendA2");
    await a.until(() => a.tags().filter((t) => t === TAG.ack).length === 2, "two acks");
    expect((await rowCounts())["room_segments"]).toBeGreaterThanOrEqual(1);
    const mark = a.frames.length;
    a.sendNamed("snapshotCovers2");
    const result = await a.expect(TAG.snapshotResult, mark);
    expect(result[1]).toBe(1);
    expect(await rowCounts()).toMatchObject({ room_snapshot: 1, room_segments: 0 });
    // A joiner now gets the snapshot, and no record.
    const b = await Client.open();
    b.sendNamed("joinOpenB");
    const catchup = await b.expect(TAG.catchup);
    expect(catchup.length).toBeGreaterThan(100);
    a.close();
    b.close();
    await wipe();
  });
});

describe("hibernation", () => {
  it("rebuilds the core on wake: restored sockets keep their place, fan-out and acks go on", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    const b = await Client.open();
    b.sendNamed("joinOpenB");
    await b.expect(TAG.joined);
    a.sendNamed("appendA1");
    await a.expect(TAG.ack);
    await b.expect(TAG.record);

    const builtBefore = probe().constructed;
    await evictDurableObject(roomStub());
    // Pings are answered by the runtime while the object sleeps.
    a.ws.send("ping");
    await a.until(() => a.texts.length === 1, "a pong while hibernated");
    expect(probe().constructed, "a ping does not wake the object").toBe(builtBefore);

    // The next record wakes it; the core is rebuilt and both sockets are restored.
    const mark = b.frames.length;
    const markAck = a.frames.length;
    a.sendNamed("appendA2");
    expect(recordSeq(await b.expect(TAG.record, mark))).toBe(2n);
    expect(probe().constructed).toBe(builtBefore + 1);
    const second = ack(await a.expect(TAG.ack, markAck));
    expect(second.seq).toBe(2n);

    // The restored member b can speak, too, and a newcomer is caught up over both rows.
    const markA = a.frames.length;
    b.sendNamed("appendB1");
    expect(recordSeq(await a.expect(TAG.record, markA))).toBe(3n);
    const c = await Client.open();
    c.sendNamed("joinOpenC");
    await c.expect(TAG.joined);
    await c.expect(TAG.catchup);
    // A peer that left after the wake is announced.
    const markB = b.frames.length;
    a.close();
    await b.expect(TAG.peerLeft, markB);
    b.close();
    c.close();
    await wipe();
  });
});

describe("pictures through the Worker", () => {
  it("stores, lists and returns a sealed picture, byte for byte", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);

    const put = await request(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: SEALED });
    expect(put.status).toBe(201);
    expect(await put.json()).toEqual({ room_images: 1, room_image_bytes: SEALED.byteLength });

    const get = await request(`/r/${ROOM}/f/${NAME}`, { headers: { authorization: MEMBER } });
    expect(get.status).toBe(200);
    expect(get.headers.get("content-type")).toBe("application/octet-stream");
    expect(new Uint8Array(await get.arrayBuffer())).toEqual(SEALED);

    const head = await request(`/r/${ROOM}/f/${NAME}`, { method: "HEAD", headers: { authorization: MEMBER } });
    expect(head.status).toBe(200);
    expect(head.headers.get("content-length")).toBe(String(SEALED.byteLength));
    expect(await head.text()).toBe("");

    const list = await request(`/r/${ROOM}/f`, { headers: { authorization: MEMBER } });
    expect(await list.json()).toEqual([{ name: NAME, size: SEALED.byteLength }]);

    // One row: the manifest and the bytes together.
    expect((await rowCounts())["room_images"]).toBe(1);
    const missing = await request(`/r/${ROOM}/f/AAAAAAAAAAAAAAAAAAAAAA`, { headers: { authorization: MEMBER } });
    expect(missing.status).toBe(404);
    expect(((await missing.json()) as { error: string }).error).toBe("image_missing");
    a.close();
    await wipe();
  });

  it("refuses bytes that are not a sealed picture, and a picture for a room that does not exist", async () => {
    const none = await request(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: SEALED });
    expect(none.status).toBe(404);
    expect(((await none.json()) as { error: string }).error).toBe("room_missing");

    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    const junk = await request(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: new Uint8Array(200) });
    expect(junk.status).toBe(400);
    expect((await rowCounts())["room_images"]).toBe(0);
    a.close();
    await wipe();
  });

  it("does not move the retention deadline: an image request is no visit", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    const before = joined(await a.expect(TAG.joined)).expiresAtMs;
    await sleep(30);
    await request(`/r/${ROOM}/f`, { headers: { authorization: MEMBER } });
    const b = await Client.open();
    b.sendNamed("joinOpenB");
    const after = joined(await b.expect(TAG.joined)).expiresAtMs;
    expect(after, "a join is a visit and moves it, a list does not").toBeGreaterThan(before);
    a.close();
    b.close();
    await wipe();
  });
});

describe("a hostile client", () => {
  it("cannot make the room throw: bad frames close that socket only", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    const bad = await Client.open();
    bad.send(unhex("ff".repeat(64)));
    await bad.untilClosed();
    // A frame the core cannot decode after the join.
    const b = await Client.open();
    b.sendNamed("joinOpenB");
    await b.expect(TAG.joined);
    b.send(unhex("10ffffffff"));
    await b.untilClosed();
    // The room still serves a.
    a.sendNamed("appendA1");
    await a.expect(TAG.ack);
    a.close();
    await wipe();
  });

  it("is refused a second join on one socket", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    a.sendNamed("joinOpenA");
    await a.expect(TAG.error);
    await a.untilClosed();
    await wipe();
  });
});

describe("the demo does not delete rooms on request", () => {
  it("answers DeleteRoom with Forbidden", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    a.sendNamed("deleteRoom");
    expect(errorCode(await a.expect(TAG.error))).toBe(4);
    await a.untilClosed();
    expect((await rowCounts())["room_meta"]).toBe(1);
    await wipe();
  });
});
