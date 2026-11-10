//! Headless tests of the device, the Vello + egui compositor and offscreen rendering.
//! They need a Vulkan, Metal or DX12 adapter; on a machine without a GPU, set
//! `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` to use Mesa lavapipe.
//! They never skip: no adapter is a failure.

#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use drafftink_gpu::{Compositor, EguiFrame, Gpu, GpuOptions, Image};
use drafftink_profile::{Counter, WorkScope};
use vello::Scene;
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Color, Fill};

const RED: Color = Color::from_rgb8(255, 0, 0);
const BASE: Color = Color::from_rgb8(10, 20, 30);

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
}

#[test]
fn scene_renders_to_the_expected_pixels() {
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
}

#[test]
fn bgra_target_gives_the_same_rgba_image() {
    let gpu = gpu();
    let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Bgra8Unorm).expect("compositor");
    let img = render(&mut comp, &gpu, &red_square_scene(), [100, 80]);
    assert!(
        close(img.pixel(40, 40).expect("inside"), [255, 0, 0, 255], 2),
        "channels swapped back: {:?}",
        img.pixel(40, 40)
    );
}

#[test]
fn egui_is_drawn_over_the_scene_in_one_submit() {
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
    let img = comp.render_to_image_blocking(&gpu, [100, 80], Some((&scene, BASE)), Some(&mut frame)).expect("render");
    assert!(close(img.pixel(70, 30).expect("egui"), [0, 255, 0, 255], 2), "egui on top: {:?}", img.pixel(70, 30));
    assert!(close(img.pixel(40, 40).expect("scene"), [255, 0, 0, 255], 2), "scene below: {:?}", img.pixel(40, 40));
    assert!(close(img.pixel(55, 15).expect("overlap"), [0, 255, 0, 255], 2));
    // Vello submits its own buffers (Vello 0.11 API); the composite is exactly one more submit.
    assert_eq!(scope.count(Counter::GpuSubmits), 2);
}

#[test]
fn a_frame_without_a_new_scene_runs_no_vello_and_one_submit() {
    let gpu = gpu();
    let mut comp = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).expect("compositor");
    let first = render(&mut comp, &gpu, &red_square_scene(), [100, 80]);
    let runs = comp.stats().vello_runs;
    let scope = WorkScope::begin();
    let again = comp.render_to_image_blocking(&gpu, [100, 80], None, None).expect("render");
    assert_eq!(comp.stats().vello_runs, runs, "Vello did not run");
    assert_eq!(scope.count(Counter::GpuSubmits), 1);
    assert_eq!(first, again, "the old texture is composited again, pixel for pixel");
}

#[test]
fn no_texture_is_allocated_per_frame() {
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
}

#[test]
fn a_lost_device_is_detected_and_a_new_one_renders() {
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
}

#[test]
fn image_is_tightly_packed_for_widths_that_need_row_padding() {
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
}
