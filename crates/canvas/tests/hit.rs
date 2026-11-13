//! Hit testing per element kind, z order, locked and deleted rules, frames, groups and marquee queries.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use common::*;
use drafftink_canvas::geom::kurbo::{Point, Rect};
use drafftink_canvas::geom::lasso::Lasso;
use drafftink_canvas::scene::{Element, ElementKind, FreehandData, GroupId, ImageData, SceneRead, SceneTx};
use drafftink_canvas::{HitOptions, HitPart, MarqueeMode, unit_group};
use smallvec::smallvec;

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn top(c: &drafftink_canvas::Canvas<drafftink_canvas::scene::MemoryStore>, x: f64, y: f64) -> Option<String> {
    c.hit_at(p(x, y)).map(|h| h.id.to_string())
}

#[test]
fn unfilled_shapes_are_hit_on_the_outline_only() {
    let c = canvas_with(vec![
        rect("r", 100.0, 100.0, 200.0, 100.0),
        ellipse("e", 400.0, 100.0, 200.0, 100.0),
        diamond("d", 100.0, 300.0, 200.0, 100.0),
    ]);
    // Rectangle: centre misses, edge hits (stroke width 2 plus 5 px of tolerance).
    assert_eq!(top(&c, 200.0, 150.0), None);
    assert_eq!(top(&c, 100.0, 150.0).as_deref(), Some("r"));
    assert_eq!(top(&c, 106.0, 150.0).as_deref(), Some("r"), "6 px inside: 1 stroke + 5 tolerance");
    assert_eq!(top(&c, 108.0, 150.0), None);
    // Ellipse: the left vertex hits, the box corner is far from the curve.
    assert_eq!(top(&c, 400.0, 150.0).as_deref(), Some("e"));
    assert_eq!(top(&c, 402.0, 102.0), None);
    assert_eq!(top(&c, 500.0, 150.0), None);
    // Diamond: its left vertex hits, its box corner does not.
    assert_eq!(top(&c, 100.0, 350.0).as_deref(), Some("d"));
    assert_eq!(top(&c, 102.0, 302.0), None);
    assert_eq!(top(&c, 200.0, 300.0).as_deref(), Some("d"));
}

#[test]
fn filled_shapes_are_hit_inside() {
    let c = canvas_with(vec![
        fill(rect("r", 100.0, 100.0, 200.0, 100.0)),
        fill(ellipse("e", 400.0, 100.0, 200.0, 100.0)),
        fill(diamond("d", 100.0, 300.0, 200.0, 100.0)),
    ]);
    assert_eq!(c.hit_at(p(200.0, 150.0)).map(|h| h.part), Some(HitPart::Fill));
    assert_eq!(c.hit_at(p(100.0, 150.0)).map(|h| h.part), Some(HitPart::Stroke));
    assert_eq!(top(&c, 500.0, 150.0).as_deref(), Some("e"));
    assert_eq!(top(&c, 410.0, 110.0), None, "inside the ellipse's box but outside the ellipse");
    assert_eq!(top(&c, 200.0, 350.0).as_deref(), Some("d"));
    assert_eq!(top(&c, 110.0, 310.0), None, "inside the diamond's box but outside the diamond");
}

