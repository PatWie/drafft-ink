//! Counting clicks for word and paragraph selection inside the text editor.

use drafftink_geom::kurbo::Point;

/// Longest gap between two clicks of one multi-click, in milliseconds (the canvas uses the same, A-227).
const MAX_GAP_MS: f64 = 500.0;
/// Farthest two clicks of one multi-click may be apart, in logical pixels.
const MAX_DISTANCE: f64 = 5.0;

/// Tells single, double and triple clicks apart.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clicks {
    last: Option<(Point, f64)>,
    count: u32,
}

impl Clicks {
    /// Registers a press at `at` (logical pixels) at time `t_ms`; returns 1 for a single click, 2 for a double, 3
    /// for a triple and on from there. A fourth click starts again at one.
    pub fn press(&mut self, at: Point, t_ms: f64) -> u32 {
        let again = self.last.is_some_and(|(p, t)| {
            let dt = t_ms - t;
            (0.0..=MAX_GAP_MS).contains(&dt) && p.distance(at) <= MAX_DISTANCE
        });
        self.count = if again && self.count < 3 { self.count + 1 } else { 1 };
        self.last = Some((at, t_ms));
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_clicks_in_one_place_count_up_to_three_then_start_again() {
        let mut c = Clicks::default();
        let p = Point::new(10.0, 10.0);
        assert_eq!(c.press(p, 0.0), 1);
        assert_eq!(c.press(p, 200.0), 2);
        assert_eq!(c.press(Point::new(12.0, 11.0), 400.0), 3);
        assert_eq!(c.press(p, 500.0), 1);
    }

    #[test]
    fn a_slow_or_distant_click_is_a_new_single_click() {
        let mut c = Clicks::default();
        assert_eq!(c.press(Point::new(0.0, 0.0), 0.0), 1);
        assert_eq!(c.press(Point::new(0.0, 0.0), 900.0), 1);
        assert_eq!(c.press(Point::new(50.0, 0.0), 1000.0), 1);
    }
}
