//! Tests of the `style` family. Pattern per command: availability, refusal text, effect, undo, one negative case.
//! No UI, no DOM: a headless session and the fake ports.

use drafftink_edit::style::{StyleProps, theme};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Color, ElementId, ElementKind, FreehandData, LinearData, MathData, SceneRead, TextData};
use serde_json::{Value, json};

use crate::batch::{BatchOpts, BatchStep};
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::state::TextEdit;
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

/// Refuses and checks that nothing changed.
fn refuse_clean(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = refuse(s, id, args);
    assert_eq!(s.fingerprint(), before, "{id} refused but changed state: {}", r.message);
    r
}

struct Fx {
    s: Session,
    rect: ElementId,
    ellipse: ElementId,
    text: ElementId,
    boxed: ElementId,
    label: ElementId,
    line: ElementId,
    closed: ElementId,
    freehand: ElementId,
    math: ElementId,
}

/// Adds an element like `testing::add`, but leaves the edit in the journal so that `settle` can sync it.
fn put(s: &mut Session, kind: ElementKind, x: f64, y: f64, w: f64, h: f64) -> ElementId {
    use drafftink_scene::{Element, SceneTx, StoreError, Transform};
    let id = s.ports.ids.element_id();
    let ret = id.clone();
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            let z = tx.next_top_index().map_err(StoreError::backend)?;
            let mut e = Element::new(id, kind, z);
            e.transform = Transform::new(x, y, w, h);
            tx.insert(e)
        })
        .unwrap();
    ret
}

fn fixture() -> Fx {
    let mut s = testing::headless_session();
    let mark = s.tab().doc.mark();
    let rect = put(&mut s, ElementKind::Rectangle, 0.0, 0.0, 100.0, 60.0);
    let ellipse = put(&mut s, ElementKind::Ellipse, 200.0, 0.0, 80.0, 80.0);
    let text = put(&mut s, ElementKind::Text(TextData::new("hello")), 0.0, 200.0, 60.0, 25.0);
    let boxed = put(&mut s, ElementKind::Rectangle, 300.0, 300.0, 120.0, 80.0);
    let mut td = TextData::new("label");
    td.container_id = Some(boxed.clone());
    let label = put(&mut s, ElementKind::Text(td), 310.0, 320.0, 50.0, 25.0);
    let line = put(
        &mut s,
        ElementKind::Line(LinearData::new(vec![Point::new(0.0, 0.0), Point::new(40.0, 30.0)])),
        0.0,
        400.0,
        40.0,
        30.0,
    );
    let mut cd = LinearData::new(vec![Point::new(0.0, 0.0), Point::new(40.0, 0.0), Point::new(0.0, 30.0)]);
    cd.closed = true;
    let closed = put(&mut s, ElementKind::Line(cd), 100.0, 400.0, 40.0, 30.0);
    let freehand = put(&mut s, ElementKind::Freehand(FreehandData::new(vec![Point::ZERO])), 500.0, 0.0, 0.0, 0.0);
    let math = put(&mut s, ElementKind::Math(MathData::new("x^2")), 500.0, 100.0, 40.0, 20.0);
    // The fixture is the document's starting point: in the replicated store, but not an undo step.
    let o = crate::state::Observed::default();
    let meta = crate::store::StepMeta { label: "Fixture", undoable: false, before: &o, after: &o, depth: 50, coalesce: None };
    s.tab_mut().doc.commit_since(mark, &meta).unwrap();
    s.tab_mut().doc.forget_journal();
    testing::select(&mut s, std::slice::from_ref(&rect));
    Fx { s, rect, ellipse, text, boxed, label, line, closed, freehand, math }
}

fn el(s: &Session, id: &ElementId) -> drafftink_scene::Element {
    s.store().get(id).unwrap().clone()
}

fn font(s: &Session, id: &ElementId) -> drafftink_scene::TextStyle {
    match el(s, id).kind {
        ElementKind::Text(t) => t.font,
        other => panic!("not text: {other:?}"),
    }
}

// ---- availability ----

#[test]
fn style_commands_are_available_and_set_refuses_where_the_catalog_says() {
    let mut f = fixture();
    for id in ["style.set", "style.get", "style.apply_theme", "style.list_themes"] {
        assert!(f.s.availability(id).is_ok(), "{id}");
    }
    // View-only: writes refuse, reads run.
    f.s.view_only = true;
    for id in ["style.set", "style.apply_theme"] {
        let r = f.s.availability(id).unwrap_err();
        assert_eq!(r.code, "view_only", "{id}");
        assert!(r.message.contains(&format!("`{id}`")), "{}", r.message);
    }
    for id in ["style.get", "style.list_themes"] {
        assert!(f.s.availability(id).is_ok(), "{id}");
    }
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"opacity": 0.5}}));
    assert_eq!(r.code, "view_only");
    f.s.view_only = false;
}

#[test]
fn set_and_theme_refuse_during_a_text_edit_with_the_catalog_text() {
    let mut f = fixture();
    f.s.text_edit = Some(TextEdit { element: Some(f.text.clone()), draft: false });
    let r = f.s.availability("style.set").unwrap_err();
    assert_eq!(r.code, "text_editing");
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"opacity": 0.5}}));
    assert_eq!(r.message, "A text element is being edited. Finish with `text.edit_end` before running `style.set`.");
    assert_eq!(refuse_clean(&mut f.s, "style.apply_theme", json!({"name": "pastel"})).code, "text_editing");
    // Reading is fine while editing.
    run(&mut f.s, "style.get", json!({}));
}

// ---- style.set: effect ----

