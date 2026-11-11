//! Tests of the drawing tools: shapes, the tool lock and the view-only gate. Lines, strokes, the eraser, the
//! laser and the text tools are in the sibling test files. Every gesture goes through the real `Recognizer`.

use drafftink_canvas::{Buttons, InputEvent, PointerEvent, PointerPhase};
use drafftink_scene::{Element, Kind};
use serde_json::{Value, json};

use crate::interact::overlay::OverlayParts;
use crate::interact::testutil::Rig;
use crate::interact::{Key, Outcome};
use crate::refusal::Refusal;
use crate::state::Tool;

// ---- helpers shared by the sibling test files -----------------------------------------------------------------

impl Rig {
    /// The overlay parts the machines own.
    pub(super) fn parts(&self) -> &OverlayParts {
        self.i.parts()
    }

    /// The preview element, if one shows.
    pub(super) fn preview(&self) -> Option<Element> {
        self.parts().preview.clone()
    }

    /// Writes a setting.
    pub(super) fn set(&mut self, key: &str, value: Value) {
        self.run("settings.set", json!({ "key": key, "value": value }));
    }

    /// The mutating commands that ran after the first `since` entries of the ring.
    pub(super) fn commands_since(&self, since: usize) -> Vec<String> {
        self.ring_ids().into_iter().skip(since).collect()
    }

    /// A pen event: contact with `pressure` (a move with the tip down is a drag sample).
    pub(super) fn pen(&mut self, phase: PointerPhase, x: f64, y: f64, pressure: f32) -> Outcome {
        self.now += 8.0;
        let mut ev = PointerEvent::pen(phase, x, y, pressure, self.now).with_modifiers(self.mods);
        if phase == PointerPhase::Move {
            ev.buttons = Buttons::PRIMARY;
        }
        self.input(InputEvent::Pointer(ev))
    }

    /// Presses at the first point, moves through the rest, and releases at the last one.
    pub(super) fn trace(&mut self, points: &[(f64, f64)]) {
        let (first, rest) = points.split_first().expect("a trace has points");
        self.down(first.0, first.1);
        for p in rest {
            self.drag_to(p.0, p.1);
        }
        let last = points.last().expect("a trace has points");
        self.up(last.0, last.1);
    }

    /// The live elements that did not exist in `before`.
    pub(super) fn added_since(&self, before: usize) -> Vec<Element> {
        self.elements().into_iter().skip(before).collect()
    }

    /// The single element the rig holds.
    pub(super) fn only(&self) -> Element {
        let mut all = self.elements();
        assert_eq!(all.len(), 1, "expected exactly one element, got {}", all.len());
        all.remove(0)
    }

    /// The refusal text of the last outcome, if any.
    pub(super) fn last_refusal(&self) -> Option<Refusal> {
        self.last.refusals.first().cloned()
    }
}

const SHAPES: [(Tool, Kind); 4] = [
    (Tool::Rectangle, Kind::Rectangle),
    (Tool::Diamond, Kind::Diamond),
    (Tool::Ellipse, Kind::Ellipse),
    (Tool::Frame, Kind::Frame),
];

/// A rig with the tool chosen and the ring position noted.
fn rig_with(tool: Tool) -> (Rig, usize, usize) {
    let mut rig = Rig::new();
    rig.tool(tool);
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());
    (rig, ring, undo)
}

// ---- shapes -----------------------------------------------------------------------------------------------------

#[test]
fn every_shape_tool_draws_one_element_with_one_command_and_one_undo_step() {
    for (tool, kind) in SHAPES {
        let (mut rig, ring, undo) = rig_with(tool);
        rig.drag((100.0, 100.0), (220.0, 180.0));

        assert_eq!(rig.commands_since(ring), ["element.create"], "{tool:?}");
        assert_eq!(rig.undo_depth(), undo + 1, "{tool:?}: one undo step");
        let e = rig.only();
        assert_eq!(e.tag(), kind);
        assert_eq!(rig.bounds(&e.id), (100.0, 100.0, 120.0, 80.0), "{tool:?}");
        assert_eq!(rig.selected(), [e.id.as_str()], "{tool:?}: the new element is selected");
        assert_eq!(rig.s.tool.active, Tool::Select, "{tool:?}: from_tool returns to select");
        assert!(!rig.parts().has_gesture_state(), "{tool:?}: the preview is gone");
        assert!(!rig.i.is_busy());
    }
}

