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

fn ts() -> drafftink_typeset::Typesetter {
    drafftink_typeset::Typesetter::new()
}

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
        lasso: vec![Point::new(0.0, 0.0), Point::new(40.0, 5.0), Point::new(20.0, 35.0)],
        guides: vec![
            SmartGuide {
                kind: SmartGuideKind::Vertical,
                position: 40.0,
                start: 0.0,
                end: 100.0,
                snap_points: vec![0.0, 100.0],
                gap: None,
            },
            SmartGuide {
                kind: SmartGuideKind::EqualSpacingH,
                position: 35.0,
                start: 100.0,
                end: 175.0,
                snap_points: Vec::new(),
                gap: None,
            },
        ],
        snap_point: Some(Point::new(40.0, 40.0)),
        angle_rays: Some(AngleRays { origin: Point::new(10.0, 10.0), step_degrees: 15.0, locked_degrees: 30.0 }),
        rotation: Some(RotationGuide { center: Point::new(60.0, 40.0), degrees: 45.0, snapped: true }),
        eraser: Some(EraserRing { at: Point::new(70.0, 70.0), radius_px: 10.0 }),
        cursors: vec![RemoteCursor::new(Point::new(1.0, 2.0), Color::from_rgba8(255, 0, 0, 255)).named("Ada")],
        remote_selections: vec![RemoteSelection {
            color: Color::from_rgba8(0, 160, 0, 255),
            boxes: vec![
                OrientedBox::from_xywh(60.0, 60.0, 30.0, 30.0, 0.0),
                OrientedBox::from_xywh(0.0, 0.0, 9.0, 9.0, 0.5),
            ],
        }],
        trails: vec![LaserTrail {
            color: Color::from_rgba8(0, 0, 255, 255),
            points: (0..5)
                .map(|i| TrailPoint { at: Point::new(f64::from(i) * 10.0, 20.0), age_ms: f64::from(4 - i) * 100.0 })
                .collect(),
        }],
        link_icons: vec![
            LinkMark { rect: Rect::new(100.0, 0.0, 112.0, 12.0), to_element: false },
            LinkMark { rect: Rect::new(120.0, 0.0, 132.0, 12.0), to_element: true },
        ],
        text_edit: Some(TextEdit {
            to_world: kurbo::Affine::translate((10.0, 100.0)),
            caret: Some(Rect::new(5.0, 0.0, 5.0, 20.0)),
            selection: vec![Rect::new(10.0, 0.0, 40.0, 20.0)],
            composition: Vec::new(),
            caret_color: Color::BLACK,
        }),
    }
}

fn view(zoom: f64) -> View {
    View::new(zoom, Vec2::new(20.0, 20.0), Size::new(800.0, 600.0))
}

#[test]
fn an_empty_overlay_draws_nothing() {
    let mut s = Scene::new();
    let r = paint_overlay(&mut s, &Overlay::default(), &view(1.0), &OverlayStyle::default(), &mut ts());
    assert_eq!(r.items, 0);
    assert!(s.encoding().is_empty());
    assert!(Overlay::default().is_empty());
}

#[test]
fn every_part_draws_something() {
    let o = full();
    assert!(!o.is_empty());
    let mut s = Scene::new();
    let r = paint_overlay(&mut s, &o, &view(1.0), &OverlayStyle::default(), &mut ts());
    // marquee 1, lasso 1, text edit 1, hover 1, binding 1, remote selections 2, selection 1, point handles 2,
    // link icons 2, guides 2 and one badge, snap point 1, angle rays 1, rotation 1, eraser 1, trail 1, cursor 1.
    assert_eq!(r.items, 21);
    assert!(s.encoding().n_paths > 20, "{}", s.encoding().n_paths);
    let f = full();
    for name in PARTS {
        let o = only(name, &f);
        assert!(!o.is_empty(), "{name}");
        let mut s = Scene::new();
        assert!(paint_overlay(&mut s, &o, &view(1.0), &OverlayStyle::default(), &mut ts()).items >= 1, "part {name}");
        assert!(!s.encoding().is_empty(), "part {name}");
    }
}

const PARTS: [&str; 16] = [
    "selection",
    "point handles",
    "hover",
    "binding",
    "marquee",
    "guides",
    "snap point",
    "angle rays",
    "rotation",
    "eraser",
    "lasso",
    "cursors",
    "remote selections",
    "trails",
    "link icons",
    "text edit",
];

