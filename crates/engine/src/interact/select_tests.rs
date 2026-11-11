//! Tests of the select tool: scripted pointer events go through the real `Recognizer` and the interaction layer
//! into the real command registry. The `exception_E<n>_` tests are the rows of `docs/exceptions.md`: the gesture
//! gives the same document as the plain command run on a fresh twin.

// `cargo xtask exceptions` looks for tests named `exception_E<n>_...`, which is not snake case.
#![allow(non_snake_case)]

use drafftink_canvas::{
    Button, Buttons, Handle, InputEvent, Intent, Meta, Modifiers, PointerEvent, PointerId, PointerKind, PointerPhase,
    Pos, Target,
};
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_scene::{ElementId, ElementKind, Kind, PathType};
use serde_json::json;

use super::super::testutil::Rig;
use super::super::{Badge, Key, UiRequest};
use crate::state::Tool;
use crate::testing;

const SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, meta: false };
const ALT: Modifiers = Modifiers { shift: false, ctrl: false, alt: true, meta: false };
const CTRL: Modifiers = Modifiers { shift: false, ctrl: true, alt: false, meta: false };

// ---- helpers ---------------------------------------------------------------------------------------------------

impl Rig {
    /// The world position of a handle of the current selection, from the overlay the renderer would draw.
    fn handle_pos(&self, handle: &Handle) -> Point {
        let overlay = self.i.overlay(&self.s);
        let found = overlay
            .canvas
            .transformer
            .as_ref()
            .and_then(|t| t.handles.iter().find(|h| &h.handle == handle).map(|h| h.world));
        found.unwrap_or_else(|| panic!("no handle {handle:?} in the overlay"))
    }

    /// Selects elements without a command (test setup; not an undo step and not in the ring).
    fn pick(&mut self, ids: &[&ElementId]) {
        let ids: Vec<ElementId> = ids.iter().map(|i| (*i).clone()).collect();
        testing::select(&mut self.s, &ids);
    }

