# drafftink-worker: the relay on Cloudflare

The Cloudflare host of the end-to-end encrypted relay (design `docs/design/collab-e2e.md`, sections 14 and 19). One
Worker serves the web app as static files and the relay on `/r/*` and `/api/*`; one SQLite-backed Durable Object per
room holds the room. The rules (membership, sequence numbers, snapshots, rate limits, retention) are the Rust crate
`drafftink-relay`, compiled to wasm. The TypeScript around it only does what the platform needs: sockets, the clock,
storage calls and timers.

```text
src/lib.rs, config.rs, edge.rs, host.rs, driver.rs   Rust: settings, edge checks, store adapters, the core as the shell calls it
src/wasm/                                            Rust: wasm-bindgen glue (wasm32 only)
src/index.ts                                         the Worker: routing, Origin check, 413/401 before any object, /api/status, quota gate
src/room.ts                                          the Durable Object `Room`: hibernation, timers, alarms, deletion
src/sql-store.ts, blobs.ts                           the room's tables; the picture backend seam (BlobBackend)
src/wake.ts, sockets.ts, effects.ts, ...             small helpers
test/                                                vitest on miniflare (real Worker, real Room, real wasm core)
tests/                                               Rust: the fixtures of the TypeScript tests, and the smoke test against `wrangler dev`
scripts/                                             build-wasm.sh, render-config.sh, dev.sh
```

## Build, test, run

Needs Node 22 (`export PATH=$HOME/.local/node22/bin:$PATH` on the dev machine), Rust with `wasm32-unknown-unknown`, and
`wasm-bindgen-cli` in the version the workspace pins (0.2.130). `wasm-opt` is optional.

```bash
cd apps/drafftink-worker
npm ci                       # versions are pinned in package.json and package-lock.json
npm run build:wasm           # cargo (release-wasm) + wasm-bindgen into pkg/ (git-ignored)
npm run typecheck            # tsc --noEmit, strict
npm test                     # vitest on miniflare; builds pkg/ first when it is missing
cargo nextest run -p drafftink-worker                    # the Rust side, native
cargo test -p drafftink-worker --test fixtures -- --ignored regenerate   # rewrite test/fixtures/frames.json
```

The frames that the TypeScript tests send are made by `tests/fixtures.rs` (real sealed records, a fixed room key and
counting nonces). A normal Rust test fails when the committed `test/fixtures/frames.json` differs from what the code
makes, so a protocol or sealing change cannot leave stale fixtures.

### `wrangler dev`

`scripts/dev.sh` builds `pkg/` when it is missing, renders a config with `scripts/render-config.sh`, and starts the
local runtime (workerd). Nothing is deployed and no Cloudflare login is used.

```bash
scripts/dev.sh --port 8791 --persist-to /tmp/drafftink-state          # demo settings; --persist-to defaults to a new temp folder
scripts/dev.sh --env private --port 8791                              # the top level mirrors the private environment
scripts/dev.sh --port 8792 -- --var ROOM_TTL_SECS:3 --var ROOM_LEAVE_GRACE_SECS:1   # rooms expire in seconds
```

Everything after `--` goes to `wrangler dev`. `--var KEY:VALUE` overrides one variable of the config for that run; any
variable of the table below can be set this way. The web app is served from `dist/` when it has an `index.html`, else
from a one-page stub (so no build is needed). The smoke test talks to a running instance:

```bash
DRAFFTINK_WORKER_URL=http://127.0.0.1:8791 cargo test -p drafftink-worker --test smoke -- --ignored
DRAFFTINK_WORKER_URL=http://127.0.0.1:8792 DRAFFTINK_WORKER_SHORT_TTL=1 cargo test -p drafftink-worker --test smoke expires -- --ignored
```

## Configuration

