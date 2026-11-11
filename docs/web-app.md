# The web app

The web build is `apps/drafftink-web` (the wasm entry point and the JavaScript API) over `crates/app` (the shared `App`) with the
browser platform in `crates/app/src/web`. `drafftink-server` serves the result and the collaboration socket from one origin, so the
whole deployment is one binary and one folder.

```
apps/drafftink-web/src/   lib.rs  entry.rs (start)  api.rs (window.drafftink)  agent.rs (relay client)
                          pairing.rs (pure rules, tested natively)  js.rs (JSON and promises)
crates/app/src/web/       mod.rs (platform)  storage.rs + idb.rs + cache.rs  files.rs  net.rs  host.rs  rng.rs  pure.rs
web/                      index.html  manifest.webmanifest  sw.js  favicon.svg  icon-*.png
scripts/                  build-web.sh  serve-web.sh  web_check.py
```

## Build and serve

```
scripts/build-web.sh            # release-wasm build -> wasm-bindgen --target web -> wasm-opt -Os -> dist/
scripts/serve-web.sh 3030       # cargo run --release -p drafftink-server -- --web-dir dist --bind 127.0.0.1:3030
```

`build-web.sh` checks that the `wasm-bindgen` CLI has the version pinned in the root `Cargo.toml` (0.2.130), runs `wasm-opt` from
`PATH` or `~/.cargo/tools/binaryen-version_*/bin`, writes `dist/` and prints the wasm size raw, after `wasm-opt`, gzip -9 and
brotli -11. It also writes `.gz` and `.br` next to each text and wasm file; `drafftink-server` serves them when the browser accepts
them (`--no-precompress` skips this, `--profile release` builds faster and bigger). `dist/` is in `.gitignore`.

`wasm-opt` runs with an explicit feature list, not `--all-features`: with binaryen 133 `--all-features` turns on experimental
encodings and Chromium refuses the module (`unknown import kind 0x7e`).

`dist/` holds `index.html`, `manifest.webmanifest`, `sw.js`, the icons, `BUILD` (the build hash) and `pkg/drafftink_web.js` plus
`pkg/drafftink_web_bg.wasm`. The build hash (12 hex digits of the wasm and the glue) replaces `__BUILD__` in `index.html` and `sw.js`.

Sizes of the first full app build (2026-11-08): raw 27.8 MB (with the name section), wasm-opt 17.3 MB, gzip -9 6.4 MB,
**brotli -11 4.4 MB**. The graphics spike was 1.36 MB (docs/stack.md); the rest is the engine, Loro, the image codecs, the bundled
fonts and the UI. There has been no size work yet.

## What the page does

`index.html` has one `<canvas id="canvas">` that fills the viewport (CSS), a loading bar and a failure box. A module script loads
the glue, awaits `start("canvas")` and removes the loading bar after the first frame. Then it sets `window.__started`
(`window.__error` on failure; the check script reads both) and asks the browser for persistent storage. If WebGPU is missing the
shell shows its own error page.

`start(canvas_id)` (in `entry.rs`):

1. installs the panic hook and `console_log`; turns profiling on only for `?perf=1` (or `?perf=overlay`, which also shows the
   overlay) or when the page set `window.__drafftinkPerf = true` before `start`;
2. reads the page address: `?room=..&server=..` (query or fragment) is the room to join, via `StartupLink::parse`;
3. `drafftink_app::web::platform(..)` opens IndexedDB (awaited), then builds the ports and the bridge queue;
4. installs `window.drafftink`, reads or resumes the agent pairing, builds `AppConfig { join, default_server: "/ws",
   stored_default_target: true, .. }` and calls `drafftink_gpu::run`.

### Ports in the browser

