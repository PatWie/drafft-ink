//! The conversion copies every part the render pass can draw: the lasso, link icons, and what other people show.

use drafftink_canvas::{
    LassoOverlay, LinkIcon, Overlay as CanvasOverlay, RemoteCursor, RemoteParts, RemoteSelection, RemoteTrail,
    TrailPoint,
};
use drafftink_geom::OrientedBox;
use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_scene::{Color, ElementId};

use drafftink_engine::interact::{AngleRays, Badge, EraserRing, InteractionOverlay};

use super::{add_interaction, to_render};

fn id(text: &str) -> ElementId {
    ElementId::new(text).expect("a valid id")
}

fn red() -> Color {
    Color::rgb(220, 30, 30)
}

#[test]
fn an_empty_description_gives_an_empty_overlay() {
    let out = to_render(&CanvasOverlay::default(), &RemoteParts::default(), &[]);
    assert!(out.is_empty());
}

#[test]
fn the_lasso_path_is_copied_in_drawing_order() {
    let points = vec![Point::new(0.0, 0.0), Point::new(40.0, 5.0), Point::new(30.0, 60.0)];
    let canvas = CanvasOverlay { lasso: Some(LassoOverlay { points: points.clone() }), ..CanvasOverlay::default() };
    let out = to_render(&canvas, &RemoteParts::default(), &[]);
    assert_eq!(out.lasso, points);
}

#[test]
fn link_icons_keep_their_rectangle_and_kind() {
    let web = LinkIcon { element: id("a"), rect: Rect::new(10.0, 0.0, 26.0, 16.0), to_element: false };
    let element = LinkIcon { element: id("b"), rect: Rect::new(50.0, 0.0, 66.0, 16.0), to_element: true };
    let canvas = CanvasOverlay { link_icons: vec![web.clone(), element.clone()], ..CanvasOverlay::default() };
    let out = to_render(&canvas, &RemoteParts::default(), &[]);
    let marks: Vec<_> = out.link_icons.iter().map(|m| (m.rect, m.to_element)).collect();
    assert_eq!(marks, vec![(web.rect, false), (element.rect, true)]);
}

#[test]
fn remote_cursors_selections_and_trails_reach_the_render_overlay() {
    let boxes = vec![OrientedBox::from_rect(Rect::new(0.0, 0.0, 100.0, 50.0))];
    let remote = RemoteParts {
        cursors: vec![
            RemoteCursor { user: "ana".into(), name: Some("Ana".into()), color: red(), pos: Point::new(5.0, 6.0) },
            RemoteCursor { user: "x".into(), name: Some(String::new()), color: red(), pos: Point::new(7.0, 8.0) },
        ],
        selections: vec![RemoteSelection { user: "ana".into(), color: red(), boxes: boxes.clone(), truncated: false }],
        trails: vec![RemoteTrail {
            user: "ana".into(),
            color: red(),
            points: vec![
                TrailPoint { pos: Point::new(1.0, 1.0), age_ms: 900.0 },
                TrailPoint { pos: Point::new(2.0, 2.0), age_ms: 100.0 },
            ],
        }],
    };
    let out = to_render(&CanvasOverlay::default(), &remote, &[]);

    let cursors: Vec<_> = out.cursors.iter().map(|c| (c.at, c.name.as_deref())).collect();
    assert_eq!(cursors, vec![(Point::new(5.0, 6.0), Some("Ana")), (Point::new(7.0, 8.0), None)]);
    assert_eq!(out.cursors[0].color, red().into());

    assert_eq!(out.remote_selections.len(), 1);
    assert_eq!(out.remote_selections[0].boxes, boxes);
    assert_eq!(out.remote_selections[0].color, red().into());

    assert_eq!(out.trails.len(), 1);
    let ages: Vec<_> = out.trails[0].points.iter().map(|p| (p.at, p.age_ms)).collect();
    assert_eq!(ages, vec![(Point::new(1.0, 1.0), 900.0), (Point::new(2.0, 2.0), 100.0)]);
}

#[test]
fn the_remote_selection_box_keeps_its_turn() {
    let turned = OrientedBox::new(Point::new(10.0, 10.0), Size::new(20.0, 10.0), 0.5);
    let remote = RemoteParts {
        selections: vec![RemoteSelection { user: "u".into(), color: red(), boxes: vec![turned], truncated: false }],
        ..RemoteParts::default()
    };
    let out = to_render(&CanvasOverlay::default(), &remote, &[]);
    assert_eq!(out.remote_selections[0].boxes[0].angle, 0.5);
}

#[test]
fn rays_the_snap_crosshair_and_the_eraser_ring_reach_the_render_overlay() {
    let interaction = InteractionOverlay {
        angle_rays: Some(AngleRays { origin: Point::new(1.0, 2.0), step_degrees: 15.0, locked_degrees: 45.0 }),
        snap_point: Some(Point::new(3.0, 4.0)),
        eraser: Some(EraserRing { at: Point::new(5.0, 6.0), radius_px: 10.0 }),
        ..InteractionOverlay::default()
    };
    let mut out = to_render(&CanvasOverlay::default(), &RemoteParts::default(), &[]);
    add_interaction(&mut out, &interaction);
    let rays = out.angle_rays.expect("rays");
    assert_eq!((rays.origin, rays.step_degrees, rays.locked_degrees), (Point::new(1.0, 2.0), 15.0, 45.0));
    assert_eq!(out.snap_point, Some(Point::new(3.0, 4.0)));
    let ring = out.eraser.expect("ring");
    assert_eq!((ring.at, ring.radius_px), (Point::new(5.0, 6.0), 10.0));
    assert!(out.rotation.is_none());
}

#[test]
fn an_angle_badge_becomes_the_rotation_guides_and_a_size_badge_does_not() {
    let angle = InteractionOverlay {
        badge: Some(Badge::Angle { center: Point::new(7.0, 8.0), degrees: 30.0, snapped: true }),
        ..InteractionOverlay::default()
    };
    let mut out = to_render(&CanvasOverlay::default(), &RemoteParts::default(), &[]);
    add_interaction(&mut out, &angle);
    let guide = out.rotation.expect("rotation guides");
    assert_eq!((guide.center, guide.degrees, guide.snapped), (Point::new(7.0, 8.0), 30.0, true));

    let size = InteractionOverlay { badge: Some(Badge::Size { w: 10.0, h: 20.0 }), ..InteractionOverlay::default() };
    let mut out = to_render(&CanvasOverlay::default(), &RemoteParts::default(), &[]);
    add_interaction(&mut out, &size);
    assert!(out.rotation.is_none());
}

#[test]
fn an_idle_interaction_adds_nothing() {
    let mut out = to_render(&CanvasOverlay::default(), &RemoteParts::default(), &[]);
    add_interaction(&mut out, &InteractionOverlay::default());
    assert!(out.is_empty());
}
