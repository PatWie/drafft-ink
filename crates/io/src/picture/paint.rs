//! A painted picture as PNG bytes: the one place the exporters bake paint.
//!
//! SVG and Excalidraw files have no brush strokes, so a picture with paint is written as a new PNG: the stored
//! picture decoded, the strokes laid over it by [`drafftink_scene::paint::bake`] (the function the renderer and
//! the command line call as well, so every output shows the same pixels), and the result encoded again.

use drafftink_scene::{Paint, paint};
use image::{ImageFormat, RgbaImage};

use super::{PictureError, decode, probe};

/// The picture `bytes` (PNG, JPEG or WebP, as stored) with `strokes` painted over it, as PNG bytes.
///
/// `natural` is the size of the stored picture the strokes are in pixels of (`ImageData::natural_width` by
/// `natural_height`). A picture decoded at another size is painted at the matching scale, as the renderer does.
/// The result has the size of the decoded picture and keeps its transparency. Without strokes it is still
/// re-encoded; callers that want the original bytes check [`Paint::is_empty`] first.
///
/// # Errors
///
/// [`PictureError`] when the bytes are not a picture this crate reads, or are too large.
pub fn baked_png(bytes: &[u8], natural: (u32, u32), strokes: &Paint) -> Result<Vec<u8>, PictureError> {
    let probed = probe(bytes)?;
    let rgba = decode(bytes, &probed)?.to_rgba8();
    let (width, height) = rgba.dimensions();
    let scale = (
        if natural.0 > 0 { f64::from(width) / f64::from(natural.0) } else { 1.0 },
        if natural.1 > 0 { f64::from(height) / f64::from(natural.1) } else { 1.0 },
    );
    let pixels = paint::bake(rgba.as_raw(), width, height, scale, strokes.values())
        .ok_or_else(|| PictureError::Unreadable("the picture could not be painted".to_owned()))?;
    let painted = RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| PictureError::Unreadable("the painted picture has the wrong size".to_owned()))?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(painted)
        .write_to(&mut out, ImageFormat::Png)
        .map_err(|e| PictureError::Unreadable(e.to_string()))?;
    Ok(out.into_inner())
}