#[test]
fn set_changes_the_selection_and_the_defaults_in_one_undo_step() {
    let mut f = fixture();
    let out = run(
        &mut f.s,
        "style.set",
        json!({"props": {"stroke_color": "blue-500", "fill_color": "#fef08a", "stroke_width": 4, "fill_pattern": "hachure"}}),
    );
    assert_eq!(out["changed"], true);
    assert_eq!(out["defaults_updated"], true);
    assert_eq!(out["applied"][f.rect.as_str()], json!(["stroke_color", "fill_color", "fill_pattern", "stroke_width"]));
    assert_eq!(out["skipped"], json!([]));
    assert_eq!(out["undo_label"], "Set style");
    let r = el(&f.s, &f.rect);
    assert_eq!(r.style.stroke_color, Color::rgb(59, 130, 246));
    assert_eq!(r.style.fill_color, Some(Color::rgb(0xfe, 0xf0, 0x8a)));
    assert_eq!((r.style.stroke_width, f.s.tab().doc.undo_depth()), (4.0, 1));
    // The defaults followed, and so did the persisted setting.
    assert_eq!(f.s.tool.style_defaults.stroke_color, Color::rgb(59, 130, 246));
    assert_eq!(f.s.tool.style_defaults.stroke_width, 4.0);
    assert_eq!(f.s.settings.get("style.defaults").unwrap()["stroke_color"], "#3b82f6");
    // Other elements are untouched.
    assert_eq!(el(&f.s, &f.ellipse).style.stroke_width, 2.0);
    // Undo takes back the element, as one step.
    f.s.undo_steps(1).unwrap();
    let r = el(&f.s, &f.rect);
    assert_eq!((r.style.stroke_width, r.style.fill_color), (2.0, None));
}

#[test]
fn set_with_nothing_selected_edits_only_the_defaults() {
    let mut f = fixture();
    testing::select(&mut f.s, &[]);
    let rev = f.s.tab().doc.revision();
    let out = run(
        &mut f.s,
        "style.set",
        json!({"props": {"font_size": 28, "sloppiness": "architect", "font_family": "vanilla_extract"}}),
    );
    assert_eq!(out["changed"], true);
    assert_eq!(out["applied"], json!({}));
    assert_eq!(out["defaults_updated"], true);
    assert_eq!(f.s.tab().doc.revision(), rev, "no element changed");
    assert_eq!(f.s.tab().doc.undo_depth(), 0, "the defaults are not in undo history");
    assert!(out.get("undo_label").is_none());
    assert_eq!(f.s.tool.text_defaults.size, 28.0);
    assert_eq!(f.s.tool.text_defaults.family, drafftink_scene::FontFamily::VanillaExtract);
    assert_eq!(f.s.tool.style_defaults.sloppiness, drafftink_scene::Sloppiness::Architect);
    // The same call again finds the state as requested.
    let again = run(&mut f.s, "style.set", json!({"props": {"font_size": 28}}));
    assert_eq!(again["changed"], false);
}

#[test]
fn set_scope_chooses_where_to_write() {
    let mut f = fixture();
    let d0 = f.s.tool.style_defaults.clone();
    let out = run(&mut f.s, "style.set", json!({"props": {"opacity": 0.5}, "scope": "selection"}));
    assert_eq!(out["defaults_updated"], false);
    assert_eq!(f.s.tool.style_defaults, d0);
    assert_eq!(el(&f.s, &f.rect).style.opacity, 0.5);

    let out = run(&mut f.s, "style.set", json!({"props": {"opacity": 0.25}, "scope": "defaults"}));
    assert_eq!((out["defaults_updated"].as_bool(), out["changed"].as_bool()), (Some(true), Some(true)));
    assert_eq!(f.s.tool.style_defaults.opacity, 0.25);
    assert_eq!(el(&f.s, &f.rect).style.opacity, 0.5, "scope defaults leaves elements alone");

    run(&mut f.s, "style.set", json!({"props": {"opacity": 0.75}, "scope": "both"}));
    assert_eq!((el(&f.s, &f.rect).style.opacity, f.s.tool.style_defaults.opacity), (0.75, 0.75));

    // ids replace the selection.
    run(
        &mut f.s,
        "style.set",
        json!({"props": {"stroke_width": 8}, "ids": [f.ellipse.as_str()], "scope": "selection"}),
    );
    assert_eq!(el(&f.s, &f.ellipse).style.stroke_width, 8.0);
    assert_eq!(el(&f.s, &f.rect).style.stroke_width, 2.0);
}

#[test]
fn set_addresses_elements_by_name() {
    let mut f = fixture();
    let ell = f.ellipse.clone();
    f.s.tab_mut()
        .doc
        .edit::<(), drafftink_scene::StoreError>(|tx| {
            use drafftink_scene::SceneTx;
            tx.update(&ell, |e| e.name = Some("sun".to_owned()))
        })
        .unwrap();
    run(&mut f.s, "style.set", json!({"props": {"fill_color": "amber-400"}, "ids": ["@sun"], "scope": "selection"}));
    assert_eq!(el(&f.s, &f.ellipse).style.fill_color, Some(Color::rgb(251, 191, 36)));
}

// ---- style.set: applicability ----

#[test]
fn unsupported_props_are_skipped_and_reported_per_element() {
    let mut f = fixture();
    testing::select(&mut f.s, &[f.rect.clone(), f.ellipse.clone(), f.text.clone()]);
    let out = run(
        &mut f.s,
        "style.set",
        json!({"props": {"corner_radius": 12, "stroke_width": 6, "opacity": 0.5}, "scope": "selection"}),
    );
    assert_eq!(out["applied"][f.rect.as_str()], json!(["stroke_width", "corner_radius", "opacity"]));
    assert_eq!(out["applied"][f.ellipse.as_str()], json!(["stroke_width", "opacity"]));
    assert_eq!(out["applied"][f.text.as_str()], json!(["opacity"]));
    let mut skipped: Vec<(String, Value)> = out["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["id"].as_str().unwrap().to_owned(), e["props"].clone()))
        .collect();
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    let mut want = vec![
        (f.ellipse.as_str().to_owned(), json!(["corner_radius"])),
        (f.text.as_str().to_owned(), json!(["stroke_width", "corner_radius"])),
    ];
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(skipped, want);
    assert_eq!(el(&f.s, &f.ellipse).style.corner_radius, 0.0);
    assert_eq!(el(&f.s, &f.text).style.stroke_width, 2.0);
}

#[test]
fn nothing_applicable_refuses_with_the_kinds_that_would_work() {
    let mut f = fixture();
    testing::select(&mut f.s, &[f.rect.clone()]);
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"font_size": 30}}));
    assert_eq!(r.code, "not_applicable");
    assert_eq!(
        r.message,
        "None of the targeted elements support `font_size`. They apply to text and math, and shapes and arrows that have a label."
    );
    // Defaults were not touched by the refused call either (the fingerprint covers the tool state).
    let r = refuse_clean(
        &mut f.s,
        "style.set",
        json!({"props": {"corner_radius": 4, "stroke_width": 3}, "ids": [f.text.as_str()]}),
    );
    assert_eq!(r.code, "not_applicable");
    assert!(r.message.contains("`corner_radius`") && r.message.contains("`stroke_width`"), "{}", r.message);
}