    /// Press at `from`, then move in two steps to `to`, and stay down.
    fn hold_drag(&mut self, from: (f64, f64), to: (f64, f64)) {
        self.down(from.0, from.1);
        self.drag_to((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
        self.drag_to(to.0, to.1);
    }

    /// Runs a closure with the modifiers set, then clears them.
    fn with_mods<R>(&mut self, mods: Modifiers, f: impl FnOnce(&mut Rig) -> R) -> R {
        self.mods = mods;
        let out = f(self);
        self.mods = Modifiers::NONE;
        out
    }

    /// A click that is not the second click of a double-click.
    fn tap(&mut self, x: f64, y: f64) {
        self.now += 600.0;
        self.click(x, y);
    }

    /// The mutating ring entries after the first `setup` ones (a test's own setup commands).
    fn ring_after(&self, setup: usize) -> Vec<String> {
        self.ring_ids().into_iter().skip(setup).collect()
    }

    fn live(&self) -> usize {
        self.elements().len()
    }

    /// Feeds an intent straight to the layer.
    fn intent(&mut self, intent: Intent, mods: Modifiers) {
        self.now += 16.0;
        let out = self.i.handle(&mut self.s, intent, self.now, mods);
        self.last = out.clone();
        self.history.push(out);
    }
}

fn pos(x: f64, y: f64) -> Pos {
    Pos { screen: Point::new(x, y), world: Point::new(x, y) }
}

fn meta(mods: Modifiers) -> Meta {
    Meta {
        pointer: PointerId(0),
        kind: PointerKind::Mouse,
        button: Button::Primary,
        modifiers: mods,
        pressure: 0.5,
        tilt: Default::default(),
        time_ms: 0.0,
    }
}

fn on(id: &ElementId) -> Target {
    Target::Element { id: id.clone(), unit_group: None }
}

/// A rectangle at (100, 100), 200 by 100, and another far away.
fn two_rects() -> (Rig, ElementId, ElementId) {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 200.0, 100.0);
    let b = rig.rect(500.0, 400.0, 100.0, 100.0);
    (rig, a, b)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

// ---- click -----------------------------------------------------------------------------------------------------

#[test]
fn a_click_selects_shift_click_toggles_and_a_click_on_empty_canvas_clears() {
    let (mut rig, a, b) = two_rects();
    rig.tap(150.0, 150.0);
    assert_eq!(rig.selected(), vec![a.as_str()]);
    rig.with_mods(SHIFT, |r| r.tap(550.0, 450.0));
    assert_eq!(rig.selected(), vec![a.as_str(), b.as_str()]);
    rig.with_mods(SHIFT, |r| r.tap(150.0, 150.0));
    assert_eq!(rig.selected(), vec![b.as_str()], "shift-click on a selected element removes it");
    rig.with_mods(SHIFT, |r| r.tap(700.0, 100.0));
    assert_eq!(rig.selected(), vec![b.as_str()], "shift-click on empty canvas keeps the selection");
    rig.tap(700.0, 100.0);
    assert!(rig.selected().is_empty());
    assert!(rig.ring_ids().iter().all(|i| i == "selection.select_at"), "{:?}", rig.ring_ids());
}

#[test]
fn clicking_selects_on_press_so_a_drag_moves_the_right_element() {
    let (mut rig, a, b) = two_rects();
    rig.pick(&[&b]);
    rig.down(150.0, 150.0);
    assert_eq!(rig.selected(), vec![a.as_str()], "the press selects");
    rig.up(150.0, 150.0);
    assert_eq!(rig.selected(), vec![a.as_str()]);
}

#[test]
fn a_click_on_a_member_of_a_multi_selection_keeps_the_selection_until_the_click_is_over() {
    let (mut rig, a, b) = two_rects();
    rig.pick(&[&a, &b]);
    rig.down(150.0, 150.0);
    assert_eq!(rig.selected().len(), 2, "the press that may start a drag keeps the multi-selection");
    rig.up(150.0, 150.0);
}

#[test]
fn shift_press_on_a_selected_element_waits_for_the_release() {
    let (mut rig, a, b) = two_rects();
    rig.pick(&[&a, &b]);
    rig.with_mods(SHIFT, |r| {
        r.down(150.0, 150.0);
        assert_eq!(r.selected().len(), 2, "nothing is toggled yet");
        r.up(150.0, 150.0);
    });
    assert_eq!(rig.selected(), vec![b.as_str()], "the click toggled it off");
}

#[test]
fn shift_drag_of_a_selected_element_keeps_the_selection_and_moves_it() {
    let (mut rig, a, b) = two_rects();
    rig.pick(&[&a, &b]);
    rig.with_mods(SHIFT, |r| r.drag((150.0, 150.0), (170.0, 150.0)));
    assert_eq!(rig.selected().len(), 2);
    assert!(near(rig.bounds(&a).0, 120.0) && near(rig.bounds(&b).0, 520.0));
}

#[test]
fn alt_click_without_a_drag_cycles_through_the_elements_under_the_point() {
    let mut rig = Rig::new();
    let lower = rig.rect(100.0, 100.0, 200.0, 200.0);
    let upper = rig.rect(150.0, 150.0, 200.0, 200.0);
    rig.tap(200.0, 200.0);
    assert_eq!(rig.selected(), vec![upper.as_str()], "the top element first");
    rig.with_mods(ALT, |r| r.tap(200.0, 200.0));
    assert_eq!(rig.selected(), vec![lower.as_str()], "alt-click steps to the one below");
    assert!(rig.ring_ids().iter().all(|i| i == "selection.select_at"));
}

#[test]
fn ctrl_click_selects_a_member_of_a_group_not_the_group() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let b = rig.rect(300.0, 100.0, 100.0, 100.0);
    rig.pick(&[&a, &b]);
    rig.run("group.create", json!({}));
    rig.click(150.0, 150.0);
    assert_eq!(rig.selected().len(), 2, "a plain click selects the group");
    rig.with_mods(CTRL, |r| r.click(150.0, 150.0));
    assert_eq!(rig.selected(), vec![a.as_str()]);
}

#[test]
fn a_press_with_the_secondary_button_does_not_select() {
    let (mut rig, _, _) = two_rects();
    rig.now += 16.0;
    let ev = PointerEvent::new(PointerId(0), PointerKind::Mouse, PointerPhase::Down, Point::new(150.0, 150.0), rig.now)
        .with_button(Button::Secondary, Buttons::SECONDARY);
    rig.input(InputEvent::Pointer(ev));
    assert!(rig.ring_ids().iter().all(|i| i != "selection.select_at"), "{:?}", rig.ring_ids());
}

// ---- marquee (E9) ----------------------------------------------------------------------------------------------

#[test]
fn drag_on_empty_canvas_selects_by_marquee() {
    let (mut rig, a, _b) = two_rects();
    rig.down(50.0, 50.0);
    rig.drag_to(200.0, 150.0);
    let marquee = rig.i.parts().marquee;
    assert_eq!(marquee.map(|r| (r.x0, r.y0, r.x1, r.y1)), Some((50.0, 50.0, 200.0, 150.0)), "the rectangle is shown");
    assert!(rig.i.overlay(&rig.s).canvas.marquee.is_some());
    assert!(rig.last.redraw.is_some(), "the marquee asks for an overlay redraw");
    assert!(rig.ring_ids().is_empty(), "no command while the marquee is dragged: {:?}", rig.ring_ids());
    rig.drag_to(320.0, 220.0);
    rig.up(320.0, 220.0);
    assert_eq!(rig.selected(), vec![a.as_str()]);
    assert_eq!(rig.ring_ids(), vec!["selection.select_region"], "exactly one command");
    assert!(rig.i.parts().marquee.is_none() && rig.i.overlay(&rig.s).canvas.marquee.is_none());
}

#[test]
fn a_marquee_dragged_backwards_selects_the_same() {
    let (mut rig, a, _b) = two_rects();
    rig.drag((320.0, 220.0), (50.0, 50.0));
    assert_eq!(rig.selected(), vec![a.as_str()]);
}

#[test]
fn a_tiny_marquee_selects_nothing_and_runs_no_command() {
    let (mut rig, a, _) = two_rects();
    rig.pick(&[&a]);
    rig.down(400.0, 300.0);
    rig.drag_to(401.0, 305.0);
    rig.drag_to(401.0, 305.0);
    rig.up(401.0, 305.0);
    assert!(rig.ring_ids().iter().all(|i| i != "selection.select_region"), "{:?}", rig.ring_ids());
    assert_eq!(rig.selected(), vec![a.as_str()]);
}

#[test]
fn shift_marquee_adds_to_the_selection() {
    let (mut rig, a, b) = two_rects();
    rig.pick(&[&b]);
    rig.with_mods(SHIFT, |r| r.drag((50.0, 50.0), (320.0, 220.0)));
    assert_eq!(rig.selected(), vec![a.as_str(), b.as_str()]);
    assert_eq!(rig.ring_ids(), vec!["selection.select_region"]);
}

#[test]
fn escape_during_a_marquee_leaves_the_selection_untouched() {
    let (mut rig, a, b) = two_rects();
    rig.pick(&[&b]);
    rig.hold_drag((50.0, 50.0), (320.0, 220.0));
    assert!(rig.i.parts().marquee.is_some());
    let out = rig.key(Key::Escape);
    assert!(out.handled, "the layer used the key");
    assert!(rig.i.parts().marquee.is_none());
    assert_eq!(rig.selected(), vec![b.as_str()]);
    rig.up(320.0, 220.0);
    assert_eq!(rig.selected(), vec![b.as_str()], "the release after the cancel does nothing");
    assert!(rig.ring_ids().is_empty(), "{:?}", rig.ring_ids());
    let _ = a;
}

#[test]
fn exception_E9_marquee_equals_plain_command() {
    for mods in [Modifiers::NONE, SHIFT] {
        let (mut rig, _a, b) = two_rects();
        rig.pick(&[&b]);
        rig.with_mods(mods, |r| r.drag((50.0, 50.0), (320.0, 220.0)));
        let (mut twin, _ta, tb) = two_rects();
        twin.pick(&[&tb]);
        let mode = if mods.shift { "add" } else { "replace" };
        twin.run("selection.select_region", json!({ "rect": { "x": 50, "y": 50, "w": 270, "h": 170 }, "mode": mode }));
        assert_eq!(rig.selected(), twin.selected(), "mods {mods:?}");
        assert_eq!(rig.normalized_doc(), twin.normalized_doc());
        assert_eq!(rig.s.tab().observed.selection, rig.s.selection().clone());
    }
}

// ---- move (E2) -------------------------------------------------------------------------------------------------

#[test]
fn dragging_a_rectangle_moves_it_in_one_undo_step_and_one_ring_entry() {
    let (mut rig, a, b) = two_rects();
    let undo_before = rig.undo_depth();
    rig.hold_drag((150.0, 150.0), (190.0, 170.0));
    assert!(rig.s.gesture_running(), "the document changes live");
    assert_eq!(rig.bounds(&a).0, 140.0, "the element follows the pointer before the release");
    rig.up(190.0, 170.0);
    assert!(!rig.s.gesture_running());
    assert_eq!(rig.bounds(&a), (140.0, 120.0, 200.0, 100.0));
    assert_eq!(rig.bounds(&b), (500.0, 400.0, 100.0, 100.0));
    assert_eq!(rig.undo_depth(), undo_before + 1, "exactly one undo step");
    assert_eq!(rig.ring_ids(), vec!["selection.select_at", "element.move"]);
    let entry = rig.ring().into_iter().find(|e| e.id == "element.move");
    assert!(entry.is_some_and(|e| e.frames.is_some_and(|f| f > 1)), "the ring entry counts the frames");
    assert_eq!(rig.s.selection().len(), 1);
}

#[test]
fn undoing_a_drag_move_restores_the_position() {
    let (mut rig, a, _) = two_rects();
    rig.drag((150.0, 150.0), (250.0, 250.0));
    assert_eq!(rig.bounds(&a).0, 200.0);
    rig.run("history.undo", json!({}));
    assert_eq!(rig.bounds(&a), (100.0, 100.0, 200.0, 100.0));
}

#[test]
fn exception_E2_move_equals_plain_command() {
    for (mods, duplicate) in [(Modifiers::NONE, false), (ALT, true)] {
        let (mut rig, a, _) = two_rects();
        rig.with_mods(mods, |r| r.drag((150.0, 150.0), (213.0, 177.0)));
        let (mut twin, ta, _) = two_rects();
        twin.pick(&[&ta]);
        twin.run("element.move", json!({ "dx": 63.0, "dy": 27.0, "duplicate": duplicate, "pointer": [213.0, 177.0] }));
        assert_eq!(rig.normalized_doc(), twin.normalized_doc(), "duplicate = {duplicate}");
        assert_eq!(rig.live(), twin.live());
        assert_eq!(rig.selected().len(), twin.selected().len());
        let _ = a;
    }
}

#[test]
fn alt_drag_duplicates_and_selects_the_copy() {
    let (mut rig, a, _) = two_rects();
    rig.with_mods(ALT, |r| r.drag((150.0, 150.0), (250.0, 150.0)));
    assert_eq!(rig.live(), 3);
    assert_eq!(rig.bounds(&a), (100.0, 100.0, 200.0, 100.0), "the original stays");
    let copy = rig.selected();
    assert_eq!(copy.len(), 1);
    assert_ne!(copy[0], a.as_str());
    let id = ElementId::new(&copy[0]).expect("id");
    assert_eq!(rig.bounds(&id).0, 200.0);
}

#[test]
fn escape_during_a_drag_cancels_the_gesture_and_restores_the_state() {
    let (mut rig, a, _) = two_rects();
    rig.tap(150.0, 150.0);
    let doc = rig.normalized_doc();
    let undo = rig.undo_depth();
    let ring_before = rig.ring_all_ids();
    rig.hold_drag((150.0, 150.0), (250.0, 250.0));
    assert!(rig.s.gesture_running());
    assert_ne!(rig.bounds(&a).0, 100.0);
    let out = rig.key(Key::Escape);
    assert!(out.handled);
    assert!(!rig.s.gesture_running());
    assert_eq!(rig.normalized_doc(), doc);
    assert_eq!(rig.undo_depth(), undo, "no undo step");
    let mut expected = ring_before;
    expected.push("selection.select_at".to_owned()); // the press of the drag
    expected.push("gesture.cancel".to_owned());
    assert_eq!(rig.ring_all_ids(), expected, "the cancel is the only entry the gesture left");
    rig.up(250.0, 250.0);
    assert_eq!(rig.normalized_doc(), doc, "the release after the cancel does nothing");
    assert_eq!(rig.ring_all_ids(), expected);
    assert!(!rig.i.parts().has_gesture_state());
    assert_eq!(rig.bounds(&a), (100.0, 100.0, 200.0, 100.0));
}

#[test]
fn a_cancelled_drag_end_intent_cancels_the_gesture_too() {
    let (mut rig, a, _) = two_rects();
    rig.click(150.0, 150.0);
    let doc = rig.normalized_doc();
    let undo = rig.undo_depth();
    let m = meta(Modifiers::NONE);
    rig.intent(Intent::Press { at: pos(150.0, 150.0), target: on(&a), meta: m.clone() }, Modifiers::NONE);
    rig.intent(
        Intent::DragStart { origin: pos(150.0, 150.0), at: pos(160.0, 150.0), target: on(&a), meta: m.clone() },
        Modifiers::NONE,
    );
    rig.intent(
        Intent::DragUpdate {
            origin: pos(150.0, 150.0),
            at: pos(180.0, 150.0),
            delta_screen: Vec2::new(30.0, 0.0),
            delta_world: Vec2::new(30.0, 0.0),
            target: on(&a),
            meta: m.clone(),
        },
        Modifiers::NONE,
    );
    assert!(rig.s.gesture_running());
    assert_eq!(rig.bounds(&a).0, 130.0);
    rig.intent(
        Intent::DragEnd {
            origin: pos(150.0, 150.0),
            at: pos(180.0, 150.0),
            delta_screen: Vec2::new(30.0, 0.0),
            delta_world: Vec2::new(30.0, 0.0),
            target: on(&a),
            cancelled: true,
            meta: m,
        },
        Modifiers::NONE,
    );
    assert!(!rig.s.gesture_running());
    assert_eq!(rig.normalized_doc(), doc);
    assert_eq!(rig.undo_depth(), undo);
    assert!(!rig.ring_ids().contains(&"element.move".to_owned()));
}

#[test]
fn a_locked_element_is_not_moved() {
    let (mut rig, a, _) = two_rects();
    rig.click(150.0, 150.0);
    rig.run("element.lock", json!({}));
    let doc = rig.normalized_doc();
    // Locked elements are not hit: the drag becomes a marquee that selects nothing.
    rig.drag((150.0, 150.0), (250.0, 250.0));
    assert_eq!(rig.normalized_doc(), doc);
    assert!(!rig.ring_ids().contains(&"element.move".to_owned()));
    // A drag intent aimed at the locked element (a stale target) is refused once.
    let m = meta(Modifiers::NONE);
    rig.intent(
        Intent::DragStart { origin: pos(150.0, 150.0), at: pos(160.0, 150.0), target: on(&a), meta: m.clone() },
        Modifiers::NONE,
    );
    assert!(!rig.s.gesture_running());
    assert_eq!(rig.last.refusals.len(), 1, "{:?}", rig.last.refusals);
    for k in 2..6 {
        let at = pos(150.0 + f64::from(k) * 10.0, 150.0);
        rig.intent(
            Intent::DragUpdate {
                origin: pos(150.0, 150.0),
                at,
                delta_screen: Vec2::new(f64::from(k) * 10.0, 0.0),
                delta_world: Vec2::new(f64::from(k) * 10.0, 0.0),
                target: on(&a),
                meta: m.clone(),
            },
            Modifiers::NONE,
        );
        assert!(rig.last.refusals.is_empty(), "the dead drag stays quiet");
    }
    assert_eq!(rig.normalized_doc(), doc);
}

#[test]
fn grid_snap_follows_the_settings_and_ctrl_turns_it_off() {
    let (mut rig, a, _) = two_rects();
    rig.run("settings.set", json!({ "key": "snap.grid", "value": true }));
    rig.drag((150.0, 150.0), (173.0, 168.0));
    let (x, y, _, _) = rig.bounds(&a);
    assert!(near(x % 20.0, 0.0) && near(y % 20.0, 0.0), "snapped to the 20 unit grid: {x} {y}");
    rig.run("history.undo", json!({}));
    rig.with_mods(CTRL, |r| r.drag((150.0, 150.0), (173.0, 168.0)));
    assert_eq!(rig.bounds(&a), (123.0, 118.0, 200.0, 100.0), "ctrl moves freely");
}

#[test]
fn smart_guides_and_the_frame_highlight_reach_the_overlay_during_the_drag() {
    let mut rig = Rig::new();
    rig.run("settings.set", json!({ "key": "snap.smart_guides", "value": true }));
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let _b = rig.rect(400.0, 100.0, 100.0, 100.0);
    rig.hold_drag((150.0, 150.0), (150.0, 154.0));
    rig.drag_to(150.0, 155.0);
    assert!(!rig.i.parts().guides.is_empty(), "a guide shows while the top edges are near-aligned");
    assert!(!rig.i.overlay(&rig.s).canvas.guides.is_empty());
    rig.up(150.0, 155.0);
    assert!(rig.i.parts().guides.is_empty(), "the guides go with the drag");
    let _ = a;
}

#[test]
fn moving_a_rectangle_keeps_the_arrow_attached() {
    let (mut rig, a, b) = two_rects();
    let arrow = testing::create_arrow_between(&mut rig.s, &a, &b).expect("arrow");
    let before = rig.element(&arrow);
    rig.drag((150.0, 150.0), (150.0, 250.0));
    let after = rig.element(&arrow);
    assert_ne!(before.transform, after.transform, "the arrow followed");
    let data = after.linear_data().expect("linear");
    assert_eq!(data.start_binding.as_ref().map(|b| b.target.clone()), Some(a.clone()));
    assert_eq!(data.end_binding.as_ref().map(|b| b.target.clone()), Some(b.clone()));
}

#[test]
fn text_tool_drags_an_element_but_draws_nothing_on_empty_canvas_in_the_select_machine() {
    let (mut rig, a, _) = two_rects();
    rig.tool(Tool::Text);
    rig.drag((150.0, 150.0), (250.0, 150.0));
    assert_eq!(rig.bounds(&a).0, 200.0, "the press and the drag started on an element: select machine");
    let ids: Vec<String> = rig.ring_ids().into_iter().filter(|i| i != "tool.set").collect();
    assert_eq!(ids, vec!["selection.select_at", "element.move"]);
}

#[test]
fn a_drag_from_an_unselected_element_selects_it_first() {
    let (mut rig, a, b) = two_rects();
    rig.pick(&[&b]);
    let m = meta(Modifiers::NONE);
    // The press never reached the tool (as for a drag that a shell started late).
    rig.intent(
        Intent::DragStart { origin: pos(150.0, 150.0), at: pos(160.0, 150.0), target: on(&a), meta: m },
        Modifiers::NONE,
    );
    assert_eq!(rig.selected(), vec![a.as_str()]);
    assert!(rig.s.gesture_running());
    rig.key(Key::Escape);
    assert!(!rig.s.gesture_running());
}

#[test]
fn tool_cancel_during_a_drag_leaves_no_stuck_state() {
    let (mut rig, a, _) = two_rects();
    rig.click(150.0, 150.0);
    let doc = rig.normalized_doc();
    rig.hold_drag((150.0, 150.0), (250.0, 250.0));
    assert!(rig.s.gesture_running());
    rig.run("tool.cancel", json!({}));
    assert!(!rig.s.gesture_running(), "the command cancelled the gesture");
    assert_eq!(rig.normalized_doc(), doc);
    rig.drag_to(300.0, 300.0);
    assert!(rig.last.refusals.is_empty(), "no refusal for the vanished gesture");
    assert!(!rig.i.parts().has_gesture_state());
    rig.up(300.0, 300.0);
    assert_eq!(rig.normalized_doc(), doc);
    assert!(!rig.i.is_busy());
    // The next drag works.
    rig.drag((150.0, 150.0), (170.0, 150.0));
    assert_eq!(rig.bounds(&a).0, 120.0);
}

#[test]
fn a_tool_change_during_a_drag_cancels_it() {
    let (mut rig, a, _) = two_rects();
    rig.click(150.0, 150.0);
    rig.hold_drag((150.0, 150.0), (250.0, 250.0));
    // The shell switches tools (a command would refuse during a gesture).
    rig.s.tool.active = Tool::Rectangle;
    rig.hover(10.0, 10.0);
    assert!(!rig.s.gesture_running());
    assert_eq!(rig.bounds(&a).0, 100.0);
}

#[test]
fn view_only_refuses_the_drag_once() {
    let (mut rig, a, _) = two_rects();
    rig.click(150.0, 150.0);
    rig.run("view.set_view_only", json!({ "on": true }));
    let doc = rig.normalized_doc();
    rig.down(150.0, 150.0);
    rig.drag_to(180.0, 150.0);
    rig.drag_to(200.0, 150.0);
    rig.drag_to(220.0, 150.0);
    rig.up(220.0, 150.0);
    let refusals: usize = rig.history.iter().map(|o| o.refusals.len()).sum();
    assert_eq!(refusals, 1, "one refusal for the whole drag");
    assert_eq!(rig.normalized_doc(), doc);
    assert!(!rig.s.gesture_running());
    let _ = a;
}

#[test]
fn a_touch_drag_moves_an_element() {
    let (mut rig, a, _) = two_rects();
    let touch = |rig: &mut Rig, phase: PointerPhase, x: f64, y: f64| {
        rig.now += 16.0;
        let buttons = if phase == PointerPhase::Up { Buttons::NONE } else { Buttons::PRIMARY };
        let ev = PointerEvent::new(PointerId(7), PointerKind::Touch, phase, Point::new(x, y), rig.now)
            .with_button(Button::Primary, buttons);
        rig.input(InputEvent::Pointer(ev));
    };
    touch(&mut rig, PointerPhase::Down, 150.0, 150.0);
    for k in 1..=4 {
        touch(&mut rig, PointerPhase::Move, 150.0 + f64::from(k) * 10.0, 150.0);
    }
    touch(&mut rig, PointerPhase::Up, 190.0, 150.0);
    assert_eq!(rig.bounds(&a).0, 140.0);
    assert_eq!(rig.ring_ids(), vec!["selection.select_at", "element.move"]);
}

#[test]
fn a_drag_in_the_middle_of_pointer_history_costs_no_command_per_move() {
    let (mut rig, _, _) = two_rects();
    rig.click(150.0, 150.0);
    rig.down(150.0, 150.0);
    let seq = rig.s.ring().next_seq();
    for k in 1..=40 {
        rig.drag_to(150.0 + f64::from(k), 150.0);
    }
    assert_eq!(rig.s.ring().next_seq(), seq, "no ring entry while the gesture runs");
    rig.up(190.0, 150.0);
}

// ---- resize (E3) -----------------------------------------------------------------------------------------------

const BOX_HANDLES: [(Handle, &str); 8] = [
    (Handle::TopLeft, "nw"),
    (Handle::Top, "n"),
    (Handle::TopRight, "ne"),
    (Handle::Right, "e"),
    (Handle::BottomRight, "se"),
    (Handle::Bottom, "s"),
    (Handle::BottomLeft, "sw"),
    (Handle::Left, "w"),
];

/// One rectangle, selected, optionally turned by `angle` degrees with a command (in the rig and in its twin alike).
fn selected_rect(angle: f64) -> (Rig, ElementId) {
    let mut rig = Rig::new();
    let a = rig.rect(200.0, 200.0, 200.0, 100.0);
    rig.pick(&[&a]);
    if angle != 0.0 {
        rig.run("element.rotate", json!({ "angle": angle }));
    }
    (rig, a)
}

#[test]
fn resizing_from_a_handle_changes_the_box_and_shows_the_size() {
    let (mut rig, a) = selected_rect(0.0);
    let from = rig.handle_pos(&Handle::BottomRight);
    rig.hold_drag((from.x, from.y), (from.x + 50.0, from.y + 30.0));
    assert_eq!(rig.bounds(&a), (200.0, 200.0, 250.0, 130.0), "the box follows before the release");
    assert_eq!(rig.i.parts().active_handle, Some(Handle::BottomRight));
    assert_eq!(rig.i.parts().badge, Some(Badge::Size { w: 250.0, h: 130.0 }));
    assert_eq!(rig.i.overlay(&rig.s).badge, Some(Badge::Size { w: 250.0, h: 130.0 }));
    assert_eq!(
        rig.i.overlay(&rig.s).canvas.transformer.and_then(|t| t.active),
        Some(Handle::BottomRight),
        "the renderer is told which handle is active"
    );
    rig.up(from.x + 50.0, from.y + 30.0);
    assert_eq!(rig.ring_ids(), vec!["element.resize"], "a press on a handle selects nothing");
    assert!(rig.i.parts().badge.is_none() && rig.i.parts().active_handle.is_none());
    assert_eq!(rig.undo_depth(), 1);
}

#[test]
fn exception_E3_resize_equals_plain_command() {
    for angle in [0.0, 30.0] {
        for (handle, word) in BOX_HANDLES {
            for (mods, keep, center) in [(Modifiers::NONE, false, false), (SHIFT, true, false), (ALT, false, true)] {
                let (mut rig, _) = selected_rect(angle);
                let from = rig.handle_pos(&handle);
                rig.with_mods(mods, |r| r.drag((from.x, from.y), (from.x + 23.0, from.y + 17.0)));
                let (mut twin, _) = selected_rect(angle);
                twin.run(
                    "element.resize",
                    json!({ "handle": word, "dx": 23.0, "dy": 17.0, "keep_aspect": keep, "from_center": center }),
                );
                assert_eq!(
                    rig.normalized_doc(),
                    twin.normalized_doc(),
                    "handle {word}, angle {angle}, keep {keep}, center {center}"
                );
                assert_eq!(rig.undo_depth(), twin.undo_depth());
            }
        }
    }
}

#[test]
fn escape_during_a_resize_restores_the_box() {
    let (mut rig, a) = selected_rect(0.0);
    let doc = rig.normalized_doc();
    let from = rig.handle_pos(&Handle::Right);
    rig.hold_drag((from.x, from.y), (from.x + 80.0, from.y));
    assert_ne!(rig.bounds(&a).2, 200.0);
    rig.key(Key::Escape);
    assert_eq!(rig.normalized_doc(), doc);
    assert!(rig.i.parts().badge.is_none() && rig.i.parts().active_handle.is_none());
    assert_eq!(rig.undo_depth(), 0);
}

#[test]
fn resizing_a_multi_selection_scales_all_of_it() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let b = rig.rect(300.0, 100.0, 100.0, 100.0);
    rig.pick(&[&a, &b]);
    let from = rig.handle_pos(&Handle::Right);
    rig.drag((from.x, from.y), (from.x + 150.0, from.y));
    assert!(rig.bounds(&b).0 > 300.0 && rig.bounds(&a).2 > 100.0);
    assert_eq!(rig.ring_ids(), vec!["element.resize"]);
}

