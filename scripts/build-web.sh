#!/usr/bin/env bash
# Build the web app into dist/: release-wasm build -> wasm-bindgen -> wasm-opt -> static files -> sizes.
#
# usage: scripts/build-web.sh [--profile NAME] [--no-opt] [--no-precompress] [--no-fallback] [--features LIST]
#   --profile NAME     cargo profile (default release-wasm; "release" is quicker to build and bigger)
#   --no-opt           skip wasm-opt
#   --no-precompress   do not write .br / .gz next to the files
#   --no-fallback      do not build the WebGL2 module for browsers without WebGPU (dist/pkg-gl)
#   --features LIST    more cargo features of drafftink-web, e.g. `scenarios` for the perf driver (not for shipping).
#                      `hand-fonts` (GelPen, GelPen Serif, Vanilla Extract) is always on: the web app is built from the repo
# env: DIST_DIR=folder   write the build there instead of dist/
# needs: rustup target add wasm32-unknown-unknown
#        wasm-bindgen CLI, the same version as the wasm-bindgen crate (cargo install wasm-bindgen-cli --version 0.2.130 --locked)
#        wasm-opt (binaryen >= 117) on PATH or in ~/.cargo/tools/binaryen-version_*/bin
# serve it: cargo run --release -p drafftink-server -- --web-dir dist   (or scripts/serve-web.sh)
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE=release-wasm
OPT=1
PRECOMPRESS=1
FALLBACK=1
# The hand-drawn fonts (GelPen, GelPen Serif, Vanilla Extract) are in the web app, which is not a package on crates.io.
FEATURES="hand-fonts"
while [ $# -gt 0 ]; do
  case "$1" in
    --profile) PROFILE="${2:?--profile needs a name}"; shift 2 ;;
    --no-opt) OPT=0; shift ;;
    --no-precompress) PRECOMPRESS=0; shift ;;
    --no-fallback) FALLBACK=0; shift ;;
    --features) FEATURES="hand-fonts,${2:?--features needs a list}"; shift 2 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
DIST="${DIST_DIR:-dist}"   # DIST_DIR: another output folder, e.g. the scenario build of `cargo xtask perf --app web`
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

# --- command schemas: a table that the page fetches, not code in the module. The module is built without the
# `schemas` feature of drafftink-engine (apps/drafftink-web depends on it with default-features = false), and
# `drafftink-cli schemas` writes what that code would have produced. Made with the native toolchain before the
# wasm flags below, so the native build is not rebuilt for them.
SCHEMAS_JSON="$(mktemp)"
trap 'rm -f "$SCHEMAS_JSON"' EXIT
CARGO_TARGET_DIR="$TARGET_DIR" cargo run -q -p drafftink-cli -- schemas > "$SCHEMAS_JSON"

# --- build
# WebAssembly SIMD (every browser with WebGPU has it): the CPU glyph rasteriser that epaint pulls in (vello_cpu with
# fearless_simd) then uses SIMD instructions instead of a scalar fallback that is copied for every pipeline, which
# is 1.7 MiB less code (docs/performance.md, "Wasm size"). Set here and not in .cargo/config.toml so that the
# npm package (crates/canvas-js) keeps building for plain WebAssembly. The WebGL2 module needs it even more: its
# strip generation (vello_common) is SIMD code.
export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+simd128"
OPT_FEATURES="--enable-bulk-memory --enable-sign-ext --enable-mutable-globals --enable-nontrapping-float-to-int --enable-reference-types --enable-multivalue --enable-simd"
WASM_OPT=""
TIMEOUT="$(command -v timeout || command -v gtimeout || true)"   # macOS has neither without coreutils: then no time limit
if [ "$OPT" = 1 ] && [ "$PROFILE" = "release-wasm" ]; then
  WASM_OPT="$(command -v wasm-opt || true)"
  [ -z "$WASM_OPT" ] && WASM_OPT="$(ls ~/.cargo/tools/binaryen-version_*/bin/wasm-opt 2>/dev/null | sort -V | tail -1 || true)"
  [ -z "$WASM_OPT" ] && echo "wasm-opt not found: skipping (install binaryen >= 117 for a smaller file)" >&2
fi
WASM="$TARGET_DIR/wasm32-unknown-unknown/$PROFILE/$NAME.wasm"

# filesize <file>: bytes, with GNU stat (Linux) or BSD stat (macOS).
filesize() { stat -c %s "$1" 2>/dev/null || stat -f %z "$1"; }

