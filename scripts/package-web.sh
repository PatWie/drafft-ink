#!/usr/bin/env bash
# Package the web build (dist/, made by scripts/build-web.sh) as a tarball and a zip:
#
#   $DIST/drafftink-web-<version>.tar.gz
#   $DIST/drafftink-web-<version>.zip      (if `zip` is installed)
#
# Usage: scripts/package-web.sh [--build] [--web-dist DIR]
#   --build      run scripts/build-web.sh first
#   --web-dist   the folder to package (default dist/)
# The folder works under any URL prefix (the page uses relative URLs); the check below fails if it does not.
# Serve it with any static host, or with drafftink-headless serve --web-dir.
set -euo pipefail
# shellcheck source=../packaging/env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../packaging/env.sh"

BUILD=0
WEB_DIST="$ROOT/dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --build) BUILD=1; shift ;;
    --web-dist) WEB_DIST="${2:?--web-dist needs a folder}"; shift 2 ;;
    -h | --help) sed -n '2,11p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done
if [ "$BUILD" = 1 ]; then (cd "$ROOT" && scripts/build-web.sh); fi
[ -f "$WEB_DIST/index.html" ] || { echo "error: $WEB_DIST/index.html not found (run scripts/build-web.sh)" >&2; exit 1; }
if grep -Eq '(src|href)="/[^/]' "$WEB_DIST/index.html"; then
  echo "error: $WEB_DIST/index.html has root-absolute URLs, so it would not work under a URL prefix" >&2
  exit 1
fi
ls "$WEB_DIST"/pkg/*_bg.wasm >/dev/null

NAME="drafftink-web-$VERSION"
WORK="$CARGO_TARGET_DIR/web-package"
rm -rf "$WORK"
mkdir -p "$WORK"
cp -R "$WEB_DIST" "$WORK/$NAME"
tar -C "$WORK" --owner=0 --group=0 -czf "$DIST/$NAME.tar.gz" "$NAME"
echo "wrote $DIST/$NAME.tar.gz"
if command -v zip >/dev/null; then
  rm -f "$DIST/$NAME.zip"
  (cd "$WORK" && zip -qr9 "$DIST/$NAME.zip" "$NAME")
  echo "wrote $DIST/$NAME.zip"
fi
ls -lh "$DIST/$NAME".*
