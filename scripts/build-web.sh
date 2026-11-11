#!/usr/bin/env bash
# Build the web app into dist/: release-wasm build -> wasm-bindgen -> wasm-opt -> static files -> sizes.
#
# usage: scripts/build-web.sh [--profile NAME] [--no-opt] [--no-precompress]
#   --profile NAME     cargo profile (default release-wasm; "release" is quicker to build and bigger)
#   --no-opt           skip wasm-opt
#   --no-precompress   do not write .br / .gz next to the files
# needs: rustup target add wasm32-unknown-unknown
#        wasm-bindgen CLI, the same version as the wasm-bindgen crate (cargo install wasm-bindgen-cli --version 0.2.130 --locked)
#        wasm-opt (binaryen >= 117) on PATH or in ~/.cargo/tools/binaryen-version_*/bin
# serve it: cargo run --release -p drafftink-server -- --web-dir dist   (or scripts/serve-web.sh)
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE=release-wasm
OPT=1
PRECOMPRESS=1
while [ $# -gt 0 ]; do
  case "$1" in
    --profile) PROFILE="${2:?--profile needs a name}"; shift 2 ;;
    --no-opt) OPT=0; shift ;;
    --no-precompress) PRECOMPRESS=0; shift ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
DIST=dist
NAME=drafftink_web

# --- wasm-bindgen: the CLI must match the crate (a mismatch fails at run time with a confusing error)
WANT="$(sed -n 's/^wasm-bindgen = "=\([0-9.]*\)".*/\1/p' Cargo.toml | head -1)"
HAVE="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)"
if [ -z "$HAVE" ]; then
  echo "wasm-bindgen CLI not found. Install it: cargo install wasm-bindgen-cli --version ${WANT:-0.2.130} --locked" >&2; exit 1
fi
if [ -n "$WANT" ] && [ "$HAVE" != "$WANT" ]; then
  echo "wasm-bindgen CLI is $HAVE but Cargo.toml pins $WANT. Run: cargo install wasm-bindgen-cli --version $WANT --locked --force" >&2; exit 1
fi

# --- build
CARGO_TARGET_DIR="$TARGET_DIR" cargo build --profile "$PROFILE" --target wasm32-unknown-unknown -p drafftink-web --lib
WASM="$TARGET_DIR/wasm32-unknown-unknown/$PROFILE/$NAME.wasm"

rm -rf "$DIST"
mkdir -p "$DIST/pkg"
# No --keep-debug: the name section alone is 1.9 MB (docs/stack.md).
wasm-bindgen --target web --no-typescript --out-dir "$DIST/pkg" --out-name "$NAME" "$WASM"
OUT="$DIST/pkg/${NAME}_bg.wasm"
RAW=$(stat -c %s "$OUT")

# --- wasm-opt
OPTSIZE=$RAW
if [ "$OPT" = 1 ] && [ "$PROFILE" = "release-wasm" ]; then
  WASM_OPT="$(command -v wasm-opt || true)"
  [ -z "$WASM_OPT" ] && WASM_OPT="$(ls ~/.cargo/tools/binaryen-version_*/bin/wasm-opt 2>/dev/null | sort -V | tail -1 || true)"
  if [ -n "$WASM_OPT" ]; then
    "$WASM_OPT" --all-features --strip-debug --strip-producers -Os "$OUT" -o "$OUT.opt"
    mv "$OUT.opt" "$OUT"
    OPTSIZE=$(stat -c %s "$OUT")
  else
    echo "wasm-opt not found: skipping (install binaryen >= 117 for a smaller file)" >&2
  fi
fi

# --- static files, with the build hash in the page and the service worker
BUILD="$(cat "$OUT" "$DIST/pkg/$NAME.js" | sha256sum | cut -c1-12)"
cp web/manifest.webmanifest web/favicon.svg web/icon-192.png web/icon-512.png web/icon-maskable-512.png "$DIST/"
sed "s/__BUILD__/$BUILD/g" web/index.html > "$DIST/index.html"
sed "s/__BUILD__/$BUILD/g" web/sw.js > "$DIST/sw.js"
echo "$BUILD" > "$DIST/BUILD"

# --- sizes of the wasm
brotli_size() {
  if command -v brotli >/dev/null; then brotli -q 11 -c "$1" | wc -c
  else "${PERF_PY:-$HOME/.venvs/perf/bin/python}" -c 'import brotli,sys; print(len(brotli.compress(open(sys.argv[1],"rb").read(), quality=11)))' "$1"; fi
}
GZ=$(gzip -9 -c "$OUT" | wc -c)
BR=$(brotli_size "$OUT" 2>/dev/null || echo "?")
mb() { awk -v n="$1" 'BEGIN { if (n == "?") print "n/a"; else printf "%.2f MB", n/1048576 }'; }
echo "build $BUILD ($PROFILE)"
printf 'wasm: raw %s | wasm-opt %s | gzip -9 %s | brotli -11 %s\n' "$(mb "$RAW")" "$(mb "$OPTSIZE")" "$(mb "$GZ")" "$(mb "$BR")"

# --- precompressed copies: drafftink-server serves .br / .gz when the browser accepts them
if [ "$PRECOMPRESS" = 1 ]; then
  find "$DIST" -type f \( -name '*.wasm' -o -name '*.js' -o -name '*.html' -o -name '*.webmanifest' -o -name '*.svg' \) -size +1k | while read -r f; do
    gzip -9 -c "$f" > "$f.gz"
    if command -v brotli >/dev/null; then brotli -q 11 -c "$f" > "$f.br"
    else "${PERF_PY:-$HOME/.venvs/perf/bin/python}" -c 'import brotli,sys; open(sys.argv[2],"wb").write(brotli.compress(open(sys.argv[1],"rb").read(), quality=11))' "$f" "$f.br"; fi
  done
fi
echo "dist/ is ready: cargo run --release -p drafftink-server -- --web-dir dist"
