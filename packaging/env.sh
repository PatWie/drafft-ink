# shellcheck shell=bash
# Shared setup for the scripts/package-*.sh scripts. Source it: `. "$(dirname "$0")/../packaging/env.sh"`.
# Modeled on solidcraft's packaging/env.sh.
#
# Exports:
#   ROOT               workspace root
#   VERSION            [workspace.package] version from Cargo.toml (override: DRAFFTINK_VERSION)
#   DIST               output directory for release artifacts (default $ROOT/dist-release)
#   CARGO_TARGET_DIR   cargo's target dir (default $ROOT/target)
#   APP_ID             reverse-DNS id: desktop file, icon name, Wayland app_id, macOS bundle id
#   BUILD_DATE         UTC build date, YYYY-MM-DD
# Functions: warn, copy_docs, sha256_of, make_icons

umask 022
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ROOT

# The version lives in one place: `[workspace.package] version` in the root Cargo.toml.
workspace_version() {
  awk '
    /^\[/ { in_pkg = ($0 == "[workspace.package]"); next }
    in_pkg && $1 == "version" { gsub(/[" ]/, "", $3); print $3; exit }
  ' "$ROOT/Cargo.toml"
}

VERSION="${DRAFFTINK_VERSION:-$(workspace_version)}"
if [ -z "$VERSION" ]; then
  echo "error: could not read [workspace.package] version from $ROOT/Cargo.toml" >&2
  exit 1
fi
export VERSION

DIST="${DIST:-$ROOT/dist-release}"
mkdir -p "$DIST"
DIST="$(cd "$DIST" && pwd)"
export DIST

export BUILD_DATE="${DRAFFTINK_BUILD_DATE:-$(date -u +%Y-%m-%d)}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# The window's Wayland app_id and X11 class must equal this (the .desktop file's StartupWMClass).
APP_ID=ink.drafft.drafftink
export APP_ID

# The binaries every desktop package carries, and the server package's.
DESKTOP_BINS="drafftink drafftink-mcp"
SERVER_BIN="drafftink-server"
export DESKTOP_BINS SERVER_BIN

# Emit a GitHub Actions warning (plain stderr outside Actions).
warn() {
  if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::warning::$*"; else echo "warning: $*" >&2; fi
}

# Copy the licence and readme files into a package directory.
copy_docs() {
  local dest="$1" f
  mkdir -p "$dest"
  for f in README.md LICENSE; do
    if [ -f "$ROOT/$f" ]; then cp "$ROOT/$f" "$dest/"; fi
  done
}

# Directory with the release binaries: $BIN_DIR if set, else target/<CROSS_TARGET>/release or target/release.
bin_dir() {
  if [ -n "${BIN_DIR:-}" ]; then echo "$BIN_DIR"
  elif [ -n "${CROSS_TARGET:-}" ]; then echo "$CARGO_TARGET_DIR/$CROSS_TARGET/release"
  else echo "$CARGO_TARGET_DIR/release"; fi
}

# Run `<bin> --version` and require the output to contain $VERSION. A stub main() fails here.
smoke_version() {
  local bin="$1" out
  out="$("$bin" --version 2>&1)" || { echo "error: '$bin --version' failed: $out" >&2; return 1; }
  case "$out" in
    *"$VERSION"*) echo "smoke: $out" ;;
    *) echo "error: '$bin --version' printed '$out', expected it to contain $VERSION" >&2; return 1 ;;
  esac
}

# Copy the png icon set into <out>/hicolor/<N>x<N>/apps/$APP_ID.png.
make_icons() {
  local out="$1" s
  for s in 16 24 32 48 64 128 256 512; do
    mkdir -p "$out/hicolor/${s}x${s}/apps"
    cp "$ROOT/packaging/icons/png/$s.png" "$out/hicolor/${s}x${s}/apps/$APP_ID.png"
  done
}
