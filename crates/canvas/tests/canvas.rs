//! The facade: store, index, viewport, recognizer, dispatcher and overlay working together.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use PointerPhase::{Down, Move, Up};
use common::*;
use drafftink_canvas::geom::guides::{SmartGuide, SmartGuideKind};
use drafftink_canvas::geom::kurbo::{Point, Size, Vec2};
use drafftink_canvas::scene::{Color, ElementId, GroupId, MemoryStore, SceneRead, SceneStore, SceneTx};
use drafftink_canvas::{
    Camera, Canvas, Cursor, Dirty, ElementEvent, EventKind, Handle, InputEvent, Intent, Node, PointerEvent, PointerId,
    PointerKind, PointerMode, PointerPhase, RemoteUser, Selector, Target, ViewSource, Viewport,
};
use smallvec::smallvec;

fn mv(c: &mut Canvas<MemoryStore>, x: f64, y: f64, t: f64) -> Vec<Intent> {
    c.handle_input(&InputEvent::Pointer(PointerEvent::mouse(Move, x, y, t)))
}

fn press(c: &mut Canvas<MemoryStore>, x: f64, y: f64, t: f64) -> Vec<Intent> {
    c.handle_input(&InputEvent::Pointer(PointerEvent::mouse(Down, x, y, t)))
}

fn drag_to(c: &mut Canvas<MemoryStore>, x: f64, y: f64, t: f64) -> Vec<Intent> {
    c.handle_input(&InputEvent::Pointer(PointerEvent::mouse(Move, x, y, t).held()))
}

fn release(c: &mut Canvas<MemoryStore>, x: f64, y: f64, t: f64) -> Vec<Intent> {
    c.handle_input(&InputEvent::Pointer(PointerEvent::mouse(Up, x, y, t)))
}

#[test]
fn hovering_the_same_target_costs_no_redraw() {
    let mut c = canvas_with(vec![fill(rect("a", 100.0, 100.0, 300.0, 300.0))]);
    c.take_dirty();
    let first = mv(&mut c, 150.0, 150.0, 0.0);
    assert!(matches!(first.as_slice(), [Intent::Hover { to: Target::Element { .. }, .. }]));
    assert!(c.take_dirty().overlay, "the hover outline appears: one redraw");
    // A thousand moves across the element: no intent, nothing dirty.
    let mut intents = 0;
    let mut redraws = 0;
    for i in 0..1000 {
        intents += mv(&mut c, 120.0 + (i % 250) as f64, 120.0 + (i % 200) as f64, 10.0 + i as f64).len();
        if c.take_dirty().any() {
            redraws += 1;
        }
    }
    assert_eq!((intents, redraws), (0, 0));
    // Leaving the element is one change, and so is the next element.
    assert_eq!(mv(&mut c, 600.0, 500.0, 2000.0).len(), 1);
    assert!(c.take_dirty().overlay);
    assert_eq!(mv(&mut c, 610.0, 510.0, 2010.0).len(), 0);
    assert!(!c.take_dirty().any(), "moving over empty space is free");
    // An idle canvas stays clean.
    assert_eq!(c.take_dirty(), Dirty::default());
    assert!(c.tick(5000.0).is_empty());
    assert!(!c.take_dirty().any());
}

#[test]
fn the_index_follows_every_way_of_changing_the_store() {
    let mut c = canvas_with(vec![fill(rect("a", 0.0, 0.0, 50.0, 50.0))]);
    assert_eq!(c.index().bulk_rebuilds(), 1);
    // Through a guard.
    {
        let mut s = c.store_mut();
        let mut tx = s.begin();
        let mut e = fill(rect("b", 200.0, 200.0, 50.0, 50.0));
        e.index = tx.next_top_index().unwrap();
        tx.insert(e).unwrap();
        tx.commit().unwrap();
    }
    assert!(c.index().contains(&id("b")));
    assert_eq!(c.hit_at(Point::new(225.0, 225.0)).map(|h| h.id), Some(id("b")));
    // A remote update applied inside the store (here: replacing the whole document) reaches the index too.
    let mut doc = drafftink_canvas::scene::snapshot(c.store());
    doc.elements.retain(|e| e.id == id("a"));
    drafftink_canvas::scene::load_document(&mut *c.store_mut(), &doc).unwrap();
    assert!(!c.index().contains(&id("b")));
    assert_eq!(c.hit_at(Point::new(225.0, 225.0)), None);
    assert_eq!(c.index().bulk_rebuilds(), 1, "small change sets stay incremental");
    c.index().verify(c.store()).unwrap();
}

