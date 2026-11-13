//! How the crop editor dims the part of a picture outside the crop window.
//!
//! The editor draws the whole picture faded, the draft of the crop over it, and the window with its handles. This
//! module owns the first part and the dark scrim over it, so that the dimmed area stays readable against a busy
//! canvas. The look is a pair of tokens per theme ([`CropDim`]), not literals at the call site.

use drafftink_geom::OrientedBox;
use kurbo::{Affine, BezPath, Point, Rect};
use peniko::{BlendMode, Color, Compose, Fill, Mix};
use vello::Scene;

use crate::theme::Theme;
use crate::view::View;

/// How the picture outside the crop window looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropDim {
    /// Opacity of the picture itself, 0 to 1.
    pub picture_opacity: f32,
    /// The dark layer over the picture area outside the window. Its alpha is the strength of the dimming.
    pub scrim: Color,
}

impl Theme {
    /// The dimming of the crop editor in this theme. Light mode needs a strong scrim to separate a pale picture
    /// from a white canvas; dark mode starts from a dark canvas, so a little more black keeps the contrast.
    pub fn crop_dim(self) -> CropDim {
        match self {
            Theme::Light => CropDim { picture_opacity: 0.5, scrim: Color::from_rgba8(0, 0, 0, 140) },
            Theme::Dark => CropDim { picture_opacity: 0.5, scrim: Color::from_rgba8(0, 0, 0, 153) },
        }
    }
}

/// Draws `whole`, the encoded uncropped picture, dimmed into `scene`, and covers the part of `picture` outside
/// `window` with the theme's scrim. Draw the cropped picture over it afterwards. The canvas around the picture is
/// left alone. Both boxes are in world space and may be rotated or mirrored.
pub fn paint_crop_dim(
    scene: &mut Scene,
    view: &View,
    theme: Theme,
    whole: &Scene,
    picture: &OrientedBox,
    window: &OrientedBox,
) {
    let dim = theme.crop_dim();
    let device = view.device_size();
    let clip = Rect::new(0.0, 0.0, device.width, device.height);
    scene.push_layer(
        Fill::NonZero,
        BlendMode::new(Mix::Normal, Compose::SrcOver),
        dim.picture_opacity,
        Affine::IDENTITY,
        &clip,
    );
    scene.append(whole, None);
    scene.pop_layer();
    if picture.is_valid() {
        scene.fill(Fill::NonZero, Affine::IDENTITY, dim.scrim, None, &ring(view, picture, window));
    }
}

/// The picture's quad with the window cut out, in device space. The window runs the other way round, so the
/// non-zero winding is 0 inside it and the two edges cancel where they coincide (a window as big as the picture
/// leaves no scrim and no seam).
fn ring(view: &View, picture: &OrientedBox, window: &OrientedBox) -> BezPath {
    let to_device = view.world_to_device();
    let mut path = BezPath::new();
    add_quad(&mut path, picture.corners().map(|p| to_device * p), Winding::Positive);
    if window.is_valid() {
        add_quad(&mut path, window.corners().map(|p| to_device * p), Winding::Negative);
    }
    path
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Winding {
    Positive,
    Negative,
}

/// Adds the closed quad `corners` as a subpath that runs the way `winding` says, whatever the order of the corners
/// (a mirrored box lists them the other way round).
fn add_quad(path: &mut BezPath, corners: [Point; 4], winding: Winding) {
    let area: f64 = corners.iter().zip(corners.iter().cycle().skip(1)).map(|(a, b)| a.x * b.y - b.x * a.y).sum();
    let positive = area >= 0.0;
    let ordered: Vec<Point> = if positive == (winding == Winding::Positive) {
        corners.to_vec()
    } else {
        corners.iter().rev().copied().collect()
    };
    if let Some((first, rest)) = ordered.split_first() {
        path.move_to(*first);
        for p in rest {
            path.line_to(*p);
        }
        path.close_path();
    }
}

#[cfg(test)]
mod tests {
    use kurbo::{Shape, Size, Vec2};

    use super::*;

    fn view() -> View {
        View::new(1.5, Vec2::new(20.0, 10.0), Size::new(800.0, 600.0))
    }

    /// The window as a part of the picture's local frame: its centre `(cx, cy)` and size, both from the picture's
    /// centre, and turned with the picture.
    fn window_in(picture: &OrientedBox, cx: f64, cy: f64, w: f64, h: f64) -> OrientedBox {
        OrientedBox::new(picture.to_world(Point::new(cx, cy)), Size::new(w, h), picture.angle)
    }

    fn covered(path: &BezPath, view: &View, world: Point) -> bool {
        path.winding(view.world_to_device() * world) != 0
    }

    #[test]
    fn the_scrim_covers_the_picture_outside_the_window_and_nothing_else() {
        for angle in [0.0, 0.4, -2.5] {
            let picture = OrientedBox::from_xywh(50.0, 40.0, 200.0, 100.0, angle);
            let window = window_in(&picture, -40.0, 0.0, 80.0, 100.0);
            let path = ring(&view(), &picture, &window);
            let local = |x, y| picture.to_world(Point::new(x, y));
            assert!(covered(&path, &view(), local(60.0, 0.0)), "outside the window, angle {angle}");
            assert!(covered(&path, &view(), local(-90.0, 30.0)), "left of the window, angle {angle}");
            assert!(!covered(&path, &view(), local(-40.0, 0.0)), "inside the window, angle {angle}");
            assert!(!covered(&path, &view(), local(150.0, 0.0)), "beyond the picture, angle {angle}");
            assert!(!covered(&path, &view(), local(0.0, 80.0)), "below the picture, angle {angle}");
        }
    }

    #[test]
    fn the_window_is_cut_out_whatever_order_the_corners_come_in() {
        // A mirrored box lists its corners the other way round; the hole must still be a hole.
        let picture = OrientedBox::from_xywh(0.0, 0.0, 100.0, 100.0, 0.0);
        let window = OrientedBox::from_xywh(25.0, 25.0, 50.0, 50.0, 0.0);
        let mut path = BezPath::new();
        let mut reversed = picture.corners();
        reversed.reverse();
        add_quad(&mut path, reversed, Winding::Positive);
        add_quad(&mut path, window.corners(), Winding::Negative);
        assert_ne!(path.winding(Point::new(10.0, 10.0)), 0);
        assert_eq!(path.winding(Point::new(50.0, 50.0)), 0);
    }

    #[test]
    fn a_window_as_big_as_the_picture_leaves_no_scrim() {
        let picture = OrientedBox::from_xywh(0.0, 0.0, 100.0, 60.0, 0.3);
        let path = ring(&view(), &picture, &picture);
        assert!(!covered(&path, &view(), picture.center));
        assert!(!covered(&path, &view(), picture.to_world(Point::new(49.0, 29.0))));
    }

    #[test]
    fn both_themes_dim_the_picture_to_a_clear_degree() {
        for theme in [Theme::Light, Theme::Dark] {
            let dim = theme.crop_dim();
            assert!(dim.picture_opacity <= 0.5, "{theme:?}: the picture is at most half there");
            assert!(dim.scrim.components[3] >= 0.5, "{theme:?}: the scrim is at least half opaque");
        }
    }
}
