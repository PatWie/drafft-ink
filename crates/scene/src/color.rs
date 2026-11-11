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

/// Share of the inversion in the dark filter.
const DARK_INVERT: f64 = 0.93;
/// The `hue-rotate(180deg)` matrix of the CSS Filter Effects specification (cos = -1, sin = 0).
const DARK_HUE: [[f64; 3]; 3] = [[-0.574, 1.430, 0.144], [0.426, 0.430, 0.144], [0.426, 1.430, -0.856]];

/// The dark filter on channels in 0..=1, without clamping: an affine map, `hue * (invert * 1 + (1 - 2 invert) * c)`.
fn dark_filter(c: [f64; 3]) -> [f64; 3] {
    let inv = c.map(|v| v * (1.0 - DARK_INVERT) + (1.0 - v) * DARK_INVERT);
    DARK_HUE.map(|row| row.iter().zip(inv).map(|(m, v)| m * v).sum())
}

/// The channels (not clamped) the dark filter maps to `shown`, or `None` when the matrix cannot be inverted.
fn solve_dark_filter(shown: [f64; 3]) -> Option<[f64; 3]> {
    let m = &DARK_HUE;
    let cof = |r: usize, c: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
        m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
    };
    let det: f64 = (0..3).map(|c| m[0][c] * cof(0, c)).sum();
    if det.abs() < 1e-9 {
        return None;
    }
    // inverse = adjugate / det, with adjugate[i][j] = cofactor[j][i].
    let inv = |row: usize| -> f64 { (0..3).map(|c| cof(c, row) * shown[c]).sum::<f64>() / det };
    let inverted = [inv(0), inv(1), inv(2)];
    // inverted = invert + (1 - 2 invert) * c  =>  c = (inverted - invert) / (1 - 2 invert)
    Some(inverted.map(|v| (v - DARK_INVERT) / (1.0 - 2.0 * DARK_INVERT)))
}

/// Why a colour string was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a colour; use #rgb, #rrggbb, #rrggbbaa or \"transparent\"")]
pub struct ColorError(String);