# bindgen_module <package dir under dist/>: wasm-bindgen and wasm-opt for the module cargo just wrote. Sets RAW and OPTSIZE.
bindgen_module() {
  local out_dir="$DIST/$1" out
  mkdir -p "$out_dir"
  # No --keep-debug: the name section alone is 1.9 MB (docs/stack.md).
  wasm-bindgen --target web --no-typescript --out-dir "$out_dir" --out-name "$NAME" "$WASM"
  out="$out_dir/${NAME}_bg.wasm"
  RAW=$(filesize "$out")
  OPTSIZE=$RAW
  if [ -n "$WASM_OPT" ]; then
    # Not --all-features: with binaryen 133 it turns on experimental encodings and Chromium then rejects the
    # module ("unknown import kind 0x7e"). These are the features rustc's wasm32 target emits.
    # -Oz: smaller than -Os by about 2% brotli, and as fast in the frame measurement (docs/performance.md, "Wasm size").
    ${TIMEOUT:+$TIMEOUT 1200} "$WASM_OPT" $OPT_FEATURES --strip-debug --strip-producers -Oz "$out" -o "$out.opt"
    mv "$out.opt" "$out"
    OPTSIZE=$(filesize "$out")
  fi
}

rm -rf "$DIST"
mkdir -p "$DIST"

# The WebGL2 module first, because cargo writes both builds to the same file and `cargo xtask size` wants the
# normal module's unstripped copy (twiggy). It is the whole app with the WebGL2 renderer instead of Vello on wgpu,
# loaded by the page only when the browser has no WebGPU (docs/web-app.md, "Browsers without WebGPU").
FB_RAW=0; FB_OPT=0; FB_GZ=0; FB_BR=0
if [ "$FALLBACK" = 1 ]; then
  CARGO_TARGET_DIR="$TARGET_DIR" cargo build --profile "$PROFILE" --target wasm32-unknown-unknown -p drafftink-web --lib \
    --no-default-features --features "webgl${FEATURES:+,$FEATURES}"
  bindgen_module pkg-gl
  FB_RAW=$RAW; FB_OPT=$OPTSIZE
fi

CARGO_TARGET_DIR="$TARGET_DIR" cargo build --profile "$PROFILE" --target wasm32-unknown-unknown -p drafftink-web --lib ${FEATURES:+--features "$FEATURES"}
bindgen_module pkg
OUT="$DIST/pkg/${NAME}_bg.wasm"

# --- fonts: separate files, not part of the wasm. The name carries a hash of the content, so a cached copy is
# valid forever. `window.__assets` maps the plain file name to the address; the first frame's font is preloaded
# and precached by the service worker, the others are fetched when the app asks (crates/app/src/web/assets.rs).
mkdir -p "$DIST/fonts"
EGUI_FONTS="$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/epaint_default_fonts-*/fonts 2>/dev/null | sort -V | tail -1 || true)"
if [ -z "$EGUI_FONTS" ]; then
  echo "epaint_default_fonts not found in the cargo registry: run 'cargo fetch' first" >&2; exit 1
fi
ASSETS=""
add_font() { # add_font <source file>
  local src="$1" name stem ext hash
  name="$(basename "$src")"; stem="${name%.*}"; ext="${name##*.}"
  hash="$(sha256sum "$src" | cut -c1-10)"
  cp "$src" "$DIST/fonts/$stem.$hash.$ext"
  ASSETS="$ASSETS\"$name\":\"fonts/$stem.$hash.$ext\","
}
for f in NotoSans-Regular.ttf NotoSans-Bold.ttf NotoSans-Italic.ttf IBMPlexSans-Variable.ttf STIXTwoMath-Regular.otf \
  NotoSansSymbols2-Fallback.ttf NotoSansMath-Fallback.ttf NotoSansKannada-Fallback.ttf NotoSansArabic-Fallback.ttf NotoSansCJK-Fallback.otf; do add_font "crates/typeset/assets/fonts/$f"; done
