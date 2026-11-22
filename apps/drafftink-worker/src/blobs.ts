import { guard, HostFailure } from "./host-error";
import { blobOf, numberOf } from "./sql-store";
import { r2Prefix } from "./wasm";
import type { BlobHost } from "./wasm";

/** How a backend's asynchronous tail ended. */
export type SettleOutcome =
  | { readonly ok: true }
  /** The write failed and its manifest rows are rolled back. `rate_limited`: R2 allows one write a second per key. */
  | { readonly ok: false; readonly reason: "rate_limited" | "unavailable" };

/**
 * Where a room keeps its sealed pictures. The Rust core calls the synchronous part (`BlobHost`) while it checks the
 * room's limits and quotas; the asynchronous part runs in the shell after the core returned.
 *
 * - The manifest (name and size of each picture) always lives in the room's database, because the quotas are
 *   checked there in one synchronous call, and listings and `HEAD` come from it.
 * - `settle()` is called once after every picture request, with no `await` between the core's call and it. It
 *   finishes what the core started: nothing for the database backend, the R2 write for the R2 backend.
 * - `purge()` removes what lives outside the room's database. It runs before the database is wiped, and may be
 *   called again after a failure: it must be safe to repeat.
 */
export interface BlobBackend extends BlobHost {
  /** The asynchronous tail of the request the core just answered. Rolls the manifest back when it fails. */
  settle(): Promise<SettleOutcome>;
  /** Removes what lives outside the room's database. */
  purge(): Promise<void>;
}

/** The manifest rows of the pictures, in the room's database. The bytes are the business of a subclass. */
abstract class ManifestBlobs implements BlobBackend {
  constructor(protected readonly sql: () => SqlStorage) {}

  abstract put(name: Uint8Array, sealed: Uint8Array): void;
  abstract get(name: Uint8Array): Uint8Array | null;
  abstract settle(): Promise<SettleOutcome>;
  abstract purge(): Promise<void>;

  head(name: Uint8Array): number | null {
    return guard(() => {
      const row = this.sql().exec("SELECT size FROM room_images WHERE name = ?", name).toArray()[0];
      return row === undefined ? null : numberOf(row["size"]);
    });
  }

  list(): [Uint8Array, number][] {
    return guard(() =>
      this.sql()
        .exec("SELECT name, size FROM room_images ORDER BY name")
        .toArray()
        .map((row): [Uint8Array, number] => [blobOf(row["name"]), numberOf(row["size"])]),
    );
  }

  stats(): [number, number] {
    return guard(() => {
      const row = this.sql().exec("SELECT count(*) AS n, coalesce(sum(size), 0) AS bytes FROM room_images").one();
      return [numberOf(row["n"]), numberOf(row["bytes"])];
    });
  }

  /** Nothing to do: the room's host wipes the whole database when the core reports the deletion. */
  deleteAll(): void {}
}

/** The demo's backend: the sealed bytes are a column of the room's own database. */
export class DatabaseBlobs extends ManifestBlobs {
  put(name: Uint8Array, sealed: Uint8Array): void {
    guard(() =>
      this.sql().exec("INSERT OR REPLACE INTO room_images (name, size, data) VALUES (?, ?, ?)", name, sealed.byteLength, sealed),
    );
  }

  get(name: Uint8Array): Uint8Array | null {
    return guard(() => {
      const row = this.sql().exec("SELECT data FROM room_images WHERE name = ?", name).toArray()[0];
      const data = row?.["data"];
      return data === undefined || data === null ? null : blobOf(data);
    });
  }

  settle(): Promise<SettleOutcome> {
    return Promise.resolve({ ok: true });
  }

  purge(): Promise<void> {
    return Promise.resolve();
  }
}

/** R2 answers a second write to one key within a second with this (HTTP 429, error code 10058). */
const R2_RATE_LIMIT = /\b10058\b|\(429\)|too many requests|reduce your concurrent request rate/i;

/** Whether an R2 exception is the rate limit of one write a second per key. */
export function isR2RateLimit(error: unknown): boolean {
  return error instanceof Error && R2_RATE_LIMIT.test(error.message);
}

