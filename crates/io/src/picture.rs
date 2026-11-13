//! Pictures for `image.insert`: what some bytes are, how big, and how they become something to store.
//!
//! Every function treats its input as hostile. A picture is identified by its first bytes (a file extension is
//! never trusted), its size is read from the header without decoding the pixels, and a picture with more than
//! [`MAX_PIXELS`] pixels is refused before anything is allocated for it. Decoding runs with the `image` crate's
//! allocation limit set. Nothing here panics.
//!
//! [`prepare`] is the one entry point for storing a picture. PNG, JPEG and WebP are stored as they are. GIF, BMP
//! and ICO are stored as a PNG made from their first frame (GIF) or their largest image (ICO): the renderer, the
//! exports and Excalidraw all read PNG, and a stored GIF would have to be decoded again at every start. A big
//! picture is then made smaller, see [`prepare`].

mod ico;
mod svg;

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, ImageReader, Limits};

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
    /// The bytes are not a picture format drafft.ink reads.
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
    #[error(
        "the picture is {width} x {height} pixels, and the limit is {MAX_PIXELS} pixels in total and {MAX_SIDE} on a side"
    )]
    TooManyPixels {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
}

/// The picture formats drafft.ink reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// PNG. Stored as it is.
    Png,
    /// JPEG. Stored as it is.
    Jpeg,
    /// WebP. Stored as it is.
    WebP,
    /// GIF. The first frame is stored, as a PNG.
    Gif,
    /// BMP. Stored as a PNG.
    Bmp,
    /// Windows icon. The largest image is stored, as a PNG.
    Ico,
    /// SVG, drawn at a fixed size and stored as a PNG. Only [`prepare`] reads it fully.
    Svg,
}

impl Kind {
    /// The media type of the format.
    #[must_use]
    pub fn mime(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Jpeg => "image/jpeg",
            Kind::WebP => "image/webp",
            Kind::Gif => "image/gif",
            Kind::Bmp => "image/bmp",
            Kind::Ico => "image/x-icon",
            Kind::Svg => "image/svg+xml",
        }
    }

    /// The format of the `image` crate, or `None` for an SVG, which is drawn and not decoded.
    fn format(self) -> Option<ImageFormat> {
        match self {
            Kind::Png => Some(ImageFormat::Png),
            Kind::Jpeg => Some(ImageFormat::Jpeg),
            Kind::WebP => Some(ImageFormat::WebP),
            Kind::Gif => Some(ImageFormat::Gif),
            Kind::Bmp => Some(ImageFormat::Bmp),
            Kind::Ico => Some(ImageFormat::Ico),
            Kind::Svg => None,
        }
    }

    /// Whether the bytes go into the document unchanged.
    fn is_stored_as_given(self) -> bool {
        matches!(self, Kind::Png | Kind::Jpeg | Kind::WebP)
    }

    /// A reader of the `image` crate for `bytes` of this kind.
    fn reader(self, bytes: &[u8]) -> Result<ImageReader<Cursor<&[u8]>>, PictureError> {
        let format =
            self.format().ok_or_else(|| PictureError::Unreadable("an SVG is drawn, not decoded".to_owned()))?;
        Ok(ImageReader::with_format(Cursor::new(bytes), format))
    }
}

/// The format and size of a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    /// The format.
    pub kind: Kind,
    /// Width in pixels, at least 1.
    pub width: u32,
    /// Height in pixels, at least 1.
    pub height: u32,
}

/// The size of a BMP header (`BITMAPCOREHEADER` and the `BITMAPINFOHEADER` family) that follows the 14-byte file
/// header. A two-letter signature alone would take any text that starts with "BM".
const BMP_HEADER_SIZES: [u32; 8] = [12, 16, 40, 52, 56, 64, 108, 124];

/// Names the format of `bytes` from its first bytes, or `None`. It reads no size and decodes nothing, so a
/// damaged picture is still named; [`probe`] and [`prepare`] find the damage.
#[must_use]
pub fn sniff(bytes: &[u8]) -> Option<Kind> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some(Kind::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(Kind::Jpeg)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        Some(Kind::WebP)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Kind::Gif)
    } else if bytes.starts_with(b"BM")
        && bytes
            .get(14..18)
            .and_then(|b| b.try_into().ok())
            .is_some_and(|b| BMP_HEADER_SIZES.contains(&u32::from_le_bytes(b)))
    {
        Some(Kind::Bmp)
    } else if ico::looks_like_ico(bytes) {
        Some(Kind::Ico)
    } else if svg::looks_like_svg(bytes) {
        Some(Kind::Svg)
    } else {
        None
    }
}