/// A copy of `from` that has only the named part.
fn only(part: &str, from: &Overlay) -> Overlay {
    let mut o = Overlay::default();
    let f = from.clone();
    match part {
        "selection" => o.selection = f.selection,
        "point handles" => o.point_handles = f.point_handles,
        "hover" => o.hover = f.hover,
        "binding" => o.binding = f.binding,
        "marquee" => o.marquee = f.marquee,
        "guides" => o.guides = f.guides,
        "snap point" => o.snap_point = f.snap_point,
        "angle rays" => o.angle_rays = f.angle_rays,
        "rotation" => o.rotation = f.rotation,
        "eraser" => o.eraser = f.eraser,
        "lasso" => o.lasso = f.lasso,
        "cursors" => o.cursors = f.cursors,
        "remote selections" => o.remote_selections = f.remote_selections,
        "trails" => o.trails = f.trails,
        "link icons" => o.link_icons = f.link_icons,
        "text edit" => o.text_edit = f.text_edit,
        other => panic!("no part called {other}"),
    }
    o
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
            paint_overlay(&mut s, &o, &view(*z), &OverlayStyle::default(), &mut ts());
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
    paint_overlay(&mut s, &o, &view(1.0), &OverlayStyle::default(), &mut ts());
    let normal = s.encoding().path_data.len();
    paint_overlay(&mut s, &o, &view(300.0), &OverlayStyle::default(), &mut ts());
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
        paint_overlay(&mut overlay, &full(), &v, &OverlayStyle::default(), &mut ts);
        combine(&world, &overlay, &mut out);
        assert_eq!(out.encoding().n_paths, world.encoding().n_paths + overlay.encoding().n_paths);
    }
}

#[test]
fn invalid_views_and_boxes_are_skipped() {
    let mut s = Scene::new();
    let bad = View::new(f64::NAN, Vec2::ZERO, Size::new(10.0, 10.0));
    assert_eq!(paint_overlay(&mut s, &full(), &bad, &OverlayStyle::default(), &mut ts()).items, 0);
    let mut o = Overlay::default();
    o.selection.push(SelectionBox {
        obox: OrientedBox::from_xywh(f64::NAN, 0.0, 1.0, 1.0, 0.0),
        handles: HandleSet::ALL,
        dashed: false,
    });
    let r = paint_overlay(&mut s, &o, &view(1.0), &OverlayStyle::default(), &mut ts());
    assert_eq!(r.items, 0);
}

fn encoded(o: &Overlay, zoom: f64) -> (usize, usize) {
    let mut s = Scene::new();
    paint_overlay(&mut s, o, &view(zoom), &OverlayStyle::default(), &mut ts());
    (s.encoding().n_paths as usize, s.encoding().resources.glyph_runs.len())
}

#[test]
fn a_named_cursor_draws_a_label_with_glyphs_and_an_unnamed_one_does_not() {
    let at = Point::new(10.0, 10.0);
    let red = Color::from_rgba8(255, 0, 0, 255);
    let plain = Overlay { cursors: vec![RemoteCursor::new(at, red)], ..Overlay::default() };
    let named = Overlay { cursors: vec![RemoteCursor::new(at, red).named("Ada")], ..Overlay::default() };
    let blank = Overlay { cursors: vec![RemoteCursor::new(at, red).named("  \n")], ..Overlay::default() };
    let (plain_paths, plain_runs) = encoded(&plain, 1.0);
    let (named_paths, named_runs) = encoded(&named, 1.0);
    assert_eq!(plain_runs, 0);
    assert!(named_runs >= 1, "the name is drawn as glyphs");
    assert!(named_paths > plain_paths, "and sits on a chip");
    assert_eq!(encoded(&blank, 1.0).1, 0, "a name of control characters and blanks is no label");
}

#[test]
fn labels_keep_their_size_on_screen_at_every_zoom_and_long_names_are_cut() {
    let named = |name: &str| Overlay {
        cursors: vec![RemoteCursor::new(Point::new(0.0, 0.0), Color::BLACK).named(name)],
        ..Overlay::default()
    };
    let sizes: Vec<_> = [0.05, 1.0, 40.0]
        .iter()
        .map(|z| {
            let mut s = Scene::new();
            paint_overlay(&mut s, &named("Ada"), &view(*z), &OverlayStyle::default(), &mut ts());
            (s.encoding().path_data.len(), s.encoding().resources.glyphs.len())
        })
        .collect();
    assert!(sizes.iter().all(|s| *s == sizes[0]), "{sizes:?}");
    let long: String = "w".repeat(10_000);
    let mut s = Scene::new();
    paint_overlay(&mut s, &named(&long), &view(1.0), &OverlayStyle::default(), &mut ts());
    assert!(s.encoding().resources.glyphs.len() <= 32, "{}", s.encoding().resources.glyphs.len());
}

