#!/usr/bin/env bash
# Writes the wrangler config of the Worker from deploy/cloudflare/wrangler.toml.in.
#
# usage: render-config.sh <demo|private> <out-file>
#
#   The top level of the generated file mirrors the environment you name (it is what `wrangler dev` and the tests
#   use without `--env`), and the sections [env.demo] and [env.private] are always there for `wrangler deploy --env`.
#
# Every @NAME@ of the template comes from an environment variable with a safe local default, so tests and
# `wrangler dev` need no .env. The variables, with their defaults:
#
#   MAIN                 absolute path of the Worker entry          <app>/src/index.ts
#   ASSETS_DIR           absolute path of the web app files          <repo>/dist if it has index.html, else a stub
#   DEMO_<X>, PRIVATE_<X> for X in
#     WORKER_NAME DOMAIN ROOM_TTL_SECS ROOM_LEAVE_GRACE_SECS BLOB_STORE ACCESS_REQUIRED ACCESS_TEAM_DOMAIN ACCESS_AUD
#     ACCESS_APP_HOSTNAME ACCESS_CERTS_URL ALLOWED_ORIGINS MAX_ROOM_STATE_BYTES MAX_IMAGE_BYTES MAX_ROOM_IMAGE_BYTES MAX_ROOM_IMAGES ALLOW_DELETE_ROOM
#   PRIVATE_R2_BUCKET    drafftink-private-blobs
#   CF_WORKER_NAME, CF_DOMAIN   the names the Makefile uses today: they default PRIVATE_WORKER_NAME and PRIVATE_DOMAIN
#
# An empty ROOM_TTL_SECS means that rooms never expire (no deadline, no alarm).
set -euo pipefail

usage() { sed -n '2,19p' "$0" >&2; exit 2; }
[ $# -eq 2 ] || usage
CHOICE="$1"
OUT="$2"
case "$CHOICE" in demo|private) ;; *) usage ;; esac

APP="$(cd "$(dirname "$0")/.." && pwd)"
REPO="$(cd "$APP/../.." && pwd)"
TEMPLATE="$REPO/deploy/cloudflare/wrangler.toml.in"

# The web app: the real build when there is one, else a one-page stub so that wrangler has a directory to serve.
if [ -z "${ASSETS_DIR:-}" ]; then
  if [ -f "$REPO/dist/index.html" ]; then
    ASSETS_DIR="$REPO/dist"
  else
    ASSETS_DIR="$APP/.wrangler/stub-assets"
    mkdir -p "$ASSETS_DIR"
    [ -f "$ASSETS_DIR/index.html" ] || printf '<!doctype html><title>drafftink relay</title><p>No web app is built here.</p>\n' > "$ASSETS_DIR/index.html"
  fi
