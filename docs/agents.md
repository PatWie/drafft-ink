# Agents: driving drafft.ink from an AI client

An agent uses drafft.ink through the same `Session::execute` that a button uses. Nothing is special-cased for it: the commands, the refusals and the undo steps are the user's own. This page covers setup, the security model, the tools, the wire protocols, and worked examples. The design is in `docs/plan.md` section 11, the command contract in `docs/commands.md`, the engine API in `docs/engine.md`.

Code: `crates/bridge` (the protocol, the loopback server, the relay, the generated tools, MCP over HTTP), `apps/drafftink-mcp` (the MCP server over stdio) and `apps/drafftink-web` (WebMCP in the page).

## 1. The ways in

| Way in | Start it | The agent drives | The user |
|---|---|---|---|
| headless | `drafftink-mcp` | its own editor session, no window | nothing to watch; files on disk |
| attached | `drafftink-mcp --attach` | the running desktop app, through a stdio helper | watches the canvas change live |
| relay | `drafftink-mcp --relay` | the open drafft.ink browser tab, through a stdio helper | watches the tab |
| desktop HTTP | `drafftink --mcp-http` (or the switch in Preferences) | the running desktop app, which serves MCP itself over streamable HTTP | watches the canvas change live |
| WebMCP | nothing: the page registers its tools when the browser has WebMCP | the open tab, for the browser's own agent | watches the tab |

All of them expose the same tools. A tool does not know which mode it runs in: every call becomes a bridge method (`execute`, `batch`, `commands`, `describe`, `recent`, `state`) and goes to a session in this process, in the desktop app, or in the page.

**Headless** owns a session with the real text engine and no renderer. Files are read and written by explicit path, only inside the folders you allow (default: the folder the server starts in; `--allow-dir DIR`, repeatable; `--allow-any-path` lifts the limit). PNG export and screenshots need a GPU renderer, which a headless process does not have: `export` with `format: png` refuses and says to use `svg`, or to attach to the desktop app.

**Attached** connects to the desktop app's bridge. It finds the app through the token file (`current.port` and `<port>.token` in the app's runtime folder, section 3) and reads the token again on every reconnect, so restarting the app needs no restart of the agent. A PNG export or screenshot uses the app's renderer.

**Desktop HTTP** needs no helper process. The desktop app listens on `127.0.0.1` and speaks MCP over streamable HTTP, for clients that connect to a URL. Section 5.4 specifies it, section 2 has the client setups.

**WebMCP** needs no process at all: the page hands its tools to the browser (section 5.5).

**Relay** is for the web version. A web page cannot listen on a socket, so the page connects out to `drafftink-mcp --relay` on `127.0.0.1`. The relay prints a pairing link; the user opens it in the browser tab; from then on the agent drives that tab. Section 5 specifies the page's side.

## 2. Setup

Build the binary once: `cargo build --release -p drafftink-mcp`, which gives `target/release/drafftink-mcp`. In the snippets below, replace `/path/to/drafftink-mcp` with its absolute path.

### Claude Desktop

Edit `claude_desktop_config.json` (Settings, Developer, Edit config), then restart Claude Desktop.

Headless:

```json
{
  "mcpServers": {
    "drafftink": {
      "command": "/path/to/drafftink-mcp",
      "args": ["--allow-dir", "/Users/me/diagrams"]
    }
  }
}
```

Attached to the desktop app (start the app first, or start it later: the server connects on the first call):

```json
{ "mcpServers": { "drafftink": { "command": "/path/to/drafftink-mcp", "args": ["--attach"] } } }
```

Relay for the browser tab. Claude Desktop hides the server's stderr, so ask the agent to call the `status` tool: it returns the pairing link.

```json
{ "mcpServers": { "drafftink": { "command": "/path/to/drafftink-mcp", "args": ["--relay"] } } }
```

### Claude Code

```sh
claude mcp add drafftink -- /path/to/drafftink-mcp                  # headless
claude mcp add drafftink -- /path/to/drafftink-mcp --attach         # desktop app
claude mcp add drafftink -- /path/to/drafftink-mcp --relay          # browser tab
```

