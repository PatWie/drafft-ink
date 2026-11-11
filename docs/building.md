# Building and running

One workspace builds three things from the same code: the desktop app (`apps/drafftink`), the web app (`apps/drafftink-web`, served by the server) and the collaboration server (`apps/drafftink-server`). The shared application code is `crates/app` (`drafftink-app`, L7, see `architecture.md`). The desktop and web apps are thin launchers: they build a `Platform` (the ports that differ per target) and call `drafftink_gpu::run(App::new(platform, config), options)`.

The pinned toolchain is in `rust-toolchain.toml` (Rust 1.97.1, edition 2024). `rustup` installs it on the first `cargo` call.

## 1. Desktop

```sh
cargo run --release -p drafftink                 # or: scripts/run-native.sh
scripts/build-native.sh                          # release build, prints the path
```

Command line (`drafftink --help`):

| Argument | Meaning |
|---|---|
| `FILE` | Open a `.drafftink`, `.json`, `.excalidraw` file or a PNG with an embedded scene. It replaces the intro document on a first run and opens in a new tab otherwise. |
| `--room ROOM` `--server URL` | Join a room at start (`--server` needs `--room`; default server: the `collab.server_url` setting, `ws://localhost:3030/ws`). With work already open, the room gets a new tab. |
| `--token-file FILE` | The access token of the collaboration server (needs `--room`). Without it the environment variable `DRAFFTINK_COLLAB_TOKEN` is used. No flag takes the token itself: arguments are visible to other users. |
| `--bridge [PORT]` | Let an AI agent drive this window (`agents.md`). Off by default. Listens on `127.0.0.1` only, on a free port or `PORT`, and prints the token file path. |
| `--mcp-http [PORT]` | Serve MCP over streamable HTTP on `127.0.0.1:PORT` (default 3737; 0 is a free port) for MCP clients that take a URL (`agents.md` section 5.4). Off by default. Every request needs the bearer token from a private file, and the Preferences have a switch for it. |
| `--perf-overlay` | Frame counts and cache numbers over the canvas. |
| `--perf-dump FILE` | At exit write the performance counters (`PerfStats` JSON, `performance.md`). Close the window to get it. |
| `--data-dir DIR` | Keep settings, documents and the workspace in `DIR`. Default: `$XDG_DATA_HOME/drafftink` or `~/.local/share/drafftink` (Linux), `~/Library/Application Support/drafftink` (macOS), `%APPDATA%\drafftink` (Windows). `DRAFFTINK_DATA_DIR` does the same. |

`RUST_LOG=drafftink_gpu::frame=debug` prints one line per drawn frame with its reason. `WGPU_BACKEND=vulkan|dx12|metal` picks the graphics API.

### Linux

Build needs only the Rust toolchain and a C linker (`build-essential`). Nothing links against X11, Wayland, fontconfig or freetype at build time: winit loads them with `dlopen` and the fonts are bundled.

At run time it needs:

| Package (Debian/Ubuntu names) | For |
|---|---|
| `libvulkan1` and a Vulkan driver (`mesa-vulkan-drivers`, or the NVIDIA/AMD driver) | The GPU. Vello needs compute shaders. There is no OpenGL fallback. `mesa-vulkan-drivers` includes lavapipe, a CPU Vulkan driver that works but is slow. |
| `libxkbcommon0`, `libx11-6`, `libxcb1` or `libwayland-client0` | The window. X11 is tested; Wayland compiles and is untested. |
| `xdg-desktop-portal` and a backend (`xdg-desktop-portal-gtk` or `-kde`) | The file open and save dialogs. Without a portal the pickers do nothing. Dropping a file on the window and the command line argument work without it. |
| `libxkbcommon-x11-0` | Keyboard on X11. |

### macOS and Windows

`cargo build --release -p drafftink` needs Xcode command line tools (macOS) or the MSVC build tools (Windows). The wgpu backends are Metal and DX12. Both are compiled by CI (`.github/workflows/ci.yml`) and **have not been run by us**: no Mac or Windows machine was available. Known differences are in `shell.md` section 6 (pen pressure, IME) and 8.

## 2. Collaboration server, and the web app from the same binary

```sh
cargo build --release -p drafftink-server
./target/release/drafftink-server --web-dir dist           # 127.0.0.1:3030
```

The server serves `/ws` (the protocol, `protocol.md`), `/health`, and the static files of `--web-dir`, so one binary and one folder are the whole deployment (`server.md`). The web page opens `/ws` on its own origin, so a page served by this server collaborates with no configuration. Rooms: `http://localhost:3030/?room=my-room` joins (or creates) the room.

## 3. Web