#[test]
fn applicability_of_fill_pattern_stroke_style_and_freehand_props() {
    let mut f = fixture();
    let all = [f.rect.clone(), f.line.clone(), f.closed.clone(), f.freehand.clone(), f.math.clone(), f.text.clone()];
    testing::select(&mut f.s, &all);
    let out = run(
        &mut f.s,
        "style.set",
        json!({"scope": "selection", "props": {"fill_color": "red-100", "stroke_style": "dotted", "freehand_style": "calligraphy", "pressure_mode": "device"}}),
    );
    let applied = |id: &ElementId| out["applied"][id.as_str()].clone();
    assert_eq!(applied(&f.rect), json!(["fill_color", "stroke_style"]));
    assert_eq!(applied(&f.closed), json!(["fill_color", "stroke_style"]));
    assert_eq!(applied(&f.line), json!(["stroke_style"]), "an open line has no fill");
    assert_eq!(applied(&f.freehand), json!(["fill_color", "stroke_style", "freehand_style", "pressure_mode"]));
    assert_eq!(applied(&f.math), Value::Null, "math supports none of them");
    match el(&f.s, &f.freehand).kind {
        ElementKind::Freehand(d) => {
            assert_eq!(
                (d.brush, d.pressure_mode),
                (drafftink_scene::Brush::Calligraphy, drafftink_scene::PressureMode::Device)
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(el(&f.s, &f.closed).style.stroke_style, drafftink_scene::StrokeStyle::Dotted);
}

#[test]
fn all_seven_fill_patterns_are_accepted() {
    let mut f = fixture();
    for (name, pattern) in [
        ("solid", drafftink_scene::FillPattern::Solid),
        ("hachure", drafftink_scene::FillPattern::Hachure),
        ("zigzag", drafftink_scene::FillPattern::ZigZag),
        ("cross_hatch", drafftink_scene::FillPattern::CrossHatch),
        ("dots", drafftink_scene::FillPattern::Dots),
        ("dashed", drafftink_scene::FillPattern::Dashed),
        ("zigzag_line", drafftink_scene::FillPattern::ZigZagLine),
    ] {
        run(&mut f.s, "style.set", json!({"props": {"fill_pattern": name}}));
        assert_eq!(el(&f.s, &f.rect).style.fill_pattern, pattern, "{name}");
    }
}

#[test]
fn null_fill_removes_the_fill_and_absent_fill_keeps_it() {
    let mut f = fixture();
    run(&mut f.s, "style.set", json!({"props": {"fill_color": "green-200"}}));
    run(&mut f.s, "style.set", json!({"props": {"opacity": 0.9}}));
    assert!(el(&f.s, &f.rect).style.fill_color.is_some());
    let out = run(&mut f.s, "style.set", json!({"props": {"fill_color": null}}));
    assert_eq!(out["changed"], true);
    assert_eq!(el(&f.s, &f.rect).style.fill_color, None);
    assert_eq!(f.s.tool.style_defaults.fill_color, None);
    assert_eq!(f.s.settings.get("style.defaults").unwrap()["fill_color"], Value::Null);
}

#[test]
fn text_props_apply_to_text_and_reach_the_label_of_a_container() {
    let mut f = fixture();
    testing::select(&mut f.s, &[f.text.clone(), f.boxed.clone()]);
    let out = run(
        &mut f.s,
        "style.set",
        json!({"scope": "selection", "props": {"font_size": 36, "font_weight": "heavy", "text_align": "center", "line_height": 1.5, "vertical_align": "middle"}}),
    );
    let t = font(&f.s, &f.text);
    assert_eq!(
        (t.size, t.weight, t.align, t.line_height),
        (36.0, drafftink_scene::FontWeight::Heavy, drafftink_scene::TextAlign::Center, 1.5)
    );
    assert_eq!(t.vertical_align, drafftink_scene::VerticalAlign::Top, "free text has no container to align in");
    let l = font(&f.s, &f.label);
    assert_eq!((l.size, l.vertical_align), (36.0, drafftink_scene::VerticalAlign::Middle));
    assert_eq!(
        out["applied"][f.label.as_str()],
        json!(["font_weight", "font_size", "text_align", "vertical_align", "line_height"])
    );
    assert_eq!(out["applied"][f.text.as_str()], json!(["font_weight", "font_size", "text_align", "line_height"]));
    assert_eq!(out["applied"].get(f.boxed.as_str()), None, "the container took nothing itself");
    assert_eq!(out["skipped"], json!([{"id": f.text.as_str(), "props": ["vertical_align"]}]));
}

#[test]
fn include_bound_text_false_keeps_the_label_out() {
    let mut f = fixture();
    testing::select(&mut f.s, &[f.boxed.clone()]);
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"font_size": 40}, "include_bound_text": false}));
    assert_eq!(r.code, "not_applicable");
    assert_eq!(font(&f.s, &f.label).size, 20.0);
    run(&mut f.s, "style.set", json!({"props": {"font_size": 40, "opacity": 0.5}, "include_bound_text": false}));
    assert_eq!(font(&f.s, &f.label).size, 20.0);
    assert_eq!(el(&f.s, &f.boxed).style.opacity, 0.5);
}

