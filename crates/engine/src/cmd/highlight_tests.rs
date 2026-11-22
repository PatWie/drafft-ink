//! Tests of `highlight.apply` and of the `highlight` style property: the effect, one undo step, refusals that
//! change nothing, the batch, the defaults, and the trip through the replicated document.

use drafftink_doc::LoroStore;
use drafftink_scene::{
    CharStyle, ElementId, ElementKind, FrameData, MarkerColor, SceneRead, SpanHighlight, TextData, TextRun,
};
use serde_json::{Value, json};

use crate::batch::{BatchOpts, BatchStep};
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

/// Refuses and checks that nothing changed.
fn refuse_clean(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    };
    assert_eq!(s.fingerprint(), before, "{id} refused but changed state: {}", r.message);
    r
}

struct Fx {
    s: Session,
    rect: ElementId,
    arrow: ElementId,
    text: ElementId,
    frame: ElementId,
}

fn fixture() -> Fx {
    let mut s = testing::headless_session();
    let rect = testing::add_rect(&mut s, 0.0, 0.0, 100.0, 60.0).unwrap();
    let arrow = {
        use drafftink_geom::kurbo::Point;
        let mut a = drafftink_scene::LinearData::new(vec![Point::new(0.0, 0.0), Point::new(80.0, 20.0)]);
        a.end_head = drafftink_scene::Head::Arrow;
        testing::add(&mut s, ElementKind::Arrow(a), 200.0, 0.0, 80.0, 20.0).unwrap()
    };
    let text = testing::add(&mut s, ElementKind::Text(TextData::new("héllo wörld")), 0.0, 200.0, 90.0, 25.0).unwrap();
    let frame = testing::add(&mut s, ElementKind::Frame(FrameData::default()), 400.0, 0.0, 200.0, 200.0).unwrap();
    testing::select(&mut s, std::slice::from_ref(&rect));
    Fx { s, rect, arrow, text, frame }
}

fn highlight(s: &Session, id: &ElementId) -> Option<MarkerColor> {
    s.store().get(id).unwrap().style.highlight
}

fn runs(s: &Session, id: &ElementId) -> Vec<TextRun> {
    s.store().get(id).and_then(|e| e.text_data().map(|t| t.runs.clone())).unwrap()
}

#[test]
fn style_set_carries_the_highlight_and_null_removes_it() {
    let mut f = fixture();
    let out = run(&mut f.s, "style.set", json!({"props": {"highlight": "green"}}));
    assert_eq!(out["changed"], true);
    assert_eq!(out["applied"][f.rect.as_str()], json!(["highlight"]));
    assert_eq!(highlight(&f.s, &f.rect), Some(MarkerColor::Green));
    let got = run(&mut f.s, "style.get", json!({"props": ["highlight"]}));
    assert_eq!(got["props"]["highlight"], json!({"applicable": true, "value": "green"}));
    run(&mut f.s, "style.set", json!({"props": {"highlight": null}}));
    assert_eq!(highlight(&f.s, &f.rect), None);
    // Two plain `style.set` calls in a row are one undo step (they coalesce), so one undo takes back both.
    f.s.undo_steps(1).unwrap();
    assert_eq!(highlight(&f.s, &f.rect), None);
    assert!(refuse_clean(&mut f.s, "style.set", json!({"props": {"highlight": "mauve"}})).message.contains("mauve"));
}

#[test]
fn the_defaults_for_new_elements_never_take_a_highlight_and_a_frame_does_not_support_it() {
    let mut f = fixture();
    run(&mut f.s, "style.set", json!({"props": {"highlight": "pink"}}));
    assert_eq!(f.s.tool.defaults().to_props().highlight, None);
    let defaults = run(&mut f.s, "style.get", json!({"scope": "defaults", "props": ["highlight"]}));
    assert_eq!(defaults["props"]["highlight"]["applicable"], false);
    testing::select(&mut f.s, std::slice::from_ref(&f.frame));
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"highlight": "pink"}, "scope": "selection"}));
    assert_eq!(r.code, "not_applicable");
}

#[test]
fn apply_marks_elements_and_character_ranges_in_one_undo_step() {
    let mut f = fixture();
    let before = f.s.tab().doc.undo_depth();
    let rev = f.s.tab().doc.revision();
    let out = run(
        &mut f.s,
        "highlight.apply",
        json!({
            "color": "yellow",
            "ids": [f.rect.as_str(), f.arrow.as_str(), f.frame.as_str()],
            "ranges": [{"id": f.text.as_str(), "start": 6, "end": 11}]
        }),
    );
    assert_eq!(out["changed"], true);
    assert_eq!(out["highlighted"], json!([f.rect.as_str(), f.arrow.as_str(), f.text.as_str()]));
    assert_eq!(out["skipped"], json!([f.frame.as_str()]));
    assert_eq!(highlight(&f.s, &f.arrow), Some(MarkerColor::Yellow));
    assert_eq!(highlight(&f.s, &f.frame), None);
    assert_eq!(
        runs(&f.s, &f.text),
        [TextRun::new(6, 11, CharStyle { highlight: SpanHighlight::On(MarkerColor::Yellow), ..Default::default() })],
        "characters, not bytes: the ö is one character"
    );
    assert_eq!(f.s.tab().doc.undo_depth(), before + 1, "one undo step");
    assert_eq!(f.s.tab().doc.revision(), rev + 1, "one change");
    f.s.undo_steps(1).unwrap();
    assert_eq!(highlight(&f.s, &f.rect), None);
    assert_eq!(highlight(&f.s, &f.arrow), None);
    assert!(runs(&f.s, &f.text).is_empty());
}

