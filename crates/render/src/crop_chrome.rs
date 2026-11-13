//! What the crop editor draws on the crop window: the corner brackets, the bars on the sides, the outline of the
//! window and of the whole picture, and the rule-of-thirds grid.
//!
//! The look follows the crop mode of Office, Google Slides and Figma and not the box of a selection: a thick
//! L-shaped bracket on each corner and a short bar in the middle of each side, drawn outside the window so that they
//! never cover the pixels being framed. A light halo around each dark mark (a dark one around a light mark in the
//! dark theme) keeps them readable on any picture. The colours are a set of tokens per theme ([`CropChrome`]), the
//! sizes are in logical pixels, so the marks keep their size at any zoom.

use drafftink_geom::OrientedBox;
use kurbo::{Affine, BezPath, Cap, Join, Point, Stroke, Vec2};
use peniko::Color;
use vello::Scene;

use crate::theme::Theme;
use crate::view::View;

/// How far the halo reaches beyond a mark on every side (logical pixels).
const HALO: f64 = 1.5;
/// The free space kept between a bar and the arms of the brackets next to it.
const BAR_GAP: f64 = 4.0;
/// A bar shorter than this is left out (a window too small to hold one).
const MIN_BAR: f64 = 10.0;
/// Arms are at most this share of a side, so brackets of a small window do not run into each other.
const ARM_SHARE: f64 = 0.34;
/// Width of the thin outlines and of the grid lines.
const LINE_WIDTH: f64 = 1.0;
/// Opacity of the outline of the whole picture, against that of the window's own.
const PICTURE_OUTLINE: f32 = 0.6;

/// The size of the handles, which follows the pointer that is in use: a finger or a pen grabs from further away than
/// a mouse (`TransformerConfig::hit_radius_touch_px`), so the marks are drawn as large as what reacts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CropHandleSize {
    /// A mouse: slim marks.
    #[default]
    Mouse,
    /// A finger or a pen: marks the size of the hit area (24 to 28 logical pixels).
    Touch,
}

impl CropHandleSize {
    /// Thickness of a bracket or bar (logical pixels).
    pub const fn thickness(self) -> f64 {
        match self {
            Self::Mouse => 3.0,
            Self::Touch => 4.0,
        }
    }

    /// Length of each arm of a corner bracket, measured from the corner of the window.
    pub const fn arm(self) -> f64 {
        match self {
            Self::Mouse => 18.0,
            Self::Touch => 28.0,
        }
    }

    /// Length of the bar in the middle of a side.
    pub const fn bar(self) -> f64 {
        match self {
            Self::Mouse => 22.0,
            Self::Touch => 28.0,
        }
    }
}

/// The colours of the crop window in one theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropChrome {
    /// The brackets and bars.
    pub ink: Color,
    /// The ring around them, the opposite of `ink` in lightness, so that the marks show on dark and light pictures.
    pub halo: Color,
    /// The thin outlines and the grid lines.
    pub line: Color,
    /// The soft shadow under a thin line, so that a white line shows on a white canvas.
    pub shadow: Color,
}

impl Theme {
    /// The colours of the crop window in this theme. The light theme has the near-black handles of Office with a
    /// white halo; the dark theme turns both round, to sit on a dark canvas.
    pub fn crop_chrome(self) -> CropChrome {
        let (ink, halo) = match self {
            Theme::Light => (Color::from_rgba8(20, 20, 24, 255), Color::from_rgba8(255, 255, 255, 235)),
            Theme::Dark => (Color::from_rgba8(240, 240, 245, 255), Color::from_rgba8(0, 0, 0, 205)),
        };
        CropChrome { ink, halo, line: Color::from_rgba8(255, 255, 255, 235), shadow: Color::from_rgba8(0, 0, 0, 115) }
    }
}

/// Draws the crop window into `scene` (device space; call it after the cropped picture): the outline of the whole
/// `picture`, the outline of the `window`, the thirds grid when `grid` is set, and the eight handles. Both boxes
/// are in world space and may be rotated or mirrored. `size` is the handle size for the pointer in use.
pub fn paint_crop_chrome(
    scene: &mut Scene,
    view: &View,
    theme: Theme,
    window: &OrientedBox,
    picture: &OrientedBox,
    grid: bool,
    size: CropHandleSize,
) {
    let chrome = theme.crop_chrome();
    let to_device = view.world_to_device();
    let px = view.logical_to_device().as_coeffs()[0];
    let device = |b: &OrientedBox| b.corners().map(|p| to_device * p);
    if picture.is_valid() {
        let outline = quad(device(picture));
        thin(scene, px, &chrome, PICTURE_OUTLINE, &outline);
    }
    if !window.is_valid() {
        return;
    }
    let corners = device(window);
    thin(scene, px, &chrome, 1.0, &quad(corners));
    if grid {
        thin(scene, px, &chrome, 1.0, &thirds(corners));
    }
    let marks = handle_marks(corners, px, size);
    let id = Affine::IDENTITY;
    let width = size.thickness() * px;
    let ring = Stroke::new(width + 2.0 * HALO * px).with_join(Join::Miter).with_caps(Cap::Square);
    scene.stroke(&ring, id, chrome.halo, None, &marks);
    scene.stroke(&Stroke::new(width).with_join(Join::Miter).with_caps(Cap::Butt), id, chrome.ink, None, &marks);
}

