//! A scene inside a PNG text chunk, so an exported PNG can be opened again as a document.
//!
//! PNG rendering lives in the gpu crate. This module only edits chunks of finished PNG bytes:
//!
//! * [`embed_scene`] adds an `iTXt` chunk (UTF-8, zlib-compressed) with the keyword [`KEYWORD`] before the first
//!   `IDAT`, and removes an earlier chunk with that keyword. `iTXt` holds any Unicode text. The old app used `zTXt`,
//!   which is Latin-1 only, so it could not export a document with a character above U+00FF at all.
//! * [`extract_scene`] reads the first chunk with that keyword, whether it is `iTXt`, `zTXt` (the old form; the text
//!   is then Latin-1) or `tEXt`.
//!
//! The payload is the scene's JSON: native (`Document::to_json`) for files from this app, the old
//! `CanvasDocument` JSON for PNGs exported by the old app. [`crate::native::sniff`] tells which.
//! Both functions bound their work: the PNG is limited by [`MAX_PNG_BYTES`], an inflated payload by
//! [`MAX_EMBEDDED_SCENE_BYTES`], and every chunk length is checked against the bytes left.

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

use crate::error::{Expected, FormatName, IoError};
use crate::limits::{MAX_EMBEDDED_SCENE_BYTES, MAX_PNG_BYTES};

/// Keyword of the chunk (the same string the old app used).
pub const KEYWORD: &str = "application/vnd.drafftink+json";

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    /// The whole chunk: length, type, data, CRC.
    raw: &'a [u8],
}

/// Iterates the chunks of a PNG. Stops at the first malformed chunk; the error says where.
fn chunks(png: &[u8]) -> Result<Vec<Chunk<'_>>, IoError> {
    if png.len() > MAX_PNG_BYTES {
        return Err(IoError::Limit { what: "PNG", max: format!("{MAX_PNG_BYTES} bytes") });
    }
    if !png.starts_with(&SIGNATURE) {
        return Err(IoError::WrongFormat { expected: Expected::Png, hint: "the PNG signature is missing".into() });
    }
    let mut out = Vec::new();
    let mut pos = SIGNATURE.len();
    while pos < png.len() {
        let bad = |what: &str| IoError::parse(FormatName::Png, format!("{what} at byte {pos}"));
        let head = png.get(pos..pos + 8).ok_or_else(|| bad("a truncated chunk header"))?;
        let len = usize::try_from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]))
            .map_err(|_| bad("a chunk length"))?;
        let kind = [head[4], head[5], head[6], head[7]];
        let end = pos.checked_add(12).and_then(|p| p.checked_add(len)).ok_or_else(|| bad("a chunk length overflow"))?;
        let raw = png.get(pos..end).ok_or_else(|| bad("a chunk longer than the file"))?;
        let data = raw.get(8..8 + len).ok_or_else(|| bad("a chunk longer than the file"))?;
        out.push(Chunk { kind, data, raw });
        pos = end;
        if kind == *b"IEND" {
            break;
        }
    }
    Ok(out)
}

fn crc(kind: &[u8; 4], data: &[u8]) -> u32 {
    let mut c = flate2::Crc::new();
    c.update(kind);
    c.update(data);
    c.sum()
}

fn deflate(data: &[u8]) -> Result<Vec<u8>, IoError> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).map_err(|e| IoError::invalid(FormatName::Png, e.to_string()))?;
    enc.finish().map_err(|e| IoError::invalid(FormatName::Png, e.to_string()))
}

/// Inflates zlib data, refusing more than `max` bytes of output.
pub(crate) fn inflate_limited(data: &[u8], max: usize) -> Result<Vec<u8>, IoError> {
    let mut out = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX);
    ZlibDecoder::new(data)
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| IoError::parse(FormatName::Png, format!("bad compressed text: {e}")))?;
    if out.len() > max {
        return Err(IoError::Limit { what: "embedded scene", max: format!("{max} bytes") });
    }
    Ok(out)
}