| Port | Web implementation |
|---|---|
| Storage | `WebStorage`: the whole `drafftink` IndexedDB (one object store, keys `<namespace>/<key>`) is read into memory before the app starts. `get` and `list` read memory. `put` and `delete` change memory and queue a write that goes out in the next microtask (a burst of writes to one key is one write). Pending writes are flushed again on `pagehide` and when the page becomes hidden. A failed transaction is retried with the next change or flush and logged. Without IndexedDB (some private modes) the storage is memory only and a notice at the bottom of the page says so. Two tabs of one site each see the database as it was when they opened; the autosave record belongs to the tab that wrote last. |
| Files | `Download{name}`: a Blob with the given media type and an anchor click. `Stored{name}`: the `Documents` namespace. Paths and handles are refused with a plain message. |
| Net | `WebTransport`. `same_origin_url("/ws")` is `ws(s)://<location.host>/ws` by the page protocol; plain `ws://` to other machines is allowed only when the page itself is on a loopback host. |
| Clock, ids | `AppClock`; `RandomIds` over `crypto.getRandomValues`. |
| Clipboard | the shared slot the shell fills from `paste` events. |
| Renderer | none (see gaps). |
| Pickers | a hidden `<input type=file>` with accept lists (documents; images, multiple; libraries). The files are read in full and posted as `Inbound::Picked`. Cancel (the input's `cancel` event) sends nothing and removes the input. `PickSaveTarget` answers at once with the download name. `ReadClipboardText` uses `navigator.clipboard.readText()`. |

## `window.drafftink`

Every method returns a Promise except `perf*`, `frames` and `agent.status`. Requests are queued for the app and run between
frames with origin `agent`, exactly like the desktop bridge (`drafftink_bridge::core::dispatch`). A refusal rejects with
`{code: -32010, message, data}`; `data` is the whole refusal (`code`, `command`, `message`, `suggest`, `details`). Bad arguments
reject with `-32602`, a full queue with `-32004`.

```js
const made = await drafftink.execute('element.create', { kind: 'rectangle', bounds: { x: 100, y: 100, w: 240, h: 140 } });
await drafftink.batch(
  [{ id: 'element.create', args: { kind: 'ellipse', bounds: { x: 400, y: 80, w: 120, h: 90 } }, as: 'oval' }],
  { label: 'add an oval', atomic: true });
const { items } = await drafftink.execute('element.query', { fields: ['id', 'kind', 'bounds'] });
await drafftink.commands({ family: 'element' });      // the registry with live availability
await drafftink.describe('element.create');          // schemas, facts, example, refusals
await drafftink.recent({ limit: 10 });                // the ring: what ran, with origin
await drafftink.state();                              // engine.state
try { await drafftink.execute('nope.nothing'); } catch (e) { console.log(e.code, e.message, e.data.suggest); }
```

| Call | Meaning |
|---|---|
| `execute(id, args?)` | one command |
| `batch(steps, opts?)` | `steps: [{id, args?, as?}]`, `opts: {label?, atomic?, dry_run?}` |
| `commands(opts?)`, `describe(id)`, `recent(opts?)`, `state()` | as in docs/agents.md section 5.1 |
| `perf()` | `PerfStats` as an object (counters and redraw reasons are always on; spans and frame times need `?perf=1` or `perf_enable(true)`) |
| `perf_reset()`, `perf_enable(on)` | reset the recorder; turn span recording on or off |
| `frames()` | `{frames, vello_runs}`: frames drawn and frames in which Vello ran since start. Two reads 10 s apart that are equal prove an idle page |
| `version` | the crate version |
| `agent.pair(url, token)` (alias `connect`), `agent.disconnect()`, `agent.status()` | next section |

## Pairing with an AI agent (docs/agents.md, section 5.2)

`drafftink-mcp --relay` prints a link `https://<this site>/#drafftink-agent-port=<port>&drafftink-agent-token=<64 hex>`. When the page
opens with it:

1. at start the page reads the two keys from `location.hash` and removes both with `history.replaceState`, even if the pair is
   unusable, so the token does not stay in the address bar or the history;
2. it builds `ws://127.0.0.1:<port>/` itself and connects. The link cannot name another host;
3. it sends `hello` with `{token, page: {version, registry: {count, digest}}}`; the relay answers with a `resume_token`, which the
   page stores with the port in `sessionStorage` (this tab only);
4. from then on it answers the relay's requests (`execute`, `batch`, `commands`, `describe`, `recent`, `state`, `ping`) through the
   same queue as `window.drafftink.execute`, so they run between frames on the session;
5. when the socket closes with any code except 1008 it reconnects with the resume token after 1, 2, 4, ... 30 s, for 10 minutes.
   A reload resumes from `sessionStorage` the same way. A wrong or used token (error -32001, close 1008) stops everything and the
   status says to ask for a new link. A first pairing that cannot reach the relay rejects with a message and does not retry;
6. `agent.disconnect()` closes with 1000 and forgets the resume token.

By hand: `await drafftink.agent.pair('ws://127.0.0.1:4555/', token)`. Any other host is refused: "The agent bridge only listens on
this machine (loopback). Use an address like ws://127.0.0.1:<port>, ws://localhost:<port> or ws://[::1]:<port>." `agent.status()`
returns `{state: idle | connecting | connected | waiting | refused | gave_up, port, message, attempts}`.

Chrome may ask the user to allow the site to reach local network addresses; Safari blocks `ws://127.0.0.1` from an `https` page.

## Service worker and caching (`web/sw.js`)

| Request | Rule |
|---|---|
| install | stores the shell (`./`, `index.html`, manifest, icons, `pkg/drafftink_web.js?v=<build>`, `pkg/drafftink_web_bg.wasm?v=<build>`) in the cache `drafftink-<build>` |
| activate | deletes every other `drafftink-*` cache and takes control of open pages |
| navigation | network first (`cache: no-cache`); when the network fails, the stored `index.html`. So a deployment shows at once and the app still starts offline. Room links (`/?room=x`) work offline too (the room itself needs the network) |
| other same-origin GET | cache first; a plain 200 that was not stored yet is stored |
| `/ws`, `/health`, other origins, non-GET | never touched |

The script and the wasm carry `?v=<build>` and are looked up with the exact address, so a cached file always belongs to the build
that asked for it. The worker is registered only on `https:` and on `localhost` or `127.0.0.1`.

## Checks

`scripts/web_check.py [--app] [--shot docs/img/web-app.png]` (python from `~/.venvs/perf`; needs `dist/` and
`target/release/drafftink-server`). It starts the server on an ephemeral port with `--web-dir dist --storage memory`, a private
Xvfb and headed Chromium with the WebGPU flags of docs/shell.md, and checks: the page starts and draws; the API surface; the
pairing refusals; the pairing keys are scrubbed from the address; 0 frames and the CPU of all Chromium processes in a 10 s idle
window. With `--app`: `state`, `commands`, `describe`, a refusal as a rejected promise, a rectangle, an arrow and a batch, the
ring shows origin `agent`, a screenshot, 0 frames after commands, the workspace in IndexedDB and back after a reload, two pages
(`127.0.0.1` and `localhost`, so separate IndexedDB) in one `?room=` seeing each other's elements, 0 frames in a room alone and
with a peer, the service worker active, and a reload with the server stopped still starting the app.

Pure rules (address check, pairing link, backoff, frames, download names, address building, the write cache) have unit tests that
run natively: `cargo test -p drafftink-web` (the files of `crates/app/src/web` that need no browser are compiled into that test
target, because the repository has no wasm test runner).

![web app](img/web-app.png)

## Known gaps

- **PNG export on the web.** The renderer port is missing: reading the GPU texture back is asynchronous and the port is
  synchronous. Commands that need the renderer port (PNG export) refuse with `port_unavailable`. A fix needs an async export path in the app (the shell's
  `Compositor::render_to_image` already works on the web).
- **Wire size.** 4.4 MB brotli for the first load; no size work yet. After the first visit the service worker serves it from cache.
- **Idle while in a room.** Every wake from the collaboration socket draws a frame, including the server's ping (every 20 s) and a
  peer's presence heartbeat (every 10 s): 1 frame in 22 s alone in a room, 5 with one peer (`web_check.py --app` reports this as
  failing). The app answers `Repaint::Now` to every `Wake`; it could pump the collab session and repaint only when something
  visible changed.
- **Two tabs.** The IndexedDB is read once per tab; the autosave record belongs to the tab that wrote last.
- **File System Access** (saving to a chosen place) is not used: saving is a download or a stored document.
- **Safari and Firefox** were not tried. The service worker, IndexedDB and pickers use standard APIs; WebGPU support decides.
- **Clipboard read** for the equation editor needs the browser's permission and a user gesture.
- **Screenshots** of the canvas by script (`drafftink.screenshot()`) are not offered: the shell exposes no read-back to the page.
