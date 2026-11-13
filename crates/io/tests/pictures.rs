#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Pictures through `picture::prepare`: which formats are read, what is stored for them, and what is refused.
//!
//! The fixtures are small and built here: bytes written by hand (so a mistake shared by an encoder and its decoder
//! cannot hide), or by the `image` crate's encoders where the format is too long to write by hand.

use std::io::Cursor;

use drafftink_io::picture::{self, Kind, PictureError};
use image::{ImageFormat, Rgba, RgbaImage};

const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

fn pixels(stored: &picture::Prepared) -> RgbaImage {
    assert_eq!(stored.mime, "image/png", "GIF, BMP and ICO are stored as PNG");
    image::load_from_memory_with_format(&stored.bytes, ImageFormat::Png).unwrap().to_rgba8()
}

fn png_of(img: &RgbaImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

// ---- GIF ----

/// A 1x1 GIF of one pixel, written by hand: a table of black and white, then the picture. The byte at
/// `GIF_TRANSPARENT` is the flags of the graphic control block, where bit 0 makes colour 0 transparent.
const GIF_1X1: &[u8] = &[
    b'G', b'I', b'F', b'8', b'9', b'a', 1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0x21, 0xf9, 4, 0, 0, 0, 0,
    0, 0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, 0x3b,
];
const GIF_TRANSPARENT: usize = 22;

/// A GIF with two frames: the first red, the second blue.
fn gif_two_frames() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        for color in [RED, BLUE] {
            enc.encode_frame(image::Frame::new(RgbaImage::from_pixel(5, 3, color))).unwrap();
        }
    }
    out
}

#[test]
fn a_gif_is_stored_as_a_png_of_its_first_frame() {
    let ready = picture::prepare(gif_two_frames(), true).unwrap();
    assert_eq!((ready.given, ready.natural, ready.shrunk), ((5, 3), (5, 3), false));
    assert!(pixels(&ready).pixels().all(|p| *p == RED), "the first frame is red, the second blue");
}

#[test]
fn a_hand_written_gif_is_read() {
    assert_eq!(picture::sniff(GIF_1X1), Some(Kind::Gif));
    let ready = picture::prepare(GIF_1X1.to_vec(), true).unwrap();
    assert_eq!(ready.natural, (1, 1));
    assert_eq!(pixels(&ready).get_pixel(0, 0).0, [0, 0, 0, 255], "colour 0 of the table is black");
    // The same picture with its transparent colour switched on keeps the transparency.
    let mut clear = GIF_1X1.to_vec();
    clear[GIF_TRANSPARENT] = 1;
    assert_eq!(pixels(&picture::prepare(clear, true).unwrap()).get_pixel(0, 0).0[3], 0);
}

#[test]
fn a_gif_that_claims_a_huge_screen_is_refused_before_it_is_decoded() {
    let mut huge = GIF_1X1.to_vec();
    huge[6..8].copy_from_slice(&60_000u16.to_le_bytes());
    huge[8..10].copy_from_slice(&60_000u16.to_le_bytes());
    assert!(matches!(picture::prepare(huge, true), Err(PictureError::TooManyPixels { .. })));
}

// ---- BMP ----