// ---- rotate (E4) -----------------------------------------------------------------------------------------------

#[test]
fn rotating_with_the_handle_turns_the_element_and_shows_the_angle() {
    let (mut rig, a) = selected_rect(0.0);
    let h = rig.handle_pos(&Handle::Rotate);
    // The centre is (300, 250): the handle is straight above it; a point to the right of it is 90 degrees on.
    rig.hold_drag((h.x, h.y), (400.0, 250.0));
    assert!(near(rig.element(&a).transform.angle.to_degrees(), 90.0), "{}", rig.element(&a).transform.angle);
    assert_eq!(rig.i.parts().active_handle, Some(Handle::Rotate));
    match rig.i.parts().badge {
        Some(Badge::Angle { center, degrees }) => {
            assert_eq!(center, Point::new(300.0, 250.0));
            assert!(near(degrees, 90.0));
        }
        other => panic!("expected an angle badge, got {other:?}"),
    }
    rig.up(400.0, 250.0);
    assert_eq!(rig.ring_ids(), vec!["element.rotate"]);
    assert_eq!(rig.undo_depth(), 1);
}

#[test]
fn shift_rotates_in_steps_of_fifteen_degrees() {
    let (mut rig, a) = selected_rect(0.0);
    let h = rig.handle_pos(&Handle::Rotate);
    // About 37 degrees from straight up.
    let end = (300.0 + 100.0 * 37f64.to_radians().sin(), 250.0 - 100.0 * 37f64.to_radians().cos());
    rig.with_mods(SHIFT, |r| r.drag((h.x, h.y), end));
    let turned = rig.element(&a).transform.angle.to_degrees();
    assert!(near(turned, 30.0) || near(turned, 45.0), "snapped to a multiple of 15: {turned}");
    assert!(near((turned / 15.0).round() * 15.0, turned), "a multiple of 15: {turned}");
}