Or commit a `.mcp.json` next to your project:

```json
{ "mcpServers": { "drafftink": { "command": "/path/to/drafftink-mcp", "args": ["--attach"] } } }
```

### Other MCP clients (Cursor, Zed, VS Code, Codex, Gemini CLI, ...)

Any client that starts a stdio server works. Give it the command and the arguments of the mode you want. Clients that list tools with a hard cap (some stop at 40 across all servers) are the reason for the grouped tool design in section 4: this server offers 29 tools (`tools/list` of the headless mode; the number follows the registry).

### Streamable HTTP (the desktop app)

For MCP clients that take a URL. The app serves MCP itself; no helper binary is needed. It is off until you ask:

```sh
drafftink --mcp-http            # port 3737
drafftink --mcp-http 4000       # another port; 0 picks a free one
```

or switch on "Serve MCP over HTTP" under Agents in Preferences (the setting `agent.mcp_http`, which a person can change and an agent cannot; the switch is on the desktop app only). The app prints the address, the token file and a reminder on stderr. The endpoint is `http://127.0.0.1:3737/mcp`. Every request must carry `Authorization: Bearer <token>`. The token is new at every start and is kept in a private file (mode 0600) in the `mcp-http` folder of the app's runtime folder (section 5.3), as `<port>.token`, next to `current.port`:

```sh
run="${DRAFFTINK_BRIDGE_DIR:-${XDG_RUNTIME_DIR:+$XDG_RUNTIME_DIR/drafftink}}"   # Linux; section 5.3 has the other places
dir="${run:-$HOME/.local/state/drafftink/run}/mcp-http"
export DRAFFTINK_MCP_TOKEN="$(cat "$dir/$(cat "$dir/current.port").token")"
```

A client keeps the token in an environment variable, so it never lands in a file you commit. Restarting the app makes a new token: export it again and restart the client.

Claude Code:

```sh
claude mcp add --transport http drafftink http://127.0.0.1:3737/mcp --header "Authorization: Bearer $DRAFFTINK_MCP_TOKEN"
```

Or in a `.mcp.json` next to your project (Claude Code expands `${VAR}`):

```json
{ "mcpServers": { "drafftink": { "type": "http", "url": "http://127.0.0.1:3737/mcp",
    "headers": { "Authorization": "Bearer ${DRAFFTINK_MCP_TOKEN}" } } } }
```

Cursor (`~/.cursor/mcp.json` or `.cursor/mcp.json`):

```json
{ "mcpServers": { "drafftink": { "url": "http://127.0.0.1:3737/mcp",
    "headers": { "Authorization": "Bearer ${env:DRAFFTINK_MCP_TOKEN}" } } } }
```

VS Code (`.vscode/mcp.json`):

```json
{ "servers": { "drafftink": { "type": "http", "url": "http://127.0.0.1:3737/mcp",
    "headers": { "Authorization": "Bearer ${env:DRAFFTINK_MCP_TOKEN}" } } } }
```

Any other client: give it the URL and the header. A client that cannot send a header (Claude Desktop) uses `drafftink-mcp --attach` above. Check the door by hand:

```sh
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:3737/mcp                                   # 401: no token
curl -s -H "Authorization: Bearer $DRAFFTINK_MCP_TOKEN" -H 'Content-Type: application/json' \
     -H 'Accept: application/json, text/event-stream' \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' http://127.0.0.1:3737/mcp | head -c 300      # 400 until a client has sent initialize; use a real client
```

### WebMCP (the browser tab)

Nothing to install. When the browser has WebMCP, drafft.ink registers its tools with it at start, and the browser's own agent (or an extension that reads the page's tools) can call them. Check it in the page console: `drafftink.webmcp.status()` answers `{supported, api, registered, tools}`. `?webmcp=off` in the address keeps the page from registering. Chrome offers WebMCP as an origin trial (as of November 2026); see section 5.5 for the API the page uses.

### Options

Options of `drafftink-mcp` (the options of `drafftink --mcp-http` are above).

