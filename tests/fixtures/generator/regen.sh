#!/usr/bin/env bash
# Rebuild the hand-authored Excalidraw inputs and regenerate (or --check) every derived fixture.
# Needs the old app checkout (never modified) and, for author_excalidraw.py, the Excalidraw checkout
# (EXCALIDRAW_REPO, default /home/ubuntu/git/excalidraw).
#
# The old checkout is DRAFFTINK_OLD_REPO (set by `cargo xtask fixtures`, which also finds it next to the main
# worktree). OLD_REPO is the older name of the same variable. Without either the default below applies.
#   ./regen.sh            write tests/fixtures/**
#   ./regen.sh --check    fail if anything on disk differs from what the old code produces now
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
old="${DRAFFTINK_OLD_REPO:-${OLD_REPO:-/home/ubuntu/git/drafft-ink}}"
if [ ! -d "$old/crates/drafftink-core" ]; then
  echo "regen.sh: the old app is not at $old (set DRAFFTINK_OLD_REPO)" >&2
  exit 2
fi
export OLD_REPO="$old"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/old-target-fixtures}"
if [ "${1:-}" != "--check" ]; then
  python3 "$here/author_excalidraw.py"
fi
# The manifest names the oracle by an absolute path. Build from a copy that points at $old, so that the
# checked-in manifest works on any machine and in any worktree.
work="$CARGO_TARGET_DIR/fixtures-gen-src"
mkdir -p "$work"
sed "s#/home/ubuntu/git/drafft-ink/#${old%/}/#" "$here/Cargo.toml" > "$work/Cargo.toml"
cp -f "$here/Cargo.lock" "$work/Cargo.lock"
ln -sfn "$here/src" "$work/src"
cargo build --release --manifest-path "$work/Cargo.toml"
"$CARGO_TARGET_DIR/release/fixtures-gen" --out "$here/.." "$@"
