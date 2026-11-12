# PNG export without a GPU

Question: can the headless tool (`drafftink-headless convert|run`, the program over `drafftink-cli`) write PNG on a machine with no GPU and no display? Yes. It was never GPU-bound: the app already turns a document into a `vello::Scene` on the CPU (`drafftink-render`), and only the last step, rasterising that scene, ran on the GPU. The tool now does that step on the CPU too (Cargo feature `png`) or, where it can, on the GPU (`png-gpu`).

## What was built

- `drafftink_render::replay` (moved from `crates/gpu/src/gl/replay.rs`, behaviour unchanged, tests moved with it): reads a finished `vello::Scene` back as immediate-mode calls on a `Canvas`. The WebGL2 fallback still uses it.
- `apps/drafftink-cli/src/raster/`: `compose` (document to scene, the app's `offscreen::compose` without the selection overlay), `canvas` (`Canvas` over `vello_cpu::RenderContext`), `gpu` (Vello on wgpu, feature `png-gpu`), `renderer` (`PngRenderer`, the `Renderer` port). `ports::session` shares the text engine between the session and the renderer.
- Refusals stay plain words: without the feature, `CliError::NoPng`; with it, a picture too large or an overlay request comes back as `render_failed`.

## Candidates

| | Needs | Pixels vs the app | Cost |
|---|---|---|---|
| (a) wgpu, any adapter (`png-gpu`) | A Vulkan/Metal/DX12 adapter: a GPU, or Mesa lavapipe on a server | Identical to the app (0.00% differ by more than 16) | Binary +7.4 MB (+35%); device start-up 0.35 s; Mesa on Linux servers; Vello returns a black picture when its buffers overflow (see below) |
| (b) `vello_cpu` (`png`) | Nothing | 0.02% to 1.2% of pixels differ by more than 16, all at edges | Binary +2.2 MB (+10%); no start-up cost |
| (c) resvg on our SVG | Nothing | Text is re-shaped by resvg, not ours; sizes differ | Not built; measured only |

(a) and (b) were prototyped. (c) was measured with a throwaway program (resvg 0.45, `text` and `raster-images` features, fonts read out of the SVG's own `@font-face` data URLs, because resvg ignores `@font-face`).

### Fidelity of (b) against the app (lavapipe; the L40S gives the same pixels, 0.00%)

Per channel difference over white, scale 2. "ink" is the pixels where either picture is not white.

| document | size | >16 | >16, of ink | >48, of ink |
|---|---|---|---|---|
| intro | 9080x1640 | 0.02% | 0.33% | 0.01% |
| all_shapes | 1360x764 | 0.26% | 1.7% | 0.11% |
| groups_nested | 1840x958 | 0.19% | 2.6% | 0.16% |
| images | 1512x909 | 0.00% | 0.00% | 0.00% |
| math | 1357x1576 | 0.03% | 1.3% | 0.13% |
| text_fonts | 2286x1750 | 0.07% | 0.8% | 0.07% |
| text_char_colors | 543x1060 | 0.08% | 1.1% | 0.05% |
| styles_freehand | 1582x611 | 0.09% | 0.9% | 0.03% |
| styles_rectangle | 2240x1560 | 1.18% | 2.8% | 0.14% |
| styles_arrow | 2400x980 | 0.20% | 5.3% | 0.36% |
| intro, dark | 9080x1640 | 0.02% | 0.02% | 0.00% |
| large_1000, scale 1 | 4738x4583 | 0.47% | 4.7% | 0.26% |

The repository's own pixel tests accept 16 per channel. Text, hatch fills, pictures, formulas and dark mode all agree, since they are the same scene.

### (c) resvg

- Our SVG has real `<text>` with embedded `@font-face`. resvg draws no text unless the fonts are loaded into its font database (done by hand here). Without that, all text is missing (the first run: boxes with no labels).
- It then lays the text out itself, so glyph positions differ from ours, and variable weights come out wrong (GelPen Heavy drawn as Regular). Of the inked pixels, 37% to 97% differ by more than 16 in text documents, 8% to 11% in shape documents after aligning by the best shift, against 0.3% to 5% for (b).
- The SVG is not the same rectangle as the PNG (it adds a margin and sizes strokes differently: 9084x1644 against 9080x1640), so every comparison needs an alignment shift, and the size is not what `doc.export` promises.
- Speed is fine (17 to 150 ms to rasterise). Not recommended: it adds a second drawing path to keep in step with the first.

### Speed and size (`dist` profile, this machine, one process per picture)

| | intro | all_shapes | large_1000 at 1x (4738x4583) | large_1000 at 2x (9476x9167) |
|---|---|---|---|---|
| (b) CPU | 0.49 s, 78 MB | 0.04 s, 22 MB | 1.65 s, 141 MB | 4.35 s, 413 MB |
| (a) L40S | 0.87 s, 310 MB | 0.41 s, 254 MB | 1.68 s, 372 MB | black picture |
| (a) lavapipe | 0.94 s, 471 MB | 0.39 s, 307 MB | | black picture |
| app path in one process, lavapipe | 5.9 s cold, 0.7 s warm | | | readback fails |

Binary: no PNG 21.37 MB; `png` 23.59 MB; `png-gpu` 28.79 MB. `vello_cpu`'s `multithreading` feature was tried and gave 4.35 s to 4.13 s, so it is off. `large_10000` at scale 0.5 takes 7.3 s on the CPU; most of that is the document, not the rasteriser.

## The black picture

Vello 0.11 returns `Ok` and a black picture when a scene needs more working buffer than the device has. At 49 million pixels (a thousand shapes at scale 2) this happened on an L40S and on lavapipe, with the adapter's own limits requested. The app's export has the same problem (there it fails at the readback instead, at 9476x9167). `png-gpu` therefore draws on the GPU only up to 16.7 million pixels and uses the CPU above, and also on any GPU error. A dense scene below that size could still overflow; the app has the same exposure.

## Recommendation

Ship `png` (CPU) in the default features of `drafftink-headless` (done; the desktop `drafftink` has no `convert` any more): pure Rust, no system dependency, the same scene as the app, 0.5 s for the largest document of the intro, a 2 MB larger binary. Keep `png-gpu` as an opt-in for people who render very large pictures on a machine with a GPU; it does not make small pictures faster. Do not take resvg.

Packaging: `drafftink-headless` enables `drafftink-cli/png` by default and `drafftink-cli/png-gpu` through its own opt-in `png-gpu`, so the release scripts need no flag for the CPU renderer. `cargo xtask layers` checks the dependency graph of `drafftink-headless` for screen crates with `png-gpu` left out.

## Reproducing

`drafftink-headless convert in.drafftink out.png` (the default build; `drafftink-cli` alone needs `--features png`); `DRAFFTINK_PNG_RENDERER=cpu` forces the CPU in a `png-gpu` build; `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` selects lavapipe. The comparison pictures were made by an example that exports through the app's `OffscreenRenderer` and a small program that counts differing pixels; neither is committed.
