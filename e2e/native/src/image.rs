//! A decoded screenshot and the pixel questions the stories ask of it.

use std::io::Cursor;

use base64::Engine as _;

use crate::error::E2eError;

/// A PNG decoded to RGBA, with counting and sampling that take a tolerance (there is no golden image).
#[derive(Debug, Clone)]
pub struct Screenshot {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    rgba: Vec<u8>,
}

/// An RGB colour.
pub type Rgb = [u8; 3];

impl Screenshot {
    /// Decodes the `bytes.base64` of a `view.screenshot` result.
    pub fn from_base64(base64: &str) -> Result<Self, E2eError> {
        let bytes =
            base64::engine::general_purpose::STANDARD.decode(base64).map_err(|e| E2eError::Png(e.to_string()))?;
        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().map_err(|e| E2eError::Png(e.to_string()))?;
        let mut buffer =
            vec![0; reader.output_buffer_size().ok_or_else(|| E2eError::Png("the image is too large".into()))?];
        let info = reader.next_frame(&mut buffer).map_err(|e| E2eError::Png(e.to_string()))?;
        let pixels = buffer.get(..info.buffer_size()).ok_or_else(|| E2eError::Png("short frame".into()))?;
        let rgba = match info.color_type {
            png::ColorType::Rgba => pixels.to_vec(),
            png::ColorType::Rgb => pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            other => return Err(E2eError::Png(format!("unsupported colour type {other:?}"))),
        };
        Ok(Self { width: info.width, height: info.height, rgba })
    }

    /// The colour at a pixel, or `None` outside the image.
    pub fn at(&self, x: u32, y: u32) -> Option<Rgb> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let o = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba.get(o..o + 3).map(|p| [p[0], p[1], p[2]])
    }

    /// How many pixels of the rectangle are within `tol` of `color` in every channel.
    pub fn count(&self, rect: (u32, u32, u32, u32), color: Rgb, tol: u8) -> usize {
        let (x0, y0, w, h) = rect;
        (y0..y0 + h)
            .flat_map(|y| (x0..x0 + w).map(move |x| (x, y)))
            .filter_map(|(x, y)| self.at(x, y))
            .filter(|p| close(*p, color, tol))
            .count()
    }

    /// The share, 0 to 1, of the rectangle that is within `tol` of `color`.
    pub fn share(&self, rect: (u32, u32, u32, u32), color: Rgb, tol: u8) -> f64 {
        let area = f64::from(rect.2) * f64::from(rect.3);
        if area == 0.0 { 0.0 } else { self.count(rect, color, tol) as f64 / area }
    }
}

/// True when every channel of `a` is within `tol` of `b`.
pub fn close(a: Rgb, b: Rgb, tol: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tol)
}

/// Parses `#rrggbb`.
pub fn hex(color: &str) -> Option<Rgb> {
    let digits = color.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_six_digits_only() {
        assert_eq!(hex("#ff8000"), Some([255, 128, 0]));
        assert_eq!(hex("ff8000"), None);
        assert_eq!(hex("#f80"), None);
        assert_eq!(hex("#gg0000"), None);
    }

    #[test]
    fn close_compares_every_channel() {
        assert!(close([10, 20, 30], [12, 18, 30], 2));
        assert!(!close([10, 20, 30], [10, 20, 34], 2));
    }
}
