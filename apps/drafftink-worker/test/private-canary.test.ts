import { env } from "cloudflare:workers";
import { afterEach, describe, expect, it } from "vitest";

import { MEMBER, ROOM, TAG, fixtures, sleep, unhex } from "./helpers";
import { r2Keys, resetAll, signedClient, signedRequest } from "./private-helpers";
import { dumpStorage, findHits, secrets, tokenNeedles, type Source } from "./scan";

const NAME = fixtures.image.name;
const encoder = new TextEncoder();

afterEach(resetAll);

/** Every object in R2: its key, its bytes, and every piece of metadata that came with it. */
async function dumpR2(): Promise<Source[]> {
  const sources: Source[] = [];
  for (const key of await r2Keys()) {
    const object = await env.BLOBS.get(key);
    sources.push({ where: `r2 key ${key}`, bytes: encoder.encode(key) });
    if (object === null) continue;
    sources.push({ where: `r2 body of ${key}`, bytes: new Uint8Array(await object.arrayBuffer()) });
    sources.push({ where: `r2 metadata of ${key}`, bytes: encoder.encode(JSON.stringify({ http: object.httpMetadata, custom: object.customMetadata })) });
  }
  return sources;
}

describe("the private instance stays blind too", () => {
  it("R2 and the room's database hold sealed bytes and names, and nothing that names the marker, the person, the key or the picture", async () => {
    const a = await signedClient();
    a.sendNamed("joinCreateA");
    await a.expect(TAG.joined);
    const b = await signedClient();
    b.sendNamed("joinOpenB");
    await b.expect(TAG.joined);
    a.sendNamed("appendA1");
    b.sendNamed("appendB1");
    await a.expect(TAG.ack);
    await b.expect(TAG.ack);
    a.sendNamed("presenceA");
    await b.expect(TAG.presenceBundle);
    const put = await signedRequest(`/r/${ROOM}/f/${NAME}`, { method: "PUT", headers: { authorization: MEMBER }, body: unhex(fixtures.image.sealedHex) });
    expect(put.status).toBe(201);
    await sleep(1_200);

    const storage = await dumpStorage();
    const r2 = await dumpR2();
    const frames: Source[] = [a, b].flatMap((client, i) =>
      client.frames.map((bytes, n) => ({ where: `client ${i} frame ${n}`, bytes })),
    );

    // The scan looks at real data: the picture is in R2, and the manifest row has a name and a size.
    expect(r2.some((s) => s.where.startsWith("r2 body") && s.bytes.length > 100)).toBe(true);
    const row = storage.filter((s) => s.where.includes("room_images"));
    expect(row.length).toBeGreaterThan(0);
    const cells = Object.fromEntries(row.map((s) => [s.where.split(" column ")[1] ?? "", s.bytes.length]));
    expect(cells["data"], "the manifest holds no bytes").toBe(0);
    expect(cells["size"]).toBeGreaterThan(0);
    // The names are the one thing R2 may know: the key ends in the keyed name.
    const nameHex = [...atob(NAME.replaceAll("-", "+").replaceAll("_", "/"))].map((c) => c.charCodeAt(0).toString(16).padStart(2, "0")).join("");
    expect((await r2Keys())[0]?.endsWith(`/${nameHex}`)).toBe(true);

    const needles = secrets();
    expect(findHits(storage, needles), "the room's database").toEqual([]);
    expect(findHits(r2, needles), "R2").toEqual([]);
    expect(findHits(frames, needles), "frames").toEqual([]);
    const token = tokenNeedles();
    expect(findHits(storage, token), "token in the database").toEqual([]);
    expect(findHits(r2, token), "token in R2").toEqual([]);
    expect(findHits(frames, token), "token in frames").toEqual([]);
    a.close();
    b.close();
  });

  it("finds a leak in R2 if there is one (the check can fail)", async () => {
    await env.BLOBS.put("r/leak", encoder.encode(`pixels ${fixtures.marker}`), { customMetadata: { who: fixtures.userName } });
    const hits = findHits(await dumpR2(), secrets());
    expect(hits.some((h) => h.includes("the marker") && h.includes("r2 body"))).toBe(true);
    expect(hits.some((h) => h.includes("the user name") && h.includes("r2 metadata"))).toBe(true);
  });
});
