//! Hue, saturation and value: the model of the custom colour picker (`surfaces/color_custom.rs`).
//!
//! The picker draws a square (saturation across, value up) and a hue bar, so the colour it edits is kept in these
//! three numbers: converting back and forth through RGB would lose the hue of every grey and black, and a drag
//! along the edge of the square would jump. [`Hsv::from_color`] is for a colour that comes from outside (the
//! document, the hex field); [`Hsv::to_color`] gives the 8-bit colour the pick stores.

use drafftink_scene::Color;

/// A colour as hue (degrees, `0..360`), saturation and value (`0..=1`). Every constructor and setter keeps the
/// three in range, and a number that is not finite counts as the lowest value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    h: f32,
    s: f32,
    v: f32,
}

impl Default for Hsv {
    /// A blue that shows on both themes, for a picker that opens with no colour to start from.
    fn default() -> Self {
        Self { h: 217.0, s: 0.76, v: 0.96 }
    }
}

/// `x` clamped into `lo..=hi`; `lo` when it is not finite.
fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x.is_finite() { x.clamp(lo, hi) } else { lo }
}

/// A channel in `0..=1` as a byte.
fn byte(x: f32) -> u8 {
    // The input is in 0..=1, so the product is in 0..=255 and the cast cannot truncate.
    (clamp(x, 0.0, 1.0) * 255.0).round() as u8
}

impl Hsv {
    /// The hue of the most saturated colour at the top of the hue bar: red again at this many degrees.
    pub const HUE_RANGE: f32 = 360.0;

    /// A colour from its three numbers. The hue wraps (360 is 0, -90 is 270); the others are clamped.
    pub fn new(h: f32, s: f32, v: f32) -> Self {
        let h = if h.is_finite() { h.rem_euclid(Self::HUE_RANGE) } else { 0.0 };
        Self { h, s: clamp(s, 0.0, 1.0), v: clamp(v, 0.0, 1.0) }
    }

    /// The hue, in degrees from 0 (inclusive) to 360 (exclusive).
    pub fn hue(self) -> f32 {
        self.h
    }

    /// The saturation, from 0 (grey) to 1.
    pub fn saturation(self) -> f32 {
        self.s
    }

    /// The value, from 0 (black) to 1.
    pub fn value(self) -> f32 {
        self.v
    }

    /// The same colour with another hue.
    #[must_use]
    pub fn with_hue(self, h: f32) -> Self {
        Self::new(h, self.s, self.v)
    }

    /// The same hue with another saturation and value.
    #[must_use]
    pub fn with_sv(self, s: f32, v: f32) -> Self {
        Self::new(self.h, s, v)
    }

    /// The numbers of an RGB colour (alpha is not part of them). A grey has no hue: it gets 0.
    pub fn from_color(c: Color) -> Self {
        let [r, g, b] = [c.r, c.g, c.b].map(|x| f32::from(x) / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let h = if delta <= 0.0 {
            0.0
        } else if max == r {
            60.0 * ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        let s = if max <= 0.0 { 0.0 } else { delta / max };
        Self::new(h, s, max)
    }

    /// The colour as 8-bit RGB with the given alpha.
    pub fn to_color(self, alpha: u8) -> Color {
        let c = self.v * self.s;
        let sector = self.h / 60.0;
        let x = c * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match sector as u8 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = self.v - c;
        Color::rgba(byte(r + m), byte(g + m), byte(b + m), alpha)
    }

    /// The most saturated, brightest colour of this hue: the colour of the top right corner of the square.
    pub fn pure_hue(self) -> Color {
        Self::new(self.h, 1.0, 1.0).to_color(255)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_primaries_and_the_greys_have_their_numbers() {
        let cases = [
            (Color::rgb(255, 0, 0), (0.0, 1.0, 1.0)),
            (Color::rgb(255, 255, 0), (60.0, 1.0, 1.0)),
            (Color::rgb(0, 255, 0), (120.0, 1.0, 1.0)),
            (Color::rgb(0, 255, 255), (180.0, 1.0, 1.0)),
            (Color::rgb(0, 0, 255), (240.0, 1.0, 1.0)),
            (Color::rgb(255, 0, 255), (300.0, 1.0, 1.0)),
            (Color::WHITE, (0.0, 0.0, 1.0)),
            (Color::BLACK, (0.0, 0.0, 0.0)),
        ];
        for (color, (h, s, v)) in cases {
            let hsv = Hsv::from_color(color);
            assert_eq!((hsv.hue(), hsv.saturation(), hsv.value()), (h, s, v), "{color:?}");
            assert_eq!(hsv.to_color(255), color, "{color:?} back");
        }
        let grey = Hsv::from_color(Color::rgb(128, 128, 128));
        assert_eq!((grey.hue(), grey.saturation()), (0.0, 0.0));
    }

    #[test]
    fn a_colour_survives_the_trip_through_hue_saturation_and_value() {
        // Every grey, and a lattice over the cube that includes its edges.
        for g in 0..=255u8 {
            let c = Color::rgb(g, g, g);
            assert_eq!(Hsv::from_color(c).to_color(255), c, "grey {g}");
        }
        let steps: Vec<u8> = (0..=255u8).step_by(3).chain([254, 255]).collect();
        for &r in &steps {
            for &g in &steps {
                for &b in &steps {
                    let c = Color::rgb(r, g, b);
                    assert_eq!(Hsv::from_color(c).to_color(255), c, "{c:?}");
                }
            }
        }
    }

    #[test]
    fn alpha_is_carried_and_not_part_of_the_numbers() {
        let c = Color::rgba(10, 200, 90, 77);
        assert_eq!(Hsv::from_color(c).to_color(77), c);
        assert_eq!(Hsv::from_color(c), Hsv::from_color(Color::rgb(10, 200, 90)));
    }

    #[test]
    fn the_hue_wraps_and_the_rest_is_clamped() {
        assert_eq!(Hsv::new(360.0, 1.0, 1.0).hue(), 0.0);
        assert_eq!(Hsv::new(-90.0, 1.0, 1.0).hue(), 270.0);
        assert_eq!(Hsv::new(725.0, 2.0, -1.0), Hsv::new(5.0, 1.0, 0.0));
        assert_eq!(Hsv::new(0.0, 1.0, 1.0).to_color(255), Color::rgb(255, 0, 0));
    }

    #[test]
    fn numbers_that_are_not_finite_never_panic_and_give_a_colour() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let hsv = Hsv::new(bad, bad, bad);
            assert!(hsv.hue().is_finite() && hsv.saturation().is_finite() && hsv.value().is_finite());
            let _ = hsv.to_color(255);
        }
    }

    #[test]
    fn a_drag_along_the_grey_edge_keeps_the_hue() {
        // The state of the picker is the three numbers, so a hue chosen on the bar survives a trip to a grey.
        let blue = Hsv::new(220.0, 0.8, 0.9);
        let grey = blue.with_sv(0.0, 0.5);
        assert_eq!(grey.hue(), 220.0);
        let back = grey.with_sv(0.8, 0.9);
        assert_eq!(back, blue);
    }

    #[test]
    fn the_pure_hue_is_the_corner_of_the_square() {
        assert_eq!(Hsv::new(120.0, 0.3, 0.2).pure_hue(), Color::rgb(0, 255, 0));
    }
}