#[test]
fn drag_flow_gives_intents_and_events_with_bubbling() {
    // Two members of group g1 and one loose element.
    let g = GroupId::new("g1").unwrap();
    let mut a = fill(rect("a", 100.0, 100.0, 100.0, 100.0));
    a.group_ids = smallvec![g.clone()];
    a.name = Some("card big".into());
    let mut b = fill(rect("b", 300.0, 100.0, 100.0, 100.0));
    b.group_ids = smallvec![g.clone()];
    let loose = fill(ellipse("e", 100.0, 300.0, 100.0, 100.0));
    let mut c = canvas_with(vec![a, b, loose]);

    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let note = |tag: &'static str, log: &Rc<RefCell<Vec<String>>>| {
        let log = Rc::clone(log);
        Box::new(move |e: &mut ElementEvent| {
            let who = match &e.current {
                Node::Element(id) => id.to_string(),
                Node::Group(g) => format!("group {g}"),
                Node::Stage => "stage".to_string(),
            };
            log.borrow_mut().push(format!("{tag}@{who}"));
        }) as Box<dyn FnMut(&mut ElementEvent)>
    };
    c.on(EventKind::Click, Selector::Id(id("a")), note("click-id", &log));
    c.on(EventKind::Click, "rectangle".parse().unwrap(), note("click-kind", &log));
    c.on(EventKind::Click, ".card".parse().unwrap(), note("click-name", &log));
    c.on(EventKind::Click, "group:g1".parse().unwrap(), note("click-group", &log));
    c.on(EventKind::Click, Selector::Stage, note("click-stage", &log));
    c.on(EventKind::DragMove, Selector::Any, note("move", &log));

    press(&mut c, 150.0, 150.0, 0.0);
    release(&mut c, 150.0, 150.0, 50.0);
    assert_eq!(
        *log.borrow(),
        ["click-id@a", "click-kind@a", "click-name@a", "click-group@group g1", "click-stage@stage"],
        "element listeners in registration order, then the group, then the stage"
    );
    log.borrow_mut().clear();

    // The loose ellipse is not a rectangle and not in the group; only the stage hears it.
    press(&mut c, 150.0, 350.0, 100.0);
    let out = release(&mut c, 150.0, 350.0, 150.0);
    assert!(matches!(out[0], Intent::Click { .. }));
    assert_eq!(*log.borrow(), ["click-stage@stage"]);
    log.borrow_mut().clear();

    // Empty space reaches the stage with no target.
    press(&mut c, 700.0, 500.0, 200.0);
    release(&mut c, 700.0, 500.0, 250.0);
    assert_eq!(*log.borrow(), ["click-stage@stage"]);
    log.borrow_mut().clear();

    // A drag of "a" is delivered to "a" and bubbles, though the pointer ends up over empty space.
    press(&mut c, 150.0, 150.0, 300.0);
    drag_to(&mut c, 200.0, 150.0, 310.0);
    drag_to(&mut c, 700.0, 500.0, 320.0);
    release(&mut c, 700.0, 500.0, 330.0);
    let moves = log.borrow().iter().filter(|s| s.starts_with("move@")).cloned().collect::<Vec<_>>();
    assert_eq!(moves, ["move@a", "move@group g1", "move@stage", "move@a", "move@group g1", "move@stage"]);
}

#[test]
fn stop_propagation_keeps_the_event_below_the_node_that_stopped_it() {
    let g = GroupId::new("g1").unwrap();
    let mut a = fill(rect("a", 100.0, 100.0, 100.0, 100.0));
    a.group_ids = smallvec![g.clone()];
    let mut c = canvas_with(vec![a]);
    let seen: Rc<RefCell<Vec<&'static str>>> = Rc::default();
    let s = Rc::clone(&seen);
    c.on(
        EventKind::Click,
        Selector::Id(id("a")),
        Box::new(move |e| {
            s.borrow_mut().push("element-1");
            e.stop_propagation();
        }),
    );
    let s = Rc::clone(&seen);
    c.on(EventKind::Click, Selector::Id(id("a")), Box::new(move |_| s.borrow_mut().push("element-2")));
    let s = Rc::clone(&seen);
    c.on(EventKind::Click, Selector::Group(g), Box::new(move |_| s.borrow_mut().push("group")));
    let s = Rc::clone(&seen);
    c.on(EventKind::Click, Selector::Stage, Box::new(move |_| s.borrow_mut().push("stage")));
    press(&mut c, 150.0, 150.0, 0.0);
    release(&mut c, 150.0, 150.0, 20.0);
    assert_eq!(*seen.borrow(), ["element-1", "element-2"], "the rest of the node's listeners still run");
}

#[test]
fn off_removes_a_listener() {
    let mut c = canvas_with(vec![fill(rect("a", 100.0, 100.0, 100.0, 100.0))]);
    let n: Rc<RefCell<u32>> = Rc::default();
    let k = Rc::clone(&n);
    let l = c.on(EventKind::Click, Selector::Any, Box::new(move |_| *k.borrow_mut() += 1));
    press(&mut c, 150.0, 150.0, 0.0);
    release(&mut c, 150.0, 150.0, 20.0);
    assert!(c.off(l));
    assert!(!c.off(l));
    press(&mut c, 150.0, 150.0, 1000.0);
    release(&mut c, 150.0, 150.0, 1020.0);
    assert_eq!(*n.borrow(), 2, "element and stage heard the first click, nobody the second");
}

