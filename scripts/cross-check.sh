#!/usr/bin/env bash
# Type-checks the native binaries for Windows and macOS from a Linux host, so a `cfg(windows)` or
# `cfg(target_os = "macos")` mistake shows up before CI on those systems does. Nothing is linked and nothing runs.
#
# usage: scripts/cross-check.sh [target ...]     (default: x86_64-pc-windows-gnu aarch64-apple-darwin x86_64-apple-darwin)
#
# C dependencies (ring, stacker) compile during `cargo check`, so each target needs a C compiler:
#   * windows-gnu: a mingw cross compiler (`x86_64-w64-mingw32-gcc` on PATH, e.g. apt `gcc-mingw-w64-x86-64`, or an
#     unpacked llvm-mingw release from https://github.com/mstorsjo/llvm-mingw, no root needed).
#   * apple-darwin: no SDK is available, so `clang` (any recent one, llvm-mingw has it) compiles the C files
#     freestanding, with the two headers ring needs from a stub directory this script writes. This is enough for
#     `cargo check`; it does not produce a usable object file.
# The MSVC target is not covered: its C files need the Windows SDK headers.
set -euo pipefail
cd "$(dirname "$0")/.."

targets=("$@")
if [ "${#targets[@]}" -eq 0 ]; then
  targets=(x86_64-pc-windows-gnu aarch64-apple-darwin x86_64-apple-darwin)
fi
packages=(-p drafftink -p drafftink-headless)

stub="$(mktemp -d)"
trap 'rm -rf "$stub"' EXIT
printf '#define TARGET_OS_MAC 1\n#define TARGET_OS_OSX 1\n#define TARGET_OS_IPHONE 0\n#define TARGET_OS_SIMULATOR 0\n' >"$stub/TargetConditionals.h"
printf '#define assert(x) ((void)0)\n' >"$stub/assert.h"
printf '#include <stddef.h>\nvoid *memcpy(void *, const void *, size_t);\nvoid *memset(void *, int, size_t);\nint memcmp(const void *, const void *, size_t);\nvoid *memmove(void *, const void *, size_t);\nsize_t strlen(const char *);\n' >"$stub/string.h"

# A separate target dir keeps the host build's artifacts and the cross builds apart.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}/cross"

for target in "${targets[@]}"; do
  rustup target add "$target" >/dev/null
  var="${target//-/_}"
  echo "==> cargo check ${packages[*]} --target $target"
  case "$target" in
    *-apple-darwin)
      env "CC_$var=clang --target=$target" "CFLAGS_$var=-ffreestanding -nostdlibinc -I$stub" \
        cargo check "${packages[@]}" --target "$target"
      ;;
    *)
      cargo check "${packages[@]}" --target "$target"
      ;;
  esac
done
echo "==> done"