| Option | Mode | Meaning |
|---|---|---|
| `--attach` | attached | Connect to the desktop app. |
| `--port N` | attached | The app's bridge port. Default: the one in `current.port`. |
| `--bridge-dir DIR` | attached | Where the app's token files are. Default: the app's runtime folder. |
| `--url ws://127.0.0.1:N/` | attached | Skip discovery. Needs `--token-file` or `DRAFFTINK_BRIDGE_TOKEN`. |
| `--token-file FILE` | attached | Read the token from this file on every reconnect. |
| `--relay` | relay | Listen for the browser tab. |
| `--relay-port N` | relay | Listen on this port. Default: a free one. |
| `--page-url URL` | relay | The page the pairing link opens. Default `https://drafft.ink/`. |
| `--allow-origin ORIGIN` | relay | Another page origin that may connect, for a self-hosted drafft.ink. Repeatable. |
| `--allow-dir DIR` | headless | A folder that files may be read from and written to. Repeatable. |
| `--allow-any-path` | headless | No folder limit. |

The token is never a command-line flag (arguments are visible to other users of the machine). `DRAFFTINK_BRIDGE_TOKEN` carries it when a sandbox cannot read the token file.

## 3. Security model

The threat: a program, a web page or another user on the same machine tries to drive the editor, or the editor's channel is used to leak data. The rules are chosen so that a web page and another user are stopped, and so that a program running as the same user is treated as that user.

| Rule | Where | Stops |
|---|---|---|
| Listens on `127.0.0.1` only. The code refuses any other address (`BridgeError::NotLoopback`), and so does the relay. | `BridgeServer::start`, `RelayServer::start` | other machines |
| A 32-byte random token per launch (OS random generator, 64 hex characters), compared in constant time. | `files::new_token`, `guard::tokens_match` | guessing |
| The token file is created new with mode `0600` in a directory of mode `0700` (checked, tightened, symlinks refused, owner checked). It is removed on exit, and only if it still holds this launch's token. The token is printed to stderr once at launch. | `files::TokenFile` | other users |
| `Host` must be `127.0.0.1:<port>` or `localhost:<port>`. | `guard::check_host` | DNS rebinding |
| `Origin` must be absent (a program), a loopback origin, or one that is configured (the relay allows `https://drafft.ink`). `null` and every other origin get HTTP 403 before the WebSocket starts. | `guard::check_origin` | a web page calling the bridge |
| The first frame must be `hello` with the token, within 5 seconds of connecting (the upgrade included). Before that, a frame over 4 KiB is refused. | `server::authenticate` | idle and slow connections |
| Frames are limited (4 MiB to the server, 16 MiB to the relay), text only, one in-flight request per connection, 8 connections, 8 bad frames. | `server::serve` | memory use, floods |
| Requests run on the shell's thread only, 64 per frame, 256 queued; more get `busy`. A request that the shell does not run within 30 s gets a timeout that says the request was **not** cancelled. | `host::BridgeHost` | starving the UI, double-applying |
| Commands with `agent: false` (gestures, tool hold, viewport size) are refused with `not_for_agents`, also inside a batch. UI-level methods such as clicks, key presses and quitting do not exist on the bridge. | `core::dispatch` | the agent acting as a pointer |
| Files in headless mode are limited to the allowed folders; paths are made canonical first (symlinks and `..` do not escape). | `ports::FileScope` | reading the home folder |
| Every call is recorded in the engine's ring with origin `agent`. | `core::dispatch` | silent edits: `recent` shows them |
| **MCP over HTTP** is off unless a person asks (`--mcp-http`, or the Preferences switch). The setting `agent.mcp_http` can be written by a person only: an agent's `settings.set`, `toggle` and `reset` refuse it, so the agent that the door would serve cannot open it. | `KeyDef::user_only`, `app::agent_server` | an agent widening its own access |
| The HTTP server listens on `127.0.0.1` only (`BridgeError::NotLoopback`), with the same token rules as the bridge: 32 random bytes per launch, a private 0600 file in a 0700 folder, removed on exit, compared in constant time. | `McpHttpServer::start` | other machines, other users |
| Every HTTP request needs `Authorization: Bearer <token>` (401 and `WWW-Authenticate: Bearer` without it). The token is never read from the address or a cookie. | `guard::check_bearer` | a page that makes the browser send a request |
| HTTP `Host` must be `127.0.0.1:<port>` or `localhost:<port>`; `Origin` must be absent or a loopback page; only `/mcp` exists (404). The checks run before any MCP code, `Host` and `Origin` first (403). No CORS header is ever sent and a preflight is not approved, so no page can read an answer. | `mcp::http::check` | DNS rebinding, a web page calling the server |
| HTTP bodies are limited to 4 MiB, 32 requests are handled at once (503 beyond), and every call goes through the same queue as the bridge (64 per frame, 30 s, not cancelled on timeout). | `mcp::http`, `host::BridgeHost` | memory use, starving the UI |
| **WebMCP** registers nothing unless the browser has the API, and the browser decides who may call the tools (same origin by default; the page never sets `exposedTo`). Read tools carry `readOnlyHint`, tools that return document text carry `untrustedContentHint`. A call runs as origin `agent` and is one undo step. | `webmcp_page` | the page exposing itself to other origins |