#[test]
fn enter_and_leave_fire_on_groups_once_and_do_not_bubble() {
    let g = GroupId::new("g1").unwrap();
    let mut a = fill(rect("a", 100.0, 100.0, 100.0, 100.0));
    a.group_ids = smallvec![g.clone()];
    let mut b = fill(rect("b", 300.0, 100.0, 100.0, 100.0));
    b.group_ids = smallvec![g.clone()];
    let other = fill(rect("o", 100.0, 300.0, 100.0, 100.0));
    let mut c = canvas_with(vec![a, b, other]);
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    for (kind, tag) in [(EventKind::PointerEnter, "enter"), (EventKind::PointerLeave, "leave")] {
        let l = Rc::clone(&log);
        c.on(
            kind,
            Selector::Any,
            Box::new(move |e| {
                let who = match &e.current {
                    Node::Element(i) => i.to_string(),
                    Node::Group(g) => format!("group {g}"),
                    Node::Stage => "stage".into(),
                };
                l.borrow_mut().push(format!("{tag} {who}"));
            }),
        );
    }
    mv(&mut c, 150.0, 150.0, 0.0);
    assert_eq!(*log.borrow(), ["enter a", "enter group g1"]);
    log.borrow_mut().clear();
    // Over empty space between the members: the group is left (this pointer left "a" and entered nothing).
    mv(&mut c, 250.0, 150.0, 10.0);
    assert_eq!(*log.borrow(), ["leave a", "leave group g1"]);
    log.borrow_mut().clear();
    mv(&mut c, 350.0, 150.0, 20.0);
    assert_eq!(*log.borrow(), ["enter b", "enter group g1"]);
    log.borrow_mut().clear();
    // Between group members directly (touching rectangles) the group would see neither; use an unrelated element.
    mv(&mut c, 150.0, 350.0, 30.0);
    assert_eq!(*log.borrow(), ["leave b", "leave group g1", "enter o"]);
}

#[test]
fn handles_win_over_elements_and_report_their_cursor() {
    let mut c =
        canvas_with(vec![fill(rect("a", 100.0, 100.0, 200.0, 100.0)), fill(rect("under", 280.0, 180.0, 100.0, 100.0))]);
    c.set_selection(vec![id("a")]);
    // The bottom-right handle at (300, 200) overlaps "under"; the handle wins.
    let out = mv(&mut c, 302.0, 202.0, 0.0);
    let Intent::Hover { to, cursor, .. } = &out[0] else { panic!("{out:?}") };
    assert_eq!((to, *cursor), (&Target::Handle(Handle::BottomRight), Cursor::ResizeNwse));
    // Dragging it: DragStart carries the handle; the overlay marks it active.
    press(&mut c, 302.0, 202.0, 10.0);
    let out = drag_to(&mut c, 340.0, 230.0, 20.0);
    let Intent::DragStart { target, origin, .. } = &out[0] else { panic!() };
    assert_eq!((target, origin.world), (&Target::Handle(Handle::BottomRight), Point::new(302.0, 202.0)));
    assert_eq!(c.overlay().transformer.unwrap().active, Some(Handle::BottomRight));
    release(&mut c, 340.0, 230.0, 30.0);
    assert_eq!(c.overlay().transformer.unwrap().active, None);
    // The rotation handle: 25 px above the top edge, whatever the zoom.
    c.zoom_at(Point::new(400.0, 300.0), 2.0);
    let top_mid = c.viewport().world_to_screen(Point::new(200.0, 100.0));
    let out = mv(&mut c, top_mid.x, top_mid.y - 25.0, 40.0);
    let Intent::Hover { to, cursor, .. } = out.last().unwrap() else { panic!() };
    assert_eq!((to, *cursor), (&Target::Handle(Handle::Rotate), Cursor::Rotate));
}

#[test]
fn handle_events_arrive_on_the_stage_as_transform_events() {
    let mut c = canvas_with(vec![fill(rect("a", 100.0, 100.0, 200.0, 100.0))]);
    c.set_selection(vec![id("a")]);
    type Seen = Rc<RefCell<Vec<(EventKind, Option<Handle>)>>>;
    let seen: Seen = Rc::default();
    for k in [EventKind::TransformStart, EventKind::Transform, EventKind::TransformEnd] {
        let s = Rc::clone(&seen);
        c.on(k, Selector::Stage, Box::new(move |e| s.borrow_mut().push((e.kind, e.handle.clone()))));
    }
    press(&mut c, 100.0, 100.0, 0.0);
    drag_to(&mut c, 80.0, 90.0, 10.0);
    release(&mut c, 80.0, 90.0, 20.0);
    let h = Some(Handle::TopLeft);
    assert_eq!(
        *seen.borrow(),
        [(EventKind::TransformStart, h.clone()), (EventKind::Transform, h.clone()), (EventKind::TransformEnd, h)]
    );
}

#[test]
fn marquee_overlay_tracks_a_drag_from_empty_space() {
    let mut c = canvas_with(vec![fill(rect("a", 100.0, 100.0, 50.0, 50.0))]);
    press(&mut c, 500.0, 400.0, 0.0);
    assert!(c.overlay().marquee.is_none());
    drag_to(&mut c, 450.0, 350.0, 10.0);
    let m = c.overlay().marquee.unwrap().rect;
    assert_eq!((m.x0, m.y0, m.x1, m.y1), (450.0, 350.0, 500.0, 400.0));
    drag_to(&mut c, 50.0, 50.0, 20.0);
    let m = c.overlay().marquee.unwrap().rect;
    assert_eq!((m.x0, m.y0, m.x1, m.y1), (50.0, 50.0, 500.0, 400.0));
    assert_eq!(c.marquee(&m, drafftink_canvas::MarqueeMode::Contain), vec![id("a")]);
    release(&mut c, 50.0, 50.0, 30.0);
    assert!(c.overlay().marquee.is_none());
}

