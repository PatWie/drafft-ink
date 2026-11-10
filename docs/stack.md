# Graphics and UI stack

Status: proven by a spike on 2026-11-07 (native Linux/X11 and Chromium WebGPU). Source of the spike:
`/home/ubuntu/git/drafftink-spikes/stack` (eframe shell), `.../stack-winit` (own winit shell), `.../kittest`,
`.../cpufallback`, `.../pinset` (checks that the root `[workspace.dependencies]` resolves together). Copy from there.

## 1. The version set

The old app used egui 0.33, wgpu 27, a vello git rev and parley 0.7. The newest releases that agree on one wgpu major:

| crate | pin | note |
|---|---|---|
| wgpu | `=30.0.1` | eframe/egui-wgpu 0.37 want `^30`, vello 0.11 wants `^30.0.0`. |
| eframe, egui, egui-wgpu, egui_kittest | `=0.37.0` | egui-winit 0.37 and winit 0.30.13 come with eframe. |
| vello | `=0.11.0` | A release matches, so no git rev is needed. Uses peniko `^0.6.1`, skrifa 0.44. |
| peniko / kurbo | `=0.6.1` / `0.13` (0.13.1 locked) | One copy of each in the lockfile. vello re-exports both. |
| parley | `=0.12.0` | fontique 0.12, `FontData`/`Blob` come from `linebender_resource_handle`, shared with peniko. |
| loro 1.17, roughr 0.14, rstar 0.14, schemars 1.2, png 0.18, image 0.25, uuid 1 | | roughr uses euclid/palette, not kurbo, so it does not conflict. |

Rejected: wgpu 29 (what `vello_hybrid` 0.2.0 needs) and the 0.36 egui line, because 0.37 + wgpu 30 + vello 0.11 already agree.
wgpu 30.0.1 is a patch above vello's `^30.0.0`; fine.

Rules that came out of the spike (all are encoded in the root `Cargo.toml`):

- Pin the graphics crates with `=` and bump them together. `Cargo.lock` is committed.
- `egui` needs `default_fonts` when `default-features = false`, or all text disappears (egui_kittest hit this too).
- `egui-wgpu` and `eframe` must not use their default features. The default turns on `wgpu/webgl` (GLES backend, naga, glow).
  Vello needs compute shaders and cannot use WebGL anyway. Without it the wasm is 0.6 MB smaller after brotli (1.93 -> 1.36 MB).
  Use `eframe` feature `wgpu_no_default_features` and give `wgpu` its backends per target in the crate that owns the device:
  native `["std", "parking_lot", "wgsl", "vulkan", "gles", "metal", "dx12"]`, wasm32 `["std", "wgsl", "webgpu"]`.
- `parley` with `default-features = false, features = ["std"]` drops `fontique/system` (fontconfig, freetype). The app bundles its
  fonts, so text is identical on web and native and the native binary links only libc/libm/libgcc.
- `wasm-bindgen` is pinned `=0.2.130` and must equal the installed `wasm-bindgen-cli` (`cargo install wasm-bindgen-cli --version 0.2.130 --locked`).
- `cargo deny check licenses bans sources` with the repo's `deny.toml` on the full pinned set (pinset scratch crate) passes; the only error was the scratch crate having no licence. Advisories were not run.
- epaint 0.37 itself depends on `vello_cpu 0.2.0`. If we also add `vello_cpu 0.3` for the fallback (section 7) the build has two
  `vello_common` versions (0.2 and 0.3). That is harmless for correctness but costs compile time and some wasm size.

## 2. Texture compositing (what we build on)

One `vello::Renderer` is created on eframe's own `Device` (`CreationContext::wgpu_render_state`). Vello renders into an
`Rgba8Unorm` texture (`STORAGE_BINDING | TEXTURE_BINDING`) sized to the canvas rect in physical pixels. The texture view is
registered with egui-wgpu once (`register_native_texture`) and re-pointed on resize (`update_egui_texture_from_wgpu_texture`).
egui shows it with `egui::Image` and draws its widgets on top. No egui callback is needed because `render_to_texture` submits its
own command buffers before egui records its pass; this is simpler than a paint callback and worked on native and WebGPU.

