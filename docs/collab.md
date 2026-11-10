# Collaboration client (crate `drafftink-collab`, L4)

The client side of `docs/protocol.md`. The server is `apps/drafftink-server` (`docs/server.md`), the document is `LoroStore` (`docs/crdt.md`). The engine (L5) owns a `CollabSession` and drives it; this crate does not know about commands, the scene, egui or the clock. Builds for wasm32.

The engine side (the `collab` command family, `Session::collab_poll`, the `Net` port) is described in `docs/engine.md` section 4, "Collaboration".

## Parts

| Part | What it is |
|---|---|
| `CollabClient` | Sans-IO state machine. Inputs: transport events, `tick(now_ms, &mut LoroStore)`, local calls. Outputs: `poll_output()` (connect, send a frame, close), `take_events()`, `next_wake_ms()`. No socket, no clock, no document of its own. |
| `Transport` | The frame pipe: `open`, `send`, `close`, `poll`. `NativeTransport` (background thread, tokio, tungstenite, rustls), `WebTransport` (browser `WebSocket`, wasm only), `loopback()` (in memory, for tests). |
| `CollabSession<T>` | A client plus a transport. `pump(now_ms, &mut store)` moves bytes both ways and returns `Pump { events, next_wake_ms, redraw }`. |
| `LocalPresence`, `PeerTable` | Ephemeral presence: cursor, selection, viewport, user, laser trail, peers with timeouts. Never enters the document. |
| `ServerUrl`, `AuthToken` | The checked address and the secret token. |

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

## Presence

Awareness is sent at most every 50 ms (or the server's slower rate), only when it changed, plus a full resend every 10 s while someone else is in the room. A client alone in a room sends nothing; a `PeerJoined` makes it send at once. A peer's presence is cleared after 30 s of silence while the peer stays listed. The laser pointer is local: protocol version 1 has no laser flag, so a receiver draws the recent cursor path (`Peer::trail`, 1.5 s, 64 points) of any peer. A real "this peer uses the laser" flag needs protocol version 2.

## Security

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
| `tests/server.rs` | Two to three clients against the real server binary (started as a child process on an ephemeral port, built by the test with `cargo build -p drafftink-server`; the crate layering forbids linking an app): concurrent edits converge, late join, server killed and restarted with edits made offline, a client that does not read is told to resync, auth on and off, version mismatch, replaced identity, presence and follow, leave, backoff against a dead port, idle means no redraw. |
| `tests/tls.rs` | `wss://` through a TLS-terminating proxy with a test certificate (`tests/fixtures`). |
| `tests/logging.rs` | The token is absent from every log line. |

## Known limits

- Importing a very large snapshot happens inside `pump` on the caller's thread (about 14 us per element, 1.5 s at 100k elements: docs/crdt.md section 15). `docs/performance.md` asks for no stall over 50 ms; for huge rooms the engine should run the first join on a worker or show a loading state.
- An update bigger than the server's `max_update_bytes` (a very large image) cannot be split. It is kept, reported as `ReduceSize`, and later edits wait behind it.
- The browser transport has no backpressure (a `WebSocket` has none) and is covered by compilation only; there is no browser test in the gates yet.