What this does not stop: a program running as the same user can read the token file and do what the user can. That is the same trust as the user's shell. In attached mode an agent that holds the token can also run `doc.save` and `doc.export` to any path the app can write; the folder limit is a headless-mode guard against accidents, not a boundary. Approve tool calls in your MCP client the way you would approve shell commands; the tools carry honest hints (`readOnlyHint` is true only for tools that cannot change anything).

The command registry marks some commands `agent: true` that change user preferences (`settings.set`, `settings.reset`). They are there because the registry says so; the registry is the place to change that.

The bearer token of the HTTP door is as sensitive as the bridge token. A program that runs as you can read the token file and do what you can; a web page and another user cannot. WebMCP has no token: the browser is the trust boundary, as for any WebMCP page. Whoever the browser lets call the tools can edit and read the open document, so turn WebMCP off with `?webmcp=off` on a machine where you do not trust the browser's agent.

## 4. The tools

The tool list is generated from the command registry. Nothing is written by hand: a command appears as soon as it is registered with `agent: true`. `tools/list` is checked against the registry by `apps/drafftink-mcp/tests/protocol.rs`.

**Decision: one tool per family, not one per command.** The registry has well over a hundred agent commands. MCP clients degrade with that many tools: some cap the total across servers, and each definition costs context on every turn. So:

- Each family that changes state is one tool taking `{command, args}`: `element`, `style`, `diagram`, `arrange`, `arrow`, `frame`, `group`, `history`, `tab`, `tool`, `view`, `settings`, ... The tool description lists every command of the family with its summary, its argument signature cut from the command's own schema, and a short example, so the model usually does not need a second call.
- All read-only commands (`doc.outline`, `doc.inspect`, `element.query`, `element.at`, `selection.get`, `view.get`, `history.state`, `view.screenshot`, ...) form one tool, `inspect`, with `readOnlyHint: true`. A client can auto-approve reads without approving edits. A read-only command called through an editing tool, or the reverse, is a tool error that names the right tool.
- Fixed tools: `batch` (several commands as one atomic undo step), `commands` (the registry with live availability), `describe` (full schema, example and refusal codes of one command; the argument is `command`, and the bridge method it calls takes `id`, section 5.1), `recent` (the newest calls by everyone), `status` (mode and connection; in relay mode, the pairing link).
- File tools `open`, `save` and `export` exist when `doc.open`, `doc.save` and `doc.export` are registered. They are shortcuts with a `path` argument for those commands.

Results: a successful call returns the command's output as compact JSON. A picture (`view.screenshot`, PNG export without a path) becomes an MCP image block. A refusal is a tool result with `isError: true` whose text is the refusal's plain words (and `Try: ...` lines for its suggestions); the model reads how to proceed. A failed batch starts with `Step N (`id`) refused: ...` and says whether it was rolled back. An unknown tool name is a JSON-RPC error `-32602` that lists the real tools.