/// A thin line in the theme's line colour over its shadow, at `opacity` (1 is the full colour).
fn thin(scene: &mut Scene, px: f64, chrome: &CropChrome, opacity: f32, path: &BezPath) {
    let id = Affine::IDENTITY;
    scene.stroke(&Stroke::new((LINE_WIDTH + 2.0) * px), id, chrome.shadow.multiply_alpha(opacity), None, path);
    scene.stroke(&Stroke::new(LINE_WIDTH * px), id, chrome.line.multiply_alpha(opacity), None, path);
}

/// The closed quad through `corners`.
fn quad(corners: [Point; 4]) -> BezPath {
    let mut path = BezPath::new();
    path.move_to(corners[0]);
    for p in &corners[1..] {
        path.line_to(*p);
    }
    path.close_path();
    path
}

/// The two lines that cut the quad `[top-left, top-right, bottom-right, bottom-left]` into thirds each way.
fn thirds(corners: [Point; 4]) -> BezPath {
    let [tl, tr, br, bl] = corners;
    let mut path = BezPath::new();
    for k in [1.0 / 3.0, 2.0 / 3.0] {
        path.move_to(tl.lerp(tr, k));
        path.line_to(bl.lerp(br, k));
        path.move_to(tl.lerp(bl, k));
        path.line_to(tr.lerp(br, k));
    }
    path
}

/// The unit vector along `v`, or `None` for a vector too short to have a direction.
fn unit(v: Vec2) -> Option<Vec2> {
    let len = v.hypot();
    (len.is_finite() && len > 1e-9).then(|| v / len)
}

/// The centre lines of the four corner brackets and of the side bars of the window with these device `corners`.
///
/// Every mark lies outside the window: its inner edge is on the outline of the window, so its centre line is half
/// a thickness out. The geometry is read off the corners, so a turned or mirrored window needs no case of its own.
/// A bracket's arms are at most [`ARM_SHARE`] of a side; a side too short for a bar and the gaps gets no bar.
pub(crate) fn handle_marks(corners: [Point; 4], px: f64, size: CropHandleSize) -> BezPath {
    let half = size.thickness() * px / 2.0;
    let centre = corners.iter().fold(Vec2::ZERO, |sum, p| sum + p.to_vec2()) / 4.0;
    let sides = [(0, 1), (1, 2), (2, 3), (3, 0)].map(|(a, b)| (corners[a], corners[b]));
    let shortest = sides.iter().map(|(a, b)| a.distance(*b)).fold(f64::INFINITY, f64::min);
    let arm = (size.arm() * px).min(shortest * ARM_SHARE);
    let mut path = BezPath::new();
    for (i, c) in corners.iter().enumerate() {
        let (prev, next) = (corners[(i + 3) % 4], corners[(i + 1) % 4]);
        let (Some(ua), Some(ub)) = (unit(next - *c), unit(prev - *c)) else { continue };
        let corner = *c - (ua + ub) * half;
        path.move_to(corner + ua * arm);
        path.line_to(corner);
        path.line_to(corner + ub * arm);
    }
    for (a, b) in sides {
        let Some(along) = unit(b - a) else { continue };
        let middle = a.midpoint(b);
        let free = a.distance(b) - 2.0 * (arm + BAR_GAP * px);
        let len = (size.bar() * px).min(free);
        if len < MIN_BAR * px {
            continue;
        }
        // Of the two normals, the one that points away from the window's centre.
        let normal = Vec2::new(-along.y, along.x);
        let out = if normal.dot(middle.to_vec2() - centre) >= 0.0 { normal } else { -normal };
        let at = middle + out * half;
        path.move_to(at - along * (len / 2.0));
        path.line_to(at + along * (len / 2.0));
    }
    path
}

#[cfg(test)]
mod tests {
    use kurbo::{PathEl, Rect, Shape, Size};

    use super::*;

    const SIZES: [CropHandleSize; 2] = [CropHandleSize::Mouse, CropHandleSize::Touch];

    fn points(path: &BezPath) -> Vec<Point> {
        path.elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect()
    }

    fn subpaths(path: &BezPath) -> usize {
        path.elements().iter().filter(|el| matches!(el, PathEl::MoveTo(_))).count()
    }

    fn window(angle: f64) -> OrientedBox {
        OrientedBox::new(Point::new(400.0, 300.0), Size::new(300.0, 200.0), angle)
    }

