#!/usr/bin/env bash
# Builds the Rust core of the Worker into pkg/ (git-ignored): cargo (release-wasm profile) -> wasm-bindgen -> wasm-opt
# when it is installed. wrangler bundles pkg/drafftink_worker_bg.wasm as a wasm module (src/wasm.ts imports it).
#
# usage: scripts/build-wasm.sh [--profile NAME]      (default release-wasm; "release" builds faster and bigger)
# needs: rustup target add wasm32-unknown-unknown
#        wasm-bindgen CLI, the same version as the crate (cargo install wasm-bindgen-cli --version 0.2.130 --locked)
set -euo pipefail
cd "$(dirname "$0")/../../.."

PROFILE=release-wasm
while [ $# -gt 0 ]; do
  case "$1" in
    --profile) PROFILE="${2:?--profile needs a name}"; shift 2 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

WANT="$(sed -n 's/^wasm-bindgen = "=\([0-9.]*\)".*/\1/p' Cargo.toml | head -1)"
HAVE="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)"
if [ -z "$HAVE" ]; then
  echo "wasm-bindgen CLI not found. Install it: cargo install wasm-bindgen-cli --version ${WANT:-0.2.130} --locked" >&2
  exit 1
fi
if [ -n "$WANT" ] && [ "$HAVE" != "$WANT" ]; then
  echo "wasm-bindgen CLI is $HAVE but Cargo.toml pins $WANT. Run: cargo install wasm-bindgen-cli --version $WANT --locked --force" >&2
  exit 1
fi

TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
OUT=apps/drafftink-worker/pkg
cargo build -p drafftink-worker --target wasm32-unknown-unknown --profile "$PROFILE"
# The profile "release" lands in target/<triple>/release, every other profile in a folder of its own name.
WASM="$TARGET_DIR/wasm32-unknown-unknown/$PROFILE/drafftink_worker.wasm"
rm -rf "$OUT"
# --target web gives `initSync({ module })`, which is how a Worker instantiates a bundled wasm module.
wasm-bindgen "$WASM" --target web --out-dir "$OUT" --out-name drafftink_worker

if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int -o "$OUT/drafftink_worker_bg.wasm" "$OUT/drafftink_worker_bg.wasm"
else
  echo "wasm-opt not found: the module is not optimised further (it still works)." >&2
fi
ls -l "$OUT"/drafftink_worker_bg.wasm
