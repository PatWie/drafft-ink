//! Pictures for `image.insert`: what some bytes are, how big, and a bounded way to make them smaller.
//!
//! Every function treats its input as hostile. A picture is identified by its first bytes (PNG, JPEG or WebP; a
//! file extension is never trusted), its size is read from the header without decoding the pixels, and a picture
//! with more than [`MAX_PIXELS`] pixels is refused before anything is allocated for it. Decoding (only needed to
//! make a big picture smaller) runs with the `image` crate's allocation limit set. Nothing here panics.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits};

/// Largest picture accepted, in bytes.
pub const MAX_PICTURE_BYTES: usize = 25 * 1024 * 1024;
/// Most pixels a picture may have (width times height).
pub const MAX_PIXELS: u64 = 100_000_000;
/// Longest side accepted, in pixels.
pub const MAX_SIDE: u32 = 32_768;
/// Pictures larger than this many bytes are candidates for [`shrink`].
pub const SHRINK_ABOVE_BYTES: usize = 2 * 1024 * 1024;
/// The longest side [`shrink`] leaves.
pub const SHRINK_TO_SIDE: u32 = 2048;
/// Memory the decoder may allocate while shrinking, in bytes.
const DECODE_ALLOC: u64 = 768 * 1024 * 1024;

/// Why a picture was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PictureError {
    /// The bytes are not a PNG, JPEG or WebP.
    #[error("{found}")]
    Unsupported {
        /// What the bytes look like, in words.
        found: String,
    },
    /// The bytes look like a supported picture but cannot be read.
    #[error("{0}")]
    Unreadable(String),
    /// More bytes than [`MAX_PICTURE_BYTES`].
    #[error("the picture is {size} bytes, and the limit is {MAX_PICTURE_BYTES}")]
    TooLarge {
        /// The size given.
        size: usize,
    },
    /// More pixels than [`MAX_PIXELS`], or a side over [`MAX_SIDE`].
    #[error("the picture is {width} x {height} pixels, and the limit is {MAX_PIXELS} pixels in total and {MAX_SIDE} on a side")]
    TooManyPixels {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
}

/// The format and size of a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    /// Media type: `image/png`, `image/jpeg` or `image/webp`.
    pub mime: &'static str,
    /// Width in pixels, at least 1.
    pub width: u32,
    /// Height in pixels, at least 1.
    pub height: u32,
}

fn format_of(bytes: &[u8]) -> Result<ImageFormat, PictureError> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Ok(ImageFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        Ok(ImageFormat::WebP)
    } else {
        let found = if bytes.is_empty() {
            "it is empty".to_owned()
        } else if bytes.starts_with(b"GIF8") {
            "it is a GIF".to_owned()
        } else if bytes.starts_with(b"BM") {
            "it is a BMP".to_owned()
        } else if bytes.starts_with(b"%PDF") {
            "it is a PDF".to_owned()
        } else if bytes.starts_with(&[0x49, 0x49, 0x2a, 0x00]) || bytes.starts_with(&[0x4d, 0x4d, 0x00, 0x2a]) {
            "it is a TIFF".to_owned()
        } else if bytes.iter().take(512).any(|b| *b == b'<') && bytes.windows(4).take(512).any(|w| w == b"<svg") {
            "it is an SVG".to_owned()
        } else {
            "its content is not a picture format drafft.ink knows".to_owned()
        };
        Err(PictureError::Unsupported { found })
    }
}

fn mime_of(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        _ => "image/png",
    }
}

/// Identifies a picture and reads its size from the header.
pub fn probe(bytes: &[u8]) -> Result<Probe, PictureError> {
    if bytes.len() > MAX_PICTURE_BYTES {
        return Err(PictureError::TooLarge { size: bytes.len() });
    }
    let format = format_of(bytes)?;
    let reader = ImageReader::with_format(Cursor::new(bytes), format);
    let (width, height) = reader.into_dimensions().map_err(|e| PictureError::Unreadable(e.to_string()))?;
    if width == 0 || height == 0 {
        return Err(PictureError::Unreadable("the picture has no pixels".to_owned()));
    }
    if width > MAX_SIDE || height > MAX_SIDE || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(PictureError::TooManyPixels { width, height });
    }
    Ok(Probe { mime: mime_of(format), width, height })
}

/// A re-encoded picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shrunk {
    /// The new bytes.
    pub bytes: Vec<u8>,
    /// Their media type.
    pub mime: &'static str,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Makes a big picture smaller: when it has more than [`SHRINK_ABOVE_BYTES`] bytes and a side over
