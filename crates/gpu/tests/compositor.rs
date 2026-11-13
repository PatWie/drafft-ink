//! Headless tests of the device, the Vello + egui compositor and offscreen rendering.
//! They need a Vulkan, Metal or DX12 adapter; on a machine without a GPU, set
//! `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` to use Mesa lavapipe.
//! They never skip: no adapter is a failure.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock, PoisonError, mpsc};
use std::thread;

use drafftink_gpu::{Compositor, EguiFrame, Gpu, GpuOptions, Image};
use drafftink_profile::{Counter, WorkScope};
use vello::Scene;
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Blob, Color, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat};

const RED: Color = Color::from_rgb8(255, 0, 0);
const BASE: Color = Color::from_rgb8(10, 20, 30);

/// Runs `test` on the one thread that owns every Vulkan device of this test binary.
///
/// With several tests creating and destroying devices at once, the NVIDIA Vulkan driver (580) deadlocks
/// about once in 20 runs: a test thread that exits (its driver thread-local destructors) races a
/// `vkDestroyDevice` on another thread, and `cargo test` then hangs for ever (gdb: threads in
/// `libnvidia-eglcore` waiting on each other, none in our code). The tests therefore hand their body to a
/// worker that creates, uses and drops all devices one after the other and never exits. A panic in the
/// body is re-raised in the test that sent it.
fn on_gpu_thread<R: Send + 'static>(test: impl FnOnce() -> R + Send + 'static) -> R {
    type Job = Box<dyn FnOnce() + Send>;
    static WORKER: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("gpu-test-worker".into())
            .spawn(move || rx.iter().for_each(|job| job()))
            .expect("spawn the gpu worker");
        Mutex::new(tx)
    });
    let (done_tx, done_rx) = mpsc::channel();
    let job: Job = Box::new(move || {
        let _ = done_tx.send(catch_unwind(AssertUnwindSafe(test)));
    });
    worker.lock().unwrap_or_else(PoisonError::into_inner).send(job).expect("the gpu worker is running");
    match done_rx.recv().expect("the gpu worker answers") {
        Ok(r) => r,
        Err(panic) => resume_unwind(panic),
    }
}

fn gpu() -> Gpu {
    let _ = env_logger::try_init();
    match Gpu::headless_blocking(&GpuOptions::default()) {
        Ok(g) => g,
        Err(e) => panic!("no GPU for the test: {e}"),
    }
}

fn red_square_scene() -> Scene {
    let mut scene = Scene::new();
    scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(20.0, 20.0, 60.0, 60.0));
    scene
}

fn close(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| x.abs_diff(*y) <= tol)
}

fn render(comp: &mut Compositor, gpu: &Gpu, scene: &Scene, size: [u32; 2]) -> Image {
    match comp.render_to_image_blocking(gpu, size, Some((scene, BASE)), None) {
        Ok(i) => i,
        Err(e) => panic!("render failed: {e}"),
    }
}

#[test]
fn adapter_info_tells_a_gpu_from_a_cpu_rasteriser() {
    on_gpu_thread(|| {
        let gpu = gpu();
        let info = gpu.info();
        println!("adapter under test: {}", info.summary());
        assert!(!info.name.is_empty());
        assert!(info.max_texture_dimension_2d >= 2048);
        // The facts agree with each other.
        // Tests started with the lavapipe ICD must see a software adapter; a real NVIDIA card must not be one.
        if std::env::var("VK_ICD_FILENAMES").is_ok_and(|v| v.contains("lvp_icd")) {
            assert!(info.is_software(), "lavapipe was requested but got {}", info.summary());
        }
        let name = info.name.to_ascii_lowercase();
        if name.contains("llvmpipe") || name.contains("lavapipe") || name.contains("swiftshader") {
            assert!(info.is_software(), "{} must be reported as software", info.name);
        }
        if info.vendor() == "NVIDIA" {
            assert!(!info.is_software());
        }
    });
}

#[test]
fn scene_renders_to_the_expected_pixels() {
    on_gpu_thread(|| {
        let gpu = gpu();
        let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).expect("compositor");
        let img = render(&mut comp, &gpu, &red_square_scene(), [100, 80]);
        assert_eq!((img.width, img.height), (100, 80));
        assert_eq!(img.rgba.len(), 100 * 80 * 4);
        assert!(
            close(img.pixel(40, 40).expect("inside"), [255, 0, 0, 255], 2),
            "inside the square: {:?}",
            img.pixel(40, 40)
        );
        assert!(close(img.pixel(5, 5).expect("outside"), [10, 20, 30, 255], 2), "base colour: {:?}", img.pixel(5, 5));
        assert!(close(img.pixel(90, 70).expect("outside"), [10, 20, 30, 255], 2));
        assert_eq!(img.pixel(100, 0), None);
    });
}

