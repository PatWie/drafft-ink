import { abortAllDurableObjects, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";

import { Client, ERROR, MEMBER, ROOM, TAG, errorCode, fixtures, joined, request, retention, roomStub, sleep, unhex } from "./helpers";
import { probe } from "./worker";

const HOUR = 3_600_000;
const TTL_MS = 172_800_000;
const NAME = fixtures.image.name;

afterEach(async () => {
  probe().clockOffsetMs = 0;
  await runInDurableObject(roomStub(), async (_instance, state) => {
    await state.storage.deleteAlarm();
    await state.storage.deleteAll();
  });
  await abortAllDurableObjects();
});

/** The alarm that the room set, in milliseconds since the epoch, or `null`. */
function alarmOf(): Promise<number | null> {
  return runInDurableObject(roomStub(), (_instance, state) => state.storage.getAlarm());
}

/** What the room's storage holds: the tables it made and the rows in them, and whether any key-value data exists. */
function storageDump(): Promise<{ tables: string[]; rows: number; kv: number; alarm: number | null }> {
  return runInDurableObject(roomStub(), async (_instance, state) => {
    const sql = state.storage.sql;
    const tables = sql
      .exec("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_cf_%'")
      .toArray()
      .map((row) => String(row["name"]));
    let rows = 0;
    for (const table of tables) rows += Number(sql.exec(`SELECT count(*) AS n FROM "${table}"`).one()["n"]);
    return { tables, rows, kv: (await state.storage.list()).size, alarm: await state.storage.getAlarm() };
  });
}

async function joinAs(frameName: "joinCreateA" | "joinOpenA" | "joinOpenB"): Promise<{ client: Client; expiresAt: number }> {
  const client = await Client.open();
  client.sendNamed(frameName);
  const frame = await client.expect(TAG.joined);
  return { client, expiresAt: Number(joined(frame).expiresAtMs) };
}

/** Closes a client and waits until the room has seen it. */
async function leave(client: Client): Promise<void> {
  client.close();
  await client.untilClosed();
  await sleep(50);
}

async function expectGone(): Promise<void> {
  const dump = await storageDump();
  expect(dump.alarm, "no alarm").toBeNull();
  expect(dump.kv, "no key-value data").toBe(0);
  expect(dump.rows, `no rows in ${dump.tables}`).toBe(0);
  // A later open join finds no room.
  const again = await Client.open();
  again.sendNamed("joinOpenA");
  expect(errorCode(await again.expect(TAG.error))).toBe(ERROR.roomMissing);
  await again.untilClosed();
}

describe("the deadline and its alarm", () => {
  it("a join sets the deadline to 48 hours from now and arms the alarm for it", async () => {
    const before = Date.now();
    const { client, expiresAt } = await joinAs("joinCreateA");
    expect(expiresAt).toBeGreaterThanOrEqual(before + TTL_MS);
    expect(expiresAt).toBeLessThanOrEqual(Date.now() + TTL_MS);
    expect(await alarmOf()).toBe(expiresAt);
    await leave(client);
  });

  it("a later visit moves the deadline and the alarm", async () => {
    const first = await joinAs("joinCreateA");
    const firstAlarm = await alarmOf();
    await leave(first.client);
    probe().clockOffsetMs = 5 * HOUR;
    const second = await joinAs("joinOpenB");
    expect(second.expiresAt).toBeGreaterThanOrEqual(first.expiresAt + 5 * HOUR);
    expect(await alarmOf()).toBe(second.expiresAt);
    expect(firstAlarm).toBe(first.expiresAt);
    await leave(second.client);
  });

  it("edits, snapshots, pictures and pings do not move it", async () => {
    const { client, expiresAt } = await joinAs("joinCreateA");
    probe().clockOffsetMs = 3 * HOUR;
    client.sendNamed("appendA1");
    await client.expect(TAG.ack);
    client.sendNamed("snapshotCovers2");
    client.ws.send("ping");
    await request(`/r/${ROOM}/f`, { headers: { authorization: MEMBER } });
    await request(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: unhex(fixtures.image.sealedHex) });
    await request(`/r/${ROOM}/f/${NAME}`, { headers: { authorization: MEMBER } });
    await sleep(100);
    expect(await alarmOf()).toBe(expiresAt);
    await leave(client);
  });

  it("an alarm before the deadline arms the alarm again for the deadline and keeps the room", async () => {
    const { client, expiresAt } = await joinAs("joinCreateA");
    await leave(client);
    probe().clockOffsetMs = 10 * HOUR;
    const ran = probe().alarms;
    expect(await runDurableObjectAlarm(roomStub())).toBe(true);
    expect(probe().alarms).toBe(ran + 1);
    expect(await alarmOf(), "re-armed for the same deadline").toBe(expiresAt);
    expect((await storageDump()).rows).toBeGreaterThan(0);
  });
});

