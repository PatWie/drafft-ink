//! Windows icon files: finding the largest image.
//!
//! An `.ico` holds several sizes of one icon. The `image` crate decodes the entry with the deepest colour and
//! only then the biggest, so a 16 px icon in 32 bits would win over a 256 px one in 8 bits. [`largest`] picks the
//! entry with the most pixels (the deeper colour breaks a tie) and writes it into a file of its own, which the
//! decoder then reads as the only choice.

use super::PictureError;

/// Bytes of the file header: reserved, type, count.
const HEADER: usize = 6;
/// Bytes of one directory entry.
const ENTRY: usize = 16;
/// The type field of an icon (cursors have 2).
const TYPE_ICON: [u8; 2] = [1, 0];

/// Where the image data starts in a file made by [`largest`]: right after its one directory entry.
const DATA_OFFSET: u32 = (HEADER + ENTRY) as u32;

/// One directory entry, as far as the choice needs it.
struct Entry<'a> {
    raw: &'a [u8; ENTRY],
}

impl Entry<'_> {
    /// The side in pixels. The byte 0 means 256.
    fn side(byte: u8) -> u32 {
        if byte == 0 { 256 } else { u32::from(byte) }
    }

    fn pixels(&self) -> u32 {
        Self::side(self.raw[0]) * Self::side(self.raw[1])
    }

    fn bits_per_pixel(&self) -> u16 {
        u16::from_le_bytes([self.raw[6], self.raw[7]])
    }

    /// The width, height, colours, planes, bits and size fields: everything before the offset.
    fn fixed_part(&self) -> &[u8] {
        self.raw.split_at(ENTRY - 4).0
    }

    fn range(&self) -> Option<std::ops::Range<usize>> {
        let size = u32::from_le_bytes([self.raw[8], self.raw[9], self.raw[10], self.raw[11]]);
        let offset = u32::from_le_bytes([self.raw[12], self.raw[13], self.raw[14], self.raw[15]]);
        let start = usize::try_from(offset).ok()?;
        Some(start..start.checked_add(usize::try_from(size).ok()?)?)
    }
}

/// Whether `bytes` start like an icon file with at least one image.
pub(super) fn looks_like_ico(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0, 0])
        && bytes.get(2..4) == Some(TYPE_ICON.as_slice())
        && entries(bytes).is_some_and(|e| e.count() > 0)
}

fn entries(bytes: &[u8]) -> Option<impl Iterator<Item = Entry<'_>>> {
    let count = usize::from(u16::from_le_bytes([*bytes.get(4)?, *bytes.get(5)?]));
    let directory = bytes.get(HEADER..HEADER + count * ENTRY)?;
    Some(directory.as_chunks::<ENTRY>().0.iter().map(|raw| Entry { raw }))
}

/// An icon file with only the largest image of `bytes`.
pub(super) fn largest(bytes: &[u8]) -> Result<Vec<u8>, PictureError> {
    let unreadable = |why: &str| PictureError::Unreadable(why.to_owned());
    let entries = entries(bytes).ok_or_else(|| unreadable("the icon directory is cut short"))?;
    let best = entries
        .max_by_key(|e| (e.pixels(), e.bits_per_pixel()))
        .ok_or_else(|| unreadable("the icon holds no image"))?;
    let data =
        best.range().and_then(|r| bytes.get(r)).ok_or_else(|| unreadable("an icon image lies outside the file"))?;
    let mut out = Vec::with_capacity(HEADER + ENTRY + data.len());
    out.extend_from_slice(&[0, 0, TYPE_ICON[0], TYPE_ICON[1], 1, 0]);
    out.extend_from_slice(best.fixed_part());
    out.extend_from_slice(&DATA_OFFSET.to_le_bytes());
    out.extend_from_slice(data);
    Ok(out)
}
