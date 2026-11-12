//! The `Renderer` port of the command line.

use drafftink_engine::PortError;
use drafftink_engine::ports::{RenderRequest, Renderer, SceneView};
use drafftink_engine::services::SharedTypesetter;
use drafftink_render::replay::{Detail, replay};
use peniko::ImageAlphaType;
use vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};

use super::canvas::{CpuCanvas, pictures_of};
use super::compose::{Composed, compose};
use super::error::RasterError;
#[cfg(feature = "png-gpu")]
use super::gpu::{GpuError, GpuRaster};

/// Draws pictures: on the GPU when the build has `png-gpu` and the machine has an adapter, else on the CPU. It holds
/// the text engine it shares with the session and, with `png-gpu`, a device opened on the first picture, so it costs
/// nothing until a PNG is asked for.
pub struct PngRenderer {
    typesetter: SharedTypesetter,
    #[cfg(feature = "png-gpu")]
    gpu: std::sync::Mutex<GpuState>,
}

/// Whether a GPU was looked for.
#[cfg(feature = "png-gpu")]
#[derive(Default)]
enum GpuState {
    /// Not looked for yet.
    #[default]
    Unopened,
    /// Looked for and found.
    Open(Box<GpuRaster>),
    /// Looked for, and the machine has none (or the user chose the CPU).
    Absent,
}

/// The environment variable that chooses the CPU even where a GPU exists (`cpu`), for comparing the two.
#[cfg(feature = "png-gpu")]
const RENDERER_VARIABLE: &str = "DRAFFTINK_PNG_RENDERER";

impl PngRenderer {
    /// A renderer that lays text out with `typesetter`, the one the session measures with.
    pub fn new(typesetter: SharedTypesetter) -> Self {
        Self {
            typesetter,
            #[cfg(feature = "png-gpu")]
            gpu: std::sync::Mutex::new(GpuState::default()),
        }
    }

    /// The picture on the GPU, `None` when there is no GPU to draw it on.
    #[cfg(feature = "png-gpu")]
    fn rasterize_on_gpu(&self, composed: &Composed) -> Option<Result<Vec<u8>, GpuError>> {
        let mut state = self.gpu.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*state, GpuState::Unopened) {
            let cpu_only = std::env::var(RENDERER_VARIABLE).is_ok_and(|v| v.eq_ignore_ascii_case("cpu"));
            let raster = if cpu_only { None } else { GpuRaster::open() };
            *state = raster.map_or(GpuState::Absent, |r| GpuState::Open(Box::new(r)));
        }
        match &mut *state {
            GpuState::Open(raster) if raster.fits(composed.size) => Some(raster.rasterize(composed)),
            GpuState::Open(_) | GpuState::Unopened | GpuState::Absent => None,
        }
    }
}

impl Renderer for PngRenderer {
    fn render_png(&self, scene: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError> {
        let composed = compose(&self.typesetter, scene, req).map_err(into_port_error)?;
        let (width, height) = (u32::from(composed.size[0]), u32::from(composed.size[1]));
        #[cfg(feature = "png-gpu")]
        let rgba = match self.rasterize_on_gpu(&composed) {
            Some(Ok(drawn)) => drawn,
            Some(Err(e)) => {
                log::warn!("{e}. Drawing on the CPU instead");
                rasterize(&composed)
            }
            None => rasterize(&composed),
        };
        #[cfg(not(feature = "png-gpu"))]
        let rgba = rasterize(&composed);
        drafftink_render::encode_png(width, height, &rgba).map_err(|e| into_port_error(e.into()))
    }
}

fn into_port_error(e: RasterError) -> PortError {
    PortError::Failed(e.to_string())
}

/// The scene as straight-alpha RGBA8, top row first.
fn rasterize(composed: &Composed) -> Vec<u8> {
    let [width, height] = composed.size;
    let mut ctx = RenderContext::new(width, height);
    let mut resources = Resources::new();
    let pictures = pictures_of(&composed.scene);
    let mut canvas = CpuCanvas { ctx: &mut ctx, resources: &mut resources, pictures: &pictures };
    let stats = replay(composed.scene.encoding(), &mut canvas, Detail::Full);
    if stats.skipped > 0 {
        log::debug!("{} draw objects left out of the picture", stats.skipped);
    }
    ctx.flush();
    let mut target = Pixmap::new(width, height);
    let settings =
        RasterizerSettings { target_init: TargetInit::Clear(composed.base), ..RasterizerSettings::default() };
    ctx.render_with(&mut target, &mut resources, settings);
    target.take_rgba8(ImageAlphaType::Alpha)
}

#[cfg(test)]
mod tests {
    use drafftink_engine::{Origin, Session};
    use drafftink_scene::SceneRead;
    use serde_json::{Value, json};

    use crate::ports::{self, IdSeed};

    /// A session with one 120 by 60 rectangle.
    fn session_with_a_rectangle() -> Session {
        let mut s = ports::session(IdSeed::Fixed(1));
        let args = json!({ "kind": "rectangle", "bounds": { "x": 0, "y": 0, "w": 120, "h": 60 } });
        s.execute_from(Origin::Script, "element.create", args).unwrap();
        s
    }

