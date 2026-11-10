//! render-gen <input.json> <output.png> [--scale N]
//!
//! Renders a native DrafftInk JSON document with the OLD renderer
//! (drafftink-render / Vello), offscreen, using the same path as the old app's
//! "Export PNG": `VelloRenderer::build_export_scene` (20px padding, white
//! background, fit to content) and the same texture/AaConfig::Area render params.
//! No scene JSON is embedded in the output PNG, so output bytes are reproducible.
use drafftink_core::canvas::CanvasDocument;
use drafftink_render::VelloRenderer;
use peniko::Color;
use vello::{AaConfig, RenderParams, Renderer, RendererOptions, wgpu};

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut scale: f64 = 1.0; // the old app default is 2; 1 keeps fixture PNGs small
    while let Some(a) = args.next() {
        if a == "--scale" {
            let v = args.next().ok_or("--scale needs a value")?;
            scale = v.parse().map_err(|e| format!("bad scale {v}: {e}"))?;
        } else {
            positional.push(a);
        }
    }
    let [input, output] = positional.as_slice() else {
        return Err("usage: render-gen <input.json> <output.png> [--scale N]".into());
    };
    let json = std::fs::read_to_string(input).map_err(|e| format!("read {input}: {e}"))?;
    let doc = CanvasDocument::from_json(&json).map_err(|e| format!("parse {input}: {e}"))?;

    let mut shape_renderer = VelloRenderer::new();
    let (scene, bounds) = shape_renderer.build_export_scene(&doc, scale);
    let bounds = bounds.ok_or("document is empty")?;
    let (width, height) = (bounds.width().ceil() as u32, bounds.height().ceil() as u32);

    // Headless device: no surface. WGPU_ADAPTER_NAME / WGPU_BACKEND select the adapter.
    let mut cx = vello::util::RenderContext::new();
    let dev_id = pollster::block_on(cx.device(None)).ok_or("no wgpu device")?;
    let handle = &cx.devices[dev_id];
    let info = handle.adapter().get_info();
    eprintln!("adapter: {} ({:?}, {:?}, driver {} {})", info.name, info.device_type, info.backend, info.driver, info.driver_info);
    let (device, queue) = (&handle.device, &handle.queue);

    let mut renderer = Renderer::new(device, RendererOptions::default()).map_err(|e| format!("{e:?}"))?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("png export texture"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let params = RenderParams { base_color: Color::WHITE, width, height, antialiasing_method: AaConfig::Area };
    renderer.render_to_texture(device, queue, &scene, &view, &params).map_err(|e| format!("{e:?}"))?;

    let bytes_per_row = (width * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("png readback buffer"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("png copy encoder") });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) } },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit(std::iter::once(enc.finish()));
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).ok();
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("map: {e:?}"))?;
    let data = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let s = (row * bytes_per_row) as usize;
        rgba.extend_from_slice(&data[s..s + (width * 4) as usize]);
    }
    drop(data);
    buffer.unmap();

    let file = std::fs::File::create(output).map_err(|e| format!("create {output}: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Best);
    enc.set_adaptive_filter(png::AdaptiveFilterType::Adaptive);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    eprintln!("wrote {output}: {width}x{height} (scale {scale})");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