The MCP prompt `drafftink_guide` teaches the model the one-page workflow (look with `doc.outline`; build with `diagram.create`; find by meaning with `element.query`; group edits in `batch`; undo; read refusals). The `initialize` result carries the short form. `crates/bridge/src/tools/guide.rs` holds the text; a test checks that every command it names exists or is in the catalog, and the guide lists the ones this build does not have yet.

## 5. The protocols

### 5.1 The bridge (desktop app)

JSON-RPC 2.0 over WebSocket at `ws://127.0.0.1:<port>/`, one JSON object per text frame, by-name parameters only, no batch arrays.

1. Connect. Send `Host: 127.0.0.1:<port>` (any client library does) and no `Origin`, or a loopback one.
2. First frame, within 5 s: `{"jsonrpc":"2.0","id":1,"method":"hello","params":{"token":"<64 hex>"}}`. The answer: `{"protocol":1,"server":"drafftink","version":"...","origin":"agent","registry":{"count":N,"digest":"..."},"methods":[...],"limits":{"max_frame_bytes":4194304}}`. A wrong token is error `-32001` and the socket closes; any other first message is `-32003`.
3. Methods:

| Method | Params | Result |
|---|---|---|
| `execute` | `{id, args?}` | the command's output (`changed`, `created`, `deleted`, `undo_label`, `warnings`, and its own keys) |
| `batch` | `{steps: [{id, args?, as?}], opts?: {label?, atomic?, dry_run?}}` | `{completed, results, failed?, rolled_back, undo_label?, changed, pending_render?}`; a batch that ran and failed is a normal result with `failed`. A batch that exports a PNG on the web waits for the picture (the host runs it again, `pending_render`) and is answered when it completes |
| `commands` | `{family?, search?, available_only?, include_schemas?}` | every command with live availability |
| `describe` | `{id}` | schemas, facts, example, refusals |
| `recent` | `{limit?, mutating_only?, since_seq?}` | `{entries, next_seq, capacity}` |
| `state` | `{}` | `engine.state` |
| `ping` | `{}` | `{}`, answered by the shell (shows that it is responsive) |
| `subscribe` / `unsubscribe` | `{}` | `{subscribed}`; after `subscribe` the server sends the notification `changed` with `{seq, revision}` when the ring or the document revision moved (at most once per shell frame) |

4. A command refusal is a JSON-RPC error with code `-32010`, `message` equal to the refusal's plain text, and `data` equal to the whole refusal (`code`, `command`, `message`, `suggest`, `details`). Other codes: `-32700` parse, `-32600` invalid request, `-32601` unknown method, `-32602` invalid params, `-32002` the shell did not run the request in time (it is not cancelled), `-32004` busy, `-32005` not connected (relay and attached clients).

The shell side: `BridgeServer::start(config, waker)` returns the server and a `BridgeHost`. The server's threads never touch the `Session`. The shell calls `host.poll(&mut session)` at the top of each frame; a request calls the waker (for example `ctx.request_repaint()`), so an idle app does nothing.

### 5.2 The relay (browser tab)

The roles turn around. The relay is the WebSocket server and, after pairing, the JSON-RPC **client**; the page is the WebSocket client and the JSON-RPC **server**.

```
agent -- stdio MCP --> drafftink-mcp --relay  <-- WebSocket (page connects out) -- drafft.ink tab
                       (JSON-RPC client)                                          (JSON-RPC server on its Session)
```

**Pairing.** The relay makes a one-time pairing token and prints this link (and returns it from the `status` tool):

`https://drafft.ink/#drafftink-agent-port=<port>&drafftink-agent-token=<64 hex>`

A fragment is never sent to a server. The link carries only a port and a token: the page builds the URL itself as `ws://127.0.0.1:<port>/`, so a link cannot point it at another host.

**What the page does** (to build in the web app as `window.drafftink.agent`):

