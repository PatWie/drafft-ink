#!/usr/bin/env bash
# Runs the Worker locally with `wrangler dev` (the local runtime: nothing is deployed, no Cloudflare login).
#
# usage: scripts/dev.sh [--env demo|private] [--port N] [--persist-to DIR] [--no-build] [-- extra wrangler args]
#
#   --env         which environment the top level of the config mirrors (default demo)
#   --port        port to listen on (default 8787)
#   --persist-to  where the Durable Object storage lives (default: a new temporary folder, printed)
#   --no-build    do not rebuild pkg/ (it is built when it is missing)
#
# A variable is changed for one run with `--var KEY:VALUE`, which wrangler takes as it is, for example
#   scripts/dev.sh -- --var ROOM_TTL_SECS:3 --var ROOM_LEAVE_GRACE_SECS:1
# The config is rendered by scripts/render-config.sh, so the same environment variables as there apply.
#
# WRANGLER names the command that runs wrangler (default `npx wrangler`, the version pinned in package.json); `make`
# passes the one from .env, so `make cf-dev` and the check and deploy targets use the same wrangler.
#
# `--env private` has Cloudflare Access required, and local dev has no Access in front, so the Worker would refuse every
# request. For local dev ONLY, pass `-- --var ACCESS_REQUIRED:false` (`make cf-dev` does, for CF_ENV=private). Never put
# it into a deploy: a deployed Worker without the check is open to everyone who finds its address.
set -euo pipefail
APP="$(cd "$(dirname "$0")/.." && pwd)"

ENVIRONMENT=demo
PORT=8787
PERSIST=""
BUILD=auto
while [ $# -gt 0 ]; do
  case "$1" in
    --env) ENVIRONMENT="${2:?--env needs demo or private}"; shift 2 ;;
    --port) PORT="${2:?--port needs a number}"; shift 2 ;;
    --persist-to) PERSIST="${2:?--persist-to needs a folder}"; shift 2 ;;
    --no-build) BUILD=no; shift ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
    --) shift; break ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

[ -n "$PERSIST" ] || PERSIST="$(mktemp -d -t drafftink-worker-dev.XXXXXX)"
if [ "$BUILD" = auto ] && [ ! -f "$APP/pkg/drafftink_worker_bg.wasm" ]; then BUILD=yes; fi
if [ "$BUILD" = yes ]; then "$APP/scripts/build-wasm.sh"; fi

CONFIG="$APP/.wrangler/dev/wrangler.toml"
"$APP/scripts/render-config.sh" "$ENVIRONMENT" "$CONFIG"
echo "state in $PERSIST, listening on http://127.0.0.1:$PORT" >&2
cd "$APP"
# $WRANGLER is a command with arguments (`npx wrangler`), so it is split into words on purpose.
# shellcheck disable=SC2086
exec ${WRANGLER:-npx wrangler} dev --config "$CONFIG" --ip 127.0.0.1 --port "$PORT" --persist-to "$PERSIST" "$@"