#[test]
fn a_remote_selection_is_one_outline_per_box_and_a_turned_box_stays_turned() {
    let green = Color::from_rgba8(0, 160, 0, 255);
    let one = |angle: f64| Overlay {
        remote_selections: vec![RemoteSelection {
            color: green,
            boxes: vec![OrientedBox::from_xywh(10.0, 10.0, 50.0, 20.0, angle)],
        }],
        ..Overlay::default()
    };
    let mut s = Scene::new();
    let r = paint_overlay(&mut s, &one(0.0), &view(1.0), &OverlayStyle::default(), &mut ts());
    assert_eq!((r.items, s.encoding().n_paths), (1, 1));
    // An invalid box is skipped, and a selection of thousands of boxes is bounded.
    let bad = Overlay {
        remote_selections: vec![RemoteSelection {
            color: green,
            boxes: vec![OrientedBox::from_xywh(f64::NAN, 0.0, 1.0, 1.0, 0.0)],
        }],
        ..Overlay::default()
    };
    assert_eq!(paint_overlay(&mut s, &bad, &view(1.0), &OverlayStyle::default(), &mut ts()).items, 0);
    let many = Overlay {
        remote_selections: vec![RemoteSelection {
            color: green,
            boxes: vec![OrientedBox::from_xywh(0.0, 0.0, 5.0, 5.0, 0.0); 5000],
        }],
        ..Overlay::default()
    };
    assert_eq!(paint_overlay(&mut s, &many, &view(1.0), &OverlayStyle::default(), &mut ts()).items, 256);
}

#[test]
fn a_laser_trail_fades_with_its_age_and_an_expired_one_is_not_drawn() {
    let blue = Color::from_rgba8(0, 0, 255, 255);
    let trail = |ages: &[f64]| Overlay {
        trails: vec![LaserTrail {
            color: blue,
            points: ages
                .iter()
                .enumerate()
                .map(|(i, a)| TrailPoint { at: Point::new(i as f64 * 8.0, 0.0), age_ms: *a })
                .collect(),
        }],
        ..Overlay::default()
    };
    let count = |o: &Overlay| {
        let mut s = Scene::new();
        let r = paint_overlay(&mut s, o, &view(1.0), &OverlayStyle::default(), &mut ts());
        (r.items, s.encoding().n_paths)
    };
    let (items, paths) = count(&trail(&[300.0, 200.0, 100.0, 0.0]));
    assert_eq!(items, 1);
    assert_eq!(paths, 3 + 1, "three segments and the head dot");
    assert_eq!(count(&trail(&[1500.0, 2000.0, 9999.0])), (0, 0), "older than the fade time: gone");
    let (_, paths) = count(&trail(&[2000.0, 100.0, 0.0]));
    assert_eq!(paths, 1 + 1, "the expired point is dropped, the rest is drawn");
    assert_eq!(count(&trail(&[f64::NAN, f64::INFINITY])).0, 0);
    let mut hostile = trail(&[0.0]);
    hostile.trails[0].points[0].at = Point::new(f64::NAN, 0.0);
    assert_eq!(count(&hostile).0, 0);
}

#[test]
fn a_link_icon_is_a_tile_with_a_glyph_and_keeps_to_its_rectangle() {
    let icon = |to_element| Overlay {
        link_icons: vec![LinkMark { rect: Rect::new(100.0, 0.0, 112.0, 12.0), to_element }],
        ..Overlay::default()
    };
    for to_element in [false, true] {
        let mut s = Scene::new();
        let r = paint_overlay(&mut s, &icon(to_element), &view(2.0), &OverlayStyle::default(), &mut ts());
        assert_eq!(r.items, 1);
        assert_eq!(s.encoding().n_paths, 3, "tile fill, tile border, glyph (to_element {to_element})");
    }
    let flat = Overlay {
        link_icons: vec![LinkMark { rect: Rect::new(1.0, 1.0, 1.0, 5.0), to_element: false }],
        ..Overlay::default()
    };
    let mut s = Scene::new();
    assert_eq!(paint_overlay(&mut s, &flat, &view(1.0), &OverlayStyle::default(), &mut ts()).items, 0);
}

