#!/usr/bin/env bash
# Build the web demo (examples/demo_web.rs) for wasm32 + WebGPU and package it into crates/gpu/web/pkg.
# usage: crates/gpu/scripts/build_web_demo.sh [profile]     profile: release-wasm (default, small and slow to build) or release (thin LTO, quicker)
# needs: rustup target add wasm32-unknown-unknown
#        wasm-bindgen CLI 0.2.130 (cargo install wasm-bindgen-cli --version 0.2.130 --locked), same version as the wasm-bindgen crate
#        optional: wasm-opt (binaryen >= 117; docs/stack.md) from ~/.cargo/tools/binaryen-version_*/bin or on PATH
set -euo pipefail
cd "$(dirname "$0")/../../.."
PROFILE="${1:-release-wasm}"
TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
export CARGO_TARGET_DIR="$TARGET_DIR"
cargo build --profile "$PROFILE" --target wasm32-unknown-unknown -p drafftink-gpu --example demo_web
WASM="$TARGET_DIR/wasm32-unknown-unknown/$PROFILE/examples/demo_web.wasm"
OUT="crates/gpu/web/pkg"
rm -rf "$OUT"
# --keep-debug is not used: the name section is 1.9 MB (docs/stack.md).
wasm-bindgen --target web --no-typescript --out-dir "$OUT" --out-name demo "$WASM"
WASM_OPT="$(command -v wasm-opt || true)"
[ -z "$WASM_OPT" ] && WASM_OPT="$(ls ~/.cargo/tools/binaryen-version_*/bin/wasm-opt 2>/dev/null | sort -V | tail -1 || true)"
if [ -n "$WASM_OPT" ] && [ "$PROFILE" = "release-wasm" ]; then
  "$WASM_OPT" --all-features --strip-debug --strip-producers -Os "$OUT/demo_bg.wasm" -o "$OUT/demo_bg.wasm"
fi
ls -l "$OUT"