#[test]
fn opacity_zero_changes_nothing_about_what_a_click_finds() {
    let hide = |mut e: drafftink_canvas::scene::Element| {
        e.style.opacity = 0.0;
        e
    };
    let c = canvas_with(vec![
        hide(fill(rect("filled", 100.0, 100.0, 200.0, 100.0))),
        hide(rect("outlined", 400.0, 100.0, 200.0, 100.0)),
        hide(text("words", 100.0, 300.0, 100.0, 30.0)),
    ]);
    assert_eq!(c.hit_at(p(200.0, 150.0)).map(|h| (h.id.to_string(), h.part)), Some(("filled".into(), HitPart::Fill)));
    assert_eq!(top(&c, 400.0, 150.0).as_deref(), Some("outlined"), "its outline is hit");
    assert_eq!(top(&c, 500.0, 150.0), None, "an unfilled shape has no inside, visible or not");
    assert_eq!(top(&c, 150.0, 315.0).as_deref(), Some("words"), "text is hit anywhere in its box");
    let ids = c.index().hit_all(c.store(), p(200.0, 150.0), 1.0, &HitOptions::default());
    assert_eq!(ids.len(), 1);
    let picked = c.index().marquee(c.store(), &Rect::new(0.0, 0.0, 700.0, 400.0), MarqueeMode::Contain, false);
    assert_eq!(picked.len(), 3, "a marquee takes them too");
}

#[test]
fn a_shape_with_no_stroke_and_no_fill_is_hit_on_its_outline() {
    let mut e = rect("ghost", 100.0, 100.0, 200.0, 100.0);
    e.style.stroke_color = drafftink_canvas::scene::Color::TRANSPARENT;
    let c = canvas_with(vec![e]);
    assert_eq!(top(&c, 100.0, 150.0).as_deref(), Some("ghost"));
    assert_eq!(top(&c, 200.0, 150.0), None);
}

#[test]
fn rotated_shapes_follow_the_rotation() {
    // A 200x20 bar turned a quarter: now vertical, occupying x 190..210 and y 40..240 around centre (200, 140).
    let mut bar = fill(rect("bar", 100.0, 130.0, 200.0, 20.0));
    bar.transform.angle = FRAC_PI_2;
    let c = canvas_with(vec![bar]);
    assert_eq!(top(&c, 200.0, 60.0).as_deref(), Some("bar"), "the top end of the turned bar");
    assert_eq!(top(&c, 120.0, 140.0), None, "where the unrotated bar used to be");
    assert_eq!(top(&c, 200.0, 250.0), None, "beyond its end");

    // A filled square at 45 degrees is a diamond: its box corner is outside, its edge middle is inside.
    let mut sq = fill(rect("sq", 100.0, 100.0, 100.0, 100.0));
    sq.transform.angle = FRAC_PI_4;
    let c = canvas_with(vec![sq]);
    assert_eq!(top(&c, 105.0, 105.0), None, "the unrotated top-left corner is now outside");
    assert_eq!(top(&c, 150.0, 90.0).as_deref(), Some("sq"), "the rotated top vertex is at y = 150 - 70.7");
    assert_eq!(top(&c, 150.0, 150.0).as_deref(), Some("sq"));

    // A filled ellipse turned a quarter swaps its axes.
    let mut el = fill(ellipse("el", 100.0, 100.0, 200.0, 40.0));
    el.transform.angle = FRAC_PI_2;
    let c = canvas_with(vec![el]);
    assert_eq!(top(&c, 200.0, 60.0).as_deref(), Some("el"));
    assert_eq!(top(&c, 120.0, 120.0), None);
}

#[test]
fn flipped_elements_hit_like_unflipped_ones() {
    let mut img = Element::new(
        id("img"),
        ElementKind::Image(ImageData { file_id: None, natural_width: 10, natural_height: 10, crop: None }),
        drafftink_canvas::scene::ZIndex::first(),
    )
    .with_bounds(100.0, 100.0, 200.0, 50.0);
    img.transform.flip_x = true;
    img.transform.flip_y = true;
    img.transform.angle = FRAC_PI_4;
    let c = canvas_with(vec![img]);
    // Centre (200, 125); rotated 45 degrees, the box still contains its centre and the point 90 along its axis.
    assert_eq!(top(&c, 200.0, 125.0).as_deref(), Some("img"));
    let along = 90.0 * FRAC_PI_4.cos();
    assert_eq!(top(&c, 200.0 + along, 125.0 + along).as_deref(), Some("img"));
    assert_eq!(top(&c, 200.0 + along, 125.0 - along), None);
}

