#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Property tests: grouping and z-order keep the structure sound, and transforms invert.

mod structure_support;

use std::collections::BTreeSet;

use drafftink_edit::arrange::{self, FlipAxis};
use drafftink_edit::group::{self, selection};
use drafftink_edit::lifecycle::{self, DuplicateOpts};
use drafftink_edit::transform::{self, RotateMode, geometry::world_bounds};
use drafftink_edit::zorder::{self, ZOp};
use drafftink_geom::kurbo::Vec2;
use drafftink_scene::*;
use proptest::prelude::*;
use structure_support::*;

/// A scene built from chunks: 0 = one plain element, 1 = a group of two, 2 = a group of three, 3 = a group of four
/// with an inner group of two. Returns the store and the element names in z-order.
fn build(chunks: &[u8]) -> (MemoryStore, Vec<String>) {
    let mut elements = Vec::new();
    let mut n = 0usize;
    for (next_group, c) in chunks.iter().enumerate() {
        let size = match c % 4 {
            0 => 1,
            1 => 2,
            2 => 3,
            _ => 4,
        };
        let outer = format!("G{next_group}");
        let inner = format!("I{next_group}");
        for k in 0..size {
            let name = format!("e{n}");
            let mut e = rect(&name, (n as f64) * 30.0, ((n * 7) % 50) as f64, 20.0 + (n % 3) as f64 * 5.0, 15.0);
            match (c % 4, k) {
                (0, _) => {}
                (3, 0 | 1) => e = e.grouped(&[&inner, &outer]),
                _ => e = e.grouped(&[&outer]),
            }
            elements.push(e);
            n += 1;
        }
    }
    let names = elements.iter().map(|e| e.id.to_string()).collect();
    (scene(elements), names)
}

fn pick(names: &[String], picks: &[prop::sample::Index]) -> Vec<ElementId> {
    let set: BTreeSet<&String> = picks.iter().map(|i| &names[i.index(names.len())]).collect();
    set.into_iter().map(|n| id(n)).collect()
}

fn stacks(s: &MemoryStore) -> Vec<(String, Vec<String>)> {
    s.iter().map(|e| (e.id.to_string(), e.group_ids.iter().map(|g| g.to_string()).collect())).collect()
}