#[test]
fn pinch_zoom_keeps_the_point_between_the_fingers_fixed() {
    let mut c = canvas_with(vec![]);
    let t = |i, phase, x, y, tm| InputEvent::Pointer(PointerEvent::touch(i, phase, x, y, tm));
    c.handle_input(&t(0, Down, 300.0, 300.0, 0.0));
    c.handle_input(&t(1, Down, 500.0, 300.0, 5.0));
    let anchor = c.viewport().screen_to_world(Point::new(400.0, 300.0));
    let out = c.handle_input(&t(1, Move, 700.0, 300.0, 20.0));
    // Fingers: 300 and 700. Centre moved 100 px right; distance doubled.
    assert!(out.iter().any(|i| matches!(i, Intent::Zoom { source: ViewSource::Touch, .. })));
    assert!((c.viewport().zoom() - 2.0).abs() < 1e-9);
    // The world point that was between the fingers at the start is now between them again (centre is 500).
    let now = c.viewport().world_to_screen(anchor);
    assert!((now - Point::new(500.0, 300.0)).hypot() < 1e-9, "{now:?}");
    let out = c.handle_input(&t(1, Up, 700.0, 300.0, 40.0));
    assert!(out.contains(&Intent::ViewEnd));
    assert!(c.take_dirty().camera);
}

#[test]
fn wheel_pans_the_view_and_ctrl_wheel_zooms_at_the_cursor() {
    let mut c = canvas_with(vec![]);
    let w = |dy, ctrl: bool| {
        InputEvent::Wheel(drafftink_canvas::WheelEvent {
            pos: Point::new(100.0, 100.0),
            delta: Vec2::new(0.0, dy),
            unit: drafftink_canvas::WheelUnit::Pixel,
            modifiers: drafftink_canvas::Modifiers { ctrl, ..Default::default() },
            time_ms: 0.0,
        })
    };
    c.take_dirty();
    c.handle_input(&w(50.0, false));
    assert_eq!(c.camera().scroll, Point::new(400.0, 350.0), "scrolling down moves the camera down");
    let under = c.viewport().screen_to_world(Point::new(100.0, 100.0));
    c.handle_input(&w(-100.0, true));
    assert!((c.camera().zoom - 1.1).abs() < 1e-12);
    assert!((c.viewport().screen_to_world(Point::new(100.0, 100.0)) - under).hypot() < 1e-9);
    assert!(c.take_dirty().camera);
}

#[test]
fn zoom_limits_hold_through_input() {
    let mut c = canvas_with(vec![]);
    for _ in 0..200 {
        c.zoom_at(Point::new(10.0, 10.0), 1.5);
    }
    assert_eq!(c.camera().zoom, 30.0);
    for _ in 0..400 {
        c.zoom_at(Point::new(10.0, 10.0), 0.5);
    }
    assert_eq!(c.camera().zoom, 0.1);
}

#[test]
fn hover_follows_a_scene_change_under_a_still_pointer() {
    let mut c = canvas_with(vec![fill(rect("a", 100.0, 100.0, 100.0, 100.0))]);
    mv(&mut c, 150.0, 150.0, 0.0);
    c.take_dirty();
    // The element moves away while the pointer rests: the hover changes without any input.
    c.transact(|tx| tx.update(&id("a"), |e| e.transform.x += 500.0)).unwrap();
    let intents = c.sync();
    let [Intent::Hover { from, to, .. }] = intents.as_slice() else { panic!("{intents:?}") };
    assert_eq!((from.element(), to), (Some(&id("a")), &Target::Empty));
    assert!(c.take_dirty().overlay);
}

#[test]
fn hovering_a_moved_element_redraws_the_overlay_but_not_when_unrelated_things_change() {
    let mut c =
        canvas_with(vec![fill(rect("a", 100.0, 100.0, 100.0, 100.0)), fill(rect("far", 600.0, 400.0, 10.0, 10.0))]);
    mv(&mut c, 150.0, 150.0, 0.0);
    c.take_dirty();
    c.transact(|tx| tx.update(&id("far"), |e| e.transform.x += 1.0)).unwrap();
    let d = c.take_dirty();
    assert!(d.scene && !d.overlay && !d.camera, "{d:?}");
    c.transact(|tx| tx.update(&id("a"), |e| e.transform.y += 1.0)).unwrap();
    assert!(c.take_dirty().overlay, "the hovered element's outline moved");
}

