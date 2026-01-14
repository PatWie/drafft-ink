#!/bin/bash
# Build WASM artifacts for the last N commits for debugging/comparison
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="/tmp/drafft-hist"
NUM_COMMITS=${1:-10}

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

# Get current branch to restore later
ORIG_BRANCH=$(git rev-parse --abbrev-ref HEAD)
ORIG_COMMIT=$(git rev-parse HEAD)

# Get last N commits
COMMITS=$(git log --oneline -n "$NUM_COMMITS" --format="%H %s")

cleanup() {
    cd "$SCRIPT_DIR"
    git checkout -q "$ORIG_COMMIT" 2>/dev/null || git checkout -q "$ORIG_BRANCH"
}
trap cleanup EXIT

echo "Building $NUM_COMMITS commits..."

INDEX="<html><head><title>Drafft.ink History</title><style>
body{font-family:sans-serif;max-width:800px;margin:2em auto;padding:0 1em}
a{display:block;padding:0.5em;margin:0.2em 0;background:#f0f0f0;text-decoration:none;color:#333}
a:hover{background:#e0e0e0}
code{background:#ddd;padding:0.2em 0.4em;font-size:0.9em}
</style></head><body><h1>Drafft.ink Build History</h1><ul>"

while IFS= read -r line; do
    HASH=$(echo "$line" | cut -d' ' -f1)
    MSG=$(echo "$line" | cut -d' ' -f2-)
    SHORT=${HASH:0:7}
    
    echo "Building $SHORT: $MSG"
    
    cd "$SCRIPT_DIR"
    git checkout -q "$HASH"
    
    if ./build.sh --wasm --release 2>/dev/null; then
        mkdir -p "$OUT_DIR/$SHORT"
        cp web/index.html web/favicon.svg "$OUT_DIR/$SHORT/" 2>/dev/null || cp web/index.html "$OUT_DIR/$SHORT/"
        cp -r web/pkg "$OUT_DIR/$SHORT/"
        INDEX="$INDEX<li><a href=\"$SHORT/\"><code>$SHORT</code> $MSG</a></li>"
        echo "  ✓ Success"
    else
        INDEX="$INDEX<li><code>$SHORT</code> $MSG (build failed)</li>"
        echo "  ✗ Failed"
    fi
done <<< "$COMMITS"

INDEX="$INDEX</ul></body></html>"
echo "$INDEX" > "$OUT_DIR/index.html"

echo ""
echo "Done! Open: file://$OUT_DIR/index.html"
echo "Or serve: cd $OUT_DIR && python3 -m http.server 8080"
