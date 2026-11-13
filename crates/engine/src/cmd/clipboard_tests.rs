//! Tests of the `clipboard` family: availability, refusal text, effect, undo and negative cases.

use std::sync::{Arc, Mutex};

use drafftink_scene::{
    BindMode, Binding, Element, ElementId, ElementKind, GroupId, LinearData, SceneRead, SceneTx, StoreError, TextData,
};
use serde_json::{Value, json};

use super::{COMPAT_KEY, CopyBuffer};
use crate::ports::fake::Fakes;
use crate::ports::{ClipKind, ClipPayload, Clipboard, PortError, Ports, RenderRequest, Renderer, SceneView};
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::ring::Origin;
use crate::services::Services;
use crate::session::Session;
use crate::spec::{CommandSpec, Facts, Typed};
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

fn refuse_clean(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = refuse(s, id, args);
    assert_eq!(s.fingerprint(), before, "{id} refused but changed state: {}", r.message);
    r
}

/// A session with every fake port and the font-free text measure.
fn fx() -> (Session, Fakes) {
    let fakes = Fakes::new();
    (Session::with_services(fakes.ports(), Services::fake()), fakes)
}

fn live(s: &Session) -> Vec<Element> {
    s.store().iter().cloned().collect()
}

fn id_list(out: &Value, key: &str) -> Vec<ElementId> {
    out[key].as_array().unwrap().iter().map(|v| ElementId::new(v.as_str().unwrap()).unwrap()).collect()
}

fn centre(e: &Element) -> (f64, f64) {
    (e.transform.x + e.transform.w / 2.0, e.transform.y + e.transform.h / 2.0)
}

fn edit(s: &mut Session, f: impl FnOnce(&mut crate::store::DocTx<'_, '_>) -> Result<(), StoreError>) {
    s.tab_mut().doc.edit::<(), StoreError>(f).unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
}

/// Two rectangles: a at (0,0) 100x60 and b at (200,50) 80x80. The middle of both is (140, 65).
fn two_rects(s: &mut Session) -> (ElementId, ElementId) {
    let a = testing::add_rect(s, 0.0, 0.0, 100.0, 60.0).unwrap();
    let b = testing::add_rect(s, 200.0, 50.0, 80.0, 80.0).unwrap();
    testing::select(s, &[a.clone(), b.clone()]);
    (a, b)
}

fn group(s: &mut Session, ids: &[&ElementId], name: &str) {
    let g = GroupId::new(name).unwrap();
    edit(s, |tx| {
        for id in ids {
            tx.update(id, |e| e.group_ids.push(g.clone()))?;
        }
        Ok(())
    });
}

fn bind(s: &mut Session, arrow: &ElementId, start: &ElementId, end: &ElementId) {
    let b = |t: &ElementId| Some(Binding { target: t.clone(), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] });
    edit(s, |tx| {
        tx.update(arrow, |e| {
            if let ElementKind::Arrow(d) = &mut e.kind {
                d.start_binding = b(start);
                d.end_binding = b(end);
            }
        })
    });
}

fn arrow_bindings(e: &Element) -> (Option<ElementId>, Option<ElementId>) {
    let ElementKind::Arrow(d) = &e.kind else { panic!("not an arrow") };
    (d.start_binding.as_ref().map(|b| b.target.clone()), d.end_binding.as_ref().map(|b| b.target.clone()))
}

fn add_arrow(s: &mut Session) -> ElementId {
    use drafftink_geom::kurbo::Point;
    testing::add(
        s,
        ElementKind::Arrow(LinearData::new(vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)])),
        110.0,
        30.0,
        100.0,
        0.0,
    )
    .unwrap()
}

// ---- availability ----

#[test]
fn copy_cut_and_the_style_commands_need_a_selection_and_say_so() {
    let (mut s, _) = fx();
    for id in ["clipboard.copy", "clipboard.cut", "clipboard.copy_style", "clipboard.paste_style"] {
        let r = s.availability(id).unwrap_err();
        assert_eq!(r.code, "no_target", "{id}");
        assert_eq!(r.message, "Nothing to act on: no elements are selected. Select elements first, or pass `ids`.");
        let e = refuse_clean(&mut s, id, json!({}));
        assert_eq!((e.code, e.message.as_str()), (r.code, r.message.as_str()), "{id}");
    }
    assert!(s.availability("clipboard.paste").is_ok());
    assert!(s.availability("clipboard.copy_as").is_ok());
}

#[test]
fn paste_style_is_unavailable_until_a_style_was_copied() {
    let (mut s, _) = fx();
    two_rects(&mut s);
    let r = s.availability("clipboard.paste_style").unwrap_err();
    assert_eq!(r.code, "nothing_to_do");
    assert!(r.message.contains("clipboard.copy_style"), "{}", r.message);
    let e = refuse_clean(&mut s, "clipboard.paste_style", json!({}));
    assert_eq!((e.code, e.message.as_str()), (r.code, r.message.as_str()));
    run(&mut s, "clipboard.copy_style", json!({}));
    assert!(s.availability("clipboard.paste_style").is_ok());
}

#[test]
fn view_only_blocks_what_changes_the_document_and_allows_copying() {
    let (mut s, _) = fx();
    two_rects(&mut s);
    s.view_only = true;
    for id in ["clipboard.copy", "clipboard.copy_style", "clipboard.copy_as"] {
        assert!(s.availability(id).is_ok(), "{id}");
    }
    for id in ["clipboard.cut", "clipboard.paste", "clipboard.paste_style"] {
        assert_eq!(s.availability(id).unwrap_err().code, "view_only", "{id}");
    }
    run(&mut s, "clipboard.copy", json!({}));
}

// ---- copy ----