fn inflate(data: &[u8]) -> Result<Vec<u8>, IoError> {
    inflate_limited(data, MAX_EMBEDDED_SCENE_BYTES)
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<(), IoError> {
    let len = u32::try_from(data.len()).map_err(|_| IoError::Limit { what: "PNG chunk", max: "4 GiB".into() })?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc(kind, data).to_be_bytes());
    Ok(())
}

/// Returns `png` with `scene_json` stored in an `iTXt` chunk. An existing chunk with [`KEYWORD`] is replaced.
pub fn embed_scene(png: &[u8], scene_json: &str) -> Result<Vec<u8>, IoError> {
    if scene_json.len() > MAX_EMBEDDED_SCENE_BYTES {
        return Err(IoError::Limit { what: "embedded scene", max: format!("{MAX_EMBEDDED_SCENE_BYTES} bytes") });
    }
    let list = chunks(png)?;
    // keyword, NUL, compression flag 1, method 0, empty language tag, NUL, empty translated keyword, NUL, text.
    let mut data = Vec::with_capacity(KEYWORD.len() + 8 + scene_json.len() / 4);
    data.extend_from_slice(KEYWORD.as_bytes());
    data.extend_from_slice(&[0, 1, 0, 0, 0]);
    data.extend_from_slice(&deflate(scene_json.as_bytes())?);
    let mut out = Vec::with_capacity(png.len() + data.len() + 12);
    out.extend_from_slice(&SIGNATURE);
    let mut placed = false;
    for c in &list {
        if text_chunk_keyword(c).as_deref() == Some(KEYWORD) {
            continue;
        }
        if c.kind == *b"IDAT" && !placed || c.kind == *b"IEND" && !placed {
            write_chunk(&mut out, b"iTXt", &data)?;
            placed = true;
        }
        out.extend_from_slice(c.raw);
    }
    if !placed {
        return Err(IoError::parse(FormatName::Png, "the PNG has no IDAT or IEND chunk"));
    }
    Ok(out)
}

fn text_chunk_keyword(c: &Chunk<'_>) -> Option<String> {
    if !matches!(&c.kind, b"iTXt" | b"zTXt" | b"tEXt") {
        return None;
    }
    let nul = c.data.iter().position(|b| *b == 0)?;
    Some(c.data.get(..nul)?.iter().map(|b| char::from(*b)).collect())
}

/// Reads the scene JSON from the first chunk with [`KEYWORD`]. `Ok(None)` when the PNG has none.
pub fn extract_scene(png: &[u8]) -> Result<Option<String>, IoError> {
    for c in chunks(png)? {
        if text_chunk_keyword(&c).as_deref() != Some(KEYWORD) {
            continue;
        }
        let Some(nul) = c.data.iter().position(|b| *b == 0) else { continue };
        let rest = c.data.get(nul + 1..).unwrap_or(&[]);
        let text = match &c.kind {
            b"tEXt" => latin1(rest),
            b"zTXt" => {
                // compression method byte, then zlib data; the text is Latin-1.
                let z = rest.get(1..).unwrap_or(&[]);
                latin1(&inflate(z)?)
            }
            _ => {
                // iTXt: flag, method, language tag NUL, translated keyword NUL, text (UTF-8).
                let (flag, rest) = (rest.first().copied().unwrap_or(0), rest.get(2..).unwrap_or(&[]));
                let skip_nul = |b: &[u8]| -> Vec<u8> {
                    b.iter().position(|x| *x == 0).and_then(|p| b.get(p + 1..)).unwrap_or(&[]).to_vec()
                };
                let body = skip_nul(&skip_nul(rest));
                let bytes = if flag == 1 { inflate(&body)? } else { body };
                String::from_utf8(bytes)
                    .map_err(|_| IoError::parse(FormatName::Png, "the embedded scene is not UTF-8"))?
            }
        };
        return Ok(Some(text));
    }
    Ok(None)
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| char::from(*b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflation_is_bounded() {
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
        enc.write_all(&vec![b'a'; 4_000_000]).unwrap();
        let z = enc.finish().unwrap();
        assert!(z.len() < 10_000);
        assert!(matches!(inflate_limited(&z, 1_000_000), Err(IoError::Limit { .. })));
        assert_eq!(inflate_limited(&z, 4_000_000).unwrap().len(), 4_000_000);
    }
}