The web build and its JavaScript API are described in `web-app.md`. In short:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked       # must equal the wasm-bindgen pin
# wasm-opt (binaryen 117 or newer; 133 is what we measured) on PATH or in ~/.cargo/tools
scripts/build-web.sh                                            # produces dist/ (WebGPU module in pkg/, WebGL2 module in pkg-gl/) and prints the sizes
scripts/build-web.sh --no-fallback                              # without the WebGL2 module for browsers without WebGPU
scripts/serve-web.sh                                            # builds the server and serves dist/
cargo xtask size                                                # builds the same way and checks the sizes against perf/budgets.json
```

The script needs `brotli` (or the Python `brotli` module, `PERF_PY`) only to print the compressed sizes. `cargo xtask size` takes about a minute with a warm cache; a cold build is several minutes on a small machine, which is why CI runs it as its own job (`size`) next to the others.

The browser needs WebGPU: Chromium-based browsers 113 and newer. Firefox and Safari depend on the version and flags and were not tested. Without it the page shows an error page that says so.

## 4. Checks

| Command | What |
|---|---|
| `cargo xtask ci` | Every gate: fmt, clippy `-D warnings`, layers, standards, wasm, deny, tests. |
| `cargo xtask wasm` | L0 to L5, `drafftink-app` and `drafftink-web` for `wasm32-unknown-unknown`. |
| `cargo test -p drafftink-app` | The app without a window, over the fake ports (start-up, restore, file argument, room link), input translation, clipboard conversion, desktop storage and files. |
| `~/.venvs/perf/bin/python scripts/native_check.py target/release/drafftink` | The desktop app under a private Xvfb with real X input and the bridge: draws a rectangle, an arrow and a text with the mouse and keyboard, checks them through the bridge, takes screenshots, measures idle, hover, pan and wheel, closes the window and checks the perf dump. Needs `Xvfb`, a GPU (or lavapipe) and the Python venv with `python-xlib` and `pillow`. |

### Measured (2026-11-08, NVIDIA L40S, Xvfb, release build, `native_check.py`)

| Window | Frames | CPU (% of one core) |
|---|---|---|
| idle 10 s | 0 | 0.4 |
| hover over empty canvas 10 s, 125 moves/s | 0 | 1.6 |
| idle 3 s after hover | 0 | 0.7 |
| middle-button pan 5 s, 125 moves/s | about 1 per move (600 to 630) | 39 to 58 (four runs) |
| idle 10 s after pan | 0 | 0.5 |
| wheel 5 s, 100 ticks/s | about 2 per tick (940 to 1050; egui smooths the wheel) | 62 to 64 |
| idle 10 s after wheel | 0 | 0.4 |
| idle 5 s after undo | 0 | 0.6 |

The scene is the 52-element intro document; frames are counted from the shell's `drafftink_gpu::frame` log lines. The script also
draws through the bridge (two rectangles, an arrow bound to both, `element.move` drags the arrow along, `view.screenshot`) and
with real X input (the tool keys `r`, `a`, `t`, then mouse drags and typing for a rectangle, an arrow and a text). Screenshots: `img/native-intro.png`, `img/native-bridge-drawn.png`, `img/native-text-edit.png`,
`img/native-drawn.png`, `img/native-view-screenshot.png`.

Web (`web_check.py --app`, headed Chromium on the same machine, release-wasm build): 0 frames in 10 s idle (0.8 % of one core,
all Chromium processes), 0 frames in 22 s alone in a room and 0 with a peer. Sizes and the rest are in `web-app.md`.

The welcome document and the README pictures are generated: `cargo xtask intro` and `cargo xtask media` (`media.md`).

## 5. Where things are

| Path | What |
|---|---|
| `crates/app` | `App`, the platform seam (`Platform`, `Host`, `SharedClipboard`, `WakeCell`), input translation, the overlay glue, the text editor, start-up. `native/` has the desktop ports. `web/` has the browser ports. |
| `apps/drafftink` | The desktop launcher (command line, bridge, perf dump). |
| `apps/drafftink-web`, `web/` | The wasm entry with `window.drafftink`, and the static page, manifest and service worker. |
| `scripts/` | `build-native.sh`, `run-native.sh`, `build-web.sh`, `serve-web.sh`, `native_check.py`, `web_check.py`. |

## 6. How the app works, briefly

A frame happens only for a reason the shell recorded (`shell.md` section 2). In a frame, `App` does, in order: take results of background work (file dialogs, the bridge queue, the collaboration client, the autosave timer), let the interaction layer run its timers, hand Esc and Enter to it when it owns them, turn wheel and pinch events into canvas input, let the text editor take its keys, draw the UI, execute the commands that came out of it, and then rebuild the scenes whose inputs changed. The document scene is rebuilt only when the document revision, the camera, the size, the theme, the grid or the hidden list changed; the overlay scene only when the interaction layer's overlay data or the other people's pointers changed. The last step asks the shell for the earliest of the app's own deadlines: the interaction layer's (long press, laser fade, throttled camera report), the collaboration client's, and the workspace autosave's. With none of them, the app asks for nothing.

Pointer samples reach the app before egui. A press that lands on a panel belongs to egui; a press on the canvas starts a canvas drag that keeps the pointer until the button is up, even over a panel. Text typed in the editor reaches the engine in one `text.edit_end`; a press anywhere else first ends the edit with the typed text.

Known gaps are listed in the final report of the task and in `backlog.md`.
