#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::field_reassign_with_default)]
//! Pixel tests on a real GPU adapter (feature `gpu-tests`; run `cargo test -p drafftink-render --features gpu-tests
//! --test gpu`). They render with Vello into an offscreen texture and compare with the PNGs the old renderer made
//! (`tests/fixtures/render`, see its README) with a per-channel tolerance of 16, and compare the composer's output
//! with a render of the same elements drawn without any caching or culling.
//!
//! The device is created here, in test code. The library never creates one.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use common::*;
use drafftink_render::*;
use drafftink_scene::*;
use drafftink_typeset::{LineHeight, Typesetter};
use kurbo::{Point, Size, Vec2};
use serde_json::Value;
use vello::wgpu;
use vello::{AaConfig, RenderParams, Renderer, RendererOptions, Scene};

const TOLERANCE: u8 = 16;

// ----------------------------------------------------------------------------------------------
// Offscreen rendering

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    adapter: String,
}

/// The shared device, or `None` on a machine without an adapter. Such a machine skips these tests with a message;
/// set `DRAFFTINK_REQUIRE_GPU=1` (the GPU runner does) to make a missing adapter a failure instead.
fn gpu_opt() -> Option<&'static Mutex<Gpu>> {
    static GPU: OnceLock<Option<Mutex<Gpu>>> = OnceLock::new();
    GPU.get_or_init(|| {
        let mut cx = vello::util::RenderContext::new();
        let Some(id) = pollster::block_on(cx.device(None)) else {
            assert!(!drafftink_profile::gpu_required(), "no GPU adapter and DRAFFTINK_REQUIRE_GPU is set");
            return None;
        };
        let handle = &cx.devices[id];
        let adapter = format!("{:?}", handle.adapter().get_info());
        let (device, queue) = (handle.device.clone(), handle.queue.clone());
        let renderer = Renderer::new(
            &device,
            RendererOptions { antialiasing_support: vello::AaSupport::area_only(), ..RendererOptions::default() },
        )
        .expect("renderer");
        Some(Mutex::new(Gpu { device, queue, renderer, adapter }))
    })
    .as_ref()
}

fn gpu() -> &'static Mutex<Gpu> {
    gpu_opt().expect("checked by need_gpu")
}

macro_rules! need_gpu {
    () => {
        if gpu_opt().is_none() {
            drafftink_profile::report_skip(
                "no GPU adapter on this machine; set DRAFFTINK_REQUIRE_GPU=1 to fail instead",
            );
            return;
        }
    };
}

struct Pixels {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

fn render(scene: &Scene, w: u32, h: u32, base: peniko::Color) -> Pixels {
    let mut g = gpu().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let g = &mut *g;
    let texture = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render test"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    g.renderer
        .render_to_texture(
            &g.device,
            &g.queue,
            scene,
            &view,
            &RenderParams { base_color: base, width: w, height: h, antialiasing_method: AaConfig::Area },
        )
        .expect("render");
    let bytes_per_row = (w * 4).next_multiple_of(256);
    let buffer = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("copy") });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    g.queue.submit(std::iter::once(enc.finish()));
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).ok();
    });
    let _ = g.device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().unwrap().unwrap();
    let data = slice.get_mapped_range().unwrap();
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let s = (row * bytes_per_row) as usize;
        rgba.extend_from_slice(&data[s..s + (w * 4) as usize]);
    }
    drop(data);
    buffer.unmap();
    Pixels { w, h, rgba }
}

/// How two renders differ.
#[derive(Debug)]
struct Diff {
    /// Largest per-channel difference.
    max: u8,
    /// Share of pixels with any channel differing by more than `TOLERANCE`.
    beyond: f64,
    /// Share of pixels that differ at all.
    any: f64,
}

fn diff(a: &Pixels, b: &Pixels) -> Diff {
    diff_masked(a, b, &[])
}

/// Like [`diff`], skipping pixels inside the `ignore` rectangles (device pixels).
fn diff_masked(a: &Pixels, b: &Pixels, ignore: &[kurbo::Rect]) -> Diff {
    assert_eq!((a.w, a.h), (b.w, b.h), "sizes differ");
    let (mut max, mut beyond, mut any) = (0u8, 0usize, 0usize);
    let mut skipped = 0usize;
    for (i, (p, q)) in a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)).enumerate() {
        let pt = Point::new(f64::from(i as u32 % a.w) + 0.5, f64::from(i as u32 / a.w) + 0.5);
        if ignore.iter().any(|r| r.contains(pt)) {
            skipped += 1;
            continue;
        }
        let d = p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        max = max.max(d);
        beyond += usize::from(d > TOLERANCE);
        any += usize::from(d > 0);
    }
    let n = ((a.w * a.h) as usize - skipped).max(1) as f64;
    Diff { max, beyond: beyond as f64 / n, any: any as f64 / n }
}

fn read_png(path: &Path) -> Pixels {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            buf[..info.buffer_size()].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        }
        other => panic!("unexpected png colour type {other:?}"),
    };
    Pixels { w: info.width, h: info.height, rgba }
}

fn write_png(path: &Path, p: &Pixels) {
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path).unwrap()), p.w, p.h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&p.rgba).unwrap();
}

