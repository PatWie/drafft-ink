import { abortAllDurableObjects, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";

import { Client, ERROR, MEMBER, ROOM, TAG, errorCode, fixtures, joined, request, roomStub, sleep, unhex } from "./helpers";
import { probe } from "./worker";

afterEach(async () => {
  probe().clockOffsetMs = 0;
  await runInDurableObject(roomStub(), async (_instance, state) => {
    await state.storage.deleteAlarm();
    await state.storage.deleteAll();
  });
  await abortAllDurableObjects();
});

function alarmOf(): Promise<number | null> {
  return runInDurableObject(roomStub(), (_instance, state) => state.storage.getAlarm());
}

function rowsOf(): Promise<number> {
  return runInDurableObject(roomStub(), (_instance, state) => {
    const sql = state.storage.sql;
    const tables = sql
      .exec("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_cf_%'")
      .toArray();
    return tables.reduce((sum, row) => sum + Number(sql.exec(`SELECT count(*) AS n FROM "${String(row["name"])}"`).one()["n"]), 0);
  });
}

describe("with ROOM_TTL_SECS unset (the private instance)", () => {
  it("never sets a deadline or an alarm", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    expect(joined(await a.expect(TAG.joined)).expiresAtMs, "0 means never").toBe(0n);
    a.sendNamed("appendA1");
    await a.expect(TAG.ack);
    await request(`/r/${ROOM}/f`, { headers: { authorization: MEMBER } });
    expect(await alarmOf()).toBeNull();
    a.close();
    await a.untilClosed();
    await sleep(100);
    expect(await alarmOf()).toBeNull();
    probe().clockOffsetMs = 400 * 24 * 3_600_000;
    expect(await runDurableObjectAlarm(roomStub()), "no alarm to run").toBe(false);
    const b = await Client.open();
    b.sendNamed("joinOpenB");
    expect(joined(await b.expect(TAG.joined)).headSeq, "the room is still there").toBe(1n);
    b.close();
  });
});

describe("DeleteRoom, where the host allows it", () => {
  it("deletes records, pictures, alarm and sockets, and the room is missing afterwards", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    const b = await Client.open();
    b.sendNamed("joinOpenB");
    await b.expect(TAG.joined);
    a.sendNamed("appendA1");
    await a.expect(TAG.ack);
    const put = await request(`/r/${ROOM}/f/${fixtures.image.name}`, {
      method: "PUT",
      headers: { authorization: MEMBER },
      body: unhex(fixtures.image.sealedHex),
    });
    expect(put.status).toBe(201);
    expect(await rowsOf()).toBeGreaterThan(0);

    a.sendNamed("deleteRoom");
    await a.untilClosed();
    await b.untilClosed();
    const dump = await runInDurableObject(roomStub(), async (_instance, state) => ({
      kv: (await state.storage.list()).size,
      alarm: await state.storage.getAlarm(),
      tables: state.storage.sql.exec("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'room_%'").toArray().length,
    }));
    expect(dump).toEqual({ kv: 0, alarm: null, tables: 0 });

    const again = await Client.open();
    again.sendNamed("joinOpenA");
    expect(errorCode(await again.expect(TAG.error))).toBe(ERROR.roomMissing);
    const list = await request(`/r/${ROOM}/f`, { headers: { authorization: MEMBER } });
    expect(list.status, "the picture list of a room that is gone").toBe(404);
  });

  it("is refused to a stranger and to a socket that has not joined", async () => {
    const owner = await Client.open();
    owner.sendNamed("joinCreateA");
    await owner.expect(TAG.joined);
    const stranger = await Client.open();
    stranger.sendNamed("joinStranger");
    await stranger.untilClosed();
    const early = await Client.open();
    early.sendNamed("deleteRoom");
    expect(errorCode(await early.expect(TAG.error))).toBe(ERROR.notJoined);
    await early.untilClosed();
    expect(await rowsOf(), "the room is untouched").toBeGreaterThan(0);
    owner.close();
  });
});