#[test]
fn exception_E4_rotate_equals_plain_command() {
    for (mods, snap) in [(Modifiers::NONE, None), (SHIFT, Some(15.0))] {
        let (mut rig, _) = selected_rect(0.0);
        let h = rig.handle_pos(&Handle::Rotate);
        // From straight above the centre to straight right of it: exactly 90 degrees.
        rig.with_mods(mods, |r| r.drag((h.x, h.y), (400.0, 250.0)));
        let (mut twin, _) = selected_rect(0.0);
        twin.run("element.rotate", json!({ "angle": 90.0, "mode": "by", "snap_degrees": snap }));
        assert_eq!(rig.normalized_doc(), twin.normalized_doc(), "mods {mods:?}");
    }
}

#[test]
fn exception_E4_rotate_a_group_of_two_equals_plain_command() {
    let setup = || {
        let mut rig = Rig::new();
        let a = rig.rect(100.0, 100.0, 100.0, 100.0);
        let b = rig.rect(300.0, 100.0, 100.0, 100.0);
        rig.pick(&[&a, &b]);
        rig
    };
    let mut rig = setup();
    let h = rig.handle_pos(&Handle::Rotate);
    let centre = (250.0, 150.0);
    rig.drag((h.x, h.y), (centre.0 + 100.0, centre.1));
    let mut twin = setup();
    twin.run("element.rotate", json!({ "angle": 90.0 }));
    assert_eq!(rig.normalized_doc(), twin.normalized_doc());
}