#[test]
fn selection_overlay_and_frame_follow_edits() {
    let mut c =
        canvas_with(vec![fill(rect("a", 100.0, 100.0, 100.0, 50.0)), fill(rect("b", 300.0, 200.0, 50.0, 50.0))]);
    c.take_dirty();
    c.set_selection(vec![id("a")]);
    assert!(c.take_dirty().overlay);
    let o = c.overlay();
    assert_eq!(o.selection.len(), 1);
    assert_eq!(o.transformer.as_ref().unwrap().handles.len(), 9);
    c.set_selection(vec![id("a"), id("b")]);
    let o = c.overlay();
    assert_eq!(o.selection.len(), 2);
    let drafftink_canvas::SelectionFrame::Box { obox, multi, .. } = c.selection_frame() else { panic!() };
    assert!(*multi);
    assert_eq!(obox.aabb(), drafftink_canvas::geom::kurbo::Rect::new(100.0, 100.0, 350.0, 250.0));
    // Moving a selected element recomputes the frame.
    c.transact(|tx| tx.update(&id("b"), |e| e.transform.x += 100.0)).unwrap();
    let drafftink_canvas::SelectionFrame::Box { obox, .. } = c.selection_frame() else { panic!() };
    assert_eq!(obox.aabb().x1, 450.0);
    assert!(c.take_dirty().overlay);
    // Deleting the selection leaves nothing to show.
    c.transact(|tx| {
        tx.update(&id("a"), |e| e.deleted = true)?;
        tx.update(&id("b"), |e| e.deleted = true)
    })
    .unwrap();
    assert!(c.overlay().transformer.is_none() && c.overlay().selection.is_empty());
}

#[test]
fn huge_selections_skip_per_element_boxes() {
    let els: Vec<_> = (0..300).map(|i| rect(&format!("r{i}"), i as f64 * 5.0, 0.0, 4.0, 100.0)).collect();
    let mut c = canvas_with(els);
    c.set_selection((0..300).map(|i| id(&format!("r{i}"))).collect());
    let o = c.overlay();
    assert!(o.selection.is_empty(), "over the cap of 256");
    assert_eq!(o.transformer.unwrap().handles.len(), 9, "the transformer still frames everything");
}

#[test]
fn overlay_describes_hover_guides_bind_candidates_and_remote_users() {
    let mut c =
        canvas_with(vec![fill(rect("a", 100.0, 100.0, 100.0, 100.0)), fill(rect("b", 300.0, 100.0, 100.0, 100.0))]);
    mv(&mut c, 150.0, 150.0, 0.0);
    assert_eq!(c.overlay().hover.as_ref().map(|h| h.id.clone()), Some(id("a")));
    c.set_selection(vec![id("a")]);
    assert!(c.overlay().hover.is_none(), "a selected element needs no hover outline");

    let guide = SmartGuide {
        kind: SmartGuideKind::Vertical,
        position: 10.0,
        start: 0.0,
        end: 100.0,
        snap_points: vec![],
        gap: None,
    };
    c.take_dirty();
    assert!(c.set_guides(vec![guide.clone()]));
    assert!(!c.set_guides(vec![guide.clone()]), "same guides: no change");
    assert!(c.take_dirty().overlay);
    assert!(!c.take_dirty().any());
    assert_eq!(c.overlay().guides, vec![guide]);

    assert!(c.set_bind_candidates(vec![id("b"), id("missing")]));
    let o = c.overlay();
    assert_eq!(o.bind_candidates.len(), 1, "an unknown id is skipped");
    assert_eq!(o.bind_candidates[0].target, id("b"));

    let user = RemoteUser {
        id: "u1".into(),
        name: Some("Ada".into()),
        color: Color::rgb(255, 0, 0),
        cursor: Some(Point::new(5.0, 6.0)),
        selection: vec![id("b"), id("missing")],
        trail: Vec::new(),
    };
    assert!(c.set_remote_users(vec![user.clone()]));
    assert!(!c.set_remote_users(vec![user]));
    let o = c.overlay();
    assert_eq!(o.remote_cursors.len(), 1);
    assert_eq!((o.remote_cursors[0].pos, o.remote_cursors[0].name.as_deref()), (Point::new(5.0, 6.0), Some("Ada")));
    assert_eq!(o.remote_selections[0].boxes.len(), 1);
    assert!(!o.remote_selections[0].truncated);
    assert!(!o.is_empty());
    assert_eq!(o.zoom, 1.0);
}

#[test]
fn visible_elements_cull_by_the_viewport() {
    let mut c = canvas_with(vec![rect("in", 350.0, 250.0, 50.0, 50.0), rect("out", 5000.0, 5000.0, 50.0, 50.0)]);
    let v: Vec<_> = c.visible_elements(0.0).into_iter().map(|e| e.id.to_string()).collect();
    assert_eq!(v, ["in"]);
    c.set_camera(Camera { scroll: Point::new(5000.0, 5000.0), zoom: 1.0 });
    let v: Vec<_> = c.visible_elements(0.0).into_iter().map(|e| e.id.to_string()).collect();
    assert_eq!(v, ["out"]);
    c.fit_to_content(20.0, None);
    assert_eq!(c.visible_elements(0.0).len(), 2);
    assert!(c.camera().zoom < 1.0);
}

#[test]
fn fit_to_elements_and_resize() {
    let mut c = canvas_with(vec![rect("a", 0.0, 0.0, 400.0, 300.0)]);
    assert!(c.fit_to_elements([&id("a")], 0.0, None));
    assert!((c.camera().zoom - 2.0).abs() < 0.1, "{:?}", c.camera());
    assert!(!c.fit_to_elements([&id("nope")], 0.0, None));
    assert!(c.set_viewport_size(Size::new(400.0, 300.0), 2.0));
    assert_eq!(c.viewport().dpr(), 2.0);
    assert!(!c.set_viewport_size(Size::new(400.0, 300.0), 2.0));
}