// ----------------------------------------------------------------------------------------------
// The old documents: a small converter (the real one is io's job), sized by the typesetter.

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn num(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn color(v: &Value) -> Color {
    let c = |k| v.get(k).and_then(Value::as_u64).unwrap_or(0) as u8;
    Color::rgba(c("r"), c("g"), c("b"), c("a"))
}

fn point(v: &Value) -> Point {
    Point::new(num(v, "x", 0.0), num(v, "y", 0.0))
}

fn style_of(v: &Value) -> Style {
    let s = &v["style"];
    let mut st = Style::default();
    st.stroke_color = color(&s["stroke_color"]);
    st.stroke_width = num(s, "stroke_width", 2.0).max(0.0);
    st.fill_color = s.get("fill_color").filter(|c| !c.is_null()).map(color);
    st.fill_pattern = match s["fill_pattern"].as_str().unwrap_or("Solid") {
        "Hachure" => FillPattern::Hachure,
        "ZigZag" => FillPattern::ZigZag,
        "CrossHatch" => FillPattern::CrossHatch,
        "Dots" => FillPattern::Dots,
        "Dashed" => FillPattern::Dashed,
        "ZigZagLine" => FillPattern::ZigZagLine,
        _ => FillPattern::Solid,
    };
    st.sloppiness = match s["sloppiness"].as_str().unwrap_or("Artist") {
        "Architect" => Sloppiness::Architect,
        "Cartoonist" => Sloppiness::Cartoonist,
        "Drunk" => Sloppiness::Drunk,
        _ => Sloppiness::Artist,
    };
    st.seed = s["seed"].as_u64().unwrap_or(0) as u32;
    st.opacity = num(s, "opacity", 1.0).clamp(0.0, 1.0);
    st
}

struct Legacy {
    /// Regions (old world coordinates) of shapes that this test does not compare; see `ignored_shape`.
    ignored: Vec<kurbo::Rect>,
    elements: Vec<Element>,
    files: Vec<FileEntry>,
    n: usize,
}

impl Legacy {
    fn push(&mut self, mut e: Element) {
        e.id = id(&format!("l{}", self.n));
        self.n += 1;
        self.elements.push(e);
    }

    fn shape(&mut self, shape: &Value, ts: &mut Typesetter) {
        let Some((tag, v)) = shape.as_object().and_then(|o| o.iter().next()) else { return };
        if tag == "Rectangle" {
            let (p, w, h) = (point(&v["position"]), num(v, "width", 0.0), num(v, "height", 0.0));
            let dots = v["style"]["fill_pattern"].as_str() == Some("Dots") && v["style"]["fill_color"].is_object();
            // Known differences from the old renderer, listed in docs/render.md: the position of the dots of a
            // Dots fill (they come from the sketch crate's random stream), and rectangles with a zero or negative
            // size (the old code drew them unnormalised; the model normalises).
            if dots || w <= 0.0 || h <= 0.0 {
                self.ignored.push(
                    kurbo::Rect::new(p.x.min(p.x + w), p.y.min(p.y + h), p.x.max(p.x + w), p.y.max(p.y + h))
                        .inflate(12.0, 12.0),
                );
            }
        }
        let mut st = style_of(v);
        let angle = num(v, "rotation", 0.0);
        let z = ZIndex::first();
        match tag.as_str() {
            "Rectangle" => {
                st.corner_radius = num(v, "corner_radius", 0.0).max(0.0);
                let p = point(&v["position"]);
                let (w, h) = (num(v, "width", 0.0), num(v, "height", 0.0));
                let mut e = Element::new(id("x"), ElementKind::Rectangle, z).with_style(st);
                e.transform = Transform {
                    x: p.x.min(p.x + w),
                    y: p.y.min(p.y + h),
                    w: w.abs(),
                    h: h.abs(),
                    angle,
                    flip_x: false,
                    flip_y: false,
                };
                self.push(e);
            }
            "Ellipse" => {
                let (c, rx, ry) = (point(&v["center"]), num(v, "radius_x", 0.0).abs(), num(v, "radius_y", 0.0).abs());
                let mut e = Element::new(id("x"), ElementKind::Ellipse, z).with_style(st);
                e.transform = Transform {
                    x: c.x - rx,
                    y: c.y - ry,
                    w: 2.0 * rx,
                    h: 2.0 * ry,
                    angle,
                    flip_x: false,
                    flip_y: false,
                };
                self.push(e);
            }
            "Line" | "Arrow" => {
                let arrow = tag == "Arrow";
                let mut pts = vec![point(&v["start"])];
                pts.extend(v["intermediate_points"].as_array().into_iter().flatten().map(point));
                pts.push(point(&v["end"]));
                let closed = v["closed"].as_bool().unwrap_or(false) && pts.len() >= 3;
                let Some(mut e) =
                    Element::linear(id("x"), if arrow { LinearKind::Arrow } else { LinearKind::Line }, z, &pts)
                else {
                    return;
                };
                if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut e.kind {
                    d.path = match v["path_style"].as_str() {
                        Some("Flowing") => PathType::Round,
                        Some("Angular") => PathType::Elbow,
                        _ => PathType::Sharp,
                    };
                    d.closed = closed;
                    if arrow {
                        d.end_head = Head::Arrow;
                        d.head_size = Some(num(v, "head_size", 15.0));
                    }
                }
                st.stroke_style = match v["stroke_style"].as_str() {
                    Some("Dashed") => StrokeStyle::Dashed,
                    Some("Dotted") => StrokeStyle::Dotted,
                    _ => StrokeStyle::Solid,
                };
                e.style = st;
                e.transform.angle = angle;
                self.push(e);
            }
            "Freehand" => {
                let pts: Vec<Point> = v["points"].as_array().into_iter().flatten().map(point).collect();
                let mut d = FreehandData::new(pts);
                d.pressures = v["pressures"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
                if d.pressures.len() != d.points.len() {
                    d.pressures.clear();
                } else if !d.pressures.is_empty() {
                    // What the legacy loader makes of a stroke that has pressures.
                    d.pressure_mode = PressureMode::Device;
                }
                d.closed = v["closed"].as_bool().unwrap_or(false);
                let mut e = Element::new(id("x"), ElementKind::Freehand(d), z).with_style(st);
                if e.refit_points().is_ok() {
                    self.push(e);
                }
            }
            "Text" => {
                let mut t = TextData::new(v["content"].as_str().unwrap_or(""));
                t.font.size = num(v, "font_size", 20.0).max(1.0);
                t.font.family = match v["font_family"].as_str() {
                    Some("NotoSans") => FontFamily::NotoSans,
                    Some("GelPenSerif") => FontFamily::GelPenSerif,
                    Some("VanillaExtract") => FontFamily::VanillaExtract,
                    _ => FontFamily::GelPen,
                };
                t.font.weight = match v["font_weight"].as_str() {
                    Some("Light") => FontWeight::Light,
                    Some("Heavy") => FontWeight::Heavy,
                    _ => FontWeight::Regular,
                };
                let colors: Vec<Option<Color>> = v["char_colors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| (!c.is_null()).then(|| color(c)))
                    .collect();
                t.runs = TextData::runs_from_char_colors(&colors, t.char_count());
                // The old renderer used the font's own line height. Express that as a multiple of the size so
                // that `text_spec` reproduces it.
                let mut e = Element::new(id("x"), ElementKind::Text(t), z).with_style(st);
                let p = point(&v["position"]);
                e.transform = Transform::new(p.x, p.y, 0.0, 0.0);
                e.transform.angle = angle;
                if let ElementKind::Text(t) = &mut e.kind {
                    let mut spec = text_spec(t, e.style.stroke_color, 0.0);
                    spec.line_height = LineHeight::Normal;
                    spec.text = "Hg".into();
                    spec.spans.clear();
                    let one_line = ts.measure(&spec).height;
                    t.font.line_height = one_line / t.font.size;
                }
                measure_text(ts, &mut e);
                self.push(e);
            }
            "Math" => {
                let mut m = MathData::new(v["latex"].as_str().unwrap_or(""));
                m.font_size = num(v, "font_size", 20.0).max(1.0);
                let p = point(&v["position"]);
                let (w, h, top) = match ts.math(&math_spec(&m)) {
                    Ok(l) => (l.width, l.height - l.depth, l.height),
                    Err(_) => (40.0, 24.0, 24.0),
                };
                let mut e = Element::new(id("x"), ElementKind::Math(m), z).with_style(st);
                // The old position is the baseline origin; the box starts `height` above it.
                e.transform = Transform { x: p.x, y: p.y - top, w, h, angle, flip_x: false, flip_y: false };
                self.push(e);
            }
            "Image" => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(v["data_base64"].as_str().unwrap_or(""))
                    .unwrap_or_default();
                let (fid, entry) = FileEntry::new("image/png", bytes);
                self.files.push(entry);
                let p = point(&v["position"]);
                let data = ImageData {
                    file_id: Some(fid),
                    natural_width: v["source_width"].as_u64().unwrap_or(0) as u32,
                    natural_height: v["source_height"].as_u64().unwrap_or(0) as u32,
                    crop: None,
                };
                // The old renderer ignored the style of an image.
                let mut e = Element::new(id("x"), ElementKind::Image(data), z);
                e.style.stroke_width = 0.0;
                e.transform = Transform {
                    x: p.x,
                    y: p.y,
                    w: num(v, "width", 0.0),
                    h: num(v, "height", 0.0),
                    angle,
                    flip_x: false,
                    flip_y: false,
                };
                self.push(e);
            }
            "Group" => {
                for c in v["children"].as_array().into_iter().flatten() {
                    self.shape(c, ts);
                }
            }
            _ => {}
        }
    }
}

struct Scene2 {
    ignored: Vec<kurbo::Rect>,
    store: MemoryStore,
    index: PaintIndex,
    bounds: [f64; 4],
}

fn load_legacy(name: &str, ts: &mut Typesetter) -> Scene2 {
    let old: Value =
        serde_json::from_str(&std::fs::read_to_string(fixtures().join(format!("legacy/{name}.json"))).unwrap())
            .unwrap();
    let facts: Value =
        serde_json::from_str(&std::fs::read_to_string(fixtures().join(format!("legacy/{name}.facts.json"))).unwrap())
            .unwrap();
    let b = &facts["doc"]["bounds"];
    let bounds = [0, 1, 2, 3].map(|i| b[i].as_f64().unwrap());
    let mut c = Legacy { ignored: Vec::new(), elements: Vec::new(), files: Vec::new(), n: 0 };
    for sid in old["z_order"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if let Some(shape) = old["shapes"].get(sid) {
            c.shape(shape, ts);
        }
    }
    let keys = ZIndex::rebalance(c.elements.len()).unwrap();
    let ignored = c.ignored;
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    for entry in c.files {
        tx.put_file(entry).unwrap();
    }
    for (mut e, k) in c.elements.into_iter().zip(keys) {
        e.index = k;
        tx.insert(e).unwrap();
    }
    tx.commit().unwrap();
    let index = build_paint_index(&store);
    Scene2 { ignored, store, index, bounds }
}

/// The old PNG export: bounds plus 20 px on every side, scale 1, white.
fn export_view(bounds: [f64; 4], w: u32, h: u32) -> View {
    View::new(1.0, Vec2::new(20.0 - bounds[0], 20.0 - bounds[1]), Size::new(f64::from(w), f64::from(h)))
}

fn render_scene2(s: &Scene2, ts: &mut Typesetter, w: u32, h: u32) -> Pixels {
    let mut composer = SceneComposer::new();
    composer.set_options(RenderOptions::for_export(Theme::Light));
    let mut scene = Scene::new();
    composer.compose(&s.store, &s.index, &export_view(s.bounds, w, h), ts, &mut scene);
    render(&scene, w, h, peniko::Color::WHITE)
}

/// Compares our render of a legacy document with the old renderer's PNG.
fn compare_with_old(name: &str, max_beyond: f64) {
    let reference = read_png(&fixtures().join(format!("render/{name}.png")));
    let mut ts = Typesetter::new();
    let s = load_legacy(name, &mut ts);
    let ours = render_scene2(&s, &mut ts, reference.w, reference.h);
    let view = export_view(s.bounds, reference.w, reference.h);
    let ignore: Vec<kurbo::Rect> = s
        .ignored
        .iter()
        .map(|r| {
            let (a, b) =
                (view.world_to_device() * Point::new(r.x0, r.y0), view.world_to_device() * Point::new(r.x1, r.y1));
            kurbo::Rect::new(a.x, a.y, b.x, b.y)
        })
        .collect();
    let d = diff_masked(&ours, &reference, &ignore);
    let adapter = gpu().lock().unwrap().adapter.clone();
    eprintln!("{name}: {d:?} on {adapter}");
    if let Ok(dir) = std::env::var("RENDER_TEST_OUT") {
        write_png(&Path::new(&dir).join(format!("{name}.png")), &ours);
        let mut mark = Pixels { w: ours.w, h: ours.h, rgba: vec![255; ours.rgba.len()] };
        for (i, (p, q)) in ours.rgba.chunks_exact(4).zip(reference.rgba.chunks_exact(4)).enumerate() {
            if p.iter().zip(q).any(|(x, y)| x.abs_diff(*y) > TOLERANCE) {
                mark.rgba[i * 4..i * 4 + 3].copy_from_slice(&[255, 0, 0]);
            }
        }
        write_png(&Path::new(&dir).join(format!("{name}_diff.png")), &mark);
    }
    assert!(
        d.beyond <= max_beyond,
        "{name}: {:.3}% of pixels differ by more than {TOLERANCE} (max {}, any {:.2}%)",
        d.beyond * 100.0,
        d.max,
        d.any * 100.0
    );
}

// ----------------------------------------------------------------------------------------------
// Against the old renderer

#[test]
fn rectangles_match_the_old_renderer() {
    need_gpu!();
    compare_with_old("styles_rectangle", 0.0);
}

#[test]
fn freehand_matches_the_old_renderer() {
    need_gpu!();
    compare_with_old("styles_freehand", 0.0);
}

#[test]
fn the_shape_sampler_matches_the_old_renderer() {
    need_gpu!();
    compare_with_old("all_shapes", 0.004);
}

#[test]
fn math_matches_the_old_renderer() {
    need_gpu!();
    compare_with_old("math", 0.015);
}

#[test]
fn text_matches_the_old_renderer() {
    need_gpu!();
    compare_with_old("text_char_colors", 0.0);
}

#[test]
fn nested_groups_match_the_old_renderer() {
    need_gpu!();
    compare_with_old("groups_nested", 0.02);
}

#[test]
fn fonts_match_the_old_renderer() {
    need_gpu!();
    compare_with_old("text_fonts", 0.02);
}

#[test]
fn images_match_the_old_renderer() {
    need_gpu!();
    compare_with_old("images", 0.03);
}

// ----------------------------------------------------------------------------------------------
// Against ourselves

fn all_at_once(store: &MemoryStore, view: &View, ts: &mut Typesetter, opts: &RenderOptions) -> Scene {
    // Every element that touches the view (brute force), in z order, painted fresh: no cache, no index, no level
    // of detail. Elements far outside the view are left out: Vello draws partial tiles at the edge of the target
    // slightly differently when the scene holds geometry far away, which is not what this test is about.
    let mut painter = ElementPainter::new();
    let mut scene = Scene::new();
    let to_dev = view.world_to_device();
    let world = view.world_rect();
    for e in store.iter().filter(|e| drafftink_geom::aabb::intersects(&world, &paint_bounds(e))) {
        let f = painter.paint(e, opts, ts, &|fid| store.file(fid).map(|x| x.bytes.clone()));
        scene.append(&f.scene, Some(to_dev * element_to_world(e)));
    }
    scene
}

#[test]
fn culling_and_caching_change_no_pixel() {
    need_gpu!();
    let mut ts = Typesetter::new();
    let store = store_of(mixed_elements(500, 3, &mut ts));
    let index = build_paint_index(&store);
    let opts = RenderOptions::for_export(Theme::Light);
    let mut composer = SceneComposer::new();
    composer.set_options(opts);
    let (w, h) = (700u32, 500u32);
    for (zoom, x, y) in [(1.0, 300.0, 200.0), (0.6, 0.0, 0.0), (2.5, 1000.0, 800.0)] {
        let view = View::new(zoom, Vec2::new(-x * zoom, -y * zoom), Size::new(f64::from(w), f64::from(h)));
        let reference = render(&all_at_once(&store, &view, &mut ts, &opts), w, h, peniko::Color::WHITE);
        let mut scene = Scene::new();
        composer.compose(&store, &index, &view, &mut ts, &mut scene);
        let cold = render(&scene, w, h, peniko::Color::WHITE);
        composer.compose(&store, &index, &view, &mut ts, &mut scene);
        let warm = render(&scene, w, h, peniko::Color::WHITE);
        for (what, px) in [("cold", &cold), ("warm", &warm)] {
            let d = diff(px, &reference);
            assert!(d.beyond == 0.0, "zoom {zoom} {what}: {d:?}");
        }
    }
}

#[test]
fn a_moved_element_looks_like_one_encoded_there() {
    need_gpu!();
    let mut ts = Typesetter::new();
    let mut store = store_of(mixed_elements(40, 8, &mut ts));
    let mut index = build_paint_index(&store);
    let mut warm = SceneComposer::new();
    warm.set_options(RenderOptions::for_export(Theme::Light));
    let view = View::new(1.0, Vec2::new(-100.0, -100.0), Size::new(600.0, 400.0));
    let mut scene = Scene::new();
    warm.compose(&store, &index, &view, &mut ts, &mut scene);
    for name in ["e0", "e1", "e3", "e4", "e5", "e6", "e7"] {
        let cs = update(&mut store, name, |e| {
            e.transform.x += 37.0;
            e.transform.y -= 11.0;
            e.transform.angle = 0.7;
        });
        update_paint_index(&mut index, &store, &cs);
    }
    warm.compose(&store, &index, &view, &mut ts, &mut scene);
    let moved = render(&scene, 600, 400, peniko::Color::WHITE);
    let mut cold = SceneComposer::new();
    cold.set_options(RenderOptions::for_export(Theme::Light));
    cold.compose(&store, &index, &view, &mut ts, &mut scene);
    let fresh = render(&scene, 600, 400, peniko::Color::WHITE);
    assert_eq!(diff(&moved, &fresh).beyond, 0.0);
}

#[test]
fn the_dark_theme_draws_light_ink_on_a_dark_background() {
    need_gpu!();
    let mut ts = Typesetter::new();
    let mut e = rect_el("r", &ZIndex::first(), 20.0, 20.0, 100.0, 60.0);
    e.style.stroke_color = Color::BLACK;
    e.style.sloppiness = Sloppiness::Architect;
    e.style.stroke_width = 4.0;
    let store = store_of(vec![e]);
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::new();
    composer.set_options(RenderOptions { theme: Theme::Dark, ..RenderOptions::default() });
    let mut scene = Scene::new();
    let view = View::new(1.0, Vec2::ZERO, Size::new(140.0, 100.0));
    composer.compose(&store, &index, &view, &mut ts, &mut scene);
    let px = render(&scene, 140, 100, composer.background(&store));
    let at = |x: u32, y: u32| {
        let i = ((y * px.w + x) * 4) as usize;
        [px.rgba[i], px.rgba[i + 1], px.rgba[i + 2]]
    };
    assert_eq!(at(5, 5), [18, 18, 18], "background");
    let edge = at(70, 20);
    assert!(edge[0] > 200, "black ink became light: {edge:?}");
}

#[test]
fn an_overlay_over_a_world_scene_renders() {
    need_gpu!();
    let mut ts = Typesetter::new();
    let store = store_of(mixed_elements(30, 2, &mut ts));
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::new();
    let view = View::new(1.0, Vec2::ZERO, Size::new(400.0, 300.0));
    let (mut world, mut overlay, mut out) = (Scene::new(), Scene::new(), Scene::new());
    composer.compose(&store, &index, &view, &mut ts, &mut world);
    let o = Overlay {
        marquee: Some(kurbo::Rect::new(50.0, 50.0, 200.0, 150.0)),
        selection: vec![SelectionBox {
            obox: drafftink_geom::OrientedBox::from_xywh(60.0, 60.0, 100.0, 80.0, 0.2),
            handles: drafftink_geom::select::HandleSet::ALL,
            dashed: true,
        }],
        ..Overlay::default()
    };
    paint_overlay(&mut overlay, &o, &view, &OverlayStyle::default(), &mut ts);
    combine(&world, &overlay, &mut out);
    let with = render(&out, 400, 300, peniko::Color::WHITE);
    let without = render(&world, 400, 300, peniko::Color::WHITE);
    assert!(diff(&with, &without).any > 0.001, "the overlay changed nothing");
}

// ----------------------------------------------------------------------------------------------
// Overlay parts and frame labels

fn rgb_at(p: &Pixels, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * p.w + x) * 4) as usize;
    [p.rgba[i], p.rgba[i + 1], p.rgba[i + 2]]
}