`deploy/cloudflare/wrangler.toml.in` is the one template. `scripts/render-config.sh <demo|private> <out-file>` fills it
in from environment variables (defaults are safe and local, so tests and `wrangler dev` need no `.env`) and writes
absolute paths for `main` and the assets. The generated file has the top level (what `wrangler dev` runs: it mirrors the
environment you name, has no route, and is named `<worker>-dev`) and the sections `[env.demo]` and `[env.private]`, which
repeat bindings and variables because environments inherit neither. `workers_dev = false` and `preview_urls = false` are
set at the top and in each environment. Deploy with `wrangler deploy --env demo` or `--env private` (never done by these
scripts).

The code reads **only** these variables (the relay announces the resulting limits in `Joined.limits`). The Rust core
validates the numbers against the ceilings of `drafftink-protocol` and the Worker answers `500 bad_config` with the
reason when a value is wrong.

| Variable | Meaning | Demo | Private |
|---|---|---|---|
| `ROOM_TTL_SECS` | seconds from the last visit to the deletion; **empty: rooms never expire, no alarm** | `172800` | empty |
| `ROOM_LEAVE_GRACE_SECS` | seconds a room past its deadline lives on after the last connection closed | `60` | `60` (unused without a deadline) |
| `BLOB_STORE` | `do`: pictures are rows of the room's database. `r2`: phase 3P, not built (the Worker answers `500`) | `do` | `r2` |
| `ACCESS_REQUIRED`, `ACCESS_TEAM_DOMAIN`, `ACCESS_AUD` | Cloudflare Access (another module). While `ACCESS_REQUIRED` is true and no check is wired in, every request is refused | `false`, empty, empty | `true`, team, audience |
| `ALLOWED_ORIGINS` | comma separated origins that may open a WebSocket from a browser; empty: the page's own origin only | `https://drafft.ink` | `https://<domain>` |
| `MAX_ROOM_STATE_BYTES` | sealed bytes of snapshot plus log | 32 MiB | 256 MiB |
| `MAX_IMAGE_BYTES` | largest picture before sealing | 4 MiB | 4 MiB |
| `MAX_ROOM_IMAGE_BYTES`, `MAX_ROOM_IMAGES` | pictures in one room | 50 MiB, 200 | 50 MiB, 200 (see below) |
| `MAX_PEERS_PER_ROOM`, `ROOM_PRESENCE_BUDGET` | people per room, presence messages per second per room | 64, 40 (code defaults) | the same |
| `ALLOW_DELETE_ROOM` | whether a member may delete the room (`DeleteRoom`) | `false` | `true` |

The design table (19.8) wants 500 MiB and 2,000 pictures for the private room, but the protocol caps a room at 50 MiB and
200 pictures today and the core refuses more. The ceilings in `drafftink-protocol` must move first. Bindings: `ROOM` (the
Durable Object, migration `new_sqlite_classes = ["Room"]`), `ASSETS`, and in the private environment `BLOBS` (R2, bucket
from `PRIVATE_R2_BUCKET`).

`render-config.sh` reads these names from the environment: `MAIN`, `ASSETS_DIR`, `DEMO_<X>` and `PRIVATE_<X>` for each
variable above plus `WORKER_NAME` and `DOMAIN`, and `PRIVATE_R2_BUCKET`. `CF_WORKER_NAME` and `CF_DOMAIN` (the names the
Makefile uses today) are the defaults of the private name and domain. An empty value that is set on purpose (for
example `PRIVATE_ROOM_TTL_SECS=`) stays empty.

**The Makefile.** `make cf-config` still fills only `@CF_WORKER_NAME@`, `@CF_DOMAIN@` and `@ASSETS_DIR@`, which are not
the placeholders of the new template, so it writes an unusable file. It must call
`apps/drafftink-worker/scripts/render-config.sh private target/cloudflare/wrangler.toml` instead (the same `CF_*` values
are the defaults), and `cf-dev`, `cf-check` and `cf-deploy` must add `--env private`, run `npm run build:wasm` first, and
`cf-check` must keep the `dist/index.html` test. The template has no route at the top level on purpose.

## How a request is handled

