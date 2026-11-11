#!/usr/bin/env bash
# Build and package Drafft.ink for macOS (runs on macOS only: lipo, hdiutil, codesign are Apple tools).
#
#   $DIST/drafftink-<version>-macos-universal.dmg          Drafftink.app (+ drafftink-mcp and drafftink-cli inside it) and an Applications link
#   $DIST/drafftink-server-<version>-macos-universal.tar.gz  drafftink-server + web-dist/ (needs the web build)
#
# Usage: scripts/package-macos.sh [--skip-build] [--sign] [--notarize] [--web-dist DIR]
#   --skip-build   use the binaries already in target/<triple>/dist for both triples
#   --sign         codesign the .app and the .dmg with $MACOS_SIGN_IDENTITY ("Developer ID Application: ..."),
#                  hardened runtime, entitlements from packaging/macos/entitlements.plist
#   --notarize     submit the .dmg with `xcrun notarytool` and staple it. Needs --sign and
#                  $APPLE_ID, $APPLE_TEAM_ID, $APPLE_APP_PASSWORD (an app-specific password)
# Without --sign the app is unsigned: users open it with right-click > Open (docs/packaging.md, "Signing").
# The universal binary is lipo of aarch64-apple-darwin and x86_64-apple-darwin; both targets are built here
# (rustup target add on the runner). Ends by running the packaged binaries with --version.
set -euo pipefail
# shellcheck source=../packaging/env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../packaging/env.sh"
HERE="$ROOT/packaging/macos"

SKIP_BUILD=0 SIGN=0 NOTARIZE=0
WEB_DIST="$ROOT/dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --sign) SIGN=1; shift ;;
    --notarize) NOTARIZE=1; shift ;;
    --web-dist) WEB_DIST="${2:?--web-dist needs a folder}"; shift 2 ;;
    -h | --help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done
[ "$(uname -s)" = Darwin ] || { echo "error: package-macos.sh runs on macOS only" >&2; exit 1; }
if [ "$NOTARIZE" = 1 ] && [ "$SIGN" = 0 ]; then echo "error: --notarize needs --sign" >&2; exit 2; fi
if [ "$SIGN" = 1 ] && [ -z "${MACOS_SIGN_IDENTITY:-}" ]; then echo "error: --sign needs MACOS_SIGN_IDENTITY" >&2; exit 2; fi

TRIPLES="aarch64-apple-darwin x86_64-apple-darwin"
echo "==> Drafft.ink $VERSION for macOS (universal)"
if [ "$SKIP_BUILD" = 0 ]; then
  for t in $TRIPLES; do
    rustup target add "$t"
    (cd "$ROOT" && cargo build --profile dist --locked -p drafftink -p drafftink-mcp -p drafftink-cli -p drafftink-server --features drafftink-server/sqlite,drafftink-server/s3 --target "$t")
  done
fi

WORK="$CARGO_TARGET_DIR/macos-package"
rm -rf "$WORK"
mkdir -p "$WORK/universal"
for b in $DESKTOP_BINS $SERVER_BIN; do
  slices=(); for t in $TRIPLES; do slices+=("$CARGO_TARGET_DIR/$t/dist/$b"); done
  lipo -create -output "$WORK/universal/$b" "${slices[@]}"
  strip "$WORK/universal/$b" 2>/dev/null || true
done

# ---- Drafftink.app -------------------------------------------------------------------------------
APP="$WORK/dmg/Drafftink.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
for b in $DESKTOP_BINS; do install -m755 "$WORK/universal/$b" "$APP/Contents/MacOS/$b"; done
sed -e "s/@APP_ID@/$APP_ID/g" -e "s/@VERSION@/${VERSION%%-*}/g" "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
cp "$ROOT/packaging/icons/drafftink.icns" "$APP/Contents/Resources/drafftink.icns"
plutil -lint "$APP/Contents/Info.plist"

sign() { codesign --force --timestamp --options runtime --entitlements "$HERE/entitlements.plist" --sign "$MACOS_SIGN_IDENTITY" "$@"; }
if [ "$SIGN" = 1 ]; then
  # Inside out: the helper first, then the bundle.
  sign "$APP/Contents/MacOS/drafftink-mcp"
  sign "$APP/Contents/MacOS/drafftink-cli"
  sign "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
else
  warn "the app is unsigned (no --sign): Gatekeeper makes users open it with right-click > Open"
fi

# ---- .dmg ----------------------------------------------------------------------------------------
DMG="$DIST/drafftink-$VERSION-macos-universal.dmg"
ln -s /Applications "$WORK/dmg/Applications"
copy_docs "$WORK/dmg"
rm -f "$DMG"
hdiutil create -volname "Drafft.ink $VERSION" -srcfolder "$WORK/dmg" -fs HFS+ -format UDZO -ov "$DMG"
if [ "$SIGN" = 1 ]; then codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$DMG"; fi
if [ "$NOTARIZE" = 1 ]; then
  : "${APPLE_ID:?}" "${APPLE_TEAM_ID:?}" "${APPLE_APP_PASSWORD:?}"
  xcrun notarytool submit "$DMG" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
  xcrun stapler staple "$DMG"
fi
echo "wrote $DMG"

# ---- server + web build --------------------------------------------------------------------------
if [ -f "$WEB_DIST/index.html" ]; then
  NAME="drafftink-server-$VERSION-macos-universal"
  SRV="$WORK/server/$NAME"
  mkdir -p "$SRV"
  install -m755 "$WORK/universal/$SERVER_BIN" "$SRV/$SERVER_BIN"
  if [ "$SIGN" = 1 ]; then sign "$SRV/$SERVER_BIN"; fi
  cp -R "$WEB_DIST" "$SRV/web-dist"
  copy_docs "$SRV"
  printf 'Run ./%s from this folder: it serves web-dist/ and the collaboration socket on http://127.0.0.1:3030\n' "$SERVER_BIN" >"$SRV/README.txt"
  tar -C "$WORK/server" -czf "$DIST/$NAME.tar.gz" "$NAME"
  echo "wrote $DIST/$NAME.tar.gz"
else
  warn "no web build in $WEB_DIST (run scripts/build-web.sh), so the server archive was not written"
fi

# ---- smoke ---------------------------------------------------------------------------------------
# The runner's architecture runs its own slice of each universal binary.
smoke_version "$APP/Contents/MacOS/drafftink"
smoke_version "$APP/Contents/MacOS/drafftink-mcp"
smoke_version "$APP/Contents/MacOS/drafftink-cli"
smoke_version "$WORK/universal/$SERVER_BIN"
lipo -archs "$APP/Contents/MacOS/drafftink"
echo "==> done"
ls -lh "$DIST"