#[test]
fn double_click_on_the_rotate_handle_resets_the_angle() {
    let (mut rig, a) = selected_rect(30.0);
    assert!(near(rig.element(&a).transform.angle.to_degrees(), 30.0));
    let setup = rig.ring_ids().len();
    let h = rig.handle_pos(&Handle::Rotate);
    rig.double_click(h.x, h.y);
    assert!(near(rig.element(&a).transform.angle, 0.0), "{}", rig.element(&a).transform.angle);
    assert_eq!(rig.ring_after(setup), vec!["element.rotate"]);
    assert!(rig.ring()[setup].args.contains("\"to\""), "{}", rig.ring()[setup].args);
}

#[test]
fn escape_during_a_rotation_restores_the_angle() {
    let (mut rig, a) = selected_rect(0.0);
    let h = rig.handle_pos(&Handle::Rotate);
    rig.hold_drag((h.x, h.y), (400.0, 250.0));
    rig.with_mods(Modifiers::NONE, |r| {
        r.key(Key::Escape);
    });
    assert!(near(rig.element(&a).transform.angle, 0.0));
    assert!(rig.i.parts().badge.is_none());
}

// ---- line and arrow points (E5) --------------------------------------------------------------------------------

/// A target rectangle at (400, 100, 100, 100) and a free arrow from (100, 150) to (300, 150), selected.
fn free_arrow() -> (Rig, ElementId, ElementId) {
    let mut rig = Rig::new();
    let target = rig.rect(400.0, 100.0, 100.0, 100.0);
    rig.run("element.create", json!({ "kind": "arrow", "points": [[100, 150], [300, 150]], "bind": "off" }));
    let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("arrow").id;
    rig.pick(&[&arrow]);
    (rig, arrow, target)
}

fn end_binding(rig: &Rig, arrow: &ElementId) -> Option<ElementId> {
    rig.element(arrow).linear_data().and_then(|d| d.end_binding.as_ref().map(|b| b.target.clone()))
}

#[test]
fn dragging_an_arrow_end_onto_a_rectangle_binds_it_and_shows_the_candidate() {
    let (mut rig, arrow, target) = free_arrow();
    let setup = rig.ring_ids().len();
    let undo0 = rig.undo_depth();
    let end = rig.handle_pos(&Handle::Point { element: arrow.clone(), index: 1 });
    assert_eq!(end, Point::new(300.0, 150.0));
    rig.hold_drag((end.x, end.y), (450.0, 150.0));
    assert_eq!(rig.i.parts().highlight, vec![target.clone()], "the candidate is highlighted during the drag");
    assert_eq!(rig.i.overlay(&rig.s).canvas.bind_candidates.len(), 1);
    assert_eq!(rig.i.parts().active_handle, Some(Handle::Point { element: arrow.clone(), index: 1 }));
    rig.up(450.0, 150.0);
    assert_eq!(end_binding(&rig, &arrow), Some(target), "the binding is stored");
    assert!(rig.i.parts().highlight.is_empty(), "the highlight goes with the drag");
    assert_eq!(rig.ring_after(setup), vec!["arrow.move_point"]);
    assert_eq!(rig.undo_depth(), undo0 + 1);
}

#[test]
fn ctrl_keeps_the_arrow_end_from_binding() {
    let (mut rig, arrow, _) = free_arrow();
    let end = rig.handle_pos(&Handle::Point { element: arrow.clone(), index: 1 });
    rig.with_mods(CTRL, |r| r.hold_drag((end.x, end.y), (450.0, 150.0)));
    assert!(rig.i.parts().highlight.is_empty());
    rig.with_mods(CTRL, |r| r.up(450.0, 150.0));
    assert_eq!(end_binding(&rig, &arrow), None);
}