/// An overlay alone on white, 400x300 at zoom 1.
fn overlay_only(o: &Overlay) -> Pixels {
    let view = View::new(1.0, Vec2::ZERO, Size::new(400.0, 300.0));
    let mut scene = Scene::new();
    paint_overlay(&mut scene, o, &view, &OverlayStyle::default(), &mut Typesetter::new());
    render(&scene, 400, 300, peniko::Color::WHITE)
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24)
}

#[test]
fn peers_are_drawn_in_their_own_colours() {
    need_gpu!();
    let green = peniko::Color::from_rgba8(0, 160, 0, 255);
    let red = peniko::Color::from_rgba8(220, 0, 0, 255);
    let blue = peniko::Color::from_rgba8(0, 0, 220, 255);
    let o = Overlay {
        // The outline stands 4 px outside the box, 2 px wide: pixels 95 and 96 on the left edge.
        remote_selections: vec![RemoteSelection {
            color: green,
            boxes: vec![drafftink_geom::OrientedBox::from_xywh(100.0, 100.0, 100.0, 60.0, 0.0)],
        }],
        // The label chip starts 14 px right of and 16 px below the tip; its left padding is a plain chip colour.
        cursors: vec![RemoteCursor::new(Point::new(20.0, 20.0), red).named("Ada")],
        // The head of the trail is a dot of 3 px radius at the newest point.
        trails: vec![LaserTrail {
            color: blue,
            points: vec![
                TrailPoint { at: Point::new(300.0, 250.0), age_ms: 200.0 },
                TrailPoint { at: Point::new(330.0, 250.0), age_ms: 0.0 },
            ],
        }],
        ..Overlay::default()
    };
    let p = overlay_only(&o);
    assert!(close(rgb_at(&p, 96, 130), [0, 160, 0]), "outline: {:?}", rgb_at(&p, 96, 130));
    assert_eq!(rgb_at(&p, 90, 130), [255, 255, 255], "nothing outside the outline");
    assert_eq!(rgb_at(&p, 150, 130), [255, 255, 255], "the inside of the box is left alone");
    assert!(close(rgb_at(&p, 36, 40), [220, 0, 0]), "label chip: {:?}", rgb_at(&p, 36, 40));
    assert!(close(rgb_at(&p, 330, 250), [0, 0, 220]), "trail head: {:?}", rgb_at(&p, 330, 250));
    let text_pixels =
        (36..80).flat_map(|x| (38..56).map(move |y| (x, y))).filter(|(x, y)| rgb_at(&p, *x, *y)[1] > 200).count();
    assert!(text_pixels > 20, "the name is written in white on the chip: {text_pixels}");
}

