//! Tests of the sticky tool: a click places a note and opens it for typing, and the tool goes back to select
//! unless it is locked. A drag sizes nothing: the note goes where the pointer is let go.

use drafftink_scene::Kind;
use serde_json::json;

use crate::interact::testutil::Rig;
use crate::state::Tool;

fn sticky_rig() -> Rig {
    let mut rig = Rig::new();
    rig.tool(Tool::Sticky);
    rig
}

/// The notes of the rig: rectangles with a label bound to them.
fn notes(rig: &Rig) -> Vec<(f64, f64, f64, f64)> {
    rig.elements().iter().filter(|e| e.tag() == Kind::Rectangle).map(|e| rig.bounds(&e.id)).collect()
}

#[test]
fn a_click_places_a_250_square_note_centred_on_the_point_and_opens_it_for_typing() {
    let mut rig = sticky_rig();
    rig.click(300.0, 200.0);
    assert_eq!(notes(&rig), [(175.0, 75.0, 250.0, 250.0)]);
    let note = rig.only_note();
    assert_eq!(rig.selected(), [note.as_str()]);
    let edit = rig.s.text_edit.as_ref().expect("the note is open for typing");
    assert_eq!(edit.element.as_ref().map(|id| id.as_str()), Some(note.as_str()));
}

#[test]
fn the_tool_goes_back_to_select_after_a_note_unless_it_is_locked() {
    let mut rig = sticky_rig();
    rig.click(100.0, 100.0);
    assert_eq!(rig.s.tool.active, Tool::Select);

    let mut locked = sticky_rig();
    locked.run("tool.lock", json!({ "on": true }));
    locked.click(100.0, 100.0);
    assert_eq!(locked.s.tool.active, Tool::Sticky, "the lock keeps the tool");
    locked.run("text.edit_end", json!({ "commit": true }));
    locked.click(600.0, 100.0);
    assert_eq!(notes(&locked).len(), 2, "and the next click makes the next note");
}

#[test]
fn a_click_is_one_create_sticky_and_one_undo_step() {
    let mut rig = sticky_rig();
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());
    rig.click(300.0, 200.0);
    let made = rig.commands_since(ring);
    assert!(made.iter().any(|c| c == "element.create_sticky"), "{made:?}");
    assert_eq!(rig.undo_depth(), undo + 1);
}

#[test]
fn a_click_over_an_element_still_places_a_note() {
    let mut rig = Rig::new();
    rig.rect(250.0, 150.0, 100.0, 100.0);
    rig.tool(Tool::Sticky);
    rig.click(300.0, 200.0);
    assert_eq!(notes(&rig).len(), 2);
}

#[test]
fn a_drag_sizes_nothing_and_places_the_note_where_the_pointer_is_let_go() {
    let mut rig = sticky_rig();
    rig.drag((100.0, 100.0), (500.0, 400.0));
    assert_eq!(notes(&rig), [(375.0, 275.0, 250.0, 250.0)]);
    assert_eq!(rig.s.tool.active, Tool::Select);
}

#[test]
fn a_cancelled_drag_places_nothing() {
    let mut rig = sticky_rig();
    rig.down(100.0, 100.0);
    rig.drag_to(300.0, 300.0);
    rig.key(crate::interact::Key::Escape);
    rig.up(300.0, 300.0);
    assert!(rig.elements().is_empty());
}

#[test]
fn view_only_refuses_the_tool_with_the_shared_words() {
    let mut rig = Rig::new();
    rig.run("view.set_view_only", json!({ "on": true }));
    assert_eq!(rig.refuse("tool.set", json!({ "tool": "sticky" })).code, "view_only_tool");
}

#[test]
#[allow(non_snake_case)]
fn exception_E20_a_sticky_click_equals_the_plain_command() {
    let mut gesture = sticky_rig();
    gesture.click(300.0, 200.0);
    let mut plain = Rig::new();
    plain.run("element.create_sticky", json!({ "at": [300, 200], "edit": true, "from_tool": true }));
    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
    assert_eq!(gesture.s.tool.active, plain.s.tool.active);
    assert_eq!(gesture.selected().len(), plain.selected().len());
    assert_eq!(gesture.s.text_edit.is_some(), plain.s.text_edit.is_some());
    assert_eq!(gesture.undo_depth(), plain.undo_depth());
}

impl Rig {
    /// The id of the one note the rig holds.
    fn only_note(&self) -> drafftink_scene::ElementId {
        let mut notes = self.elements().into_iter().filter(|e| e.tag() == Kind::Rectangle);
        let note = notes.next().expect("a note");
        assert!(notes.next().is_none(), "only one note");
        note.id
    }
}