```rust
// App::ui(&mut self, ui, frame)
let ppp = ui.ctx().pixels_per_point();
let rect = ui.available_rect_before_wrap();
let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
// ... input -> view change -> self.dirty = true  (no request_repaint)
if let Some(rs) = frame.wgpu_render_state().cloned() {
    let (w, h) = ((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32);
    self.ensure_target(&rs, w, h);          // (re)create texture + register/update the egui TextureId
    if self.dirty { self.render_vello(&rs, ppp as f64); }
}
egui::Image::new(egui::load::SizedTexture::new(target.id, rect.size())).paint_at(ui, rect);

fn render_vello(&mut self, rs: &RenderState, ppp: f64) {
    // view: world -> physical pixels (pan is in logical points)
    let view = Affine::scale(ppp) * Affine::translate(self.pan) * Affine::scale(self.zoom);
    let vis = view.inverse().transform_rect_bbox(Rect::new(0.0, 0.0, w as f64, h as f64));
    self.frame_scene.reset();
    for item in &self.world {                         // one cached vello::Scene per element
        if overlaps(&item.bbox, &vis) { self.frame_scene.append(&item.scene, Some(view)); }
    }
    self.vello.render_to_texture(&rs.device, &rs.queue, &self.frame_scene, &target.view,
        &RenderParams { base_color, width: w, height: h, antialiasing_method: AaConfig::Area })?;
    self.dirty = false;
}
```

Vello setup: `RendererOptions { antialiasing_support: AaSupport::area_only(), ..Default::default() }` (fewer shader permutations,
startup to first frame was 0.6 s on web). Text: `parley` builds a `Layout`; each `GlyphRun` goes to
`scene.draw_glyphs(run.font()).font_size(..).transform(..).brush(..).draw(Fill::NonZero, glyphs.map(|g| vello::Glyph{id:g.id,x:g.x,y:g.y}))`.
Fonts are registered from bytes (`font_cx.collection.register_fonts(peniko::Blob::new(Arc::new(BYTES)), None)`), because wasm has no system fonts.

**Culling matters more than anything else in the frame cost.** Vello resolves every glyph run and path in the scene it is given,
whether visible or not. Spike scene: 4000 shapes + 400 text labels (about 15k glyphs).

| scene given to Vello | CPU time of `append` + `render_to_texture` (native, release) |
|---|---|
| everything (no culling) | 8.9 ms (shapes only 1.5 ms, text only 7.8 ms) |
| only items overlapping the view (528 of 4400 visible) | 0.95 ms |

So the engine keeps one encoded `Scene` per element (invalidated on change) and appends only the elements the spatial index returns
for the viewport. `Scene::append` of a cached child scene with a transform cost 0.07 ms for the whole 4400.

## 3. Repaint rules

1. Nothing calls `request_repaint` on its own. Every redraw has a reason: a winit/DOM event that changed state, an egui animation
   (egui asks), or a timed request (`request_repaint_after`) tied to pending work.
2. State changes set `dirty`. The Vello pass runs only if `dirty` or the target size changed. A frame that only moved a menu
   re-composites the old texture without running Vello.
3. The HUD/overlay text is egui, so a counter change there also needs a frame; do not put a ticking clock on screen.
4. Verified: native idle 0.3 % of a core, 0 egui frames, 0 Vello frames over 10 s, also 10 s after a pan and after wheel input
   (`scripts/native_cpu.py`). Web: 0 frames in every idle window (`scripts/web_run.py`).
5. Caveat (web only): `eframe::WebRunner` keeps a `requestAnimationFrame` loop alive forever and merely skips painting
   (eframe-0.37 `web/events.rs: paint_and_schedule`). Cost: 3.3-4.2 % of a core idle against 0.4-0.6 % for a blank page. See section 5.

## 4. Measurements

Machine: 64 threads, NVIDIA L40S, Xvfb (no vsync, so frame rates are not paced by a display), Chromium 1208 headed, WebGPU via ANGLE/Vulkan on the NVIDIA adapter.
Scene: 4000 shapes + 400 labels, culled, 1280x800 native / 1400x860 web. Input is real X input via XTEST (125 pointer moves/s,
100 wheel ticks/s). CPU is % of one core.

| | idle | drag-pan | wheel |
|---|---|---|---|
| native (eframe, Vulkan) | 0.3 %, 0 frames | 65.6 % (177 fps) | 58.8 % (228 fps) |
| native, no culling | 0.2 % | 87.4 % (60 fps) | 88.6 % (58 fps) |
| web, eframe (all Chromium processes) | 3.8-4.2 % (blank page 0.6 %), 0 frames | 103 % (49 fps) | 99 % (49 fps) |
| web, own winit shell (`stack-winit`) | 0.7-1.2 %, 0 frames | 103 % (49 fps) | 98 % (50 fps) |

Web pan is GPU-process bound under Xvfb (the research in `web-perf.md` found the same ~30-50 fps ceiling there); the app side is a
small part. Do not read absolute web fps from Xvfb.

Wasm size of the spike (eframe + vello + parley + egui + one font), `wasm-bindgen --target web`, binaryen 133 `wasm-opt --all-features --strip-debug --strip-producers`:

| profile | raw | wasm-opt -Os | wasm-opt -Oz | gzip -9 (of -Os) | brotli -11 (of -Os) |
|---|---|---|---|---|---|
| opt-level `s`, with `webgl` (egui-wgpu defaults) | 11.94 MB | 8.03 MB | 7.69 MB | 2.65 MB | 1.92 MB |
| opt-level `s`, WebGPU only (the pin) | 8.55 MB | 6.04 MB | 5.75 MB | 1.88 MB | **1.36 MB** |
| opt-level `z`, WebGPU only | 9.49 MB | 6.16 MB | 5.83 MB | 1.85 MB | 1.33 MB |

`z` gives 2 % smaller brotli and slower code, so use `s` (already the `release-wasm` profile). The old app was 10.3 MB raw, 3.46 MB brotli.
Code is 4.9 MB of the raw file, data 1.7 MB (egui fonts about 1.2 MB, Vello shaders, ICU data), the name section 1.9 MB (removed by `--strip-debug`).
Use `strip = "debuginfo"` rather than `strip = true`: with `strip = true` wasm-bindgen cannot read the target features.
Startup in Chromium: 0.6 s from navigation to first Vello frame (adapter, device, 1 shader set, 4400 elements).

Tools: `wasm-bindgen-cli 0.2.130` in `~/.cargo/bin`, binaryen 133 in `~/.cargo/tools/binaryen-version_133/bin/wasm-opt` (the wasm-pack cached
one is v117, too old for current rustc output). `trunk` was not needed: `scripts/build_web.sh` plus a 20-line `index.html` is enough.

## 5. Decision for the shell: eframe now, winit shell as the proven alternative

Both shells were built and measured (`stack` and `stack-winit`). They share all app and Vello code.

- **eframe 0.37**: native and web work, IME/clipboard/drag-and-drop/hidden-tab handling on web come for free. Web idle costs about 3 points of a core
  because of the permanent rAF loop; no frames are painted.
- **own shell** (winit 0.30.13 + egui-winit + egui-wgpu, 372 lines, generic `App` trait with `run_native`/`run_web`): no rAF loop. Web idle
  0.7-1.2 %. Delayed repaint through `ControlFlow::WaitUntil` was verified (a 100 ms timer gives exactly 10 fps and 0 Vello frames). Same wasm size.
  Cost: on web it must supply what eframe did: IME and soft keyboard (winit 0.30 web has no IME events; Latin typing works), clipboard
  and copy/paste events, file drop and image paste, `OpenUrl`, canvas resize/DPI handling (done, tested on resize; real hi-dpi untested), touch/pinch (untested).
  Our canvas text editor is Parley-based and needs its own hidden `<textarea>` for IME in any case.

Recommendation: start on eframe, keep the app behind a thin `run(app)` entry in the `ui` crate so the shell can be swapped, and decide at the
web-app milestone using `cargo xtask perf --idle` on the web build. If the 3-point idle cost fails the idle gate on a real device, switch to the own shell.
This keeps `egui` types and all widget code identical in both cases.

## 6. Tests without a GPU: egui_kittest 0.37

Works. `Harness::new_ui_state(|ui, state| ..., state)`, `h.get_by_label("Increment").click(); h.run();`, `get_by_role(Role::TextInput).type_text("hi")`.
- Click and text-edit tests need only `egui_kittest` with default features (no `wgpu`, no GPU, no display; 2 tests in 0.02 s; also run with `DISPLAY`/`WAYLAND_DISPLAY` unset).
- Snapshot tests need `features = ["snapshot", "wgpu"]`. They render with a wgpu adapter. With lavapipe only
  (`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`) the snapshot is produced, and the same PNG then also passes on the NVIDIA adapter
  (default dif threshold). CI without a GPU can therefore run snapshots on Mesa lavapipe. Missing snapshots fail the test and write `*.new.png`; `UPDATE_SNAPSHOTS=1` accepts them.
