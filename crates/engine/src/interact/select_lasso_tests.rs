//! Tests of the lasso: the lasso tool, and Alt-drag from the select tool. Scripted pointer events go through the
//! real recognizer and the interaction layer into the real registry; `exception_E9_lasso_...` is the row of
//! `docs/exceptions.md` (the gesture gives the same selection as the plain command).

// `cargo xtask exceptions` looks for tests named `exception_E<n>_...`, which is not snake case.
#![allow(non_snake_case)]

use drafftink_canvas::Modifiers;
use drafftink_scene::ElementId;
use serde_json::json;

use super::super::Key;
use super::super::testutil::Rig;
use crate::state::Tool;

const SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, meta: false };
const ALT: Modifiers = Modifiers { shift: false, ctrl: false, alt: true, meta: false };

impl Rig {
    /// Press at the first point, move through the others, release at the last.
    fn lasso_path(&mut self, path: &[(f64, f64)]) {
        let (first, rest) = path.split_first().expect("a path");
        self.down(first.0, first.1);
        for p in rest {
            self.drag_to(p.0, p.1);
        }
        let last = path.last().expect("a path");
        self.up(last.0, last.1);
    }

    /// [`Rig::lasso_path`] with modifiers held.
    fn lasso_path_with(&mut self, mods: Modifiers, path: &[(f64, f64)]) {
        self.mods = mods;
        self.lasso_path(path);
        self.mods = Modifiers::NONE;
    }

    /// Press and move through all but the last point, and stay down.
    fn hold_lasso(&mut self, path: &[(f64, f64)]) {
        let (first, rest) = path.split_first().expect("a path");
        self.down(first.0, first.1);
        for p in rest {
            self.drag_to(p.0, p.1);
        }
    }
}

/// A box a at (100,100,100,100), b at (400,100,100,100) and c at (100,400,100,100), all filled.
fn three() -> (Rig, ElementId, ElementId, ElementId) {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let b = rig.rect(400.0, 100.0, 100.0, 100.0);
    let c = rig.rect(100.0, 400.0, 100.0, 100.0);
    (rig, a, b, c)
}

/// A triangle over the top left of the canvas: it holds a, and b and c are beyond its slanted side.
const TRIANGLE: [(f64, f64); 4] = [(50.0, 50.0), (430.0, 50.0), (50.0, 430.0), (60.0, 60.0)];
/// A loop around a and b only.
const LOOP: [(f64, f64); 5] = [(50.0, 50.0), (550.0, 50.0), (550.0, 250.0), (50.0, 250.0), (50.0, 60.0)];

#[test]
fn the_lasso_tool_selects_what_the_loop_surrounds_and_stays_the_tool() {
    let (mut rig, a, b, _c) = three();
    rig.tool(Tool::Lasso);
    rig.lasso_path(&LOOP);
    assert_eq!(rig.selected(), vec![a.as_str().to_owned(), b.as_str().to_owned()]);
    assert_eq!(rig.s.tool.effective(), Tool::Lasso, "the lasso tool stays the tool");
    assert_eq!(rig.ring_ids(), ["tool.set", "selection.select_lasso"].map(str::to_owned));
}

#[test]
fn the_slanted_side_of_a_lasso_leaves_out_what_a_rectangle_would_take() {
    let (mut rig, a, _b, _c) = three();
    rig.tool(Tool::Lasso);
    rig.lasso_path(&TRIANGLE);
    assert_eq!(rig.selected(), vec![a.as_str().to_owned()]);
    rig.tool(Tool::Select);
    rig.drag((50.0, 50.0), (430.0, 430.0));
    assert_eq!(rig.selected().len(), 3, "the marquee over the same area takes all three");
}

#[test]
fn exception_E9_lasso_equals_plain_command() {
    for mods in [Modifiers::NONE, SHIFT] {
        let (mut rig, _a, _b, c) = three();
        rig.tool(Tool::Lasso);
        rig.run("selection.select", json!({ "ids": [c.as_str()] }));
        rig.lasso_path_with(mods, &LOOP);
        let (mut twin, _ta, _tb, tc) = three();
        twin.run("selection.select", json!({ "ids": [tc.as_str()] }));
        let mode = if mods.shift { "add" } else { "replace" };
        let points: Vec<[f64; 2]> = LOOP.iter().map(|&(x, y)| [x, y]).collect();
        twin.run("selection.select_lasso", json!({ "points": points, "mode": mode, "simplify": 4 }));
        assert_eq!(rig.selected(), twin.selected(), "mods {mods:?}");
        assert_eq!(rig.normalized_doc(), twin.normalized_doc());
        assert_eq!(rig.s.tab().observed.selection, rig.s.selection().clone());
    }
}

