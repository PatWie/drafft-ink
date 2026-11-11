#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Two and three replicas editing concurrently converge to the same scene, field by field.

mod common;

use common::*;
use drafftink_doc::LoroStore;
use drafftink_scene::*;
use kurbo::Point;
use proptest::prelude::*;

fn red() -> Color {
    Color::parse("#ff0000").unwrap()
}

/// Two replicas that both hold element `r` (and nothing else).
fn pair_with_rect() -> (LoroStore, LoroStore) {
    let mut a = peer(1);
    let mut b = peer(2);
    add(&mut a, vec![rect("r", &zs(1)[0])]);
    sync(&mut a, &mut b);
    (a, b)
}

#[test]
fn move_and_recolor_of_one_element_both_survive() {
    let (mut a, mut b) = pair_with_rect();
    edit(&mut a, "r", |e| {
        e.transform.x = 500.0;
        e.transform.y = 600.0;
    });
    edit(&mut b, "r", |e| e.style.stroke_color = red());
    let to_b = push(&a, &mut b);
    let to_a = push(&b, &mut a);
    for s in [&a, &b] {
        let e = s.get(&eid("r")).unwrap();
        assert_eq!((e.transform.x, e.transform.y), (500.0, 600.0));
        assert_eq!(e.style.stroke_color, red());
    }
    assert_same(&a, &b);
    // What the receiver is told: only the fields the other side touched.
    assert_eq!(to_b.get(&eid("r")).unwrap().fields, Fields::POSITION);
    assert_eq!(to_a.get(&eid("r")).unwrap().fields, Fields::STYLE);
    assert_view_matches_loro(&a);
    assert_view_matches_loro(&b);
}

#[test]
fn concurrent_edits_of_the_same_field_pick_one_winner_everywhere() {
    let (mut a, mut b) = pair_with_rect();
    edit(&mut a, "r", |e| e.transform.x = 111.0);
    edit(&mut b, "r", |e| e.transform.x = 222.0);
    sync(&mut a, &mut b);
    assert_same(&a, &b);
    let x = a.get(&eid("r")).unwrap().transform.x;
    assert!(x == 111.0 || x == 222.0);
}

#[test]
fn resize_and_rotate_and_rename_are_independent_fields() {
    let (mut a, mut b) = pair_with_rect();
    edit(&mut a, "r", |e| {
        e.transform.w = 300.0;
        e.transform.h = 40.0;
    });
    edit(&mut b, "r", |e| {
        e.transform.angle = 1.0;
        e.name = Some("box".into());
        e.locked = true;
    });
    sync(&mut a, &mut b);
    assert_same(&a, &b);
    let e = a.get(&eid("r")).unwrap();
    assert_eq!((e.transform.w, e.transform.h, e.transform.angle), (300.0, 40.0, 1.0));
    assert_eq!((e.name.as_deref(), e.locked), (Some("box"), true));
}

#[test]
fn delete_against_edit_keeps_both_when_deleting_is_a_tombstone() {
    let (mut a, mut b) = pair_with_rect();
    edit(&mut a, "r", |e| e.deleted = true);
    edit(&mut b, "r", |e| e.transform.x = 77.0);
    sync(&mut a, &mut b);
    assert_same(&a, &b);
    let e = a.get(&eid("r")).unwrap();
    assert!(e.deleted && e.transform.x == 77.0, "deleted and moved");
    assert_eq!(a.len(), 0);
    // Undeleting later brings back the edited element.
    edit(&mut b, "r", |e| e.deleted = false);
    sync(&mut a, &mut b);
    assert_eq!(a.get(&eid("r")).unwrap().transform.x, 77.0);
    assert_eq!(a.len(), 1);
}

#[test]
fn hard_remove_against_edit_removes_the_element_everywhere() {
    let (mut a, mut b) = pair_with_rect();
    let mut tx = a.begin();
    tx.remove(&eid("r")).unwrap();
    tx.commit().unwrap();
    edit(&mut b, "r", |e| e.style.opacity = 0.5);
    let cs = push(&a, &mut b);
    assert_eq!(cs.get(&eid("r")).unwrap().kind, ChangeKind::Removed);
    push(&b, &mut a);
    assert!(a.get(&eid("r")).is_none() && b.get(&eid("r")).is_none());
    assert_same(&a, &b);
    assert_view_matches_loro(&a);
}