#[test]
fn link_icons_and_the_text_selection_show() {
    need_gpu!();
    let o = Overlay {
        link_icons: vec![LinkMark { rect: kurbo::Rect::new(100.0, 100.0, 112.0, 112.0), to_element: false }],
        text_edit: Some(TextEdit {
            to_world: kurbo::Affine::translate((200.0, 100.0)),
            caret: Some(kurbo::Rect::new(60.0, 0.0, 60.0, 20.0)),
            selection: vec![kurbo::Rect::new(0.0, 0.0, 40.0, 20.0)],
            composition: Vec::new(),
            caret_color: peniko::Color::BLACK,
        }),
        ..Overlay::default()
    };
    let p = overlay_only(&o);
    // The tile is white with a bluish border and glyph: some pixels inside the 12 px square are not white.
    let ink = (100..112)
        .flat_map(|x| (100..112).map(move |y| (x, y)))
        .filter(|(x, y)| rgb_at(&p, *x, *y) != [255, 255, 255])
        .count();
    assert!(ink > 15, "{ink}");
    assert_eq!(rgb_at(&p, 90, 90), [255, 255, 255]);
    // The selection tints the paper with the accent colour; the caret is black.
    let tint = rgb_at(&p, 220, 110);
    assert!(tint[2] > tint[0] + 40 && tint[0] < 255, "{tint:?}");
    // 1.5 px wide, centred on x = 260: the two pixels beside it are three quarters covered.
    assert!(rgb_at(&p, 260, 110)[0] < 100, "caret: {:?}", rgb_at(&p, 260, 110));
    assert_eq!(rgb_at(&p, 300, 110), [255, 255, 255]);
}