- **Worker (`src/index.ts`).** A path that is not `/r/<16 bytes>/ws|f|f/<name>` or `/api/status` is a `404`. A bad room id
  is `404 room_missing`. Picture routes need `Authorization: Drafft-Member <base64url token>`: the Worker hashes the
  32 bytes (`checkMember` in the wasm core, `RoomId::accepts`) and answers `401` before any object is called; a bad name is
  `400`, a `Content-Length` over the sealed picture limit is `413` (so an over-size row never reaches SQLite, which would
  answer `SQLITE_TOOBIG` and a `500`). The Worker reads no body and hashes no payload, because the Free plan gives it
  10 ms of CPU and the spike measured 20 ms for 4 MiB of SHA-256 in wasm.
- **WebSocket upgrades are different.** The proof is in the first frame, and a socket cannot be handed to the object after
  the Worker has read it. So the Worker can check the room id and the `Origin` header (browsers only; native clients send
  none), but not membership: **a stranger's upgrade costs one object request, and the core refuses him at `Join`.** This
  deviates from design section 15 ("the Worker rejects a non-member without calling the object"), which holds for the
  picture routes only. A stranger gets no room task of his own beyond the connection and can store nothing.
- **`/api/status`** answers `{"status":"ok","env","now","reset_at_ms"}` and never touches an object. When an object call
  failed with a quota error, the Worker remembers it in the isolate and answers `503 {"error":"daily_limit","retry_after_s":N}`
  on this route and on the picture routes until 00:00 UTC, and accepts upgrades only to send `Error{DailyLimit, retry_after_ms}`
  and close with 1013. A second isolate learns it from its own first failed call.
- **Room (`src/room.ts`).** See below.

## The Durable Object

- **Hibernation.** `ctx.acceptWebSocket`, the handlers `webSocketMessage`, `webSocketClose`, `webSocketError`, and
  `setWebSocketAutoResponse("ping", "pong")` in the constructor: the runtime answers a client's keepalive without waking
  the object. Each socket carries an attachment (`{conn, connectedAt, lastMessageAt, client, order}`, about 100 bytes of
  the 16 KiB). The core is rebuilt on every wake (`Relay::restore_member` / `restore_waiting` for each socket of
  `ctx.getWebSockets()`), and `getWebSocketAutoResponseTimestamp(ws)` is the liveness of a socket that only pings
  (`Relay::heard`). What is lost by design: buffered presence, rate-limit fill, the grace of a room that was closing.
- **Storage.** Tables are created on every wake (`CREATE TABLE IF NOT EXISTS`), each with an `INTEGER PRIMARY KEY` or
  `WITHOUT ROWID`, so an insert writes one row: `room_meta` (one row, the core's meta in the shared codec of
  `drafftink_relay::codec`), `room_snapshot` (one row), `room_segments` (one row per flush, keyed by its first sequence
  number), `room_images` (name, size, bytes). A segment plus the meta, and a snapshot swap, are each one `transactionSync`.
- **Timers.** A wake within a minute is a `setTimeout`: flush (1 s after the first buffered record), presence tick, advice
  timeout, a late join, the grace after the last person left. A pending timer keeps the object awake, which is the point
  for a room with people. A farther wake (the deadline, 48 h out; the 1 h recheck of an expiring room) is `setAlarm` only,
  because an alarm does not block hibernation; the alarm is written only when it differs from the one set. `alarm()` rebuilds
  the core, feeds liveness, calls `on_wake(now)`, applies the output, and never throws (a failure sets a new alarm in
  5 minutes). Visits move the deadline; edits, snapshots, pictures and pings do not.
- **Deletion.** When the core reports `deleted`: close every socket, `deleteAlarm()`, `deleteAll()` (the explicit
  `deleteAlarm()` keeps this independent of the compatibility date; `deleteAll()` clears the alarm from 2026-02-24 on),
  forget the core. The next request wakes a fresh instance that creates empty tables, so a later open join is
  `RoomMissing`.