#[test]
fn concurrent_inserts_at_the_same_z_position_get_one_order() {
    let (mut a, mut b) = pair_with_rect();
    // Both put a new element on top: the same key.
    let ka = a.next_top_index().unwrap();
    let kb = b.next_top_index().unwrap();
    assert_eq!(ka, kb);
    add(&mut a, vec![rect("from_a", &ka)]);
    add(&mut b, vec![rect("from_b", &kb)]);
    sync(&mut a, &mut b);
    assert_eq!(order(&a), order(&b));
    assert_eq!(order(&a), vec!["r", "from_a", "from_b"], "ties break by id");
    assert_view_matches_loro(&a);
}

#[test]
fn concurrent_inserts_between_the_same_neighbours_interleave_consistently() {
    let mut a = peer(1);
    let mut b = peer(2);
    let z = zs(2);
    add(&mut a, vec![rect("lo", &z[0]), rect("hi", &z[1])]);
    sync(&mut a, &mut b);
    let mid_a = ZIndex::between(Some(&z[0]), Some(&z[1])).unwrap();
    add(&mut a, vec![rect("a1", &mid_a)]);
    let mid_b = ZIndex::between(Some(&z[0]), Some(&z[1])).unwrap();
    add(&mut b, vec![rect("b1", &mid_b)]);
    sync(&mut a, &mut b);
    assert_eq!(order(&a), order(&b));
    assert_eq!(order(&a).first().map(String::as_str), Some("lo"));
    assert_eq!(order(&a).last().map(String::as_str), Some("hi"));
}

#[test]
fn reordering_while_another_peer_edits_style_merges() {
    let mut a = peer(1);
    let mut b = peer(2);
    let z = zs(3);
    add(&mut a, vec![rect("x", &z[0]), rect("y", &z[1]), rect("z", &z[2])]);
    sync(&mut a, &mut b);
    // A brings x to the front; B recolors x.
    let top = a.next_top_index().unwrap();
    edit(&mut a, "x", |e| e.index = top);
    edit(&mut b, "x", |e| e.style.stroke_color = red());
    sync(&mut a, &mut b);
    assert_eq!(order(&a), vec!["y", "z", "x"]);
    assert_eq!(b.get(&eid("x")).unwrap().style.stroke_color, red());
    assert_same(&a, &b);
}

#[test]
fn both_peers_creating_the_same_id_merge_into_one_element() {
    let mut a = peer(1);
    let mut b = peer(2);
    let z = zs(1);
    add(&mut a, vec![rect("same", &z[0])]);
    let mut e = rect("same", &z[0]);
    e.style.stroke_color = red();
    add(&mut b, vec![e]);
    sync(&mut a, &mut b);
    assert_eq!(a.len(), 1);
    assert_same(&a, &b);
    assert_view_matches_loro(&a);
}

#[test]
fn typing_in_the_same_text_from_two_peers_keeps_both() {
    let mut a = peer(1);
    let mut b = peer(2);
    add(&mut a, vec![text("t", &zs(1)[0], "hello world")]);
    sync(&mut a, &mut b);
    let set = |s: &mut LoroStore, new: &str| {
        edit(s, "t", |e| {
            if let ElementKind::Text(t) = &mut e.kind {
                t.content = new.to_owned();
            }
        });
    };
    set(&mut a, "Oh, hello world");
    set(&mut b, "hello world!");
    sync(&mut a, &mut b);
    assert_same(&a, &b);
    let content = |s: &LoroStore| s.get(&eid("t")).unwrap().text_data().unwrap().content.clone();
    assert_eq!(content(&a), "Oh, hello world!");
    assert_view_matches_loro(&a);
}

/// Inline math is plain text in the `LoroText` with `$` signs, and a setting beside it: typing a formula merges
/// character by character, and turning the setting off is its own key.
#[test]
fn typing_a_formula_and_switching_inline_math_off_merge() {
    let mut a = peer(1);
    let mut b = peer(2);
    add(&mut a, vec![text("t", &zs(1)[0], "Energy  here")]);
    sync(&mut a, &mut b);
    edit(&mut a, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.content = "Energy $E=mc^2$ here".to_owned();
        }
    });
    edit(&mut b, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.font.inline_math = InlineMath::Off;
        }
    });
    sync(&mut a, &mut b);
    assert_same(&a, &b);
    let t = a.get(&eid("t")).unwrap();
    let t = t.text_data().unwrap();
    assert_eq!((t.content.as_str(), t.font.inline_math), ("Energy $E=mc^2$ here", InlineMath::Off));
    assert_eq!(a.debug_json()["elements"]["t"]["kind.font.inline_math"], "off");
    // Back to the default deletes the key.
    edit(&mut a, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.font.inline_math = InlineMath::Auto;
        }
    });
    assert!(a.debug_json()["elements"]["t"].get("kind.font.inline_math").is_none());
    assert_view_matches_loro(&a);
}