#[test]
fn bgra_target_gives_the_same_rgba_image() {
    on_gpu_thread(|| {
        let gpu = gpu();
        let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Bgra8Unorm).expect("compositor");
        let img = render(&mut comp, &gpu, &red_square_scene(), [100, 80]);
        assert!(
            close(img.pixel(40, 40).expect("inside"), [255, 0, 0, 255], 2),
            "channels swapped back: {:?}",
            img.pixel(40, 40)
        );
    });
}

#[test]
fn egui_is_drawn_over_the_scene_in_one_submit() {
    on_gpu_thread(|| {
        let gpu = gpu();
        let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).expect("compositor");
        let scene = red_square_scene();
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 80.0))),
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            let painter = ui.painter();
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(50.0, 10.0), egui::pos2(90.0, 50.0)),
                0.0,
                egui::Color32::from_rgb(0, 255, 0),
            );
        });
        let mut frame = EguiFrame::from_output(&ctx, output);
        let scope = WorkScope::begin();
        let img =
            comp.render_to_image_blocking(&gpu, [100, 80], Some((&scene, BASE)), Some(&mut frame)).expect("render");
        assert!(close(img.pixel(70, 30).expect("egui"), [0, 255, 0, 255], 2), "egui on top: {:?}", img.pixel(70, 30));
        assert!(close(img.pixel(40, 40).expect("scene"), [255, 0, 0, 255], 2), "scene below: {:?}", img.pixel(40, 40));
        assert!(close(img.pixel(55, 15).expect("overlap"), [0, 255, 0, 255], 2));
        // Vello submits its own buffers (Vello 0.11 API); the composite is exactly one more submit.
        assert_eq!(scope.count(Counter::GpuSubmits), 2);
    });
}

#[test]
fn a_frame_without_a_new_scene_runs_no_vello_and_one_submit() {
    on_gpu_thread(|| {
        let gpu = gpu();
        let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).expect("compositor");
        let first = render(&mut comp, &gpu, &red_square_scene(), [100, 80]);
        let runs = comp.stats().vello_runs;
        let scope = WorkScope::begin();
        let again = comp.render_to_image_blocking(&gpu, [100, 80], None, None).expect("render");
        assert_eq!(comp.stats().vello_runs, runs, "Vello did not run");
        assert_eq!(scope.count(Counter::GpuSubmits), 1);
        assert_eq!(first, again, "the old texture is composited again, pixel for pixel");
    });
}

#[test]
fn no_texture_is_allocated_per_frame() {
    on_gpu_thread(|| {
        let gpu = gpu();
        let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).expect("compositor");
        let scene = red_square_scene();
        for _ in 0..30 {
            let _ = render(&mut comp, &gpu, &scene, [128, 64]);
        }
        let s = comp.stats();
        assert_eq!(s.scene_textures_created, 1, "{s:?}");
        assert_eq!(s.offscreen_targets_created, 1, "{s:?}");
        assert_eq!(s.vello_runs, 30);
        // A new size allocates once more, and only once.
        for _ in 0..5 {
            let _ = render(&mut comp, &gpu, &scene, [200, 100]);
        }
        let s = comp.stats();
        assert_eq!(s.scene_textures_created, 2, "{s:?}");
        assert_eq!(s.offscreen_targets_created, 2, "{s:?}");
        assert_eq!(comp.scene_size(), Some([200, 100]));
    });
}

#[test]
fn a_lost_device_is_detected_and_a_new_one_renders() {
    on_gpu_thread(|| {
        let old = gpu();
        assert!(old.lost().is_none());
        old.destroy_for_test();
        let lost = old.lost().expect("device reported lost");
        assert!(lost.destroyed);
        let fresh = pollster::block_on(old.recover(None, &GpuOptions::default())).expect("recover");
        assert!(fresh.lost().is_none());
        let mut comp = Compositor::new(&fresh, wgpu::TextureFormat::Rgba8Unorm).expect("compositor on the new device");
        let img = render(&mut comp, &fresh, &red_square_scene(), [100, 80]);
        assert!(close(img.pixel(40, 40).expect("inside"), [255, 0, 0, 255], 2));
    });
}