#[test]
fn copy_keeps_the_buffer_and_writes_the_payload_with_the_excalidraw_flavour() {
    let (mut s, fakes) = fx();
    let (a, _b) = two_rects(&mut s);
    let rev = s.tab().doc.revision();
    let out = run(&mut s, "clipboard.copy", json!({}));
    assert_eq!((out["count"].as_u64(), out["system"].as_bool()), (Some(2), Some(true)));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.revision(), rev, "a copy changes no document");
    assert_eq!(s.tab().doc.undo_depth(), 0);
    assert_eq!(s.selection().len(), 2);
    let Some(ClipPayload::Elements(v)) = fakes.clipboard.content() else { panic!("no payload") };
    assert_eq!(v["format"], "drafftink-clipboard");
    assert_eq!(v["document"]["elements"].as_array().unwrap().len(), 2);
    assert!(v["text"].as_str().unwrap().contains("excalidraw/clipboard"));
    assert!(s.ext.get::<CopyBuffer>().is_some_and(|b| b.envelope.is_some()));
    let _ = a;
}

#[test]
fn the_excalidraw_flavour_follows_the_setting() {
    let (mut s, fakes) = fx();
    two_rects(&mut s);
    s.settings.set_raw(COMPAT_KEY, json!(false));
    run(&mut s, "clipboard.copy", json!({}));
    let Some(ClipPayload::Elements(v)) = fakes.clipboard.content() else { panic!() };
    assert!(v.get("text").is_none());
}

#[test]
fn copy_takes_bound_text_and_frame_children_along() {
    let (mut s, fakes) = fx();
    let frame = testing::add(&mut s, ElementKind::Frame(drafftink_scene::FrameData { name: None, clip: true }), -10.0, -10.0, 400.0, 400.0).unwrap();
    let boxed = testing::add_rect(&mut s, 0.0, 0.0, 100.0, 60.0).unwrap();
    let mut data = TextData::new("hi");
    data.container_id = Some(boxed.clone());
    let label = testing::add(&mut s, ElementKind::Text(data), 10.0, 10.0, 20.0, 20.0).unwrap();
    edit(&mut s, |tx| {
        tx.update(&boxed, |e| e.frame_id = Some(frame.clone()))?;
        tx.update(&label, |e| e.frame_id = Some(frame.clone()))
    });
    testing::select(&mut s, std::slice::from_ref(&frame));
    let out = run(&mut s, "clipboard.copy", json!({}));
    assert_eq!(out["count"], 3, "the frame, its child and the child's label");
    // Only the box: its label comes with it, the frame stays behind.
    testing::select(&mut s, std::slice::from_ref(&boxed));
    let out = run(&mut s, "clipboard.copy", json!({}));
    assert_eq!(out["count"], 2);
    let Some(ClipPayload::Elements(v)) = fakes.clipboard.content() else { panic!() };
    let els = v["document"]["elements"].as_array().unwrap();
    assert!(els.iter().all(|e| e.get("frame_id").is_none()), "the frame was not copied, so membership is cut");
}

#[test]
fn copy_without_a_clipboard_port_still_copies_inside_the_session() {
    let mut s = testing::headless_session();
    two_rects(&mut s);
    let out = run(&mut s, "clipboard.copy", json!({}));
    assert_eq!(out["system"], false);
    let w = out["warnings"].as_array().unwrap();
    assert!(w[0].as_str().unwrap().contains("system clipboard is not available"), "{w:?}");
    let pasted = run(&mut s, "clipboard.paste", json!({"source": "internal", "at": [0, 0]}));
    assert_eq!(pasted["count"], 2);
}

#[test]
fn copy_with_system_false_leaves_the_clipboard_alone() {
    let (mut s, fakes) = fx();
    two_rects(&mut s);
    run(&mut s, "clipboard.copy", json!({"system": false}));
    assert!(fakes.clipboard.content().is_none());
    assert!(s.ext.get::<CopyBuffer>().is_some());
}

#[test]
fn copy_accepts_ids_and_refuses_unknown_ones() {
    let (mut s, _) = fx();
    let (a, _b) = two_rects(&mut s);
    s.tab_mut().observed.selection.clear();
    let out = run(&mut s, "clipboard.copy", json!({"ids": [a.as_str()]}));
    assert_eq!(out["count"], 1);
    let r = refuse_clean(&mut s, "clipboard.copy", json!({"ids": ["nope"]}));
    assert_eq!(r.code, "not_found");
}

// ---- cut ----

#[test]
fn cut_copies_and_deletes_in_one_undo_step() {
    let (mut s, fakes) = fx();
    let (a, b) = two_rects(&mut s);
    let out = run(&mut s, "clipboard.cut", json!({}));
    assert_eq!(out["count"], 2);
    assert!(live(&s).is_empty());
    assert!(s.selection().is_empty());
    assert!(matches!(fakes.clipboard.content(), Some(ClipPayload::Elements(_))));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert_eq!(out["undo_label"], "Cut");
    s.undo_steps(1).unwrap();
    assert_eq!(live(&s).len(), 2);
    assert!(s.element(&a).is_some() && s.element(&b).is_some());
    // The copy survives the undo: paste brings the elements back as new ones.
    run(&mut s, "clipboard.paste", json!({"at": [0, 0]}));
    assert_eq!(live(&s).len(), 4);
}

#[test]
fn cut_of_a_locked_element_refuses_and_leaves_clipboard_and_buffer_alone() {
    let (mut s, fakes) = fx();
    let (a, _b) = two_rects(&mut s);
    edit(&mut s, |tx| tx.update(&a, |e| e.locked = true));
    let r = refuse_clean(&mut s, "clipboard.cut", json!({}));
    assert_eq!(r.code, "locked");
    assert!(r.message.contains(a.as_str()), "{}", r.message);
    assert!(fakes.clipboard.content().is_none());
    assert!(s.ext.get::<CopyBuffer>().is_none());
    // Copying a locked element is fine.
    run(&mut s, "clipboard.copy", json!({}));
}

#[test]
fn cut_removes_only_what_was_named_from_the_selection() {
    let (mut s, _) = fx();
    let (a, b) = two_rects(&mut s);
    run(&mut s, "clipboard.cut", json!({"ids": [a.as_str()]}));
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), vec![&b]);
}

// ---- paste ----

