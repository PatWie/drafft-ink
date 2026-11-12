# The headless command line: `drafftink-headless`

`drafftink-headless` is Drafft.ink without a screen. It has the collaboration server (`serve`, `server.md`), an MCP server (`mcp`, `agents.md`) and the tools for documents on this page. It never links GPU, window, egui or file-dialog code (`cargo xtask layers` checks it), so it runs over SSH, in CI and in a container. The desktop app `drafftink` is the other program, the one with a screen.

The tools own an editor `Session` without a window and drive it through the same `execute` and `execute_batch` as the app, MCP and the tests. A script that works here does the same everywhere. The tool adds nothing the registry does not know: opening a file is `doc.open`, saving is `doc.save`, SVG is `doc.export`, a Mermaid file is `diagram.import_mermaid`, the outline is `doc.outline`.

```sh
cargo install drafftink-headless                 # drafftink-headless convert ..., run ..., serve, mcp
cargo build --release -p drafftink-headless      # target/release/drafftink-headless, from the repository
drafftink-headless --help                        # lists the subcommands, one line each
drafftink-headless help convert                  # or: drafftink-headless convert --help
```

Without a subcommand it prints this help and exits with 2; it never starts serving by itself.

### Where the code is

The argument definitions and the work are written once, in library crates, and the two programs only put them under one name. They have no binary of their own:

| Crate | What it holds | Used by |
|---|---|---|
| `drafftink-cli` | `convert`, `run`, `outline`, `inspect`, `commands`, `schemas` (`cli::Command`, `cli::run_command`) | `drafftink-headless` |
| `drafftink-mcp` | the MCP server: `Args` (all three modes) and `BridgeArgs` (attach and relay) | `drafftink-headless mcp`; `drafftink mcp` takes `BridgeArgs` |
| `drafftink-server` | the collaboration server (`Args`, `run`, `start`) | `drafftink-headless serve` |

The old standalone installs `drafftink-cli`, `drafftink-mcp` and `drafftink-server` are gone as programs. (Their names stay as the library crates; `publishing.md` says what happened to each on crates.io.)

### `drafftink` and the tools that moved

`drafftink convert|run|outline|inspect|commands|schemas ...` and a headless `drafftink mcp` (`--allow-dir`, `--allow-any-path`) do not run anything. They print one line to standard error and exit with 2:

```text
That is part of drafftink-headless: `cargo install drafftink-headless`, then `drafftink-headless convert …`
```

### Subcommand or file

`drafftink` opens a window; `drafftink mcp` is the only subcommand that does something, and it never creates a window, a GPU device or a connection to the display. The first word decides:

- `drafftink`, `drafftink FILE` and the window flags (`--room`, `--bridge`, `--mcp-http`, `--data-dir`, ...) open the window.
- A first word that is a subcommand (`mcp`, `help`, and the names above that print the line about `drafftink-headless`) is the subcommand. The window flags do not mix with `mcp`: `drafftink mcp --room r` is a usage error (exit 2).
- A file that has such a name is opened by giving it a path, `drafftink ./run`, or by ending the options with `--`, `drafftink -- run`. Any other name (`drafftink notes.drafftink`) needs nothing.

Nothing is written unless you name an output file. Results go to standard output, notes and errors to standard error. The exit code is 0 when everything ran, 1 when a step, a file or a format failed, and 2 for a mistake in the arguments.

## Subcommands

| Command | What it does |
|---|---|
| `run SCRIPT [--doc FILE] [--out FILE] [--seed N]` | Runs a command script, optionally on a document, optionally writing the result. `-` reads the script from standard input. |
| `convert INPUT OUTPUT` | Reads a document in one format and writes it in another. |
| `outline FILE [--json]` | Prints the outline of a document: one line per shape and per arrow, in reading order (`doc.outline`). |
| `inspect FILE` | Prints the summary of a document: counts, bounds, groups, frames (`doc.inspect`). |
| `commands [--family F] [--search TEXT] [--json]` | Lists the registry, one line per command. `--json` adds the argument and output schemas of each. |
| `commands ID` | One command with its schemas, facts, refusals and example (`engine.describe`). |

