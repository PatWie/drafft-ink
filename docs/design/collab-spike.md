# Collaboration spike (phase 0): results

Date: 2026-11-22. Status: throwaway spike, nothing deployed. Everything here ran on the local runtime (`wrangler dev`, which runs workerd) with wrangler 4.166.0, Node 22.23.5, rustc 1.100. No Cloudflare login or credentials were used. The spike code is in `/tmp/cf-spike` and is not part of the repo:

- `/tmp/cf-spike/ts`: Worker plus SQLite Durable Object in TypeScript (`src/index.ts`, `wrangler.toml`, tests `t1.mjs` to `t8.mjs`).
- `/tmp/cf-spike/shell`: the same Worker with a Rust core compiled to wasm (`core/`, `pkg/`).
- `/tmp/cf-spike/rs`: the same Worker in Rust with workers-rs (`room/`, `RESULTS.md`).
- `/tmp/cf-spike/*.md`: copies of the Cloudflare docs pages read today (fetched as markdown).

"Verified" means I saw it happen locally. "Docs" means a Cloudflare page says so. "Unverified" means only a real account can answer it. The local runtime does not enforce billing, quotas, or the Free-plan CPU limit, so those are always unverified.

## Summary

| # | Question | Answer |
|---|---|---|
| 1 | Hibernation and `ping`/`pong` | Works. The auto-response answers while the object is evicted and does not run the constructor or any handler. |
| 2 | Alarms | 48 h ahead works locally. `deleteAll()` also clears the alarm on compatibility date 2026-02-24 or later, and does not on earlier dates. Keep `deleteAlarm()` first anyway. |
| 3 | 4 MiB PUT, limits | 1 row, 17 to 25 ms locally. Row limit 8 MB (docs). 16 MiB and 32 MiB WebSocket messages work, 33 MiB closes with 1009. No chunking needed. |
| 4 | Assets plus Worker | Works as the design wants. Use `not_found_handling = "single-page-application"` so unknown paths do not wake the Worker. |
| 5 | workers-rs or TS shell | Both work. Recommendation: TS shell around the Rust core. |
| 6 | Access and WebSocket | Not testable locally. Docs are silent. First real-account test is listed below. |
| 7 | Free-plan CPU and quota errors | Docs only. The Free CPU limit for objects is contradictory in the docs. Error texts for the Free daily limit are undocumented. |
| 8 | `workers_dev`, `preview_urls` | Both keys confirmed. Both inherit into environments. |

## 1. SQLite Durable Object, hibernation, `ping`/`pong`

What I built: Worker `fetch` routes `/r/<id>/ws` to a Durable Object (`idFromName`), the object is declared with `[[migrations]] new_sqlite_classes = ["Room"]`, accepts sockets with `ctx.acceptWebSocket`, keeps `{clientId, n}` with `serializeAttachment`, and calls `ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping","pong"))` in the constructor. The constructor and each handler log a line.

Test `t1.mjs` (one client, log lines from `dev.log`):

```
14:59:19 send "hello1"  -> reply from instance 39e5ee49, n=1       (log: CTOR 39e5ee49, webSocketMessage n=1)
14:59:19 send "ping"    -> "pong"                                  (log: nothing)
         ... idle 15 s, no events ...
14:59:35 send "ping" x3 -> "pong" x3                               (log: nothing; no CTOR)
14:59:36 send "hello2"  -> reply from instance 7c7747ff, n=2       (log: CTOR 7c7747ff ws=1, webSocketMessage n=2)
```

- The object was evicted during the 15 s idle period: `hello2` ran in a new instance (new random id, constructor ran again, `ctx.getWebSockets()` returned the socket).
- The three pings during hibernation got `pong` with no constructor and no handler call. So the auto-response does not wake the object (verified locally).
- The attachment survived: `clientId` was the same and `n` went from 1 to 2.
- `webSocketClose` ran with code 1000. Calling `ws.close(1005)` inside it throws (1005 is not a legal code to send). Echo only legal codes.

