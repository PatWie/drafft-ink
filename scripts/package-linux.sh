#!/usr/bin/env bash
# Build and package Drafft.ink for Linux (x86_64 or aarch64, the host's architecture unless CROSS_ARCH is set):
#
#   $DIST/drafftink-<version>-linux-<arch>.tar.gz          desktop app (`drafftink mcp` is its MCP bridge), FHS tree (bin/, share/)
#   $DIST/drafftink-<version>-linux-<arch>.AppImage        the same, one file (any distro with glibc >= the build host's)
#   $DIST/drafftink-<version>-linux-<arch>.deb             Debian, Ubuntu, Mint (dpkg-deb; no nfpm needed)
#   $DIST/drafftink-headless-<version>-linux-<arch>.tar.gz drafftink-headless + web-dist/ (the web build), run `./drafftink-headless serve` from that folder
#
# Usage: scripts/package-linux.sh [--skip-build] [--formats "tar appimage deb headless"] [--web-dist DIR]
#   --skip-build   package the binaries already in target/dist (or $BIN_DIR)
#   --formats      default: "tar appimage deb headless". "headless" needs the web build (--web-dist, default dist/,
#                  made by scripts/build-web.sh); without it the headless archive is skipped with a warning.
# Environment: DIST (output dir), BIN_DIR (binaries), CROSS_TARGET + CROSS_ARCH (cross build), APPIMAGETOOL.
# Needs: cargo, tar, dpkg-deb (for deb). The AppImage needs appimagetool (downloaded into target/ if missing).
# Build on an old distro (CI: Ubuntu 22.04, glibc 2.35) so the binaries run on newer ones.
# The script ends by running every packaged binary with --version.
set -euo pipefail
# shellcheck source=../packaging/env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../packaging/env.sh"
HERE="$ROOT/packaging/linux"

SKIP_BUILD=0
FORMATS="tar appimage deb headless"
WEB_DIST="$ROOT/dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="${2:?--formats needs a list}"; shift 2 ;;
    --web-dist) WEB_DIST="${2:?--web-dist needs a folder}"; shift 2 ;;
    -h | --help) sed -n '2,17p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

ARCH="${CROSS_ARCH:-$(uname -m)}"
case "$ARCH" in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
has() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
BASENAME="drafftink-$VERSION-linux-$ARCH"
HEADLESS_BASENAME="drafftink-headless-$VERSION-linux-$ARCH"
echo "==> Drafft.ink $VERSION for Linux $ARCH ($FORMATS)"

if [ "$SKIP_BUILD" = 0 ]; then
  # The headless program ships with every room store: sqlite and s3 are cargo features that are off by default.
  cargo_args=(build --profile dist --locked -p drafftink -p drafftink-headless --features "drafftink-headless/sqlite,drafftink-headless/s3,drafftink-headless/hand-fonts,drafftink/hand-fonts")
  if [ -n "${CROSS_TARGET:-}" ]; then cargo_args+=(--target "$CROSS_TARGET"); fi
  (cd "$ROOT" && cargo "${cargo_args[@]}")
fi
BIN="$(bin_dir)"
for b in $DESKTOP_BINS $HEADLESS_BIN; do
  [ -x "$BIN/$b" ] || { echo "error: $BIN/$b not found (build first or drop --skip-build)" >&2; exit 1; }
done

WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree (shared by tar, deb and AppImage) ----------------------------------------
for b in $DESKTOP_BINS; do install -Dm755 "$BIN/$b" "$STAGE/usr/bin/$b"; done
strip "$STAGE"/usr/bin/* 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$HERE/application-x-drafftink.xml" "$STAGE/usr/share/mime/packages/$APP_ID.xml"
mkdir -p "$STAGE/usr/share/icons"
make_icons "$WORK/icons"
cp -R "$WORK/icons/hicolor" "$STAGE/usr/share/icons/"
copy_docs "$STAGE/usr/share/doc/drafftink"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi

# ---- .tar.gz -------------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  tar -C "$WORK/tar" --owner=0 --group=0 -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# ---- .deb ----------------------------------------------------------------------------------------
if has deb; then
  if command -v dpkg-deb >/dev/null; then
    DEB="$WORK/deb"
    mkdir -p "$DEB/DEBIAN"
    cp -R "$STAGE/usr" "$DEB/usr"
    installed_kb="$(du -sk "$DEB/usr" | cut -f1)"
    cat >"$DEB/DEBIAN/control" <<CONTROL
Package: drafftink
Version: ${VERSION//-/\~}
Section: graphics
Priority: optional
Architecture: $DEB_ARCH
Installed-Size: $installed_kb
Maintainer: Drafft.ink contributors <https://github.com/PatWie/drafft-ink>
Depends: libc6, libvulkan1, libxkbcommon0
Recommends: mesa-vulkan-drivers, xdg-desktop-portal
Homepage: https://drafft.ink
Description: Infinite canvas whiteboard
 Drafft.ink draws on a GPU canvas (Vello on Vulkan), syncs rooms in real time
 and has an MCP bridge (drafftink mcp) so an AI agent can draw in the window.
CONTROL
    dpkg-deb --root-owner-group --build "$DEB" "$DIST/$BASENAME.deb" >/dev/null
    dpkg-deb --info "$DIST/$BASENAME.deb" | sed -n '1,12p'
    echo "wrote $DIST/$BASENAME.deb"
  else
    warn "dpkg-deb not found, so the .deb was not written"
  fi
fi

# ---- AppImage ------------------------------------------------------------------------------------
if has appimage; then
  APPDIR="$WORK/Drafftink.AppDir"
  cp -R "$STAGE" "$APPDIR"
  rm -rf "$APPDIR/usr/share/doc"
  install -Dm755 "$HERE/AppRun" "$APPDIR/AppRun"
  cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
  cp "$WORK/icons/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
  ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [ -z "$TOOL" ]; then
    TOOL="$CARGO_TARGET_DIR/appimagetool-$ARCH.AppImage"
    if [ ! -x "$TOOL" ]; then
      curl -fsSL -o "$TOOL" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
      chmod +x "$TOOL"
    fi
  fi
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  OUT="$DIST/$BASENAME.AppImage"
  # Extract-and-run: works without FUSE (containers, CI). The update-information field is left out until
  # releases are signed and the .zsync is published (docs/packaging.md).
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream "$APPDIR" "$OUT")
  echo "wrote $OUT"
fi

# ---- headless + web build ------------------------------------------------------------------------
if has headless; then
  if [ -f "$WEB_DIST/index.html" ]; then
    HL="$WORK/headless/$HEADLESS_BASENAME"
    mkdir -p "$HL"
    install -m755 "$BIN/$HEADLESS_BIN" "$HL/$HEADLESS_BIN"
    strip "$HL/$HEADLESS_BIN" 2>/dev/null || true
    # The default --web-dir of `serve` is ./web-dist, so running it from this folder serves the web app.
    cp -R "$WEB_DIST" "$HL/web-dist"
    cat >"$HL/README.txt" <<README
Drafft.ink without a screen $VERSION

  ./$HEADLESS_BIN serve

serves the web app (web-dist/) and the collaboration socket on http://127.0.0.1:3030 .
Run it from this folder, or point it elsewhere with --web-dir / DRAFFTINK_WEB_DIR.
./$HEADLESS_BIN serve --help lists the options (bind address, storage, auth tokens, limits).

The same program has an MCP server for AI agents (mcp) and tools for documents
(convert, run, outline, inspect, commands, schemas): ./$HEADLESS_BIN --help
README
    copy_docs "$HL"
    tar -C "$WORK/headless" --owner=0 --group=0 -czf "$DIST/$HEADLESS_BASENAME.tar.gz" "$HEADLESS_BASENAME"
    echo "wrote $DIST/$HEADLESS_BASENAME.tar.gz"
  else
    warn "no web build in $WEB_DIST (run scripts/build-web.sh), so the headless archive was not written"
  fi
fi

# ---- smoke ---------------------------------------------------------------------------------------
# Only when the packaged binaries can run on this machine.
if [ -z "${CROSS_TARGET:-}" ]; then
  for b in $DESKTOP_BINS; do smoke_version "$STAGE/usr/bin/$b"; done
  smoke_version "$BIN/$HEADLESS_BIN"
  if has tar; then
    tar -tzf "$DIST/$BASENAME.tar.gz" | grep "/bin/drafftink$" >/dev/null || { echo "error: tar.gz lacks bin/drafftink" >&2; exit 1; }
  fi
  if has appimage; then
    APPIMAGE_EXTRACT_AND_RUN=1 "$DIST/$BASENAME.AppImage" --version
  fi
fi
echo "==> done"
ls -lh "$DIST"
