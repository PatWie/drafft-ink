#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `selection_cmd` and `query_cmd`: picking many targets, picking a group, the pointer rules of select-at, and
//! the `@name` filter.

mod structure_support;

use std::collections::BTreeSet;

use drafftink_edit::group::selection::*;
use drafftink_edit::query::{AtHit, AtOpts, Query, QueryCtx, QueryError, elements_at};
use drafftink_edit::query_cmd::{ALL_FIELDS, RefConnectedTo, RefQuery, all_matches, hit_members};
use drafftink_edit::selection_cmd::*;
use drafftink_scene::*;
use kurbo::Point;
use structure_support::*;

fn set(names: &[&str]) -> BTreeSet<ElementId> {
    names.iter().map(|n| id(n)).collect()
}

/// x, y in B inside A with z; w free; l locked; frame f with child kid; text t labelled to w.
fn world() -> MemoryStore {
    scene(vec![
        rect("x", 0.0, 0.0, 10.0, 10.0).grouped(&["B", "A"]),
        rect("y", 20.0, 0.0, 10.0, 10.0).grouped(&["B", "A"]),
        rect("z", 40.0, 0.0, 10.0, 10.0).grouped(&["A"]),
        rect("w", 60.0, 0.0, 10.0, 10.0),
        label("lw", "w", 61.0, 1.0, 5.0, 5.0),
        rect("l", 80.0, 0.0, 10.0, 10.0).locked(),
        frame("f", 0.0, 100.0, 100.0, 100.0),
        rect("kid", 5.0, 105.0, 10.0, 10.0).in_frame("f"),
    ])
}

fn opts(mode: ClickMode, deep: bool) -> ClickOpts {
    ClickOpts { mode, deep, include_locked: false }
}

fn states(s: &MemoryStore) -> Vec<Selection> {
    let none = Selection::default();
    let w = click(s, &none, &ClickTarget::Element(id("w")), ClickOpts::default()).unwrap();
    let a = click(s, &none, &ClickTarget::Element(id("x")), ClickOpts::default()).unwrap();
    let editing = enter_group(s, &a, None, Some(&id("x"))).unwrap();
    let deep = click(s, &none, &ClickTarget::Element(id("x")), opts(ClickMode::Replace, true)).unwrap();
    vec![none, w, a, editing, deep]
}