#[test]
fn the_preview_shows_mid_drag_and_the_document_is_untouched() {
    for (tool, kind) in SHAPES {
        let (mut rig, ring, undo) = rig_with(tool);
        rig.down(100.0, 100.0);
        let out = rig.drag_to(160.0, 150.0);
        assert!(out.redraw.is_some(), "{tool:?}: the preview asks for an overlay redraw");

        let p = rig.preview().unwrap_or_else(|| panic!("{tool:?}: no preview mid-drag"));
        assert_eq!(p.tag(), kind);
        assert_eq!((p.transform.x, p.transform.y, p.transform.w, p.transform.h), (100.0, 100.0, 60.0, 50.0));
        assert_eq!(p.style.stroke_color, rig.s.tool.defaults().style.stroke_color, "{tool:?}: session style defaults");
        assert!(rig.i.is_busy());
        assert!(rig.elements().is_empty(), "{tool:?}: nothing in the document yet");
        assert_eq!(rig.commands_since(ring), Vec::<String>::new());
        assert_eq!(rig.undo_depth(), undo);

        rig.up(160.0, 150.0);
        assert!(rig.preview().is_none(), "{tool:?}: the preview is gone after the release");
        assert_eq!(rig.elements().len(), 1);
    }
}

#[test]
fn a_drag_in_any_direction_makes_the_same_box() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    rig.drag((220.0, 180.0), (100.0, 100.0));
    let e = rig.only();
    assert_eq!(rig.bounds(&e.id), (100.0, 100.0, 120.0, 80.0));
}

#[test]
fn a_drag_shorter_than_one_unit_creates_nothing_and_keeps_the_tool() {
    for (tool, _) in SHAPES {
        for (to_x, to_y) in [(100.5, 200.0), (200.0, 100.5), (100.0, 100.0)] {
            let (mut rig, ring, undo) = rig_with(tool);
            rig.drag((100.0, 100.0), (to_x, to_y));
            assert!(rig.elements().is_empty(), "{tool:?} {to_x},{to_y}");
            assert_eq!(rig.commands_since(ring), Vec::<String>::new(), "no command, so no refusal either");
            assert_eq!(rig.undo_depth(), undo);
            assert!(rig.last.refusals.is_empty());
            assert!(rig.history.iter().all(|o| o.refusals.is_empty()));
            assert_eq!(rig.s.tool.active, tool, "the tool stays");
            assert!(!rig.parts().has_gesture_state());
        }
    }
}

#[test]
fn a_click_with_a_shape_tool_creates_nothing() {
    let (mut rig, ring, _) = rig_with(Tool::Rectangle);
    rig.click(300.0, 300.0);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert_eq!(rig.s.tool.active, Tool::Rectangle);
}

#[test]
fn shift_makes_a_square() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    rig.mods.shift = true;
    rig.drag((100.0, 100.0), (220.0, 150.0));
    let e = rig.only();
    assert_eq!(rig.bounds(&e.id), (100.0, 100.0, 120.0, 120.0));
}

#[test]
fn shift_keeps_the_direction_of_the_drag() {
    let (mut rig, _, _) = rig_with(Tool::Ellipse);
    rig.mods.shift = true;
    rig.drag((200.0, 200.0), (150.0, 260.0));
    let e = rig.only();
    assert_eq!(rig.bounds(&e.id), (140.0, 200.0, 60.0, 60.0));
}

#[test]
fn alt_draws_from_the_centre() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    rig.mods.alt = true;
    rig.drag((200.0, 200.0), (260.0, 230.0));
    let e = rig.only();
    assert_eq!(rig.bounds(&e.id), (140.0, 170.0, 120.0, 60.0));
}

#[test]
fn shift_and_alt_draw_a_square_from_the_centre() {
    let (mut rig, _, _) = rig_with(Tool::Diamond);
    rig.mods.shift = true;
    rig.mods.alt = true;
    rig.drag((200.0, 200.0), (260.0, 230.0));
    let e = rig.only();
    assert_eq!(rig.bounds(&e.id), (140.0, 140.0, 120.0, 120.0));
}

#[test]
fn the_grid_snaps_both_corners_when_the_setting_is_on() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    rig.set("snap.grid", json!(true));
    rig.drag((103.0, 98.0), (147.0, 142.0));
    let e = rig.only();
    assert_eq!(rig.bounds(&e.id), (100.0, 100.0, 40.0, 40.0));
}

#[test]
fn without_the_setting_nothing_snaps() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    rig.drag((103.0, 98.0), (147.0, 142.0));
    let e = rig.only();
    assert_eq!(rig.bounds(&e.id), (103.0, 98.0, 44.0, 44.0));
}