#[test]
fn lines_are_hit_near_the_stroke_and_the_tolerance_is_in_screen_pixels() {
    let mut c = canvas_with(vec![line("l", &[(100.0, 100.0), (300.0, 100.0), (300.0, 300.0)])]);
    assert_eq!(top(&c, 200.0, 100.0).as_deref(), Some("l"));
    assert_eq!(top(&c, 200.0, 105.0).as_deref(), Some("l"), "1 stroke + 5 px tolerance at zoom 1");
    assert_eq!(top(&c, 200.0, 107.0), None);
    assert_eq!(top(&c, 300.0, 200.0).as_deref(), Some("l"), "second segment");
    assert_eq!(top(&c, 200.0, 200.0), None, "inside the bend");
    // Zoomed out to 0.5 a screen pixel is two world units, so the world reach doubles for the tolerance part.
    c.zoom_at(p(400.0, 300.0), 0.5);
    let near = c.viewport().world_to_screen(p(200.0, 100.0 + 10.5));
    assert_eq!(
        c.hit_at(near).map(|h| h.id.to_string()).as_deref(),
        Some("l"),
        "10.5 world = 5.25 px - within 5 + 1 px"
    );
    let far = c.viewport().world_to_screen(p(200.0, 100.0 + 14.0));
    assert_eq!(c.hit_at(far), None);
    // Zoomed in to 4 the same pixel tolerance is a quarter of a world unit.
    c.zoom_at(p(400.0, 300.0), 8.0);
    let near = c.viewport().world_to_screen(p(200.0, 100.0 + 2.0));
    assert_eq!(
        c.hit_at(near).map(|h| h.id.to_string()).as_deref(),
        Some("l"),
        "2 world = 8 px - 4 px of stroke is 5 + 4"
    );
    let far = c.viewport().world_to_screen(p(200.0, 100.0 + 3.0));
    assert_eq!(c.hit_at(far), None);
}

#[test]
fn a_thick_stroke_is_hit_over_its_full_width() {
    let mut l = line("l", &[(100.0, 100.0), (300.0, 100.0)]);
    l.style.stroke_width = 40.0;
    let c = canvas_with(vec![l]);
    assert_eq!(top(&c, 200.0, 118.0).as_deref(), Some("l"));
    assert_eq!(top(&c, 200.0, 130.0), None);
}

#[test]
fn closed_lines_and_freehand_are_hit_inside_only_when_filled() {
    let tri = [(100.0, 100.0), (300.0, 100.0), (200.0, 300.0)];
    let mut closed = line("tri", &tri);
    if let ElementKind::Line(d) = &mut closed.kind {
        d.closed = true;
    }
    let mut open = line("open", &tri);
    open.transform.x += 400.0;
    let mut filled = fill(closed.clone());
    filled.id = id("filled");
    filled.transform.y += 300.0;
    let c = canvas_with(vec![closed, open, filled]);
    assert_eq!(top(&c, 200.0, 150.0), None, "closed but unfilled");
    assert_eq!(top(&c, 600.0, 150.0), None, "open");
    assert_eq!(top(&c, 200.0, 450.0).as_deref(), Some("filled"));
    assert_eq!(top(&c, 200.0, 100.0).as_deref(), Some("tri"), "its top edge");

    let mut fh = freehand();
    fh.transform.x = 100.0;
    fh.transform.y = 100.0;
    let c = canvas_with(vec![fh]);
    assert_eq!(top(&c, 150.0, 100.0).as_deref(), Some("fh"));
    assert_eq!(top(&c, 150.0, 150.0), None);
}

fn freehand() -> Element {
    let mut e = Element::new(
        id("fh"),
        ElementKind::Freehand(FreehandData::new(vec![p(0.0, 0.0), p(100.0, 0.0), p(100.0, 100.0)])),
        drafftink_canvas::scene::ZIndex::first(),
    );
    e.refit_points().unwrap();
    e
}