describe("what happens at the deadline", () => {
  it("deletes everything when nobody is connected", async () => {
    const { client } = await joinAs("joinCreateA");
    client.sendNamed("appendA1");
    await client.expect(TAG.ack);
    await request(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: unhex(fixtures.image.sealedHex) });
    await leave(client);
    probe().clockOffsetMs = TTL_MS + 2 * HOUR;
    expect(await runDurableObjectAlarm(roomStub())).toBe(true);
    await expectGone();
  });

  it("a second alarm after the deletion changes nothing (it is idempotent)", async () => {
    const { client } = await joinAs("joinCreateA");
    await leave(client);
    probe().clockOffsetMs = TTL_MS + HOUR;
    await runDurableObjectAlarm(roomStub());
    expect(await runDurableObjectAlarm(roomStub()), "no alarm is left to run").toBe(false);
    await expectGone();
  });

  it("with someone connected the room is expiring: the person is told, and it is checked again in an hour", async () => {
    const { client } = await joinAs("joinCreateA");
    probe().clockOffsetMs = TTL_MS + 2 * HOUR;
    client.ws.send("ping");
    await client.until(() => client.texts.length === 1, "a pong");
    const mark = client.frames.length;
    expect(await runDurableObjectAlarm(roomStub())).toBe(true);
    const notice = retention(await client.expect(TAG.retention, mark));
    expect(notice.expiring).toBe(true);
    const next = await alarmOf();
    expect(next).not.toBeNull();
    const expected = Date.now() + probe().clockOffsetMs + HOUR;
    expect(Math.abs((next ?? 0) - expected)).toBeLessThan(10_000);
    expect((await storageDump()).rows, "the room is still there").toBeGreaterThan(0);

    // The last person leaves: the room goes after the grace (1 second here), not at once and not never.
    client.close();
    await client.untilClosed();
    await waitUntil(async () => (await storageDump()).rows === 0, "the room to be deleted after the grace");
    await expectGone();
  });

  it("a person who rejoins inside the grace saves the room, and it gets a new 48 hours", async () => {
    const { client } = await joinAs("joinCreateA");
    probe().clockOffsetMs = TTL_MS + 2 * HOUR;
    client.ws.send("ping");
    await client.until(() => client.texts.length === 1, "a pong");
    await runDurableObjectAlarm(roomStub());
    await client.expect(TAG.retention);
    client.close();
    await client.untilClosed();
    const back = await joinAs("joinOpenB");
    expect(back.expiresAt).toBeGreaterThan(Date.now() + probe().clockOffsetMs + TTL_MS - 10_000);
    await sleep(1_800);
    expect((await storageDump()).rows, "the grace passed and the room is still there").toBeGreaterThan(0);
    expect(await alarmOf()).toBe(back.expiresAt);
    await leave(back.client);
  });

  it("a socket that has been silent for more than two minutes does not hold a room", async () => {
    const { client } = await joinAs("joinCreateA");
    // No ping after the join: the runtime's last answer is none, the last frame was the join.
    probe().clockOffsetMs = TTL_MS + 2 * HOUR;
    expect(await runDurableObjectAlarm(roomStub())).toBe(true);
    await expectGone();
    const closed = await client.untilClosed();
    expect(closed.code).toBe(1000);
  });

  it("a ping keeps the same socket alive across the deadline (the contrast of the test before)", async () => {
    const { client } = await joinAs("joinCreateA");
    probe().clockOffsetMs = TTL_MS + 2 * HOUR;
    client.ws.send("ping");
    await client.until(() => client.texts.length === 1, "a pong");
    await runDurableObjectAlarm(roomStub());
    expect((await storageDump()).rows).toBeGreaterThan(0);
    expect(client.closed).toBeNull();
    await leave(client);
  });
});

describe("a room that was deleted", () => {
  it("can be created again, as a new room with no memory of the old one", async () => {
    const old = await joinAs("joinCreateA");
    old.client.sendNamed("appendA1");
    await old.client.expect(TAG.ack);
    await leave(old.client);
    probe().clockOffsetMs = TTL_MS + HOUR;
    await runDurableObjectAlarm(roomStub());
    probe().clockOffsetMs = 0;
    const fresh = await Client.open();
    fresh.sendNamed("joinCreateA");
    const frame = joined(await fresh.expect(TAG.joined));
    expect(frame.roomState).toBe(1);
    expect(frame.headSeq).toBe(0n);
    await leave(fresh);
  });
});

async function waitUntil(condition: () => Promise<boolean>, what: string, timeoutMs = 8_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!(await condition())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await sleep(100);
  }
}