New fact that helps the design: `ctx.getWebSocketAutoResponseTimestamp(ws)` returns the time of the last auto-response for that socket, without waking anything (test `t7.mjs`: `null` before any ping, then the ping time, also after hibernation). The retention check in section 10.2 ("only sockets heard from within 2 minutes count as live") can use it, because a client's `ping` never reaches `webSocketMessage`.

Other facts:
- Attachments are limited to 16,384 serialized bytes. A 16,300 byte string was accepted, 16,384 failed with "'attachment' cannot be larger than 16384 bytes" (test `t8.mjs`). The limit counts the structured-clone overhead, so keep real data well below.
- Hibernation needs no `setTimeout`/`setInterval`, no open outbound connection and no pending I/O (docs: https://developers.cloudflare.com/durable-objects/concepts/durable-object-lifecycle/). The 100 ms presence tick in section 11.3 therefore keeps a room awake while it runs. That matches the design (rooms with people stay awake). Hibernation happens after 10 s idle (docs; consistent with my 15 s test).
- Billing (docs, pricing page footnote 3): auto-response messages "will not incur additional wall-clock time". Whether they also count as requests (the 20:1 rule) is not stated. Unverified.

## 2. Alarms

All on local workerd, test commands in `/tmp/cf-spike/ts` (`curl localhost:8701/r/<id>/dbg/...`).

- `setAlarm(now + 48 h)` is accepted, and `getAlarm()` returns the time (`1795532384542` = Tue 24 Nov 2026 14:59 UTC). The docs name no maximum horizon (https://developers.cloudflare.com/durable-objects/api/alarms/). That the real service keeps a 48 h alarm is unverified, but nothing in the docs forbids it. Also: the alarm fired in a fresh instance after the object had been evicted (control run: alarm at 15:00:15.574 fired with a new constructor call).
- `deleteAll()` and a pending alarm, by compatibility date. This is now documented, and I reproduced both sides:

| Compatibility date | `deleteAll()` without `deleteAlarm()` | Evidence |
|---|---|---|
| `2026-11-01` | alarm cleared (`getAlarm()` is `null`, never fires) | `alA` run: no "ALARM FIRED" line |
| `2026-01-15` | alarm stays (`getAlarm()` still returns the time) and **fires** on the emptied object | `oldA` run: "ALARM FIRED at 15:02:30.592" |
| `2026-01-15` plus flag `delete_all_deletes_alarm` | alarm cleared | `oldB` run: `null`, no fire |

  Source: SQLite storage API page, `deleteAll`: "For Workers with a compatibility date of 2026-02-24 or later, `deleteAll()` also deletes any active alarm. For earlier compatibility dates, `deleteAll()` does not delete alarms" (https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/). **Decision: keep calling `deleteAlarm()` before `deleteAll()`, and set `compatibility_date` to 2026-02-24 or later.** The call is cheap and makes the code independent of the date.
- After `deleteAll()` the next request ran in a new instance (new constructor call). Treat the object as gone after `deleteAll()`: create tables in a function that runs on every wake, not once, and do not use the old `this` state afterwards. (The workers-rs spike hit this too: its tables were gone after `delete_all` until re-created.)
- An alarm firing wakes an evicted object in a new instance, so `alarm()` must reload its state from SQLite. It receives no WebSocket events.
- The docs say a spend cap holds alarms and deletes those older than 7 days (https://developers.cloudflare.com/durable-objects/platform/spend-management/). That is already in section 11.5.

## 3. A 4 MiB PUT, row limits, 16 MiB WebSocket messages

- 4 MiB `PUT /r/put4/f/img1` through the Worker (the Worker forwards the same `Request` to the object with `stub.fetch(req)`): HTTP 200 in 17 to 25 ms inside the object, **1 row**, read back with an identical SHA-256 (`dabefd94...`). With 1 MiB chunks it is 4 rows. No reason to chunk.
- Rows written (this is what the Free plan counts, 100,000 per day): the 4 MiB insert reported `rowsWritten = 2` with `name TEXT PRIMARY KEY`, because the primary-key index is a second row. **`INTEGER PRIMARY KEY` and `WITHOUT ROWID` both report 1** (`/dbg/rw`: `int_pk 1, without_rowid 1, text_pk 2, delete 1`). Use one of those for every table in the relay, to halve the row-write cost. Whether the real billing counts index rows the same way is unverified (the pricing page says rows written "match D1", and D1 counts index writes).
- Limits (docs, https://developers.cloudflare.com/durable-objects/platform/limits/, updated 2026-11-13): SQLite string, BLOB or row: **8 MB**. Statement length 100 KB. Bound parameters 100. Received WebSocket message: **32 MiB**. KV-style value in a SQLite object: key plus value 8 MB.
- Local row test (`/dbg/rowtest`, a BLOB of N bytes): OK at 1, 2, 4, 7, 8,000,000, 8,388,608 and 8,388,609 bytes. **`SQLITE_TOOBIG` at 9,437,184 (9 MiB) and 16 MiB.** The local ceiling sits between 8 MiB + 1 and 9 MiB, so the "8 MB" is not smaller than 8 MiB locally. A 3 MiB string also fit. The 12 MiB image PUT returned a 500 with `SQLITE_TOOBIG`: the Worker needs an early size check and a clean 413 for anything over `MAX_RECORD_BYTES` and the image limit (the design already says "early reject").
- WebSocket messages (`t3.mjs`, binary and text, the object replies with the size it saw): 16 MiB binary and 16 MiB text are delivered intact. 31 MiB and 32 MiB binary are delivered. **33 MiB is refused**: the object logs `webSocketError ... statusCode = 1009 ... Message is too large` and the client sees the socket go to closing. This matches the docs (32 MiB). Transfer time on a real network for a 15 MiB snapshot is unverified (the local test only shows delivery).
- Real-account unknowns for this item: whether a 4 MiB request body costs more than 10 ms of Free-plan CPU (see question 7), and the real behaviour at 8 MB (the local limit is not the production limit).
- `CF-Connecting-IP` inside the object (named in the phase 0 list): the header is present in the object's `fetch` request when the Worker forwards the original `Request` (value `127.0.0.1` locally, `/dbg/stats`). For a WebSocket it exists only on the upgrade request, so the object must read it there and keep it in the attachment. Only a real account shows the real value.

## 4. Static assets and the Worker in one deploy

Config (`/tmp/cf-spike/ts/wrangler.toml`):

```toml
[assets]
directory = "./public"
binding = "ASSETS"
not_found_handling = "single-page-application"
run_worker_first = ["/r/*", "/api/*"]
```

Log evidence (a `WORKER fetch ...` line is printed on every Worker invocation; none appeared for static files):

| Request | Result | Worker ran? |
|---|---|---|
| `/` , `/app.js` | 200 static | no |
| `/whatever/deep`, `/missing.js` (with `not_found_handling = "single-page-application"`) | 200 `index.html` | no |
| `/api/status` | 200 JSON from the Worker | yes |
| `/r/x/dbg/stats`, `/r/x/f/none` | handled by the object | yes |
| `/r/hib/ws` (WebSocket upgrade) | 101, works as in question 1 | yes |
| `/missing` without `not_found_handling` | 404, **the Worker ran** | yes |

- So the same-origin layout works. With no `not_found_handling` setting, a miss on a static path invokes the Worker (and so counts as a Worker request). Use the SPA setting for drafft.ink: `/r/<id>` is never navigated to as a page (the room key is in the `#` fragment), so a SPA fallback is harmless.
- `/api/zzz` matched `run_worker_first`, so the Worker saw it and answered itself.
- Worker-first also covers `/r/*` WebSocket upgrades; no extra config.
- Not tested: negated patterns (`!/path`) in `run_worker_first`. The design does not need them.
- Docs on billing: static asset requests that do not run the Worker are free and unlimited (https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/). Spending-limit page: "Static assets served without running your Worker continue to be available. Requests ... routed through `run_worker_first` can fail" (https://developers.cloudflare.com/workers/platform/spend-management/). That is the behaviour the design wants.
- Unverified (account): that the daily Free quota of 100,000 Worker requests does not count static hits (docs say so).

## 5. Rust Worker (workers-rs) or TypeScript object around a Rust core

Same feature set in each: routes, WebSocket upgrade, hibernation handlers, attachment, `ping`/`pong` auto-response, SQL BLOB write and read, alarm, `deleteAll`.

| | workers-rs 0.9.0 | TS object plus Rust core in wasm |
|---|---|---|
| Version | `worker` 0.9.0, published 2026-11-11 (crates.io and GitHub release v0.9.0). Earlier: 0.8.7 on 2026-09-25, 0.8.6 on 2026-09-15, 0.8.5 on 2026-06-12. | wrangler 4.166.0, wasm-bindgen 0.2.130 |
| Beta label | The repository README does not call the crate beta (only queues "beta", D1 "alpha", RPC "experimental"). The Cloudflare docs page lists Rust support as Beta, as the design says. | not applicable |
| SQLite object | works | works |
| Hibernation (`accept_web_socket`, `websocket_message`, `websocket_close`) | works | works |
| Attachments | works, serde based (`serialize_attachment<T: Serialize>`) | works |
| `ping`/`pong` auto-response | works (`set_websocket_auto_response`), handler bypassed | works |
| Auto-response timestamp (`getWebSocketAutoResponseTimestamp`) | **missing**: not in worker 0.9.0 or worker-sys, and `State` exposes no raw JS handle, so adding it needs an upstream change or unsafe code | works |
| Alarms (`set_alarm`, `alarm()`, `delete_alarm`) | works; `alarm()` must return `Result<Response>` | works |
| SQL | works: typed cursor, `raw()`; BLOB reads go through `raw()`; `Integer` is lossy beyond 2^53 | works (JS values) |
| R2 streaming put (open item 12 in section 17 and 19.12) | the source shows `Data::ReadableStream` and `Data::Stream(FixedLengthStream)` in `worker/src/r2/mod.rs`, so streaming looks supported (not run) | works (native JS) |
| Local build, cold | 24.4 s (`worker-build --release`; 21 s on a second cold build); incremental about 4 s | Rust core 6.7 s cold; TypeScript is bundled by wrangler, no step |
| Bundle (`wrangler deploy --dry-run --outdir`) | **314 KiB** total upload, **87.7 KiB gzip** (wasm 273,729 B with wasm-opt, 328,851 B without) | **58.7 KiB**, **20.9 KiB gzip** (JS 13 KB, wasm 47 KB) |
| Effort to a working version | about 15 min, 0 compile errors | about 25 min including the wasm-bindgen import |
| Awkward bits | sync `new()` (async setup needs `block_concurrency_while`, new in 0.9.0); `.unwrap()` inside `accept_web_socket` and `set_websocket_auto_response` (a failure aborts the instance); no hand-written router for object paths; every body copies between JS and Rust | the wasm import and `initSync` glue; types at the boundary are numbers and bytes only |

Sizes are tiny compared to the limit (Worker size 64 MiB on Free and Paid, https://developers.cloudflare.com/workers/platform/limits/). Neither is close, so size does not decide. Details of the Rust build are in `/tmp/cf-spike/rs/RESULTS.md`.

The Rust core in the TS shell (`/tmp/cf-spike/shell/core/src/lib.rs`, 80 lines): a sans-IO `Core` with `joined(now)`, `closed(now)`, `wake(now) -> Action`, `room_id(token)` (SHA-256), `check_record(bytes)`. The host passes `now_ms`; the core never reads a clock. The TS object loads it with `import wasmModule from ".../relay_core_bg.wasm"` then `initSync({module})`, and it ran the hibernation test, the alarm test and a 4 MiB message through `check_record` (`t5.mjs`, `t6.mjs`). Measured through the 4 MiB hash: first call 189 ms (wasm tier-up in the local runtime), later calls 18 to 28 ms. That is a local wall-clock number, not Free-plan CPU, but it shows one thing clearly: **do not process payload bytes in wasm on the hot path** (hashing 4 MiB with the `sha2` crate at opt-level "s" costs about 20 ms, over the 10 ms Free CPU figure in question 7). The relay never needs to: it stores sealed bytes and hashes only the 32-byte membership token. Use `crypto.subtle` for any other hash and for the Access JWT signature (section 19.5), which is plain JS in the shell and needs `web-sys` glue in workers-rs.

**Recommendation: the TypeScript Durable Object and Worker shell, with the Rust `relay` core compiled to wasm.** Reasons:
1. `getWebSocketAutoResponseTimestamp` is missing in workers-rs, and the retention rule needs it (question 1). Platform features arrive in JS first; Rust follows with a release lag (about a month between releases).
2. The shell is small (about 150 lines) and the rules stay in the Rust core, which is what the design wants anyway. The Worker host is `L7` glue.
3. R2 streaming, Web Crypto for the Access JWT, and miniflare tests (`@cloudflare/vitest-pool-workers`) are native in JS.
4. Smaller bundle (59 KiB against 314 KiB) and no 20 s Rust build in the deploy loop, though neither matters much.
5. workers-rs worked on the first try, so this is not a "workers-rs is broken" finding. If the project later wants one language, the sans-IO core makes the move cheap: the host is the only part that changes. Open question 8 in the roadmap ("Proposed: TypeScript shell") stands.

## 6. Cloudflare Access on a WebSocket upgrade, service-token headers

Cannot be tested locally. What the docs say today (read 2026-11-22):

- Worker-level Access (the "Protect this Worker" shortcut) does not support WebSockets: the upgrade fails with 403. Use a hostname-based application instead (https://developers.cloudflare.com/workers/configuration/cloudflare-access/). Unchanged from section 19.3.
- Nothing in the Access pages (authorization cookie, service tokens, session management, self-hosted app) mentions WebSockets, cookies on upgrades, or service-token headers on upgrades. The service-token page says to add `CF-Access-Client-Id` and `CF-Access-Client-Secret` "to the headers of any HTTP request", and offers a single-header mode (`read_service_tokens_from_header`, for example `Authorization`) for clients that can send one header only. An upgrade is an HTTP request, so it should work, but it is not stated.
- Session management: an expired session returns a 302 to the login page, or a 401 when the request has `X-Requested-With: XMLHttpRequest` (https://developers.cloudflare.com/cloudflare-one/access-controls/access-settings/session-management/). Nothing is said about connections that are already open.
- Same-origin cookies are sent on a browser's WebSocket handshake by RFC 6455 section 4.1. A Cloudflare Agents guide states the same for same-origin Workers ("If the client and server share the origin, the browser will send cookies during the WebSocket handshake", https://developers.cloudflare.com/agents/guides/cross-domain-authentication/). A community report describes a stale `CF_Authorization` cookie leaving a page with failed WebSocket reconnects after the session expired, which fits "an open socket survives, the next upgrade fails" (GitHub: mayerwin/HA-Cloudflare-Access-Recovery). Neither is an Access statement.

**First real-account test (private account, hostname-based Access application on a test hostname, `workers_dev` off):**
1. In a signed-in browser, open the page and then `new WebSocket("wss://<host>/r/<id>/ws")`. Check that the upgrade returns 101 and that the Worker sees the `Cf-Access-Jwt-Assertion` header on the upgrade request (log its presence, not its value).
2. In a signed-out browser or `curl` without credentials, send an upgrade request and note the response: 302 or 401 or 403 (this decides the text the app shows).
3. From a Rust client (tungstenite) or `wscat`, send the upgrade with `CF-Access-Client-Id` and `CF-Access-Client-Secret` against an application with a Service Auth policy. Check 101 and that the JWT has `common_name` and no `email`. Repeat with a token that is not in the policy (expect a refusal) and with the single-header mode.
4. Open a socket, set the application's session duration to the shortest value, wait until it expires, and see whether the open socket keeps working and whether a new upgrade fails with the 302 or the 401. Also see whether an Access-blocked request counts as a Worker request in the dashboard.
5. Strict service-token mode (accounts created on or after 2026-10-05): confirm a service token is refused unless a Service Auth policy lists it.

## 7. CPU limits on Free and the error shapes

All from the docs. Nothing here can be measured locally: workerd did not enforce any CPU or request quota, and `wrangler dev` has no billing.

CPU:
- Workers Free: **10 ms CPU per HTTP request**; Paid: 30 s default, up to 5 min (https://developers.cloudflare.com/workers/platform/limits/). Waiting on I/O (storage, fetch) is not CPU time. Over the limit the client gets Cloudflare error 1102 "Worker exceeded resource limits".
- Durable Objects (limits page, 2026-11-13): "CPU per request: 30 seconds (default)", per HTTP request, WebSocket message or alarm, extendable with `[limits] cpu_ms` (https://developers.cloudflare.com/durable-objects/platform/limits/). The same page's FAQ says objects "have the same per invocation CPU limits as any Workers do". **These two statements conflict for the Free plan**, and no page states which one wins for an object on Free. Unverified. The Worker in front of the object is a Worker, so its 10 ms applies in any case: the 4 MiB PUT must not spend more than 10 ms of CPU in the Worker (forwarding the body as a stream, without parsing it, costs close to nothing; I did not measure CPU).
- Design effect: keep payload processing out of the hot path (see question 5), and in the Worker only forward, check headers, and verify the Access JWT with Web Crypto.

Errors seen by a client (docs):

| Situation | Plan | What happens | Source |
|---|---|---|---|
| Daily Workers requests exhausted (100,000, reset 00:00 UTC) | Free | Cloudflare error 1027 page (or fail open, per the Worker's setting) | workers/platform/limits, "Daily requests" |
| Workers CPU or memory exceeded | both | error 1102 | workers/platform/limits |
| Durable Object free-tier limit (requests, duration, rows) | Free | "further operations of that type will fail with an error". **The error text, and the close code seen on an open WebSocket, are not documented.** Unverified. | durable-objects/platform/pricing |
| SQLite storage full (10 GB, or the Free storage limit) | both | exception message `database or disk is full: SQLITE_FULL`; reads and deletes still work | durable-objects/platform/limits, FAQ |
| WebSocket message over 32 MiB | both | socket closed with 1009 (verified locally) | limits page, `t3.mjs` |
| Durable Object spending cap | Paid | `Exceeded the account's spending cap for Durable Objects.` on new stub requests, SQL queries, `setAlarm`; open sockets keep working; held alarms older than 7 days are deleted | durable-objects/platform/spend-management and observability/troubleshooting |
| Workers spending cap | Paid | HTTP error 1103; requests that go through `run_worker_first` fail, static files are served | workers/platform/spend-management |
| Object overloaded (over about 1,000 requests a second) | both | "overloaded" exception | durable-objects/platform/limits, FAQ |

Still open, and only the account can answer: the real error text and HTTP status or close code at each Free daily limit (requests, duration, rows written); whether spending caps exist on Free (the docs describe paid usage only); whether the Free plan gives an object 10 ms or 30 s of CPU; the SQLite storage limit on Free (the limits table says 5 GB per account, the FAQ says 1 GB per object: conflicting).

## 8. `workers_dev = false` and `preview_urls = false`

- Both keys exist in wrangler 4.166.0 and are booleans. Configuration docs: `workers_dev` "defaults to `true` when the configuration has no `route` or `routes`, and `false` otherwise"; `preview_urls` "enables Version URLs and workers.dev Preview URLs; if omitted, Wrangler does not change an existing setting; if no setting exists, its initial value depends on `workers_dev`" (https://developers.cloudflare.com/workers/wrangler/configuration/).
- Check in the installed wrangler (`node_modules/wrangler/wrangler-dist/cli.js` lines 10856-10857): both are read with `inheritable(...)`, so an environment inherits the top-level value, and an environment may override it. `wrangler deploy --dry-run` accepts the config with no warning, and a misspelled key (`preview_url`) prints `Unexpected fields found in top-level field: "preview_url"`, so the real spelling is confirmed by wrangler itself.
- A config with `[env.private]` that sets `routes = [{ pattern = "canvas.example.com", custom_domain = true }]`, `workers_dev = false` and `preview_urls = false`, plus its own `[env.private.assets]`, bindings and migration, passes `wrangler deploy --dry-run --env private` (`/tmp/cf-spike/ts/w-env.toml`).
- Because a custom-domain route already turns `workers_dev` off by default, but `preview_urls` stays "unchanged or follows workers_dev", **set both keys explicitly in the top level and in every environment**. Wrangler also warns about a "mixed state" (`workers_dev` and `preview_urls` different) on non-CI deploys.
- Unverified (account): that a deploy with both keys false removes an existing `*.workers.dev` route and preview URLs, and that an already-deployed Worker stops answering there. Check by requesting the `workers.dev` URL after the first deploy.

## What this changes in `collab-e2e.md`

1. Section 10.2 and 17 item 10: the `deleteAll()`-and-alarm question has an answer (compatibility date 2026-02-24 and later: it deletes the alarm). Still call `deleteAlarm()` first.
2. Section 10.2: use `ctx.getWebSocketAutoResponseTimestamp(ws)` as `last_heard_ms` for sockets that only send `ping`. Put the connect time and the last non-ping message time in the attachment.
3. Section 14: set the Worker language to the TypeScript shell; workers-rs 0.9.0 lacks the timestamp call. `RelayStore` tables use `INTEGER PRIMARY KEY` or `WITHOUT ROWID` (1 row written per insert, not 2).
4. Section 11.1: the DO CPU entry stays "unverified"; the pages conflict.
5. Section 8 and Worker routing: reject over-size bodies in the Worker with a 413 before the object (an over-size row gives `SQLITE_TOOBIG` and a 500).
6. Add `not_found_handling = "single-page-application"` next to `run_worker_first` in section 14 and phase 4, and `compatibility_date >= 2026-02-24`.

## Real-account checks (the list for phase 3 and 3P)

1. Free-plan CPU: put a 4 MiB body and a 15 MiB WebSocket message through a deployed Worker and object on the Free plan; check "CPU time" in the dashboard and whether error 1102 appears. Decides the 10 ms against 30 s question for objects.
2. Free daily limits: exhaust (or read the usage meters for) DO requests, rows written and duration on a throwaway account; record the exact exception text, HTTP status, and WebSocket close code for each. Also see what the Worker sees from `stub.fetch` and from `webSocketMessage` when a storage write fails.
3. Spending caps on Free (does the dashboard offer one), and on Paid the code seen at the cap (`Exceeded the account's spending cap for Durable Objects.`).
4. A 48 h alarm on the real service: set it, read `getAlarm()`, and look at it after a day. Also `deleteAll()` without `deleteAlarm()` on the real service with the final compatibility date.
5. Auto-response billing: whether `ping` auto-responses appear as DO requests in the usage meter, and whether an idle object with only pings goes to zero duration.
6. Real SQLite row ceiling: store 8 MB, 8,000,000 and 8,388,608 byte blobs in one row.
7. `CF-Connecting-IP` as seen by the object on a real upgrade request.
8. Access tests from question 6 (cookie on upgrade, service token on upgrade, session expiry with an open socket, 401 with `X-Requested-With`, strict service-token mode).
9. `workers_dev` and preview URLs actually off after the first deploy of the private environment; custom-domain deploy with the planned token scopes.
10. Static assets: Worker request counter does not move for static hits, and does not move for Access-blocked requests.
11. Bundle upload on Free with the real relay core (the 64 MiB Worker size is documented; startup time under 1 s is the real constraint for the wasm core).

## Sources

Cloudflare pages read on 2026-11-22: durable-objects/platform/limits, durable-objects/platform/pricing, durable-objects/platform/spend-management, durable-objects/api/sqlite-storage-api, durable-objects/api/alarms, durable-objects/best-practices/websockets, durable-objects/concepts/durable-object-lifecycle, durable-objects/observability/troubleshooting, workers/platform/limits, workers/platform/spend-management, workers/wrangler/configuration, workers/configuration/cloudflare-access, cloudflare-one/access-controls/{service-credentials/service-tokens, access-settings/session-management, applications/http-apps/authorization-cookie}, agents/guides/cross-domain-authentication. Code: https://github.com/cloudflare/workers-rs at tag v0.9.0 (`worker/src/durable.rs`, `worker/src/r2/mod.rs`, `worker-sys/src/types/durable_object/state.rs`). RFC 6455 section 4.1.