#[test]
fn text_is_hit_anywhere_in_its_box() {
    let c = canvas_with(vec![text("t", 100.0, 100.0, 120.0, 30.0)]);
    assert_eq!(c.hit_at(p(160.0, 115.0)).map(|h| h.part), Some(HitPart::Body));
    assert_eq!(top(&c, 99.0, 115.0).as_deref(), Some("t"), "tolerance around the border");
    assert_eq!(top(&c, 150.0, 140.0), None);
}

#[test]
fn z_order_decides_and_hit_all_lists_topmost_first() {
    let mut c = canvas_with(vec![
        fill(rect("bottom", 100.0, 100.0, 200.0, 200.0)),
        fill(rect("middle", 150.0, 150.0, 200.0, 200.0)),
        fill(rect("top", 200.0, 200.0, 200.0, 200.0)),
    ]);
    assert_eq!(top(&c, 250.0, 250.0).as_deref(), Some("top"));
    assert_eq!(top(&c, 170.0, 170.0).as_deref(), Some("middle"));
    assert_eq!(top(&c, 110.0, 110.0).as_deref(), Some("bottom"));
    let all: Vec<String> = c.hit_all_at(p(250.0, 250.0)).into_iter().map(|h| h.id.to_string()).collect();
    assert_eq!(all, ["top", "middle", "bottom"]);
    // Bring the bottom one to the front: the index key changes, the geometry does not.
    c.transact(|tx| {
        let z = tx.next_top_index().unwrap();
        tx.update(&id("bottom"), |e| e.index = z)
    })
    .unwrap();
    assert_eq!(top(&c, 250.0, 250.0).as_deref(), Some("bottom"));
    let all: Vec<String> = c.hit_all_at(p(250.0, 250.0)).into_iter().map(|h| h.id.to_string()).collect();
    assert_eq!(all, ["bottom", "top", "middle"]);
}

#[test]
fn locked_and_deleted_elements_are_not_hit() {
    let mut c = canvas_with(vec![
        fill(rect("under", 100.0, 100.0, 200.0, 200.0)),
        fill(rect("over", 100.0, 100.0, 200.0, 200.0)),
    ]);
    assert_eq!(top(&c, 200.0, 200.0).as_deref(), Some("over"));
    c.transact(|tx| tx.update(&id("over"), |e| e.locked = true)).unwrap();
    assert_eq!(top(&c, 200.0, 200.0).as_deref(), Some("under"), "a locked element lets the click through");
    let opts = HitOptions { include_locked: true, ..HitOptions::default() };
    assert_eq!(c.hit_with(p(200.0, 200.0), &opts).map(|h| h.id.to_string()).as_deref(), Some("over"));
    c.transact(|tx| tx.update(&id("under"), |e| e.deleted = true)).unwrap();
    assert_eq!(top(&c, 200.0, 200.0), None);
    assert!(!c.index().contains(&id("under")), "tombstones leave the index");
    c.transact(|tx| tx.update(&id("under"), |e| e.deleted = false)).unwrap();
    c.transact(|tx| tx.update(&id("over"), |e| e.locked = false)).unwrap();
    assert!(c.index().contains(&id("under")), "undelete brings it back");
    assert_eq!(top(&c, 200.0, 200.0).as_deref(), Some("over"));
}

#[test]
fn filter_hides_elements_from_a_query() {
    let c = canvas_with(vec![fill(rect("a", 0.0, 0.0, 100.0, 100.0)), fill(rect("b", 0.0, 0.0, 100.0, 100.0))]);
    let skip_b = |e: &drafftink_canvas::scene::Element| e.id.as_str() != "b";
    let opts = HitOptions { filter: Some(&skip_b), ..HitOptions::default() };
    assert_eq!(c.hit_with(p(50.0, 50.0), &opts).map(|h| h.id.to_string()).as_deref(), Some("a"));
}