/// The rows and columns that hold ink in the region `x0..x1`, `y0..y1`: (min x, max x, min y, max y).
fn ink_bbox(p: &Pixels, x0: u32, x1: u32, y0: u32, y1: u32) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in y0..y1 {
        for x in x0..x1 {
            if rgb_at(p, x, y)[0] < 160 {
                b = Some(match b {
                    None => (x, x, y, y),
                    Some((a, c, d, e)) => (a.min(x), c.max(x), d.min(y), e.max(y)),
                });
            }
        }
    }
    b
}

#[test]
fn a_frame_label_is_as_big_on_screen_at_every_zoom() {
    need_gpu!();
    let mut ts = Typesetter::new();
    let mut f = frame_el("f", &ZIndex::first(), 40.0, 40.0, 400.0, 300.0);
    f.kind = ElementKind::Frame(FrameData { name: Some("Plan".into()), clip: true });
    f.style.stroke_color = Color::rgb(0, 0, 0);
    let store = store_of(vec![f]);
    let index = build_paint_index(&store);
    let mut sizes = Vec::new();
    for zoom in [0.5, 1.0, 2.0] {
        let view = View::new(zoom, Vec2::ZERO, Size::new(1000.0, 800.0));
        let mut composer = SceneComposer::new();
        let mut scene = Scene::new();
        composer.compose(&store, &index, &view, &mut ts, &mut scene);
        let p = render(&scene, 1000, 800, peniko::Color::WHITE);
        let top = (40.0 * zoom) as u32;
        // The label is the ink above the top edge of the frame (rows 0..top-3), starting at its left edge.
        let (x0, x1, y0, y1) = ink_bbox(&p, 0, 1000, 0, top - 2).unwrap();
        assert!(y1 < top, "above the frame: zoom {zoom}");
        assert!(x0 + 2 >= top && x0 <= top + 4, "starts at the left edge ({x0} vs {top}) at zoom {zoom}");
        sizes.push((x1 - x0, y1 - y0));
    }
    let (w0, h0) = sizes[0];
    for (w, h) in &sizes {
        assert!(w.abs_diff(w0) <= 2 && h.abs_diff(h0) <= 2, "{sizes:?}");
    }
    assert!(h0 >= 8 && w0 >= 20, "a readable label: {sizes:?}");
}