#[test]
fn smart_guides_snap_the_dragged_corner_to_another_element_and_show_while_dragging() {
    let mut rig = Rig::new();
    rig.rect(300.0, 100.0, 100.0, 100.0);
    rig.tool(Tool::Rectangle);
    rig.set("snap.smart_guides", json!(true));
    rig.down(100.0, 300.0);
    rig.drag_to(303.0, 152.0);
    assert!(!rig.parts().guides.is_empty(), "guides show while the corner is snapped");
    let p = rig.preview().expect("a preview");
    assert_eq!((p.transform.x, p.transform.y, p.transform.w, p.transform.h), (100.0, 150.0, 200.0, 150.0));
    rig.up(303.0, 152.0);
    assert!(rig.parts().guides.is_empty(), "guides go with the gesture");
    let made = rig.added_since(1);
    assert_eq!(made.len(), 1);
    assert_eq!(rig.bounds(&made[0].id), (100.0, 150.0, 200.0, 150.0));
}

#[test]
fn smart_guides_stay_off_without_the_setting() {
    let mut rig = Rig::new();
    rig.rect(300.0, 100.0, 100.0, 100.0);
    rig.tool(Tool::Rectangle);
    rig.down(100.0, 300.0);
    rig.drag_to(303.0, 152.0);
    assert!(rig.parts().guides.is_empty());
    let p = rig.preview().expect("a preview");
    assert_eq!((p.transform.w, p.transform.h), (203.0, 148.0));
}

#[test]
fn escape_mid_drag_leaves_document_ring_and_undo_untouched() {
    for (tool, _) in SHAPES {
        let (mut rig, ring, undo) = rig_with(tool);
        let fingerprint = rig.s.fingerprint();
        rig.down(100.0, 100.0);
        rig.drag_to(200.0, 160.0);
        assert!(rig.preview().is_some());

        let out = rig.key(Key::Escape);
        assert!(out.handled, "{tool:?}: the layer used the key");
        assert!(rig.preview().is_none(), "{tool:?}");
        assert!(!rig.parts().has_gesture_state());
        rig.up(200.0, 160.0);

        assert!(rig.elements().is_empty());
        assert_eq!(rig.commands_since(ring), Vec::<String>::new());
        assert_eq!(rig.undo_depth(), undo);
        assert_eq!(rig.s.fingerprint(), fingerprint, "{tool:?}: nothing changed");
        assert_eq!(rig.s.tool.active, tool);
        assert!(!rig.i.is_busy());
    }
}

#[test]
fn a_new_drag_works_after_a_cancelled_one() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    rig.down(100.0, 100.0);
    rig.drag_to(200.0, 160.0);
    rig.key(Key::Escape);
    rig.up(200.0, 160.0);
    rig.drag((300.0, 300.0), (360.0, 340.0));
    let e = rig.only();
    assert_eq!(rig.bounds(&e.id), (300.0, 300.0, 60.0, 40.0));
}

#[test]
fn a_pointer_move_that_changes_nothing_asks_for_no_redraw() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    assert_eq!(rig.hover(300.0, 300.0).redraw, None, "hover with a shape tool");
    assert_eq!(rig.hover(310.0, 300.0).redraw, None);

    rig.down(100.0, 100.0);
    rig.drag_to(150.0, 150.0);
    assert!(rig.drag_to(170.0, 150.0).redraw.is_some(), "a changed box redraws");
    assert_eq!(rig.drag_to(170.0, 150.0).redraw, None, "the same pointer position again changes nothing");
}

#[test]
fn a_tool_change_drops_the_drag() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    rig.down(100.0, 100.0);
    rig.drag_to(200.0, 160.0);
    rig.tool(Tool::Ellipse);
    let ring = rig.ring_ids().len();
    rig.drag_to(210.0, 170.0);
    assert!(rig.preview().is_none(), "the old preview does not outlive its tool");
    rig.up(210.0, 170.0);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
}

// ---- tool lock --------------------------------------------------------------------------------------------------

#[test]
fn without_the_lock_the_tool_returns_to_select_and_a_second_drag_is_not_a_drawing() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    rig.drag((100.0, 100.0), (200.0, 160.0));
    assert_eq!(rig.s.tool.active, Tool::Select);
    rig.drag((300.0, 300.0), (400.0, 360.0));
    assert_eq!(rig.elements().len(), 1, "the second drag was a marquee, not a rectangle");
}