#[test]
fn entered_group_changes_the_unit_group_of_the_hover() {
    let (g1, g2) = (GroupId::new("g1").unwrap(), GroupId::new("g2").unwrap());
    let mut a = fill(rect("a", 100.0, 100.0, 100.0, 100.0));
    a.group_ids = smallvec![g1.clone(), g2.clone()];
    let mut b = fill(rect("b", 300.0, 100.0, 100.0, 100.0));
    b.group_ids = smallvec![g1.clone(), g2.clone()];
    let mut c = canvas_with(vec![a, b]);
    let out = mv(&mut c, 150.0, 150.0, 0.0);
    assert!(matches!(&out[0], Intent::Hover { to: Target::Element { unit_group: Some(g), .. }, .. } if g == &g2));
    c.set_entered_group(Some(g2));
    let out = c.sync();
    assert!(
        matches!(&out[0], Intent::Hover { to: Target::Element { unit_group: Some(g), .. }, .. } if g == &g1),
        "{out:?}"
    );
}

#[test]
fn a_group_of_one_is_not_a_unit() {
    let g = GroupId::new("solo").unwrap();
    let mut a = fill(rect("a", 100.0, 100.0, 100.0, 100.0));
    a.group_ids = smallvec![g];
    let c = canvas_with(vec![a]);
    let Target::Element { unit_group, .. } = c.pick(Point::new(150.0, 150.0), PointerKind::Mouse) else { panic!() };
    assert_eq!(unit_group, None, "fewer than two live members counts as no group");
}

#[test]
fn pointer_mode_switches_the_meaning_of_a_drag() {
    let mut c = canvas_with(vec![]);
    c.set_pointer_mode(PointerMode::Hand);
    press(&mut c, 100.0, 100.0, 0.0);
    drag_to(&mut c, 140.0, 100.0, 10.0);
    release(&mut c, 140.0, 100.0, 20.0);
    assert_eq!(c.camera().scroll, Point::new(360.0, 300.0), "content followed the pointer by 40 px");
    c.set_pointer_mode(PointerMode::Select);
    assert_eq!(c.pointer_mode(), PointerMode::Select);
}

#[test]
fn long_press_waits_for_the_deadline() {
    let mut c = canvas_with(vec![fill(rect("a", 100.0, 100.0, 100.0, 100.0))]);
    let touch = |phase, tm| InputEvent::Pointer(PointerEvent::touch(0, phase, 150.0, 150.0, tm));
    c.handle_input(&touch(Down, 0.0));
    assert_eq!(c.next_deadline(), Some(500.0));
    assert!(c.tick(400.0).is_empty());
    let out = c.tick(500.0);
    assert!(matches!(&out[..], [Intent::LongPress { target: Target::Element { .. }, .. }]));
    assert_eq!(c.next_deadline(), None);
}

#[test]
fn transact_is_atomic_and_the_store_comes_back() {
    let mut c = canvas_with(vec![rect("a", 0.0, 0.0, 10.0, 10.0)]);
    let r = c.transact(|tx| {
        tx.update(&id("a"), |e| e.transform.x = 99.0)?;
        tx.remove(&ElementId::new("missing").unwrap())
    });
    assert!(r.is_err());
    assert_eq!(c.store().get(&id("a")).unwrap().transform.x, 0.0, "rolled back");
    assert_eq!(c.index().bounds_of(&id("a")).unwrap().x0, -1.0);
    let store = c.into_store();
    assert_eq!(store.len(), 1);
}

#[test]
fn removing_the_pointer_clears_the_hover() {
    let mut c = canvas_with(vec![fill(rect("a", 100.0, 100.0, 100.0, 100.0))]);
    mv(&mut c, 150.0, 150.0, 0.0);
    let out = c.handle_input(&InputEvent::PointerLeave { id: PointerId(0) });
    assert!(matches!(&out[..], [Intent::Hover { to: Target::None, .. }]));
    assert_eq!(c.hover(), &Target::None);
}

#[test]
fn a_manual_dispatch_bubbles_like_a_real_one() {
    let g = GroupId::new("g1").unwrap();
    let mut a = fill(rect("a", 100.0, 100.0, 100.0, 100.0));
    a.group_ids = smallvec![g.clone()];
    let mut c = canvas_with(vec![a]);
    let hits: Rc<RefCell<u32>> = Rc::default();
    let h = Rc::clone(&hits);
    c.on(EventKind::Click, Selector::Group(g), Box::new(move |_| *h.borrow_mut() += 1));
    let pos = drafftink_canvas::Pos { screen: Point::ZERO, world: Point::ZERO };
    let ev = c.dispatch(ElementEvent::new(EventKind::Click, Some(id("a")), pos));
    assert_eq!(*hits.borrow(), 1);
    assert_eq!(ev.current, Node::Stage);
}

#[test]
fn viewport_default_is_usable() {
    let v = Viewport::default();
    assert_eq!(v.size(), Size::new(800.0, 600.0));
}

