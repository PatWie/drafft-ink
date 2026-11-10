#!/usr/bin/env bash
# Rebuild the hand-authored Excalidraw inputs and regenerate (or --check) every derived fixture.
# Needs the old app checkout (OLD_REPO, default /home/ubuntu/git/drafft-ink at 8ce40ab, never modified)
# and, for author_excalidraw.py, the Excalidraw checkout (EXCALIDRAW_REPO, default /home/ubuntu/git/excalidraw).
#   ./regen.sh            write tests/fixtures/**
#   ./regen.sh --check    fail if anything on disk differs from what the old code produces now
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/old-target-fixtures}"
if [ "${1:-}" != "--check" ]; then
  python3 "$here/author_excalidraw.py"
fi
cargo build --release --manifest-path "$here/Cargo.toml"
"$CARGO_TARGET_DIR/release/fixtures-gen" "$@"
