//! Writing the pictures: lossless, as small as the `png` crate can make them.

use png::{BitDepth, ColorType, Compression, Encoder, Filter};

/// Why a picture could not be encoded or decoded.
#[derive(Debug, thiserror::Error)]
pub enum PngError {
    #[error("cannot encode the PNG: {0}")]
    Encode(#[from] png::EncodingError),
    #[error("cannot read the PNG the export made: {0}")]
    Decode(#[from] png::DecodingError),
    #[error("the picture has {0} bytes per pixel and this tool handles RGB and RGBA only")]
    Format(usize),
}

/// An 8-bit RGB PNG of an opaque RGBA image: the best compression and the adaptive filter. Screenshots and
/// exports are opaque, so the alpha channel is dropped.
pub fn encode_opaque(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, PngError> {
    let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| p.iter().take(3).copied()).collect();
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out, width, height);
    encoder.set_color(ColorType::Rgb);
    encoder.set_depth(BitDepth::Eight);
    encoder.set_compression(Compression::High);
    encoder.set_filter(Filter::Adaptive);
    encoder.write_header()?.write_image_data(&rgb)?;
    Ok(out)
}

/// Writes a PNG again with [`encode_opaque`]'s settings. Used for the picture the export path makes.
pub fn recompress(png: &[u8]) -> Result<Vec<u8>, PngError> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or_default()];
    let info = reader.next_frame(&mut buf)?;
    let bytes = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        ColorType::Rgba => bytes.to_vec(),
        ColorType::Rgb => bytes.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        other => return Err(PngError::Format(other.samples())),
    };
    encode_opaque(info.width, info.height, &rgba)
}
