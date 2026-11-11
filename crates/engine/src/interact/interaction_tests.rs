//! Tests of the interaction layer itself: camera, hover, cursor, context menu, cancel plumbing.

use drafftink_canvas::Cursor;

use super::testutil::Rig;

#[test]
fn the_rig_starts_with_screen_equal_to_world() {
    let rig = Rig::new();
    assert_eq!(rig.s.view().zoom, 1.0);
    assert_eq!(rig.s.view().scroll.x, 400.0);
    assert_eq!(rig.s.view().viewport.w, 800.0);
}

#[test]
fn hovering_an_element_sets_a_cursor() {
    let mut rig = Rig::new();
    rig.rect(100.0, 100.0, 100.0, 100.0);
    let out = rig.hover(150.0, 150.0);
    assert_eq!(out.cursor, Some(Cursor::Move));
}