#[test]
fn bold_and_italic_runs_follow_their_characters_and_merge_when_two_people_style_the_same_word() {
    let mut a = peer(1);
    let mut b = peer(2);
    let mut t = TextData::new("abc def");
    t.runs = vec![TextRun { start: 4, end: 7, bold: true, ..Default::default() }];
    add(&mut a, vec![Element::new(eid("t"), ElementKind::Text(t), zs(1)[0].clone()).with_bounds(0.0, 0.0, 10.0, 10.0)]);
    sync(&mut a, &mut b);
    // B types in front of the bold word: its bold moves with it.
    edit(&mut b, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.content = "XY abc def".into();
            t.runs = vec![TextRun { start: 7, end: 10, bold: true, ..Default::default() }];
        }
    });
    sync(&mut a, &mut b);
    let t = a.get(&eid("t")).unwrap().text_data().unwrap().clone();
    assert_eq!(t.runs, vec![TextRun { start: 7, end: 10, bold: true, ..Default::default() }]);
    // A makes the word italic while B colours it: each mark stays, nothing overwrites another.
    edit(&mut a, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = vec![TextRun { start: 7, end: 10, bold: true, italic: true, ..Default::default() }];
        }
    });
    edit(&mut b, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = vec![TextRun { start: 7, end: 10, color: Some(red()), bold: true, italic: false }];
        }
    });
    sync(&mut a, &mut b);
    assert_same(&a, &b);
    let merged = a.get(&eid("t")).unwrap().text_data().unwrap().runs.clone();
    assert_eq!(merged, vec![TextRun { start: 7, end: 10, color: Some(red()), bold: true, italic: true }]);
    assert_view_matches_loro(&a);
}

#[test]
fn colour_runs_follow_their_characters_when_somebody_types_in_front() {
    let mut a = peer(1);
    let mut b = peer(2);
    let mut t = TextData::new("abc def");
    t.runs = vec![TextRun { start: 4, end: 7, color: Some(red()), ..Default::default() }];
    add(&mut a, vec![Element::new(eid("t"), ElementKind::Text(t), zs(1)[0].clone()).with_bounds(0.0, 0.0, 10.0, 10.0)]);
    sync(&mut a, &mut b);
    edit(&mut b, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.content = "XY abc def".into();
            t.runs = vec![TextRun { start: 7, end: 10, color: Some(red()), ..Default::default() }];
        }
    });
    sync(&mut a, &mut b);
    let t = a.get(&eid("t")).unwrap().text_data().unwrap().clone();
    assert_eq!(t.content, "XY abc def");
    assert_eq!(t.runs, vec![TextRun { start: 7, end: 10, color: Some(red()), ..Default::default() }]);
    // Meanwhile A colours the first word; the merge has both runs.
    edit(&mut a, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs.insert(
                0,
                TextRun { start: 3, end: 6, color: Some(Color::parse("#0000ff").unwrap()), ..Default::default() },
            );
        }
    });
    sync(&mut a, &mut b);
    assert_eq!(b.get(&eid("t")).unwrap().text_data().unwrap().runs.len(), 2);
    assert_same(&a, &b);
}

#[test]
fn editing_a_point_while_another_peer_moves_the_line_keeps_the_box_consistent() {
    let mut a = peer(1);
    let mut b = peer(2);
    add(&mut a, vec![arrow("ar", &zs(1)[0])]);
    sync(&mut a, &mut b);
    // A moves the arrow. B drags the end point far away (new points, new size).
    edit(&mut a, "ar", |e| {
        e.transform.x += 100.0;
        e.transform.y += 100.0;
    });
    edit(&mut b, "ar", |e| {
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.points[1] = Point::new(300.0, 90.0);
        }
        e.refit_points().unwrap();
    });
    sync(&mut a, &mut b);
    assert_same(&a, &b);
    let e = a.get(&eid("ar")).unwrap();
    assert!(validate_element(e).is_ok());
    assert_eq!(a.invalid_elements().count(), 0);
}

