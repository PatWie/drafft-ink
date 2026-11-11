# Collaboration server

Crate: `apps/drafftink-server` (binary `drafftink-server`). It implements `docs/protocol.md` with the types from `crates/protocol`, keeps one Loro document per active room, and serves the web client, so one binary is the whole deployment.

## Running

```bash
cargo run --release -p drafftink-server                      # 127.0.0.1:3030, rooms in ./data
cargo run --release -p drafftink-server --features sqlite,s3 # with the optional storage providers
```

| Path | What |
|---|---|
| `/ws` | WebSocket, binary protocol frames (JSON text frames only with `--debug-json`) |
| `/health` | `{"status":"ok","version":"…","rooms":n,"connections":n}`; lists no room ids |
| everything else | static files from `--web-dir` (default `./web-dist`); unknown paths fall back to `index.html` |

If the web directory is missing the server still starts, logs a warning, and serves only `/ws` and `/health`. `SIGINT` and `SIGTERM` stop the server cleanly: peers get `Unavailable`, every room is flushed and compacted.

## Configuration

Every flag has an environment variable (`DRAFFTINK_` + the flag name in capitals, `-` as `_`), so a container needs no arguments. Run `drafftink-server --help` for the full list.

| Flag | Default | Meaning |
|---|---|---|
| `--bind` | `127.0.0.1:3030` | Listen address. Anything other than loopback must be passed explicitly. |
| `--web-dir` | `web-dist` | Built web client. |
| `--storage` | `filesystem` | `filesystem`, `memory`, `sqlite`, `s3`. |
| `--allowed-origin` | none | Extra origins allowed to open `/ws` (repeat, or comma-separated). `*` allows all. |
| `--auth-token`, `--auth-token-file` | none | Turn auth on; accepted bearer tokens. |
| `--debug-json` | off | Accept JSON text frames on `/ws`. |
| `--max-rooms` / `--max-peers-per-room` / `--max-connections` | 1000 / 64 / 4096 | Capacity. Peers per room is at most 256. |
| `--max-frame-bytes` / `--max-update-bytes` | 16 MiB / 4 MiB | Must stay within the protocol ceilings. |
| `--max-updates-per-sec` / `--max-awareness-per-sec` | 60 / 20 | Token bucket per connection; the burst is twice the rate for updates. |
| `--max-room-bytes` | 256 MiB | A room stops accepting updates (`TooLarge`) at this stored size. |
| `--join-timeout-secs` | 10 | Close a connection that sends no `Join`. |
| `--flush-interval-ms` | 2000 | Batch window for store writes. 0 writes before every `Ack`. |
| `--room-idle-secs` | 600 | An empty room is saved, compacted and unloaded after this. |
| `--compact-entries` / `--compact-bytes` | 1000 / 8 MiB | Log size that triggers compaction. |
| `--room-expiry-days` | off | Delete rooms nobody wrote to for this long. |

## Storage providers

All providers implement `RoomStore` (`src/store/mod.rs`; contract in `docs/protocol.md` §6): a compacted snapshot plus an append-only update log. `tests/stores.rs` runs one conformance suite on each. A store must have one server writing to it.

| Provider | Flags | Notes |
|---|---|---|
| `filesystem` (default) | `--data-dir data` | `<dir>/<room>/snapshot.bin`, `updates/<n>.bin`. Every file is written to a temp name, fsynced, renamed, and the directory fsynced. Upper-case letters in room ids are stored as `~x` so case-insensitive filesystems keep rooms apart. |
| `memory` | | Lost on restart. |
| `sqlite` (feature `sqlite`) | `--sqlite-path` (default `<data-dir>/drafftink.sqlite3`) | WAL mode, `synchronous=FULL`; compaction is one transaction. Bundles SQLite. |
| `s3` (feature `s3`) | `--s3-bucket`, `--s3-endpoint`, `--s3-region`, `--s3-prefix`, `--s3-allow-http`, `--s3-virtual-hosted-style`, `--s3-disable-bulk-delete`; credentials from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN` | Objects `rooms/<room>/snapshot` and `rooms/<room>/updates/<n>`. Works with any S3-compatible service (path-style by default). Each flush is one PUT, so keep the flush interval above 0. |

Example with MinIO: `AWS_ACCESS_KEY_ID=… AWS_SECRET_ACCESS_KEY=… drafftink-server --storage s3 --s3-bucket drafftink --s3-endpoint http://minio:9000 --s3-allow-http`.