#[test]
fn frames_are_hit_on_their_border_and_clip_their_children() {
    let mut child_in = fill(rect("child", 150.0, 150.0, 400.0, 100.0));
    child_in.frame_id = Some(id("f"));
    let c = canvas_with(vec![frame("f", 100.0, 100.0, 300.0, 300.0), child_in]);
    assert_eq!(top(&c, 120.0, 300.0), None, "inside an empty part of the frame");
    assert_eq!(top(&c, 100.0, 300.0).as_deref(), Some("f"), "the border");
    assert_eq!(top(&c, 200.0, 200.0).as_deref(), Some("child"));
    assert_eq!(top(&c, 500.0, 200.0), None, "the part of the child outside the clipping frame is invisible");
    assert_eq!(top(&c, 399.0, 200.0).as_deref(), Some("child"));
    // The same child without a frame is hit outside.
    let free = canvas_with(vec![fill(rect("child", 150.0, 150.0, 400.0, 100.0))]);
    assert_eq!(top(&free, 500.0, 200.0).as_deref(), Some("child"));
}

#[test]
fn non_clipping_and_deleted_frames_do_not_clip() {
    let mut f = frame("f", 100.0, 100.0, 300.0, 300.0);
    if let ElementKind::Frame(d) = &mut f.kind {
        d.clip = false;
    }
    let mut child = fill(rect("child", 150.0, 150.0, 400.0, 100.0));
    child.frame_id = Some(id("f"));
    let c = canvas_with(vec![f, child.clone()]);
    assert_eq!(top(&c, 500.0, 200.0).as_deref(), Some("child"));
    let mut f2 = frame("f", 100.0, 100.0, 300.0, 300.0);
    f2.deleted = true;
    let c = canvas_with(vec![f2, child]);
    assert_eq!(top(&c, 500.0, 200.0).as_deref(), Some("child"));
}

#[test]
fn unit_group_resolves_the_outermost_group_or_the_next_one_inside_the_entered_group() {
    let (g1, g2) = (GroupId::new("g1").unwrap(), GroupId::new("g2").unwrap());
    let mut e = rect("a", 0.0, 0.0, 1.0, 1.0);
    e.group_ids = smallvec![g1.clone(), g2.clone()]; // innermost first
    let always = |_: &GroupId| true;
    assert_eq!(unit_group(&e, None, always), Some(g2.clone()));
    assert_eq!(
        unit_group(&e, Some(&g2), always),
        Some(g1.clone()),
        "entered the outer group: the inner one is the unit"
    );
    assert_eq!(unit_group(&e, Some(&g1), always), None, "entered the innermost group: the element itself");
    let other = GroupId::new("other").unwrap();
    assert_eq!(unit_group(&e, Some(&other), always), Some(g2.clone()), "not inside the entered group");
    assert_eq!(unit_group(&e, None, |g| g != &g2), Some(g1), "a group that no longer counts is skipped");
    assert_eq!(unit_group(&rect("b", 0.0, 0.0, 1.0, 1.0), None, always), None);
}

#[test]
fn pick_reports_the_unit_group_with_the_entered_group() {
    let (g1, g2) = (GroupId::new("g1").unwrap(), GroupId::new("g2").unwrap());
    let mut a = fill(rect("a", 100.0, 100.0, 100.0, 100.0));
    a.group_ids = smallvec![g1.clone(), g2.clone()];
    let mut b = fill(rect("b", 300.0, 100.0, 100.0, 100.0));
    b.group_ids = smallvec![g1.clone(), g2.clone()];
    let mut c = canvas_with(vec![a, b]);
    let unit = |c: &drafftink_canvas::Canvas<_>| match c.pick(p(150.0, 150.0), drafftink_canvas::PointerKind::Mouse) {
        drafftink_canvas::Target::Element { unit_group, .. } => unit_group,
        other => panic!("{other:?}"),
    };
    assert_eq!(unit(&c), Some(g2.clone()));
    c.set_entered_group(Some(g2.clone()));
    assert_eq!(unit(&c), Some(g1.clone()));
    c.set_entered_group(Some(g1));
    assert_eq!(unit(&c), None);
}