#[test]
fn paste_centres_the_copy_on_at_with_new_ids_and_selects_it() {
    let (mut s, _) = fx();
    let (a, b) = two_rects(&mut s);
    run(&mut s, "clipboard.copy", json!({}));
    let out = run(&mut s, "clipboard.paste", json!({"at": [500, 500]}));
    assert_eq!(out["kind"], "elements");
    let new = id_list(&out, "ids");
    assert_eq!(new.len(), 2);
    assert!(!new.contains(&a) && !new.contains(&b));
    assert_eq!(live(&s).len(), 4);
    let (pa, pb) = (s.element(&new[0]).unwrap(), s.element(&new[1]).unwrap());
    assert_eq!((pa.transform.x, pa.transform.y), (360.0, 435.0));
    assert_eq!((pb.transform.x, pb.transform.y), (560.0, 485.0));
    assert_eq!(s.selection().len(), 2);
    assert!(new.iter().all(|i| s.selection().contains(i)));
    assert_eq!(out["undo_label"], "Paste");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert!(pa.index > s.element(&b).unwrap().index, "the paste goes on top");
    s.undo_steps(1).unwrap();
    assert_eq!(live(&s).len(), 2);
    assert_eq!(s.selection().len(), 2, "undo restores the old selection");
}

#[test]
fn the_default_place_is_the_viewport_centre_and_cursor_is_only_a_word() {
    let (mut s, _) = fx();
    two_rects(&mut s);
    run(&mut s, "clipboard.copy", json!({}));
    run(&mut s, "view.set", json!({"scroll": [1000, 1000]}));
    for args in [json!({}), json!({"at": "viewport_center"}), json!({"at": "$cursor"})] {
        let out = run(&mut s, "clipboard.paste", args);
        let new = id_list(&out, "ids");
        let xs: Vec<(f64, f64)> = new.iter().map(|i| centre(s.element(i).unwrap())).collect();
        let mid = ((xs[0].0 + xs[1].0) / 2.0, (xs[0].1 + xs[1].1) / 2.0);
        // The rectangles are 100x60 and 80x80; the middle of their bounds is at the scroll point.
        let (a, b) = (s.element(&new[0]).unwrap(), s.element(&new[1]).unwrap());
        let bx = (a.transform.x.min(b.transform.x) + (a.transform.x + a.transform.w).max(b.transform.x + b.transform.w)) / 2.0;
        assert!((bx - 1000.0).abs() < 1e-9, "{bx} {mid:?}");
    }
}

#[test]
fn the_origin_of_the_call_does_not_change_where_a_paste_lands() {
    // The old app pasted at the cursor from the keyboard and at the viewport centre from the buttons.
    let place = |origin: Origin| {
        let (mut s, _) = fx();
        two_rects(&mut s);
        s.execute_from(origin, "clipboard.copy", json!({})).unwrap();
        let out = s.execute_from(origin, "clipboard.paste", json!({"at": [10, 20]})).unwrap();
        centre(s.element(&id_list(&out, "ids")[0]).unwrap())
    };
    assert_eq!(place(Origin::Shortcut), place(Origin::Ui));
    assert_eq!(place(Origin::Ui), place(Origin::Palette));
}

#[test]
fn pasted_groups_are_new_and_never_join_an_existing_group() {
    let (mut s, _) = fx();
    let (a, b) = two_rects(&mut s);
    group(&mut s, &[&a, &b], "g_old");
    run(&mut s, "clipboard.copy", json!({}));
    let first = id_list(&run(&mut s, "clipboard.paste", json!({"at": [0, 0]})), "ids");
    let second = id_list(&run(&mut s, "clipboard.paste", json!({"at": [0, 0]})), "ids");
    let g = |i: &ElementId| s.element(i).unwrap().group_ids.to_vec();
    assert_eq!(g(&first[0]).len(), 1);
    assert_eq!(g(&first[0]), g(&first[1]), "the copies of one group share the new group");
    assert_ne!(g(&first[0]), g(&second[0]), "each paste makes its own group");
    assert!(!g(&first[0]).contains(&GroupId::new("g_old").unwrap()));
    assert_eq!(g(&a), vec![GroupId::new("g_old").unwrap()], "the original group is untouched");
    assert_eq!(s.tab().observed.selected_groups.len(), 1, "the pasted group is selected as a whole");
}

#[test]
fn a_group_with_one_member_among_the_copied_elements_is_dropped() {
    let (mut s, _) = fx();
    let (a, b) = two_rects(&mut s);
    group(&mut s, &[&a, &b], "g_old");
    run(&mut s, "clipboard.copy", json!({"ids": [a.as_str()]}));
    let new = id_list(&run(&mut s, "clipboard.paste", json!({"at": [0, 0]})), "ids");
    assert!(s.element(&new[0]).unwrap().group_ids.is_empty());
}

#[test]
fn bindings_point_at_the_copies_and_a_binding_to_something_left_behind_is_cut() {
    let (mut s, _) = fx();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 100.0, 60.0).unwrap();
    let b = testing::add_rect(&mut s, 300.0, 0.0, 100.0, 60.0).unwrap();
    let arrow = add_arrow(&mut s);
    bind(&mut s, &arrow, &a, &b);

    testing::select(&mut s, &[a.clone(), b.clone(), arrow.clone()]);
    run(&mut s, "clipboard.copy", json!({}));
    let new = id_list(&run(&mut s, "clipboard.paste", json!({"at": [0, 500]})), "ids");
    let arrow_copy = new.iter().filter_map(|i| s.element(i)).find(|e| matches!(e.kind, ElementKind::Arrow(_))).unwrap();
    let (start, end) = arrow_bindings(arrow_copy);
    let rects: Vec<&ElementId> = new.iter().filter(|i| **i != arrow_copy.id).collect();
    assert!(start.as_ref().is_some_and(|t| rects.contains(&t)) && end.as_ref().is_some_and(|t| rects.contains(&t)));
    assert_ne!(start, end);
    assert_eq!(arrow_bindings(s.element(&arrow).unwrap()), (Some(a.clone()), Some(b.clone())), "the original is untouched");

    testing::select(&mut s, &[a.clone(), arrow.clone()]);
    run(&mut s, "clipboard.copy", json!({}));
    let new = id_list(&run(&mut s, "clipboard.paste", json!({"at": [0, 900]})), "ids");
    let arrow_copy = new.iter().filter_map(|i| s.element(i)).find(|e| matches!(e.kind, ElementKind::Arrow(_))).unwrap();
    let (start, end) = arrow_bindings(arrow_copy);
    assert!(start.is_some() && start != Some(a.clone()), "bound to the copy of a");
    assert_eq!(end, None, "b was not copied, so that end is free");
}