fi
MAIN="${MAIN:-$APP/src/index.ts}"
case "$MAIN" in /*) ;; *) MAIN="$PWD/$MAIN" ;; esac
case "$ASSETS_DIR" in /*) ;; *) ASSETS_DIR="$PWD/$ASSETS_DIR" ;; esac

# Per-environment defaults (section 19.8 of the design). "${VAR-default}" keeps an empty value that was set on purpose.
DEMO_WORKER_NAME="${DEMO_WORKER_NAME:-drafftink-relay-demo}"
DEMO_DOMAIN="${DEMO_DOMAIN:-drafft.ink}"
DEMO_ROOM_TTL_SECS="${DEMO_ROOM_TTL_SECS-172800}"
DEMO_ROOM_LEAVE_GRACE_SECS="${DEMO_ROOM_LEAVE_GRACE_SECS-60}"
DEMO_BLOB_STORE="${DEMO_BLOB_STORE:-do}"
DEMO_ACCESS_REQUIRED="${DEMO_ACCESS_REQUIRED:-false}"
DEMO_ACCESS_TEAM_DOMAIN="${DEMO_ACCESS_TEAM_DOMAIN-}"
DEMO_ACCESS_AUD="${DEMO_ACCESS_AUD-}"
DEMO_ACCESS_APP_HOSTNAME="${DEMO_ACCESS_APP_HOSTNAME-}"
DEMO_ACCESS_CERTS_URL="${DEMO_ACCESS_CERTS_URL-}"
DEMO_ALLOWED_ORIGINS="${DEMO_ALLOWED_ORIGINS-https://$DEMO_DOMAIN}"
DEMO_MAX_ROOM_STATE_BYTES="${DEMO_MAX_ROOM_STATE_BYTES:-33554432}"
DEMO_MAX_IMAGE_BYTES="${DEMO_MAX_IMAGE_BYTES:-4194304}"
DEMO_MAX_ROOM_IMAGE_BYTES="${DEMO_MAX_ROOM_IMAGE_BYTES:-52428800}"
DEMO_MAX_ROOM_IMAGES="${DEMO_MAX_ROOM_IMAGES:-200}"
DEMO_ALLOW_DELETE_ROOM="${DEMO_ALLOW_DELETE_ROOM:-false}"

PRIVATE_WORKER_NAME="${PRIVATE_WORKER_NAME:-${CF_WORKER_NAME:-drafftink-relay-private}}"
PRIVATE_DOMAIN="${PRIVATE_DOMAIN:-${CF_DOMAIN:-canvas.example.com}}"
PRIVATE_ROOM_TTL_SECS="${PRIVATE_ROOM_TTL_SECS-}"
PRIVATE_ROOM_LEAVE_GRACE_SECS="${PRIVATE_ROOM_LEAVE_GRACE_SECS-60}"
PRIVATE_BLOB_STORE="${PRIVATE_BLOB_STORE:-r2}"
PRIVATE_ACCESS_REQUIRED="${PRIVATE_ACCESS_REQUIRED:-true}"
PRIVATE_ACCESS_TEAM_DOMAIN="${PRIVATE_ACCESS_TEAM_DOMAIN-}"
PRIVATE_ACCESS_AUD="${PRIVATE_ACCESS_AUD-}"
# The host people open. Without it the guard uses the host of the request, which is wrong on a workers.dev address.
PRIVATE_ACCESS_APP_HOSTNAME="${PRIVATE_ACCESS_APP_HOSTNAME-$PRIVATE_DOMAIN}"
# Test and local override of the Access key set URL. Empty in a deployment.
PRIVATE_ACCESS_CERTS_URL="${PRIVATE_ACCESS_CERTS_URL-}"
PRIVATE_ALLOWED_ORIGINS="${PRIVATE_ALLOWED_ORIGINS-https://$PRIVATE_DOMAIN}"
PRIVATE_MAX_ROOM_STATE_BYTES="${PRIVATE_MAX_ROOM_STATE_BYTES:-268435456}"
PRIVATE_MAX_IMAGE_BYTES="${PRIVATE_MAX_IMAGE_BYTES:-4194304}"
# The design (19.8) wants 500 MiB and 2,000 pictures here, but `drafftink-protocol` caps a room at 50 MiB and 200
# pictures today, and the core refuses a larger value. Raise the ceilings in the protocol first, then these.
PRIVATE_MAX_ROOM_IMAGE_BYTES="${PRIVATE_MAX_ROOM_IMAGE_BYTES:-52428800}"
PRIVATE_MAX_ROOM_IMAGES="${PRIVATE_MAX_ROOM_IMAGES:-200}"
PRIVATE_ALLOW_DELETE_ROOM="${PRIVATE_ALLOW_DELETE_ROOM:-true}"
PRIVATE_R2_BUCKET="${PRIVATE_R2_BUCKET:-drafftink-private-blobs}"

SETTINGS="ROOM_TTL_SECS ROOM_LEAVE_GRACE_SECS BLOB_STORE ACCESS_REQUIRED ACCESS_TEAM_DOMAIN ACCESS_AUD ACCESS_APP_HOSTNAME ACCESS_CERTS_URL ALLOWED_ORIGINS MAX_ROOM_STATE_BYTES MAX_IMAGE_BYTES MAX_ROOM_IMAGE_BYTES MAX_ROOM_IMAGES ALLOW_DELETE_ROOM"

# The top level mirrors the chosen environment. The Worker is named after the environment too, so a deploy without
# `--env` (which has no route) does not overwrite the Worker of the other one.
CHOSEN="$(printf '%s' "$CHOICE" | tr '[:lower:]' '[:upper:]')"
SED_ARGS=()
add() { SED_ARGS+=(-e "s|@$1@|$(printf '%s' "$2" | sed -e 's/[&|\\]/\\&/g')|g"); }
for name in $SETTINGS; do
  pick="${CHOSEN}_${name}"
  add "TOP_$name" "${!pick}"
done
[ "$CHOICE" = demo ] && TOP_NAME="${DEMO_WORKER_NAME}-dev" || TOP_NAME="${PRIVATE_WORKER_NAME}-dev"
add TOP_WORKER_NAME "$TOP_NAME"
add TOP_ENVIRONMENT "$CHOICE"
for prefix in DEMO PRIVATE; do
  for name in $SETTINGS WORKER_NAME DOMAIN; do
    var="${prefix}_${name}"
    add "${prefix}_$name" "${!var}"
  done
done
add PRIVATE_R2_BUCKET "$PRIVATE_R2_BUCKET"
add MAIN "$MAIN"
add ASSETS_DIR "$ASSETS_DIR"

mkdir -p "$(dirname "$OUT")"
sed "${SED_ARGS[@]}" "$TEMPLATE" > "$OUT"
if grep -n '@[A-Z_]*@' "$OUT" >&2; then
  echo "render-config.sh: placeholders are left in $OUT (see above)" >&2
  exit 1
fi
echo "wrote $OUT (top level mirrors $CHOICE)"