#[test]
fn math_takes_font_size_only() {
    let mut f = fixture();
    testing::select(&mut f.s, &[f.math.clone()]);
    let out = run(
        &mut f.s,
        "style.set",
        json!({"props": {"font_size": 36, "stroke_color": "#ff0000"}, "scope": "selection"}),
    );
    assert_eq!(out["applied"][f.math.as_str()], json!(["stroke_color", "font_size"]));
    match el(&f.s, &f.math).kind {
        ElementKind::Math(m) => assert_eq!(m.font_size, 36.0),
        other => panic!("{other:?}"),
    }
    let r = refuse(&mut f.s, "style.set", json!({"props": {"font_family": "noto_sans"}, "scope": "selection"}));
    assert_eq!(r.code, "not_applicable");
}

#[test]
fn plex_sans_is_a_font_family_for_text_and_the_label_of_a_container() {
    let mut f = fixture();
    testing::select(&mut f.s, &[f.text.clone(), f.boxed.clone()]);
    let out = run(&mut f.s, "style.set", json!({"scope": "selection", "props": {"font_family": "plex_sans"}}));
    assert_eq!(font(&f.s, &f.text).family, drafftink_scene::FontFamily::PlexSans);
    assert_eq!(font(&f.s, &f.label).family, drafftink_scene::FontFamily::PlexSans, "the label of the container too");
    assert_eq!(out["applied"][f.text.as_str()], json!(["font_family"]));
    // The next text is made in it, and a bad word names the new one among the accepted.
    run(&mut f.s, "style.set", json!({"props": {"font_family": "plex_sans"}}));
    assert_eq!(f.s.tool.text_defaults.family, drafftink_scene::FontFamily::PlexSans);
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"font_family": "plex"}}));
    assert!(r.message.contains("`plex_sans`"), "{}", r.message);
}

// ---- style.set: refusals ----

#[test]
fn bad_values_name_the_prop_and_what_is_accepted() {
    let mut f = fixture();
    let cases = [
        (json!({"stroke_width": 0}), "`props.stroke_width`", "above 0"),
        (json!({"stroke_width": 100000}), "`props.stroke_width`", "at most 1000"),
        (json!({"opacity": 1.5}), "`props.opacity`", "from 0 to 1"),
        (json!({"corner_radius": -2}), "`props.corner_radius`", "from 0"),
        (json!({"font_size": 0}), "`props.font_size`", "above 0"),
        (json!({"line_height": 3.5}), "`props.line_height`", "from 1 to 3"),
        (json!({"fill_pattern": "wavy"}), "props.fill_pattern", "`solid`"),
        (json!({"sloppiness": "wild"}), "props.sloppiness", "architect"),
        (json!({"stroke_color": "mauve-500"}), "props.stroke_color", "blue-500"),
        (json!({"stroke_color": "#12"}), "props.stroke_color", "not a colour"),
        (json!({"font_family": "comic_sans"}), "props.font_family", "gelpen"),
        (json!({"stroke_width": "thick"}), "props.stroke_width", "invalid type"),
        (json!({"strok_color": "#fff"}), "props", "unknown field `strok_color`"),
    ];
    for (props, key, hint) in cases {
        let r = refuse_clean(&mut f.s, "style.set", json!({"props": props}));
        assert_eq!(r.code, "bad_args", "{props}");
        assert!(r.message.contains(key) && r.message.contains(hint), "{props}: {}", r.message);
    }
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {}}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("give at least one style property"), "{}", r.message);
    assert_eq!(refuse_clean(&mut f.s, "style.set", json!({})).code, "bad_args");
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"opacity": 1}, "scope": "everywhere"}));
    assert!(r.message.contains("`scope`") || r.message.contains("everywhere"), "{}", r.message);
}

#[test]
fn explicit_targets_and_scopes_refuse_with_the_shared_texts() {
    let mut f = fixture();
    testing::select(&mut f.s, &[]);
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"opacity": 0.5}, "scope": "selection"}));
    assert_eq!(r.code, "no_target");
    assert_eq!(r.message, "Nothing to act on: no elements are selected. Select elements first, or pass `ids`.");
    assert_eq!(
        refuse_clean(&mut f.s, "style.set", json!({"props": {"opacity": 0.5}, "scope": "both"})).code,
        "no_target"
    );
    // An explicit empty list is "resolves to nothing", not "use the defaults".
    assert_eq!(refuse_clean(&mut f.s, "style.set", json!({"props": {"opacity": 0.5}, "ids": []})).code, "no_target");
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"opacity": 0.5}, "ids": ["nope"]}));
    assert_eq!(r.code, "not_found");
    let r = refuse_clean(
        &mut f.s,
        "style.set",
        json!({"props": {"opacity": 0.5}, "ids": [f.rect.as_str()], "scope": "defaults"}),
    );
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`ids`") && r.message.contains("scope \"defaults\""), "{}", r.message);
}