### Scripts

A script is one of:

- **JSON Lines**: one step per line, `{"id": "element.create", "args": {...}}`. Blank lines and lines that start with `//` are skipped, so a script kept in a repository can explain itself.
- A **JSON array** of steps, or **one step**.
- A **batch object** `{"steps": [...], "opts": {...}}`. A step may be named with `"as"`, and later steps use `"$name"` and `"$name.field"`. It runs as one transaction: one undo step, rolled back at the first refusal.

`run` runs plain steps one after the other and prints one JSON line per step:

```json
{"step":1,"id":"element.create","ok":true,"result":{"created":["0tFDtiYO__ed0v2c"],"changed":true,"undo_label":"Create element"}}
{"step":2,"id":"element.delete","ok":false,"refusal":{"code":"not_found","message":"No element matches `nope`. ..."}}
```

It stops at the first refusal (exit 1, nothing written). `--keep-going` runs the rest and still exits 1. `--batch` runs the steps of a list as one batch, with `as` and `$name`; `--dry-run` runs a batch, reports the outputs, and rolls everything back; `--label` sets the undo label. `--seed N` makes element ids and hand-drawn jitter come from a fixed seed, so the same script writes the same file every time (the welcome document is built this way, `media.md`).

```sh
drafftink-headless run build.jsonl --out out.drafftink
drafftink-headless run - --doc in.drafftink --out out.svg <<< '{"id":"doc.set_background","args":{"color":"#fafafa"}}'
drafftink-headless run steps.json --batch --dry-run
```

### Formats

| Format | Read | Write | Notes |
|---|---|---|---|
| Native `.drafftink`, `.json` | yes | yes | Versioned JSON with images embedded. Documents of the first version (no version field) are migrated on reading. `--from legacy` says the same as `--from native`. |
| Excalidraw `.excalidraw` | yes | yes | Fields Excalidraw has no place for are dropped, with a warning. |
| Mermaid `.mmd`, `.mermaid` | yes | no | Flowchart and sequence diagram. Subgraphs become boxes (`commands.md` 2.19). |
| PNG with the scene embedded | yes | **no** | Writing a PNG needs a GPU. This tool has none, so `--to png` or a `.png` output fails at once with a way out: export SVG, or export PNG from the desktop app, or attach `drafftink mcp --attach` to it. |
| SVG | no | yes | Fonts embedded. The whole document, not the selection. |

The format is taken from the extension, or from `--from` and `--to`.

## What it does not do

- No PNG and no screenshots (no renderer port). A script that calls `doc.export` with `format: "png"` gets the engine's refusal and its words.
- No clipboard, no persistent storage, no network: commands that need them refuse with `port_unavailable`.
- Files are read and written wherever the person who runs the tool can. `drafftink-headless mcp` restricts them to chosen folders because an agent chooses the paths there; these tools are run by the person who chooses.

## Tests

`apps/drafftink-headless/tests/subcommands.rs` runs the real `drafftink-headless` binary with no display and compares `outline`, `inspect`, `commands`, `schemas`, `convert` and `run` (output, notes, written files, exit codes, failures) with what the `drafftink-cli` library gives for the same words. It also checks that no subcommand prints the help with exit 2, `--version`, and that `mcp` answers `initialize` and `tools/list` on stdio. `tests/cli.rs` of that package covers the tools one by one: scripts in the three spellings, the refusal path and its exit code, `--keep-going`, batches with names and rollback, standard input, conversions between native, Excalidraw, SVG and Mermaid, the PNG message, the outline and the command listing. `tests/mcp_stdio.rs` is the stdio protocol and the mode flags, `tests/serve.rs` starts `serve` on a free port and asks `/health`, and `tests/intro.rs` checks the welcome document. Unit tests of the script reader are in `apps/drafftink-cli/src/script.rs`.

`tests/subcommands.rs` of the `drafftink` package checks the other side: the line and the exit code of every tool that moved, `mcp` attaching by default and saying so on its first call when no app is running, and `--help`. The unit tests in `src/main.rs` cover the rule above (subcommand or file).
