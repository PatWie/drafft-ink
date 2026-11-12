# The headless command line: `drafftink` subcommands and `drafftink-cli`

`apps/drafftink-cli` owns an editor `Session` without a window and drives it through the same `execute` and `execute_batch` as the app, MCP and the tests. A script that works here does the same everywhere. The tool adds nothing the registry does not know: opening a file is `doc.open`, saving is `doc.save`, SVG is `doc.export`, a Mermaid file is `diagram.import_mermaid`, the outline is `doc.outline`.

Two names, one program. The desktop app has these as subcommands, and the same code is the small standalone binary `drafftink-cli` for servers and CI, where the window and its libraries are not wanted:

```sh
cargo install drafftink                     # drafftink convert ..., drafftink run ..., drafftink mcp ...
cargo install drafftink-cli                 # drafftink-cli convert ..., the same without the window
cargo build --release -p drafftink-cli      # target/release/drafftink-cli, from the repository
drafftink --help                            # lists the subcommands, one line each
drafftink help convert                      # or: drafftink convert --help
```

`drafftink convert|run|outline|inspect|commands|schemas ...` behaves exactly like `drafftink-cli convert|run|outline|inspect|commands|schemas ...`: the same arguments, results, notes and exit codes. The one difference is the name that an error line starts with (`drafftink: ...` against `drafftink-cli: ...`). `drafftink mcp ...` is `drafftink-mcp ...` (`agents.md`). Everything below says `drafftink-cli`; `drafftink` works in its place. The argument definitions are written once, in the `drafftink-cli` and `drafftink-mcp` crates; the `drafftink` binary embeds them (`src/tool.rs`), so the two cannot drift.

### Subcommand or file

`drafftink` opens a window; a subcommand runs a tool and never creates a window, a GPU device or a connection to the display, so it works over SSH, in a container and with `DISPLAY` and `WAYLAND_DISPLAY` unset. The first word decides:

- `drafftink`, `drafftink FILE` and the window flags (`--room`, `--bridge`, `--mcp-http`, `--data-dir`, ...) open the window, as before.
- A first word that is a subcommand (`run`, `convert`, `outline`, `inspect`, `commands`, `schemas`, `mcp`, `help`) is the subcommand. The window flags do not mix with it: `drafftink --room r outline x` and `drafftink outline x --room r` are usage errors (exit 2).
- A file that has such a name is opened by giving it a path, `drafftink ./run`, or by ending the options with `--`, `drafftink -- run`. Any other name (`drafftink notes.drafftink`, `drafftink ./mcp`) needs nothing.

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
drafftink-cli run build.jsonl --out out.drafftink
drafftink-cli run - --doc in.drafftink --out out.svg <<< '{"id":"doc.set_background","args":{"color":"#fafafa"}}'
drafftink-cli run steps.json --batch --dry-run
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
- Files are read and written wherever the person who runs the tool can. `drafftink-mcp` restricts them to chosen folders because an agent chooses the paths there; this tool is run by the person who chooses.

## Tests

`tests/subcommands.rs` of the `drafftink` package runs the real `drafftink` binary with no display and compares `outline`, `inspect`, `commands`, `schemas`, `convert` and `run` (output, notes, written files, exit codes, failures) with what the `drafftink-cli` code gives for the same words, checks that `drafftink mcp` answers `initialize` and `tools/list` on stdio, and checks `--help` and `help mcp`. The unit tests in `src/main.rs` cover the rule above (subcommand or file) and that a subcommand is dispatched without the window.

`apps/drafftink-cli/tests/cli.rs` runs the real binary: scripts in the three spellings, the refusal path and its exit code, `--keep-going`, batches with names and rollback, standard input, conversions between native, Excalidraw, SVG and Mermaid, the PNG message, the outline and the command listing. Unit tests of the script reader are in `src/script.rs`.
