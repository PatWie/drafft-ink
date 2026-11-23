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
    assert_eq!(highlight(&f.s, &f.rect), Some(MarkerColor::from_name("green-400").unwrap()));
    let got = run(&mut f.s, "style.get", json!({"props": ["highlight"]}));
    assert_eq!(got["props"]["highlight"], json!({"applicable": true, "value": "green-400"}));
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
    assert_eq!(highlight(&f.s, &f.arrow), Some(MarkerColor::YELLOW));
    assert_eq!(highlight(&f.s, &f.frame), None);
    assert_eq!(
        runs(&f.s, &f.text),
        [TextRun::new(6, 11, CharStyle { highlight: SpanHighlight::On(MarkerColor::YELLOW), ..Default::default() })],
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
    assert_eq!(highlight(&f.s, &f.rect), Some(MarkerColor::from_name("blue-400").unwrap()));
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
    assert_eq!(peer.get(&f.rect).unwrap().style.highlight, Some(MarkerColor::from_name("green-400").unwrap()));
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

#[test]
fn the_old_names_work_in_every_command_that_takes_a_marker_and_come_back_as_the_new_ones() {
    for (old, new) in [
        ("yellow", "yellow-400"),
        ("green", "green-400"),
        ("pink", "pink-400"),
        ("blue", "blue-400"),
        ("orange", "orange-400"),
    ] {
        let mut f = fixture();
        run(&mut f.s, "style.set", json!({"props": {"highlight": old}, "scope": "selection"}));
        let got = run(&mut f.s, "style.get", json!({"props": ["highlight"]}));
        assert_eq!(got["props"]["highlight"]["value"], new, "style.set {old}");
        run(&mut f.s, "highlight.apply", json!({"color": null}));
        run(&mut f.s, "highlight.apply", json!({"color": old, "ranges": [{"id": f.text.as_str(), "start": 0, "end": 2}]}));
        assert_eq!(
            runs(&f.s, &f.text)[0].highlight,
            SpanHighlight::On(MarkerColor::from_name(new).unwrap()),
            "highlight.apply {old}"
        );
        let out = run(&mut f.s, "tool.set_marker", json!({"color": old}));
        assert_eq!(out["color"], new, "tool.set_marker {old}");
        assert_eq!(f.s.tool.marker, MarkerColor::from_name(new), "{old}");
    }
}

#[test]
fn every_palette_shade_and_a_custom_colour_is_taken_exactly_and_a_malformed_one_is_refused_with_the_choices() {
    let mut f = fixture();
    for name in ["slate-300", "emerald-25", "rose-950", "Amber-400", "blue-100", "yellow-600", "#facc15", "#FACC15"] {
        run(&mut f.s, "highlight.apply", json!({"color": name}));
        let got = run(&mut f.s, "style.get", json!({"props": ["highlight"]}));
        assert_eq!(got["props"]["highlight"]["value"], name.to_lowercase(), "{name}: stored as picked, not clamped");
    }
    run(&mut f.s, "style.set", json!({"props": {"highlight": "#12abef"}, "scope": "selection"}));
    let got = run(&mut f.s, "style.get", json!({"props": ["highlight"]}));
    assert_eq!(got["props"]["highlight"]["value"], "#12abef", "style.set takes the same set");
    for bad in ["blue-150", "blue-1000", "#12", "#gg0000", "mauve-400", "banana", "none"] {
        let before = f.s.fingerprint();
        let r = f.s.execute_from(Origin::Test, "highlight.apply", json!({"color": bad})).unwrap_err();
        assert_eq!(r.code, "bad_args", "{bad}");
        assert!(r.message.contains("950") && r.message.contains("#rrggbb"), "{}", r.message);
        assert_eq!(f.s.fingerprint(), before, "{bad}");
        assert_eq!(refuse_clean(&mut f.s, "style.set", json!({"props": {"highlight": bad}})).code, "bad_args", "{bad}");
    }
    let out = run(&mut f.s, "highlight.apply", json!({"color": "blue"}));
    assert_eq!(out["changed"], true, "`blue` is the old name of blue-400");
}

#[test]
fn a_custom_colour_and_a_pale_shade_mark_ranges_of_a_text() {
    let mut f = fixture();
    run(&mut f.s, "highlight.apply", json!({"color": "#12abef", "ranges": [{"id": f.text.as_str(), "start": 0, "end": 2}]}));
    let custom = MarkerColor::from_name("#12abef").unwrap();
    assert_eq!(runs(&f.s, &f.text)[0].highlight, SpanHighlight::On(custom));
    run(&mut f.s, "highlight.apply", json!({"color": "blue-25", "ranges": [{"id": f.text.as_str(), "start": 3, "end": 4}]}));
    assert!(runs(&f.s, &f.text).iter().any(|r| r.highlight == SpanHighlight::On(MarkerColor::from_name("blue-25").unwrap())));
}

#[test]
fn clear_takes_the_highlight_off_elements_and_characters_in_one_undo_step() {
    let mut f = fixture();
    run(&mut f.s, "highlight.apply", json!({"color": "yellow", "ids": [f.rect.as_str()]}));
    run(&mut f.s, "highlight.apply", json!({"color": "blue", "ranges": [{"id": f.text.as_str(), "start": 0, "end": 5}]}));
    testing::select(&mut f.s, &[f.rect.clone(), f.text.clone(), f.frame.clone()]);
    let before = f.s.tab().doc.undo_depth();
    let out = run(&mut f.s, "highlight.clear", json!({}));
    assert_eq!(out["highlighted"], json!([f.rect.as_str(), f.text.as_str()]));
    assert_eq!(out["skipped"], json!([f.frame.as_str()]));
    assert_eq!(highlight(&f.s, &f.rect), None);
    assert!(runs(&f.s, &f.text).is_empty(), "the characters' own highlight goes with the element's");
    assert_eq!(f.s.tab().doc.undo_depth(), before + 1, "one undo step");
    // One undo brings back both the element's and the characters' highlight.
    f.s.undo_steps(1).unwrap();
    assert_eq!(highlight(&f.s, &f.rect), Some(MarkerColor::YELLOW));
    assert_eq!(runs(&f.s, &f.text).len(), 1);
}

#[test]
fn clear_takes_off_the_highlight_of_a_text_that_only_its_characters_have() {
    let mut f = fixture();
    run(&mut f.s, "highlight.apply", json!({"color": "pink", "ranges": [{"id": f.text.as_str(), "start": 1, "end": 3}]}));
    testing::select(&mut f.s, std::slice::from_ref(&f.text));
    assert_eq!(highlight(&f.s, &f.text), None);
    assert!(f.s.availability("highlight.clear").is_ok());
    let out = run(&mut f.s, "highlight.clear", json!({}));
    assert_eq!(out["changed"], true);
    assert!(runs(&f.s, &f.text).is_empty());
}

#[test]
fn clear_is_refused_while_nothing_selected_is_highlighted_and_changes_nothing() {
    let mut f = fixture();
    let reason = f.s.availability("highlight.clear").unwrap_err();
    assert_eq!((reason.code, reason.message.as_str()), ("nothing_to_do", "Nothing selected is highlighted."));
    assert_eq!(refuse_clean(&mut f.s, "highlight.clear", json!({})).message, reason.message);
    // Nothing selected is refused for the same reason: there is nothing to act on.
    testing::select(&mut f.s, &[]);
    assert!(f.s.availability("highlight.clear").is_err());
    // A highlight elsewhere does not count, and a frame cannot carry one.
    run(&mut f.s, "highlight.apply", json!({"color": "green", "ids": [f.arrow.as_str()]}));
    testing::select(&mut f.s, &[f.rect.clone(), f.frame.clone()]);
    assert!(f.s.availability("highlight.clear").is_err());
    testing::select(&mut f.s, &[f.rect.clone(), f.arrow.clone()]);
    assert!(f.s.availability("highlight.clear").is_ok());
}

#[test]
fn clear_takes_ids_and_ranges_and_never_refuses_for_having_nothing_to_clear() {
    let mut f = fixture();
    run(&mut f.s, "highlight.apply", json!({"color": "orange", "ids": [f.text.as_str()]}));
    let out = run(&mut f.s, "highlight.clear", json!({"ranges": [{"id": f.text.as_str(), "start": 0, "end": 5}]}));
    assert_eq!(out["changed"], true);
    assert_eq!(runs(&f.s, &f.text)[0].highlight, SpanHighlight::Off, "the characters opt out of the whole text's");
    let out = run(&mut f.s, "highlight.clear", json!({"ids": [f.arrow.as_str()]}));
    assert_eq!((out["changed"].clone(), out["unchanged"].clone()), (json!(false), json!([f.arrow.as_str()])));
    assert!(refuse_clean(&mut f.s, "highlight.clear", json!({"ids": [f.frame.as_str()]})).message.contains("highlight"));
}

#[test]
fn highlight_apply_stays_available_with_nothing_highlighted() {
    let f = fixture();
    assert!(f.s.availability("highlight.apply").is_ok());
}