#[test]
fn a_locked_element_refuses_the_whole_call_before_anything_changes() {
    let mut f = fixture();
    let b = f.ellipse.clone();
    f.s.tab_mut()
        .doc
        .edit::<(), drafftink_scene::StoreError>(|tx| {
            use drafftink_scene::SceneTx;
            tx.update(&b, |e| e.locked = true)
        })
        .unwrap();
    f.s.tab_mut().doc.forget_journal();
    let r = refuse_clean(
        &mut f.s,
        "style.set",
        json!({"props": {"opacity": 0.1}, "ids": [f.rect.as_str(), f.ellipse.as_str()]}),
    );
    assert_eq!(r.code, "locked");
    assert_eq!(
        r.message,
        format!(
            "Element `{}` is locked. Unlock it first with `element.lock` (locked=false) or `element.unlock_all`.",
            f.ellipse
        )
    );
    assert_eq!(el(&f.s, &f.rect).style.opacity, 1.0);
    assert_eq!(f.s.tool.style_defaults.opacity, 1.0);
    // The label of a locked container's sibling is not special: a locked label refuses too.
    let l = f.label.clone();
    f.s.tab_mut()
        .doc
        .edit::<(), drafftink_scene::StoreError>(|tx| {
            use drafftink_scene::SceneTx;
            tx.update(&l, |e| e.locked = true)
        })
        .unwrap();
    let r = refuse_clean(&mut f.s, "style.set", json!({"props": {"font_size": 30}, "ids": [f.boxed.as_str()]}));
    assert_eq!(r.code, "locked");
}

// ---- gesture, batch, history ----

#[test]
fn set_is_a_gesture_command_with_coalescing_declared() {
    let f = fixture();
    let spec = f.s.spec("style.set").unwrap();
    assert!(spec.is_gesture());
    let c = spec.coalesce.unwrap();
    assert_eq!((c.key, c.ms), ("style.set", 1000));
}

#[test]
fn scrubbing_opacity_is_one_gesture_and_one_undo_step() {
    let mut f = fixture();
    let begin = run(&mut f.s, "gesture.begin", json!({"command": "style.set", "args": {"props": {"opacity": 0.9}}}));
    let g = begin["gesture"].as_str().unwrap().to_owned();
    for o in [0.8, 0.6, 0.4, 0.2] {
        run(&mut f.s, "gesture.update", json!({"gesture": g, "args": {"props": {"opacity": o}}}));
        assert_eq!(el(&f.s, &f.rect).style.opacity, o);
        assert_eq!(f.s.tool.style_defaults.opacity, o, "the defaults follow the frame");
    }
    // Live frames do not persist the setting.
    assert_eq!(f.s.settings.get("style.defaults").unwrap()["opacity"], 1.0);
    assert_eq!(f.s.tab().doc.undo_depth(), 0);
    let end = run(&mut f.s, "gesture.end", json!({"gesture": g}));
    assert_eq!(end["changed"], true);
    assert_eq!(f.s.tab().doc.undo_depth(), 1);
    assert_eq!(el(&f.s, &f.rect).style.opacity, 0.2);
    assert_eq!(f.s.settings.get("style.defaults").unwrap()["opacity"], 0.2);
    f.s.undo_steps(1).unwrap();
    assert_eq!(el(&f.s, &f.rect).style.opacity, 1.0);
}

/// docs/exceptions.md E13: a scrub (`gesture.begin`, `update`, `end`) ends in the same document, the same
/// defaults and the same persisted setting as one plain `style.set` with the final props.
#[test]
#[allow(non_snake_case)]
fn exception_E13_scrub_equals_plain_command() {
    for (first, last) in [
        (json!({"opacity": 0.9}), json!({"opacity": 0.2})),
        (json!({"stroke_width": 3}), json!({"stroke_width": 8, "stroke_color": "teal-500"})),
    ] {
        let mut scrubbed = fixture();
        let begin = run(&mut scrubbed.s, "gesture.begin", json!({"command": "style.set", "args": {"props": first}}));
        let g = begin["gesture"].as_str().unwrap().to_owned();
        run(&mut scrubbed.s, "gesture.update", json!({"gesture": g, "args": {"props": last}}));
        run(&mut scrubbed.s, "gesture.end", json!({"gesture": g}));

        let mut plain = fixture();
        run(&mut plain.s, "style.set", json!({"props": last}));

        assert_eq!(el(&scrubbed.s, &scrubbed.rect).style, el(&plain.s, &plain.rect).style, "{last}");
        assert_eq!(scrubbed.s.tool.style_defaults, plain.s.tool.style_defaults, "{last}");
        assert_eq!(scrubbed.s.settings.get("style.defaults"), plain.s.settings.get("style.defaults"), "{last}");
        assert_eq!(scrubbed.s.fingerprint(), plain.s.fingerprint(), "{last}");
        assert_eq!(scrubbed.s.tab().doc.undo_depth(), plain.s.tab().doc.undo_depth(), "one undo step either way");
    }
}

#[test]
fn cancelling_the_gesture_restores_the_element_the_defaults_and_the_setting() {
    let mut f = fixture();
    let before = f.s.fingerprint();
    let begin =
        run(&mut f.s, "gesture.begin", json!({"command": "style.set", "args": {"props": {"stroke_color": "red-500"}}}));
    let g = begin["gesture"].as_str().unwrap().to_owned();
    run(&mut f.s, "gesture.update", json!({"gesture": g, "args": {"props": {"stroke_color": "teal-500"}}}));
    assert_eq!(el(&f.s, &f.rect).style.stroke_color, Color::rgb(20, 184, 166));
    run(&mut f.s, "gesture.cancel", json!({"gesture": g}));
    assert_eq!(f.s.fingerprint(), before);
    assert_eq!(f.s.settings.get("style.defaults").unwrap()["stroke_color"], "#6366f1");
    // A style command during a gesture is refused.
    run(&mut f.s, "gesture.begin", json!({"command": "style.set", "args": {"props": {"opacity": 0.5}}}));
    let r = refuse(&mut f.s, "style.set", json!({"props": {"opacity": 0.1}}));
    assert_eq!(r.code, "gesture_active");
    run(&mut f.s, "gesture.cancel", json!({}));
}

