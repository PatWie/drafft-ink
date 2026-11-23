# Collaboration client (crate `drafftink-collab`, L4)

The client side of `docs/protocol.md`. The server is `apps/drafftink-server`, run as `drafftink-headless serve` (`docs/server.md`), the document is `LoroStore` (`docs/crdt.md`). The engine (L5) owns a `CollabSession` and drives it; this crate does not know about commands, the scene, egui or the clock. Builds for wasm32.

The engine side (the `collab` command family, `Session::collab_poll`, the `Net` port) is described in `docs/engine.md` section 4, "Collaboration".

## Parts

| Part | What it is |
|---|---|
| `CollabClient` | Sans-IO state machine. Inputs: transport events, `tick(now_ms, &mut LoroStore)`, local calls. Outputs: `poll_output()` (connect, send a frame, close), `take_events()`, `next_wake_ms()`. No socket, no clock, no document of its own. |
| `Transport` | The frame pipe: `open`, `send`, `close`, `poll`. `NativeTransport` (background thread, tokio, tungstenite, rustls), `WebTransport` (browser `WebSocket`, wasm only), `loopback()` (in memory, for tests). `set_access` gives it the service token of a private server (default: ignored, as in the browser). |
| `CollabSession<T>` | A client plus a transport. `pump(now_ms, &mut store)` moves bytes both ways and returns `Pump { events, next_wake_ms, redraw }`. |
| `LocalPresence`, `PeerTable` | Ephemeral presence: cursor, selection, viewport, user, laser trail, peers with timeouts. Never enters the document. |
| `ServerUrl`, `AuthToken` | The checked address and the secret token. |
| `AccessCredentials`, `AccessOrigin`, `AccessGrant`, `AccessDenial` | The service token of a private server, the origin it belongs to, the two bound together (the only way to read the token out), and why a server's gate turned a request away. See "Access service token" below. |

| `SealedClient` | The same kind of state machine for an end-to-end encrypted room (protocol v2, module `sealed`). See "Encrypted rooms" below. |
| `RoomClient` | One type for the session and the engine: `Plain(CollabClient)` or `Sealed(SealedClient)`. Every method forwards. |
| `BlobTransport` | The second pipe, for the pictures of an encrypted room: plain HTTP requests, `send` and `poll`. `NativeBlobTransport` (worker threads, `ureq` with rustls and ring), `WebBlobTransport` (browser `fetch`, wasm only), `loopback_blobs()` (in memory, for tests). |

## Encrypted rooms

`SealedClient` is built from a `SealedConfig { url, key, client_id, user, mode, resume, .. }`. The room id and the path `/r/<id>/ws` come from the key. It seals every update, snapshot and presence message with `drafftink-seal` and opens everything it receives, and it keeps three things that the plaintext client gets from its server:

- **`relay_vv`**: the version of what the relay is known to hold (the end version of every applied record and snapshot, and of every acknowledged update of its own). The relay cannot say which operations a client lacks. After every catch-up the client uploads `export_updates_since(relay_vv)`, one step that repairs offline edits, a lost `Ack`, a rate limit and a crashed tab.
- **`last_seq`**: the highest sequence number held with none missing below it. A reconnect joins with it and gets only what is new. A client that remembers a number above the room's head, or that created the room, forgets both (the room is new under the old id).
- **Counters**: one per kind of record it makes. A jump in the update counter of a sender, or a hole in the sequence numbers that stays for 3 s (an acknowledgement of its own record fills one), makes it ask for the records after `last_seq`.

On advice from the relay it seals a snapshot with the sequence number it covers and swaps it in by compare-and-set (`PutSnapshot`). A room that is empty (a new one, or an expired one restored from a local copy) is seeded the same way: one client wins, the others rejoin and merge. A record that cannot be opened, or that Loro refuses, is skipped, counted (`skipped()`) and shown once. `resume()` gives `{last_seq, relay_vv}` for the workspace record; `expires_at_ms()` and `expiring()` carry the retention state (`CollabEvent::Retention`).

The keepalive is the text frame `ping` (`Transport::send_text`) and the answer `pong` (`TransportEvent::Text`). Not built yet: the peer-to-peer fallback when the relay's storage fails (a `Direct` request goes unanswered) and the crash fence for a malformed update. Pictures are in the next section.

## Pictures in encrypted rooms