#[test]
fn pick_of_one_target_equals_click_in_every_mode() {
    let s = world();
    for sel in states(&s) {
        for mode in [ClickMode::Replace, ClickMode::Add, ClickMode::Remove, ClickMode::Toggle] {
            for deep in [false, true] {
                for hit in ["x", "y", "z", "w", "lw", "kid", "f", "l"] {
                    for include_locked in [false, true] {
                        let o = ClickOpts { mode, deep, include_locked };
                        let by_click = click(&s, &sel, &ClickTarget::Element(id(hit)), o);
                        let by_pick = pick(&s, &sel, &[id(hit)], o);
                        match (by_click, by_pick) {
                            (Ok(c), Ok(p)) => assert_eq!(
                                p.selection, c,
                                "sel={sel:?} mode={mode:?} deep={deep} hit={hit} include_locked={include_locked}"
                            ),
                            (Err(SelectionError::Locked(ids)), Ok(p)) => {
                                assert_eq!(p.selection, sel, "a skipped target changes nothing");
                                assert_eq!(p.skipped_locked, ids);
                            }
                            (c, p) => panic!("click {c:?} but pick {p:?} for {hit}"),
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn pick_selects_many_units_at_once() {
    let s = world();
    let p = pick(&s, &Selection::default(), &[id("x"), id("w")], ClickOpts::default()).unwrap();
    assert_eq!(p.selection.ids, set(&["x", "y", "z", "w"]));
    assert_eq!(p.selection.groups.len(), 1);
    assert!(p.skipped_locked.is_empty());
}

#[test]
fn pick_toggle_removes_only_when_every_unit_is_selected() {
    let s = world();
    let both = pick(&s, &Selection::default(), &[id("x"), id("w")], ClickOpts::default()).unwrap().selection;
    let t = ClickOpts { mode: ClickMode::Toggle, ..ClickOpts::default() };
    let gone = pick(&s, &both, &[id("x"), id("w")], t).unwrap().selection;
    assert!(gone.ids.is_empty(), "both were selected, both go");
    let w_only = pick(&s, &Selection::default(), &[id("w")], ClickOpts::default()).unwrap().selection;
    let grown = pick(&s, &w_only, &[id("x"), id("w")], t).unwrap().selection;
    assert_eq!(grown.ids, set(&["x", "y", "z", "w"]), "one of them was not selected, so both are added");
}

#[test]
fn pick_skips_locked_and_reports_them() {
    let s = world();
    let p = pick(&s, &Selection::default(), &[id("w"), id("l")], ClickOpts::default()).unwrap();
    assert_eq!(p.selection.ids, set(&["w"]));
    assert_eq!(p.skipped_locked, vec![id("l")]);
    let p = pick(
        &s,
        &Selection::default(),
        &[id("w"), id("l")],
        ClickOpts { include_locked: true, ..ClickOpts::default() },
    )
    .unwrap();
    assert_eq!(p.selection.ids, set(&["w", "l"]));
    assert!(p.skipped_locked.is_empty());
}

#[test]
fn pick_of_only_locked_targets_keeps_the_selection() {
    let s = world();
    let w = pick(&s, &Selection::default(), &[id("w")], ClickOpts::default()).unwrap().selection;
    let p = pick(&s, &w, &[id("l")], ClickOpts::default()).unwrap();
    assert_eq!(p.selection, w, "replace with nothing usable must not clear the selection");
    assert_eq!(p.skipped_locked, vec![id("l")]);
}

#[test]
fn pick_refuses_unknown_and_deleted_ids() {
    let s = world();
    let e = pick(&s, &Selection::default(), &[id("w"), id("nope")], ClickOpts::default()).unwrap_err();
    assert_eq!(e, SelectionError::NotFound(id("nope")));
}

#[test]
fn pick_of_a_label_and_a_frame_child_follows_the_rules_of_click() {
    let s = world();
    let p = pick(&s, &Selection::default(), &[id("lw")], ClickOpts::default()).unwrap();
    assert_eq!(p.selection.ids, set(&["w"]), "bound text is its container");
    let p = pick(&s, &Selection::default(), &[id("f"), id("kid")], ClickOpts::default()).unwrap();
    assert_eq!(p.selection.ids, set(&["f"]), "a selected frame excludes its children");
}

#[test]
fn pick_deep_enters_the_shared_group() {
    let s = world();
    let p = pick(&s, &Selection::default(), &[id("x"), id("y")], opts(ClickMode::Replace, true)).unwrap();
    assert_eq!(p.selection.ids, set(&["x", "y"]));
    assert_eq!(p.selection.editing_group, Some(gid("B")));
    // z's innermost group is A, x's is B: no single innermost group is shared (the rule of a deep click).
    let p = pick(&s, &Selection::default(), &[id("x"), id("z")], opts(ClickMode::Replace, true)).unwrap();
    assert_eq!(p.selection.editing_group, None);
    let p = pick(&s, &Selection::default(), &[id("z")], opts(ClickMode::Replace, true)).unwrap();
    assert_eq!(p.selection.editing_group, Some(gid("A")));
    let p = pick(&s, &Selection::default(), &[id("x"), id("w")], opts(ClickMode::Replace, true)).unwrap();
    assert_eq!(p.selection.editing_group, None, "no group is shared");
}

#[test]
fn pick_group_selects_all_members_and_refuses_a_non_group() {
    let s = world();
    let p = pick_group(&s, &Selection::default(), &gid("B"), ClickOpts::default()).unwrap();
    assert_eq!(p.selection.ids, set(&["x", "y", "z"]), "without deep the outermost group is the unit");
    let p = pick_group(&s, &Selection::default(), &gid("B"), opts(ClickMode::Replace, true)).unwrap();
    assert_eq!(p.selection.ids, set(&["x", "y"]));
    assert_eq!(p.selection.editing_group, Some(gid("B")));
    assert_eq!(
        pick_group(&s, &Selection::default(), &gid("nope"), ClickOpts::default()).unwrap_err(),
        SelectionError::NoGroup
    );
}

fn hits(s: &MemoryStore, x: f64, y: f64, all: bool) -> Vec<AtHit> {
    elements_at(s, Point::new(x, y), &AtOpts { all, ..AtOpts::default() })
}

#[test]
fn click_target_is_the_top_hit_or_empty() {
    let s = world();
    let none = Selection::default();
    let h = hits(&s, 5.0, 5.0, false);
    assert_eq!(click_target(&s, &none, &h, Point::new(5.0, 5.0), false, false), ClickTarget::Element(id("x")));
    let h = hits(&s, 500.0, 500.0, false);
    assert_eq!(
        click_target(&s, &none, &h, Point::new(500.0, 500.0), false, false),
        ClickTarget::Empty { inside_editing_group: false }
    );
}

#[test]
fn an_empty_click_inside_the_editing_group_bounds_is_flagged() {
    let s = world();
    let a = click(&s, &Selection::default(), &ClickTarget::Element(id("x")), ClickOpts::default()).unwrap();
    let sel = enter_group(&s, &a, None, Some(&id("x"))).unwrap();
    // The gap between y and z lies inside A's bounds (0..50) and hits nothing.
    let p = Point::new(35.0, 5.0);
    let h = elements_at(&s, p, &AtOpts { tolerance: 1.0, ..AtOpts::default() });
    assert!(h.is_empty());
    assert_eq!(click_target(&s, &sel, &h, p, false, false), ClickTarget::Empty { inside_editing_group: true });
    let q = Point::new(500.0, 5.0);
    assert_eq!(click_target(&s, &sel, &[], q, false, false), ClickTarget::Empty { inside_editing_group: false });
}

#[test]
fn cycling_visits_the_units_under_the_point_top_to_bottom() {
    // Three rectangles stacked over the same spot: top, mid, low.
    let s = scene(vec![
        rect("low", 0.0, 0.0, 100.0, 100.0),
        rect("mid", 0.0, 0.0, 100.0, 100.0),
        rect("top", 0.0, 0.0, 100.0, 100.0),
    ]);
    let p = Point::new(50.0, 50.0);
    // Filled hit needs a fill; with none only outlines hit, so use a point on the outline.
    let p = Point::new(0.0, p.y);
    let all = hits(&s, p.x, p.y, true);
    assert_eq!(all.len(), 3, "{all:?}");
    let mut sel = Selection::default();
    let mut seen = Vec::new();
    for _ in 0..4 {
        let t = click_target(&s, &sel, &all, p, false, true);
        let ClickTarget::Element(e) = &t else { panic!("{t:?}") };
        seen.push(e.to_string());
        sel = click(&s, &sel, &t, ClickOpts::default()).unwrap();
    }
    assert_eq!(seen, ["top", "mid", "low", "top"], "wraps around");
}

#[test]
fn a_click_inside_a_multi_selection_keeps_it_unless_it_hits_something_else() {
    let s = world();
    let sel = pick(&s, &Selection::default(), &[id("w"), id("x")], ClickOpts::default()).unwrap().selection;
    // Inside the common bounds (0..70), on nothing.
    let gap = Point::new(55.0, 5.0);
    assert!(keeps_multi_selection(&s, &sel, &hits(&s, gap.x, gap.y, false), gap));
    // On a selected element.
    let on = Point::new(5.0, 5.0);
    assert!(keeps_multi_selection(&s, &sel, &hits(&s, on.x, on.y, false), on));
    // Outside the bounds.
    let out = Point::new(300.0, 300.0);
    assert!(!keeps_multi_selection(&s, &sel, &hits(&s, out.x, out.y, false), out));
    // A single selected element is never kept by a click on nothing.
    let one = pick(&s, &Selection::default(), &[id("w")], ClickOpts::default()).unwrap().selection;
    assert!(!keeps_multi_selection(&s, &one, &[], Point::new(65.0, 5.0)));
    // Inside the bounds but on an unselected element below the frame outline: that element wins.
    let sel2 = pick(&s, &Selection::default(), &[id("w"), id("kid")], ClickOpts::default()).unwrap().selection;
    let at_f = Point::new(0.0, 150.0);
    let h = hits(&s, at_f.x, at_f.y, false);
    assert_eq!(h.first().map(|h| h.id.clone()), Some(id("f")));
    assert!(!keeps_multi_selection(&s, &sel2, &h, at_f));
}

// ---- query_cmd ----

#[test]
fn ref_query_resolves_names_and_refuses_unknown_ones() {
    let mut s = world();
    run(&mut s, |tx| tx.update(&id("w"), |e| e.name = Some("title".into())).unwrap());
    let q = RefQuery { ids: Some(vec!["@title".into(), "x".into()]), ..RefQuery::default() };
    assert_eq!(q.resolve(&s).unwrap().ids.unwrap(), vec![id("w"), id("x")]);
    let q = RefQuery {
        connected_to: Some(RefConnectedTo { target: "@title".into(), direction: Default::default() }),
        frame: Some("f".into()),
        ..RefQuery::default()
    };
    let strict = q.resolve(&s).unwrap();
    assert_eq!(strict.connected_to.unwrap().target, id("w"));
    assert_eq!(strict.frame, Some(id("f")));
    let bad = RefQuery { bound_text_of: Some("@nope".into()), ..RefQuery::default() };
    assert_eq!(bad.resolve(&s).unwrap_err(), QueryError::UnknownName("nope".into()));
    let bad = RefQuery { ids: Some(vec!["gone".into()]), ..RefQuery::default() };
    assert_eq!(bad.resolve(&s).unwrap_err(), QueryError::Missing(id("gone")));
    let bad = RefQuery { ids: Some(vec!["../x".into()]), ..RefQuery::default() };
    assert!(matches!(bad.resolve(&s).unwrap_err(), QueryError::BadRef(_)));
}

#[test]
fn all_matches_collects_every_page() {
    let mut els = Vec::new();
    for i in 0..2300 {
        els.push(rect(&format!("r{i}"), f64::from(i), 0.0, 1.0, 1.0));
    }
    let s = scene(els);
    let all = all_matches(&s, &Query::default(), &QueryCtx::default()).unwrap();
    assert_eq!(all.len(), 2300, "more than two pages of 1000");
    let unique: BTreeSet<_> = all.iter().collect();
    assert_eq!(unique.len(), 2300);
}

#[test]
fn hit_members_lists_the_group_without_labels() {
    let s = world();
    let h = AtHit { id: id("x"), group: Some(gid("A")) };
    assert_eq!(hit_members(&s, &h), ids(&["x", "y", "z"]));
    let single = AtHit { id: id("w"), group: None };
    assert_eq!(hit_members(&s, &single), ids(&["w"]));
    assert_eq!(ALL_FIELDS.len(), 14);
}

#[test]
fn ref_query_describe_names_only_the_fields_that_are_set() {
    assert_eq!(RefQuery::default().describe(), "");
    let q = RefQuery { ids: Some(vec!["a".into(), "@b".into()]), locked: Some(true), ..RefQuery::default() };
    assert_eq!(q.describe(), "ids: a, @b, locked: true");
    let q = RefQuery { frame: Some("f".into()), ..RefQuery::default() };
    assert_eq!(q.describe(), "frame: f");
}