#[test]
fn marquee_contain_versus_intersect() {
    let c = canvas_with(vec![
        rect("hollow", 100.0, 100.0, 200.0, 200.0),
        fill(rect("solid", 400.0, 100.0, 200.0, 200.0)),
        frame("fr", 100.0, 400.0, 200.0, 150.0),
        text("txt", 400.0, 400.0, 100.0, 30.0),
    ]);
    let names = |v: Vec<drafftink_canvas::scene::ElementId>| v.into_iter().map(|i| i.to_string()).collect::<Vec<_>>();
    // Inside the hollow rectangle without touching its outline: nothing. Inside the solid one: it is selected.
    let inside = Rect::new(150.0, 150.0, 250.0, 250.0);
    assert!(c.marquee(&inside, MarqueeMode::Intersect).is_empty());
    let inside_solid = Rect::new(450.0, 150.0, 550.0, 250.0);
    assert_eq!(names(c.marquee(&inside_solid, MarqueeMode::Intersect)), ["solid"]);
    assert!(c.marquee(&inside_solid, MarqueeMode::Contain).is_empty());
    // Crossing the hollow outline selects it.
    let crossing = Rect::new(50.0, 50.0, 150.0, 150.0);
    assert_eq!(names(c.marquee(&crossing, MarqueeMode::Intersect)), ["hollow"]);
    assert!(c.marquee(&crossing, MarqueeMode::Contain).is_empty());
    // Containing everything.
    let all = Rect::new(0.0, 0.0, 700.0, 700.0);
    assert_eq!(names(c.marquee(&all, MarqueeMode::Contain)), ["hollow", "solid", "fr", "txt"]);
    // A frame is selected by a marquee only when contained, never by crossing its border.
    let across_frame = Rect::new(150.0, 380.0, 250.0, 450.0);
    assert!(c.marquee(&across_frame, MarqueeMode::Intersect).is_empty());
    assert_eq!(names(c.marquee(&Rect::new(90.0, 390.0, 310.0, 560.0), MarqueeMode::Intersect)), ["fr"]);
    // Text is a whole box.
    assert_eq!(names(c.marquee(&Rect::new(420.0, 405.0, 430.0, 415.0), MarqueeMode::Intersect)), ["txt"]);
    // Reversed rectangles and hostile input.
    assert_eq!(c.marquee(&Rect::new(700.0, 700.0, 0.0, 0.0), MarqueeMode::Contain).len(), 4);
    assert!(c.marquee(&Rect::new(f64::NAN, 0.0, 1.0, 1.0), MarqueeMode::Contain).is_empty());
}

#[test]
fn marquee_skips_locked_and_deleted() {
    let mut c = canvas_with(vec![
        fill(rect("a", 0.0, 0.0, 10.0, 10.0)),
        fill(rect("b", 20.0, 0.0, 10.0, 10.0)),
        fill(rect("c", 40.0, 0.0, 10.0, 10.0)),
    ]);
    c.transact(|tx| {
        tx.update(&id("a"), |e| e.locked = true)?;
        tx.update(&id("b"), |e| e.deleted = true)
    })
    .unwrap();
    let r = c.marquee(&Rect::new(-5.0, -5.0, 100.0, 20.0), MarqueeMode::Contain);
    assert_eq!(r, vec![id("c")]);
    assert_eq!(live_ids(c.store()).len(), 2);
}

fn lasso(pts: &[(f64, f64)]) -> Lasso {
    let pts: Vec<Point> = pts.iter().map(|&(x, y)| p(x, y)).collect();
    Lasso::new(&pts, 0.0).unwrap()
}

