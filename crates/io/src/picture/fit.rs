//! [`fit_into`]: makes a stored picture small enough for a byte budget.

use std::io::Cursor;

use image::DynamicImage;
use image::imageops::FilterType;

use super::{Kind, PictureError, Prepared, SHRINK_TO_SIDE, decode, encode_png, probe};

/// The shortest long side the ladder reaches, in pixels.
const MIN_SIDE: u32 = 256;
/// Each step of the ladder keeps this share of the side before it.
const STEP: f64 = 0.8;
/// JPEG quality of the pictures without transparency.
const JPEG_QUALITY: u8 = 85;

/// Makes `prepared` at most `max_bytes` bytes, and returns it unchanged when it already is.
///
/// A picture that is too big is decoded once and encoded again at smaller sizes: the long side is
/// [`SHRINK_TO_SIDE`] (or the picture's own, when that is smaller), then 0.8 times the side before, down to 256
/// pixels. A picture without transparency becomes a JPEG at quality 85; one with transparency becomes a PNG. The
/// first result within `max_bytes` is returned, with `shrunk` set, `natural` and `mime` of the new bytes, and
/// `given` as it was.
///
/// # Errors
///
/// [`PictureError::TooLarge`] with the size of `prepared` when not even the smallest size fits, and the errors of
/// [`probe`] and of decoding when the bytes cannot be read.
pub fn fit_into(prepared: Prepared, max_bytes: usize) -> Result<Prepared, PictureError> {
    let size = prepared.bytes.len();
    if size <= max_bytes {
        return Ok(prepared);
    }
    let probed = probe(&prepared.bytes)?;
    let img = decode(&prepared.bytes, &probed)?;
    let kind = if has_transparency(&img) { Kind::Png } else { Kind::Jpeg };
    let long = probed.width.max(probed.height);
    for side in ladder(long) {
        let (width, height) = scaled((probed.width, probed.height), side);
        let bytes = if (width, height) == (probed.width, probed.height) {
            encode(&img, kind)?
        } else {
            encode(&img.resize_exact(width, height, FilterType::Triangle), kind)?
        };
        if bytes.len() <= max_bytes {
            return Ok(Prepared { bytes, mime: kind.mime(), natural: (width, height), shrunk: true, ..prepared });
        }
    }
    Err(PictureError::TooLarge { size })
}

/// The long sides to try for a picture whose long side is `long`: at most [`SHRINK_TO_SIDE`], then 0.8 times the
/// one before, down to [`MIN_SIDE`]. A picture shorter than [`MIN_SIDE`] is tried at its own size only.
fn ladder(long: u32) -> impl Iterator<Item = u32> {
    let first = long.min(SHRINK_TO_SIDE);
    std::iter::successors(Some(first), |&side| {
        let next = (f64::from(side) * STEP).floor() as u32;
        (next >= MIN_SIDE).then_some(next)
    })
}

/// `(width, height)` scaled so the longer side is `side`, each at least 1.
fn scaled((width, height): (u32, u32), side: u32) -> (u32, u32) {
    let k = f64::from(side) / f64::from(width.max(height));
    let scale = |n: u32| ((f64::from(n) * k).round() as u32).max(1);
    (scale(width), scale(height))
}

/// Whether any pixel of `img` is not fully opaque.
fn has_transparency(img: &DynamicImage) -> bool {
    img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] < u8::MAX)
}

fn encode(img: &DynamicImage, kind: Kind) -> Result<Vec<u8>, PictureError> {
    match kind {
        Kind::Jpeg => {
            let mut out = Cursor::new(Vec::new());
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
            img.to_rgb8().write_with_encoder(encoder).map_err(|e| PictureError::Unreadable(e.to_string()))?;
            Ok(out.into_inner())
        }
        _ => encode_png(img),
    }
}