function hexOf(bytes: Uint8Array): string {
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** A picture whose manifest row is written and whose bytes are on their way to R2. */
interface Pending {
  readonly name: Uint8Array;
  readonly bytes: Uint8Array;
  /** The size the manifest had for this name before, or `null` when the picture is new. */
  readonly previousSize: number | null;
}

/**
 * The private environment's backend (`BLOB_STORE=r2`, binding `BLOBS`): the room's database holds the manifest only
 * (name and size, never the bytes), R2 holds the sealed bytes under `r/<room id hex>/<name hex>`.
 *
 * An upload is "manifest row first": the core checks the room's limits and quotas and the row is written inside
 * its synchronous call; `settle()` then writes to R2 and, when R2 fails, puts the manifest back as it was, so the
 * manifest never lists a picture that R2 lacks. (A crash of the object between the two leaves such a row; the client
 * then finds the picture missing on download and uploads it again.) The sealed picture is at most 4 MiB and the core
 * has to read it to check its header, so the body is buffered once and handed to R2 as bytes, not as a stream.
 *
 * Downloads never come here: the Worker reads R2 itself after its membership check, so no object is woken.
 */
export class R2Blobs extends ManifestBlobs {
  #pending: Pending[] = [];

  constructor(
    sql: () => SqlStorage,
    private readonly bucket: () => R2Bucket,
    private readonly room: string,
    private readonly pageSize: number,
  ) {
    super(sql);
  }

  put(name: Uint8Array, sealed: Uint8Array): void {
    guard(() => {
      const before = this.head(name);
      this.sql().exec("INSERT OR REPLACE INTO room_images (name, size, data) VALUES (?, ?, NULL)", name, sealed.byteLength);
      this.#pending.push({ name, bytes: sealed.slice(), previousSize: before });
    });
  }

  /** Downloads are served by the Worker from R2. The core never has the bytes in this mode. */
  get(): Uint8Array | null {
    throw new HostFailure("storage", "pictures are served from R2 by the Worker, not by the room");
  }

  async settle(): Promise<SettleOutcome> {
    // Taken at once, before the first await: a request that comes in meanwhile has its own list.
    const pending = this.#pending;
    this.#pending = [];
    const written: Pending[] = [];
    for (const item of pending) {
      try {
        await this.bucket().put(this.keyOf(item.name), item.bytes, { httpMetadata: { contentType: "application/octet-stream" } });
        written.push(item);
      } catch (error) {
        console.error("r2 write failed", error);
        // The rows of the pictures that did not get to R2 go back to what they were.
        for (const lost of pending.filter((p) => !written.includes(p))) this.rollBack(lost);
        return { ok: false, reason: isR2RateLimit(error) ? "rate_limited" : "unavailable" };
      }
    }
    return { ok: true };
  }

  async purge(): Promise<void> {
    const bucket = this.bucket();
    const prefix = r2Prefix(this.room);
    if (prefix === undefined) throw new HostFailure("storage", "the room id is not valid");
    let cursor: string | undefined;
    do {
      const page = await bucket.list(cursor === undefined ? { prefix, limit: this.pageSize } : { prefix, limit: this.pageSize, cursor });
      const keys = page.objects.map((object) => object.key);
      if (keys.length > 0) await bucket.delete(keys);
      cursor = page.truncated ? page.cursor : undefined;
    } while (cursor !== undefined);
  }

  private keyOf(name: Uint8Array): string {
    const prefix = r2Prefix(this.room);
    if (prefix === undefined) throw new HostFailure("storage", "the room id is not valid");
    return `${prefix}${hexOf(name)}`;
  }

  private rollBack(item: Pending): void {
    try {
      if (item.previousSize === null) {
        this.sql().exec("DELETE FROM room_images WHERE name = ?", item.name);
      } else {
        this.sql().exec("UPDATE room_images SET size = ? WHERE name = ?", item.previousSize, item.name);
      }
    } catch (error) {
      console.error("r2 write failed and the manifest could not be rolled back", error);
    }
  }
}