fn keys_increase(s: &MemoryStore) -> bool {
    let k: Vec<&ZIndex> = s.iter_all().map(|e| &e.index).collect();
    k.windows(2).all(|w| w[0] < w[1])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn group_then_ungroup_restores_the_stacks(chunks in prop::collection::vec(0u8..4, 2..6), picks in prop::collection::vec(any::<prop::sample::Index>(), 2..5)) {
        let (mut s, names) = build(&chunks);
        let who = pick(&names, &picks);
        let before = stacks(&s);
        let order_before = order(&s);
        match run(&mut s, |tx| group::group(tx, &who, None, gid("NEW"))) {
            Ok(r) => {
                // Nothing else was reordered.
                let members: BTreeSet<String> = s.iter().filter(|e| e.group_ids.contains(&gid("NEW"))).map(|e| e.id.to_string()).collect();
                let others_before: Vec<&String> = order_before.iter().filter(|n| !members.contains(*n)).collect();
                let order_after = order(&s);
                let others_after: Vec<&String> = order_after.iter().filter(|n| !members.contains(*n)).collect();
                prop_assert_eq!(others_before, others_after);
                prop_assert!(group::non_contiguous_groups(&s).is_empty());
                prop_assert!(keys_increase(&s));
                prop_assert!(!validate_scene(&s).has_errors());
                let order_grouped = order(&s);
                run(&mut s, |tx| group::ungroup(tx, &[], Some(&r.group), None)).unwrap();
                prop_assert_eq!(stacks(&s), {
                    // Same stacks as before, in the new order.
                    let mut m: std::collections::HashMap<String, Vec<String>> = before.iter().cloned().collect();
                    order_grouped.iter().map(|n| (n.clone(), m.remove(n).unwrap())).collect::<Vec<_>>()
                });
                prop_assert_eq!(order(&s), order_grouped, "ungroup never reorders");
            }
            Err(_) => {
                prop_assert_eq!(stacks(&s), before);
                prop_assert_eq!(order(&s), order_before);
            }
        }
    }

    #[test]
    fn z_operations_keep_groups_contiguous_and_the_rest_in_order(
        chunks in prop::collection::vec(0u8..4, 2..6),
        picks in prop::collection::vec(any::<prop::sample::Index>(), 1..4),
        ops in prop::collection::vec(0u8..4, 1..6),
        enter in any::<bool>(),
    ) {
        let (mut s, names) = build(&chunks);
        let who = pick(&names, &picks);
        let editing = if enter { s.iter().find(|e| e.id == who[0]).and_then(|e| e.group_ids.last().cloned()) } else { None };
        for op in ops {
            let op = [ZOp::Front, ZOp::Back, ZOp::Forward, ZOp::Backward][op as usize];
            let before = order_all(&s);
            // Elements of the units that move.
            let r = run(&mut s, |tx| zorder::z_order(tx, &who, op, editing.as_ref()));
            let r = match r { Ok(r) => r, Err(e) => panic!("{e}") };
            let after = order_all(&s);
            let mut a = before.clone();
            let mut b = after.clone();
            a.sort();
            b.sort();
            prop_assert_eq!(a, b, "a permutation");
            prop_assert!(keys_increase(&s));
            prop_assert!(group::non_contiguous_groups(&s).is_empty(), "groups stay contiguous");
            // Elements that did not get a new key kept their relative order.
            let moved: BTreeSet<String> = r.moved.iter().map(ToString::to_string).collect();
            let rel = |v: &[String]| v.iter().filter(|n| !moved.contains(*n)).cloned().collect::<Vec<_>>();
            prop_assert_eq!(rel(&before), rel(&after));
            prop_assert!(!validate_scene(&s).has_errors());
        }
    }

    #[test]
    fn front_puts_the_units_last_and_back_first(chunks in prop::collection::vec(0u8..4, 2..6), picks in prop::collection::vec(any::<prop::sample::Index>(), 1..3)) {
        let (mut s, names) = build(&chunks);
        let who = pick(&names, &picks);
        // The unit members of the selection at root level.
        let rel = drafftink_edit::group::units::Relations::build(&s);
        let us = drafftink_edit::group::units::units(&s, &rel, &who, None).unwrap();
        let members: BTreeSet<String> = us.iter().flat_map(|u| u.elements.iter().map(ToString::to_string)).collect();
        run(&mut s, |tx| zorder::z_order(tx, &who, ZOp::Front, None)).unwrap();
        let o = order(&s);
        let tail: BTreeSet<String> = o[o.len() - members.len()..].iter().cloned().collect();
        prop_assert_eq!(&tail, &members);
        run(&mut s, |tx| zorder::z_order(tx, &who, ZOp::Back, None)).unwrap();
        let o = order(&s);
        let head: BTreeSet<String> = o[..members.len()].iter().cloned().collect();
        prop_assert_eq!(&head, &members);
    }

    #[test]
    fn select_ids_is_idempotent_and_groups_are_complete(chunks in prop::collection::vec(0u8..4, 2..6), picks in prop::collection::vec(any::<prop::sample::Index>(), 1..6), enter in any::<bool>()) {
        let (s, names) = build(&chunks);
        let who = pick(&names, &picks);
        let editing = if enter { s.iter().find(|e| e.id == who[0]).and_then(|e| e.group_ids.last().cloned()) } else { None };
        let a = selection::select_ids(&s, &who, editing.as_ref());
        let again: Vec<ElementId> = a.ids.iter().cloned().collect();
        let b = selection::select_ids(&s, &again, a.editing_group.as_ref());
        prop_assert_eq!(&a, &b);
        for g in &a.groups {
            let members: BTreeSet<ElementId> = s.iter().filter(|e| e.group_ids.contains(g)).map(|e| e.id.clone()).collect();
            prop_assert!(members.is_subset(&a.ids));
        }
    }

    #[test]
    fn move_and_back_is_the_identity(chunks in prop::collection::vec(0u8..4, 2..6), picks in prop::collection::vec(any::<prop::sample::Index>(), 1..4), dx in -500.0f64..500.0, dy in -500.0f64..500.0) {
        let (mut s, names) = build(&chunks);
        let who = pick(&names, &picks);
        let before: Vec<Element> = s.iter().cloned().collect();
        run(&mut s, |tx| transform::move_by(tx, &who, Vec2::new(dx, dy))).unwrap();
        run(&mut s, |tx| transform::move_by(tx, &who, Vec2::new(-dx, -dy))).unwrap();
        for b in &before {
            let a = get(&s, b.id.as_str());
            prop_assert!((a.transform.x - b.transform.x).abs() < 1e-9 && (a.transform.y - b.transform.y).abs() < 1e-9);
        }
    }

    #[test]
    fn rotation_is_rigid_for_the_whole_selection(chunks in prop::collection::vec(0u8..4, 2..6), picks in prop::collection::vec(any::<prop::sample::Index>(), 1..4), a in -6.0f64..6.0) {
        let (mut s, names) = build(&chunks);
        let who = pick(&names, &picks);
        let centres = |s: &MemoryStore| who.iter().map(|i| get(s, i.as_str()).transform.center()).collect::<Vec<_>>();
        let angles = |s: &MemoryStore| who.iter().map(|i| get(s, i.as_str()).transform.angle).collect::<Vec<_>>();
        let (c0, a0) = (centres(&s), angles(&s));
        run(&mut s, |tx| transform::rotate(tx, &who, a, RotateMode::By, None)).unwrap();
        let (c1, a1) = (centres(&s), angles(&s));
        for i in 0..who.len() {
            for j in 0..who.len() {
                prop_assert!(((c0[i] - c0[j]).hypot() - (c1[i] - c1[j]).hypot()).abs() < 1e-7, "distances are kept");
            }
            let turned = (a1[i] - a0[i] - a).rem_euclid(std::f64::consts::TAU);
            prop_assert!(turned < 1e-9 || std::f64::consts::TAU - turned < 1e-9, "every member's own angle changed by a");
        }
        prop_assert!(!validate_scene(&s).has_errors());
    }

    #[test]
    fn rotating_one_element_and_back_is_the_identity(chunks in prop::collection::vec(0u8..4, 2..6), pick1 in any::<prop::sample::Index>(), a in -6.0f64..6.0) {
        let (mut s, names) = build(&chunks);
        let who = vec![id(&names[pick1.index(names.len())])];
        let before = get(&s, who[0].as_str()).clone();
        run(&mut s, |tx| transform::rotate(tx, &who, a, RotateMode::By, None)).unwrap();
        run(&mut s, |tx| transform::rotate(tx, &who, -a, RotateMode::By, None)).unwrap();
        let e = get(&s, who[0].as_str());
        prop_assert!((e.transform.x - before.transform.x).abs() < 1e-9 && (e.transform.y - before.transform.y).abs() < 1e-9);
        let d = (e.transform.angle - before.transform.angle).rem_euclid(std::f64::consts::TAU);
        prop_assert!(d < 1e-9 || std::f64::consts::TAU - d < 1e-9);
    }

    #[test]
    fn flipping_twice_is_the_identity(chunks in prop::collection::vec(0u8..4, 2..6), picks in prop::collection::vec(any::<prop::sample::Index>(), 1..5), horizontal in any::<bool>(), angle in -3.0f64..3.0) {
        let (mut s, names) = build(&chunks);
        run(&mut s, |tx| tx.update(&id(&names[0]), |e| e.transform.angle = angle)).unwrap();
        let who = pick(&names, &picks);
        let axis = if horizontal { FlipAxis::Horizontal } else { FlipAxis::Vertical };
        let before: Vec<Element> = s.iter().cloned().collect();
        run(&mut s, |tx| arrange::flip(tx, &who, axis)).unwrap();
        prop_assert!(!validate_scene(&s).has_errors());
        run(&mut s, |tx| arrange::flip(tx, &who, axis)).unwrap();
        for b in &before {
            let e = get(&s, b.id.as_str());
            let (wa, wb) = (world_bounds(e), world_bounds(b));
            prop_assert!((wa.x0 - wb.x0).abs() < 1e-7 && (wa.x1 - wb.x1).abs() < 1e-7 && (wa.y0 - wb.y0).abs() < 1e-7 && (wa.y1 - wb.y1).abs() < 1e-7);
        }
    }

    #[test]
    fn duplicate_keeps_the_scene_valid_and_groups_contiguous(chunks in prop::collection::vec(0u8..4, 2..6), picks in prop::collection::vec(any::<prop::sample::Index>(), 1..5)) {
        let (mut s, names) = build(&chunks);
        let who = pick(&names, &picks);
        let n = s.len();
        let mut src = SeqIds::new();
        // Ids from SeqIds ("e1", ...) collide with the fixtures; the function must skip them.
        let r = run(&mut s, |tx| lifecycle::duplicate(tx, &who, &DuplicateOpts { offset: Vec2::new(5.0, 5.0), editing: None }, &mut src)).unwrap();
        prop_assert_eq!(s.len(), n + r.copies.len());
        let all: BTreeSet<ElementId> = s.iter().map(|e| e.id.clone()).collect();
        prop_assert_eq!(all.len(), s.len());
        prop_assert!(group::non_contiguous_groups(&s).is_empty());
        prop_assert!(!validate_scene(&s).has_errors());
        prop_assert!(keys_increase(&s));
    }
}