#[test]
fn a_failing_step_rolls_back_the_earlier_style_step_and_the_defaults() {
    let mut f = fixture();
    let before = f.s.fingerprint();
    let steps = [
        BatchStep::new("style.set", json!({"props": {"fill_color": "pink-200", "stroke_width": 8}})),
        BatchStep::new("style.set", json!({"props": {"font_size": 30}, "scope": "selection"})),
    ];
    let out = f.s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    let failed = out.failed.unwrap();
    assert_eq!((failed.index, failed.refusal.code), (1, "not_applicable"));
    assert!(out.rolled_back);
    assert_eq!(el(&f.s, &f.rect).style.stroke_width, 2.0);
    assert_eq!(f.s.tool.style_defaults.stroke_width, 2.0);
    assert_eq!(f.s.tab().doc.undo_depth(), 0);
    assert_eq!(f.s.fingerprint().lines().next(), before.lines().next());
}

#[test]
fn a_batch_of_style_steps_is_one_undo_step() {
    let mut f = fixture();
    let steps = [
        BatchStep::new("style.set", json!({"props": {"stroke_width": 8}})),
        BatchStep::new(
            "style.set",
            json!({"props": {"opacity": 0.5}, "ids": [f.ellipse.as_str()], "scope": "selection"}),
        ),
    ];
    let out = f.s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.failed.is_none());
    assert_eq!(f.s.tab().doc.undo_depth(), 1);
    f.s.undo_steps(1).unwrap();
    assert_eq!((el(&f.s, &f.rect).style.stroke_width, el(&f.s, &f.ellipse).style.opacity), (2.0, 1.0));
}

// ---- style.get ----

#[test]
fn get_reports_values_applicability_and_kinds_for_one_element() {
    let mut f = fixture();
    let out = run(&mut f.s, "style.get", json!({}));
    assert_eq!(out["changed"], false);
    assert_eq!(out["source"], "selection");
    assert_eq!(out["ids"], json!([f.rect.as_str()]));
    assert_eq!(out["kinds"], json!(["rectangle"]));
    assert_eq!(out["props"]["stroke_color"], json!({"applicable": true, "value": "#6366f1"}));
    assert_eq!(out["props"]["fill_color"], json!({"applicable": true, "value": null}));
    assert_eq!(out["props"]["corner_radius"], json!({"applicable": true, "value": 0.0}));
    // A property that no target supports carries the words `style.set` refuses with for it, so that a control can
    // grey itself with the engine's text.
    let said = refuse(&mut f.s, "style.set", json!({"props": {"font_size": 20}, "scope": "selection"}));
    assert_eq!(out["props"]["font_size"], json!({"applicable": false, "reason": said.message}));
    assert_eq!(out["props"]["sloppiness"]["value"], "artist");
    assert_eq!(out["props"].as_object().unwrap().len(), 16);
}

#[test]
fn get_shows_mixed_for_differing_values_and_only_counts_elements_that_support_the_prop() {
    let mut f = fixture();
    run(&mut f.s, "style.set", json!({"props": {"stroke_width": 6}, "scope": "selection"}));
    testing::select(&mut f.s, &[f.rect.clone(), f.ellipse.clone(), f.text.clone()]);
    let out =
        run(&mut f.s, "style.get", json!({"props": ["stroke_width", "stroke_color", "font_size", "corner_radius"]}));
    assert_eq!(out["props"].as_object().unwrap().len(), 4);
    assert_eq!(out["props"]["stroke_width"], json!({"applicable": true, "value": "mixed"}));
    assert_eq!(out["props"]["stroke_color"]["value"], "#6366f1", "all three agree");
    assert_eq!(out["props"]["font_size"], json!({"applicable": true, "value": 20.0}), "only the text supports it");
    assert_eq!(
        out["props"]["corner_radius"]["value"], 0.0,
        "the ellipse and the text cannot have one, so only the rectangle counts"
    );
    assert_eq!(out["kinds"], json!(["rectangle", "ellipse", "text"]));
}

#[test]
fn get_corner_radius_ignores_elements_that_cannot_have_one() {
    let mut f = fixture();
    testing::select(&mut f.s, &[f.rect.clone(), f.ellipse.clone()]);
    let out = run(&mut f.s, "style.get", json!({"props": ["corner_radius"]}));
    assert_eq!(out["props"]["corner_radius"], json!({"applicable": true, "value": 0.0}));
}

#[test]
fn get_reports_the_heads_of_the_selected_lines_and_arrows() {
    let mut f = fixture();
    // No line or arrow selected: no `heads` at all.
    assert!(run(&mut f.s, "style.get", json!({})).get("heads").is_none());
    let mut a = LinearData::new(vec![Point::new(0.0, 0.0), Point::new(40.0, 0.0)]);
    a.start_head = drafftink_scene::Head::Circle;
    a.end_head = drafftink_scene::Head::Triangle;
    let arrow = put(&mut f.s, ElementKind::Arrow(a), 0.0, 500.0, 40.0, 0.0);
    testing::select(&mut f.s, std::slice::from_ref(&arrow));
    let out = run(&mut f.s, "style.get", json!({}));
    assert_eq!(out["heads"], json!({"start": "circle", "end": "triangle"}));
    // A line with no heads next to it: the ends disagree.
    testing::select(&mut f.s, &[arrow.clone(), f.line.clone()]);
    let out = run(&mut f.s, "style.get", json!({"props": ["stroke_color"]}));
    assert_eq!(out["heads"], json!({"start": "mixed", "end": "mixed"}));
    // Two arrows that agree on one end.
    let mut b = LinearData::new(vec![Point::new(0.0, 0.0), Point::new(40.0, 0.0)]);
    b.start_head = drafftink_scene::Head::Circle;
    let second = put(&mut f.s, ElementKind::Arrow(b), 0.0, 600.0, 40.0, 0.0);
    testing::select(&mut f.s, &[arrow.clone(), second]);
    let out = run(&mut f.s, "style.get", json!({}));
    assert_eq!(out["heads"], json!({"start": "circle", "end": "mixed"}));
    // The defaults have none.
    testing::select(&mut f.s, &[]);
    assert!(run(&mut f.s, "style.get", json!({})).get("heads").is_none());
}