In an encrypted room the document holds only `{mime, size}` of a picture. The bytes are sealed with the room's image key and stored on the relay under a keyed name (`docs/design/collab-e2e.md` section 9). The sealed client does the transfer; the engine owns the local copy of the bytes. The client is sans-IO like the rest: it emits `BlobRequest`s and consumes `BlobResponse`s, and `CollabSession::set_blobs` plus `pump` move them through a `BlobTransport`. A plaintext room embeds pictures in the document, and its client ignores all of this (`images()` is zero).

| Call | Meaning |
|---|---|
| `offer_image(now, id, bytes)` | "I hold these bytes." Uploads them unless the relay has the name. The same picture twice is one name and one request. Offering again retries a refused one. |
| `want_image(now, id)` | "Show me this picture." Downloads it, opens it, checks `SHA-256(bytes) == id`, and tells the engine with `CollabEvent::Image(ImageEvent::Arrived { id, bytes })`. |
| `images()` | `ImageProgress { uploading, downloading, waiting, failed, room_images, room_image_bytes }` for a status line. The room totals come from the list and from each `201`. |

What the client does:

- **One list per connection.** When the client is online it asks `GET /r/<room>/f`. An offer whose name is listed is done; the others upload (`PUT`). Bytes of an offer are let go once the relay has them, so after a room expired and was made again the engine offers its pictures again from its cache.
- **Limits.** At most 3 downloads and 2 uploads in flight. An offer above `max_image_bytes` from `Joined.limits` is `ImageEvent::Rejected { why: TooLarge }` at once, with no request. The relay's own answers `413` and `507` end an upload for good (`Rejected { TooLarge | RoomFull }`). The room total and the picture count are the relay's to check (`507`).
- **Retries.** `404` on a download (the manifest can reach a peer before the upload does), `429`, `503`, any other status and network failures are tried again after 1, 2, 4, 8 ... seconds, at most 2 minutes. `Retry-After` of a `429` is honoured (at least 1 s, at most 2 minutes). `401` is final (no event; counted in `failed`). A `PUT` whose answer was lost (network failure) is checked with `HEAD` before it is sent again: a `200` of the right length ends it, a `404` or another length sends the `PUT` again, which also overwrites a poisoned name. A request with no answer after 2 minutes counts as a network failure.
- **Damaged pictures.** A download that does not open under its name (flipped bit, a picture moved to another name, garbage) or whose hash is not the id is `ImageEvent::Damaged`: the bytes are dropped and it is not asked for again.
- **A reconnect** forgets requests in flight and keeps offers and wants. The new connection starts with a new list.
- **Status.** `Status::unsynced` is true while an upload is pending, offline too. `next_wake_ms` carries every retry time, so an idle client with no pending picture asks for no wake.

`NativeBlobTransport` runs blocking `ureq` calls on up to 5 worker threads and calls the wake hook after each answer. TLS is rustls with the ring provider and the bundled Mozilla roots (no aws-lc, no native-tls). Plain `http://` is only for this machine (the rule of `ws://`), unless `NativeBlobOptions::allow_insecure`. Redirects are not followed (the `Authorization` header would go along), no proxy from the environment is used, a body over `MAX_BLOB_BODY_BYTES` is a failure, and a request gives up after 100 s. `WebBlobTransport` uses `fetch` with `cache: no-store` and `redirect: error`. The token is in the header `Authorization: Drafft-Member <base64url>`, kept out of `Debug` output (`BlobAuth`) and out of failure texts. `ServerUrl::http_base()` maps `ws://host:port/ws` to `http://host:port` and `wss://` to `https://`.

Tests: `tests/images.rs` runs the real relay core behind `loopback_blobs` in simulated time (limits, membership, damaged downloads, one name, resume after a drop, backoff of 1, 2, 4 s, `Retry-After`, lost answers and `HEAD`, concurrency, counts, the wake times). `apps/drafftink-server/tests/blobs.rs` runs `NativeBlobTransport` against the real server and against misbehaving ones, and `images.rs` there runs the whole client against the real server. The browser transport is covered by compilation only, like `WebTransport`.

## How the engine drives the pictures

`crates/engine/src/cmd/collab/pictures.rs` is the glue between the client, the tab and the local blob cache (`Ns::Blobs`, plaintext, key = file id). `Session::collab_poll` calls it after the pump. It does work only when something changed that matters: the manifest revision of the replicated store, the client turning online (the client dropped its offers, and a relay that expired is empty), a picture arriving, and, only while some missing picture waits for the camera, the camera or the revision of the document. An idle session compares four numbers per poll and does nothing else (test `an_idle_session_with_pictures_does_no_picture_work_per_poll`).