1. At start, read `location.hash`. If it holds both keys, remove them with `history.replaceState` at once, then pair. The UI also offers "Connect an agent" with a field for the link or token and port.
2. `window.drafftink.agent.connect(url, token)` refuses unless `url` is `ws://` with the host `127.0.0.1`, `localhost` or `[::1]`. It rejects with a message that says the bridge only listens on loopback.
3. Open the WebSocket. The browser sends `Origin` and `Host` itself.
4. Send, as the first frame: `{"jsonrpc":"2.0","id":1,"method":"hello","params":{"token":"<pairing or resume token>","page":{"version":"...","registry":{"count":N,"digest":"..."}}}}`.
5. The relay answers `{"protocol":1,"server":"drafftink-relay","role":"relay","resume_token":"<64 hex>","calls":[...],"limits":{...}}`. The pairing token is now used up. Store `resume_token` and the port in `sessionStorage` (per tab, gone when the tab closes). A wrong or used token is error `-32001` and close code 1008: show "ask for a new link".
6. From now on the relay sends requests. The methods and parameters are exactly the table in 5.1 (`execute`, `batch`, `commands`, `describe`, `recent`, `state`, `ping`). The page answers each with a response that has the same `id`, computed by the same `core::dispatch` logic (the `drafftink-bridge` core builds for wasm) on its `Session`, queued and run between frames like any command. Origin `agent` goes into the ring. A page must answer every request. It may send `ping` to the relay (answer `{}`). It sends nothing else.
7. When the socket closes with a code other than 1008, reconnect with the resume token after 1 s, 2 s, 4 s, up to 30 s, for 10 minutes. After a reload, a tab that finds a resume token in `sessionStorage` reconnects the same way without the user doing anything. A newer connection replaces the older one.
8. `window.drafftink.agent.disconnect()` closes with 1000 and forgets the resume token.

**The relay's checks** are those of the bridge: loopback bind, `Host` check, `Origin` check (the default allow-list is `https://drafft.ink`; `--allow-origin` adds a self-hosted origin; loopback origins always pass, for local development), 5 s to say `hello`, 16 MiB frames. A call to a page that is not connected is error `-32005` whose text says how to pair or reload; a call the page does not answer in 60 s is `-32002` and says that the request was not cancelled.

**Browser notes.** An `https://` page may open `ws://127.0.0.1` in Chrome and Firefox (loopback counts as a secure context); Safari blocks it, so use the desktop app there. Recent Chrome versions ask the user to allow a site to reach local network addresses: the user must allow it. A hidden tab may run late; the timeout text says so.

The fake page `drafftink_bridge::page_sim::FakePage` plays these steps against a real `Session`; `crates/bridge/tests/relay.rs` and `apps/drafftink-mcp/tests/modes.rs` use it. It is the executable form of this section, and the web app's tests should run the same scenarios against the real page.

### 5.3 Finding the app (attached mode)

The app's runtime folder is `$DRAFFTINK_BRIDGE_DIR`, else `$XDG_RUNTIME_DIR/drafftink`, else `~/.local/state/drafftink/run` (Linux), `~/Library/Application Support/drafftink/run` (macOS), `%LOCALAPPDATA%\drafftink\run` (Windows). It holds `current.port` (the newest launch) and `<port>.token`. The MCP server refuses a token file that other users can read.

### 5.4 MCP over streamable HTTP (desktop app)

The app serves the same tools as `drafftink-mcp` (section 4) on `http://127.0.0.1:<port>/mcp`, using the official `rmcp` SDK's streamable HTTP transport (the `initialize` handshake, `tools/list`, `tools/call`, `prompts`, event streams for clients that open them). The code is `crates/bridge/src/mcp/`. The tool list, the arguments, the results and the refusal text are those of section 4, because one generator builds them (`crates/bridge/src/tools/`); `status` answers `{"mode":"desktop","connected":true,"listening":...}`.