// ---- a mixed scene under random operations ----------------------------------------------------------------------

fn mixed() -> (MemoryStore, Vec<String>) {
    let mut a1 = arrow("arr1", &[(100.0, 25.0), (300.0, 25.0)]);
    bind(&mut a1, true, "r1", [1.0, 0.5]);
    bind(&mut a1, false, "r2", [0.0, 0.5]);
    let elements = vec![
        rect("k1", 5.0, 105.0, 20.0, 20.0).in_frame("f"),
        label("k1l", "k1", 6.0, 106.0, 8.0, 8.0).in_frame("f"),
        rect("k2", 40.0, 105.0, 20.0, 20.0).in_frame("f"),
        frame("f", 0.0, 100.0, 120.0, 100.0),
        rect("r1", 0.0, 0.0, 100.0, 50.0).grouped(&["G"]),
        label("r1l", "r1", 10.0, 10.0, 30.0, 10.0).grouped(&["G"]),
        rect("r2", 300.0, 0.0, 100.0, 50.0).grouped(&["G"]),
        a1,
        ellipse("o1", 500.0, 0.0, 40.0, 40.0).grouped(&["I", "H"]),
        ellipse("o2", 560.0, 0.0, 40.0, 40.0).grouped(&["I", "H"]),
        ellipse("o3", 620.0, 0.0, 40.0, 40.0).grouped(&["H"]),
        text("t", 700.0, 0.0, 60.0, 20.0, "hello"),
        image("im", 800.0, 0.0, 60.0, 30.0),
        freehand("fh", &[(900.0, 0.0), (930.0, 20.0), (960.0, 5.0)]),
        line("ln", &[(1000.0, 0.0), (1050.0, 40.0)]),
    ];
    let names = elements.iter().map(|e| e.id.to_string()).collect();
    (scene(elements), names)
}