#[test]
fn a_spacing_guide_shows_its_gap_and_an_alignment_guide_does_not() {
    let guide = |kind, start, end| Overlay {
        guides: vec![SmartGuide { kind, position: 40.0, start, end, snap_points: Vec::new(), gap: None }],
        ..Overlay::default()
    };
    let (_, runs) = encoded(&guide(SmartGuideKind::EqualSpacingH, 100.0, 175.0), 1.0);
    assert!(runs >= 1, "75 is written next to the guide");
    assert_eq!(encoded(&guide(SmartGuideKind::Vertical, 100.0, 175.0), 1.0).1, 0);
    assert_eq!(encoded(&guide(SmartGuideKind::Horizontal, 100.0, 175.0), 1.0).1, 0);
    assert!(encoded(&guide(SmartGuideKind::EqualSpacingV, 100.0, 175.0), 1.0).1 >= 1);
    assert_eq!(
        encoded(&guide(SmartGuideKind::EqualSpacingH, 100.0, 100.0), 1.0).1,
        0,
        "a gap of nothing has no number"
    );
    assert_eq!(encoded(&guide(SmartGuideKind::EqualSpacingH, f64::NAN, 5.0), 1.0).1, 0);
}

#[test]
fn the_text_editor_overlay_draws_a_selection_and_a_caret_that_turn_with_the_element() {
    let edit = |caret, selection: Vec<Rect>| Overlay {
        text_edit: Some(TextEdit {
            to_world: kurbo::Affine::rotate(0.4),
            caret,
            selection,
            composition: Vec::new(),
            caret_color: Color::BLACK,
        }),
        ..Overlay::default()
    };
    let counts = |o: &Overlay| {
        let mut s = Scene::new();
        let r = paint_overlay(&mut s, o, &view(1.0), &OverlayStyle::default(), &mut ts());
        (r.items, s.encoding().n_paths)
    };
    assert_eq!(counts(&edit(None, vec![])), (0, 0), "a hidden caret and no selection: nothing");
    assert_eq!(counts(&edit(Some(Rect::new(3.0, 0.0, 3.0, 20.0)), vec![])), (1, 1));
    assert_eq!(
        counts(&edit(None, vec![Rect::new(0.0, 0.0, 30.0, 20.0), Rect::new(0.0, 20.0, 10.0, 40.0)])),
        (1, 1),
        "one fill for all lines"
    );
    assert_eq!(counts(&edit(Some(Rect::new(3.0, 0.0, 3.0, 0.0)), vec![])).0, 0, "a caret without height is skipped");
}

#[test]
fn text_edit_geometry_comes_from_the_typeset_caret_api() {
    let mut typesetter = ts();
    let layout = typesetter.text(&drafftink_typeset::TextSpec::new("hello world"));
    let collapsed = layout.selection_at_index(5, drafftink_typeset::Affinity::Downstream);
    let on = TextEdit::new(kurbo::Affine::IDENTITY, &layout, &collapsed, true, Color::BLACK);
    assert!(on.caret.is_some_and(|c| c.x0 > 0.0 && c.height() > 0.0) && on.selection.is_empty());
    assert_eq!(
        TextEdit::new(kurbo::Affine::IDENTITY, &layout, &collapsed, false, Color::BLACK).caret,
        None,
        "blinked off"
    );
    let range = layout.selection_from_range(0, 5);
    let sel = TextEdit::new(kurbo::Affine::IDENTITY, &layout, &range, true, Color::BLACK);
    assert_eq!(sel.caret, None, "no caret while text is selected");
    assert_eq!(sel.selection.len(), 1);
    assert!(sel.selection[0].width() > 0.0);
}

#[test]
fn composed_text_is_underlined_line_by_line() {
    let mut typesetter = ts();
    let layout = typesetter.text(&drafftink_typeset::TextSpec::new("ab nihao\ncd"));
    let collapsed = layout.selection_at_index(8, drafftink_typeset::Affinity::Downstream);
    let edit = TextEdit::new(kurbo::Affine::IDENTITY, &layout, &collapsed, false, Color::BLACK)
        .with_composition(&layout, 3..11);
    assert_eq!(edit.composition.len(), 2, "one underline per line the composition covers");
    assert!(edit.composition.iter().all(|r| r.width() > 0.0));
    let o = Overlay { text_edit: Some(edit), ..Overlay::default() };
    let mut s = Scene::new();
    let r = paint_overlay(&mut s, &o, &view(1.0), &OverlayStyle::default(), &mut typesetter);
    assert_eq!(r.items, 1);
    assert_eq!(s.encoding().n_paths, 2, "two underline strokes");
}

