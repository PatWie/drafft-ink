//! The picture behind PNG export and screenshots: a region of a document as a Vello scene, and the PNG file made of
//! the pixels that a renderer drew from it.
//!
//! The desktop and web apps and the command line all draw exports. They differ in who rasterises the scene (a GPU,
//! the WebGL2 fallback, `vello_cpu`) and in whether the selection is marked, never in what the scene holds, so the
//! scene is made here once ([`compose_export`]) and the file is written here once ([`encode_png`]).

use drafftink_scene::SceneRead;
use drafftink_typeset::Typesetter;
use kurbo::{Size, Vec2};
use peniko::Color;
use vello::Scene;

use crate::{RenderOptions, SceneComposer, Theme, View, build_paint_index};

/// Longest side of a picture, in pixels.
pub const MAX_SIDE: u32 = 16_384;

/// Which part of a document to draw, and how.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportSpec {
    /// The scene region to draw: left, top, width, height.
    pub region: (f64, f64, f64, f64),
    /// The picture's size in pixels. The region is scaled to its width.
    pub size_px: (u32, u32),
    /// Whether to draw the dark theme.
    pub dark: bool,
    /// Whether the picture starts with the document's background colour (else transparent).
    pub background: bool,
}

/// A scene ready to be rasterised.
pub struct Exported {
    /// The elements, in picture pixels.
    pub scene: Scene,
    /// The colour under the scene (transparent when the spec has no background).
    pub base: Color,
    /// Width and height in pixels.
    pub size: [u32; 2],
    /// The view that maps the region to the picture, for drawing more into the same space (the selection marks).
    pub view: View,
}

/// Why a picture could not be made.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The picture has a side that cannot be drawn.
    #[error("the picture would be {width} by {height} pixels; the limit is {max} on a side")]
    Size {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
        /// Longest side that is drawn.
        max: u32,
    },
    /// The pixels could not be written as a PNG.
    #[error("could not encode the PNG: {0}")]
    Encode(#[from] png::EncodingError),
}

/// Encodes the elements of `store` that `spec` asks for. Fails for a size that cannot be drawn, before any work.
pub fn compose_export<S: SceneRead>(
    store: &S,
    spec: &ExportSpec,
    typesetter: &mut Typesetter,
) -> Result<Exported, ExportError> {
    let (width, height) = spec.size_px;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(ExportError::Size { width, height, max: MAX_SIDE });
    }
    let (x, y, region_width, _) = spec.region;
    let zoom = f64::from(width) / region_width.max(f64::EPSILON);
    let view = View::new(zoom, Vec2::new(-x * zoom, -y * zoom), Size::new(f64::from(width), f64::from(height)));

    let mut composer = SceneComposer::new();
    composer.set_options(RenderOptions::for_export(if spec.dark { Theme::Dark } else { Theme::Light }));
    let index = build_paint_index(store);
    let mut scene = Scene::new();
    composer.compose(store, &index, &view, typesetter, &mut scene);
    let base = if spec.background { composer.background(store) } else { Color::TRANSPARENT };
    Ok(Exported { scene, base, size: [width, height], view })
}

/// An 8-bit RGBA PNG of straight-alpha pixels, top row first.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, ExportError> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use drafftink_scene::MemoryStore;

    use super::*;

    fn spec() -> ExportSpec {
        ExportSpec { region: (0.0, 0.0, 10.0, 10.0), size_px: (4, 4), dark: false, background: true }
    }

    #[test]
    fn a_small_picture_is_composed_at_the_size_asked_for() {
        let exported = compose_export(&MemoryStore::new(), &spec(), &mut Typesetter::new()).unwrap();
        assert_eq!(exported.size, [4, 4]);
    }

    #[test]
    fn a_picture_without_background_starts_transparent() {
        let spec = ExportSpec { background: false, ..spec() };
        let exported = compose_export(&MemoryStore::new(), &spec, &mut Typesetter::new()).unwrap();
        assert_eq!(exported.base, Color::TRANSPARENT);
    }

    #[test]
    fn a_side_that_cannot_be_drawn_is_refused_in_plain_words() {
        for size_px in [(0, 5), (5, 0), (MAX_SIDE + 1, 5), (5, MAX_SIDE + 1)] {
            let err = compose_export(&MemoryStore::new(), &ExportSpec { size_px, ..spec() }, &mut Typesetter::new())
                .err()
                .unwrap();
            assert!(matches!(err, ExportError::Size { .. }), "{err}");
            assert!(err.to_string().contains("pixels"), "{err}");
        }
    }

    #[test]
    fn png_has_the_signature_and_the_size() {
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes([png[16], png[17], png[18], png[19]]), 2);
    }
}