For each manifest of the room:

- The tab holds the bytes: `offer_image` (the client uploads unless the relay has the name) and a write to the blob cache.
- The tab lacks them and the blob cache has them: they are taken from the cache (a reload, a picture seen before, offline).
- Else `want_image`, when an element that shows the picture is within one viewport of the visible rectangle. A viewport the shell never sized counts everything as near. `want_room_images` (an export) ignores the camera.

`ImageEvent::Arrived` goes to `TabDoc::supply_file` (the replicated view and the working store, no Loro write, no undo step, no edit: the logical revision, the saved state and the content fingerprint stay), the blob cache, and a scene redraw. The renderer draws the empty picture until the bytes are there, then encodes the element again: the working store's revision moves and the elements that show the picture are in `TabDoc::take_touched`. `Damaged` raises an info notice and the element keeps the empty picture. `Rejected` raises a problem notice with the plain texts of section 9.4. The content fingerprint counts the manifests besides the files of the working store, so a picture that arrives changes nothing.

A tab that joins a room is rebuilt (`TabDoc::restart_replicated(FilePolicy::External)`) from its working store: the old Loro history holds the bytes of embedded pictures and must not reach the room. The pictures it holds are offered and uploaded sealed. A tab restored from the workspace has the policy set by `read_room`. Leaving a room puts the policy back to `Embedded` and takes the blob cache's pictures into the tab. The workspace save deletes the blob cache entries that no tab names (a full save, and the first write after a tab was closed). `collab.status` shows `images` and, while visible pictures come, `loading.phase = "images"`. The engine tests use `FakeRelay` (`cmd/collab/fake_relay.rs`), which serves the picture routes of the fake network with the relay core.

## How the engine drives it

```text
construct:   CollabClient::new(config)  ->  CollabSession::new(client, NativeTransport::new(wake))
join:        session.client_mut().start(now)
every wake:  let p = session.pump(now, &mut store);
             for e in p.events { ... }          // route to the engine, see below
             if p.redraw { request one redraw with RedrawReason::Collab }
             schedule a wake at p.next_wake_ms  // request_repaint_after / setTimeout; None = nothing scheduled
after each local commit:  client_mut().local_changed(now); pump
presence:    client_mut().set_cursor / set_selection / set_viewport / set_user / set_laser / follow; pump
leave:       client_mut().leave(now, &mut store); pump
```

`wake` is the closure the transport calls from its own thread (native) or callback (browser) when a frame arrives. The shell turns it into `egui::Context::request_repaint` or a message to its loop, so the app never polls. Between wakes the only timers are the ones `next_wake_ms` names: a ping every 20 s, the awareness heartbeat every 10 s while someone else is in the room, and the retry after a lost connection. An idle connected client therefore costs about three tiny wake-ups a minute and no frames (`Pump::redraw` is false when no event came out). Tests: `an_idle_connected_client_causes_no_redraw_requests`, `an_online_idle_client_wakes_only_for_pings_and_never_in_the_past`.