#[test]
fn concurrent_changes_to_the_two_binding_ends_both_apply() {
    let mut a = peer(1);
    let mut b = peer(2);
    let z = zs(4);
    add(&mut a, vec![rect("t1", &z[0]), rect("t2", &z[1]), arrow("ar", &z[2])]);
    sync(&mut a, &mut b);
    let bind = |t: &str| Binding { target: eid(t), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] };
    edit(&mut a, "ar", |e| {
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.start_binding = Some(bind("t1"));
        }
    });
    edit(&mut b, "ar", |e| {
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.end_binding = Some(bind("t2"));
        }
    });
    sync(&mut a, &mut b);
    let d = a.get(&eid("ar")).unwrap().linear_data().unwrap().clone();
    assert!(d.start_binding.is_some() && d.end_binding.is_some());
    assert_eq!(a.derived().bound_arrows(&eid("t1")).count(), 1);
    assert_eq!(a.derived().bound_arrows(&eid("t2")).count(), 1);
    assert_same(&a, &b);
}

#[test]
fn name_and_background_are_separate_fields() {
    let mut a = peer(1);
    let mut b = peer(2);
    sync(&mut a, &mut b);
    let mut tx = a.begin();
    tx.set_name("Plan").unwrap();
    tx.commit().unwrap();
    let mut tx = b.begin();
    tx.set_background(Color::parse("#202020").unwrap());
    tx.commit().unwrap();
    sync(&mut a, &mut b);
    for s in [&a, &b] {
        assert_eq!(s.meta().name, "Plan");
        assert_eq!(s.meta().background, Color::parse("#202020").unwrap());
    }
}

#[test]
fn image_blobs_travel_once_by_hash() {
    let mut a = peer(1);
    let mut b = peer(2);
    let (fid, entry) = FileEntry::new("image/png", vec![7u8; 4096]);
    let mut tx = a.begin();
    tx.put_file(entry).unwrap();
    let cs = tx.commit().unwrap();
    assert_eq!(cs.files, vec![fid.clone()]);
    let cs = push(&a, &mut b);
    assert_eq!(cs.files, vec![fid.clone()]);
    assert_eq!(b.file(&fid).unwrap().bytes.len(), 4096);
}

#[test]
fn three_peers_converge_whatever_the_delivery_order() {
    let mut p = [peer(1), peer(2), peer(3)];
    let z = zs(6);
    add(&mut p[0], (0..6).map(|i| rect(&format!("e{i}"), &z[i])).collect());
    for i in [1usize, 2] {
        let (left, right) = p.split_at_mut(i);
        push(&left[0], &mut right[0]);
    }
    // Concurrent edits, all different, some overlapping.
    edit(&mut p[0], "e0", |e| e.transform.x = 1.0);
    edit(&mut p[0], "e1", |e| e.style.stroke_color = red());
    edit(&mut p[1], "e1", |e| e.transform.y = 2.0);
    edit(&mut p[1], "e2", |e| e.deleted = true);
    edit(&mut p[2], "e2", |e| e.transform.w = 3.0);
    edit(&mut p[2], "e3", |e| e.name = Some("three".into()));
    let top = p[2].next_top_index().unwrap();
    add(&mut p[2], vec![rect("new_c", &top)]);
    let top = p[0].next_top_index().unwrap();
    add(&mut p[0], vec![rect("new_a", &top)]);

    // Replica 0 hears 1 then 2; replica 1 hears 2 then 0; replica 2 hears 0 then 1.
    for (to, order) in [(0usize, [1usize, 2]), (1, [2, 0]), (2, [0, 1])] {
        for from in order {
            let bytes = p[from].export_updates_since(&p[to].version_vector().unwrap()).unwrap();
            p[to].apply_remote(&bytes).unwrap();
        }
    }
    // One more round so that everyone has everything.
    for to in 0..3 {
        for from in 0..3 {
            if from != to {
                let bytes = p[from].export_updates_since(&p[to].version_vector().unwrap()).unwrap();
                p[to].apply_remote(&bytes).unwrap();
            }
        }
    }
    assert_same(&p[0], &p[1]);
    assert_same(&p[1], &p[2]);
    let e1 = p[0].get(&eid("e1")).unwrap();
    assert_eq!((e1.style.stroke_color, e1.transform.y), (red(), 2.0));
    let e2 = p[0].get(&eid("e2")).unwrap();
    assert!(e2.deleted && e2.transform.w == 3.0);
    for s in &p {
        assert_view_matches_loro(s);
    }
}