- **Start.** `drafftink --mcp-http [PORT]` (default 3737; 0 is a free port), or the setting `agent.mcp_http`. The flag is for one launch and does not change the setting; the switch starts the server when it turns on and stops it when it turns off. If the port is taken, Preferences says so in words and the flag exits with the error.
- **Door.** Section 3 lists the checks. In short: loopback bind, bearer token, `Host`, `Origin`, path `/mcp`, 4 MiB bodies.
- **Where the token is.** `<runtime folder>/mcp-http/<port>.token` and `current.port` beside it (section 5.3 names the runtime folder; `DRAFFTINK_BRIDGE_DIR` moves it). The bridge keeps its own files in the folder above, so the two never share a token.
- **Which session.** The app's open document, through the app's job queue: the request runs on the app's thread between frames, with origin `agent`, like a bridge request. The bridge and the HTTP server can run together; they share the queue.
- **Renderer.** `view.screenshot` and PNG export use the app's renderer; the picture is an MCP image block.

`crates/bridge/tests/mcp_http.rs` runs a real MCP client (the `rmcp` client over HTTP) against the server with a simulated shell, and every refusal of the door. `e2e/native/tests/mcp_http.rs` starts `drafftink --mcp-http 0` on a display and builds the flowchart of section 7 through a real client.

### 5.5 WebMCP (the page)

WebMCP is the browser API with which a page hands tools to the browser's agents. It is a draft of the Web Machine Learning Community Group (November 2026), in Chrome as an origin trial. The page feature-detects it and does nothing when it is absent. What the page uses:

- `document.modelContext` (the current draft) or, in the first previews, `navigator.modelContext`. The page prefers the document, falls back to the navigator, and needs only `registerTool`.
- `registerTool({name, title, description, inputSchema, annotations, execute}, {signal})`, once per tool. The signal belongs to one `AbortController`; aborting it removes the tools (`drafftink.webmcp.unregister()`). Where the preview API has `unregisterTool(name)`, that is called too. A registration that fails is logged as a warning and does not stop the others. The page never passes `exposedTo`, so the tools are same-origin.
- `execute(input, {signal})` runs `Service::call` (section 4) on a `PageCaller`, which queues the bridge method on the page's job queue, as `window.drafftink.execute` does. It resolves with `{content: [{type: "text", text} | {type: "image", data, mimeType}], isError}`, the MCP result shape; the browser serialises it to JSON for the agent. A refusal is a result with `isError: true` and the words, not a rejection. Arguments that are not an object are an error result as well.
- The tools are the ones of section 4, generated from the registry: `inspect` (read-only), one tool per family, `batch`, `commands`, `describe`, `recent`, `status`, and `open`/`save`/`export` when registered. Names use letters, digits and `_` only, within the draft's 1 to 128 characters. Annotations: `readOnlyHint` for reads; `untrustedContentHint` for `inspect` and `recent`, whose results carry text from the document; `consequentialHint` false for all (an edit is one undo step).

`window.drafftink.webmcp` has `status()` (`{supported, api, registered, tools}`), `register()` (true when the tools are registered) and `unregister()`. `?webmcp=off` in the address (also `0`, `false`, `no`) stops the registration at start; `register()` still works. The pure rules are in `apps/drafftink-web/src/webmcp.rs` (unit-tested natively), the browser glue is in `webmcp_page.rs`. `e2e/web/specs/shell/webmcp.spec.ts` installs a stand-in for either API before the page starts, checks that the tools are registered, calls them as a browser agent does and reads the document back; it also checks that a browser without WebMCP is unaffected.

## 6. Limits

| What | Limit |
|---|---|
| Frame to the app | 4 MiB (`max_frame_bytes`) |
| Frame to the relay or from the app | 16 MiB / 64 MiB |
| Connections to the app (bridge) | 8 |
| Requests handled at once, MCP over HTTP | 32 (then `503`) |
| Request body, MCP over HTTP | 4 MiB |
| Requests queued for the shell | 256; 64 run per frame |
| Wait for the shell | 30 s, then `-32002` (not cancelled) |
| Wait for the page (relay) | 60 s, then `-32002` (not cancelled) |
| Steps in a batch | 10 000 and 10 MiB (engine limit) |
| Time to say `hello` | 5 s |

## 7. Worked examples

Each example is a tool call as the model makes it. They are written so that a test can replay them; `apps/drafftink-mcp/tests/modes.rs` runs the flowchart (with `diagram.create` once it is registered) and the refusal example today.

