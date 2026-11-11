#!/usr/bin/env bash
# Build the server and serve dist/ (run scripts/build-web.sh first) with /ws on the same origin.
# usage: scripts/serve-web.sh [port]        default 3030; extra arguments after the port go to the server
set -euo pipefail
cd "$(dirname "$0")/.."
PORT="${1:-3030}"
[ $# -gt 0 ] && shift
[ -f dist/index.html ] || { echo "dist/ is empty. Run scripts/build-web.sh first." >&2; exit 1; }
exec cargo run --release -p drafftink-server -- --web-dir dist --bind "127.0.0.1:$PORT" "$@"