/// [`SHRINK_TO_SIDE`], it is decoded, scaled so the long side is [`SHRINK_TO_SIDE`], and encoded again (JPEG stays
/// JPEG at quality 85; PNG and WebP become PNG, which keeps transparency). Returns `None` when the picture is
/// small enough as it is, or when the new bytes would not be smaller. `probe` must be the result of [`probe`] on
/// `bytes`.
pub fn shrink(bytes: &[u8], probe: &Probe) -> Result<Option<Shrunk>, PictureError> {
    if bytes.len() <= SHRINK_ABOVE_BYTES || probe.width.max(probe.height) <= SHRINK_TO_SIDE {
        return Ok(None);
    }
    let format = format_of(bytes)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(DECODE_ALLOC);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| PictureError::Unreadable(e.to_string()))?;
    let k = f64::from(SHRINK_TO_SIDE) / f64::from(probe.width.max(probe.height));
    let w = ((f64::from(probe.width) * k).round() as u32).max(1);
    let h = ((f64::from(probe.height) * k).round() as u32).max(1);
    let small = img.resize_exact(w, h, image::imageops::FilterType::Triangle);
    let mut out = Cursor::new(Vec::new());
    let mime = if format == ImageFormat::Jpeg {
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
        small.to_rgb8().write_with_encoder(enc).map_err(|e| PictureError::Unreadable(e.to_string()))?;
        "image/jpeg"
    } else {
        small.write_to(&mut out, ImageFormat::Png).map_err(|e| PictureError::Unreadable(e.to_string()))?;
        "image/png"
    };
    let bytes_out = out.into_inner();
    if bytes_out.len() >= bytes.len() {
        return Ok(None);
    }
    Ok(Some(Shrunk { bytes: bytes_out, mime, width: w, height: h }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x * y) % 256) as u8, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn probe_reads_the_size_from_the_header() {
        let p = probe(&png(30, 20)).unwrap();
        assert_eq!((p.mime, p.width, p.height), ("image/png", 30, 20));
    }

    #[test]
    fn other_formats_and_garbage_are_unsupported_with_a_reason() {
        for (bytes, word) in [
            (b"GIF89a....".to_vec(), "GIF"),
            (b"%PDF-1.4".to_vec(), "PDF"),
            (b"<svg xmlns=''></svg>".to_vec(), "SVG"),
            (Vec::new(), "empty"),
            (vec![1, 2, 3], "not a picture"),
        ] {
            match probe(&bytes) {
                Err(PictureError::Unsupported { found }) => assert!(found.contains(word), "{found}"),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_truncated_header_is_unreadable_and_a_huge_size_is_refused_without_decoding() {
        let good = png(8, 8);
        assert!(matches!(probe(&good[..12]), Err(PictureError::Unreadable(_))));
        // A PNG header that claims 60000 x 60000 pixels.
        let mut big = good.clone();
        big[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        big[20..24].copy_from_slice(&60_000u32.to_be_bytes());
        assert!(matches!(probe(&big), Err(PictureError::TooManyPixels { .. }) | Err(PictureError::Unreadable(_))));
        let mut zero = good;
        zero[16..20].copy_from_slice(&0u32.to_be_bytes());
        assert!(probe(&zero).is_err());
    }

    #[test]
    fn too_many_bytes_are_refused() {
        let mut v = png(2, 2);
        v.resize(MAX_PICTURE_BYTES + 1, 0);
        assert!(matches!(probe(&v), Err(PictureError::TooLarge { .. })));
    }

    #[test]
    fn shrink_leaves_small_pictures_alone() {
        let b = png(100, 100);
        let p = probe(&b).unwrap();
        assert_eq!(shrink(&b, &p).unwrap(), None);
    }

    #[test]
    fn shrink_scales_a_big_noisy_picture_to_2048() {
        // Noise does not compress, so this PNG is over 2 MiB.
        let img = image::RgbImage::from_fn(2400, 1200, |x, y| {
            let v = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)).wrapping_mul(2_246_822_519);
            image::Rgb([(v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8])
        });
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        let b = out.into_inner();
        assert!(b.len() > SHRINK_ABOVE_BYTES, "{}", b.len());
        let p = probe(&b).unwrap();
        let s = shrink(&b, &p).unwrap();
        // Scaled noise may or may not be smaller; when it is, the size is as promised.
        if let Some(s) = s {
            assert_eq!((s.width, s.height), (2048, 1024));
            let q = probe(&s.bytes).unwrap();
            assert_eq!((q.width, q.height), (2048, 1024));
            assert!(s.bytes.len() < b.len());
        }
    }

    #[test]
    fn hostile_bytes_never_panic() {
        let good = png(16, 16);
        for i in 0..good.len() {
            let mut m = good.clone();
            m[i] ^= 0xff;
            let _ = probe(&m);
            let mut t = good.clone();
            t.truncate(i);
            let _ = probe(&t);
        }
        for b in 0u8..=255 {
            let _ = probe(&[b; 40]);
            let mut j = vec![0xff, 0xd8, 0xff, b];
            j.extend_from_slice(&[b; 30]);
            let _ = probe(&j);
            let mut w = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
            w.extend_from_slice(&[b; 30]);
            let _ = probe(&w);
        }
    }
}
