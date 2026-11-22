import { runInDurableObject } from "cloudflare:test";

import { fixtures, hex, unhex, roomStub } from "./helpers";

const encoder = new TextEncoder();

/** A named value that must not be found where the relay keeps or sends its data. */
export interface Needle {
  name: string;
  bytes: Uint8Array;
}

/** One place that was searched: a table cell, a key-value entry, a frame. */
export interface Source {
  where: string;
  bytes: Uint8Array;
}

function base64(bytes: Uint8Array, url: boolean, pad: boolean): string {
  let text = btoa(String.fromCharCode(...bytes));
  if (url) text = text.replaceAll("+", "-").replaceAll("/", "_");
  return pad ? text : text.replace(/=+$/, "");
}

/** A secret in each of the ways a program might write it down: raw, hex, base64 and base64url (padded or not). */
export function forms(name: string, raw: Uint8Array): Needle[] {
  return [
    { name: `${name} (raw)`, bytes: raw },
    { name: `${name} (hex)`, bytes: encoder.encode(hex(raw)) },
    { name: `${name} (HEX)`, bytes: encoder.encode(hex(raw).toUpperCase()) },
    { name: `${name} (base64)`, bytes: encoder.encode(base64(raw, false, true)) },
    { name: `${name} (base64 unpadded)`, bytes: encoder.encode(base64(raw, false, false)) },
    { name: `${name} (base64url)`, bytes: encoder.encode(base64(raw, true, true)) },
    { name: `${name} (base64url unpadded)`, bytes: encoder.encode(base64(raw, true, false)) },
  ];
}

/** What the canary looks for: the plaintext, the key and everything derived from it, and the picture's identity. */
export function secrets(): Needle[] {
  const text = (name: string, value: string): Needle[] => [
    { name, bytes: encoder.encode(value) },
    { name: `${name} (UTF-16)`, bytes: new Uint8Array([...value].flatMap((c) => [c.charCodeAt(0), 0])) },
    // The text in base64 and base64url (the first three forms are the raw bytes and hex, which the text itself covers).
    ...forms(`${name} as text`, encoder.encode(value)).slice(3),
  ];
  const subkeys = Object.entries(fixtures.room.subkeys).flatMap(([name, value]) => forms(`subkey ${name}`, unhex(value.hex)));
  return [
    ...text("the marker", fixtures.marker),
    ...text("the user name", fixtures.userName),
    ...forms("the room key", unhex(fixtures.room.key.hex)),
    ...subkeys,
    ...forms("the file id", unhex(fixtures.image.fileId)),
    ...forms("the SHA-256 of the png", unhex(fixtures.image.pngSha256)),
  ];
}

/** The membership token: allowed in a join frame and an `Authorization` header, nowhere else. */
export function tokenNeedles(): Needle[] {
  return forms("the membership token", unhex(fixtures.room.token.hex));
}

function indexOf(hay: Uint8Array, needle: Uint8Array): number {
  if (needle.length === 0 || needle.length > hay.length) return -1;
  outer: for (let i = 0; i + needle.length <= hay.length; i++) {
    for (let j = 0; j < needle.length; j++) if (hay[i + j] !== needle[j]) continue outer;
    return i;
  }
  return -1;
}

/** Every hit of any needle in any source, as readable lines. Empty means clean. */
export function findHits(sources: Source[], needles: Needle[]): string[] {
  const hits: string[] = [];
  for (const source of sources) {
    for (const needle of needles) {
      if (indexOf(source.bytes, needle.bytes) >= 0) hits.push(`${needle.name} found in ${source.where}`);
    }
  }
  return hits;
}

type Cell = string | number | ArrayBuffer | null;

function cellBytes(value: Cell): Uint8Array {
  if (value === null) return new Uint8Array(0);
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  return encoder.encode(String(value));
}

/**
 * Everything that the room's Durable Object keeps: the schema, every row of every table (the key-value area is a
 * table too, `_cf_KV`), the key-value API's view of it, and the alarm.
 */
export function dumpStorage(): Promise<Source[]> {
  return runInDurableObject(roomStub(), async (_instance, state) => {
    const sources: Source[] = [];
    const sql = state.storage.sql;
    const objects = sql.exec("SELECT type, name, tbl_name, sql FROM sqlite_master").toArray();
    for (const object of objects) {
      const text = [object["type"], object["name"], object["tbl_name"], object["sql"]].map((v) => String(v ?? "")).join("\n");
      sources.push({ where: `sqlite_master ${String(object["name"])}`, bytes: encoder.encode(text) });
    }
    const tables = objects.filter((o) => o["type"] === "table").map((o) => String(o["name"]));
    for (const table of tables) {
      let rows: Record<string, Cell>[];
      try {
        rows = sql.exec(`SELECT * FROM "${table}"`).toArray() as Record<string, Cell>[];
      } catch {
        // An internal table that SQL may not read: the key-value listing below covers it.
        continue;
      }
      rows.forEach((row, index) => {
        for (const [column, value] of Object.entries(row)) {
          sources.push({ where: `table ${table} row ${index} column ${column}`, bytes: cellBytes(value) });
        }
      });
    }
    for (const [key, value] of await state.storage.list()) {
      sources.push({ where: `key-value key ${key}`, bytes: encoder.encode(key) });
      sources.push({ where: `key-value value of ${key}`, bytes: encoder.encode(JSON.stringify(value) ?? "") });
    }
    sources.push({ where: "the alarm", bytes: encoder.encode(String(await state.storage.getAlarm())) });
    return sources;
  });
}