#[test]
fn lasso_contain_versus_intersect_follows_the_marquee_rules() {
    let c = canvas_with(vec![
        rect("hollow", 100.0, 100.0, 200.0, 200.0),
        fill(rect("solid", 400.0, 100.0, 200.0, 200.0)),
        frame("fr", 100.0, 400.0, 200.0, 150.0),
        text("txt", 400.0, 400.0, 100.0, 30.0),
    ]);
    let names = |v: Vec<drafftink_canvas::scene::ElementId>| v.into_iter().map(|i| i.to_string()).collect::<Vec<_>>();
    // A triangle over the top left: it holds the hollow rectangle, cuts the solid one and misses the rest.
    let top_half = lasso(&[(50.0, 50.0), (900.0, 50.0), (50.0, 600.0)]);
    assert_eq!(names(c.lasso(&top_half, MarqueeMode::Contain)), ["hollow"], "the slanted side cuts the solid one");
    assert_eq!(names(c.lasso(&top_half, MarqueeMode::Intersect)), ["hollow", "solid"]);
    // Inside the hollow rectangle without touching its outline: nothing. Inside the solid one: it is selected.
    let in_hollow = lasso(&[(150.0, 150.0), (250.0, 150.0), (200.0, 250.0)]);
    assert!(c.lasso(&in_hollow, MarqueeMode::Intersect).is_empty());
    let in_solid = lasso(&[(450.0, 150.0), (550.0, 150.0), (500.0, 250.0)]);
    assert_eq!(names(c.lasso(&in_solid, MarqueeMode::Intersect)), ["solid"]);
    assert!(c.lasso(&in_solid, MarqueeMode::Contain).is_empty());
    // A frame is selected only when contained, never by crossing its border.
    let across_frame = lasso(&[(150.0, 380.0), (250.0, 380.0), (200.0, 450.0)]);
    assert!(c.lasso(&across_frame, MarqueeMode::Intersect).is_empty());
    let around_frame = lasso(&[(90.0, 390.0), (310.0, 390.0), (310.0, 560.0), (90.0, 560.0)]);
    assert_eq!(names(c.lasso(&around_frame, MarqueeMode::Intersect)), ["fr"]);
    // Text is a whole box.
    assert_eq!(
        names(c.lasso(&lasso(&[(420.0, 405.0), (430.0, 405.0), (425.0, 415.0)]), MarqueeMode::Intersect)),
        ["txt"]
    );
}

#[test]
fn lasso_skips_locked_and_deleted() {
    let mut c = canvas_with(vec![
        fill(rect("a", 0.0, 0.0, 10.0, 10.0)),
        fill(rect("b", 20.0, 0.0, 10.0, 10.0)),
        fill(rect("c", 40.0, 0.0, 10.0, 10.0)),
    ]);
    c.transact(|tx| {
        tx.update(&id("a"), |e| e.locked = true)?;
        tx.update(&id("b"), |e| e.deleted = true)
    })
    .unwrap();
    let all = lasso(&[(-5.0, -5.0), (100.0, -5.0), (100.0, 20.0), (-5.0, 20.0)]);
    assert_eq!(c.lasso(&all, MarqueeMode::Contain), vec![id("c")]);
}

#[test]
fn degenerate_elements_never_panic_a_query() {
    let mut zero = rect("zero", 100.0, 100.0, 0.0, 0.0);
    zero.style.fill_color = Some(drafftink_canvas::scene::Color::BLACK);
    let c = canvas_with(vec![zero, text("empty", 200.0, 200.0, 0.0, 0.0)]);
    let _ = c.hit_at(p(100.0, 100.0));
    let _ = c.hit_at(p(f64::NAN, 1.0));
    let _ = c.hit_all_at(p(f64::INFINITY, 1.0));
    let _ = c.marquee(&Rect::new(0.0, 0.0, 1e300, 1e300), MarqueeMode::Intersect);
    assert!(c.store().get(&id("zero")).is_some());
}
