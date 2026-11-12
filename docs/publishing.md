# Publishing to crates.io

`cargo install drafftink` installs the desktop app, `cargo install drafftink-headless` the program without a screen (the collaboration server, the headless MCP server and the tools for documents), and the
libraries below are there for anyone who builds on them. This page says what is published, how the hand-drawn fonts are
kept out of it, and what the maintainer runs by hand. The tooling never publishes anything: `cargo xtask publish` is a dry
run and refuses every other mode.

## What is published

Version 0.2.0 for every crate of ours, except the two vendored ones.

| Crate | What |
|---|---|
| `drafftink-geom`, `-profile`, `-protocol` | L0 |
| `drafftink-scene`, `-typeset`, `-sketch` | L1 |
| `drafftink-rex` 0.1.2, `drafftink-unicode-math` 0.1.0 | copies of ReX and its symbol tables, used by `typeset` (below) |
| `drafftink-render` | L2 |
| `drafftink-canvas`, `-doc` | L3 |
| `drafftink-edit`, `-io`, `-collab` | L4 |
| `drafftink-engine` | L5 |
| `drafftink-gpu`, `-ui`, `-bridge` | L6 |
| `drafftink-app` | the glue the desktop launcher needs (L7) |
| `drafftink-cli`, `drafftink-mcp`, `drafftink-server` | libraries only, with no binary: the tools for documents, the MCP server and the collaboration server. They hold the clap definitions and the work; the two programs below are thin wrappers (below) |
| `drafftink` | the desktop app: the package at the repository root (`src/main.rs`). Its one subcommand, `mcp`, is the bridge to the open window and uses `drafftink-mcp` |
| `drafftink-headless` | the program without a screen, a binary for `cargo install`: `serve` (`drafftink-server`), `mcp` (`drafftink-mcp`) and `convert`, `run`, `outline`, `inspect`, `commands`, `schemas` (`drafftink-cli`). Features `sqlite`, `s3` and `hand-fonts` pass through to the libraries |
| `drafftink-core` 0.2.0 | an empty placeholder in `deprecated/drafftink-core` (below) |

Not published (`publish = false`): `drafftink-testkit`, `drafftink-e2e`, `xtask`, `drafftink-canvas-js` (the npm package
`drafftink-canvas`) and `drafftink-web` (the wasm app). No published crate has one of them as a normal dependency.
Dev-dependencies on workspace crates are written with `path` and no `version`; cargo leaves such a dev-dependency out of the
package, so the tests of a crate can use the testkit and the hand-drawn fonts without the package needing them.

Names on crates.io: the owner of the account already holds `drafftink` (0.1.1, the old app), `drafftink-core` (0.1.0),
`drafftink-render` (0.1.0) and `drafftink-server` (0.1.0, the old server binary). The other names are new.

## The hand-drawn fonts are not on crates.io

GelPen, GelPen Serif and Vanilla Extract are by Shara Weber. Their redistribution terms are not confirmed (`ATTRIBUTION.md`),
so no package on crates.io contains them. They stay in the Git repository and in the web app on drafft.ink.

- `drafftink-typeset` has the Cargo feature `hand-fonts` (off by default). With it the seven files are embedded. Without it
  the crate builds and works with the OFL fonts only: the three families are drawn with Noto Sans (`fonts::face_for`,
  `font_file`), formulas use Noto Sans for their letters, and the family names stay in documents.
- `Cargo.toml` of `drafftink-typeset` lists the files in `exclude`, so `cargo package` never has them. `build.rs` fails the
  build with a plain message when the feature is on and a file is missing, so nobody gets "couldn't read GelPen.ttf" from
  `include_bytes!`.
- `cargo xtask publish` lists the files of every package and fails when a name contains `gelpen` or `vanilla`.
- Other crates pass the feature on: `drafftink-app`, `drafftink-engine` (`hand-fonts`), `drafftink` (root), `drafftink-web`,
  `drafftink-headless`, `drafftink-mcp` and `drafftink-cli`. The libraries do not turn it on themselves, because a package from crates.io
  must build without it.
- Repository builds enable it: `scripts/build-native.sh`, `scripts/run-native.sh`, `scripts/build-web.sh` (always, so the
  deploy and release jobs have it), `scripts/package-linux.sh`, `package-macos.sh`, `package-windows.ps1` (so `release.yml`
  has it), `cargo xtask size --native`, `cargo xtask perf --app native` and `cargo xtask e2e`. `--all-features` runs
  (`cargo xtask clippy`, `test`, `wasm`) have it. The tests of the crates that draw text have a path-only dev-dependency on
  `drafftink-typeset` with the feature, and `drafftink-testkit` and `drafftink-canvas-js` always use it.
- `cargo xtask test` also runs `cargo test -p drafftink-typeset` alone, which has no `hand-fonts` (a single `-p` build does
  not unify features with the other members), so the Noto Sans stand-in is tested with the same command that a
  crates.io user would run. Tests that need the exact fonts carry `#[cfg(feature = "hand-fonts")]`.
- Loading the fonts at run time from a folder (next to the binary or in the config directory) is not done: the family
  table is a pure function of the style, and a font that arrives later would need state in it. The files would need the same
  permission anyway.

The notice, in the wording of the owner, is in `README.md` (Licence), `ATTRIBUTION.md` and
`crates/typeset/assets/licenses/HAND-FONTS-NOTICE.txt`: these fonts are not covered by the AGPL, they remain under Shara
Weber's terms, and they are not part of the crates.io packages.

## ReX is vendored

crates.io takes no Git dependency, and ReX (TeX math layout) was one. `third_party/rex` is a copy of KenyC/ReX at the old
pin with only the `ttf-parser` backend, published as `drafftink-rex` (library name `rex`), and `third_party/unicode-math` is
its symbol-table crate, published as `drafftink-unicode-math`. Details and licences: `typeset.md` (ReX), `ATTRIBUTION.md`.

