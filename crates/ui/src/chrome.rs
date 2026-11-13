//! Where the chrome is. Each surface that takes room records its rectangle in the frame it is drawn, and the
//! surfaces drawn after it (and the popovers that open next to it) read it, instead of repeating the height of
//! a bar as a number.
//!
//! The rectangles belong to one frame: [`ChromeRects::default`] starts every `draw`, and the order of
//! `surfaces::show_all` is the order of dependencies (the strip first, then the bottom bar, then the tabs, then
//! everything that sits above or below them).

use egui::{Rect, Vec2};

use crate::theme::{MARGIN, space};

/// The gap between a bar and what is placed next to it.
pub const GAP: f32 = space::LG;

/// The rectangles of the chrome surfaces drawn so far in this frame. `None`: not drawn (zen mode, or not yet).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ChromeRects {
    /// The top strip, across the window: the menu, the colours and the properties in two rows.
    pub strip: Option<Rect>,
    /// The hamburger button in the strip: the menu opens under it.
    pub menu_button: Option<Rect>,
    /// The style button of a strip too narrow for the colour group.
    pub style_button: Option<Rect>,
    /// The bottom-left bar (undo, grid, zoom, snapping).
    pub bottom_bar: Option<Rect>,
    /// The tab bar.
    pub tabs: Option<Rect>,
    /// The tool column.
    pub tools: Option<Rect>,
    /// The library drawer, when it is open. It is drawn after every bar and panel above, so it is the highest
    /// of the bottom surfaces: things that float above the bottom chrome sit over it.
    pub library: Option<Rect>,
    /// The minimap card, bottom right, when it is on. The presence panel stands above it.
    pub minimap: Option<Rect>,
}

impl ChromeRects {
    /// The y of the lower edge of the strip, or of the window top when there is none.
    pub fn below_strip(&self, screen: Rect) -> f32 {
        self.strip.map_or(screen.top(), |r| r.bottom())
    }

    /// The y of the upper edge of the bottom-left bar, or of the window bottom when there is none.
    pub fn above_bottom_bar(&self, screen: Rect) -> f32 {
        self.bottom_bar.map_or(screen.bottom(), |r| r.top())
    }

    /// The y of the upper edge of what stands on the bottom left (the bottom bar, the library drawer), or of the
    /// window bottom when there is none: panels anchored to the bottom left sit over it.
    pub fn above_bottom_left(&self, screen: Rect) -> f32 {
        [self.bottom_bar, self.library].into_iter().flatten().map(|r| r.top()).fold(screen.bottom(), f32::min)
    }

    /// The y of the highest bottom surface (the bottom bar, the tabs or the library drawer): things that float
    /// above the bottom chrome (toasts, the convert popup) sit over it.
    pub fn bottom_floor(&self, screen: Rect) -> f32 {
        [self.bottom_bar, self.tabs, self.library]
            .into_iter()
            .flatten()
            .map(|r| r.top())
            .fold(screen.bottom(), f32::min)
    }

    /// The tallest a panel can be that is centred on the window's middle and stays clear of the strip (with a
    /// gap). The side panels are centred, so the limit is twice the distance from the middle to the strip.
    pub fn centred_panel_height(&self, screen: Rect) -> f32 {
        2.0 * (screen.center().y - self.below_strip(screen) - GAP).max(0.0)
    }

    /// Like [`Self::centred_panel_height`], and also clear of the bottom-left bar (with a gap): for the tool column,
    /// which stands above that bar at the left edge. Without it, the last tools lie under the bar's first buttons
    /// on a window that is not tall (an iPad in landscape).
    pub fn centred_panel_height_above_bottom_bar(&self, screen: Rect) -> f32 {
        let below = (self.above_bottom_bar(screen) - screen.center().y - GAP).max(0.0);
        self.centred_panel_height(screen).min(2.0 * below)
    }
}

impl ChromeRects {
    /// How far a card of `size` anchored to the bottom right corner (with the window margin) must lift to stay clear
    /// of the bottom chrome: the margin when nothing reaches under it, else enough to stand a gap above the highest
    /// bottom surface that does (a narrow window, where the bar and the tabs span the width; the open library drawer).
    pub fn lift_bottom_right(&self, screen: Rect, size: Vec2) -> f32 {
        let left = screen.right() - MARGIN - size.x;
        [self.bottom_bar, self.tabs, self.library]
            .into_iter()
            .flatten()
            .filter(|r| r.right() + GAP > left)
            .map(|r| lift_to(screen, r.top(), GAP))
            .fold(MARGIN, f32::max)
    }
}

/// How far a surface anchored to the bottom edge must lift to sit `gap` above the line `y`.
pub fn lift_to(screen: Rect, y: f32, gap: f32) -> f32 {
    (screen.bottom() - y + gap).max(0.0)
}

#[cfg(test)]
mod tests {
    use egui::{pos2, vec2};

    use super::*;