    #[test]
    fn the_marks_lie_outside_the_window_at_every_angle_and_size() {
        for size in SIZES {
            for angle in [0.0, 0.4, 1.7, -2.5] {
                let w = window(angle);
                let marks = handle_marks(w.corners(), 1.0, size);
                assert!(marks.bounding_box().area() > 0.0);
                for p in points(&marks) {
                    // The centre line of a mark is half a thickness out of the window: its inner edge is on the
                    // outline.
                    let d = w.distance_to_point(p);
                    assert!(!w.contains(p), "{size:?}: {p:?} at {angle} lies inside the window");
                    assert!(d >= size.thickness() / 2.0 - 1e-9, "{size:?}: {p:?} at {angle} is only {d} away");
                }
            }
        }
    }

    #[test]
    fn four_brackets_and_four_bars_on_a_roomy_window() {
        for size in SIZES {
            assert_eq!(subpaths(&handle_marks(window(0.0).corners(), 1.0, size)), 8, "{size:?}");
        }
    }

    #[test]
    fn a_small_window_keeps_its_brackets_and_loses_its_bars() {
        let small = OrientedBox::new(Point::new(50.0, 50.0), Size::new(30.0, 30.0), 0.0);
        for size in SIZES {
            let marks = handle_marks(small.corners(), 1.0, size);
            assert_eq!(subpaths(&marks), 4, "{size:?}: four brackets and no bar");
            // The arms never reach further than a third of the side, so two brackets do not touch.
            let pts = points(&marks);
            assert!(pts[0].distance(pts[1]) <= 30.0 * ARM_SHARE + 1e-9, "{size:?}");
        }
    }

    #[test]
    fn marks_keep_their_screen_size_at_any_scale() {
        let big = OrientedBox::new(Point::new(900.0, 700.0), Size::new(1600.0, 1200.0), 0.0);
        for size in SIZES {
            let arm = |px: f64| {
                let p = points(&handle_marks(big.corners(), px, size));
                p[0].distance(p[1])
            };
            // The same window with twice the pixel density: the arm is twice as long in device units.
            assert!((arm(2.0) - 2.0 * arm(1.0)).abs() < 1e-9, "{size:?}");
        }
    }

    /// The longest straight stretch of one bar or bracket arm, in logical pixels, on a window with room to spare.
    fn longest_mark(size: CropHandleSize) -> f64 {
        let big = OrientedBox::new(Point::new(900.0, 700.0), Size::new(1600.0, 1200.0), 0.0);
        let mut from = Point::ZERO;
        let mut longest = 0.0_f64;
        for el in handle_marks(big.corners(), 1.0, size).elements() {
            match el {
                PathEl::MoveTo(p) => from = *p,
                PathEl::LineTo(p) => {
                    longest = longest.max(from.distance(*p));
                    from = *p;
                }
                _ => {}
            }
        }
        longest
    }

    #[test]
    fn touch_marks_are_drawn_as_large_as_what_reacts_and_mouse_marks_stay_smaller() {
        let (mouse, touch) = (CropHandleSize::Mouse, CropHandleSize::Touch);
        assert!(touch.arm() >= 24.0 && touch.arm() <= 28.0, "an arm of 24 to 28 px, not {}", touch.arm());
        assert!(touch.bar() >= 24.0 && touch.bar() <= 28.0, "a bar of 24 to 28 px, not {}", touch.bar());
        assert!(touch.arm() > mouse.arm() && touch.bar() > mouse.bar() && touch.thickness() > mouse.thickness());
        // The geometry follows the size: the touch marks really are longer on the screen.
        assert!(longest_mark(touch) > longest_mark(mouse) + 5.0);
        assert!((longest_mark(touch) - touch.bar().max(touch.arm())).abs() < 1e-9);
    }

    #[test]
    fn a_degenerate_window_draws_nothing_and_does_not_panic() {
        let flat = OrientedBox::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0), 0.0);
        for size in SIZES {
            assert!(handle_marks(flat.corners(), 1.0, size).elements().is_empty());
        }
    }

    #[test]
    fn the_thirds_cut_each_side_into_three_equal_parts() {
        let corners = OrientedBox::from_rect(Rect::new(0.0, 0.0, 90.0, 60.0)).corners();
        let ps = points(&thirds(corners));
        assert_eq!(ps.len(), 8);
        let has = |values: Vec<f64>, v: f64| values.iter().any(|x| (x - v).abs() < 1e-9);
        assert!(has(ps.iter().map(|p| p.x).collect(), 30.0) && has(ps.iter().map(|p| p.x).collect(), 60.0));
        assert!(has(ps.iter().map(|p| p.y).collect(), 20.0) && has(ps.iter().map(|p| p.y).collect(), 40.0));
    }

    #[test]
    fn both_themes_have_a_mark_that_contrasts_with_its_halo() {
        let luma = |c: Color| {
            let [r, g, b, _] = c.components;
            0.299 * r + 0.587 * g + 0.114 * b
        };
        for theme in [Theme::Light, Theme::Dark] {
            let c = theme.crop_chrome();
            assert!((luma(c.ink) - luma(c.halo)).abs() > 0.6, "{theme:?}: ink and halo are far apart in lightness");
        }
        assert!(Theme::Light.crop_chrome().ink.components[0] < 0.2, "Office's near-black handles");
    }
}