#[test]
fn exception_E5_point_drag_equals_plain_command() {
    // A bind.
    let (mut rig, arrow, _) = free_arrow();
    let end = rig.handle_pos(&Handle::Point { element: arrow.clone(), index: 1 });
    rig.drag((end.x, end.y), (450.0, 150.0));
    let (mut twin, tarrow, _) = free_arrow();
    twin.run("arrow.move_point", json!({ "arrow": tarrow.as_str(), "index": 1, "to": [450.0, 150.0] }));
    assert_eq!(rig.normalized_doc(), twin.normalized_doc(), "bind");
    assert!(end_binding(&rig, &arrow).is_some());

    // No binding with Ctrl.
    let (mut rig, arrow, _) = free_arrow();
    let end = rig.handle_pos(&Handle::Point { element: arrow.clone(), index: 1 });
    rig.with_mods(CTRL, |r| r.drag((end.x, end.y), (450.0, 150.0)));
    let (mut twin, tarrow, _) = free_arrow();
    twin.run("arrow.move_point", json!({ "arrow": tarrow.as_str(), "index": 1, "to": [450.0, 150.0], "bind": "off" }));
    assert_eq!(rig.normalized_doc(), twin.normalized_doc(), "ctrl");

    // An unbind: an arrow bound at both ends is dragged away from its target.
    let bound = || {
        let mut rig = Rig::new();
        let a = rig.rect(100.0, 100.0, 100.0, 100.0);
        let b = rig.rect(400.0, 100.0, 100.0, 100.0);
        let arrow = testing::create_arrow_between(&mut rig.s, &a, &b).expect("arrow");
        rig.pick(&[&arrow]);
        (rig, arrow)
    };
    let (mut rig, arrow) = bound();
    assert!(end_binding(&rig, &arrow).is_some());
    let end = rig.handle_pos(&Handle::Point { element: arrow.clone(), index: 1 });
    rig.drag((end.x, end.y), (620.0, 400.0));
    assert_eq!(end_binding(&rig, &arrow), None, "dragged away: unbound");
    let (mut twin, tarrow) = bound();
    twin.run("arrow.move_point", json!({ "arrow": tarrow.as_str(), "index": 1, "to": [620.0, 400.0] }));
    assert_eq!(rig.normalized_doc(), twin.normalized_doc(), "unbind");

    // An insert: dragging a midpoint adds a vertex.
    let (mut rig, arrow, _) = free_arrow();
    let mid = rig.handle_pos(&Handle::Midpoint { element: arrow.clone(), segment: 0 });
    rig.drag((mid.x, mid.y), (200.0, 260.0));
    assert_eq!(rig.element(&arrow).linear_data().map(|d| d.points.len()), Some(3));
    let (mut twin, tarrow, _) = free_arrow();
    twin.run("arrow.move_point", json!({ "arrow": tarrow.as_str(), "index": 1, "insert": true, "to": [200.0, 260.0] }));
    assert_eq!(rig.normalized_doc(), twin.normalized_doc(), "insert");
}

#[test]
fn dragging_a_vertex_of_a_polyline_moves_only_that_vertex() {
    let mut rig = Rig::new();
    rig.run("element.create", json!({ "kind": "line", "points": [[100, 100], [200, 200], [300, 100]] }));
    let line = rig.elements().into_iter().next().expect("line").id;
    rig.pick(&[&line]);
    let v = rig.handle_pos(&Handle::Point { element: line.clone(), index: 1 });
    rig.drag((v.x, v.y), (v.x, v.y + 60.0));
    let pts = rig.element(&line).linear_data().map(|d| d.points.clone()).unwrap_or_default();
    let t = rig.element(&line).transform;
    let world: Vec<Point> = pts.iter().map(|p| t.local_to_world(*p)).collect();
    assert_eq!(world.len(), 3);
    assert!(near(world[1].y, 260.0) && near(world[0].y, 100.0) && near(world[2].y, 100.0), "{world:?}");
}

#[test]
fn escape_during_a_point_drag_restores_the_arrow_and_clears_the_highlight() {
    let (mut rig, arrow, _) = free_arrow();
    let doc = rig.normalized_doc();
    let end = rig.handle_pos(&Handle::Point { element: arrow, index: 1 });
    rig.hold_drag((end.x, end.y), (450.0, 150.0));
    assert!(!rig.i.parts().highlight.is_empty());
    rig.key(Key::Escape);
    assert_eq!(rig.normalized_doc(), doc);
    assert!(rig.i.parts().highlight.is_empty() && rig.i.parts().active_handle.is_none());
}

