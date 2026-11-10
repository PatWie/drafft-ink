//! Colour as stored in a document: 8-bit sRGB with straight alpha, written as `#rrggbb` or `#rrggbbaa`.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB colour with straight (non-premultiplied) alpha.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red, 0-255.
    pub r: u8,
    /// Green, 0-255.
    pub g: u8,
    /// Blue, 0-255.
    pub b: u8,
    /// Alpha, 0 (clear) to 255 (opaque).
    pub a: u8,
}

/// Why a colour string was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a colour; use #rgb, #rrggbb, #rrggbbaa or \"transparent\"")]
pub struct ColorError(pub String);

impl Color {
    /// Opaque black.
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0, a: 255 };
    /// Opaque white.
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255, a: 255 };
    /// Fully clear.
    pub const TRANSPARENT: Color = Color { r: 0, g: 0, b: 0, a: 0 };

    /// A colour from 8-bit channels.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Parses `#rgb`, `#rrggbb`, `#rrggbbaa` (case-insensitive) or `transparent`. Works on bytes, so non-ASCII input
    /// is an error and never a panic (the old app sliced by byte offset and crashed).
    pub fn parse(s: &str) -> Result<Self, ColorError> {
        let err = || ColorError(s.chars().take(40).collect());
        if s.eq_ignore_ascii_case("transparent") {
            return Ok(Self::TRANSPARENT);
        }
        let hex = s.strip_prefix('#').ok_or_else(err)?.as_bytes();
        let nib = |b: u8| -> Result<u8, ColorError> {
            match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                b'A'..=b'F' => Ok(b - b'A' + 10),
                _ => Err(err()),
            }
        };
        let byte = |i: usize| -> Result<u8, ColorError> {
            let hi = nib(hex.get(i).copied().ok_or_else(err)?)?;
            let lo = nib(hex.get(i + 1).copied().ok_or_else(err)?)?;
            Ok(hi * 16 + lo)
        };
        match hex.len() {
            3 => {
                let n = |i: usize| -> Result<u8, ColorError> { Ok(nib(hex.get(i).copied().ok_or_else(err)?)? * 17) };
                Ok(Self::rgb(n(0)?, n(1)?, n(2)?))
            }
            6 => Ok(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Ok(Self::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => Err(err()),
        }
    }

    /// `#rrggbb`, or `#rrggbbaa` when not opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// The same colour with its alpha multiplied by `opacity` (clamped to 0..=1).
    pub fn with_opacity(self, opacity: f64) -> Self {
        let o = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 1.0 };
        // The product is in 0..=255 by construction, so the cast cannot truncate.
        Self { a: (f64::from(self.a) * o).round() as u8, ..self }
    }
}

impl fmt::Debug for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Color({})", self.to_hex())
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::BLACK
    }
}

impl From<peniko::Color> for Color {
    fn from(c: peniko::Color) -> Self {
        let p = c.to_rgba8();
        Self { r: p.r, g: p.g, b: p.b, a: p.a }
    }
}

impl From<Color> for peniko::Color {
    fn from(c: Color) -> Self {
        peniko::Color::from_rgba8(c.r, c.g, c.b, c.a)
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        Color::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Color {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Color".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "#rrggbb, or #rrggbbaa for a non-opaque colour. 'transparent' and #rgb are also accepted.",
            "pattern": "^(#[0-9a-fA-F]{3}|#[0-9a-fA-F]{6}|#[0-9a-fA-F]{8}|transparent)$"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_forms() {
        assert_eq!(Color::parse("#000").unwrap(), Color::BLACK);
        assert_eq!(Color::parse("#fff").unwrap(), Color::WHITE);
        assert_eq!(Color::parse("#6366F1").unwrap(), Color::rgb(0x63, 0x66, 0xf1));
        assert_eq!(Color::parse("#ff000080").unwrap(), Color::rgba(255, 0, 0, 128));
        assert_eq!(Color::parse("transparent").unwrap(), Color::TRANSPARENT);
    }

    #[test]
    fn round_trips_through_text() {
        for c in [Color::BLACK, Color::rgba(1, 2, 3, 4), Color::TRANSPARENT, Color::rgb(255, 128, 0)] {
            assert_eq!(Color::parse(&c.to_hex()).unwrap(), c);
        }
        assert_eq!(Color::rgb(0x63, 0x66, 0xf1).to_hex(), "#6366f1");
    }

    #[test]
    fn hostile_strings_are_errors() {
        for bad in ["", "#", "#aé", "#éé", "#12345", "red", "rgb(1,2,3)", "#ggg", "#1234", "000000", "#00000000f"] {
            assert!(Color::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn peniko_conversion_round_trips() {
        let c = Color::rgba(10, 20, 30, 200);
        let p: peniko::Color = c.into();
        assert_eq!(Color::from(p), c);
    }

    #[test]
    fn opacity_scales_alpha() {
        assert_eq!(Color::WHITE.with_opacity(0.5).a, 128);
        assert_eq!(Color::WHITE.with_opacity(f64::NAN).a, 255);
        assert_eq!(Color::WHITE.with_opacity(7.0).a, 255);
    }
}
