#!/usr/bin/env bash
# Regenerate tests/fixtures/render/*.png from tests/fixtures/legacy/*.json (old renderer, NVIDIA via Vulkan).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/tmp/old-target-render}
export WGPU_ADAPTER_NAME=${WGPU_ADAPTER_NAME:-NVIDIA}
cargo build --release --locked --manifest-path "$here/Cargo.toml"
legacy="$here/../legacy"; out="$here/../render"
for n in all_shapes styles_rectangle styles_freehand text_fonts text_char_colors math images groups_nested welcome; do
  [ -f "$legacy/$n.json" ] || { echo "skip $n (missing)"; continue; }
  "$CARGO_TARGET_DIR/release/render-gen" "$legacy/$n.json" "$out/$n.png" --scale "${SCALE:-1}"
done
# Mermaid expected docs (optional extras)
for n in flowchart_subgraph sequence_basic; do
  f="$here/../mermaid/expected/$n.json"
  [ -f "$f" ] && "$CARGO_TARGET_DIR/release/render-gen" "$f" "$out/mermaid_$n.png" --scale "${SCALE:-1}"
done
