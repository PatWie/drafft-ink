#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The grid costs a bounded amount at every zoom.

use drafftink_render::*;
use kurbo::{Size, Vec2};
use proptest::prelude::*;
use vello::Scene;

const STYLES: [GridStyle; 4] = [GridStyle::Lines, GridStyle::HorizontalLines, GridStyle::Crosses, GridStyle::Dots];

fn draw(view: &View, style: GridStyle) -> (GridStats, Scene) {
    let mut scene = Scene::new();
    let stats = paint_grid(&mut scene, view, &GridSpec::new(style), Theme::Light);
    (stats, scene)
}

#[test]
fn the_old_worst_case_is_now_small() {
    // Zoom 0.1 on 1920x1080 with crosses or dots was about 520,000 cells in the old renderer.
    let v = View::new(0.1, Vec2::ZERO, Size::new(1920.0, 1080.0));
    for style in [GridStyle::Crosses, GridStyle::Dots] {
        let (stats, scene) = draw(&v, style);
        assert!(stats.points > 0 && stats.points <= MAX_POINTS, "{style:?} {stats:?}");
        assert!(scene.encoding().path_tags.len() < 6 * MAX_POINTS, "{}", scene.encoding().path_tags.len());
    }
    let (stats, _) = draw(&v, GridStyle::Lines);
    assert!(stats.lines <= MAX_LINES, "{stats:?}");
}

#[test]
fn at_normal_zoom_the_base_spacing_is_kept() {
    let v = View::new(1.0, Vec2::ZERO, Size::new(1200.0, 800.0));
    for style in STYLES {
        let (stats, _) = draw(&v, style);
        assert_eq!(stats.spacing, 20.0, "{style:?}");
    }
}

#[test]
fn spacing_doubles_as_the_view_zooms_out() {
    let mut last = 0.0;
    for zoom in [2.0, 1.0, 0.5, 0.25, 0.1, 0.05, 0.01, 0.001] {
        let v = View::new(zoom, Vec2::ZERO, Size::new(1200.0, 800.0));
        let s = grid_spacing(&v, &GridSpec::new(GridStyle::Dots)).unwrap();
        assert!(s >= last, "zoom {zoom}: {s} < {last}");
        let k = (s / 20.0).log2();
        assert!((k - k.round()).abs() < 1e-9, "{s} is not 20 * 2^k");
        assert!(s * zoom >= MIN_CELL_PX, "zoom {zoom}: cells are {} px apart", s * zoom);
        last = s;
    }
}

#[test]
fn no_grid_for_no_style_or_a_bad_view() {
    let v = View::new(1.0, Vec2::ZERO, Size::new(100.0, 100.0));
    assert!(grid_spacing(&v, &GridSpec::new(GridStyle::None)).is_none());
    let bad = View::new(f64::NAN, Vec2::ZERO, Size::new(100.0, 100.0));
    assert!(grid_spacing(&bad, &GridSpec::new(GridStyle::Dots)).is_none());
    let mut spec = GridSpec::new(GridStyle::Lines);
    spec.spacing = 0.0;
    assert!(grid_spacing(&v, &spec).is_none());
    spec.spacing = f64::NAN;
    assert!(grid_spacing(&v, &spec).is_none());
}

#[test]
fn lines_use_one_draw_call() {
    let v = View::new(1.0, Vec2::ZERO, Size::new(1200.0, 800.0));
    for style in STYLES {
        let (_, scene) = draw(&v, style);
        assert_eq!(scene.encoding().n_paths, 1, "{style:?}");
    }
}

proptest! {
    #[test]
    fn grid_work_is_bounded_for_any_view(
        zoom in 1e-6f64..1e3, ox in -1e7f64..1e7, oy in -1e7f64..1e7, w in 1.0f64..8000.0, h in 1.0f64..8000.0,
        dpi in 0.5f64..4.0, style in 0usize..4, base in 1.0f64..200.0,
    ) {
        let v = View::new(zoom, Vec2::new(ox, oy), Size::new(w, h)).with_dpi(dpi);
        let mut spec = GridSpec::new(STYLES[style]);
        spec.spacing = base;
        let mut scene = Scene::new();
        let stats = paint_grid(&mut scene, &v, &spec, Theme::Dark);
        prop_assert!(stats.lines <= MAX_LINES, "{stats:?}");
        prop_assert!(stats.points <= MAX_POINTS, "{stats:?}");
        if stats.spacing > 0.0 {
            let k = (stats.spacing / base).log2();
            prop_assert!((k - k.round()).abs() < 1e-6);
            // Cells are far enough apart unless the limit on the count forced the spacing up instead.
            prop_assert!(stats.spacing * v.scale() >= MIN_CELL_PX - 1e-9);
        }
    }
}