#[test]
fn get_reads_a_label_through_its_container_and_the_defaults_when_nothing_is_selected() {
    let mut f = fixture();
    run(&mut f.s, "style.set", json!({"props": {"font_size": 33}, "ids": [f.label.as_str()], "scope": "selection"}));
    let out = run(&mut f.s, "style.get", json!({"ids": [f.boxed.as_str()], "props": ["font_size", "vertical_align"]}));
    assert_eq!(out["props"]["font_size"]["value"], 33.0);
    assert_eq!(out["props"]["vertical_align"]["applicable"], true);

    testing::select(&mut f.s, &[]);
    let out = run(&mut f.s, "style.get", json!({}));
    assert_eq!((out["source"].as_str(), out["ids"].clone()), (Some("defaults"), json!([])));
    assert_eq!(out["props"]["font_size"], json!({"applicable": true, "value": 20.0}));
    assert_eq!(out["props"]["stroke_color"]["value"], "#6366f1");
    let out = run(&mut f.s, "style.get", json!({"scope": "defaults", "props": ["text_align"]}));
    assert_eq!(out["props"]["text_align"]["value"], "left");
}

#[test]
fn get_refusals() {
    let mut f = fixture();
    testing::select(&mut f.s, &[]);
    assert_eq!(refuse_clean(&mut f.s, "style.get", json!({"scope": "selection"})).code, "no_target");
    assert_eq!(refuse_clean(&mut f.s, "style.get", json!({"ids": ["nope"]})).code, "not_found");
    let r = refuse_clean(&mut f.s, "style.get", json!({"props": ["colour"]}));
    assert_eq!(r.code, "bad_args");
    assert!(
        r.message.contains("unknown style property `colour`") && r.message.contains("stroke_color"),
        "{}",
        r.message
    );
    assert_eq!(refuse_clean(&mut f.s, "style.get", json!({"scope": "defaults", "ids": ["x"]})).code, "bad_args");
}

#[test]
fn get_follows_a_set_and_ignores_view_only_and_gestures() {
    let mut f = fixture();
    run(&mut f.s, "style.set", json!({"props": {"fill_color": "sky-300", "sloppiness": "drunk"}}));
    f.s.view_only = true;
    let out = run(&mut f.s, "style.get", json!({"props": ["fill_color", "sloppiness"]}));
    assert_eq!(out["props"]["fill_color"]["value"], "#7dd3fc");
    assert_eq!(out["props"]["sloppiness"]["value"], "drunk");
    assert_eq!(f.s.tab().doc.undo_depth(), 1, "reading adds no step");
}

// ---- themes ----

#[test]
fn list_themes_returns_the_catalogued_bundles() {
    let mut f = fixture();
    let out = run(&mut f.s, "style.list_themes", json!({}));
    let themes = out["themes"].as_array().unwrap();
    let names: Vec<&str> = themes.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["default", "monochrome", "pastel", "blueprint", "high_contrast"]);
    let by = |n: &str| themes.iter().find(|t| t["name"] == n).unwrap().clone();
    assert_eq!(
        by("default"),
        json!({"name": "default", "background": "#ffffff", "props": {
            "stroke_color": "#6366f1", "fill_color": null, "fill_pattern": "solid", "stroke_width": 2.0, "stroke_style": "solid",
            "sloppiness": "artist", "corner_radius": 0.0, "opacity": 1.0, "font_family": "gelpen", "font_weight": "regular",
            "font_size": 20.0, "text_align": "left"}})
    );
    assert_eq!(
        by("monochrome"),
        json!({"name": "monochrome", "background": "#ffffff", "props": {
            "stroke_color": "#000000", "fill_color": null, "fill_pattern": "solid", "stroke_width": 2.0, "sloppiness": "architect"}})
    );
    assert_eq!(
        by("pastel"),
        json!({"name": "pastel", "background": "#fafafa", "props": {
            "stroke_color": "#334155", "fill_color": "#dbeafe", "fill_pattern": "solid", "stroke_width": 2.0, "sloppiness": "artist"}})
    );
    assert_eq!(
        by("blueprint"),
        json!({"name": "blueprint", "background": "#1e3a8a", "props": {
            "stroke_color": "#ffffff", "fill_color": null, "stroke_width": 2.0, "sloppiness": "architect"}})
    );
    assert_eq!(
        by("high_contrast"),
        json!({"name": "high_contrast", "background": "#ffffff", "props": {
            "stroke_color": "#000000", "fill_color": "#fef08a", "fill_pattern": "solid", "stroke_width": 4.0, "sloppiness": "architect"}})
    );
    // Every bundle is a valid `style.set` argument.
    for t in themes {
        let p: StyleProps = serde_json::from_value(t["props"].clone()).unwrap();
        p.validate().unwrap();
        assert_eq!(theme(t["name"].as_str().unwrap()).unwrap().props, p);
    }
}