// ----------------------------------------------------------------------------------------------
// Glyph fallback

/// How many pixels of `p` are ink (they differ from white by more than the tolerance).
fn ink_pixels(p: &Pixels) -> usize {
    p.rgba.chunks_exact(4).filter(|px| (0..3).any(|c| px[c] < 255 - 60)).count()
}

#[test]
fn characters_of_the_fallback_fonts_are_drawn_and_are_not_the_box_of_a_missing_glyph() {
    need_gpu!();
    let mut ts = Typesetter::new();
    let (w, h) = (120, 90);
    let mut draw = |text: &str| {
        let e = inline_math_element(&mut ts, text);
        render_text_element(e, Theme::Light, w, h)
    };
    // U+E000 is in no font: the first font of the stack draws its box of a missing glyph.
    let tofu = draw("\u{E000}");
    assert!(ink_pixels(&tofu) > 20, "the box of a missing glyph is ink");
    for text in ["ಠ", "◕", "｡", "ツ", "→", "٩"] {
        let px = draw(text);
        let ink = ink_pixels(&px);
        assert!(ink > 20, "{text:?} is drawn: {ink} ink pixels");
        let d = diff(&px, &tofu);
        assert!(d.any * f64::from(w * h) > ink as f64 * 0.3, "{text:?} is not the box of a missing glyph: {d:?}");
    }
}

// ----------------------------------------------------------------------------------------------
// Inline math

/// Renders one text element at zoom 1 on white (or the dark background) and returns the pixels.
fn render_text_element(e: Element, theme: Theme, w: u32, h: u32) -> Pixels {
    let mut ts = Typesetter::new();
    let store = store_of(vec![e]);
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::new();
    composer.set_options(RenderOptions { theme, lod: LodConfig::OFF });
    let mut scene = Scene::new();
    let view = View::new(1.0, Vec2::ZERO, Size::new(f64::from(w), f64::from(h)));
    composer.compose(&store, &index, &view, &mut ts, &mut scene);
    render(&scene, w, h, composer.background(&store))
}

/// Rows `y0..y1` of `p` that have a pixel differing from `background` by more than the tolerance, as `(first, last)`.
fn ink_rows(p: &Pixels, background: [u8; 3], x0: u32, x1: u32) -> Option<(u32, u32)> {
    let dark = |x: u32, y: u32| {
        let i = ((y * p.w + x) * 4) as usize;
        (0..3).any(|c| p.rgba[i + c].abs_diff(background[c]) > 60)
    };
    let rows: Vec<u32> = (0..p.h).filter(|&y| (x0..x1.min(p.w)).any(|x| dark(x, y))).collect();
    Some((*rows.first()?, *rows.last()?))
}

fn ink_cols(p: &Pixels, background: [u8; 3]) -> Option<(u32, u32)> {
    let dark = |x: u32, y: u32| {
        let i = ((y * p.w + x) * 4) as usize;
        (0..3).any(|c| p.rgba[i + c].abs_diff(background[c]) > 60)
    };
    let cols: Vec<u32> = (0..p.w).filter(|&x| (0..p.h).any(|y| dark(x, y))).collect();
    Some((*cols.first()?, *cols.last()?))
}

fn inline_math_element(ts: &mut Typesetter, content: &str) -> Element {
    let mut e = text_el(ts, "t", &ZIndex::first(), 10.0, 10.0, content);
    e.style.stroke_color = Color::BLACK;
    if let ElementKind::Text(t) = &mut e.kind {
        t.font.size = 32.0;
    }
    measure_text(ts, &mut e);
    e
}

#[test]
fn inline_math_draws_ink_where_the_layout_put_the_formula() {
    need_gpu!();
    let mut ts = Typesetter::new();
    // The same text without the formula, and with it. The fraction is taller than the line, so the ink of the
    // second reaches above and below the ink of the first, and the text after the formula is shifted right.
    let plain = inline_math_element(&mut ts, "ab  cd");
    let math = inline_math_element(&mut ts, "ab $\\frac{x}{y}$ cd");
    let (w, h) = (260, 120);
    let (a, b) =
        (render_text_element(plain.clone(), Theme::Light, w, h), render_text_element(math.clone(), Theme::Light, w, h));
    let white = [255, 255, 255];
    let (ra, rb) = (ink_rows(&a, white, 0, w).unwrap(), ink_rows(&b, white, 0, w).unwrap());
    assert!(rb.1 - rb.0 > ra.1 - ra.0 + 8, "the fraction makes the ink taller: {ra:?} {rb:?}");
    let (ca, cb) = (ink_cols(&a, white).unwrap(), ink_cols(&b, white).unwrap());
    assert!(cb.1 > ca.1 + 6, "the formula is wider than the gap it replaces: {ca:?} {cb:?}");
    // The formula's ink is inside the box the layout reserved for it.
    let l = ts.text(&drafftink_render::text_spec(
        match &math.kind {
            ElementKind::Text(t) => t,
            _ => unreachable!(),
        },
        math.style.stroke_color,
        math.transform.w,
    ));
    let f = &l.formulas[0];
    let (x0, x1) = (f.rect.x0 as u32 + 10, f.rect.x1 as u32 + 10);
    let inside = ink_rows(&b, white, x0 + 1, x1.saturating_sub(1)).unwrap();
    assert!(inside.0 + 2 >= (f.rect.y0 as u32 + 10).saturating_sub(2), "{inside:?} {:?}", f.rect);
    assert!(inside.1 <= f.rect.y1 as u32 + 10 + 2, "{inside:?} {:?}", f.rect);
    // No ink between the text before and the formula's box, or in the gap after it: the zero-width placeholder
    // draws nothing.
    assert!(ink_rows(&b, white, x1 + 1, x1 + 4).is_none() || f.rect.width() > 0.0);
}

#[test]
fn inline_math_in_the_dark_theme_is_light_ink_on_dark() {
    need_gpu!();
    let mut ts = Typesetter::new();
    let e = inline_math_element(&mut ts, "ab $\\frac{x}{y}$ cd");
    let px = render_text_element(e, Theme::Dark, 260, 120);
    let bg = [18, 18, 18];
    // Every ink pixel is lighter than the background, and there is ink in the formula's rows.
    let lighter = px.rgba.chunks_exact(4).filter(|p| p[0] > 150 && p[1] > 150 && p[2] > 150).count();
    let darker = px.rgba.chunks_exact(4).filter(|p| p[0] < 10).count();
    assert!(lighter > 200, "{lighter}");
    assert_eq!(darker, 0, "nothing is black on the dark background");
    assert!(ink_rows(&px, bg, 0, 260).is_some());
}

