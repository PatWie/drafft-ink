import { guard, HostFailure } from "./host-error";
import type { SqlHost } from "./wasm";

/**
 * The tables of a room. Every table has an `INTEGER PRIMARY KEY` or is `WITHOUT ROWID`, so an insert writes one
 * row and not two (a table with a text key and a rowid writes its index too; docs/design/collab-spike.md, section
 * 3). Created on every wake: after the room's database was wiped the next call must find them again.
 *
 * - `room_meta`: one row, the meta of the core, encoded by `drafftink_relay::codec`.
 * - `room_snapshot`: one row, the sealed snapshot and the sequence number it covers.
 * - `room_segments`: one row per flush, keyed by its first sequence number.
 * - `room_images`: the manifest of the pictures. `data` holds the sealed bytes on the demo and stays `NULL` when
 *   the bytes are in R2 (`src/blobs.ts`): then a row is the name and the size, nothing else.
 * - `room_tombstone`: written only when a deletion could not be finished (R2 did not answer). Its presence means
 *   "this room is deleted, finish the cleaning before anything else". A finished deletion wipes the database.
 */
const SCHEMA = `
CREATE TABLE IF NOT EXISTS room_meta (id INTEGER PRIMARY KEY CHECK (id = 0), data BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS room_snapshot (id INTEGER PRIMARY KEY CHECK (id = 0), covers INTEGER NOT NULL, record BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS room_segments (first INTEGER PRIMARY KEY, last INTEGER NOT NULL, data BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS room_images (name BLOB PRIMARY KEY, size INTEGER NOT NULL, data BLOB) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS room_tombstone (id INTEGER PRIMARY KEY CHECK (id = 0));
`;

/** Creates the tables that are missing. */
export function ensureSchema(sql: SqlStorage): void {
  guard(() => sql.exec(SCHEMA));
}

/** Notes that the room is deleted and its cleaning is not finished. */
export function markDeleting(sql: SqlStorage): void {
  guard(() => sql.exec("INSERT OR REPLACE INTO room_tombstone (id) VALUES (0)"));
}

/** Whether a deletion was started and not finished. */
export function isDeleting(sql: SqlStorage): boolean {
  return guard(() => sql.exec("SELECT id FROM room_tombstone").toArray().length > 0);
}

/** The bytes of a BLOB column. */
export function blobOf(value: SqlStorageValue | undefined): Uint8Array {
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  throw new HostFailure("storage", "a stored row is not in the shape this build wrote");
}

/** A number column. */
export function numberOf(value: SqlStorageValue | undefined): number {
  if (typeof value === "number") return value;
  throw new HostFailure("storage", "a stored row is not in the shape this build wrote");
}

/** The room's rows in the SQLite database of its Durable Object. Every call is synchronous. */
export class SqlRoomStore implements SqlHost {
  constructor(private readonly storage: DurableObjectStorage) {}

  get #sql(): SqlStorage {
    return this.storage.sql;
  }

  loadMeta(): Uint8Array | null {
    return guard(() => {
      const row = this.#sql.exec("SELECT data FROM room_meta WHERE id = 0").toArray()[0];
      return row === undefined ? null : blobOf(row["data"]);
    });
  }

  saveMeta(meta: Uint8Array): void {
    guard(() => this.#sql.exec("INSERT OR REPLACE INTO room_meta (id, data) VALUES (0, ?)", meta));
  }

  appendSegment(first: number, last: number, data: Uint8Array, meta: Uint8Array): void {
    guard(() =>
      this.storage.transactionSync(() => {
        this.#sql.exec("INSERT OR REPLACE INTO room_segments (first, last, data) VALUES (?, ?, ?)", first, last, data);
        this.#sql.exec("INSERT OR REPLACE INTO room_meta (id, data) VALUES (0, ?)", meta);
      }),
    );
  }

  readSegments(afterSeq: number, maxBytes: number): Uint8Array[] {
    return guard(() => {
      // Segments do not overlap, so the one that holds `afterSeq + 1` is the last one that starts at or before it.
      // Seeking to it keeps the rows read (which are billed) to the segments that are needed.
      const seek = this.#sql.exec("SELECT max(first) AS first FROM room_segments WHERE first <= ?", afterSeq + 1);
      const start = seek.toArray()[0]?.["first"];
      const from = typeof start === "number" ? start : 0;
      const rows = this.#sql.exec("SELECT last, data FROM room_segments WHERE first >= ? ORDER BY first", from);
      const out: Uint8Array[] = [];
      let bytes = 0;
      for (const row of rows) {
        if (numberOf(row["last"]) <= afterSeq) continue;
        const data = blobOf(row["data"]);
        out.push(data);
        bytes += data.byteLength;
        if (maxBytes !== 0 && bytes >= maxBytes) break;
      }
      return out;
    });
  }

  readSnapshot(): [number, Uint8Array] | null {
    return guard(() => {
      const row = this.#sql.exec("SELECT covers, record FROM room_snapshot WHERE id = 0").toArray()[0];
      return row === undefined ? null : [numberOf(row["covers"]), blobOf(row["record"])];
    });
  }

  putSnapshot(covers: number, record: Uint8Array, meta: Uint8Array): void {
    guard(() =>
      this.storage.transactionSync(() => {
        this.#sql.exec("INSERT OR REPLACE INTO room_snapshot (id, covers, record) VALUES (0, ?, ?)", covers, record);
        this.#sql.exec("INSERT OR REPLACE INTO room_meta (id, data) VALUES (0, ?)", meta);
        this.#sql.exec("DELETE FROM room_segments WHERE first <= ? AND last <= ?", covers, covers);
      }),
    );
  }

  /** Nothing to do: the room's host wipes the whole database when the core reports the deletion. */
  deleteAll(): void {}
}