#[test]
fn apply_defaults_to_the_selection_and_clears_with_a_missing_colour() {
    let mut f = fixture();
    run(&mut f.s, "highlight.apply", json!({"color": "blue"}));
    assert_eq!(highlight(&f.s, &f.rect), Some(MarkerColor::Blue));
    let again = run(&mut f.s, "highlight.apply", json!({"color": "blue"}));
    assert_eq!((again["changed"].clone(), again["unchanged"].clone()), (json!(false), json!([f.rect.as_str()])));
    run(&mut f.s, "highlight.apply", json!({}));
    assert_eq!(highlight(&f.s, &f.rect), None);
}

#[test]
fn a_range_with_nothing_else_does_not_need_a_selection() {
    let mut f = fixture();
    testing::select(&mut f.s, &[]);
    run(&mut f.s, "highlight.apply", json!({"color": "orange", "ranges": [{"id": f.text.as_str(), "start": 0, "end": 5}]}));
    assert_eq!(runs(&f.s, &f.text).len(), 1);
    // Taking the highlight off a part of a highlighted text.
    run(&mut f.s, "highlight.apply", json!({"color": "orange", "ids": [f.text.as_str()]}));
    assert!(runs(&f.s, &f.text).is_empty(), "the whole text says it now");
    run(&mut f.s, "highlight.apply", json!({"color": null, "ranges": [{"id": f.text.as_str(), "start": 2, "end": 4}]}));
    assert_eq!(runs(&f.s, &f.text)[0].highlight, SpanHighlight::Off);
}

#[test]
fn refusals_change_nothing() {
    let mut f = fixture();
    let t = f.text.as_str().to_owned();
    for (args, code) in [
        (json!({"color": "yellow", "ranges": [{"id": t, "start": 3, "end": 40}]}), "bad_range"),
        (json!({"color": "yellow", "ranges": [{"id": t, "start": 3, "end": 3}]}), "bad_range"),
        (json!({"color": "yellow", "ranges": [{"id": f.rect.as_str(), "start": 0, "end": 1}]}), "wrong_kind"),
        (json!({"color": "yellow", "ids": [f.frame.as_str()]}), "not_applicable"),
        (json!({"color": "yellow", "ids": ["nope"]}), "not_found"),
        (json!({"color": "chartreuse"}), "bad_args"),
        (json!({"color": "yellow", "extra": 1}), "bad_args"),
    ] {
        let r = refuse_clean(&mut f.s, "highlight.apply", args.clone());
        assert_eq!(r.code, code, "{args}: {}", r.message);
    }
    testing::select(&mut f.s, &[]);
    assert_eq!(refuse_clean(&mut f.s, "highlight.apply", json!({"color": "yellow"})).code, "no_target");
    f.s.view_only = true;
    assert_eq!(refuse_clean(&mut f.s, "highlight.apply", json!({"color": "yellow", "ids": [f.rect.as_str()]})).code, "view_only");
}

#[test]
fn a_locked_element_refuses_the_whole_call() {
    let mut f = fixture();
    run(&mut f.s, "element.lock", json!({"ids": [f.arrow.as_str()]}));
    let r = refuse_clean(
        &mut f.s,
        "highlight.apply",
        json!({"color": "yellow", "ids": [f.rect.as_str(), f.arrow.as_str()]}),
    );
    assert_eq!(r.code, "locked");
    assert_eq!(highlight(&f.s, &f.rect), None);
}

#[test]
fn a_batch_of_applies_is_one_undo_step() {
    let mut f = fixture();
    let before = f.s.tab().doc.undo_depth();
    let step = |args: Value| BatchStep { id: "highlight.apply".to_owned(), args, as_name: None };
    let out = f.s.execute_batch(
        Origin::Test,
        &[
            step(json!({"color": "yellow", "ids": [f.rect.as_str()]})),
            step(json!({"color": "pink", "ranges": [{"id": f.text.as_str(), "start": 0, "end": 5}]})),
            step(json!({"color": "green", "ids": [f.arrow.as_str()]})),
        ],
        &BatchOpts::default(),
    );
    assert!(out.failed.is_none(), "{:?}", out.failed);
    assert_eq!(f.s.tab().doc.undo_depth(), before + 1);
    f.s.undo_steps(1).unwrap();
    assert!(highlight(&f.s, &f.rect).is_none() && highlight(&f.s, &f.arrow).is_none() && runs(&f.s, &f.text).is_empty());
}

#[test]
fn the_highlight_arrives_in_a_second_replica() {
    let mut f = fixture();
    run(
        &mut f.s,
        "highlight.apply",
        json!({"color": "green", "ids": [f.rect.as_str()], "ranges": [{"id": f.text.as_str(), "start": 0, "end": 5}]}),
    );
    let mut peer = LoroStore::new();
    peer.apply_remote(&f.s.tab().doc.replicated().export_snapshot().unwrap()).unwrap();
    assert_eq!(peer.get(&f.rect).unwrap().style.highlight, Some(MarkerColor::Green));
    let got = peer.get(&f.text).and_then(|e| e.text_data().map(|t| t.runs.clone())).unwrap();
    assert_eq!(got, runs(&f.s, &f.text));
}

#[test]
fn the_catalog_says_what_the_command_is() {
    let s = testing::headless_session();
    let spec = s.spec("highlight.apply").unwrap();
    assert!(spec.host.text_editor_takes_style);
    assert_eq!(spec.family, "highlight");
}