/// A 24-bit BMP written by hand: `rows` from the top, each a list of `[r, g, b]`.
fn bmp(rows: &[&[[u8; 3]]]) -> Vec<u8> {
    let (w, h) = (rows[0].len(), rows.len());
    let row_bytes = (w * 3).div_ceil(4) * 4;
    let size = 54 + row_bytes * h;
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as u32).to_le_bytes());
    out.extend_from_slice(&(h as u32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    // The rows are stored bottom to top, the colours as blue, green, red.
    for row in rows.iter().rev() {
        let mut bytes: Vec<u8> = row.iter().flat_map(|[r, g, b]| [*b, *g, *r]).collect();
        bytes.resize(row_bytes, 0);
        out.extend_from_slice(&bytes);
    }
    out
}

#[test]
fn a_bmp_is_stored_as_a_png() {
    let red = [255, 0, 0];
    let blue = [0, 0, 255];
    let ready = picture::prepare(bmp(&[&[red, blue, red], &[blue, red, blue]]), true).unwrap();
    assert_eq!((ready.given, ready.natural), ((3, 2), (3, 2)));
    let img = pixels(&ready);
    assert_eq!((img.get_pixel(0, 0), img.get_pixel(1, 0), img.get_pixel(0, 1)), (&RED, &BLUE, &BLUE), "top row first");
}

#[test]
fn bmp_needs_more_than_two_letters() {
    // Text that starts with "BM" is not a picture, and a real BMP is known by its header size too.
    assert_eq!(picture::sniff(b"BMW is a car maker, and this is its story"), None);
    assert_eq!(picture::sniff(&bmp(&[&[[1, 2, 3]]])), Some(Kind::Bmp));
}

#[test]
fn a_bmp_that_claims_a_huge_size_is_refused() {
    let mut huge = bmp(&[&[[1, 2, 3]]]);
    huge[18..22].copy_from_slice(&60_000u32.to_le_bytes());
    huge[22..26].copy_from_slice(&60_000u32.to_le_bytes());
    assert!(matches!(picture::prepare(huge, true), Err(PictureError::TooManyPixels { .. })));
}

// ---- ICO ----

/// An icon file of PNG images. Each entry is `(side, declared bits per pixel, color)`. An entry's data is a PNG of
/// that colour, `side` pixels wide.
fn ico(entries: &[(u32, u16, Rgba<u8>)]) -> Vec<u8> {
    let images: Vec<Vec<u8>> =
        entries.iter().map(|(side, _, color)| png_of(&RgbaImage::from_pixel(*side, *side, *color))).collect();
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len();
    for ((side, bits, _), data) in entries.iter().zip(&images) {
        // A side of 256 is written as 0.
        let byte = u8::try_from(*side).unwrap_or(0);
        out.extend_from_slice(&[byte, byte, 0, 0, 1, 0]);
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    images.iter().for_each(|data| out.extend_from_slice(data));
    out
}

#[test]
fn an_ico_is_stored_as_its_largest_image() {
    // The small image has the deeper colour, which the decoder would prefer. The largest one is wanted.
    let file = ico(&[(16, 32, RED), (48, 8, BLUE), (32, 32, RED)]);
    assert_eq!(picture::sniff(&file), Some(Kind::Ico));
    let ready = picture::prepare(file, true).unwrap();
    assert_eq!((ready.given, ready.natural), ((48, 48), (48, 48)));
    assert!(pixels(&ready).pixels().all(|p| *p == BLUE));
}

#[test]
fn an_ico_with_one_image_and_one_of_256_pixels() {
    let ready = picture::prepare(ico(&[(256, 32, RED)]), true).unwrap();
    assert_eq!(ready.natural, (256, 256));
    let ready = picture::prepare(ico(&[(8, 32, RED)]), true).unwrap();
    assert_eq!(ready.natural, (8, 8));
}

#[test]
fn a_damaged_ico_is_refused_without_panic() {
    let good = ico(&[(16, 32, RED), (32, 32, BLUE)]);
    // The directory promises more entries than the file holds.
    let mut many = good.clone();
    many[4..6].copy_from_slice(&500u16.to_le_bytes());
    assert!(picture::prepare(many, true).is_err());
    // An entry whose data lies outside the file.
    let mut outside = good.clone();
    outside[6 + 16 + 12..6 + 16 + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(picture::prepare(outside, true), Err(PictureError::Unreadable(_))));
    // Cut short in the middle of the image data.
    assert!(picture::prepare(good[..good.len() - 40].to_vec(), true).is_err());
    // No entries at all is not an icon.
    assert!(matches!(picture::prepare(vec![0, 0, 1, 0, 0, 0], true), Err(PictureError::Unsupported { .. })));
}

// ---- all of them ----

#[test]
fn damaged_and_cut_pictures_of_every_format_never_panic() {
    let samples = [
        GIF_1X1.to_vec(),
        gif_two_frames(),
        bmp(&[&[[255, 0, 0], [0, 0, 255]], &[[0, 0, 255], [255, 0, 0]]]),
        ico(&[(16, 32, RED), (24, 32, BLUE)]),
    ];
    for good in &samples {
        for i in 0..good.len() {
            let mut flipped = good.clone();
            flipped[i] ^= 0xff;
            let _ = picture::prepare(flipped, true);
            let _ = picture::prepare(good[..i].to_vec(), true);
        }
    }
}