fn kind_of(bytes: &[u8]) -> Result<Kind, PictureError> {
    sniff(bytes).ok_or_else(|| {
        let found = if bytes.is_empty() {
            "it is empty".to_owned()
        } else if bytes.starts_with(b"%PDF") {
            "it is a PDF".to_owned()
        } else if bytes.starts_with(&[0x49, 0x49, 0x2a, 0x00]) || bytes.starts_with(&[0x4d, 0x4d, 0x00, 0x2a]) {
            "it is a TIFF".to_owned()
        } else {
            "its content is not a picture format drafft.ink knows".to_owned()
        };
        PictureError::Unsupported { found }
    })
}

/// The bytes the `image` crate reads for `kind`. An ICO is cut down to its largest image first, because the
/// decoder prefers the deepest colour over the biggest size.
fn readable(bytes: &[u8], kind: Kind) -> Result<std::borrow::Cow<'_, [u8]>, PictureError> {
    match kind {
        Kind::Ico => ico::largest(bytes).map(std::borrow::Cow::Owned),
        _ => Ok(std::borrow::Cow::Borrowed(bytes)),
    }
}

/// Identifies a picture and reads its size from the header.
pub fn probe(bytes: &[u8]) -> Result<Probe, PictureError> {
    if bytes.len() > MAX_PICTURE_BYTES {
        return Err(PictureError::TooLarge { size: bytes.len() });
    }
    let kind = kind_of(bytes)?;
    let (width, height) = if kind == Kind::Svg {
        svg::parse(bytes)?.given()
    } else {
        let readable = readable(bytes, kind)?;
        kind.reader(&readable)?.into_dimensions().map_err(|e| PictureError::Unreadable(e.to_string()))?
    };
    if width == 0 || height == 0 {
        return Err(PictureError::Unreadable("the picture has no pixels".to_owned()));
    }
    if width > MAX_SIDE || height > MAX_SIDE || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(PictureError::TooManyPixels { width, height });
    }
    Ok(Probe { kind, width, height })
}

/// Decodes a picture, with the side limits of `probe` and the allocation limit of this module.
fn decode(bytes: &[u8], probe: &Probe) -> Result<DynamicImage, PictureError> {
    let readable = readable(bytes, probe.kind)?;
    let mut reader = probe.kind.reader(&readable)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(probe.width);
    limits.max_image_height = Some(probe.height);
    limits.max_alloc = Some(DECODE_ALLOC);
    reader.limits(limits);
    reader.decode().map_err(|e| PictureError::Unreadable(e.to_string()))
}

/// Decodes the whole picture once, to find damaged pixel data now. [`probe`] reads only the header, so a picture
/// that is cut short or corrupt would otherwise be stored and fail every time it is drawn. The pixels are dropped
/// at once.
fn validate_pixels(bytes: &[u8], probe: &Probe) -> Result<(), PictureError> {
    decode(bytes, probe).map(drop)
}

fn encode_png(img: &DynamicImage) -> Result<Vec<u8>, PictureError> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).map_err(|e| PictureError::Unreadable(e.to_string()))?;
    Ok(out.into_inner())
}

/// A picture that is ready to be stored in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The bytes to store.
    pub bytes: Vec<u8>,
    /// Their media type: `image/png`, `image/jpeg` or `image/webp`.
    pub mime: &'static str,
    /// Width and height in pixels of the stored picture.
    pub natural: (u32, u32),
    /// Width and height in pixels of the picture as it was given. A picture is shown at this size (before the
    /// caller's fit), also when a smaller copy is stored.
    pub given: (u32, u32),
    /// True when a big picture was stored smaller.
    pub shrunk: bool,
    /// Plain-words notes for the person: what in the picture was not kept.
    pub warnings: Vec<String>,
}