#[test]
fn bound_text_follows_its_container_and_frame_membership_needs_the_frame() {
    let (mut s, _) = fx();
    let frame = testing::add(&mut s, ElementKind::Frame(drafftink_scene::FrameData { name: None, clip: true }), -10.0, -10.0, 400.0, 400.0).unwrap();
    let boxed = testing::add_rect(&mut s, 0.0, 0.0, 100.0, 60.0).unwrap();
    let mut data = TextData::new("hi");
    data.container_id = Some(boxed.clone());
    let label = testing::add(&mut s, ElementKind::Text(data), 10.0, 10.0, 20.0, 20.0).unwrap();
    edit(&mut s, |tx| tx.update(&boxed, |e| e.frame_id = Some(frame.clone())));

    testing::select(&mut s, std::slice::from_ref(&frame));
    run(&mut s, "clipboard.copy", json!({}));
    let out = run(&mut s, "clipboard.paste", json!({"at": [2000, 0]}));
    assert_eq!(out["count"], 3);
    let new: Vec<Element> = live(&s).into_iter().filter(|e| e.id != frame && e.id != boxed && e.id != label).collect();
    let nframe = new.iter().find(|e| matches!(e.kind, ElementKind::Frame(_))).unwrap();
    let nbox = new.iter().find(|e| matches!(e.kind, ElementKind::Rectangle)).unwrap();
    let ntext = new.iter().find(|e| matches!(e.kind, ElementKind::Text(_))).unwrap();
    assert_eq!(nbox.frame_id.as_ref(), Some(&nframe.id));
    assert_eq!(ntext.container_id(), Some(&nbox.id));
    assert_eq!(out["ids"].as_array().unwrap().len(), 2, "the label is not selected");

    // The box alone: its frame stays behind, so the copy is in no frame.
    testing::select(&mut s, std::slice::from_ref(&boxed));
    run(&mut s, "clipboard.copy", json!({}));
    let new = id_list(&run(&mut s, "clipboard.paste", json!({"at": [0, 2000]})), "ids");
    assert_eq!(s.element(&new[0]).unwrap().frame_id, None);
}

#[test]
fn paste_works_across_the_system_clipboard_between_sessions() {
    let (mut one, fakes) = fx();
    two_rects(&mut one);
    run(&mut one, "clipboard.copy", json!({}));
    let mut two = Session::with_services(fakes.ports(), Services::fake());
    let out = run(&mut two, "clipboard.paste", json!({"at": [0, 0]}));
    assert_eq!(out["count"], 2);
    assert_eq!(live(&two).len(), 2);
}

// ---- paste: other sources ----

const EXCALIDRAW: &str = r##"{"type":"excalidraw/clipboard","elements":[{"type":"rectangle","id":"r1","x":0,"y":0,"width":100,"height":50,"strokeColor":"#000000","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,"roughness":1,"opacity":100,"angle":0,"seed":1,"version":1,"isDeleted":false,"groupIds":[],"frameId":null,"boundElements":null,"locked":false}],"files":{}}"##;

#[test]
fn excalidraw_clipboard_text_is_pasted_as_elements() {
    let (mut s, fakes) = fx();
    fakes.clipboard.set(ClipPayload::Text(EXCALIDRAW.to_owned()));
    let out = run(&mut s, "clipboard.paste", json!({"at": [100, 100]}));
    assert_eq!(out["kind"], "excalidraw");
    let e = s.element(&id_list(&out, "ids")[0]).unwrap();
    assert_eq!(centre(e), (100.0, 100.0));
    // Explicit data, as the web shell passes it.
    let out = run(&mut s, "clipboard.paste", json!({"source": {"text": EXCALIDRAW}, "at": [0, 0]}));
    assert_eq!(out["kind"], "excalidraw");
    let v: Value = serde_json::from_str(EXCALIDRAW).unwrap();
    let out = run(&mut s, "clipboard.paste", json!({"source": {"elements": v}, "at": [0, 0]}));
    assert_eq!(out["kind"], "excalidraw");
    assert_eq!(live(&s).len(), 3);
}

