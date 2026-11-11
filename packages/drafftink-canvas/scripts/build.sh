#!/usr/bin/env bash
# Builds dist/: the wasm module, its JS glue and the generated TypeScript types.
#
#   cargo build --target wasm32-unknown-unknown --profile release-wasm -p drafftink-canvas-js
#   wasm-bindgen --target web            (CLI 0.2.130, the version of the wasm-bindgen crate in Cargo.toml)
#   wasm-opt -Os                         (binaryen 117 or newer; docs/stack.md)
#
# usage: scripts/build.sh [profile]      profile: release-wasm (default)
set -euo pipefail
cd "$(dirname "$0")/../../.."
PROFILE="${1:-release-wasm}"
TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
OUT="packages/drafftink-canvas/dist"

cargo build --profile "$PROFILE" --target wasm32-unknown-unknown -p drafftink-canvas-js
WASM="$TARGET_DIR/wasm32-unknown-unknown/$PROFILE/drafftink_canvas_js.wasm"

rm -rf "$OUT"
# No --keep-debug: the name section alone is about a quarter of the file (docs/stack.md).
wasm-bindgen --target web --typescript --out-dir "$OUT" "$WASM"

WASM_OPT="$(command -v wasm-opt || true)"
[ -z "$WASM_OPT" ] && WASM_OPT="$(ls ~/.cargo/tools/binaryen-version_*/bin/wasm-opt 2>/dev/null | sort -V | tail -1 || true)"
if [ -z "$WASM_OPT" ]; then
  echo "wasm-opt not found: the package is built without it (about a quarter larger)" >&2
else
  # Not --all-features: binaryen 133 then writes a compact import section, which Chromium 1208 rejects with
  # "unknown import kind 0x7f". These are the features rustc 1.97 emits for wasm32-unknown-unknown.
  "$WASM_OPT" --enable-bulk-memory --enable-bulk-memory-opt --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals --enable-multivalue --enable-reference-types --enable-simd \
    --strip-debug --strip-producers -Os "$OUT/drafftink_canvas_js_bg.wasm" -o "$OUT/drafftink_canvas_js_bg.wasm"
fi
ls -l "$OUT"
