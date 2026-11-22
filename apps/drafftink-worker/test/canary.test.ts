import { runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";

import { Client, MEMBER, ROOM, STRANGER, TAG, fixtures, request, roomStub, sleep, unhex } from "./helpers";
import { dumpStorage, findHits, secrets, tokenNeedles, type Source } from "./scan";

const NAME = fixtures.image.name;

/** What a client saw and sent, as sources to search. */
function framesOf(who: string, client: Client): { received: Source[]; sent: Source[] } {
  const received = [
    ...client.frames.map((bytes, i) => ({ where: `${who} received frame ${i} (tag ${bytes[0]?.toString(16)})`, bytes })),
    ...client.texts.map((text, i) => ({ where: `${who} received text ${i}`, bytes: new TextEncoder().encode(text) })),
  ];
  const sent = client.sent.map((bytes, i) => ({ where: `${who} sent frame ${i}`, bytes }));
  return { received, sent };
}

async function reply(where: string, response: Response): Promise<Source> {
  const headers = [...response.headers].map(([k, v]) => `${k}: ${v}`).join("\n");
  const body = new Uint8Array(await response.arrayBuffer());
  const bytes = new Uint8Array(headers.length + body.length);
  bytes.set(new TextEncoder().encode(headers), 0);
  bytes.set(body, headers.length);
  return { where, bytes };
}

describe("the relay stays blind", () => {
  it("holds and sends nothing that names the marker, the person, the key, a subkey or the picture", async () => {
    const a = await Client.open();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    const b = await Client.open();
    b.sendNamed("joinOpenB");
    await b.expect(TAG.joined);

    // Two members append sealed records that hold the marker, and one that does not.
    a.sendNamed("appendA1");
    b.sendNamed("appendB1");
    a.sendNamed("appendA2");
    await a.until(() => a.tags().filter((t) => t === TAG.ack).length === 2, "two acks of a");
    await b.expect(TAG.ack);
    // A sealed presence record with the person's name, relayed in a bundle.
    a.sendNamed("presenceA");
    await b.expect(TAG.presenceBundle);
    // A snapshot that holds the marker, covering what was written.
    a.sendNamed("snapshotCovers2");
    await a.expect(TAG.snapshotResult);
    // A sealed picture whose plaintext holds the marker, and its readers.
    const http: Source[] = [];
    http.push(await reply("PUT image", await request(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: unhex(fixtures.image.sealedHex) })));
    http.push(await reply("GET image", await request(`/r/${ROOM}/f/${NAME}`, { headers: { authorization: MEMBER } })));
    http.push(await reply("HEAD image", await request(`/r/${ROOM}/f/${NAME}`, { method: "HEAD", headers: { authorization: MEMBER } })));
    http.push(await reply("list", await request(`/r/${ROOM}/f`, { headers: { authorization: MEMBER } })));
    http.push(await reply("stranger", await request(`/r/${ROOM}/f`, { headers: { authorization: STRANGER } })));
    http.push(await reply("status", await request("/api/status")));
    // A late joiner is caught up from storage.
    const c = await Client.open();
    c.sendNamed("joinOpenC");
    await c.expect(TAG.catchup);
    await sleep(1_200);

    const storage = await dumpStorage();
    const frames = [framesOf("a", a), framesOf("b", b), framesOf("c", c)];
    const received = frames.flatMap((f) => f.received);
    const sent = frames.flatMap((f) => f.sent);

    // The scan looks at real data: the storage holds rows, the clients saw frames.
    expect(storage.some((s) => s.where.includes("room_segments"))).toBe(true);
    expect(storage.some((s) => s.where.includes("room_images") && s.bytes.length > 100)).toBe(true);
    expect(received.length).toBeGreaterThan(8);

    const needles = secrets();
    expect(needles.length).toBeGreaterThan(40);
    expect(findHits(storage, needles), "storage").toEqual([]);
    expect(findHits(received, needles), "frames the relay sent").toEqual([]);
    expect(findHits(sent, needles), "frames the clients sent").toEqual([]);
    expect(findHits(http, needles), "HTTP replies").toEqual([]);

    // The membership token is allowed in a join frame and in the header, and nowhere that the relay writes.
    const token = tokenNeedles();
    expect(findHits(storage, token), "token in storage").toEqual([]);
    expect(findHits(received, token), "token in frames the relay sent").toEqual([]);
    expect(findHits(http, token), "token in HTTP replies").toEqual([]);
    expect(findHits(sent, token).length, "the join frames do carry it, so the scan can see it").toBeGreaterThan(0);

    a.close();
    b.close();
    c.close();
  });
});