#[derive(Debug, Clone)]
enum Op {
    Group,
    Ungroup,
    Z(u8),
    Move(f64, f64),
    Resize(u8, f64, f64),
    Rotate(f64),
    Flip(bool),
    Duplicate,
    Delete(bool),
    Restore,
    Lock,
    FrameAdd,
    FrameRemove,
    Align(u8),
    Distribute(bool),
    Wrap,
    Unwrap,
    Drop(f64, f64),
    SetBounds(f64, f64),
    Name,
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::Group),
        Just(Op::Ungroup),
        (0u8..4).prop_map(Op::Z),
        (-300.0f64..300.0, -300.0f64..300.0).prop_map(|(x, y)| Op::Move(x, y)),
        (0u8..8, -200.0f64..200.0, -200.0f64..200.0).prop_map(|(h, x, y)| Op::Resize(h, x, y)),
        (-4.0f64..4.0).prop_map(Op::Rotate),
        any::<bool>().prop_map(Op::Flip),
        Just(Op::Duplicate),
        any::<bool>().prop_map(Op::Delete),
        Just(Op::Restore),
        Just(Op::Lock),
        Just(Op::FrameAdd),
        Just(Op::FrameRemove),
        (0u8..6).prop_map(Op::Align),
        any::<bool>().prop_map(Op::Distribute),
        Just(Op::Wrap),
        Just(Op::Unwrap),
        (-100.0f64..300.0, -100.0f64..300.0).prop_map(|(x, y)| Op::Drop(x, y)),
        (1.0f64..300.0, 1.0f64..300.0).prop_map(|(w, h)| Op::SetBounds(w, h)),
        Just(Op::Name),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, failure_persistence: None, ..ProptestConfig::default() })]

    /// Any sequence of operations on any selection either succeeds or is refused, and the scene stays valid.
    #[test]
    fn random_operations_never_break_the_scene(
        ops in prop::collection::vec((op_strategy(), prop::collection::vec(any::<prop::sample::Index>(), 0..5)), 1..14),
    ) {
        use drafftink_edit::frame;
        let (mut s, names) = mixed();
        let mut src = SeqIds::new();
        for (counter, (op, picks)) in ops.into_iter().enumerate() {
            // Sometimes name an element that does not exist.
            let mut who: Vec<ElementId> = picks.iter().map(|i| i.index(names.len() + 1)).map(|i| id(names.get(i).map_or("ghost", String::as_str))).collect();
            who.sort();
            who.dedup();
            let before_valid = !validate_scene(&s).has_errors();
            prop_assert!(before_valid);
            match op {
                Op::Group => { let _ = run(&mut s, |tx| group::group(tx, &who, None, gid(&format!("N{counter}")))); }
                Op::Ungroup => { let _ = run(&mut s, |tx| group::ungroup(tx, &who, None, None)); }
                Op::Z(k) => { let op = [ZOp::Front, ZOp::Back, ZOp::Forward, ZOp::Backward][k as usize]; let _ = run(&mut s, |tx| zorder::z_order(tx, &who, op, None)); }
                Op::Move(x, y) => { let _ = run(&mut s, |tx| transform::move_by(tx, &who, Vec2::new(x, y))); }
                Op::Resize(h, x, y) => {
                    use drafftink_geom::select::{BoxHandle, Corner, Edge};
                    let handle = [BoxHandle::Corner(Corner::TopLeft), BoxHandle::Corner(Corner::TopRight), BoxHandle::Corner(Corner::BottomRight), BoxHandle::Corner(Corner::BottomLeft), BoxHandle::Edge(Edge::Top), BoxHandle::Edge(Edge::Right), BoxHandle::Edge(Edge::Bottom), BoxHandle::Edge(Edge::Left)][h as usize];
                    let _ = run(&mut s, |tx| transform::resize(tx, &who, transform::ResizeSpec { handle, delta: Vec2::new(x, y), keep_aspect: false, from_center: false }));
                }
                Op::Rotate(a) => { let _ = run(&mut s, |tx| transform::rotate(tx, &who, a, RotateMode::By, None)); }
                Op::Flip(h) => { let _ = run(&mut s, |tx| arrange::flip(tx, &who, if h { FlipAxis::Horizontal } else { FlipAxis::Vertical })); }
                Op::Duplicate => { let _ = run(&mut s, |tx| lifecycle::duplicate(tx, &who, &DuplicateOpts { offset: Vec2::new(10.0, 10.0), editing: None }, &mut src)); }
                Op::Delete(release) => { let _ = run(&mut s, |tx| lifecycle::delete(tx, &who, if release { lifecycle::ChildPolicy::Release } else { lifecycle::ChildPolicy::Delete })); }
                Op::Restore => { let _ = run(&mut s, |tx| lifecycle::restore(tx, &who)); }
                Op::Lock => { let _ = run(&mut s, |tx| lifecycle::lock(tx, &who, None)); }
                Op::FrameAdd => { let _ = run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &who)); }
                Op::FrameRemove => { let _ = run(&mut s, |tx| frame::remove_from_frame(tx, &who)); }
                Op::Align(m) => {
                    use drafftink_edit::arrange::AlignMode;
                    let mode = [AlignMode::Left, AlignMode::Right, AlignMode::Top, AlignMode::Bottom, AlignMode::CenterX, AlignMode::CenterY][m as usize];
                    let _ = run(&mut s, |tx| arrange::align(tx, &who, mode, None, None));
                }
                Op::Distribute(h) => {
                    use drafftink_edit::arrange::{Axis, DistributeOpts};
                    let _ = run(&mut s, |tx| arrange::distribute(tx, &who, if h { Axis::X } else { Axis::Y }, DistributeOpts::default(), None));
                }
                Op::Wrap => { let _ = run(&mut s, |tx| frame::wrap(tx, &who, &mut src, &frame::WrapOpts::default())); }
                Op::Unwrap => { let _ = run(&mut s, |tx| frame::unwrap(tx, &id("f"))); }
                Op::Drop(x, y) => {
                    let r = run(&mut s, |tx| transform::move_by(tx, &who, Vec2::new(x, y)));
                    if r.is_ok() {
                        let _ = run(&mut s, |tx| frame::update_membership_after_move(tx, &who, Some(drafftink_geom::kurbo::Point::new(x, y))));
                    }
                }
                Op::SetBounds(w, h) => {
                    if let Some(first) = who.first() {
                        let _ = run(&mut s, |tx| transform::set_bounds(tx, first, transform::BoundsSpec { w: Some(w), h: Some(h), ..transform::BoundsSpec::default() }));
                    }
                }
                Op::Name => { let _ = run(&mut s, |tx| group::set_group_name(tx, &gid("G"), Some("name"))); }
            }
            let report = validate_scene(&s);
            prop_assert!(!report.has_errors(), "after {:?}: {:?}", op, report.errors().collect::<Vec<_>>());
            prop_assert!(keys_increase(&s));
            prop_assert!(group::non_contiguous_groups(&s).is_empty(), "after {:?}", op);
            // Relations this crate owns never end in a warning: deleting a target frees the arrows bound to it.
            let bad: Vec<_> = report
                .warnings()
                .filter(|i| matches!(i.error, DocError::FrameDeleted(_) | DocError::ChildAboveFrame(_) | DocError::ContainerDeleted(_) | DocError::LabelBelowContainer(_) | DocError::BindingTargetDeleted { .. }))
                .collect();
            prop_assert!(bad.is_empty(), "after {:?}: {:?}", op, bad);
        }
    }
}