- **Failures.** Every storage exception is mapped: a quota-looking message becomes `DailyLimit` (an `Error` frame and close
  1013, HTTP `503 daily_limit`), anything else `Unavailable`. A handler never throws to the runtime: a failing frame closes
  its socket (1011) and the core is rebuilt on the next call. Close codes sent are legal ones only (1005, 1006 and 1015 are
  never passed to `ws.close`). The error texts of the Free plan are undocumented: `src/host-error.ts` matches a message
  by words (a best guess) and must be checked against a real account.

### The picture seam (phase 3P)

`BlobBackend` in `src/blobs.ts` is the interface between the core's synchronous `BlobHost` calls and the storage of the
sealed bytes. The demo's `DatabaseBlobs` writes the bytes into the manifest row. For R2: `put` writes the manifest row with
`data = NULL` and remembers the bytes; after the core returned (and answered `201`), `settle()` does `await bucket.put(key, bytes)`
with the key `r/<room>/<name>` and, when it fails, deletes the manifest row and throws so the reply becomes `503`; `purge()`
deletes `r/<room>/` when the room is deleted; downloads are read from R2 by the Worker after its membership check, so they
wake no object. The doc comment of `BlobBackend` says the same.

## Tests

- **Rust** (`cargo nextest run -p drafftink-worker`): settings validation, edge checks, the store adapters and the
  relay over an in-memory host (records, restore after a rebuild, snapshot swap, pictures, daily limit, deadline), and
  the fixtures test.
- **vitest** (`npm test`, miniflare, the real Worker, Room and wasm core; `test/worker.ts` subclasses the Room only to move
  its clock, count requests, and make storage fail):
  `routing` (bad ids and paths, 401 and 413 before any object using an object-call counter, `/api/status`, Origin),
  `room` (join, fan-out, Ack after the write, ping without a handler call, snapshot swap, hibernation by evicting the
  object, pictures, hostile frames), `retention` (deadline and alarm after a join, a later visit, edits do not move it,
  an early alarm re-arms, deletion at the deadline, expiring with someone connected and the 1 h recheck, the grace, a rejoin
  in the grace, a stale socket, the contrast with a pinging one, re-creation), `no-expiry` (no deadline and no alarm,
  `DeleteRoom`), `quota` and `quota-wake` (a used-up limit inside a room, in the Worker, and while a room wakes), and
  `canary`.
- **The canary** sends sealed records and a sealed picture that hold `CANARY-7f3a91c2`, and a sealed presence with the name
  `Zelda-Canary`; then it dumps every table of the room's SQLite (including `sqlite_master` and the key-value area), every
  frame each client sent and received and the HTTP replies, and asserts no hit for the marker, the name, the room key, every
  derived subkey, the file id and the SHA-256 of the png, each as raw bytes, hex, base64 and base64url. The membership
  token may be in the join frames and the `Authorization` header, never in storage. Further tests prove that the scan
  finds a deliberately leaky row, blob, table name, key-value entry and frame.
- Noise: miniflare prints `Application called abortAllDurableObjects()` when a test drops an instance, and an
  `uncaught exception ... the object is overloaded` line for the test that makes an object call throw. Neither is a failure.

## Parity suite

The same scenarios run against this Worker and against the other relays through the real native clients
(`apps/drafftink-server/tests/parity/`, see `docs/testing.md`, "Transport parity"). Each test starts its own `wrangler dev`
(the pinned one of `node_modules`, a config rendered by `scripts/render-config.sh`, a free port, a temporary
`--persist-to` folder) and stops its process group when it ends. It needs `npm ci` and `npm run build:wasm` here once.

```bash
DRAFFTINK_PARITY_WRANGLER=1 cargo nextest run -p drafftink-server parity                     # all targets, about a minute
DRAFFTINK_PARITY_WRANGLER=1 cargo nextest run -p drafftink-server parity::wrangler::expire   # one scenario on this Worker
cargo xtask worker                                                                           # the TypeScript tests, then the parity suite
```

Without the variable the Worker tests print `SKIPPED:` and pass. Output of each run: `target/parity-wrangler/<test>.log`
(printed when the test fails).