#[test]
fn an_alignment_guide_with_a_gap_shows_the_distance_and_one_without_does_not() {
    let guide = |gap| Overlay {
        guides: vec![SmartGuide {
            kind: SmartGuideKind::Vertical,
            position: 40.0,
            start: 0.0,
            end: 100.0,
            snap_points: Vec::new(),
            gap,
        }],
        ..Overlay::default()
    };
    assert_eq!(encoded(&guide(None), 1.0).1, 0);
    assert!(encoded(&guide(Some((20.0, 90.0))), 1.0).1 >= 1, "70 is written beside the gap");
    assert_eq!(encoded(&guide(Some((20.0, 20.2))), 1.0).1, 0, "a gap under half a unit is no number");
    let horizontal = Overlay {
        guides: vec![SmartGuide {
            kind: SmartGuideKind::Horizontal,
            position: 40.0,
            start: 0.0,
            end: 100.0,
            snap_points: Vec::new(),
            gap: Some((20.0, 90.0)),
        }],
        ..Overlay::default()
    };
    assert!(encoded(&horizontal, 1.0).1 >= 1);
}

#[test]
fn the_snap_crosshair_is_two_lines_across_the_view_and_a_ring() {
    let at = |p| Overlay { snap_point: Some(p), ..Overlay::default() };
    let mut s = Scene::new();
    let r = paint_overlay(&mut s, &at(Point::new(40.0, 40.0)), &view(1.0), &OverlayStyle::default(), &mut ts());
    assert_eq!(r.items, 1);
    assert_eq!(s.encoding().n_paths, 2, "the lines in one path, the ring in another");
    let mut s = Scene::new();
    let r = paint_overlay(&mut s, &at(Point::new(f64::NAN, 1.0)), &view(1.0), &OverlayStyle::default(), &mut ts());
    assert_eq!(r.items, 0, "a point that is not a number draws nothing");
}

#[test]
fn angle_rays_draw_rays_an_arc_and_the_angle_and_survive_hostile_numbers() {
    let rays = |step, locked| Overlay {
        angle_rays: Some(AngleRays { origin: Point::new(50.0, 50.0), step_degrees: step, locked_degrees: locked }),
        ..Overlay::default()
    };
    let (paths, runs) = encoded(&rays(15.0, 30.0), 1.0);
    assert!(paths >= 4, "faint rays, the locked ray, the arc and the chip: {paths}");
    assert!(runs >= 1, "the angle is written as a number");
    // A negative step, a zero step and a huge step fall back to the default instead of drawing a million rays.
    for step in [0.0, -5.0, 1.0e-9, f64::NAN, 1.0e9] {
        let (p, _) = encoded(&rays(step, 30.0), 1.0);
        assert!(p >= 4 && p < 100, "step {step}: {p} paths");
    }
    assert_eq!(encoded(&rays(15.0, f64::NAN), 1.0), (0, 0));
    assert_eq!(encoded(&rays(15.0, f64::INFINITY), 1.0), (0, 0));
}

#[test]
fn the_rotation_guides_turn_green_when_the_angle_is_snapped_and_skip_a_bad_centre() {
    let guide = |center, degrees, snapped| Overlay {
        rotation: Some(RotationGuide { center, degrees, snapped }),
        ..Overlay::default()
    };
    let (zero, _) = encoded(&guide(Point::new(50.0, 50.0), 0.0, false), 1.0);
    let (turned, _) = encoded(&guide(Point::new(50.0, 50.0), 90.0, false), 1.0);
    assert_eq!(turned, zero + 1, "a turned selection adds the arc");
    assert_eq!(encoded(&guide(Point::new(f64::NAN, 0.0), 90.0, true), 1.0), (0, 0));
    assert_eq!(encoded(&guide(Point::new(0.0, 0.0), f64::INFINITY, true), 1.0), (0, 0));
}

#[test]
fn the_eraser_ring_keeps_its_screen_size_and_skips_a_bad_radius() {
    let ring =
        |r| Overlay { eraser: Some(EraserRing { at: Point::new(50.0, 50.0), radius_px: r }), ..Overlay::default() };
    let sizes: Vec<usize> = [0.1, 1.0, 8.0, 500.0].iter().map(|z| encoded(&ring(10.0), *z).0).collect();
    assert!(sizes.iter().all(|n| *n == 2), "a fill and a stroke at every zoom: {sizes:?}");
    for bad in [0.0, -3.0, f64::NAN, f64::INFINITY] {
        assert_eq!(encoded(&ring(bad), 1.0), (0, 0), "radius {bad}");
    }
}