#[test]
fn a_listener_that_stops_propagation_is_seen_on_the_returned_event() {
    let mut c = canvas_with(vec![fill(rect("a", 100.0, 100.0, 100.0, 100.0))]);
    c.on(EventKind::Click, Selector::Id(id("a")), Box::new(ElementEvent::stop_propagation));
    let pos = drafftink_canvas::Pos { screen: Point::ZERO, world: Point::ZERO };
    let ev = c.dispatch(ElementEvent::new(EventKind::Click, Some(id("a")), pos));
    assert!(ev.is_propagation_stopped());
    let other = c.dispatch(ElementEvent::new(EventKind::Click, None, pos));
    assert!(!other.is_propagation_stopped());
}

#[test]
fn elements_in_rect_lists_what_touches_the_rectangle_back_to_front() {
    let c = canvas_with(vec![
        rect("a", 0.0, 0.0, 50.0, 50.0),
        rect("b", 40.0, 40.0, 50.0, 50.0),
        rect("far", 500.0, 500.0, 10.0, 10.0),
    ]);
    let hit: Vec<&str> = c
        .elements_in_rect(&drafftink_canvas::geom::kurbo::Rect::new(45.0, 45.0, 60.0, 60.0))
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(hit.len(), 2);
    assert!(hit.contains(&"a") && hit.contains(&"b") && !hit.contains(&"far"));
}

#[test]
fn the_viewport_centres_zooms_and_tells_what_is_visible() {
    let mut v = Viewport::default();
    let r = drafftink_canvas::geom::kurbo::Rect::new(1000.0, 1000.0, 1010.0, 1010.0);
    assert!(!v.is_visible(&r));
    assert!(v.center_on(Point::new(1005.0, 1005.0)));
    assert!(v.is_visible(&r));
    assert!(!v.center_on(Point::new(1005.0, 1005.0)), "centring on the same point changes nothing");
    let before = v.camera().zoom;
    assert!(v.set_zoom(before * 2.0));
    assert!((v.camera().zoom - before * 2.0).abs() < 1e-9);
    assert!(!v.set_zoom(f64::NAN));
}

fn linked(mut e: drafftink_canvas::scene::Element, link: &str) -> drafftink_canvas::scene::Element {
    e.link = Some(link.into());
    e
}

#[test]
fn the_link_icon_is_a_target_of_its_own_that_wins_over_what_lies_under_it() {
    // The icon of "a" sits at about (108, -8) for a box at 0..100; "under" lies there.
    let mut c = canvas_with(vec![
        linked(fill(rect("a", 0.0, 0.0, 100.0, 50.0)), "https://example.com"),
        fill(rect("under", 90.0, -30.0, 60.0, 60.0)),
    ]);
    let icon = Point::new(108.0, -8.0);
    assert_eq!(c.pick(icon, PointerKind::Mouse), Target::LinkIcon { id: id("a") });
    assert_eq!(c.pick(icon, PointerKind::Mouse).element(), None, "the icon is not the element");
    assert_eq!(c.pick(icon, PointerKind::Mouse).link_icon(), Some(&id("a")));
    assert_eq!(c.pick(icon, PointerKind::Mouse).without_link_icon(), &Target::Empty);
    // Over the icon the pointer is a pointing hand.
    let at = c.viewport().world_to_screen(icon);
    let out = mv(&mut c, at.x, at.y, 1.0);
    let Some(Intent::Hover { cursor, .. }) = out.last() else { panic!("{out:?}") };
    assert_eq!(*cursor, Cursor::Pointer);
    // The body of the linked element is the element.
    assert!(
        matches!(c.pick(Point::new(30.0, 25.0), PointerKind::Mouse), Target::Element { id: i, .. } if i == id("a"))
    );
    // A selected element shows its handles there instead.
    c.set_selection(vec![id("a")]);
    assert!(!matches!(c.pick(icon, PointerKind::Mouse), Target::LinkIcon { .. }));
    // An element without a link has none.
    c.set_selection(Vec::new());
    assert!(!matches!(c.pick(Point::new(158.0, -38.0), PointerKind::Mouse), Target::LinkIcon { .. }));
}

#[test]
fn a_click_on_a_link_icon_is_an_intent_with_that_target_and_the_icon_keeps_its_place_at_any_zoom() {
    let mut c = canvas_with(vec![linked(fill(rect("a", 0.0, 0.0, 100.0, 50.0)), "#other")]);
    let at = c.viewport().world_to_screen(Point::new(108.0, -8.0));
    press(&mut c, at.x, at.y, 0.0);
    let out = release(&mut c, at.x, at.y, 40.0);
    assert!(
        out.iter().any(|i| matches!(i, Intent::Click { target: Target::LinkIcon { id: i }, .. } if *i == id("a"))),
        "{out:?}"
    );
    for zoom in [0.25, 1.0, 4.0] {
        c.zoom_at(Point::new(400.0, 300.0), zoom / c.viewport().zoom());
        let world = drafftink_canvas::geom::link_icon::icon_rect(
            drafftink_canvas::geom::kurbo::Rect::new(0.0, 0.0, 100.0, 50.0),
            0.0,
            c.viewport().zoom(),
        )
        .center();
        let screen = c.viewport().world_to_screen(world);
        assert_eq!(c.pick(screen, PointerKind::Mouse), Target::LinkIcon { id: id("a") }, "zoom {zoom}");
    }
}