#[test]
fn alt_drag_with_the_select_tool_is_a_lasso_and_a_plain_drag_is_a_marquee() {
    let (mut rig, a, _b, _c) = three();
    rig.lasso_path_with(ALT, &TRIANGLE);
    assert_eq!(rig.selected(), vec![a.as_str().to_owned()]);
    assert_eq!(rig.ring_ids(), ["selection.select_lasso"].map(str::to_owned));
    assert_eq!(rig.s.tool.effective(), Tool::Select);
    let (mut rig, _a, _b, _c) = three();
    rig.lasso_path(&TRIANGLE);
    assert_eq!(rig.ring_ids(), ["selection.select_region"].map(str::to_owned), "no Alt: the marquee");
}

#[test]
fn the_path_is_in_the_overlay_while_it_is_drawn_and_gone_after() {
    let (mut rig, _a, _b, _c) = three();
    rig.tool(Tool::Lasso);
    rig.hold_lasso(&LOOP[..4]);
    let shown = rig.i.overlay(&rig.s).canvas.lasso.expect("the lasso is in the overlay");
    assert_eq!(shown.points.len(), 4, "{:?}", shown.points);
    assert_eq!(rig.i.parts().lasso.len(), 4);
    assert!(rig.i.parts().has_gesture_state());
    assert!(rig.i.overlay(&rig.s).canvas.marquee.is_none());
    rig.up(50.0, 250.0);
    assert!(rig.i.overlay(&rig.s).canvas.lasso.is_none());
    assert!(!rig.i.parts().has_gesture_state());
}

#[test]
fn escape_drops_the_lasso_and_the_selection_stays() {
    let (mut rig, a, _b, _c) = three();
    rig.tool(Tool::Lasso);
    rig.run("selection.select", json!({ "ids": [a.as_str()] }));
    rig.hold_lasso(&LOOP[..4]);
    let out = rig.key(Key::Escape);
    assert!(out.handled);
    assert!(rig.i.parts().lasso.is_empty());
    rig.up(50.0, 250.0);
    assert_eq!(rig.selected(), vec![a.as_str().to_owned()], "the release after the cancel does nothing");
    assert_eq!(rig.ring_ids(), ["tool.set", "selection.select"].map(str::to_owned));
}

#[test]
fn a_flat_or_tiny_lasso_selects_nothing_and_says_nothing() {
    let (mut rig, a, _b, _c) = three();
    rig.tool(Tool::Lasso);
    rig.run("selection.select", json!({ "ids": [a.as_str()] }));
    rig.lasso_path(&[(300.0, 300.0), (350.0, 350.0), (400.0, 400.0)]);
    assert!(rig.last.refusals.is_empty(), "{:?}", rig.last.refusals);
    rig.lasso_path(&[(300.0, 300.0), (330.0, 300.0), (330.0, 301.0)]);
    assert!(rig.last.refusals.is_empty(), "{:?}", rig.last.refusals);
    assert_eq!(rig.selected(), vec![a.as_str().to_owned()], "nothing ran, so the selection is as it was");
    assert_eq!(rig.ring_ids(), ["tool.set", "selection.select"].map(str::to_owned));
}

#[test]
fn a_click_with_the_lasso_tool_selects_like_the_select_tool_and_clears_on_empty_canvas() {
    let (mut rig, a, b, _c) = three();
    rig.tool(Tool::Lasso);
    rig.click(150.0, 150.0);
    assert_eq!(rig.selected(), vec![a.as_str().to_owned()]);
    rig.click(450.0, 150.0);
    assert_eq!(rig.selected(), vec![b.as_str().to_owned()], "a click on another element replaces");
    rig.click(300.0, 300.0);
    assert!(rig.selected().is_empty(), "a click on empty canvas clears");
}

#[test]
fn a_drag_from_an_element_that_is_not_selected_is_a_lasso_and_from_a_selected_one_moves_it() {
    let (mut rig, a, b, _c) = three();
    rig.tool(Tool::Lasso);
    // Start on b (not selected): the press selects nothing, and the drag is a lasso around a.
    rig.hold_lasso(&[(450.0, 150.0), (460.0, 20.0), (60.0, 20.0), (60.0, 260.0), (460.0, 260.0)]);
    assert!(rig.selected().is_empty(), "nothing is selected before the release");
    rig.up(450.0, 160.0);
    assert_eq!(rig.selected(), vec![a.as_str().to_owned(), b.as_str().to_owned()]);
    // Now a is selected: dragging it moves it.
    rig.run("selection.select", json!({ "ids": [a.as_str()] }));
    let before = rig.bounds(&a);
    rig.drag((150.0, 150.0), (170.0, 160.0));
    let after = rig.bounds(&a);
    assert_eq!((after.0 - before.0, after.1 - before.1), (20.0, 10.0));
}

#[test]
fn the_lasso_is_allowed_in_view_only_mode_and_a_locked_element_is_not_selected() {
    let (mut rig, a, _b, _c) = three();
    rig.run("element.lock", json!({ "ids": [a.as_str()], "locked": true }));
    rig.run("view.set_view_only", json!({ "on": true }));
    rig.run("tool.set", json!({ "tool": "lasso" }));
    rig.lasso_path(&LOOP);
    assert_eq!(rig.s.tool.effective(), Tool::Lasso);
    assert!(!rig.selected().contains(&a.as_str().to_owned()));
}
