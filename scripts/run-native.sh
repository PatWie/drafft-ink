#!/usr/bin/env bash
# Runs the desktop app (release build). Arguments go to the app: see `drafftink --help`.
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --release -p drafftink --features hand-fonts -- "$@"
