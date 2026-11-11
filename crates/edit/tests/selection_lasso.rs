#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Lasso selection: the marquee rules with a free-hand outline (frames, groups, locked elements, bound text,
//! the modes replace and add, touch and contain).

mod structure_support;

use std::collections::BTreeSet;

use drafftink_edit::group::selection::{RegionMode, RegionOpts, Selection, select_lasso, select_region};
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_geom::lasso::Lasso;
use drafftink_geom::select::MarqueeMode;
use drafftink_scene::{Color, Element, ElementId, GroupId};
use structure_support::*;

fn set(names: &[&str]) -> BTreeSet<ElementId> {
    names.iter().map(|n| id(n)).collect()
}

fn gset(names: &[&str]) -> BTreeSet<GroupId> {
    names.iter().map(|n| gid(n)).collect()
}

fn filled(mut e: Element) -> Element {
    e.style.fill_color = Some(Color::rgb(200, 0, 0));
    e
}

fn lasso(pts: &[(f64, f64)]) -> Lasso {
    let pts: Vec<Point> = pts.iter().map(|&(x, y)| Point::new(x, y)).collect();
    Lasso::new(&pts, 0.0).unwrap()
}

fn contain() -> RegionOpts {
    RegionOpts { contain: MarqueeMode::Contain, ..RegionOpts::default() }
}

/// A triangle that covers the top-left half of a 200 by 200 square.
fn triangle() -> Lasso {
    lasso(&[(-10.0, -10.0), (210.0, -10.0), (-10.0, 210.0)])
}

#[test]
fn a_lasso_picks_what_its_outline_surrounds_and_not_what_is_beyond_the_slanted_side() {
    let s = scene(vec![filled(rect("near", 10.0, 10.0, 30.0, 30.0)), filled(rect("far", 150.0, 150.0, 30.0, 30.0))]);
    let sel = select_lasso(&s, &Selection::default(), &triangle(), contain());
    assert_eq!(sel.ids, set(&["near"]));
    // The same area as a marquee would select both; that is the point of a lasso.
    let marquee = select_region(&s, &Selection::default(), Rect::new(-10.0, -10.0, 210.0, 210.0), contain());
    assert_eq!(marquee.ids, set(&["near", "far"]));
}

#[test]
fn touch_selects_what_the_outline_crosses_and_contain_does_not() {
    let s = scene(vec![filled(rect("a", 80.0, 80.0, 60.0, 60.0))]);
    let l = triangle();
    let touch = select_lasso(&s, &Selection::default(), &l, RegionOpts::default());
    assert_eq!(touch.ids, set(&["a"]), "the slanted side crosses it");
    let contained = select_lasso(&s, &Selection::default(), &l, contain());
    assert!(contained.ids.is_empty());
}

#[test]
fn a_lasso_inside_a_filled_shape_selects_it_and_inside_an_empty_one_does_not() {
    let s = scene(vec![filled(rect("solid", 0.0, 0.0, 200.0, 200.0)), rect("hollow", 300.0, 0.0, 200.0, 200.0)]);
    let in_solid = select_lasso(
        &s,
        &Selection::default(),
        &lasso(&[(50.0, 50.0), (100.0, 50.0), (75.0, 100.0)]),
        RegionOpts::default(),
    );
    assert_eq!(in_solid.ids, set(&["solid"]));
    let in_hollow = select_lasso(
        &s,
        &Selection::default(),
        &lasso(&[(350.0, 50.0), (400.0, 50.0), (375.0, 100.0)]),
        RegionOpts::default(),
    );
    assert!(in_hollow.ids.is_empty(), "as with the marquee");
}

#[test]
fn one_member_selects_the_whole_group_in_both_modes() {
    let s = scene(vec![
        rect("a", 0.0, 0.0, 20.0, 20.0).grouped(&["G"]),
        rect("b", 300.0, 0.0, 20.0, 20.0).grouped(&["G"]),
        rect("c", 0.0, 100.0, 20.0, 20.0),
    ]);
    let l = lasso(&[(-5.0, -5.0), (40.0, -5.0), (40.0, 40.0), (-5.0, 40.0)]);
    for opts in [RegionOpts::default(), contain()] {
        let sel = select_lasso(&s, &Selection::default(), &l, opts);
        assert_eq!(sel.ids, set(&["a", "b"]));
        assert_eq!(sel.groups, gset(&["G"]));
    }
}

