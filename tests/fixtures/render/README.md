# Reference renders from the OLD renderer

PNG renders of legacy fixtures (`../legacy/*.json`, `../mermaid/expected/*.json`) produced by the old
app's renderer (`drafftink-render`, Vello, commit 8ce40ab of `/home/ubuntu/git/drafft-ink`), headless/offscreen.
They are visual references for the rewrite, not byte-exact golden files.

## Generator

`../render-gen/` is a standalone cargo project (own empty `[workspace]`). It depends on the old
`drafftink-core` and `drafftink-render` by absolute path and copies the old workspace's
`[patch.crates-io]` vello git pin (rev 1771112) because path dependencies do not inherit it. `Cargo.lock`
was seeded from the old repo's lock. No old file was modified and no copy of the old crates was needed.

```
export CARGO_TARGET_DIR=/tmp/old-target-render
cd tests/fixtures/render-gen
cargo build --release --locked
WGPU_ADAPTER_NAME=NVIDIA $CARGO_TARGET_DIR/release/render-gen <input.json> <output.png> [--scale N]
./gen.sh            # regenerates every PNG in this directory (SCALE=2 to override the scale)
```

Render path = the old app's "Export PNG" (`app.rs` `UiAction::ExportPng` + `render_scene_to_png`):
`VelloRenderer::build_export_scene(&doc, scale)` (fit to document bounds, 20 px padding, white
background, no grid or selection), `Rgba8Unorm` storage texture, `RenderParams { base_color: WHITE,
AaConfig::Area }`, `vello::Renderer::new(.., RendererOptions::default())`, readback via
`copy_texture_to_buffer`. Differences: no scene JSON is embedded in the PNG (the app embeds a zTXt chunk),
the PNG encoder uses best compression with adaptive filters, and the default scale is 1 (the app's UI default is 2).
Output size = ceil((content bounds + 2*20) * scale).

## Adapter

`NVIDIA L40S (DiscreteGpu, Vulkan, driver NVIDIA 580.178.04)`, selected through the `WGPU_ADAPTER_NAME=NVIDIA`
env var read by `vello::util::RenderContext` (without it, the default pick was also the NVIDIA GPU). The tool
prints the adapter to stderr on every run. Headless rendering works; no surface or window is needed.

## Files (all scale 1)

| PNG | source | size |
|---|---|---|
| all_shapes | legacy/all_shapes.json | 680x382 |
| styles_rectangle | legacy/styles_rectangle.json | 1120x780 |
| styles_freehand | legacy/styles_freehand.json | 791x306 |
| text_fonts | legacy/text_fonts.json | 1187x874 |
| text_char_colors | legacy/text_char_colors.json | 348x529 |
| math | legacy/math.json | 840x774 |
| images | legacy/images.json | 756x440 |
| groups_nested | legacy/groups_nested.json | 920x462 |
| welcome | legacy/welcome.json | 1896x1044 |
| mermaid_flowchart_subgraph | mermaid/expected/flowchart_subgraph.json | 507x704 |
| mermaid_sequence_basic | mermaid/expected/sequence_basic.json | 319x404 |

## Determinism

Rendering the same document twice with the same adapter gives byte-identical PNGs (checked for all 11 files,
separate processes). The old renderer is deterministic: rough/hatch randomness comes from the seed stored in
each shape, and parley loads only the bundled fonts (no system fonts, `default-features = false`).

## Caveats

- Cross-GPU: the same renders on llvmpipe (Mesa 25.2.8, `WGPU_ADAPTER_NAME=llvmpipe`) differ from these NVIDIA PNGs by at most 15 per
  channel (of 255), on 0.2% to 1.2% of pixels (13% for images.png, but with max difference 1); at most
  0.03% of pixels differ by more than 8. Do not compare pixel-exactly. Suggested tolerance for comparing
  another renderer: per-channel difference <= 16 (or perceptual, e.g. SSIM >= 0.98), allowing a few
  percent of edge pixels beyond that for anti-aliasing and stroke-join differences.
- Fixture quirks visible in the references (old behaviour, not bugs of the tool): overlapping text boxes in
  text_fonts; emoji, CJK and tab characters render as tofu boxes because no fallback font is bundled.
- `styles_rectangle.png` is about 440 KB (dense hatching), above the 300 KB target. The other files are
  below 300 KB (largest: welcome 277 KB, text_fonts 242 KB).
- Large fixtures (`large_*.json.gz`), `z_order`, other `styles_*` and the `crdt/` docs are not rendered.
- The tool takes native `CanvasDocument` JSON only (`CanvasDocument::from_json`). An empty document is an error.