#[test]
fn unreadable_excalidraw_data_is_parse_failed() {
    let (mut s, _) = fx();
    let r = refuse_clean(&mut s, "clipboard.paste", json!({"source": {"text": r#"{"type":"excalidraw/clipboard","elements":[]}"#}}));
    assert_eq!(r.code, "parse_failed");
    assert!(r.message.starts_with("The pasted Excalidraw data could not be read: "), "{}", r.message);
    let r = refuse_clean(&mut s, "clipboard.paste", json!({"source": {"elements": {"hello": 1}}}));
    assert_eq!(r.code, "parse_failed");
    let r = refuse_clean(&mut s, "clipboard.paste", json!({"source": {"elements": "text"}}));
    assert_eq!(r.code, "parse_failed");
    let r = refuse_clean(&mut s, "clipboard.paste", json!({"source": {"elements": {"format": "drafftink-clipboard", "version": 1}}}));
    assert_eq!(r.code, "parse_failed");
}

#[test]
fn plain_text_skips_the_sniffing() {
    let (mut s, fakes) = fx();
    fakes.clipboard.set(ClipPayload::Text(EXCALIDRAW.to_owned()));
    let out = run(&mut s, "clipboard.paste", json!({"plain_text": true, "at": [0, 0]}));
    assert_eq!(out["kind"], "text");
    let e = s.element(&id_list(&out, "ids")[0]).unwrap();
    let ElementKind::Text(t) = &e.kind else { panic!() };
    assert_eq!(t.content, EXCALIDRAW);
    // With only an image on the clipboard there is no text to paste.
    fakes.clipboard.set(ClipPayload::Image { bytes: crate::cmd::image::tests::png(4, 4), mime: "image/png".into() });
    let r = refuse_clean(&mut s, "clipboard.paste", json!({"plain_text": true}));
    assert_eq!(r.code, "clipboard_empty");
    let r = refuse_clean(&mut s, "clipboard.paste", json!({"source": "internal", "plain_text": true}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn text_becomes_a_centred_selected_text_element_in_the_default_font() {
    let (mut s, fakes) = fx();
    fakes.clipboard.set(ClipPayload::Text("hello\nworld".to_owned()));
    let out = run(&mut s, "clipboard.paste", json!({"at": [50, 60]}));
    assert_eq!(out["kind"], "text");
    let e = s.element(&id_list(&out, "ids")[0]).unwrap();
    let (cx, cy) = centre(e);
    assert!((cx - 50.0).abs() < 1e-9 && (cy - 60.0).abs() < 1e-9, "{cx} {cy}");
    let ElementKind::Text(t) = &e.kind else { panic!() };
    assert_eq!(t.font, s.tool.text_defaults);
    assert!(t.auto_resize);
    assert_eq!(s.selection().len(), 1);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    // select=false leaves the selection.
    testing::select(&mut s, &[]);
    run(&mut s, "clipboard.paste", json!({"at": [0, 0], "select": false}));
    assert!(s.selection().is_empty());
}

#[test]
fn an_image_on_the_clipboard_is_inserted_with_the_800_rule() {
    let (mut s, fakes) = fx();
    fakes.clipboard.set(ClipPayload::Image { bytes: crate::cmd::image::tests::png(1600, 400), mime: "image/png".into() });
    let out = run(&mut s, "clipboard.paste", json!({"at": [0, 0]}));
    assert_eq!(out["kind"], "image");
    let e = s.element(&id_list(&out, "ids")[0]).unwrap();
    assert_eq!((e.transform.w, e.transform.h), (800.0, 200.0));
    assert_eq!(centre(e), (0.0, 0.0));
    // A pasted picture has no outline: the stroke colour is clear.
    assert_eq!(e.style.stroke_color, drafftink_scene::Color::TRANSPARENT);
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, crate::cmd::image::tests::png(10, 10));
    let out = run(&mut s, "clipboard.paste", json!({"source": {"image": {"base64": b64}}, "at": [0, 0]}));
    assert_eq!(out["kind"], "image");
    let e = s.element(&id_list(&out, "ids")[0]).unwrap();
    assert_eq!(e.style.stroke_color, drafftink_scene::Color::TRANSPARENT, "also from a given source");
    // An image that is not a picture format we read, with nothing else to paste.
    fakes.clipboard.set(ClipPayload::Image { bytes: b"%PDF-1.4 ....".to_vec(), mime: "application/pdf".into() });
    let r = refuse_clean(&mut s, "clipboard.paste", json!({}));
    assert_eq!(r.code, "unsupported_format");
}

#[test]
fn a_gif_bmp_or_ico_on_the_clipboard_is_pasted_as_a_picture() {
    use crate::cmd::image::tests::{GIF_1X1, bmp_2x1, ico_4_and_20};
    for (bytes, mime, size) in [
        (GIF_1X1.to_vec(), "image/gif", [1.0, 1.0]),
        (bmp_2x1(), "image/bmp", [2.0, 1.0]),
        (ico_4_and_20(), "image/x-icon", [20.0, 20.0]),
    ] {
        let (mut s, fakes) = fx();
        fakes.clipboard.set(ClipPayload::Image { bytes, mime: mime.into() });
        let out = run(&mut s, "clipboard.paste", json!({"at": [0, 0]}));
        assert_eq!(out["kind"], "image", "{mime}");
        let e = s.element(&id_list(&out, "ids")[0]).unwrap();
        assert_eq!([e.transform.w, e.transform.h], size, "{mime}");
    }
}

#[test]
fn a_png_with_a_scene_on_the_clipboard_is_still_pasted_as_a_picture() {
    // Excalidraw does the same: a scene is opened by dropping or opening the file, and a pasted picture is a
    // picture, so copying a screenshot or an export out of another app never replaces or merges a drawing.
    let png = include_bytes!("../../../../tests/fixtures/excalidraw/smiley_embedded_v2.png");
    let (mut s, fakes) = fx();
    fakes.clipboard.set(ClipPayload::Image { bytes: png.to_vec(), mime: "image/png".into() });
    let out = run(&mut s, "clipboard.paste", json!({"at": [0, 0]}));
    assert_eq!(out["kind"], "image");
    let kinds: Vec<_> = s.store().iter().map(|e| e.tag()).collect();
    assert_eq!(kinds, [drafftink_scene::Kind::Image]);
}

/// A clipboard that holds several flavours at once, as a real one does.
#[derive(Clone, Default)]
struct Multi(Arc<Mutex<Vec<ClipPayload>>>);

impl Clipboard for Multi {
    fn read(&self, want: &[ClipKind]) -> Result<Option<ClipPayload>, PortError> {
        Ok(self.0.lock().unwrap().iter().find(|p| want.contains(&p.kind())).cloned())
    }
    fn write(&mut self, payload: ClipPayload) -> Result<(), PortError> {
        *self.0.lock().unwrap() = vec![payload];
        Ok(())
    }
}

fn multi(items: Vec<ClipPayload>) -> Session {
    let fakes = Fakes::new();
    let m = Multi(Arc::new(Mutex::new(items)));
    let ports: Ports = fakes.headless_ports().with_clipboard(Box::new(m));
    Session::with_services(ports, Services::fake())
}

#[test]
fn the_system_order_is_elements_then_text_formats_then_image_then_plain_text() {
    let img = || ClipPayload::Image { bytes: crate::cmd::image::tests::png(30, 30), mime: "image/png".into() };
    let mut s = multi(vec![ClipPayload::Text("just words".into()), img()]);
    assert_eq!(run(&mut s, "clipboard.paste", json!({}))["kind"], "image", "an image beats plain text");
    let mut s = multi(vec![ClipPayload::Text(EXCALIDRAW.into()), img()]);
    assert_eq!(run(&mut s, "clipboard.paste", json!({}))["kind"], "excalidraw", "Excalidraw data beats an image");
    let (mut src, _) = fx();
    two_rects(&mut src);
    run(&mut src, "clipboard.copy", json!({}));
    let payload = src.ext.with::<CopyBuffer, _>(|b| b.envelope.clone()).unwrap();
    let mut s = multi(vec![ClipPayload::Text("words".into()), img(), ClipPayload::Elements(payload)]);
    assert_eq!(run(&mut s, "clipboard.paste", json!({}))["kind"], "elements", "our own payload beats everything");
    // A picture format we cannot read does not hide the text next to it.
    let mut s = multi(vec![ClipPayload::Text("words".into()), ClipPayload::Image { bytes: vec![1, 2, 3], mime: "image/bmp".into() }]);
    let out = run(&mut s, "clipboard.paste", json!({}));
    assert_eq!(out["kind"], "text");
    assert!(out["warnings"][0].as_str().unwrap().contains("pasted instead"), "{out}");
}

// ---- paste: Mermaid ----

#[derive(Default)]
struct Seen(Vec<Value>);

fn stub_import(s: &mut Session, args: Value) -> Result<Value, Refusal> {
    s.ext.with::<Seen, _>(|seen| seen.0.push(args));
    Ok(json!({}))
}

static STUB: &[CommandSpec] = &[CommandSpec::new(
    "diagram.import_mermaid",
    "diagram",
    "Import Mermaid diagram",
    "A stand-in for the diagram family, which records what it was asked.",
    &Typed::new(stub_import_typed),
    Facts { palette: false, ..Facts::READ_ONLY },
)];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct StubArgs {
    source: Value,
    at: Value,
    select: bool,
}

fn stub_import_typed(s: &mut Session, a: StubArgs) -> Result<Value, Refusal> {
    stub_import(s, json!({"source": a.source, "at": a.at, "select": a.select}))
}

#[test]
fn mermaid_text_goes_to_the_diagram_command_when_it_exists() {
    let fakes = Fakes::new();
    let reg = Arc::new(Registry::from_families(&[super::COMMANDS, STUB]));
    let mut s = Session::with_registry(fakes.ports(), reg);
    s.services = Services::fake();
    fakes.clipboard.set(ClipPayload::Text("graph TD\nA-->B".into()));
    let out = run(&mut s, "clipboard.paste", json!({"at": [10, 20]}));
    assert_eq!(out["kind"], "mermaid");
    let seen = s.ext.with::<Seen, _>(|seen| seen.0.clone());
    assert_eq!(seen, vec![json!({"source": {"text": "graph TD\nA-->B"}, "at": [10.0, 20.0], "select": true})]);
    // plain_text pastes the same text as text.
    let out = run(&mut s, "clipboard.paste", json!({"plain_text": true, "at": [0, 0]}));
    assert_eq!(out["kind"], "text");
    assert_eq!(s.ext.with::<Seen, _>(|seen| seen.0.len()), 1);
}

#[test]
fn mermaid_text_without_the_diagram_command_stays_text_and_says_so() {
    let (mut s, fakes) = fx();
    if s.spec("diagram.import_mermaid").is_some() {
        return; // the diagram family is registered: the routing test above covers it
    }
    fakes.clipboard.set(ClipPayload::Text("graph TD\nA-->B".into()));
    let out = run(&mut s, "clipboard.paste", json!({"at": [0, 0]}));
    assert_eq!(out["kind"], "text");
    assert!(out["warnings"][0].as_str().unwrap().contains("Mermaid import is not available"), "{out}");
}

#[test]
fn text_that_only_looks_like_mermaid_is_text() {
    let (mut s, fakes) = fx();
    fakes.clipboard.set(ClipPayload::Text("graph TD\n".into()));
    let out = run(&mut s, "clipboard.paste", json!({"at": [0, 0]}));
    assert_eq!(out["kind"], "text");
    assert!(out["warnings"][0].as_str().unwrap().contains("looks like a Mermaid diagram"), "{out}");
}

// ---- paste: refusals ----

#[test]
fn an_empty_clipboard_refuses_with_what_is_supported() {
    let (mut s, fakes) = fx();
    let r = refuse_clean(&mut s, "clipboard.paste", json!({}));
    assert_eq!(r.code, "clipboard_empty");
    assert_eq!(
        r.message,
        "The clipboard has nothing drafft.ink can paste. Supported: copied elements, Excalidraw data, images, Mermaid diagrams and plain text."
    );
    assert_eq!(refuse_clean(&mut s, "clipboard.paste", json!({"source": "internal"})).code, "clipboard_empty");
    assert_eq!(refuse_clean(&mut s, "clipboard.paste", json!({"source": {"text": "  \n"}})).code, "clipboard_empty");
    fakes.clipboard.set(ClipPayload::Svg("<svg/>".into()));
    assert_eq!(refuse_clean(&mut s, "clipboard.paste", json!({})).code, "clipboard_empty");
}

#[test]
fn without_a_clipboard_port_the_system_source_names_the_alternative() {
    let mut s = testing::headless_session();
    let r = refuse_clean(&mut s, "clipboard.paste", json!({}));
    assert_eq!(r.code, "port_unavailable");
    assert_eq!(
        r.message,
        "`clipboard.paste` with source system needs the system clipboard, which this session does not provide. Pass `source` with the data instead."
    );
    // Explicit data needs no port.
    run(&mut s, "clipboard.paste", json!({"source": {"text": "hi"}, "at": [0, 0]}));
}

#[test]
fn bad_places_are_refused() {
    let (mut s, _) = fx();
    for at in [json!([1e300, 0]), json!("somewhere"), json!([1])] {
        let r = refuse_clean(&mut s, "clipboard.paste", json!({"source": {"text": "x"}, "at": at}));
        assert_eq!(r.code, "bad_args");
    }
}

#[test]
fn a_huge_text_is_refused_not_cut() {
    let (mut s, _) = fx();
    let text = "x".repeat(1_000_001);
    let r = refuse_clean(&mut s, "clipboard.paste", json!({"source": {"text": text}, "at": [0, 0]}));
    assert_eq!(r.code, "limit_exceeded");
}

// ---- copy_style and paste_style ----

#[test]
fn a_style_is_copied_from_one_element_and_pasted_onto_others() {
    let (mut s, _) = fx();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let b = testing::add_rect(&mut s, 100.0, 0.0, 50.0, 50.0).unwrap();
    let c = testing::add(&mut s, ElementKind::Ellipse, 200.0, 0.0, 50.0, 50.0).unwrap();
    testing::select(&mut s, std::slice::from_ref(&a));
    run(&mut s, "style.set", json!({"props": {"stroke_color": "red-500", "fill_color": "blue-100", "stroke_width": 8, "opacity": 0.5}, "scope": "selection"}));
    let out = run(&mut s, "clipboard.copy_style", json!({}));
    assert_eq!(out["from"], a.as_str());
    assert_eq!(out["kind"], "rectangle");
    assert_eq!(out["props"]["stroke_width"], 8.0);
    assert_eq!(out["changed"], false);

    testing::select(&mut s, &[b.clone(), c.clone()]);
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "clipboard.paste_style", json!({}));
    assert_eq!(out["undo_label"], "Paste style");
    assert_eq!(s.tab().doc.undo_depth(), depth + 1);
    for id in [&b, &c] {
        let e = s.element(id).unwrap();
        assert_eq!(e.style.stroke_width, 8.0);
        assert_eq!(e.style.opacity, 0.5);
        assert_eq!(e.style.stroke_color, s.element(&a).unwrap().style.stroke_color);
    }
    // Pasting the same style again changes nothing and makes no undo step.
    let out = run(&mut s, "clipboard.paste_style", json!({}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), depth + 1);
    s.undo_steps(1).unwrap();
    assert_eq!(s.element(&b).unwrap().style.stroke_width, 2.0);
}

#[test]
fn copy_style_takes_the_first_selected_in_z_order_unless_told() {
    let (mut s, _) = fx();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    let b = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    testing::select(&mut s, &[b.clone(), a.clone()]);
    assert_eq!(run(&mut s, "clipboard.copy_style", json!({}))["from"], a.as_str());
    assert_eq!(run(&mut s, "clipboard.copy_style", json!({"from": b.as_str()}))["from"], b.as_str());
    let r = refuse_clean(&mut s, "clipboard.copy_style", json!({"from": "nope"}));
    assert_eq!(r.code, "not_found");
}

#[test]
fn text_properties_are_skipped_on_shapes_without_a_label_and_reported() {
    let (mut s, _) = fx();
    let t = testing::add(&mut s, ElementKind::Text(TextData::new("hi")), 0.0, 0.0, 20.0, 20.0).unwrap();
    let r = testing::add_rect(&mut s, 100.0, 0.0, 50.0, 50.0).unwrap();
    testing::select(&mut s, std::slice::from_ref(&t));
    run(&mut s, "style.set", json!({"props": {"font_size": 36, "stroke_color": "green-500"}, "scope": "selection"}));
    run(&mut s, "clipboard.copy_style", json!({}));
    testing::select(&mut s, std::slice::from_ref(&r));
    let out = run(&mut s, "clipboard.paste_style", json!({}));
    let skipped = out["skipped"].as_array().unwrap();
    assert_eq!(skipped[0]["id"], r.as_str());
    assert!(skipped[0]["props"].as_array().unwrap().iter().any(|p| p == "font_size"), "{out}");
    assert_eq!(s.element(&r).unwrap().style.stroke_color, s.element(&t).unwrap().style.stroke_color);
}

#[test]
fn paste_style_before_any_copy_says_what_to_do_first() {
    let (mut s, _) = fx();
    two_rects(&mut s);
    let r = refuse_clean(&mut s, "clipboard.paste_style", json!({}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "No style has been copied yet. Copy one with `clipboard.copy_style` first.");
}

#[test]
fn paste_style_refuses_locked_targets_and_unknown_ids() {
    let (mut s, _) = fx();
    let (a, b) = two_rects(&mut s);
    run(&mut s, "clipboard.copy_style", json!({"from": a.as_str()}));
    edit(&mut s, |tx| tx.update(&b, |e| e.locked = true));
    let r = refuse_clean(&mut s, "clipboard.paste_style", json!({"ids": [b.as_str()]}));
    assert_eq!(r.code, "locked");
    let r = refuse_clean(&mut s, "clipboard.paste_style", json!({"ids": ["nope"]}));
    assert_eq!(r.code, "not_found");
}

// ---- copy_as ----

#[test]
fn copy_as_png_renders_the_selection_and_writes_the_image() {
    let (mut s, fakes) = fx();
    let (_a, _b) = two_rects(&mut s);
    testing::add_rect(&mut s, 5000.0, 5000.0, 10.0, 10.0).unwrap();
    let out = run(&mut s, "clipboard.copy_as", json!({"scale": 1, "padding": 10}));
    assert_eq!(out["format"], "png");
    assert_eq!(out["count"], 2);
    // The bounds of the two rectangles are 280 x 130; padding 10 on every side.
    assert_eq!(out["size_px"], json!([300, 150]));
    assert_eq!(fakes.renderer.calls(), 1);
    let Some(ClipPayload::Image { bytes, mime }) = fakes.clipboard.content() else { panic!("no image") };
    assert_eq!(mime, "image/png");
    let text = String::from_utf8_lossy(&bytes).to_string();
    assert!(text.contains("300x150") && text.contains("elements=2"), "only the selection is drawn: {text}");
}

#[test]
fn copy_as_svg_and_text() {
    let (mut s, fakes) = fx();
    let (a, _b) = two_rects(&mut s);
    testing::add(&mut s, ElementKind::Text(TextData::new("note one")), 0.0, 200.0, 80.0, 20.0).unwrap();
    testing::add(&mut s, ElementKind::Text(TextData::new("note two")), 0.0, 240.0, 80.0, 20.0).unwrap();
    run(&mut s, "clipboard.copy_as", json!({"format": "svg", "scope": "all"}));
    let Some(ClipPayload::Svg(svg)) = fakes.clipboard.content() else { panic!("no svg") };
    assert!(svg.starts_with("<svg") && svg.contains("note one"), "{}", &svg[..svg.len().min(80)]);
    assert!(!svg.contains("drafftink-scene"), "no embedded scene on the clipboard");
    let out = run(&mut s, "clipboard.copy_as", json!({"format": "text", "scope": "all"}));
    assert_eq!(fakes.clipboard.content(), Some(ClipPayload::Text("note one\nnote two".into())));
    assert_eq!(out["count"], 4);
    let r = refuse_clean(&mut s, "clipboard.copy_as", json!({"format": "text", "scope": "ids", "ids": [a.as_str()]}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "Nothing to copy: none of the elements holds text.");
}

#[test]
fn copy_as_refusals() {
    let (mut s, _) = fx();
    let r = refuse_clean(&mut s, "clipboard.copy_as", json!({}));
    assert_eq!(r.code, "no_target");
    let r = refuse_clean(&mut s, "clipboard.copy_as", json!({"scope": "all"}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "Nothing to copy: the document has no elements. Draw something first.");
    two_rects(&mut s);
    for bad in [json!({"scale": 0}), json!({"scale": 9}), json!({"padding": -1}), json!({"scale": 4, "padding": 1000, "scope": "all"}), json!({"ids": ["x"]}), json!({"scope": "ids"})] {
        let r = refuse_clean(&mut s, "clipboard.copy_as", bad.clone());
        assert!(matches!(r.code, "bad_args"), "{bad}: {}", r.message);
    }

    let mut headless = testing::headless_session();
    two_rects(&mut headless);
    let r = refuse_clean(&mut headless, "clipboard.copy_as", json!({"format": "svg"}));
    assert_eq!(r.code, "port_unavailable");
    assert!(r.message.contains("system clipboard"), "{}", r.message);

    // A clipboard but no renderer: png refuses, svg works.
    let fakes = Fakes::new();
    let ports = fakes.headless_ports().with_clipboard(Box::new(fakes.clipboard.clone()));
    let mut s = Session::with_services(ports, Services::fake());
    two_rects(&mut s);
    let r = refuse_clean(&mut s, "clipboard.copy_as", json!({}));
    assert_eq!(r.code, "port_unavailable");
    assert!(r.message.contains("the renderer"), "{}", r.message);
    run(&mut s, "clipboard.copy_as", json!({"format": "svg"}));
}

/// A renderer that records every request.
#[derive(Clone, Default)]
struct RecordingRenderer(Arc<Mutex<Vec<RenderRequest>>>);

impl Renderer for RecordingRenderer {
    fn render_png(&self, _: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError> {
        self.0.lock().unwrap().push(req.clone());
        Ok(b"\x89PNG-recorded".to_vec())
    }
}

fn recording(s: &mut Session) -> RecordingRenderer {
    let r = RecordingRenderer::default();
    s.ports.renderer = Some(Box::new(r.clone()));
    r
}

#[test]
fn copy_as_dark_draws_with_the_dark_palette_without_a_warning() {
    let (mut s, _) = fx();
    let seen = recording(&mut s);
    two_rects(&mut s);
    let out = run(&mut s, "clipboard.copy_as", json!({"dark": true}));
    assert_eq!(out["warnings"], json!([]), "dark is supported now");
    assert!(seen.0.lock().unwrap().last().unwrap().dark);
    // The setting `export.dark` is the default, as for `doc.export`.
    run(&mut s, "settings.set", json!({"key": "export.dark", "value": true}));
    run(&mut s, "clipboard.copy_as", json!({}));
    assert!(seen.0.lock().unwrap().last().unwrap().dark);
    run(&mut s, "clipboard.copy_as", json!({"dark": false}));
    assert!(!seen.0.lock().unwrap().last().unwrap().dark);
}

#[test]
fn copy_as_svg_embeds_fonts_and_honours_dark_like_doc_export() {
    let (mut s, fakes) = fx();
    run(&mut s, "element.create", json!({"kind": "text", "text": "hi", "at": [0, 0], "style": {"font_family": "gelpen_serif"}}));
    let out = run(&mut s, "clipboard.copy_as", json!({"format": "svg", "scope": "all", "dark": true}));
    assert_eq!(out["warnings"], json!([]), "dark is supported for svg now");
    let Some(ClipPayload::Svg(svg)) = fakes.clipboard.content() else { panic!("no svg") };
    assert!(svg.contains("@font-face{font-family:'GelPen Serif';font-weight:400;"), "{}", &svg[..svg.len().min(300)]);
    assert!(svg.contains("fill=\"#121212\""), "the dark background");
}

#[test]
fn copy_as_png_and_doc_export_make_the_same_render_request() {
    let (mut s, _) = fx();
    let seen = recording(&mut s);
    two_rects(&mut s);
    let args = json!({"scale": 1.5, "padding": 7, "background": false, "dark": true});
    run(&mut s, "clipboard.copy_as", args.clone());
    let mut export = args;
    export["scope"] = json!("selection");
    export["embed_scene"] = json!(false);
    run(&mut s, "doc.export", export);
    let reqs = seen.0.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0], reqs[1], "one PNG implementation: the same scene and options give the same request");
}

// ---- hostile input ----

#[test]
fn hostile_payloads_never_panic() {
    let (mut s, _) = fx();
    let junk = [
        json!({"format": "drafftink-clipboard", "version": 1, "document": {"elements": [{"id": "a"}]}}),
        json!({"format": "drafftink-clipboard", "version": 1, "document": {"elements": [], "files": {"x": 5}}}),
        json!({"type": "excalidraw/clipboard", "elements": [{"type": "arrow", "points": [[1e308, 0], [0, 1e308]], "x": 1e308}]}),
        json!({"type": "excalidraw/clipboard", "elements": [{"type": "text", "text": "x".repeat(5000)}]}),
        json!([[[[[[[[[[1]]]]]]]]]]),
    ];
    for v in junk {
        for source in [json!({"elements": v.clone()}), json!({"text": v.to_string()})] {
            if let Err(r) = s.execute("clipboard.paste", json!({"source": source, "at": [0, 0]})) {
                assert_ne!(r.code, "internal", "{}", r.message);
            }
        }
    }
}