/// Checks a picture and makes it storable, in this order:
///
/// 1. [`probe`]: the format, and the size from the header (too many pixels is refused here).
/// 2. GIF, BMP and ICO become a PNG (first frame; largest image).
/// 3. With `downscale`, a picture of more than [`SHRINK_ABOVE_BYTES`] bytes and a side over [`SHRINK_TO_SIDE`]
///    is scaled so the long side is [`SHRINK_TO_SIDE`] and encoded again (JPEG stays JPEG at quality 85; the others
///    become PNG, which keeps transparency). The result is kept only when it has fewer bytes.
/// 4. A picture that was neither converted nor shrunk is decoded once and dropped, to find damaged pixel data now.
pub fn prepare(bytes: Vec<u8>, downscale: bool) -> Result<Prepared, PictureError> {
    if bytes.len() > MAX_PICTURE_BYTES {
        return Err(PictureError::TooLarge { size: bytes.len() });
    }
    if kind_of(&bytes)? == Kind::Svg {
        return Ok(svg::prepare(&svg::parse(&bytes)?)?);
    }
    let probe = probe(&bytes)?;
    let given = (probe.width, probe.height);
    let (bytes, kind, natural) = if probe.kind.is_stored_as_given() {
        (bytes, probe.kind, given)
    } else {
        let png = encode_png(&decode(&bytes, &probe)?)?;
        (png, Kind::Png, given)
    };
    let converted = kind != probe.kind;
    let stored = Probe { kind, width: natural.0, height: natural.1 };
    let shrunk = if downscale { shrink(&bytes, &stored)? } else { None };
    if shrunk.is_none() && !converted {
        validate_pixels(&bytes, &stored)?;
    }
    Ok(match shrunk {
        Some(sh) => Prepared {
            bytes: sh.bytes,
            mime: sh.kind.mime(),
            natural: (sh.width, sh.height),
            given,
            shrunk: true,
            warnings: Vec::new(),
        },
        None => Prepared { bytes, mime: kind.mime(), natural, given, shrunk: false, warnings: Vec::new() },
    })
}

/// A re-encoded picture.
struct Reencoded {
    bytes: Vec<u8>,
    kind: Kind,
    width: u32,
    height: u32,
}

/// Makes a big picture smaller, see [`prepare`]. Returns `None` when the picture is small enough as it is, or when
/// the new bytes would not be smaller. `probe` must be the result of [`probe`] on `bytes`, which are PNG, JPEG or
/// WebP.
fn shrink(bytes: &[u8], probe: &Probe) -> Result<Option<Reencoded>, PictureError> {
    if bytes.len() <= SHRINK_ABOVE_BYTES || probe.width.max(probe.height) <= SHRINK_TO_SIDE {
        return Ok(None);
    }
    let mut reader = probe.kind.reader(bytes)?;
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
    let (out, kind) = if probe.kind == Kind::Jpeg {
        let mut out = Cursor::new(Vec::new());
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
        small.to_rgb8().write_with_encoder(enc).map_err(|e| PictureError::Unreadable(e.to_string()))?;
        (out.into_inner(), Kind::Jpeg)
    } else {
        (encode_png(&small)?, Kind::Png)
    };
    if out.len() >= bytes.len() {
        return Ok(None);
    }
    Ok(Some(Reencoded { bytes: out, kind, width: w, height: h }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x * y) % 256) as u8, 255])
        });
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn probe_reads_the_size_from_the_header() {
        let p = probe(&png(30, 20)).unwrap();
        assert_eq!((p.kind, p.width, p.height), (Kind::Png, 30, 20));
    }

    #[test]
    fn other_formats_and_garbage_are_unsupported_with_a_reason() {
        for (bytes, word) in [
            (b"%PDF-1.4".to_vec(), "PDF"),
            (b"BMW notes about a bike".to_vec(), "not a picture"),
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
    fn damaged_pixel_data_is_found_by_validate_and_not_by_probe() {
        let good = png(40, 40);
        let p = probe(&good).unwrap();
        assert_eq!(validate_pixels(&good, &p), Ok(()));
        // The header is intact, the pixel data is cut short: the size can be read, the picture cannot.
        let cut = &good[..good.len() - 30];
        let p = probe(cut).unwrap();
        assert_eq!((p.width, p.height), (40, 40));
        assert!(matches!(validate_pixels(cut, &p), Err(PictureError::Unreadable(_))));
        // A flipped byte inside the compressed data.
        let mut flipped = good.clone();
        let mid = flipped.len() / 2;
        flipped[mid] ^= 0xff;
        let p = probe(&flipped).unwrap();
        assert!(matches!(validate_pixels(&flipped, &p), Err(PictureError::Unreadable(_))));
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
        assert!(shrink(&b, &p).unwrap().is_none());
        let ready = prepare(b.clone(), true).unwrap();
        assert_eq!((ready.bytes.as_slice(), ready.mime, ready.shrunk), (b.as_slice(), "image/png", false));
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
        let s = prepare(b.clone(), true).unwrap();
        assert_eq!(s.given, (2400, 1200));
        // Scaled noise may or may not be smaller; when it is, the size is as promised.
        if s.shrunk {
            assert_eq!(s.natural, (2048, 1024));
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
