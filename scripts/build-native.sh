#!/usr/bin/env bash
# Builds the desktop app in release mode and prints where it is.
# usage: scripts/build-native.sh [extra cargo args]
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p drafftink "$@"
bin="target/release/drafftink"
[ -f "target/release/drafftink.exe" ] && bin="target/release/drafftink.exe"
ls -l "$bin"
echo "run it with: $bin [FILE] [--room ROOM] [--server URL] [--bridge [PORT]]"
