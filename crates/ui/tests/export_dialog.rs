#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The export dialog (`ui.open_export`): every option reaches `doc.export` or `clipboard.copy_as` with the right
//! arguments, and the real command writes the file.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::ports::FileRef;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::{Call, Verb};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

type H = Harness<'static, Fixture>;

fn open(mut f: Fixture) -> H {
    f.state.apply_verb(Verb::OpenExport, &mut Vec::new());
    let mut h = dialog::harness(f);
    h.run();
    h
}

/// A named document with two rectangles, the first one selected when `select_first`.
fn drawing(select_first: bool) -> Fixture {
    let mut f = dialog::fixture();
    dialog::run(&mut f, "doc.rename", json!({ "name": "Plan" }));
    let a = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    add_rect(&mut f.session, 100.0, 0.0, 50.0, 50.0).unwrap();
    if select_first {
        select(&mut f.session, std::slice::from_ref(&a));
    }
    f.refresh();
    f
}

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

fn click(h: &mut H, label: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, label).click();
    h.run();
}

fn export_call(h: &mut H) -> Call {
    h.state_mut().clear();
    click(h, "Export");
    let all = calls(h);
    assert_eq!(all.len(), 1, "{all:?}");
    all.into_iter().next().unwrap()
}

#[test]
fn the_options_start_from_the_export_settings() {
    let mut f = drawing(false);
    dialog::run(&mut f, "settings.set", json!({ "key": "export.scale", "value": 3.0 }));
    dialog::run(&mut f, "settings.set", json!({ "key": "export.dark", "value": true }));
    dialog::run(&mut f, "settings.set", json!({ "key": "export.background", "value": false }));
    dialog::run(&mut f, "settings.set", json!({ "key": "export.embed_scene", "value": false }));
    let mut h = open(f);
    let call = export_call(&mut h);
    assert_eq!(call.id, "doc.export");
    assert_eq!(call.origin, Origin::Ui);
    assert_eq!(
        call.args,
        json!({
            "format": "png", "scope": "all", "to": { "download": "Plan.png" },
            "scale": 3.0, "background": false, "dark": true, "embed_scene": false,
        })
    );
}

#[test]
fn each_format_has_its_extension_and_the_picture_options_only_for_pictures() {
    for (button, format, file) in [
        ("PNG", "png", "Plan.png"),
        ("SVG", "svg", "Plan.svg"),
        ("JSON", "json", "Plan.json"),
        (".excalidraw", "excalidraw", "Plan.excalidraw"),
    ] {
        let mut h = open(drawing(false));
        click(&mut h, button);
        let call = export_call(&mut h);
        assert_eq!(call.args["format"], json!(format), "{button}");
        assert_eq!(call.args["to"], json!({ "download": file }), "{button}");
        let picture = format == "png" || format == "svg";
        assert_eq!(call.args.get("scale").is_some(), picture, "{button}: scale only for pictures");
        assert_eq!(call.args.get("dark").is_some(), picture, "{button}");
    }
}

#[test]
fn scale_background_dark_and_embed_scene_change_the_arguments() {
    let mut h = open(drawing(false));
    click(&mut h, "3x");
    click(&mut h, "Background");
    click(&mut h, "Dark");
    click(&mut h, "Embed scene");
    let call = export_call(&mut h);
    assert_eq!(call.args["scale"], json!(3.0));
    assert_eq!(call.args["background"], json!(false), "Background was on, one click turned it off");
    assert_eq!(call.args["dark"], json!(true));
    assert_eq!(call.args["embed_scene"], json!(false));
}

#[test]
fn selection_only_follows_the_selection_and_is_greyed_without_one() {
    let mut h = open(drawing(true));
    assert_eq!(export_call(&mut h).args["scope"], json!("selection"), "a selection starts selection-only");
    click(&mut h, "Selection only");
    assert_eq!(export_call(&mut h).args["scope"], json!("all"));

    let mut h = open(drawing(false));
    h.state_mut().clear();
    click(&mut h, "Selection only");
    assert_eq!(export_call(&mut h).args["scope"], json!("all"), "nothing is selected: the choice does nothing");
}

#[test]
fn exporting_svg_writes_the_download_through_the_real_command() {
    let mut h = open(drawing(false));
    click(&mut h, "SVG");
    h.state_mut().clear();
    click(&mut h, "Export");
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
    let bytes = h.state().fakes.files.bytes(&FileRef::Download { download: "Plan.svg".to_owned() });
    let bytes = bytes.expect("the file was written");
    assert!(String::from_utf8_lossy(&bytes).contains("<svg"));
    assert!(h.state().state.export.is_none(), "a finished export closes the dialog");
}

#[test]
fn exporting_json_and_excalidraw_writes_a_file_each() {
    for (button, file) in [("JSON", "Plan.json"), (".excalidraw", "Plan.excalidraw")] {
        let mut h = open(drawing(false));
        click(&mut h, button);
        h.state_mut().clear();
        click(&mut h, "Export");
        let report = h.state_mut().dispatch();
        assert!(report.refusals().next().is_none(), "{button}: {:?}", report.ran);
        let written = h.state().fakes.files.bytes(&FileRef::Download { download: file.to_owned() });
        assert!(written.is_some_and(|b| !b.is_empty()), "{file}");
    }
}

#[test]
fn a_refused_export_keeps_the_dialog_and_shows_the_words() {
    // Nothing to export: the command refuses with `nothing_to_do`.
    let mut f = dialog::fixture();
    dialog::run(&mut f, "doc.rename", json!({ "name": "Empty" }));
    let mut h = open(f);
    click(&mut h, "SVG");
    h.state_mut().clear();
    click(&mut h, "Export");
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().next().map(|r| r.code), Some("nothing_to_do"));
    assert!(h.state().state.export.is_some());
    assert_eq!(h.state().state.toasts.len(), 1);
}

#[test]
fn copy_runs_copy_as_with_the_same_options() {
    let mut h = open(drawing(true));
    click(&mut h, "2x");
    click(&mut h, "Dark");
    h.state_mut().clear();
    click(&mut h, "Copy to clipboard");
    assert_eq!(
        calls(&h),
        vec![Call::new(
            Origin::Ui,
            "clipboard.copy_as",
            json!({ "format": "png", "scope": "selection", "scale": 2.0, "background": true, "dark": true })
        )]
    );
    click(&mut h, "SVG");
    h.state_mut().clear();
    click(&mut h, "Copy to clipboard");
    assert_eq!(calls(&h)[0].args["format"], json!("svg"));
    click(&mut h, "JSON");
    assert!(h.query_by_label("Copy to clipboard").is_none(), "only pictures can be copied");
}

#[test]
fn the_menu_has_an_export_row_that_opens_the_dialog() {
    let mut f = drawing(false);
    f.state.menu_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h.state_mut().clear();
    click(&mut h, "Export…");
    assert_eq!(dialog::verbs(h.state()), vec![Verb::OpenExport]);
}