#[test]
fn image_is_tightly_packed_for_widths_that_need_row_padding() {
    on_gpu_thread(|| {
        // 13 px * 4 bytes = 52, not a multiple of 256: the readback pads rows and the image must not.
        let gpu = gpu();
        let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).expect("compositor");
        let mut scene = Scene::new();
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &Rect::new(0.0, 0.0, 13.0, 3.0));
        let img = render(&mut comp, &gpu, &scene, [13, 5]);
        assert_eq!(img.rgba.len(), 13 * 5 * 4);
        for x in 0..13 {
            assert!(close(img.pixel(x, 1).expect("row 1"), [255, 0, 0, 255], 2), "x={x}");
            assert!(close(img.pixel(x, 4).expect("row 4"), [10, 20, 30, 255], 2), "x={x}");
        }
    });
}

#[test]
fn the_renderer_info_agrees_with_the_adapter() {
    on_gpu_thread(|| {
        let gpu = gpu();
        let info = gpu.info().renderer_info();
        assert_eq!(info.adapter, gpu.info().name);
        assert_eq!(info.rasteriser == drafftink_gpu::Rasteriser::Cpu, gpu.info().is_software());
        assert_ne!(info.api, drafftink_gpu::GraphicsApi::Other, "a native adapter runs on a named API");
    });
}

/// The composite pass is timed with timestamp queries when the device has them, and the result arrives without
/// the CPU waiting for it. A device without them says so; it never reports a time of 0.
#[test]
fn the_composite_pass_is_timed_when_the_device_can_and_only_while_asked() {
    use drafftink_gpu::GpuTime;
    on_gpu_thread(|| {
        let gpu = gpu();
        let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).expect("compositor");
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("timing target"),
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut frame = EguiFrame::empty();
        let has_queries = gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY);

        // Off: nothing is written and no result is waited for.
        comp.composite(&gpu, &mut frame, &view, [64, 64]).expect("composite");
        let expected_idle = if has_queries { GpuTime::Pending } else { GpuTime::Unsupported };
        assert_eq!(comp.gpu_time(&gpu), expected_idle);

        comp.set_timing(true);
        let mut last = comp.gpu_time(&gpu);
        for _ in 0..200 {
            comp.composite(&gpu, &mut frame, &view, [64, 64]).expect("composite");
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("the GPU finishes");
            last = comp.gpu_time(&gpu);
            if matches!(last, GpuTime::Measured(_)) {
                break;
            }
        }
        match last {
            GpuTime::Measured(ms) => {
                assert!(has_queries);
                assert!((0.0..1000.0).contains(&ms), "a 64 pixel pass does not take {ms} ms");
            }
            GpuTime::Unsupported => assert!(!has_queries, "a device with timestamp queries must measure"),
            GpuTime::Pending => panic!("no result after 200 passes on a device with timestamp queries"),
        }
        println!("composite pass: {last:?} (timestamp queries: {has_queries})");
    });
}

/// A scene that draws `picture` (one flat colour) over the square (20, 20)..(60, 60), the way the painter does:
/// as an image fill, so Vello keeps the pixels in its image atlas.
fn picture_scene(picture: &ImageData) -> Scene {
    let mut scene = Scene::new();
    let brush = ImageBrush::new(picture.clone());
    scene.fill(Fill::NonZero, Affine::IDENTITY, &brush, None, &Rect::new(20.0, 20.0, 60.0, 60.0));
    scene
}

fn flat_picture(rgba: [u8; 4]) -> ImageData {
    ImageData {
        data: Blob::new(Arc::new(rgba.repeat(8 * 8))),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: 8,
        height: 8,
    }
}

#[test]
fn a_picture_is_drawn_again_after_frames_that_draw_no_picture() {
    on_gpu_thread(|| {
        let gpu = gpu();
        let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).expect("compositor");
        let green = [0, 255, 0, 255];
        let picture = flat_picture(green);
        let shown = |comp: &mut Compositor| render(comp, &gpu, &picture_scene(&picture), [100, 80]).pixel(40, 40);

        assert_eq!(shown(&mut comp), Some(green), "the first frame draws the picture");
        // Panned away: a frame with only plain shapes. Vello drops its image atlas for it.
        let away = render(&mut comp, &gpu, &red_square_scene(), [100, 80]);
        assert!(close(away.pixel(40, 40).expect("inside"), [255, 0, 0, 255], 2));
        for frame in 0..3 {
            let back = shown(&mut comp).expect("inside");
            assert!(close(back, green, 2), "panned back, frame {frame}: {back:?}");
        }
    });
}