#[test]
fn a_broken_formula_shows_its_source_in_red() {
    need_gpu!();
    let mut ts = Typesetter::new();
    let e = inline_math_element(&mut ts, "ab $\\nope{$ cd");
    let px = render_text_element(e, Theme::Light, 300, 80);
    let red = px.rgba.chunks_exact(4).filter(|p| p[0] > 180 && p[1] < 110 && p[2] < 110).count();
    assert!(red > 100, "the source is drawn in the warning colour: {red} red pixels");
}

// ----------------------------------------------------------------------------------------------
// The crop editor's dimming

/// A pale picture of 200x100 pixels, rotated, mirrored, in a store with its file.
fn crop_picture() -> (MemoryStore, Element) {
    let img = image::RgbaImage::from_pixel(200, 100, image::Rgba([250, 230, 120, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let (file, entry) = FileEntry::new("image/png", bytes.into_inner());
    let mut e = Element::new(
        id("pic"),
        ElementKind::Image(ImageData { file_id: Some(file), natural_width: 200, natural_height: 100, crop: None }),
        ZIndex::first(),
    );
    e.transform = Transform { angle: 0.4, flip_x: true, ..Transform::new(100.0, 60.0, 200.0, 100.0) };
    e.style.stroke_width = 0.0;
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    tx.put_file(entry).unwrap();
    tx.commit().unwrap();
    (store, e)
}

fn luma(p: [u8; 3]) -> f64 {
    0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])
}

#[test]
fn the_crop_editor_darkens_the_picture_outside_the_window_in_both_themes() {
    need_gpu!();
    let (store, pic) = crop_picture();
    let picture = drafftink_geom::OrientedBox::from_xywh(100.0, 60.0, 200.0, 100.0, 0.4);
    // The window covers the left 80 units of the picture's own frame; it turns with the picture.
    let window = drafftink_geom::OrientedBox::new(
        picture.to_world(Point::new(-60.0, 0.0)),
        Size::new(80.0, 100.0),
        picture.angle,
    );
    let at = |x: f64, y: f64| {
        let w = picture.to_world(Point::new(x, y));
        (w.x.round() as u32, w.y.round() as u32)
    };
    let view = View::new(1.0, Vec2::ZERO, Size::new(400.0, 300.0));
    for theme in [Theme::Light, Theme::Dark] {
        let mut ts = Typesetter::new();
        let mut composer = SceneComposer::new();
        composer.set_options(RenderOptions { theme, ..RenderOptions::default() });
        let base = composer.background(&store);
        let mut whole = Scene::new();
        composer.compose_transient(&store, std::slice::from_ref(&pic), &view, &mut ts, &mut whole);
        // The same picture drawn normally.
        let normal = render(&whole, 400, 300, base);
        // The editor: the dimmed picture, the scrim, the cropped picture over it (here the whole one stands in).
        let mut dimmed = Scene::new();
        paint_crop_dim(&mut dimmed, &view, theme, &whole, &picture, &window);
        dimmed.append(&whole, None);
        let editor = render(&dimmed, 400, 300, base);
        let mut outside_only = Scene::new();
        paint_crop_dim(&mut outside_only, &view, theme, &whole, &picture, &window);
        let dimmed_px = render(&outside_only, 400, 300, base);

        let (ox, oy) = at(60.0, 0.0);
        let (ix, iy) = at(-60.0, 0.0);
        let plain_out = luma(rgb_at(&normal, ox, oy));
        let dim_out = luma(rgb_at(&dimmed_px, ox, oy));
        assert!(plain_out > 100.0, "{theme:?}: the picture is visible when drawn normally: {plain_out}");
        assert!(dim_out < plain_out * 0.55, "{theme:?}: outside the window {dim_out} against {plain_out}");
        assert!(plain_out - dim_out > 70.0, "{theme:?}: a clear margin, {dim_out} against {plain_out}");
        // The window keeps the picture as it is drawn normally.
        assert!(close(rgb_at(&editor, ix, iy), rgb_at(&normal, ix, iy)), "{theme:?}: inside the window");
        // The canvas around the picture is not dimmed.
        assert_eq!(rgb_at(&dimmed_px, 5, 5), rgb_at(&normal, 5, 5), "{theme:?}: the canvas");
        assert_eq!(rgb_at(&dimmed_px, 390, 290), rgb_at(&normal, 390, 290), "{theme:?}: the canvas");
    }
}

// ----------------------------------------------------------------------------------------------
// The crop window: brackets, bars, grid

/// The window of the chrome tests: 200 by 120 with its top-left corner at (100, 100), on a 400 by 300 canvas.
fn chrome_window() -> drafftink_geom::OrientedBox {
    drafftink_geom::OrientedBox::from_xywh(100.0, 100.0, 200.0, 120.0, 0.0)
}

/// The window's chrome alone on a canvas of colour `base`.
fn chrome_on(base: peniko::Color, theme: Theme, grid: bool, size: CropHandleSize) -> Pixels {
    let view = View::new(1.0, Vec2::ZERO, Size::new(400.0, 300.0));
    let mut scene = Scene::new();
    let window = chrome_window();
    paint_crop_chrome(&mut scene, &view, theme, &window, &window, grid, size);
    render(&scene, 400, 300, base)
}

#[test]
fn the_crop_brackets_read_on_a_white_and_on_a_black_picture_in_both_themes() {
    need_gpu!();
    for theme in [Theme::Light, Theme::Dark] {
        for (name, base) in [("white", peniko::Color::WHITE), ("black", peniko::Color::BLACK)] {
            let px = chrome_on(base, theme, false, CropHandleSize::Mouse);
            let canvas = rgb_at(&chrome_on(base, theme, false, CropHandleSize::Mouse), 20, 20);
            // The top arm of the top-left bracket: 3 px thick above the window's top edge, so the rows 97 to 99; the
            // halo is the 1.5 px above it. The bar on the top side is centred at x = 200.
            for (what, x) in [("bracket", 108), ("bar", 200)] {
                let ink = rgb_at(&px, x, 98);
                let halo = rgb_at(&px, x, 96);
                assert!(
                    (luma(ink) - luma(halo)).abs() > 100.0,
                    "{theme:?} on {name}: the {what} stands out from its halo ({ink:?} against {halo:?})"
                );
                let best = (luma(ink) - luma(canvas)).abs().max((luma(halo) - luma(canvas)).abs());
                assert!(best > 100.0, "{theme:?} on {name}: the {what} stands out from the picture");
                // The ink is the theme's colour (a dark bracket in the light theme, a light one in the dark theme).
                let want = theme.crop_chrome().ink.components;
                assert!((f64::from(ink[0]) / 255.0 - f64::from(want[0])).abs() < 0.1, "{theme:?}: {ink:?}");
            }
            // Nothing is drawn over the pixels being framed: well inside the window the canvas is untouched.
            assert_eq!(rgb_at(&px, 200, 160), canvas, "{theme:?} on {name}: inside the window");
        }
    }
}

#[test]
fn touch_brackets_reach_further_along_the_edge_than_mouse_brackets() {
    need_gpu!();
    let (mouse, touch) = (
        chrome_on(peniko::Color::WHITE, Theme::Light, false, CropHandleSize::Mouse),
        chrome_on(peniko::Color::WHITE, Theme::Light, false, CropHandleSize::Touch),
    );
    // The arm of a mouse bracket ends at x = 98.5 + 18 = 116.5; a touch bracket's at 98.5 + 28 = 126.5.
    // Row 97 is ink for both sizes and clear of the soft shadow under the window's own outline.
    let row = |p: &Pixels, x| luma(rgb_at(p, x, 97));
    assert!(row(&mouse, 112) < 80.0 && row(&mouse, 122) > 200.0, "mouse: {} {}", row(&mouse, 112), row(&mouse, 122));
    assert!(row(&touch, 112) < 80.0 && row(&touch, 122) < 80.0, "touch: {} {}", row(&touch, 112), row(&touch, 122));
    assert!(row(&touch, 132) > 200.0, "and ends at 126.5");
    // The touch marks are thicker, too: 4 px against 3, so the row above the mouse bracket's ink is ink for touch.
    // Row 96 is half ink for touch (the mark spans 96.5 to 100.5) and halo, which is white here, for the mouse.
    assert!(luma(rgb_at(&touch, 108, 96)) < 180.0 && luma(rgb_at(&mouse, 108, 96)) > 220.0);
}

#[test]
fn the_thirds_grid_is_drawn_only_when_asked() {
    need_gpu!();
    let plain = chrome_on(peniko::Color::BLACK, Theme::Light, false, CropHandleSize::Mouse);
    let gridded = chrome_on(peniko::Color::BLACK, Theme::Light, true, CropHandleSize::Mouse);
    // The vertical lines are a third and two thirds across: x = 100 + 200/3 and 100 + 400/3.
    // A one pixel line at a fractional position covers two pixels, so the brightest of the neighbours is looked at.
    let brightest = |p: &Pixels, x: u32, y: u32| (x - 1..=x + 1).map(|x| luma(rgb_at(p, x, y))).fold(0.0, f64::max);
    for x in [167, 233] {
        assert!(brightest(&plain, x, 160) < 10.0, "no line at {x} without the grid");
        assert!(brightest(&gridded, x, 160) > 150.0, "a line at {x} with the grid");
    }
    // And the horizontal ones at y = 140 and 180.
    let across = |p: &Pixels, y: u32| (y - 1..=y + 1).map(|y| luma(rgb_at(p, 200, y))).fold(0.0, f64::max);
    assert!(across(&gridded, 140) > 90.0 && across(&gridded, 180) > 90.0 && across(&plain, 140) < 10.0);
}

/// With `DRAFFTINK_CROP_SHOTS=<dir>` set, writes pictures of the whole crop editor (the dimmed picture, the window
/// with its brackets, in both themes, with and without the grid, for a mouse and for a finger) to that directory,
/// for a person to look at. Without it the test only checks that the editor draws.
#[test]
fn the_crop_editor_draws_and_can_leave_pictures_for_review() {
    need_gpu!();
    let (store, pic) = crop_picture();
    let picture = drafftink_geom::OrientedBox::from_xywh(100.0, 60.0, 200.0, 100.0, 0.4);
    let window = drafftink_geom::OrientedBox::new(
        picture.to_world(Point::new(-30.0, 0.0)),
        Size::new(120.0, 80.0),
        picture.angle,
    );
    let dir = std::env::var_os("DRAFFTINK_CROP_SHOTS").map(PathBuf::from);
    let view = View::new(1.0, Vec2::ZERO, Size::new(400.0, 300.0));
    for theme in [Theme::Light, Theme::Dark] {
        for (name, grid, size) in [
            ("mouse", false, CropHandleSize::Mouse),
            ("mouse-grid", true, CropHandleSize::Mouse),
            ("touch", false, CropHandleSize::Touch),
        ] {
            let mut composer = SceneComposer::new();
            composer.set_options(RenderOptions { theme, ..RenderOptions::default() });
            let base = composer.background(&store);
            let mut whole = Scene::new();
            composer.compose_transient(&store, std::slice::from_ref(&pic), &view, &mut Typesetter::new(), &mut whole);
            let mut scene = Scene::new();
            paint_crop_dim(&mut scene, &view, theme, &whole, &picture, &window);
            // The cropped picture: the whole one, cut to the window.
            let clip: kurbo::BezPath = {
                let c = window.corners();
                let mut path = kurbo::BezPath::new();
                path.move_to(c[0]);
                c[1..].iter().for_each(|p| path.line_to(*p));
                path.close_path();
                path
            };
            scene.push_layer(
                peniko::Fill::NonZero,
                peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcOver),
                1.0,
                kurbo::Affine::IDENTITY,
                &clip,
            );
            scene.append(&whole, None);
            scene.pop_layer();
            paint_crop_chrome(&mut scene, &view, theme, &window, &picture, grid, size);
            let shot = render(&scene, 400, 300, base);
            // The brackets are there: the top-left corner of the window has ink next to it.
            let corner = window.corners()[0];
            let ink = theme.crop_chrome().ink.components;
            let near_corner = (corner.x as u32).saturating_sub(3)..(corner.x as u32 + 3);
            let found = near_corner.clone().any(|x| {
                (corner.y as u32 - 4..corner.y as u32 + 2).any(|y| {
                    let p = rgb_at(&shot, x, y);
                    (f64::from(p[0]) / 255.0 - f64::from(ink[0])).abs() < 0.15
                        && (f64::from(p[2]) / 255.0 - f64::from(ink[2])).abs() < 0.15
                })
            });
            assert!(found, "{theme:?} {name}: ink near the corner of the window");
            if let Some(dir) = &dir {
                std::fs::create_dir_all(dir).unwrap();
                let png = encode_png(shot.w, shot.h, &shot.rgba).unwrap();
                std::fs::write(
                    dir.join(format!("crop-{}-{name}.png", if theme == Theme::Light { "light" } else { "dark" })),
                    png,
                )
                .unwrap();
            }
        }
    }
}