#[test]
fn with_the_lock_the_tool_stays_and_a_second_shape_can_be_drawn() {
    let (mut rig, ring, undo) = rig_with(Tool::Rectangle);
    rig.run("tool.lock", json!({ "on": true }));
    rig.drag((100.0, 100.0), (200.0, 160.0));
    assert_eq!(rig.s.tool.active, Tool::Rectangle);
    rig.drag((300.0, 300.0), (400.0, 360.0));
    assert_eq!(rig.s.tool.active, Tool::Rectangle);

    let all = rig.elements();
    assert_eq!(all.len(), 2);
    assert_eq!(rig.bounds(&all[0].id), (100.0, 100.0, 100.0, 60.0));
    assert_eq!(rig.bounds(&all[1].id), (300.0, 300.0, 100.0, 60.0));
    assert_eq!(rig.commands_since(ring).iter().filter(|c| *c == "element.create").count(), 2);
    assert_eq!(rig.undo_depth(), undo + 2);
    assert_eq!(rig.selected(), [all[1].id.as_str()]);
}

// ---- view-only --------------------------------------------------------------------------------------------------

#[test]
fn in_view_only_mode_the_drawing_tools_are_refused_and_nothing_can_be_drawn() {
    let mut rig = Rig::new();
    rig.run("view.set_view_only", json!({ "on": true }));
    for tool in [
        Tool::Rectangle,
        Tool::Diamond,
        Tool::Ellipse,
        Tool::Frame,
        Tool::Line,
        Tool::Arrow,
        Tool::Freehand,
        Tool::Highlighter,
        Tool::Eraser,
        Tool::Text,
        Tool::Math,
    ] {
        let r = rig.refuse("tool.set", json!({ "tool": tool.name() }));
        assert_eq!(r.code, "view_only_tool", "{tool:?}");
    }
    assert_eq!(rig.s.tool.active, Tool::Select);
    rig.drag((100.0, 100.0), (200.0, 160.0));
    assert!(rig.elements().is_empty());
    rig.run("tool.set", json!({ "tool": "laser" }));
    assert_eq!(rig.s.tool.active, Tool::Laser);
}

#[test]
fn a_shape_drawn_while_view_only_turns_on_is_refused_with_words() {
    let (mut rig, _, _) = rig_with(Tool::Rectangle);
    rig.down(100.0, 100.0);
    rig.drag_to(200.0, 160.0);
    rig.run("view.set_view_only", json!({ "on": true }));
    rig.up(200.0, 160.0);
    assert!(rig.elements().is_empty());
    let r = rig.last_refusal().expect("the refusal reaches the outcome");
    assert_eq!(r.code, "view_only");
    assert!(!r.message.is_empty());
}

// ---- E6: the gesture equals the plain command ---------------------------------------------------------------

/// The rig both runs start from: a rectangle inside the area that gets drawn on (a frame takes it in).
fn e6_rig(tool: Option<Tool>) -> Rig {
    let mut rig = Rig::new();
    rig.rect(110.0, 110.0, 30.0, 30.0);
    rig.rect(500.0, 400.0, 50.0, 50.0);
    if let Some(t) = tool {
        rig.tool(t);
    }
    rig
}

#[test]
#[allow(non_snake_case)]
fn exception_E6_draw_equals_plain_command() {
    for (tool, kind) in SHAPES {
        let mut gesture = e6_rig(Some(tool));
        gesture.drag((100.0, 100.0), (250.0, 200.0));

        let mut plain = e6_rig(None);
        plain.run(
            "element.create",
            json!({ "kind": kind.name(), "bounds": { "x": 100, "y": 100, "w": 150, "h": 100 }, "from_tool": true }),
        );

        assert_eq!(gesture.normalized_doc(), plain.normalized_doc(), "{tool:?}");
        assert_eq!(gesture.selected().len(), plain.selected().len());
        assert_eq!(gesture.s.tool.active, plain.s.tool.active);
        assert_eq!(gesture.undo_depth(), plain.undo_depth());
    }
}

#[test]
#[allow(non_snake_case)]
fn exception_E6_a_frame_drawn_around_elements_takes_them_in() {
    let mut rig = e6_rig(Some(Tool::Frame));
    rig.drag((100.0, 100.0), (250.0, 200.0));
    let frame = rig.elements().into_iter().find(|e| e.tag() == Kind::Frame).expect("a frame");
    let inner = rig.elements().into_iter().find(|e| e.transform.x == 110.0).expect("the inner rectangle");
    assert_eq!(inner.frame_id.as_ref(), Some(&frame.id));
}
