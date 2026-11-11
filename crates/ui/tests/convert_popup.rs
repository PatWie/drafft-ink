#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The convert-type popup: Tab runs `element.convert`, and the popup then offers every type the selection can
//! become. A click converts to that type.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::{Call, UiAction};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

type H = Harness<'static, Fixture>;

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

/// A selected rectangle after a Tab press (the shortcut origin) that the popup reacts to.
fn after_tab(kind: &str) -> H {
    let mut f = dialog::fixture();
    let id = match kind {
        "rectangle" => add_rect(&mut f.session, 0.0, 0.0, 60.0, 40.0).unwrap(),
        _ => {
            let out = dialog::run(&mut f, "element.create", json!({ "kind": kind, "points": [[0, 0], [80, 30]] }));
            drafftink_scene::ElementId::new(out["id"].as_str().unwrap()).unwrap()
        }
    };
    select(&mut f.session, std::slice::from_ref(&id));
    f.refresh();
    let tab = UiAction::run_from(Origin::Shortcut, "element.convert", json!({ "reverse": false }));
    let report = drafftink_ui::dispatch(&mut f.session, &mut f.state, vec![tab], 0.0);
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
    f.refresh();
    let mut h = dialog::harness(f);
    h.run();
    h
}

fn kind(h: &mut H) -> String {
    let f = h.state_mut();
    f.session.execute("element.query", json!({ "filter": { "selected": true } })).unwrap()["items"][0]["kind"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn tab_opens_the_popup_with_the_type_it_converted_to() {
    let mut h = after_tab("rectangle");
    assert_eq!(h.state().state.convert_popup.as_ref().and_then(|p| p.current.as_deref()), Some("diamond"));
    assert_eq!(kind(&mut h), "diamond");
    h.get_by_label("Convert to rectangle");
    h.get_by_label("Convert to ellipse");
}

#[test]
fn a_click_on_a_type_converts_the_selection_to_it_and_keeps_the_popup() {
    let mut h = after_tab("rectangle");
    h.state_mut().clear();
    h.get_by_label("Convert to ellipse").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "element.convert", json!({ "to": "ellipse" }))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
    assert_eq!(kind(&mut h), "ellipse");
    assert_eq!(h.state().state.convert_popup.as_ref().and_then(|p| p.current.as_deref()), Some("ellipse"));
}

#[test]
fn arrows_get_line_and_the_three_arrow_types() {
    let h = after_tab("arrow");
    for text in ["line", "sharp arrow", "round arrow", "elbow arrow"] {
        h.get_by_label(&format!("Convert to {text}"));
    }
    assert!(h.query_by_label("Convert to ellipse").is_none());
}

#[test]
fn the_popup_closes_with_escape_and_with_the_selection() {
    let mut h = after_tab("rectangle");
    assert!(h.state().state.any_overlay_open(), "so that Esc closes it first");
    h.state_mut().state.close_overlays();
    h.run();
    assert!(h.query_by_label("Convert to ellipse").is_none());

    let mut h = after_tab("rectangle");
    h.state_mut().session.execute("selection.clear", json!({})).unwrap();
    h.state_mut().refresh();
    h.run();
    assert!(h.state().state.convert_popup.is_none());
}

#[test]
fn a_conversion_from_a_menu_does_not_open_the_popup() {
    let mut f = dialog::fixture();
    let id = add_rect(&mut f.session, 0.0, 0.0, 60.0, 40.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    f.refresh();
    let from_menu = UiAction::run_from(Origin::ContextMenu, "element.convert", json!({ "to": "ellipse" }));
    drafftink_ui::dispatch(&mut f.session, &mut f.state, vec![from_menu], 0.0);
    assert!(f.state.convert_popup.is_none());
}