## `drafftink-core`

`deprecated/drafftink-core` is a crate with no code. Its README and crate docs say that the crate is replaced by
`drafftink-engine`, `drafftink-scene` and the others, with links. It has a workspace of its own and `exclude = ["deprecated/*"]`
keeps it out of the repository workspace, so no build, test or lint touches it. Publishing it as 0.2.0 gives the name a page
that points to the new crates. After it is published, yank 0.1.0.

## The two programs and the library crates

There are two programs, split by a screen, and three library crates that hold their code:

- `drafftink-headless` (`apps/drafftink-headless`) depends on `drafftink-cli`, `drafftink-mcp` and `drafftink-server` (normal dependencies with a `version`). Its `src/main.rs` is a clap enum with `Serve(drafftink_server::Args)`, `Mcp(drafftink_mcp::cli::Args)` and the flattened `drafftink_cli::cli::Command`, and three calls. It links no GPU, window, egui or file-dialog code; `cargo xtask layers` checks the resolved graph.
- `drafftink` (root) depends on `drafftink-mcp` only, for `drafftink mcp` (`drafftink_mcp::cli::BridgeArgs`: attach by default, or relay). `convert`, `run`, `outline`, `inspect`, `commands`, `schemas` and a headless `mcp` are hidden subcommands that print where they went (`src/tool.rs`).
- `drafftink-cli`, `drafftink-mcp` and `drafftink-server` have a `lib.rs` and no `main.rs`. `cargo install drafftink-server` therefore fails with cargo's "there is nothing to install" message, and the README of each crate says to install `drafftink-headless`.

The publish order puts the three libraries before `drafftink-headless` and `drafftink-mcp` before `drafftink`; `cargo xtask publish` prints it. The `hand-fonts` feature of `drafftink` turns on `drafftink-app` and `drafftink-mcp`; the one of `drafftink-headless` turns on `drafftink-cli` and `drafftink-mcp`, so a repository build draws the same in the window and in the tools. A version bump (below) touches the path dependencies in the root `Cargo.toml` and in `apps/drafftink-headless/Cargo.toml`.

### Why the old names stay

The three library crates keep the names `drafftink-cli`, `drafftink-mcp` and `drafftink-server`, and are not merged into one `drafftink-tools`:

- `drafftink-server` is already on crates.io (0.1.0, owned by the project). Publishing 0.2.0 as a library keeps one history for the name and needs no pointer placeholder, because the server code stays in this crate. A placeholder like `deprecated/drafftink-core` is for a name whose code went elsewhere, and this one did not move. 0.1.0 stays installable with `cargo install drafftink-server --version 0.1.0`; yank it only if nobody should get the old server.
- The crates have different dependencies (axum and tokio for the server, rmcp for MCP, almost nothing for the tools). One crate would give the desktop app the server's dependencies, or need cargo features to take them away again, and it would be one bigger crate to compile for every user of one part. The split also keeps each clap definition next to the code it drives.
- The names say what the part is. They are not "the binary" any more, but "cli" for the tools that work on documents on the command line is still what the code is.

## Before a release

1. Bump `version` in `[workspace.package]` and every `version = "0.2.0"` next to a `path` in the crates' manifests (the
   `drafftink-rex` and `drafftink-unicode-math` versions are their own). `cargo xtask publish` fails on a path dependency
   without a version, not on a stale one: `cargo publish` fails on that, in the dry run.
2. Commit. A real publish needs a clean tree.
3. Run `cargo xtask ci`, then `cargo xtask publish`.

## `cargo xtask publish`

```
cargo xtask publish [--dry-run] [--allow-dirty] [--no-verify]
```

1. Lists the published crates in publish order (a crate after the crates it needs; ties by name).
2. Checks each manifest: description, license, repository, homepage, readme file, one to five keywords, a category, a `version`
   on every path dependency, no Git dependency, no dependency on an unpublished crate. Checks the file list of every package for
   the hand-drawn fonts.
3. Runs `cargo publish --workspace --dry-run --locked`. Cargo packages every crate and builds it from its package, with the
   unpublished siblings standing in for the registry (cargo 1.90 and later). This is the check that the package is complete: a
   file outside the crate, a missing `exclude`d file or a missing version fails here.
4. Runs the same for `deprecated/drafftink-core`.
5. Prints the size of each `.crate` (crates.io accepts 10 MiB) and the commands below.

`--dry-run` is accepted and changes nothing; `--execute`, `--publish`, `--no-dry-run`, `--token` and every other flag are
refused. `--allow-dirty` is for working on a branch; `--no-verify` skips the builds. The command is not part of `cargo xtask ci`
(it builds the workspace again from the packages).

## Publishing, by hand

From a clean checkout of the release commit, logged in with `cargo login`:

```sh
cargo xtask publish                 # the dry run must pass first
cargo publish --workspace           # all crates, in dependency order
cargo publish --manifest-path deprecated/drafftink-core/Cargo.toml
cargo yank --version 0.1.0 drafftink-core
```

Or one crate at a time, in the order that `cargo xtask publish` prints (`cargo publish -p <name>`).

crates.io limits how fast new crates can be published (a short burst, then about one every ten minutes); about twenty of the
names are new. When a run stops on that limit, wait and publish the rest from the printed order, then verify that
`cargo install drafftink` works in a clean environment. The crates.io index needs a minute to show a new version, so a
dependent crate can fail right after its dependency; run it again.

A published version cannot be changed or deleted, only yanked. Check `cargo package --list -p drafftink-typeset` for
fonts (`GelPen`, `Vanilla`) once more before the real run.