#[test]
fn apply_theme_styles_the_selection_and_the_defaults_but_not_the_page_by_default() {
    let mut f = fixture();
    let bg = f.s.store().meta().background;
    testing::select(&mut f.s, &[f.rect.clone(), f.text.clone()]);
    let out = run(&mut f.s, "style.apply_theme", json!({"name": "pastel"}));
    assert_eq!(
        (out["theme"].as_str(), out["changed"].as_bool(), out["background_set"].as_bool()),
        (Some("pastel"), Some(true), Some(false))
    );
    assert_eq!(out["undo_label"], "Apply theme");
    let r = el(&f.s, &f.rect);
    assert_eq!((r.style.stroke_color, r.style.fill_color), (Color::rgb(51, 65, 85), Some(Color::rgb(219, 234, 254))));
    assert_eq!(r.style.sloppiness, drafftink_scene::Sloppiness::Artist);
    // The text takes the colour only; fill, width and sloppiness are skipped for it.
    let t = el(&f.s, &f.text);
    assert_eq!((t.style.stroke_color, t.style.fill_color), (Color::rgb(51, 65, 85), None));
    assert_eq!(out["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(out["skipped"][0]["props"], json!(["fill_color", "fill_pattern", "stroke_width", "sloppiness"]));
    assert_eq!(f.s.store().meta().background, bg);
    assert_eq!(f.s.tool.style_defaults.fill_color, Some(Color::rgb(219, 234, 254)));
    assert_eq!(f.s.tab().doc.undo_depth(), 1);
    f.s.undo_steps(1).unwrap();
    assert_eq!(el(&f.s, &f.rect).style.fill_color, None);
}

#[test]
fn apply_theme_sets_the_background_only_when_asked_and_undo_restores_it() {
    let mut f = fixture();
    let bg = f.s.store().meta().background;
    testing::select(&mut f.s, &[]);
    let out = run(&mut f.s, "style.apply_theme", json!({"name": "blueprint", "set_background": true}));
    assert_eq!((out["background_set"].as_bool(), out["changed"].as_bool()), (Some(true), Some(true)));
    assert_eq!(f.s.store().meta().background, Color::rgb(0x1e, 0x3a, 0x8a));
    assert_eq!(f.s.tool.style_defaults.stroke_color, Color::WHITE);
    assert_eq!(f.s.tool.style_defaults.sloppiness, drafftink_scene::Sloppiness::Architect);
    assert_eq!(f.s.tab().doc.undo_depth(), 1, "the background is a document change");
    f.s.undo_steps(1).unwrap();
    assert_eq!(f.s.store().meta().background, bg);
    // The same theme again: nothing differs, no new step.
    run(&mut f.s, "style.apply_theme", json!({"name": "blueprint", "set_background": true}));
    let depth = f.s.tab().doc.undo_depth();
    let again = run(&mut f.s, "style.apply_theme", json!({"name": "blueprint", "set_background": true}));
    assert_eq!((again["changed"].as_bool(), f.s.tab().doc.undo_depth()), (Some(false), depth));
}

#[test]
fn every_built_in_theme_sets_its_bundle_on_a_rectangle() {
    let mut f = fixture();
    for t in ["monochrome", "pastel", "blueprint", "high_contrast", "default"] {
        run(&mut f.s, "style.apply_theme", json!({"name": t, "scope": "selection"}));
        let bundle = theme(t).unwrap().props;
        let got = drafftink_edit::style::props_of(&el(&f.s, &f.rect));
        for p in bundle.set_props() {
            match got.value(p) {
                Some(v) => assert_eq!(Some(v), bundle.value(p), "{t}: {}", p.name()),
                None => assert!(p.is_text(), "{t}: a rectangle should take {}", p.name()),
            }
        }
    }
}

#[test]
fn unknown_theme_and_target_refusals() {
    let mut f = fixture();
    let r = refuse_clean(&mut f.s, "style.apply_theme", json!({"name": "neon"}));
    assert_eq!(r.code, "unknown_theme");
    assert_eq!(
        r.message,
        "Unknown theme `neon`. Available themes: default, monochrome, pastel, blueprint, high_contrast. See them with `style.list_themes`."
    );
    assert_eq!(r.suggest[0].command, "style.list_themes");
    let r = refuse_clean(&mut f.s, "style.apply_theme", json!({"name": "x".repeat(5000)}));
    assert_eq!(r.code, "unknown_theme");
    assert!(r.message.len() < 400);
    testing::select(&mut f.s, &[]);
    assert_eq!(
        refuse_clean(&mut f.s, "style.apply_theme", json!({"name": "pastel", "scope": "selection"})).code,
        "no_target"
    );
    assert_eq!(refuse_clean(&mut f.s, "style.apply_theme", json!({"name": "pastel", "ids": []})).code, "no_target");
    // Case and separators do not matter.
    run(&mut f.s, "style.apply_theme", json!({"name": "High-Contrast"}));
    assert_eq!(f.s.tool.style_defaults.stroke_width, 4.0);
}

#[test]
fn apply_theme_to_a_locked_or_unsupporting_target_refuses_cleanly() {
    let mut f = fixture();
    let img = testing::add(
        &mut f.s,
        ElementKind::Image(drafftink_scene::ImageData {
            file_id: None,
            natural_width: 4,
            natural_height: 4,
            crop: None,
        }),
        0.0,
        600.0,
        40.0,
        40.0,
    )
    .unwrap();
    // An image has no stroke or fill: nothing in a theme applies to it except nothing at all.
    let r = refuse_clean(
        &mut f.s,
        "style.apply_theme",
        json!({"name": "pastel", "ids": [img.as_str()], "scope": "selection"}),
    );
    assert_eq!(r.code, "not_applicable");
}

#[test]
fn persisted_defaults_track_every_change_and_load_back() {
    let mut f = fixture();
    run(
        &mut f.s,
        "style.set",
        json!({"props": {"stroke_color": "rose-600", "font_size": 36, "freehand_style": "calligraphy"}, "scope": "defaults"}),
    );
    let stored: StyleProps = serde_json::from_value(f.s.settings.get("style.defaults").unwrap().clone()).unwrap();
    let loaded = drafftink_edit::style::StyleDefaults::from_props(&stored);
    assert_eq!(loaded, f.s.tool.defaults());
    assert_eq!(loaded.text.size, 36.0);
    assert_eq!(loaded.brush, drafftink_scene::Brush::Calligraphy);
}

#[test]
fn style_set_is_recorded_in_the_ring_and_a_refusal_too() {
    let mut f = fixture();
    run(&mut f.s, "style.set", json!({"props": {"opacity": 0.5}}));
    let _ = f.s.execute("style.set", json!({"props": {"opacity": 7}}));
    let mut last: Vec<_> = f.s.ring().iter().map(|e| (e.id.clone(), e.outcome.code().map(str::to_owned))).collect();
    last.reverse();
    assert_eq!(last[0], ("style.set".to_owned(), Some("bad_args".to_owned())));
    assert_eq!(last[1], ("style.set".to_owned(), None));
}