Durability: with the default 2 s flush interval, `Ack` means "merged", and a crash can lose the last 2 s of edits on the server. Clients still hold them and upload them when they rejoin. With `--flush-interval-ms 0` an `Ack` means the store has the update.

## How a room works

One task per loaded room owns the Loro document. Joins load the snapshot and log, import them, and answer with `Joined` and a `SyncResponse` computed from the client's version vector (updates when the diff is smaller than a snapshot, otherwise a snapshot). Updates are imported inside `catch_unwind`; bytes Loro rejects get `InvalidUpdate` and are not relayed. So does an update that raises `meta.schema` beyond the schema the server serves: Loro cannot take an import back, so the room document is rebuilt from a snapshot of the state before the update (cost: the size of the room, paid only for such an update). A relay goes to each peer through a bounded queue (256 messages and 32 MiB). A full queue drops the queued relays and sends `ResyncRequired`; the client replies with `RequestSync`. Awareness is relayed, never stored, and silently dropped above its rate.

A storage failure on load answers `Unavailable` and never overwrites stored data. A failed write is retried every second while peers keep editing in memory.

## Deployment

- **TLS**: the server speaks plain HTTP and WebSocket. Put a TLS-terminating reverse proxy in front and use `wss://`. Caddy: `draw.example.com { reverse_proxy 127.0.0.1:3030 }`. nginx needs `proxy_http_version 1.1`, `Upgrade`/`Connection` headers, the `Host` header passed through, and `proxy_read_timeout` above 60 s (the server pings every 20 s).
- **Same origin**: with the proxy passing `Host`, a page served by this server is allowed to open `/ws` without `--allowed-origin`. Add `--allowed-origin https://other.example` only for pages hosted elsewhere.
- **Binding**: behind a proxy on the same host keep the default loopback bind. In a container use `--bind 0.0.0.0:3030` and publish the port only to the proxy. Binding a non-loopback address logs a warning when auth is off.
- **Backups**: filesystem and sqlite are plain files (for sqlite copy with `sqlite3 .backup`, not while writing); S3 is the bucket.
- **One writer**: do not run two servers on the same store.
- **Logs**: `RUST_LOG=info` (default). Logs hold counts and errors, a short hash instead of room ids, no tokens and no document content.

## Security defaults

- Loopback bind by default; wide binds are explicit and warned about.
- Origin check on `/ws`: a request from a page whose origin is neither the server's own nor listed gets HTTP 403. On a loopback bind, "own origin" also requires a loopback host name, which blocks DNS rebinding. Requests without `Origin` (native clients) pass.
- Auth is off by default: a room id is a capability (128 random bits from the client). With tokens configured, `Join.auth_token` must match one (constant-time compare); tokens travel in the first binary frame, never in the URL, and are not logged. There are no per-peer permissions: anyone in a room can read and edit it.
- Every input is hostile: decoding is strict and bounded before allocation, text frames are refused unless `--debug-json`, rate limits per connection (repeated breaches close the connection), connection, room, peer and room-size caps, join timeout, idle timeout.
- Static files are served by `tower-http` `ServeDir` (no path traversal), with `X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`. No CORS headers are sent.
- `unsafe` is forbidden and `unwrap`/`expect`/`panic!` are denied in the crate's non-test code.

## Tests

`cargo test -p drafftink-server --all-features` runs unit tests and the integration suites in `apps/drafftink-server/tests/`: `sync.rs` (join, catch-up, live edits, reconnect, awareness), `hostile.rs` (limits, garbage frames, slow reader, Origin, auth, static files), `persistence.rs` (restart and crash per provider, compaction, expiry, failing stores) and `stores.rs` (the conformance suite). The S3 cases run against an in-process S3 mock (`tests/common/mock_s3.rs`) and, when `DRAFFTINK_TEST_S3_ENDPOINT` is set, also against a real service (`DRAFFTINK_TEST_S3_BUCKET`, `DRAFFTINK_TEST_S3_ACCESS_KEY`, `DRAFFTINK_TEST_S3_SECRET_KEY`). Example: `docker run -p 9000:9000 -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin123 bitnamilegacy/minio`, create the bucket, then set the variables.
