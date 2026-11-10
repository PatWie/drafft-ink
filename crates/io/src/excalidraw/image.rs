//! Pictures: decoding Excalidraw `dataURL`s and reading the pixel size from the first bytes of an encoded image.

use base64::Engine as _;
use base64::alphabet::STANDARD as ALPHABET;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use drafftink_scene::limits::MAX_FILE_BYTES;

/// Standard alphabet, padding optional: some producers strip the `=`.
const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &ALPHABET,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Why a data URL could not be used.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DataUrlError {
    /// Not `data:<mime>;base64,<payload>`.
    NotBase64DataUrl,
    /// The payload decodes to more than the scene's file size limit.
    TooLarge,
    /// The payload is not valid base64.
    BadBase64,
}

impl DataUrlError {
    pub(super) fn describe(&self) -> &'static str {
        match self {
            DataUrlError::NotBase64DataUrl => "it is not a base64 data URL",
            DataUrlError::TooLarge => "it is larger than the picture size limit",
            DataUrlError::BadBase64 => "its base64 data is damaged",
        }
    }
}

/// Splits `data:<mime>;base64,<payload>` and decodes the payload. Returns the media type named in the URL (may be
/// empty) and the bytes.
pub(super) fn decode_data_url(url: &str) -> Result<(String, Vec<u8>), DataUrlError> {
    let rest = url.strip_prefix("data:").ok_or(DataUrlError::NotBase64DataUrl)?;
    let (header, payload) = rest.split_once(',').ok_or(DataUrlError::NotBase64DataUrl)?;
    let mut parts = header.split(';');
    let mime = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    if !parts.any(|p| p.trim().eq_ignore_ascii_case("base64")) {
        return Err(DataUrlError::NotBase64DataUrl);
    }
    // Four characters decode to at most three bytes. Reject before allocating.
    if payload.len() / 4 * 3 > MAX_FILE_BYTES + 3 {
        return Err(DataUrlError::TooLarge);
    }
    let bytes = if payload.bytes().any(|b| b.is_ascii_whitespace()) {
        let clean: String = payload.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        LENIENT.decode(clean.as_bytes())
    } else {
        LENIENT.decode(payload.as_bytes())
    }
    .map_err(|_| DataUrlError::BadBase64)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(DataUrlError::TooLarge);
    }
    Ok((mime, bytes))
}

/// True when the media type passes the scene validator's rule for file entries.
pub(super) fn mime_is_accepted(mime: &str) -> bool {
    mime.len() <= 64
        && mime.starts_with("image/")
        && mime.len() > 6
        && mime.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'+' | b'.' | b'-'))
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?])))
}
fn le16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?])))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, *b.get(at + 3)?]))
}
fn le24(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, 0]))
}

/// The pixel size `(width, height)` of a PNG, JPEG, GIF or WebP, read from the headers. `None` for other formats
/// (SVG has no pixel size) and for damaged headers. Reads at most the first bytes it needs and never panics.
pub(super) fn sniff_size(b: &[u8]) -> Option<(u32, u32)> {
    let (w, h) = if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        (be32(b, 16)?, be32(b, 20)?)
    } else if b.starts_with(b"GIF8") {
        (le16(b, 6)?, le16(b, 8)?)
    } else if b.starts_with(&[0xff, 0xd8]) {
        jpeg_size(b)?
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP".as_slice()) {
        webp_size(b)?
    } else {
        return None;
    };
    (w > 0 && h > 0).then_some((w, h))
}

fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    loop {
        // Find the next marker: 0xFF followed by a non-0xFF, non-zero byte.
        while *b.get(i)? != 0xff {
            i += 1;
        }
        while *b.get(i)? == 0xff {
            i += 1;
        }
        let marker = *b.get(i)?;
        i += 1;
        match marker {
            0x00 | 0x01 | 0xd0..=0xd8 => continue,
            0xd9 | 0xda => return None,
            // Start of frame: every SOFn except DHT (c4), JPG (c8) and DAC (cc).
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                return Some((be16(b, i + 5)?, be16(b, i + 3)?));
            }
            _ => {
                let len = usize::try_from(be16(b, i)?).ok()?;
                if len < 2 {
                    return None;
                }
                i += len;
            }
        }
    }
}

fn webp_size(b: &[u8]) -> Option<(u32, u32)> {
    match b.get(12..16)? {
        b"VP8 " => {
            if b.get(23..26)? != [0x9d, 0x01, 0x2a] {
                return None;
            }
            Some((le16(b, 26)? & 0x3fff, le16(b, 28)? & 0x3fff))
        }
        b"VP8L" => {
            if *b.get(20)? != 0x2f {
                return None;
            }
            let bits = u32::from_le_bytes([*b.get(21)?, *b.get(22)?, *b.get(23)?, *b.get(24)?]);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => Some((le24(b, 24)? + 1, le24(b, 27)? + 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_data_url_and_rejects_the_rest() {
        let (mime, bytes) = decode_data_url("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!((mime.as_str(), bytes.as_slice()), ("image/png", b"hello".as_slice()));
        // Missing padding and embedded whitespace are tolerated.
        assert_eq!(decode_data_url("data:image/png;base64,aGVs bG8").unwrap().1, b"hello");
        assert_eq!(decode_data_url("image/png;base64,aGVsbG8=").unwrap_err(), DataUrlError::NotBase64DataUrl);
        assert_eq!(decode_data_url("data:image/svg+xml;utf8,<svg/>").unwrap_err(), DataUrlError::NotBase64DataUrl);
        assert_eq!(decode_data_url("data:image/png;base64,@@@@").unwrap_err(), DataUrlError::BadBase64);
        assert_eq!(decode_data_url("data:image/png;base64").unwrap_err(), DataUrlError::NotBase64DataUrl);
    }

    #[test]
    fn media_type_rule_matches_the_scene_validator() {
        assert!(mime_is_accepted("image/png"));
        assert!(mime_is_accepted("image/svg+xml"));
        assert!(!mime_is_accepted("image/"));
        assert!(!mime_is_accepted("text/html"));
        assert!(!mime_is_accepted("image/png; charset=x"));
        assert!(!mime_is_accepted(&format!("image/{}", "a".repeat(80))));
    }

    #[test]
    fn reads_sizes_from_headers() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend_from_slice(&7u32.to_be_bytes());
        png.extend_from_slice(&9u32.to_be_bytes());
        assert_eq!(sniff_size(&png), Some((7, 9)));
        let gif = [b'G', b'I', b'F', b'8', b'9', b'a', 5, 0, 6, 0];
        assert_eq!(sniff_size(&gif), Some((5, 6)));
        // SOI, APP0 (length 4), SOF0 for 20 x 10.
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0, 10, 0, 20];
        assert_eq!(sniff_size(&jpeg), Some((20, 10)));
        assert_eq!(sniff_size(b"<svg/>"), None);
        assert_eq!(sniff_size(&[]), None);
        assert_eq!(sniff_size(&png[..18]), None);
    }

    #[test]
    fn truncated_or_hostile_headers_give_none() {
        for len in 0..40 {
            let junk: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
            let _ = sniff_size(&junk);
        }
        let mut jpeg = vec![0xff, 0xd8];
        jpeg.extend(std::iter::repeat_n(0xff, 1000));
        assert_eq!(sniff_size(&jpeg), None);
        let looping = [0xff, 0xd8, 0xff, 0xe0, 0, 1, 0xff, 0xe0, 0, 0];
        assert_eq!(sniff_size(&looping), None);
        assert_eq!(sniff_size(b"RIFF\0\0\0\0WEBPVP8X"), None);
    }
}