    /// Exports to a file, returns what `doc.export` said and the decoded picture.
    fn export(s: &mut Session, args: Value) -> (Value, png::OutputInfo, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.png");
        let mut args = args;
        args["format"] = json!("png");
        args["to"] = json!({ "path": path.to_str().unwrap() });
        let said = s.execute_from(Origin::Script, "doc.export", args).unwrap();
        let mut reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap())).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        pixels.truncate(info.buffer_size());
        (said, info, pixels)
    }

    fn pixel(info: &png::OutputInfo, pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * info.width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }

    #[test]
    fn the_picture_is_the_content_plus_padding_at_the_scale() {
        let mut s = session_with_a_rectangle();
        let (said, info, _) = export(&mut s, json!({ "scale": 1, "padding": 20 }));
        assert_eq!((info.width, info.height), (160, 100));
        assert_eq!((said["width"].as_u64(), said["height"].as_u64()), (Some(160), Some(100)));
        assert_eq!(info.color_type, png::ColorType::Rgba);
    }

    #[test]
    fn the_background_is_painted_or_left_transparent() {
        let mut s = session_with_a_rectangle();
        let (_, info, pixels) = export(&mut s, json!({ "scale": 1, "background": true }));
        assert_eq!(pixel(&info, &pixels, 1, 1)[3], 255, "opaque corner");
        let (_, info, pixels) = export(&mut s, json!({ "scale": 1, "background": false }));
        assert_eq!(pixel(&info, &pixels, 1, 1), [0, 0, 0, 0], "transparent corner");
    }

    #[test]
    fn the_rectangle_is_drawn_inside_the_padding_only() {
        let mut s = session_with_a_rectangle();
        let (_, info, pixels) = export(&mut s, json!({ "scale": 1, "padding": 20, "background": false }));
        let inked = |x: u32, y: u32| pixel(&info, &pixels, x, y)[3] > 0;
        let inked_rows = (0..info.height).filter(|&y| (0..info.width).any(|x| inked(x, y))).collect::<Vec<_>>();
        let inked_columns = (0..info.width).filter(|&x| (0..info.height).any(|y| inked(x, y))).collect::<Vec<_>>();
        assert!(inked_rows.first().is_some_and(|&y| y >= 15) && inked_rows.last().is_some_and(|&y| y <= 85));
        assert!(inked_columns.first().is_some_and(|&x| x >= 15) && inked_columns.last().is_some_and(|&x| x <= 145));
        assert!(inked_rows.len() > 40 && inked_columns.len() > 100, "the outline spans the rectangle");
    }

    /// A 40 by 30 picture of one flat green, written to a file in `dir`.
    fn green_picture(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("green.png");
        let file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let mut encoder = png::Encoder::new(file, 40, 30);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0u8, 160, 0].repeat(40 * 30)).unwrap();
        writer.finish().unwrap();
        path
    }

    /// The pixels of the one picture in a new session, exported with no padding, after `restyle` ran.
    fn picture_export(restyle: Option<Value>) -> (png::OutputInfo, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ports::session(IdSeed::Fixed(1));
        let source = json!({ "path": green_picture(dir.path()).to_str().unwrap() });
        let out = s.execute_from(Origin::Script, "image.insert", json!({ "images": [source], "at": [0, 0] })).unwrap();
        if let Some(props) = restyle {
            let id = out["ids"][0].clone();
            s.execute_from(Origin::Script, "style.set", json!({ "ids": [id], "props": props })).unwrap();
        }
        let (_, info, pixels) = export(&mut s, json!({ "scale": 1, "padding": 0, "background": false }));
        (info, pixels)
    }

    /// True for a pixel that is neither the picture's green nor clear: the colour of an outline.
    fn is_outline(p: [u8; 4]) -> bool {
        p[3] > 0 && p[..3] != [0, 160, 0]
    }

    #[test]
    fn a_new_picture_has_no_outline_pixels_on_its_edges() {
        let (info, pixels) = picture_export(None);
        assert_eq!((info.width, info.height), (40, 30));
        let edges = (0..info.width).flat_map(|x| [(x, 0), (x, info.height - 1)]);
        let edges = edges.chain((0..info.height).flat_map(|y| [(0, y), (info.width - 1, y)]));
        for (x, y) in edges {
            let p = pixel(&info, &pixels, x, y);
            assert_eq!(p, [0, 160, 0, 255], "the edge pixel ({x}, {y}) is the picture itself, not the default indigo");
        }
        assert!(!(0..info.height).any(|y| (0..info.width).any(|x| is_outline(pixel(&info, &pixels, x, y)))));
    }

    #[test]
    fn a_picture_given_a_stroke_is_drawn_with_that_outline() {
        let (info, pixels) = picture_export(Some(json!({ "stroke_color": "#ff0000", "stroke_width": 4 })));
        // The outline is centred on the edge, so 2 of its 4 units are inside the export box.
        assert_eq!((info.width, info.height), (40, 30));
        let at = |x, y| pixel(&info, &pixels, x, y);
        assert_eq!([at(1, 15), at(20, 1), at(38, 15), at(20, 28)], [[255, 0, 0, 255]; 4]);
        assert_eq!(at(20, 15), [0, 160, 0, 255], "the picture is inside");
        assert_eq!(at(3, 15), [0, 160, 0, 255], "the outline is 2 units deep here");
        // Taking the stroke away again leaves the bare picture.
        let (info, pixels) = picture_export(Some(json!({ "stroke_color": "transparent" })));
        assert!(!(0..info.height).any(|y| (0..info.width).any(|x| is_outline(pixel(&info, &pixels, x, y)))));
    }

    #[test]
    fn the_scene_is_embedded_so_that_the_picture_opens_again() {
        let mut s = session_with_a_rectangle();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.png");
        let to = json!({ "path": path.to_str().unwrap() });
        s.execute_from(Origin::Script, "doc.export", json!({ "format": "png", "to": to })).unwrap();
        let mut other = ports::session(IdSeed::Fixed(2));
        let source = json!({ "path": path.to_str().unwrap() });
        let args = json!({ "source": source, "format": "png", "mode": "replace" });
        other.execute_from(Origin::Script, "doc.open", args).unwrap();
        assert_eq!(other.store().iter().count(), 1);
    }
}