#[test]
fn a_click_on_a_vertex_handle_selects_that_point_in_the_point_editor() {
    let mut rig = Rig::new();
    rig.run("element.create", json!({ "kind": "line", "points": [[100, 100], [200, 200], [300, 100]] }));
    let line = rig.elements().into_iter().next().expect("line").id;
    rig.pick(&[&line]);
    let setup = rig.ring_ids().len();
    let undo = rig.undo_depth();
    let doc = rig.normalized_doc();
    let v = rig.handle_pos(&Handle::Point { element: line.clone(), index: 1 });
    rig.click(v.x, v.y);
    assert_eq!(rig.ring_after(setup), vec!["arrow.edit_points"]);
    let ed = rig.s.tab().observed.linear_editor.clone().expect("the point editor is on");
    assert_eq!((ed.element, ed.selected_points.into_iter().collect::<Vec<_>>()), (line.clone(), vec![1]));
    assert_eq!(rig.normalized_doc(), doc, "a click moves nothing");
    assert_eq!(rig.undo_depth(), undo);
    // Another vertex replaces the selected point.
    let w = rig.handle_pos(&Handle::Point { element: line.clone(), index: 2 });
    rig.click(w.x, w.y);
    let ed = rig.s.tab().observed.linear_editor.clone().expect("still on");
    assert_eq!(ed.selected_points.into_iter().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn a_drag_of_a_vertex_handle_still_moves_the_point_and_a_click_on_a_midpoint_selects_nothing() {
    let mut rig = Rig::new();
    rig.run("element.create", json!({ "kind": "line", "points": [[100, 100], [200, 200], [300, 100]] }));
    let line = rig.elements().into_iter().next().expect("line").id;
    rig.pick(&[&line]);
    let setup = rig.ring_ids().len();
    let m = rig.handle_pos(&Handle::Midpoint { element: line.clone(), segment: 0 });
    rig.click(m.x, m.y);
    assert!(rig.ring_after(setup).is_empty(), "{:?}", rig.ring_after(setup));
    let v = rig.handle_pos(&Handle::Point { element: line.clone(), index: 1 });
    rig.drag((v.x, v.y), (v.x, v.y + 60.0));
    assert_eq!(rig.ring_after(setup), vec!["arrow.move_point"]);
}

#[test]
fn double_click_on_a_point_handle_does_nothing_and_on_a_line_edits_its_points() {
    let (mut rig, arrow, _) = free_arrow();
    let setup = rig.ring_ids().len();
    let doc = rig.normalized_doc();
    let end = rig.handle_pos(&Handle::Point { element: arrow.clone(), index: 1 });
    rig.double_click(end.x, end.y);
    // Its clicks select the point (see a_click_on_a_vertex_handle_...); the double click itself adds nothing.
    assert!(rig.ring_after(setup).iter().all(|c| c == "arrow.edit_points"), "{:?}", rig.ring_after(setup));
    assert_eq!(rig.normalized_doc(), doc);
    assert!(rig.s.tab().observed.linear_editor.as_ref().is_some_and(|e| e.selected_points.iter().eq([1].iter())));

    let mut rig = Rig::new();
    rig.run("element.create", json!({ "kind": "line", "points": [[100, 100], [300, 100], [300, 300]] }));
    let line = rig.elements().into_iter().next().expect("line").id;
    let setup = rig.ring_ids().len();
    rig.double_click(150.0, 100.0);
    assert!(rig.ring_after(setup).contains(&"arrow.edit_points".to_owned()), "{:?}", rig.ring_after(setup));
    assert_eq!(rig.s.tab().observed.linear_editor.as_ref().map(|e| e.element.clone()), Some(line));
}

// ---- elbow arrows ----------------------------------------------------------------------------------------------

fn elbow() -> (Rig, ElementId) {
    let mut rig = Rig::new();
    rig.run(
        "element.create",
        json!({ "kind": "arrow", "points": [[100, 100], [500, 400]], "arrow_type": "elbow", "bind": "off" }),
    );
    let arrow = rig.elements().into_iter().next().expect("arrow").id;
    rig.pick(&[&arrow]);
    (rig, arrow)
}

/// The handle of the elbow's first segment that has a midpoint handle, and its number as the command counts it.
fn first_segment_handle(rig: &Rig, arrow: &ElementId) -> (Handle, usize) {
    let overlay = rig.i.overlay(&rig.s);
    let handles = overlay.canvas.transformer.map(|t| t.handles).unwrap_or_default();
    handles
        .into_iter()
        .find_map(|h| match h.handle {
            Handle::Midpoint { ref element, segment } if element == arrow => Some((h.handle.clone(), segment + 1)),
            _ => None,
        })
        .expect("a midpoint handle")
}

#[test]
fn dragging_an_elbow_segment_handle_pins_the_segment() {
    let (mut rig, arrow) = elbow();
    assert_eq!(rig.element(&arrow).linear_data().map(|d| d.path), Some(PathType::Elbow));
    let setup = rig.ring_ids().len();
    let undo0 = rig.undo_depth();
    let (handle, _) = first_segment_handle(&rig, &arrow);
    let at = rig.handle_pos(&handle);
    rig.drag((at.x, at.y), (at.x + 30.0, at.y + 30.0));
    assert_eq!(rig.ring_after(setup), vec!["arrow.move_segment"], "a gesture of arrow.move_segment");
    let undo_after = rig.undo_depth();
    let pinned = rig.element(&arrow).linear_data().map(|d| d.fixed_segments.len());
    assert_eq!(pinned, Some(1));
    assert_eq!(undo_after, undo0 + 1);
}

#[test]
fn exception_E5_elbow_segment_drag_equals_plain_command() {
    let (mut rig, arrow) = elbow();
    let (handle, number) = first_segment_handle(&rig, &arrow);
    let at = rig.handle_pos(&handle);
    rig.drag((at.x, at.y), (at.x + 30.0, at.y + 30.0));
    let (mut twin, tarrow) = elbow();
    twin.run(
        "arrow.move_segment",
        json!({ "arrow": tarrow.as_str(), "segment": number, "to": [at.x + 30.0, at.y + 30.0] }),
    );
    assert_eq!(rig.normalized_doc(), twin.normalized_doc());
}

#[test]
fn double_click_on_an_elbow_segment_handle_releases_the_pin() {
    let (mut rig, arrow) = elbow();
    let (handle, _) = first_segment_handle(&rig, &arrow);
    let at = rig.handle_pos(&handle);
    rig.drag((at.x, at.y), (at.x + 30.0, at.y + 30.0));
    assert_eq!(rig.element(&arrow).linear_data().map(|d| d.fixed_segments.len()), Some(1));
    // The pinned segment's handle has moved with it: find it again.
    let (handle, _) = first_segment_handle(&rig, &arrow);
    let at = rig.handle_pos(&handle);
    rig.now += 600.0;
    rig.double_click(at.x, at.y);
    assert_eq!(rig.element(&arrow).linear_data().map(|d| d.fixed_segments.len()), Some(0), "released");
    assert!(rig.ring_ids().contains(&"arrow.release_segment".to_owned()), "{:?}", rig.ring_ids());
}

#[test]
fn an_elbow_middle_vertex_cannot_be_dragged_and_says_so_once() {
    let (mut rig, arrow) = elbow();
    let n = rig.element(&arrow).linear_data().map_or(0, |d| d.points.len());
    assert!(n > 2, "the route has middle vertices");
    let v = rig.handle_pos(&Handle::Point { element: arrow.clone(), index: 1 });
    let doc = rig.normalized_doc();
    rig.drag((v.x, v.y), (v.x + 40.0, v.y));
    let refusals: usize = rig.history.iter().map(|o| o.refusals.len()).sum();
    assert_eq!(refusals, 1);
    assert_eq!(rig.normalized_doc(), doc);
}

// ---- double-click ----------------------------------------------------------------------------------------------

#[test]
fn double_click_on_a_group_enters_it_and_the_next_click_selects_a_member() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let b = rig.rect(300.0, 100.0, 100.0, 100.0);
    rig.pick(&[&a, &b]);
    rig.run("group.create", json!({}));
    let group = rig.element(&a).group_ids.last().cloned().expect("grouped");
    rig.tap(150.0, 150.0);
    assert_eq!(rig.selected().len(), 2, "the first click selects the group");
    rig.now += 600.0;
    rig.double_click(150.0, 150.0);
    assert_eq!(rig.s.tab().observed.editing_group, Some(group), "the group is being edited");
    assert_eq!(rig.selected(), vec![a.as_str()], "the double-clicked member is selected");
    assert!(rig.ring_ids().contains(&"group.enter".to_owned()));
    rig.tap(350.0, 150.0);
    assert_eq!(rig.selected(), vec![b.as_str()], "inside the group a click selects one member");
    assert!(rig.last.ui_request.is_none());
}

#[test]
fn double_click_on_a_member_of_an_entered_group_edits_it() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let b = rig.rect(300.0, 100.0, 100.0, 100.0);
    rig.pick(&[&a, &b]);
    rig.run("group.create", json!({}));
    rig.run("group.enter", json!({ "select": a.as_str() }));
    rig.double_click(150.0, 150.0);
    assert_eq!(rig.s.text_edit.as_ref().and_then(|t| t.element.clone()), Some(a));
}

fn text_edit_target(rig: &Rig) -> Option<ElementId> {
    rig.s.text_edit.as_ref().and_then(|t| t.element.clone())
}

#[test]
fn double_click_on_a_shape_starts_a_text_edit_and_asks_for_the_editor() {
    use drafftink_scene::{Color, SceneTx, StoreError};
    for (name, kind) in
        [("rectangle", ElementKind::Rectangle), ("ellipse", ElementKind::Ellipse), ("diamond", ElementKind::Diamond)]
    {
        let mut rig = Rig::new();
        let shape = testing::add(&mut rig.s, kind, 100.0, 100.0, 200.0, 100.0).expect("shape");
        rig.s
            .tab_mut()
            .doc
            .edit::<(), StoreError>(|tx| tx.update(&shape, |e| e.style.fill_color = Some(Color::rgb(255, 255, 255))))
            .expect("fill");
        rig.s.tab_mut().doc.commit_untracked().expect("commit");
        rig.double_click(180.0, 150.0);
        assert_eq!(text_edit_target(&rig), Some(shape.clone()), "{name}");
        assert_eq!(
            rig.last.ui_request,
            Some(UiRequest::OpenTextEditor { element: Some(shape), at: Point::new(180.0, 150.0), width: None }),
            "{name}"
        );
    }
}

#[test]
fn double_click_on_text_starts_a_text_edit() {
    let mut rig = Rig::new();
    rig.run("element.create", json!({ "kind": "text", "text": "hello", "at": [100, 100] }));
    let text = rig.elements().into_iter().next().expect("text");
    let (x, y, w, h) = (text.transform.x, text.transform.y, text.transform.w, text.transform.h);
    rig.double_click(x + w / 2.0, y + h / 2.0);
    assert_eq!(text_edit_target(&rig), Some(text.id.clone()));
    assert_eq!(
        rig.last.ui_request,
        Some(UiRequest::OpenTextEditor {
            element: Some(text.id),
            at: Point::new(x + w / 2.0, y + h / 2.0),
            width: None
        })
    );
}