- `egui` needs the `default_fonts` feature here too (the spike's first kittest build failed to render text without it).
- Checked with the pinned root set (`pinset`): `egui_kittest` workspace dep has `default-features = false`; a crate adds `features = ["snapshot", "wgpu"]` itself.

## 7. CPU / WebGL fallback for browsers without WebGPU

Vello 0.11 is compute-only, so it has no WebGL2 path. Two other renderers from the same project share one API with each other (not with Vello):

- `vello_cpu 0.3.0` (`vello_common 0.3.0`, kurbo 0.13 / peniko 0.6, so the geometry and colour types are the same as ours). `RenderContext` is
  immediate-mode (`fill_path`, `stroke_path`, `glyph_run(..).fill_glyphs`), the output is a `Pixmap` that goes to a 2D canvas with `putImageData`.
- `vello_gpu 0.3.0` (the new name of `vello_hybrid`; `vello_hybrid 0.2.0` is the old line and needs wgpu 29). It needs wgpu 30 for its `wgpu` feature and has a
  `webgl` feature (raw web-sys WebGL2 with GLSL shaders, `WebGlRenderer`), so it would not clash with our wgpu pin. Not run in this spike.

Proof for `vello_cpu` (`/home/ubuntu/git/drafftink-spikes/cpufallback`): 3000 shapes (rects, rounded rects, bezier blobs, freehand lines; fill + stroke) plus 8136 glyphs
via Parley at 1600x900, single thread, same code on both targets, Chromium headless (2D canvas screenshots work headless):

| target | build (path to strips) | raster | wasm |
|---|---|---|---|
| native release | 39 ms | 9 ms | n/a |
| wasm32 without simd128 | 510 ms | 3800 ms | 4.9 MB raw |
| wasm32 `-C target-feature=+simd128` | 115 ms | 227 ms | 3.5 MB raw |

Assessment: feasible as a degraded mode, not as a drop-in. (1) It needs `+simd128` (all current browsers have it) or it is unusable. (2) A full-screen 3000-shape
frame takes about 350 ms in wasm, so the fallback only suits culled, mostly static views and must render on demand and never during animations; the viewport culling
we need for Vello makes this workable. (3) The draw API differs from `vello::Scene` (immediate `RenderContext` against retained `Scene` with glyph runs), so `render`
must produce an intermediate display list or a small `trait Painter` that two backends implement; do not let `vello::Scene` leak above L2. (4) Glyph drawing works from Parley
glyph runs the same way. (5) Fallback adds about 3.5 MB raw wasm if shipped in the same module, so ship it as a second wasm module loaded only when `navigator.gpu` is absent.
The no-WebGPU detection path itself (eframe returns an error from `WebRunner::start`) was not exercised.
An alternative if the fallback matters more than expected is `vello_gpu` with `webgl`, which keeps GPU speed; evaluate it when the fallback is scheduled.

## 8. Platform notes

- **Linux**: winit 0.30 with `x11` and `wayland` (eframe features `x11`, `wayland`). Wayland compiled only, X11 run. The binary links only libc, libm and libgcc; winit loads the X11/Wayland
  and xkbcommon libraries with dlopen at run time (`libxkbcommon`, `libX11`/`libxcb` or `libwayland-client`). Vello needs a Vulkan driver (`libvulkan.so.1` + ICD; Mesa lavapipe works
  for headless CI). The `gles` feature is optional and not used by Vello (compute shaders and storage textures are required).
- **Windows**: wgpu `dx12` (Vulkan also works if present). Not run here; there is no Windows machine. No extra DLLs are expected.
- **macOS**: wgpu `metal`; `egui-wgpu` default `macos-window-resize-jitter-fix` is dropped with `default-features = false`, so re-enable it for the macOS target if resize jitter shows (feature `egui-wgpu/macos-window-resize-jitter-fix`). Not run here.
- **Web**: needs WebGPU: Chromium-based browsers (verified, Chromium 1208), Safari/Firefox depend on version and flags and were not tested. Headless Chromium renders a blank canvas for WebGPU on this machine; use headed Chromium under Xvfb
  with `--enable-unsafe-webgpu --enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan --use-angle=vulkan --use-vulkan=native --ignore-gpu-blocklist --disable-vulkan-surface --no-sandbox --disable-gpu-sandbox`.
  Screenshot of the spike (Chromium WebGPU, 4000 shapes and 400 labels, wasm32): `docs/img/stack-spike-web.png`.
- Rust 1.97.1 (the pinned toolchain), edition 2024. wasm-bindgen CLI 0.2.130.

## 9. How to reproduce

```
cd /home/ubuntu/git/drafftink-spikes/stack
cargo build --release                       # native; run with WGPU_BACKEND=vulkan
scripts/build_web.sh s                      # web/pkg-s (needs wasm-bindgen-cli 0.2.130)
~/.venvs/perf/bin/python scripts/web_run.py pkg-s --shot /tmp/shot.png      # headed Chromium on Xvfb, idle/pan/wheel numbers
~/.venvs/perf/bin/python scripts/native_cpu.py <binary> WGPU_BACKEND=vulkan # native numbers
~/.venvs/perf/bin/python scripts/sizes.py web/pkg-s/stack_spike_bg.wasm     # raw / wasm-opt / gzip / brotli
```
Set `SPIKE_NO_CULL=1`, `SPIKE_NO_TEXT=1`, `SPIKE_NO_SHAPES=1`, `SPIKE_LOG=1` to repeat the culling experiment.
Do not pipe the scripts into `head`: the SIGPIPE leaves Xvfb and the app running and pollutes the next run.