for f in Ubuntu-Light.ttf Hack-Regular.ttf emoji-icon-font.ttf; do add_font "$EGUI_FONTS/$f"; done
# The schema table is a file of the same kind: the hash is in the name, `window.__assets` maps the plain name, and the
# page preloads it so that it is in flight before the module has started.
mkdir -p "$DIST/schemas"
SCHEMAS_FILE="schemas/commands.$(sha256sum "$SCHEMAS_JSON" | cut -c1-10).json"
cp "$SCHEMAS_JSON" "$DIST/$SCHEMAS_FILE"
ASSETS="$ASSETS\"commands.schema.json\":\"$SCHEMAS_FILE\","
ASSETS_JSON="{${ASSETS%,}}"
FIRST_FONT="$(sed -n 's/.*"Ubuntu-Light.ttf":"\([^"]*\)".*/\1/p' <<<"$ASSETS_JSON")"
# The welcome document has a formula, so the math font is wanted right after the first frame: fetch it with the module.
MATH_FONT="$(sed -n 's/.*"STIXTwoMath-Regular.otf":"\([^"]*\)".*/\1/p' <<<"$ASSETS_JSON")"
mkdir -p "$DIST/fonts/licenses"
cp crates/typeset/assets/licenses/* "$DIST/fonts/licenses/"
cp "$EGUI_FONTS"/*.txt "$DIST/fonts/licenses/"

# --- static files, with the build hash in the page and the service worker
BUILD="$(cat "$OUT" "$DIST/pkg/$NAME.js" | sha256sum | cut -c1-12)"
BUILD_GL=none
[ "$FALLBACK" = 1 ] && BUILD_GL="$(cat "$DIST/pkg-gl/${NAME}_bg.wasm" "$DIST/pkg-gl/$NAME.js" | sha256sum | cut -c1-12)"
cp web/manifest.webmanifest web/favicon.svg web/icon-192.png web/icon-512.png web/icon-maskable-512.png "$DIST/"
sed -e "s/__BUILD__/$BUILD/g" -e "s/__BUILD_GL__/$BUILD_GL/g" -e "s|__ASSETS__|$ASSETS_JSON|" \
    -e "s|__PRELOAD__|<link rel=\"preload\" as=\"fetch\" crossorigin href=\"$FIRST_FONT\"><link rel=\"preload\" as=\"fetch\" crossorigin href=\"$MATH_FONT\"><link rel=\"preload\" as=\"fetch\" crossorigin href=\"$SCHEMAS_FILE\">|" web/index.html > "$DIST/index.html"
sed -e "s/__BUILD__/$BUILD/g" -e "s/__BUILD_GL__/$BUILD_GL/g" -e "s|__SHELL_FONTS__|[\"$FIRST_FONT\"]|" web/sw.js > "$DIST/sw.js"
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
SCHEMAS_BR=$(brotli_size "$DIST/$SCHEMAS_FILE" 2>/dev/null || echo 0)
if [ "$FALLBACK" = 1 ]; then
  FB_GZ=$(gzip -9 -c "$DIST/pkg-gl/${NAME}_bg.wasm" | wc -c)
  FB_BR=$(brotli_size "$DIST/pkg-gl/${NAME}_bg.wasm" 2>/dev/null || echo 0)
  printf 'WebGL2 module (loaded only without WebGPU): raw %s | wasm-opt %s | gzip -9 %s | brotli -11 %s\n' \
    "$(mb "$FB_RAW")" "$(mb "$FB_OPT")" "$(mb "$FB_GZ")" "$(mb "$FB_BR")"
fi
FONT_BR=0
for f in "$DIST"/fonts/*.ttf "$DIST"/fonts/*.otf; do FONT_BR=$((FONT_BR + $(brotli_size "$f"))); done
printf 'fonts (loaded on demand, brotli -11): %s in %s files\n' "$(mb "$FONT_BR")" "$(ls "$DIST"/fonts/*.ttf "$DIST"/fonts/*.otf | wc -l)"
# The numbers `cargo xtask size` checks (xtask/src/size.rs), in bytes.
printf 'command schemas (fetched with the page, brotli -q 11): %s\n' "$(mb "$SCHEMAS_BR")"
printf '{"raw": %s, "opt": %s, "gzip": %s, "brotli": %s, "fonts_brotli": %s, "schemas_brotli": %s, "fallback_raw": %s, "fallback_opt": %s, "fallback_gzip": %s, "fallback_brotli": %s}\n' \
  "$RAW" "$OPTSIZE" "$GZ" "$BR" "$FONT_BR" "$SCHEMAS_BR" "$FB_RAW" "$FB_OPT" "$FB_GZ" "$FB_BR" > "$DIST/sizes.json"

# --- precompressed copies: drafftink-server serves .br / .gz when the browser accepts them
if [ "$PRECOMPRESS" = 1 ]; then
  find "$DIST" -type f \( -name '*.wasm' -o -name '*.js' -o -name '*.json' -o -name '*.html' -o -name '*.webmanifest' -o -name '*.svg' -o -name '*.ttf' -o -name '*.otf' \) -size +1k | while read -r f; do
    gzip -9 -c "$f" > "$f.gz"
    if command -v brotli >/dev/null; then brotli -q 11 -c "$f" > "$f.br"
    else "${PERF_PY:-$HOME/.venvs/perf/bin/python}" -c 'import brotli,sys; open(sys.argv[2],"wb").write(brotli.compress(open(sys.argv[1],"rb").read(), quality=11))' "$f" "$f.br"; fi
  done
fi
echo "dist/ is ready: cargo run --release -p drafftink-server -- --web-dir dist"