#[test]
fn double_click_on_math_opens_the_equation_editor() {
    let mut rig = Rig::new();
    rig.run("element.create", json!({ "kind": "math", "at": [100, 100] }));
    let math = rig.elements().into_iter().next().expect("math");
    let (x, y, w, h) = (math.transform.x, math.transform.y, math.transform.w, math.transform.h);
    rig.double_click(x + w / 2.0, y + h / 2.0);
    assert_eq!(text_edit_target(&rig), Some(math.id.clone()));
    assert_eq!(rig.last.ui_request, Some(UiRequest::OpenEquationEditor { element: math.id }));
}

#[test]
fn double_click_on_an_arrow_starts_a_label_edit() {
    let (mut rig, arrow, _) = free_arrow();
    rig.pick(&[]);
    rig.double_click(150.0, 150.0);
    assert_eq!(text_edit_target(&rig), Some(arrow));
}

#[test]
fn double_click_on_empty_canvas_starts_a_draft_at_the_point() {
    let (mut rig, _, _) = two_rects();
    rig.double_click(700.0, 100.0);
    let edit = rig.s.text_edit.as_ref().expect("a text edit");
    assert!(edit.draft && edit.element.is_none());
    assert_eq!(
        rig.last.ui_request,
        Some(UiRequest::OpenTextEditor { element: None, at: Point::new(700.0, 100.0), width: None })
    );
}

#[test]
fn double_click_on_a_stroke_does_nothing() {
    let mut rig = Rig::new();
    rig.run("element.create", json!({ "kind": "freehand", "points": [[100, 100], [200, 120], [300, 100]] }));
    let setup = rig.ring_ids().len();
    rig.double_click(200.0, 120.0);
    assert!(rig.s.text_edit.is_none());
    assert!(rig.last.ui_request.is_none());
    assert!(rig.ring_all_ids().iter().skip(setup).all(|i| i == "selection.select_at"), "{:?}", rig.ring_all_ids());
}

#[test]
fn a_double_click_on_a_vanished_element_opens_no_editor() {
    let (mut rig, a, _) = two_rects();
    rig.run("element.delete", json!({ "ids": [a.as_str()] }));
    // A stale target: the element is gone when the double-click arrives.
    let m = meta(Modifiers::NONE);
    rig.intent(Intent::DoubleClick { at: pos(150.0, 150.0), target: on(&a), meta: m }, Modifiers::NONE);
    assert!(rig.last.ui_request.is_none());
    assert!(rig.s.text_edit.is_none());
}

// ---- the overlay and the layer ---------------------------------------------------------------------------------

#[test]
fn the_tool_keeps_its_hands_off_the_laser_trail_and_the_draw_preview() {
    use super::super::overlay::LaserPoint;
    let (mut rig, a, _) = two_rects();
    rig.pick(&[&a]);
    rig.i.parts.laser.push(LaserPoint { at: Point::new(1.0, 2.0), t_ms: 0.0 });
    rig.drag((150.0, 150.0), (170.0, 150.0));
    rig.drag((50.0, 50.0), (60.0, 60.0));
    assert_eq!(rig.i.parts().laser.len(), 1);
}

#[test]
fn the_overlay_redraws_during_a_drag_and_the_layer_is_busy() {
    let (mut rig, _, _) = two_rects();
    rig.tap(150.0, 150.0);
    rig.down(150.0, 150.0);
    rig.drag_to(170.0, 150.0);
    let out = rig.drag_to(180.0, 150.0);
    assert!(out.redraw.is_some(), "every frame redraws");
    assert!(rig.i.is_busy());
    rig.up(180.0, 150.0);
    assert!(!rig.i.is_busy());
}

#[test]
fn a_hand_built_intent_sequence_selects_marquees_and_moves() {
    let (mut rig, a, b) = two_rects();
    let m = meta(Modifiers::NONE);
    // Click on empty canvas, then a marquee from empty canvas.
    rig.intent(Intent::Press { at: pos(700.0, 50.0), target: Target::Empty, meta: m.clone() }, Modifiers::NONE);
    rig.intent(Intent::Click { at: pos(700.0, 50.0), target: Target::Empty, meta: m.clone() }, Modifiers::NONE);
    rig.intent(
        Intent::DragStart { origin: pos(50.0, 50.0), at: pos(60.0, 60.0), target: Target::Empty, meta: m.clone() },
        Modifiers::NONE,
    );
    rig.intent(
        Intent::DragUpdate {
            origin: pos(50.0, 50.0),
            at: pos(320.0, 220.0),
            delta_screen: Vec2::new(270.0, 170.0),
            delta_world: Vec2::new(270.0, 170.0),
            target: Target::Empty,
            meta: m.clone(),
        },
        Modifiers::NONE,
    );
    assert!(rig.i.parts().marquee.is_some());
    rig.intent(
        Intent::DragEnd {
            origin: pos(50.0, 50.0),
            at: pos(320.0, 220.0),
            delta_screen: Vec2::new(270.0, 170.0),
            delta_world: Vec2::new(270.0, 170.0),
            target: Target::Empty,
            cancelled: false,
            meta: m.clone(),
        },
        Modifiers::NONE,
    );
    assert_eq!(rig.selected(), vec![a.as_str()]);
    // A move drag by intents, ended normally.
    rig.intent(
        Intent::DragStart { origin: pos(150.0, 150.0), at: pos(155.0, 150.0), target: on(&a), meta: m.clone() },
        Modifiers::NONE,
    );
    rig.intent(
        Intent::DragEnd {
            origin: pos(150.0, 150.0),
            at: pos(250.0, 150.0),
            delta_screen: Vec2::new(100.0, 0.0),
            delta_world: Vec2::new(100.0, 0.0),
            target: on(&a),
            cancelled: false,
            meta: m,
        },
        Modifiers::NONE,
    );
    assert_eq!(rig.bounds(&a).0, 200.0, "the end frame carries the final delta");
    assert_eq!(rig.bounds(&b).0, 500.0);
    assert_eq!(rig.ring_ids().last().map(String::as_str), Some("element.move"));
}

#[test]
fn zoomed_and_scrolled_views_map_the_pointer_to_world_units() {
    let mut rig = Rig::new();
    rig.run("view.set", json!({ "scroll": [1000, 500], "zoom": 2 }));
    // The scroll is the world point at the middle of the 800 by 600 canvas.
    let a = rig.rect(1100.0, 600.0, 100.0, 50.0);
    let before = rig.bounds(&a);
    let screen = |wx: f64, wy: f64| ((wx - 1000.0) * 2.0 + 400.0, (wy - 500.0) * 2.0 + 300.0);
    let start = screen(1150.0, 625.0);
    rig.drag(start, (start.0 + 40.0, start.1 + 20.0));
    let after = rig.bounds(&a);
    assert!(near(after.0 - before.0, 20.0) && near(after.1 - before.1, 10.0), "moved by screen / zoom: {after:?}");
}

#[test]
fn dropping_over_a_frame_highlights_it_during_the_drag_and_joins_it_on_release() {
    let mut rig = Rig::new();
    rig.run("element.create", json!({ "kind": "frame", "bounds": { "x": 400, "y": 300, "w": 300, "h": 250 } }));
    let frame = rig.elements().into_iter().next().expect("frame").id;
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.hold_drag((150.0, 150.0), (500.0, 400.0));
    assert_eq!(rig.i.parts().highlight, vec![frame.clone()], "the frame that would take the element");
    rig.up(500.0, 400.0);
    assert!(rig.i.parts().highlight.is_empty());
    assert_eq!(rig.element(&a).frame_id, Some(frame), "membership is set once, by the committed run");
}
