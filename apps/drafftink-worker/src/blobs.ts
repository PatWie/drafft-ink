import { blobOf, numberOf } from "./sql-store";
import { guard } from "./host-error";
import type { BlobHost } from "./wasm";

/**
 * Where a room keeps its sealed pictures. The Rust core calls the synchronous part (`BlobHost`) while it checks
 * the room's limits; the asynchronous part runs in the shell after the core returned.
 *
 * ## Plugging in R2 (phase 3P, the private environment)
 *
 * The manifest (name and size) always lives in the room's database, because the quotas are checked there, in one
 * synchronous call. An R2 backend implements the same interface like this:
 *
 * - `put(name, sealed)`: write the manifest row with `data = NULL` and remember `{name, sealed}` in a list.
 * - `settle()`: after the core returned (and answered `201` in its own mind), `await bucket.put(key, sealed)` for
 *   each remembered picture, with the key `r/<room>/<name>`. If a write fails, delete that manifest row and throw,
 *   so the HTTP reply becomes `503` and the client retries; the manifest never lists a picture that R2 lacks.
 * - `get`, `head`: the Worker reads R2 directly after its membership check, so a download wakes no object. The
 *   object's `get` is then a fallback that returns `null`.
 * - `purge()`: list `r/<room>/` and delete the objects. The shell calls it when the core reports the room as
 *   deleted, before it wipes the database.
 */
export interface BlobBackend extends BlobHost {
  /** The asynchronous tail of the requests the core answered since the last call. Throws to roll them back. */
  settle(): Promise<void>;
  /** Removes what lives outside the room's database. */
  purge(): Promise<void>;
}

/** The demo's backend: the sealed bytes are a column of the room's own database. */
export class DatabaseBlobs implements BlobBackend {
  constructor(private readonly sql: () => SqlStorage) {}

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

  settle(): Promise<void> {
    return Promise.resolve();
  }

  purge(): Promise<void> {
    return Promise.resolve();
  }
}