**Look before drawing.**

```
inspect {"command": "doc.outline"}
```

Returns nodes, edges, groups, frames and bounds. For one line per node and edge: `{"command": "doc.outline", "args": {"format": "text"}}`.

**A flowchart in one call.**

```
diagram {"command": "diagram.create", "args": {
  "nodes": [{"id": "start", "label": "Start", "shape": "ellipse"},
            {"id": "check", "label": "Valid?", "shape": "diamond"},
            {"id": "ok", "label": "Save"}, {"id": "bad", "label": "Show error"}],
  "edges": [{"from": "start", "to": "check"}, {"from": "check", "to": "ok", "label": "yes"},
            {"from": "check", "to": "bad", "label": "no"}],
  "layout": {"algorithm": "layered", "direction": "down"}}}
```

The result maps your symbolic ids to real element ids. Edges are arrows bound to the shapes, so moving a node moves its arrows. Then `inspect {"command": "doc.outline"}` shows `Start`, `Valid?`, `Save` and `Show error`, and `history {"command": "history.undo"}` removes the whole diagram in one step.

**Find by meaning, then change.**

```
inspect {"command": "element.query", "args": {"filter": {"text": {"contains": "error"}}}}
style {"command": "style.set", "args": {"ids": ["<id from the query>"], "props": {"stroke_color": "red-600"}}}
```

**Edits that belong together, atomically.**

```
batch {"label": "Add a titled box", "steps": [
  {"command": "element.create", "args": {"kind": "rectangle", "name": "box", "bounds": {"x": 0, "y": 0, "w": 160, "h": 80}}, "as": "box"},
  {"command": "element.relabel", "args": {"id": "$box", "text": "Hello"}}]}
```

Every step is checked before the first runs. If one refuses, nothing changed and the result starts with `Step N (`id`) refused: ...`. `history.undo` undoes the whole batch.

**Reading a refusal.** This runs on an empty document:

```
view {"command": "view.zoom_to_fit"}
```

The tool result is an error whose text names the way out: it says there is nothing to fit and to use `view.center`. The document and the camera are unchanged.

**What did the user change?**

```
recent {"limit": 20, "mutating_only": true}
```

Entries have `source`: `agent` for your calls, `ui`, `shortcut` and so on for the user's.

**Export without a GPU (headless).**

```
export {"format": "svg", "path": "/Users/me/diagrams/flow.svg"}
export {"format": "png", "path": "/Users/me/diagrams/flow.png"}   // refused: says to use svg or attach to the app
```

**Pair the browser tab (relay).**

```
status {}
```

returns `{"mode":"relay","connected":false,"paired":false,"pairing_link":"https://drafft.ink/#drafftink-agent-port=...","next_step":"Ask the user to open the pairing link ..."}`. After the user opens it, `status` shows `"connected": true` and every other tool drives the tab.

## 8. Testing and extending

- `cargo test -p drafftink-bridge`: JSON-RPC round trips against a real `Session`; security tests (wrong token, no `hello` within the deadline, bad `Origin`, bad `Host`, non-loopback bind, oversized frame, binary frame, connection cap, token file permissions); the relay with the fake page (pairing, one-time token, resume, replacement, timeouts).
- `cargo test -p drafftink-bridge --test mcp_http`: a real MCP client over HTTP against the desktop server with a simulated shell; token, `Host`, `Origin`, preflight, path, loopback and token-file tests.
- `cargo xtask e2e`: the native suite runs `e2e/native/tests/mcp_http.rs` (a client over HTTP builds a flowchart in the running app and sees its picture), and the browser suite runs `webmcp.spec.ts`.
- `cargo test -p drafftink-mcp`: MCP protocol tests over an in-memory pipe and over the real binary's stdio (initialize, `tools/list` against the registry, refusal text as a tool error, batch rollback, prompts); attached mode against a simulated shell; relay mode against the fake page; the flowchart.
- A new command needs nothing here. A new tool kind needs a variant in `crates/bridge/src/tools/catalog.rs` and a branch in `service.rs`: the stdio server, the HTTP server and the page all pick it up.