#[test]
fn a_late_joiner_gets_the_state_from_a_snapshot_then_updates() {
    let mut a = peer(1);
    let z = zs(50);
    add(&mut a, (0..50).map(|i| rect(&format!("e{i}"), &z[i])).collect());
    let snap = a.export_snapshot().unwrap();
    edit(&mut a, "e3", |e| e.transform.x = 9.0);
    let mut late = LoroStore::from_snapshot(&snap).unwrap();
    push(&a, &mut late);
    assert_same(&a, &late);
    // The joiner is a full replica: its edits reach the original.
    edit(&mut late, "e4", |e| e.locked = true);
    push(&late, &mut a);
    assert!(a.get(&eid("e4")).unwrap().locked);
}

#[test]
fn pending_remote_operations_are_reported_and_resolved() {
    let mut a = peer(1);
    add(&mut a, vec![rect("one", &zs(1)[0])]);
    let v1 = a.version_vector().unwrap();
    edit(&mut a, "one", |e| e.transform.x = 1.0);
    let second = a.export_updates_since(&v1).unwrap();
    let mut b = peer(2);
    // The second update arrives before the first: it waits for its predecessors.
    let cs = b.apply_remote(&second).unwrap();
    assert!(cs.is_empty());
    assert!(b.has_pending_remote());
    let all = a.export_updates_since(&b.version_vector().unwrap()).unwrap();
    b.apply_remote(&all).unwrap();
    assert!(!b.has_pending_remote());
    assert_same(&a, &b);
}

// ---- randomised: three replicas, random edits, random partial syncs, then everything converges ------------------

#[derive(Debug, Clone)]
struct Op {
    peer: usize,
    kind: u8,
    target: u8,
    val: u8,
}

fn op() -> impl Strategy<Value = Op> {
    (0usize..3, 0u8..18, 0u8..8, any::<u8>()).prop_map(|(peer, kind, target, val)| Op { peer, kind, target, val })
}