#[test]
fn the_overlay_lists_the_link_icons_in_view_that_are_not_selected() {
    let mut c = canvas_with(vec![
        linked(fill(rect("web", 0.0, 100.0, 100.0, 50.0)), "https://example.com"),
        linked(fill(rect("to", 200.0, 100.0, 100.0, 50.0)), "#web"),
        linked(fill(rect("far", 9000.0, 9000.0, 10.0, 10.0)), "https://example.com/far"),
        fill(rect("plain", 400.0, 100.0, 100.0, 50.0)),
    ]);
    let icons = c.overlay().link_icons;
    let flags: Vec<_> = icons.iter().map(|i| (i.element.clone(), i.to_element)).collect();
    assert_eq!(flags, vec![(id("web"), false), (id("to"), true)], "back to front, in view only");
    c.set_selection(vec![id("web")]);
    assert_eq!(c.overlay().link_icons.len(), 1, "a selected element shows its handles instead");
}

/// `e` with opacity 0, which draws nothing.
fn hidden(mut e: drafftink_canvas::scene::Element) -> drafftink_canvas::scene::Element {
    e.style.opacity = 0.0;
    e
}

#[test]
fn the_overlay_outlines_what_draws_nothing_in_view_and_not_selected() {
    let mut c = canvas_with(vec![
        hidden(fill(rect("faded", 0.0, 100.0, 100.0, 50.0))),
        {
            let mut e = rect("ghost", 200.0, 100.0, 100.0, 50.0);
            e.style.stroke_color = Color::TRANSPARENT;
            e
        },
        hidden(fill(rect("far", 9000.0, 9000.0, 10.0, 10.0))),
        fill(rect("plain", 400.0, 100.0, 100.0, 50.0)),
        rect("outlined", 600.0, 100.0, 100.0, 50.0),
    ]);
    let listed =
        |c: &Canvas<MemoryStore>| c.overlay().invisible.iter().map(|i| i.element.to_string()).collect::<Vec<_>>();
    assert_eq!(listed(&c), ["faded", "ghost"], "in view only, in id order; visible elements are not listed");
    assert!(!c.overlay().is_empty());
    c.set_selection(vec![id("faded")]);
    assert_eq!(listed(&c), ["ghost"], "a selected element shows its handles instead");
    c.set_selection(Vec::new());

    // The list follows edits: opaque again, then hidden again, then deleted.
    c.transact(|tx| tx.update(&id("faded"), |e| e.style.opacity = 1.0)).unwrap();
    assert_eq!(listed(&c), ["ghost"]);
    c.transact(|tx| tx.update(&id("plain"), |e| e.style.opacity = 0.0)).unwrap();
    assert_eq!(listed(&c), ["ghost", "plain"]);
    c.transact(|tx| tx.update(&id("ghost"), |e| e.deleted = true)).unwrap();
    assert_eq!(listed(&c), ["plain"]);
    assert_eq!(c.index().verify(c.store()), Ok(()), "the list is what a recomputation gives");
}

#[test]
fn a_document_without_invisible_elements_lists_none_and_a_pan_brings_them_into_view() {
    let mut c =
        canvas_with(vec![fill(rect("a", 0.0, 0.0, 50.0, 50.0)), hidden(fill(rect("far", 5000.0, 0.0, 50.0, 50.0)))]);
    assert!(c.overlay().invisible.is_empty());
    c.set_camera(Camera { scroll: Point::new(5025.0, 25.0), zoom: 1.0 });
    assert_eq!(c.overlay().invisible.len(), 1);
}

#[test]
fn remote_parts_make_cursors_boxes_and_trails_and_skip_what_is_gone() {
    use drafftink_canvas::{TrailPoint, remote_parts};
    let c = canvas_with(vec![fill(rect("a", 0.0, 0.0, 10.0, 10.0)), fill(rect("b", 20.0, 0.0, 10.0, 10.0))]);
    let user = RemoteUser {
        id: "u".into(),
        name: None,
        color: Color::rgb(0, 255, 0),
        cursor: None,
        selection: vec![id("a"), id("b"), id("gone")],
        trail: vec![TrailPoint { pos: Point::new(1.0, 2.0), age_ms: 30.0 }],
    };
    let parts = remote_parts(c.store(), &[user], 1);
    assert!(parts.cursors.is_empty(), "no pointer: no cursor");
    assert_eq!(parts.selections[0].boxes.len(), 1);
    assert!(parts.selections[0].truncated);
    assert_eq!(parts.trails[0].points.len(), 1);
}

#[test]
fn cursors_serialise_as_their_names_for_the_javascript_shell() {
    for (cursor, name) in [(Cursor::Text, "Text"), (Cursor::Pointer, "Pointer"), (Cursor::ResizeNwse, "ResizeNwse")] {
        assert_eq!(serde_json::to_string(&cursor).unwrap(), format!("\"{name}\""));
        assert_eq!(serde_json::from_str::<Cursor>(&format!("\"{name}\"")).unwrap(), cursor);
    }
}
