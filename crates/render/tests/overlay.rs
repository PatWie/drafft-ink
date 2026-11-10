#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The overlay pass is independent of the document and keeps its on-screen size at every zoom.

mod common;

use common::*;
use drafftink_geom::OrientedBox;
use drafftink_geom::guides::{SmartGuide, SmartGuideKind};
use drafftink_geom::select::HandleSet;
use drafftink_render::*;
use kurbo::{BezPath, Point, Rect, Size, Vec2};
use peniko::Color;
use vello::Scene;

fn full() -> Overlay {
    let mut outline = BezPath::new();
    outline.move_to((0.0, 0.0));
    outline.line_to((50.0, 0.0));
    outline.line_to((50.0, 50.0));
    outline.close_path();
    Overlay {
        selection: vec![SelectionBox {
            obox: OrientedBox::from_xywh(10.0, 10.0, 100.0, 60.0, 0.3),
            handles: HandleSet::ALL,
            dashed: true,
        }],
        point_handles: vec![
            PointHandle { at: Point::new(0.0, 0.0), kind: PointKind::End },
            PointHandle { at: Point::new(5.0, 5.0), kind: PointKind::Midpoint },
        ],
        hover: vec![OrientedBox::from_xywh(0.0, 0.0, 20.0, 20.0, 0.0)],
        binding: vec![BindingHighlight { outline, at: Some(Point::new(10.0, 0.0)) }],
        marquee: Some(Rect::new(0.0, 0.0, 30.0, 30.0)),
        guides: vec![SmartGuide {
            kind: SmartGuideKind::Vertical,
            position: 40.0,
            start: 0.0,
            end: 100.0,
            snap_points: vec![0.0, 100.0],
        }],
        snap_point: Some(Point::new(40.0, 40.0)),
        cursors: vec![RemoteCursor { at: Point::new(1.0, 2.0), color: Color::from_rgba8(255, 0, 0, 255) }],
    }
}

fn view(zoom: f64) -> View {
    View::new(zoom, Vec2::new(20.0, 20.0), Size::new(800.0, 600.0))
}

#[test]
fn an_empty_overlay_draws_nothing() {
    let mut s = Scene::new();
    let r = paint_overlay(&mut s, &Overlay::default(), &view(1.0), &OverlayStyle::default());
    assert_eq!(r.items, 0);
    assert!(s.encoding().is_empty());
    assert!(Overlay::default().is_empty());
}

#[test]
fn every_part_draws_something() {
    let o = full();
    assert!(!o.is_empty());
    let mut s = Scene::new();
    let r = paint_overlay(&mut s, &o, &view(1.0), &OverlayStyle::default());
    assert_eq!(r.items, 9);
    assert!(s.encoding().n_paths > 20, "{}", s.encoding().n_paths);
    for part in 0..8 {
        let mut o = Overlay::default();
        let f = full();
        match part {
            0 => o.selection = f.selection,
            1 => o.point_handles = f.point_handles,
            2 => o.hover = f.hover,
            3 => o.binding = f.binding,
            4 => o.marquee = f.marquee,
            5 => o.guides = f.guides,
            6 => o.snap_point = f.snap_point,
            _ => o.cursors = f.cursors,
        }
        let mut s = Scene::new();
        assert!(paint_overlay(&mut s, &o, &view(1.0), &OverlayStyle::default()).items >= 1, "part {part}");
        assert!(!s.encoding().is_empty(), "part {part}");
    }
}

#[test]
fn handles_do_not_grow_with_the_zoom() {
    // With a solid outline the encoded size of the overlay is the same at any zoom: the work does not depend on it.
    let mut o = full();
    o.selection[0].dashed = false;
    let sizes: Vec<usize> = [0.1, 1.0, 8.0, 500.0]
        .iter()
        .map(|z| {
            let mut s = Scene::new();
            paint_overlay(&mut s, &o, &view(*z), &OverlayStyle::default());
            s.encoding().path_data.len()
        })
        .collect();
    assert!(sizes.iter().all(|s| *s == sizes[0]), "{sizes:?}");
}

#[test]
fn a_huge_dashed_box_is_drawn_solid_instead_of_expanding_into_thousands_of_dashes() {
    let mut o = Overlay::default();
    o.selection.push(SelectionBox {
        obox: OrientedBox::from_xywh(0.0, 0.0, 1000.0, 1000.0, 0.0),
        handles: HandleSet::ALL,
        dashed: true,
    });
    let mut s = Scene::new();
    paint_overlay(&mut s, &o, &view(1.0), &OverlayStyle::default());
    let normal = s.encoding().path_data.len();
    paint_overlay(&mut s, &o, &view(300.0), &OverlayStyle::default());
    let zoomed = s.encoding().path_data.len();
    assert!(zoomed < normal * 2 + 2000, "{normal} -> {zoomed}");
}

#[test]
fn the_overlay_does_not_depend_on_the_document() {
    let mut ts = drafftink_typeset::Typesetter::new();
    let mut composer = SceneComposer::new();
    let mut world = Scene::new();
    let mut overlay = Scene::new();
    let mut out = Scene::new();
    for n in [10usize, 2000] {
        let store = store_of(mixed_elements(n, 1, &mut ts));
        let index = build_paint_index(&store);
        let v = view(1.0);
        composer.compose(&store, &index, &v, &mut ts, &mut world);
        paint_overlay(&mut overlay, &full(), &v, &OverlayStyle::default());
        combine(&world, &overlay, &mut out);
        assert_eq!(out.encoding().n_paths, world.encoding().n_paths + overlay.encoding().n_paths);
    }
}

#[test]
fn invalid_views_and_boxes_are_skipped() {
    let mut s = Scene::new();
    let bad = View::new(f64::NAN, Vec2::ZERO, Size::new(10.0, 10.0));
    assert_eq!(paint_overlay(&mut s, &full(), &bad, &OverlayStyle::default()).items, 0);
    let mut o = Overlay::default();
    o.selection.push(SelectionBox {
        obox: OrientedBox::from_xywh(f64::NAN, 0.0, 1.0, 1.0, 0.0),
        handles: HandleSet::ALL,
        dashed: false,
    });
    let r = paint_overlay(&mut s, &o, &view(1.0), &OverlayStyle::default());
    assert_eq!(r.items, 0);
}