    fn screen() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0))
    }

    #[test]
    fn without_chrome_the_edges_are_the_window() {
        let r = ChromeRects::default();
        assert_eq!(r.below_strip(screen()), 0.0);
        assert_eq!(r.above_bottom_bar(screen()), 800.0);
        assert_eq!(r.bottom_floor(screen()), 800.0);
    }

    #[test]
    fn the_bottom_floor_is_the_highest_bottom_surface() {
        let r = ChromeRects {
            bottom_bar: Some(Rect::from_min_size(pos2(10.0, 740.0), vec2(300.0, 48.0))),
            tabs: Some(Rect::from_min_size(pos2(400.0, 700.0), vec2(200.0, 48.0))),
            ..ChromeRects::default()
        };
        assert_eq!(r.bottom_floor(screen()), 700.0);
        assert_eq!(r.above_bottom_bar(screen()), 740.0);
    }

    #[test]
    fn the_library_drawer_is_the_highest_bottom_surface_when_it_is_open() {
        let mut r = ChromeRects {
            bottom_bar: Some(Rect::from_min_size(pos2(10.0, 740.0), vec2(300.0, 48.0))),
            tabs: Some(Rect::from_min_size(pos2(400.0, 700.0), vec2(200.0, 48.0))),
            ..ChromeRects::default()
        };
        r.library = Some(Rect::from_min_size(pos2(10.0, 500.0), vec2(980.0, 190.0)));
        assert_eq!(r.bottom_floor(screen()), 500.0);
        assert_eq!(r.above_bottom_left(screen()), 500.0);
        assert_eq!(r.above_bottom_bar(screen()), 740.0, "the bar itself is where it was");
    }

    #[test]
    fn a_centred_panel_stays_under_the_strip() {
        let r = ChromeRects {
            strip: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 40.0))),
            ..ChromeRects::default()
        };
        // The panel is centred at 400 and may reach up to 48: 352 above the middle, so 704 in all.
        assert_eq!(r.centred_panel_height(screen()), 2.0 * (400.0 - 40.0 - GAP));
        let short = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 60.0));
        assert_eq!(r.centred_panel_height(short), 0.0, "never negative");
    }

    #[test]
    fn a_panel_above_the_bottom_bar_is_shorter_when_the_bar_is_in_the_way() {
        let strip = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 40.0));
        let bar = Rect::from_min_size(pos2(10.0, 700.0), vec2(300.0, 48.0));
        let r = ChromeRects { strip: Some(strip), bottom_bar: Some(bar), ..ChromeRects::default() };
        // The middle is at 400: 312 of room above the bar after the gap, 352 under the strip.
        assert_eq!(r.centred_panel_height_above_bottom_bar(screen()), 2.0 * (700.0 - 400.0 - GAP));
        let without_bar = ChromeRects { strip: Some(strip), ..ChromeRects::default() };
        assert_eq!(
            without_bar.centred_panel_height_above_bottom_bar(screen()),
            without_bar.centred_panel_height(screen()),
            "no bar, no change"
        );
        let high_bar = ChromeRects { bottom_bar: Some(Rect::from_min_size(pos2(10.0, 380.0), vec2(300.0, 48.0))), ..r };
        assert_eq!(high_bar.centred_panel_height_above_bottom_bar(screen()), 0.0, "never negative");
    }

    #[test]
    fn a_bottom_right_card_stays_at_the_margin_when_the_bottom_chrome_is_clear_of_it() {
        let r = ChromeRects {
            bottom_bar: Some(Rect::from_min_size(pos2(10.0, 740.0), vec2(300.0, 48.0))),
            tabs: Some(Rect::from_min_size(pos2(420.0, 740.0), vec2(160.0, 48.0))),
            ..ChromeRects::default()
        };
        assert_eq!(r.lift_bottom_right(screen(), vec2(200.0, 130.0)), MARGIN);
    }

    #[test]
    fn a_bottom_right_card_lifts_above_the_chrome_that_reaches_under_it() {
        let wide_bar = Rect::from_min_size(pos2(10.0, 740.0), vec2(900.0, 48.0));
        let r = ChromeRects { bottom_bar: Some(wide_bar), ..ChromeRects::default() };
        assert_eq!(r.lift_bottom_right(screen(), vec2(200.0, 130.0)), lift_to(screen(), 740.0, GAP));
        let library = Rect::from_min_size(pos2(10.0, 500.0), vec2(980.0, 190.0));
        let open = ChromeRects { bottom_bar: Some(wide_bar), library: Some(library), ..ChromeRects::default() };
        assert_eq!(
            open.lift_bottom_right(screen(), vec2(200.0, 130.0)),
            lift_to(screen(), 500.0, GAP),
            "the highest wins"
        );
    }

    #[test]
    fn lifting_never_goes_below_the_window_edge() {
        assert_eq!(lift_to(screen(), 740.0, 8.0), 68.0);
        assert_eq!(lift_to(screen(), 900.0, 8.0), 0.0);
    }
}