describe("the check can fail", () => {
  const leaky = (write: (state: DurableObjectState) => Promise<void> | void) =>
    runInDurableObject(roomStub(), async (_instance, state) => {
      await write(state);
    });

  it("finds the marker in a row, in a blob, and in a table name", async () => {
    await leaky((state) => {
      state.storage.sql.exec("CREATE TABLE IF NOT EXISTS leak_text (note TEXT)");
      state.storage.sql.exec("INSERT INTO leak_text (note) VALUES (?)", `see ${fixtures.marker} here`);
      state.storage.sql.exec("CREATE TABLE IF NOT EXISTS leak_blob (data BLOB)");
      state.storage.sql.exec("INSERT INTO leak_blob (data) VALUES (?)", new TextEncoder().encode(`x${fixtures.userName}y`));
      state.storage.sql.exec(`CREATE TABLE IF NOT EXISTS "tbl_${fixtures.marker.replaceAll("-", "_")}" (x)`);
    });
    const hits = findHits(await dumpStorage(), secrets());
    expect(hits.some((h) => h.includes("the marker") && h.includes("leak_text"))).toBe(true);
    expect(hits.some((h) => h.includes("the user name") && h.includes("leak_blob"))).toBe(true);
    await leaky((state) => {
      state.storage.sql.exec("DROP TABLE leak_text");
      state.storage.sql.exec("DROP TABLE leak_blob");
    });
  });

  it("finds the key in every spelling, and the picture's hash", async () => {
    const spellings = [
      fixtures.room.key.hex,
      fixtures.room.key.base64,
      fixtures.room.key.base64url,
      fixtures.room.key.base64urlNoPad,
      fixtures.room.subkeys.content.hex,
      fixtures.image.fileId,
      fixtures.image.pngSha256,
    ];
    for (const spelling of spellings) {
      await leaky((state) => {
        state.storage.sql.exec("CREATE TABLE IF NOT EXISTS leak_key (v TEXT)");
        state.storage.sql.exec("DELETE FROM leak_key");
        state.storage.sql.exec("INSERT INTO leak_key (v) VALUES (?)", spelling);
      });
      const hits = findHits(await dumpStorage(), secrets());
      expect(hits.length, spelling).toBeGreaterThan(0);
    }
    await leaky((state) => {
      state.storage.sql.exec("DROP TABLE leak_key");
    });
  });

  it("finds a secret in the key-value area and the raw key bytes in a blob", async () => {
    await leaky(async (state) => {
      await state.storage.put("note", fixtures.marker);
      state.storage.sql.exec("CREATE TABLE IF NOT EXISTS leak_raw (v BLOB)");
      state.storage.sql.exec("INSERT INTO leak_raw (v) VALUES (?)", unhex(fixtures.room.key.hex));
    });
    const hits = findHits(await dumpStorage(), secrets());
    expect(hits.some((h) => h.includes("key-value"))).toBe(true);
    expect(hits.some((h) => h.includes("the room key (raw)"))).toBe(true);
    await leaky(async (state) => {
      await state.storage.delete("note");
      state.storage.sql.exec("DROP TABLE leak_raw");
    });
  });

  it("finds a leak in a recorded frame, and the token where it must not be", () => {
    const leakyFrame: Source = { where: "a frame", bytes: new TextEncoder().encode(`xx${fixtures.marker}yy`) };
    expect(findHits([leakyFrame], secrets()).length).toBeGreaterThan(0);
    const tokenInStorage: Source = { where: "a row", bytes: unhex(fixtures.room.token.hex) };
    expect(findHits([tokenInStorage], tokenNeedles()).length).toBeGreaterThan(0);
    expect(findHits([{ where: "clean", bytes: new Uint8Array([1, 2, 3]) }], [...secrets(), ...tokenNeedles()])).toEqual([]);
  });
});