#[test]
fn locked_elements_and_bound_text_are_ignored_unless_locked_ones_are_asked_for() {
    let s = scene(vec![
        rect("open", 0.0, 0.0, 40.0, 40.0),
        rect("lockd", 100.0, 0.0, 40.0, 40.0).locked(),
        rect("box", 200.0, 0.0, 100.0, 50.0),
        label("lab", "box", 210.0, 10.0, 50.0, 20.0),
    ]);
    let l = lasso(&[(-10.0, -10.0), (310.0, -10.0), (310.0, 60.0), (-10.0, 60.0)]);
    let sel = select_lasso(&s, &Selection::default(), &l, contain());
    assert_eq!(sel.ids, set(&["open", "box"]), "the label follows its container and is not picked on its own");
    let with_locked = select_lasso(&s, &Selection::default(), &l, RegionOpts { include_locked: true, ..contain() });
    assert_eq!(with_locked.ids, set(&["open", "lockd", "box"]));
    let over_label = lasso(&[(212.0, 12.0), (230.0, 12.0), (221.0, 28.0)]);
    let only = select_lasso(&s, &Selection::default(), &over_label, RegionOpts::default());
    assert!(only.ids.is_empty(), "inside the hollow container, over its label");
}

#[test]
fn the_children_of_a_selected_frame_are_not_selected_with_it() {
    let s = scene(vec![rect("kid", 5.0, 5.0, 10.0, 10.0).in_frame("f"), frame("f", 0.0, 0.0, 100.0, 100.0)]);
    let all = lasso(&[(-10.0, -10.0), (150.0, -10.0), (150.0, 150.0), (-10.0, 150.0)]);
    let sel = select_lasso(&s, &Selection::default(), &all, contain());
    assert_eq!(sel.ids, set(&["f"]));
    let inner = lasso(&[(1.0, 1.0), (30.0, 1.0), (30.0, 30.0), (1.0, 30.0)]);
    let kid = select_lasso(&s, &Selection::default(), &inner, RegionOpts::default());
    assert_eq!(kid.ids, set(&["kid"]));
}

#[test]
fn add_keeps_the_selection_and_replace_drops_it() {
    let s = scene(vec![rect("a", 0.0, 0.0, 20.0, 20.0), rect("b", 100.0, 0.0, 20.0, 20.0)]);
    let on_a = lasso(&[(-5.0, -5.0), (30.0, -5.0), (30.0, 30.0), (-5.0, 30.0)]);
    let on_b = lasso(&[(95.0, -5.0), (130.0, -5.0), (130.0, 30.0), (95.0, 30.0)]);
    let first = select_lasso(&s, &Selection::default(), &on_a, contain());
    let replaced = select_lasso(&s, &first, &on_b, contain());
    assert_eq!(replaced.ids, set(&["b"]));
    let added = select_lasso(&s, &first, &on_b, RegionOpts { mode: RegionMode::Add, ..contain() });
    assert_eq!(added.ids, set(&["a", "b"]));
}

#[test]
fn a_concave_lasso_leaves_out_what_sits_in_its_notch() {
    let s = scene(vec![filled(rect("arm", 5.0, 10.0, 20.0, 20.0)), filled(rect("notch", 40.0, 10.0, 20.0, 20.0))]);
    let u = lasso(&[
        (0.0, 0.0),
        (30.0, 0.0),
        (30.0, 70.0),
        (70.0, 70.0),
        (70.0, 0.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (0.0, 100.0),
    ]);
    let sel = select_lasso(&s, &Selection::default(), &u, contain());
    assert_eq!(sel.ids, set(&["arm"]));
}

#[test]
fn lines_arrows_and_strokes_are_selected_by_their_path() {
    let s = scene(vec![
        line("inside", &[(10.0, 10.0), (60.0, 30.0)]),
        line("crossing", &[(50.0, 50.0), (400.0, 50.0)]),
        freehand("scribble", &[(20.0, 80.0), (40.0, 90.0), (60.0, 80.0)]),
    ]);
    let l = lasso(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
    let contained = select_lasso(&s, &Selection::default(), &l, contain());
    assert_eq!(contained.ids, set(&["inside", "scribble"]));
    let touched = select_lasso(&s, &Selection::default(), &l, RegionOpts::default());
    assert_eq!(touched.ids, set(&["inside", "crossing", "scribble"]));
}