`now_ms` is any monotonic millisecond counter that starts at or near zero (the engine's `Clock` port). It must never go backwards.

## What the events mean for the engine

| Event | Engine action |
|---|---|
| `Remote(RemoteBatch)` | The change is already in the store. Mark the `ChangeSet` as `origin = remote` (docs/protocol.md section 9), reconcile selection and gestures, mark the region dirty. `kind == Replaced` also clears local undo history. |
| `Status(Status)` | Status bar: `state`, plain-words `text`, `unsynced` (a "saving" dot), `peers`, `retry_at_ms`, standing `problem`. |
| `Problem(Problem)` | A toast. `text` says what happened and what to do, `action` is the button to offer (`NewToken`, `UpdateApp`, `Reload`, `ReduceSize`, `SaveCopy`, `AskAdmin`, `Decide`, `Wait`), `fatal` says the client stopped. |
| `PeerJoined`, `PeerLeft` | "Ana joined" toasts. |
| `Presence` | Redraw remote cursors; read `client().peers()`. A peer's `trail()` is its recent cursor path (laser). |
| `FollowViewport`, `FollowEnded` | Move the camera to the followed peer's viewport; leave follow mode. The engine calls `stop_following()` when the user moves the view. |
| `NeedsDecision` | The server's copy cannot merge into this one (its history was trimmed). Ask the user, then `resolve_trimmed(TrimmedChoice::ReplaceWithServer | KeepMine)`. A headless client sets `replace_when_trimmed`. |

## Sync, in short

1. `Join` carries the store's version vector. The reply is `Joined`, then a `SyncResponse` (updates or a snapshot), applied with `LoroStore::apply_remote`.
2. After every `SyncResponse` the client compares the server's version vector with its own. If the server lacks operations, one `Update` carries them (`export_updates_since(server_version)`). This single step repairs reconnects, edits made offline, updates the server dropped (rate limit), and updates that were never acknowledged.
3. Live edits: `local_changed` schedules a send 30 ms later (never closer than the server's rate limit). `take_unsent_update` yields all commits since the last send as one update. At most 32 updates are unacknowledged; further edits wait in the store and leave together when an `Ack` frees the window. An update with no `Ack` for 20 s triggers step 2.
4. A remote update that arrives before its predecessors (`has_pending_remote`) triggers a `RequestSync`. Requests are spaced 1 s apart, and the gap doubles up to 30 s while they keep failing.
5. `ResyncRequired` (the server dropped relays for a slow reader) sends a `RequestSync` at once. The native transport stops reading its socket when the owner does not drain events (8 MiB queue), so a stalled UI thread turns into this path instead of unbounded memory.
6. A lost connection waits `reconnect_delay_ms(attempt, jitter)` (0.25-0.5 s doubling to 15-30 s), or the server's `retry_after_ms` if longer. The attempt count resets after 10 s online. A connection must open within 15 s and finish the join within 15 s; a connection silent for 50 s is dropped (the client pings after 20 s).
7. Codes that the protocol says not to retry (`Unauthorized`, `Forbidden`, `UnsupportedVersion`, `Replaced`) stop the client. A `TooLarge` while joining stops it too. Everything else retries.

Remote operations are never sent back. This needed a fix in `drafftink-doc`: `take_unsent_update` used to return operations that arrived by `apply_remote` (regression test `remote_operations_are_not_handed_back_as_unsent`).

Named versions (`docs/design/versions.md`) need no step of their own: a save is one commit with the origin `sys:local`, so it leaves as one ordinary update (at most 2 MiB of state, under the 4 MiB update limit), and a rename or a delete is a few bytes. The test `versions_named_at_once_by_two_clients_reach_everyone_and_a_late_joiner_through_the_server` runs two clients, a late joiner and a server restart against the real server.

## The access token

A server started with `auth_tokens` (`docs/server.md`) accepts a join only when the first frame carries one of them. The engine passes the token to `CollabConfig::auth_token`; the client holds it as `AuthToken` and sends it in the `Join` frame. A missing or wrong token ends the client with `Unauthorized`, whose text asks for a new token.

Where the token comes from:

| Place | How |
|---|---|
| Command | `collab.connect {url, room, token}`, or `collab.connect {url, token}` and later `collab.join_room {room}`. `collab.join_room {room, token}` overrides the token of `connect`. The engine keeps it in memory with the chosen server: `collab.leave_room` keeps it, `collab.disconnect` forgets it. |
| Desktop launcher | `drafftink --room R --server URL --token-file FILE`, or the environment variable `DRAFFTINK_COLLAB_TOKEN`. The app then calls `collab.connect` with it. There is no flag that takes the token itself, because arguments are visible to every user of the machine (the same rule as the bridge token in `docs/agents.md`). |
| Web | Only through the command (for example `window.drafftink.execute("collab.connect", {url, room, token})`). The Collaborate dialog has no token field yet, and the token is never read from the page address. |

It is not a setting on purpose: `settings.get` would hand it to every agent, `settings.set` would echo it, and the settings record is written to disk. The token is not recorded in the ring (`CommandSpec::secret_args` makes `engine.recent` show `"<hidden>"`), not in any command output or status, not in `Debug` output (`TokenArg`, `StartupToken` and `AuthToken` print a placeholder), and not in a refusal (a token that is empty or longer than 4096 bytes is `bad_args` without repeating it). Tests: `the_token_is_never_recorded_or_shown` (engine), `the_token_of_the_launcher_reaches_a_server_that_asks_for_one_and_stays_out_of_the_ring` (app), `tests/logging.rs` (client).

## Access service token

A private instance (`docs/design/collab-e2e.md` section 19) sits behind Cloudflare Access. A browser signs in once and carries a cookie. A program that cannot sign in (the desktop app, a script, an MCP server) sends a **service token** instead: the headers `CF-Access-Client-Id` and `CF-Access-Client-Secret` on every request, the WebSocket handshake included. The token is a machine identity and a secret. These are the rules, and each has a test:

| Rule | Where it is decided | Test |
|---|---|---|
| The token goes to **one origin** (scheme, host and port) and no other: another host, another port, the same machine under another name, a sub-domain, a host that only starts with the same text. | `AccessGrant::headers_for` | `access::tests` (unit), `tests/access.rs` (wire: the mock servers see no header) |
| **Only over `https`/`wss`, or to this machine** (`localhost`, `127.x.x.x`, `::1`). Plain `ws://` or `http://` to another machine never gets it, **even when insecure connections are allowed** (`UrlPolicy::allow_insecure` protects a drawing, this protects a password). | `AccessGrant::headers_for`, `NativeBlobTransport::check` | `access::tests`, `tests/access.rs` |
| **No redirect is followed** for a request that carries the headers. tungstenite follows none, and `ureq` is set to follow none. A `302` to `*.cloudflareaccess.com` is Access saying "not signed in". | `NativeTransport`, `NativeBlobTransport` | `tests/access.rs` (the second mock server gets no connection) |
| **A denial is typed**, not a text in a log: `TransportEvent::Denied(AccessDenial)` for the handshake and `BlobReply::denial` for a picture request. | `AccessDenial::of_reply` | `access::tests`, `tests/access.rs` |
| The secret is **in no log line, `Debug`, ring entry, `collab.status`, refusal text, notice or link.** | `AccessCredentials` (no `Display`, `Debug` prints `AccessCredentials(<hidden>)`, a zeroizing buffer), `SecretText`, `secret_args` | `tests/logging.rs`, `the_token_is_in_no_output_no_ring_entry_no_status_no_notice_and_no_refusal` |

**What the person reads.** `AccessDenial::of_reply(status, body, sent)` decides from the status and the Worker's JSON (`{"error":"access_required"|"access_invalid","message":...}`):

| Answer | No token was sent | A token was sent |
|---|---|---|
| `401` or a redirect, no JSON | `Required` | `Rejected` |
| `access_required` | `Required` | `Rejected` |
| `access_invalid` | `Rejected` | `Rejected` |
| a `401` or `403` with another JSON `error` (the relay's own `unauthorized`: a membership token that does not open the room) | none | none |
| a `403` without that JSON | none (a relay says `403` for its own reasons) | none |

`Required`: "This server is private. Open Preferences, Collaboration, and enter the access token for `<host>`. The server's owner can create one." `Rejected`: "The server did not accept the access token. It may have expired or been revoked. Ask the server's owner for a new one." Both have the action `AskAdmin`. A denial of the handshake stops the client (`StopReason::AccessDenied`, fatal: the same token would be refused again). A denial of a picture request is a non-fatal problem, told once per connection: the room keeps working and the pictures wait.

**Where the token lives.**

| Program | Store | Notes |
|---|---|---|
| Desktop app | `<data folder>/secrets/access.json`, mode `0600` in a folder of mode `0700` (`NativeSecrets`) | Created with those modes from the first byte (a temporary file renamed over the old one). A file or folder found with looser modes is tightened. On Windows it takes the permissions of the user's own application folder; no access list is added. The OS credential store (the `keyring` crate) is a later option: it pulls in D-Bus on Linux. Format: `{"version":1,"tokens":{"https://canvas.example.com":{"client_id":"...","client_secret":"..."}}}`, one entry per origin. Read once per session, written through. |
| Headless programs (`drafftink-headless mcp`) | The environment: `DRAFFTINK_ACCESS_CLIENT_ID` and `DRAFFTINK_ACCESS_CLIENT_SECRET_FILE` (the path of a file that holds the secret) | No flag and no variable takes the secret itself: arguments and the environment are visible to other users, a file has permissions (a file readable by others is reported). Both or neither. The token applies to the server of the setting `collab.server_url` and to no other (`Secrets::ambient`). `collab.set_access` and `collab.clear_access` refuse here and say where the token comes from. These programs have no network port yet, so the collab commands still refuse with `port_unavailable`: the token is ready for the day there is one. |
| Web app | none (no `Secrets` port) | The browser carries the Access cookie itself. The commands refuse with `port_unavailable`; `collab.status` says `access: "browser"`. |

**Commands.** `collab.set_access {server?, client_id, client_secret}` stores a token, `collab.clear_access {server?}` removes it, `collab.status` shows `access: "service_token" | "browser" | "none"` and never a value. A token takes effect at the next connection: leave the room (or disconnect) and join again. They are not in a batch, not in the palette and not for agents, and both values are `secret_args`. A value that is empty, over 512 bytes or not printable ASCII is `bad_args` and is not repeated; a value that is not text is refused as "must be text" without quoting it. Not in `settings`: `settings.get` would hand it to every agent and the settings record is written to disk.

**Not in links.** A room link never holds it. The link is a room capability; the token is a server capability.

**Browser, expired sign-in.** The picture requests of the web build send `X-Requested-With: XMLHttpRequest`, so that Access answers an ended session with a `401` that `fetch` can read (not a redirect it can only report as a network error). That `401` is marked `AccessDenial::SignInExpired`: "Your sign-in has expired. Reload the page to sign in again. Your drawing is saved on this device." A **WebSocket** cannot carry the header and a failed upgrade is only a `Closed` event, so the web transport cannot tell an ended sign-in from a server that is down. The web transport therefore asks the `/api/status` probe (`docs/design/collab-e2e.md` sections 11.6 and 19.6, `crates/collab/src/web.rs`) when a socket closes before it opened: a `401` is `SignInExpired`, a `404` (a host without a relay, GitHub Pages for one) ends the connect with "<host> only serves the app and does not host shared rooms..." (`StopReason::NoRelay`, no retry), and a spent daily quota waits and tries again. A healthy answer leaves the plain "Cannot reach the server".

**Logs.** tungstenite writes the whole handshake request (the service token's headers) and the first frame (the plaintext room's token) at the `trace` level, and `ureq` may do the same. The client's own log lines and everything up to `debug` hold no secret (`tests/logging.rs` runs at `debug`). Do not enable `trace` for those crates on a machine whose logs others read.

**Deleting a room.** `collab.delete_room {confirm}` sends `DeleteRoom` from a joined member (`SealedClient::delete_room`). A host that allows it (the private instance) deletes the room's records, snapshot and pictures and closes the connection; the client takes that clean close for the deletion, since the relay sends nothing else: `CollabEvent::RoomDeleted`, `StopReason::RoomDeleted`. The engine then ends the room as `collab.leave_room` does (the tab keeps its drawing as a local copy, the key and the saved link are dropped) and `collab.status` has `room_deleted: true`. A host that does not allow it answers `Forbidden`: the problem "This server does not allow deleting rooms." (not fatal, the client joins again). A connection that breaks during the request proves nothing; the client reconnects and a room that is gone is the usual "This room is gone".

## Presence

Awareness is sent at most every 50 ms (or the server's slower rate), only when it changed, plus a full resend every 10 s while someone else is in the room. A client alone in a room sends nothing; a `PeerJoined` makes it send at once. A peer's presence is cleared after 30 s of silence while the peer stays listed. The laser pointer is local: protocol version 1 has no laser flag, so a receiver draws the recent cursor path (`Peer::trail`, 1.5 s, 64 points) of any peer. A real "this peer uses the laser" flag needs protocol version 2.

## Security

- The service token of a private server never leaves its origin, never goes over plain text to another machine, and is never sent after a redirect (see "Access service token").
- `ServerUrl::parse` accepts only `ws://` and `wss://`, refuses `user:password@`, `#fragments`, control characters and overlong text, and refuses `ws://` to any host that is not loopback unless `UrlPolicy { allow_insecure: true }`. No transport takes a plain string.
- `AuthToken` has no `Display`, `Debug` prints `AuthToken(<hidden>)`, and no log line prints a protocol message that holds it (tests: `tests/logging.rs`, `debug_output_never_contains_the_token`). The token travels in the first binary frame, never in the URL.
- `wss://` uses rustls with the ring provider named explicitly (no process-wide default that another crate could change), the system roots (bundled Mozilla roots as a fallback), and `TlsOptions::extra_roots_der` for a private CA. A certificate that is not trusted stops the handshake before any frame, so the token never reaches the server (`tests/tls.rs`).
- Server frames are decoded with the protocol crate's bounded decoder; presence from peers is validated again before it is stored; every remote byte goes through `LoroStore::apply_remote`, which reports errors and never panics. `tests/hostile.rs` runs random message sequences, random bytes and random time.

## Tests

`cargo test -p drafftink-collab`:

| File | What |
|---|---|
| `src/*.rs` | URL policy, presence throttling and expiry, error wording. |
| `tests/state_machine.rs` | The client against a scripted server: join, upload, debounce, window, resync, rate limit, every error code, backoff, offline edits, ping and idle, trimmed history, leave, presence, follow, status. |
| `tests/session.rs` | `CollabSession` over the loopback transport, wake-ups, idle pumps. |
| `tests/hostile.rs` | Property tests: random sequences never panic, never ask to be woken in the past, never reconnect a stopped client. |
| `tests/server.rs` | Two to three clients against the real server (`drafftink-headless serve`, started as a child process on an ephemeral port, built by the test with `cargo build -p drafftink-headless`; the crate layering forbids linking an app): concurrent edits converge, late join, server killed and restarted with edits made offline, a client that does not read is told to resync, auth on and off, version mismatch, replaced identity, presence and follow, leave, backoff against a dead port, idle means no redraw. |
| `tests/tls.rs` | `wss://` through a TLS-terminating proxy with a test certificate (`tests/fixtures`). |
| `tests/logging.rs` | The token and the service token are absent from every log line (at `debug`). |
| `tests/access.rs` | The service token on the wire against local mock servers: the handshake headers and every kind of picture request, only to the stored origin, never to another, never after a redirect, the Worker's `401` JSON, the relay's own `401`. |

## Big rooms

A tab with no content that joins a room of 256 KiB or more (about 1,700 elements) does not decode it in one piece. `LoroStore::apply_remote` imports the bytes into a fresh CRDT document, which is lazy and takes about 10 ms for 100,000 elements, and leaves the view empty. `Session::collab_poll` then calls `LoroStore::import_step` for at most 8 ms per poll and mirrors each slice into the working store, so the app draws a frame and answers commands between two slices. The room appears in slices; `collab.status` shows `loading: {phase: "importing", done, total}`. Numbers (release, `cargo bench -p drafftink-doc --bench store -- staged_import`): 100,000 elements take about 1.1 s in about 400 slices of 256 items, the slowest 17 ms, after a first call of about 100 ms; the old single call took 4.1 s. The same code runs natively and in the browser. A worker thread on desktop was considered and not built: the replicated store holds `Rc` state and cannot move to another thread, and slicing needs no second code path and works in the browser, which has no threads. The cost is that the desktop spends the same CPU on the UI thread, spread over frames.

Rules while a room loads: local edits are allowed and are sent; `doc.save` and `doc.export` refuse with `room_loading`; `collab.leave_room` finishes the import first; a second update from a peer is applied at once for the elements it touches. Tests: `crates/doc/tests/remote.rs` (slices, equal result, edits between slices, no staging with content), `crates/engine/src/cmd/collab_tests.rs` (`a_big_room_is_taken_in_slices_and_the_session_answers_between_them` and its neighbours).

## Known limits

- Pictures: a client that wants nothing and offers nothing makes no picture request. The client keeps no copy of the bytes it has uploaded (the engine's blob cache is the copy), and the web build has no browser test. Encrypted rooms: no peer fallback and no crash fence yet (`docs/design/collab-e2e.md` sections 8.2 and 8.5); a record that makes Loro assert still ends a wasm page.

- A very large room joined into an empty tab is taken in slices (next section). Importing a big update into a tab that already has content still happens inside `pump` on the caller's thread (about 25 us per element, 2.5 s at 100k elements): the engine holds it back for one poll so the app can draw "loading" first, but it cannot be cut, because merging into existing content needs the CRDT's own diff. The same holds for a trimmed snapshot that replaces a copy with its own work (`NeedsDecision`).
- The one part of a staged import that cannot be sliced is the first call: about 10 ms plus 0.8 us per element to list the ids (80 ms at 100k elements), so it stays inside one 16 ms frame up to about 8,000 elements. Past that a web page has one long frame, once, until the import runs in a Web Worker.
- An update bigger than the server's `max_update_bytes` (a very large image) cannot be split. It is kept, reported as `ReduceSize`, and later edits wait behind it.
- The browser transport has no backpressure (a `WebSocket` has none) and is covered by compilation only; there is no browser test in the gates yet.