impl ColorError {
    /// The text that was rejected, cut to its first 40 characters.
    pub fn input(&self) -> &str {
        &self.0
    }
}

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

    /// The colour as the dark theme shows it. This reproduces the CSS filter `invert(93%) hue-rotate(180deg)` that
    /// Excalidraw uses for its dark theme, so white becomes `#121212`, black ink becomes a light grey and hues keep
    /// their meaning. Alpha is untouched. The canvas (`render::Theme::Dark`), the SVG export and every colour the UI
    /// shows for an element share this one function (and [`Color::from_dark_theme`], its inverse); the matrices come
    /// from the CSS Filter Effects specification.
    pub fn for_dark_theme(self) -> Self {
        let [r, g, b] = dark_filter([self.r, self.g, self.b].map(|v| f64::from(v) / 255.0));
        let out = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self::rgba(out(r), out(g), out(b), self.a)
    }

    /// The stored colour whose dark-theme display is `self`: what to store when the person picks a colour they see
    /// on the dark canvas, so that the canvas draws exactly that colour. The result is theme independent, and a
    /// light theme shows it as it is.
    ///
    /// The filter is an affine map of the RGB cube that clamps what leaves it, so it is invertible except at the
    /// edge of what it can show: the dark theme cannot draw pure white or the most saturated hues (white shows at
    /// most as `#ededed`, the display of black). For such a colour the result is the stored colour whose display is
    /// the nearest one (least squared RGB distance), found by solving the affine map and then searching two steps
    /// around the clamped solution. For any colour the dark theme can show,
    /// `c.for_dark_theme().from_dark_theme().for_dark_theme() == c.for_dark_theme()`. Alpha is untouched.
    pub fn from_dark_theme(self) -> Self {
        let target = [self.r, self.g, self.b].map(|v| f64::from(v) / 255.0);
        let Some(solution) = solve_dark_filter(target) else { return self };
        let centre = solution.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as i32);
        let distance = |c: [i32; 3]| -> f64 {
            let shown = Self::rgb(c[0] as u8, c[1] as u8, c[2] as u8).for_dark_theme();
            [shown.r, shown.g, shown.b]
                .iter()
                .zip([self.r, self.g, self.b])
                .map(|(s, t)| (f64::from(*s) - f64::from(t)).powi(2))
                .sum()
        };
        let mut best = (f64::MAX, centre);
        for dr in -2..=2 {
            for dg in -2..=2 {
                for db in -2..=2 {
                    let c = [centre[0] + dr, centre[1] + dg, centre[2] + db];
                    if c.iter().any(|v| !(0..=255).contains(v)) {
                        continue;
                    }
                    // Nearer display first; between equal ones the candidate closest to the solved colour.
                    let d = distance(c) + 1e-3 * f64::from(dr * dr + dg * dg + db * db);
                    if d < best.0 {
                        best = (d, c);
                    }
                }
            }
        }
        let c = best.1;
        Self::rgba(c[0] as u8, c[1] as u8, c[2] as u8, self.a)
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
    fn the_dark_theme_maps_white_to_the_dark_canvas_and_keeps_alpha() {
        assert_eq!(Color::WHITE.for_dark_theme(), Color::rgb(18, 18, 18));
        assert_eq!(Color::BLACK.for_dark_theme(), Color::rgb(237, 237, 237));
        assert_eq!(Color::rgba(10, 200, 30, 77).for_dark_theme().a, 77);
    }

    #[test]
    fn the_dark_display_is_inverted_exactly_where_the_filter_does_not_clamp() {
        for r in (0..=255u16).step_by(15) {
            for g in (0..=255u16).step_by(15) {
                for b in (0..=255u16).step_by(15) {
                    let stored = Color::rgba(r as u8, g as u8, b as u8, 9);
                    let shown = stored.for_dark_theme();
                    let back = shown.from_dark_theme();
                    assert_eq!(back.a, 9, "alpha is untouched");
                    let again = back.for_dark_theme();
                    let edge = [shown.r, shown.g, shown.b].iter().any(|v| *v == 0 || *v == 255);
                    let off = [(shown.r, again.r), (shown.g, again.g), (shown.b, again.b)]
                        .iter()
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap_or(0);
                    // At the edge of what the filter can show (a channel clamped to 0 or 255) the stored colour is
                    // one of several that look alike; it is never more than one step off.
                    assert!(
                        if edge { off <= 1 } else { off == 0 },
                        "{stored:?} shows as {shown:?}, stored again as {back:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_colour_the_dark_theme_cannot_show_is_stored_as_the_nearest_one_it_can() {
        let miss = |stored: Color, asked: Color| {
            let s = stored.for_dark_theme();
            (i32::from(s.r) - i32::from(asked.r)).pow(2)
                + (i32::from(s.g) - i32::from(asked.g)).pow(2)
                + (i32::from(s.b) - i32::from(asked.b)).pow(2)
        };
        // White shows at best as the display of black, `#ededed`.
        let white = Color::WHITE.from_dark_theme();
        assert!(miss(white, Color::WHITE) <= miss(Color::BLACK, Color::WHITE), "{white:?}");
        assert!(white.for_dark_theme().r >= 236, "{white:?}");
        // The e2e spec `pixels.spec.ts` stores this colour for the White swatch of the dark theme.
        assert_eq!(Color::WHITE.from_dark_theme(), Color::rgb(1, 0, 1));
        // The dark canvas colour is the display of white.
        assert_eq!(Color::rgb(18, 18, 18).from_dark_theme(), Color::WHITE);
        // Pure red is out of reach too; the stored colour is the nearest one anywhere near it.
        let red = Color::rgb(255, 0, 0);
        let stored = red.from_dark_theme();
        for other in
            [Color::BLACK, Color::WHITE, Color::rgb(255, 0, 0), Color::rgb(0, 255, 255), Color::rgb(128, 128, 128)]
        {
            assert!(miss(stored, red) <= miss(other, red), "{stored:?} against {other:?}");
        }
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