fn run_op(s: &mut LoroStore, ops: &Op, counter: &mut u32) {
    let ids: Vec<ElementId> = s.iter_all().map(|e| e.id.clone()).collect();
    let target = ids.get(usize::from(ops.target) % ids.len().max(1)).cloned();
    let v = f64::from(ops.val);
    let mut tx = s.begin();
    match (ops.kind, target) {
        (0, _) | (_, None) => {
            *counter += 1;
            let id = format!("p{}_{}", ops.peer, counter);
            let top = tx.top_z();
            let z = ZIndex::between(top.as_ref(), None).unwrap();
            let _ = tx.insert(if ops.val.is_multiple_of(2) { rect(&id, &z) } else { text(&id, &z, "txt") });
        }
        (1, Some(id)) => {
            let _ = tx.update(&id, |e| e.transform.x = v);
        }
        (2, Some(id)) => {
            let _ = tx.update(&id, |e| e.style.stroke_width = 1.0 + v);
        }
        (3, Some(id)) => {
            let _ = tx.update(&id, |e| e.deleted = ops.val.is_multiple_of(2));
        }
        (4, Some(id)) => {
            let _ = tx.update(&id, |e| e.transform.w = v);
        }
        (5, Some(id)) => {
            let top = tx.top_z();
            let z = ZIndex::between(top.as_ref(), None).unwrap();
            let _ = tx.update(&id, |e| e.index = z);
        }
        (6, Some(id)) => {
            let _ = tx.update(&id, |e| {
                if let ElementKind::Text(t) = &mut e.kind {
                    let pos = usize::from(ops.val) % (t.content.chars().count() + 1);
                    let mut chars: Vec<char> = t.content.chars().collect();
                    chars.insert(pos, char::from(b'a' + ops.val % 26));
                    t.content = chars.into_iter().collect();
                    t.runs.clear();
                }
            });
        }
        (7, Some(id)) => {
            let _ = tx
                .update(&id, |e| e.name = if ops.val.is_multiple_of(3) { None } else { Some(format!("n{}", ops.val)) });
        }
        (8, Some(id)) => {
            let _ = tx.update(&id, |e| e.locked = !e.locked);
        }
        // Structure: groups, bindings, frames, arrows, point edits, undo and redo. The relations between elements
        // that these make can be invalid after a merge; `drafftink-engine` repairs them (`remote_repair`), and its
        // tests check `validate_scene`. Here the store itself must converge and keep every element valid.
        (10, Some(id)) => {
            let group = (!ops.val.is_multiple_of(7)).then(|| GroupId::new(&format!("g{}", ops.val % 3)).unwrap());
            let _ = tx.update(&id, |e| e.group_ids = group.into_iter().collect());
        }
        (11, _) => {
            *counter += 1;
            let id = format!("p{}_{}", ops.peer, counter);
            let top = tx.top_z();
            let z = ZIndex::between(top.as_ref(), None).unwrap();
            let _ = tx.insert(arrow(&id, &z));
        }
        (12, Some(id)) => {
            let other = ids.get(usize::from(ops.val) % ids.len()).cloned();
            let _ = tx.update(&id, |e| {
                if let (ElementKind::Arrow(d), Some(t)) = (&mut e.kind, other) {
                    let b = Binding { target: t, mode: BindMode::Orbit, fixed_point: [0.5, 0.5] };
                    if ops.val.is_multiple_of(2) {
                        d.start_binding = Some(b);
                    } else {
                        d.end_binding = Some(b);
                    }
                }
            });
        }
        (13, Some(id)) => {
            let frame =
                (!ops.val.is_multiple_of(4)).then(|| ids.get(usize::from(ops.val) % ids.len()).cloned()).flatten();
            let _ = tx.update(&id, |e| e.frame_id = frame);
        }
        (14, Some(id)) => {
            let _ = tx.update(&id, |e| {
                if let ElementKind::Arrow(_) = e.kind {
                    let to = Point::new(20.0 + v, 10.0 + v / 2.0);
                    if let Some(fresh) =
                        Element::linear(e.id.clone(), LinearKind::Arrow, e.index.clone(), &[Point::new(0.0, 0.0), to])
                    {
                        e.kind = fresh.kind;
                        e.transform.w = fresh.transform.w;
                        e.transform.h = fresh.transform.h;
                    }
                }
            });
        }
        (15, _) => {
            drop(tx);
            let _ = s.undo();
            return;
        }
        (16, _) => {
            drop(tx);
            let _ = s.redo();
            return;
        }
        (_, Some(id)) => {
            if ops.val.is_multiple_of(5) {
                let _ = tx.remove(&id);
            } else {
                let _ = tx.update(&id, |e| e.transform.angle = v / 100.0);
            }
        }
    }
    let _ = tx.commit();
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn random_concurrent_edits_converge(ops in proptest::collection::vec(op(), 1..60), syncs in proptest::collection::vec((0usize..3, 0usize..3), 0..20)) {
        let mut p = [peer(11), peer(22), peer(33)];
        let mut counter = 0u32;
        let mut sync_iter = syncs.into_iter();
        for (i, o) in ops.iter().enumerate() {
            run_op(&mut p[o.peer], o, &mut counter);
            if i % 3 == 2 && let Some((from, to)) = sync_iter.next() && from != to {
                let bytes = p[from].export_updates_since(&p[to].version_vector().unwrap()).unwrap();
                p[to].apply_remote(&bytes).unwrap();
            }
        }
        for _ in 0..2 {
            for to in 0..3 {
                for from in 0..3 {
                    if from != to {
                        let bytes = p[from].export_updates_since(&p[to].version_vector().unwrap()).unwrap();
                        p[to].apply_remote(&bytes).unwrap();
                    }
                }
            }
        }
        assert_same(&p[0], &p[1]);
        assert_same(&p[1], &p[2]);
        for s in &p {
            assert_view_matches_loro(s);
            // Each element is valid on its own, whatever the merge did to its fields.
            let broken: Vec<String> = validate_scene(s)
                .issues
                .iter()
                .filter(|i| matches!(i.error, DocError::Element(_)))
                .map(ToString::to_string)
                .collect();
            assert!(broken.is_empty(), "{}", broken.join("\n"));
        }
    }
}
