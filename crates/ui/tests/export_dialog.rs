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

/// A command sink whose picture is not ready for the first calls, as in the browser.
struct SlowPicture {
    not_ready: u32,
    runs: u32,
}

impl drafftink_ui::CommandSink for SlowPicture {
    fn run(
        &mut self,
        _origin: Origin,
        id: &str,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, drafftink_engine::Refusal> {
        self.runs += 1;
        if self.not_ready > 0 {
            self.not_ready -= 1;
            return Err(drafftink_engine::Refusal::render_pending(id, "picture"));
        }
        Ok(json!({ "changed": false }))
    }
}

#[test]
fn an_export_whose_picture_is_not_ready_runs_again_and_closes_the_dialog_when_it_is() {
    let mut state = drafftink_ui::UiState::new();
    state.apply_verb(Verb::OpenExport, &mut Vec::new());
    let mut sink = SlowPicture { not_ready: 2, runs: 0 };
    let export = drafftink_ui::UiAction::run("doc.export", json!({ "format": "png" }));

    let report = drafftink_ui::dispatch(&mut sink, &mut state, vec![export], 0.0);
    assert!(report.ran[0].result.is_err());
    assert!(state.waiting_for_picture(), "the call waits for its picture");
    assert!(state.export.is_some(), "the dialog stays open meanwhile");
    assert_eq!(state.toasts.len(), 1);
    assert!(state.toasts[0].text.contains("Drawing the picture"), "{:?}", state.toasts);

    // The next frames run the waiting call again; the toast is not repeated.
    for _ in 0..2 {
        let again = state.retries().into_iter().map(drafftink_ui::UiAction::Command).collect();
        drafftink_ui::dispatch(&mut sink, &mut state, again, 0.1);
    }
    assert_eq!(sink.runs, 3);
    assert!(!state.waiting_for_picture(), "the third run got the picture");
    assert!(state.export.is_none(), "the export succeeded, so the dialog closed");
    assert_eq!(state.toasts.len(), 1, "no error toast for a picture that was only late");
}

// ---- the direct "Export as PNG" entries run the dialog's command, with the dialog's arguments ----

/// The `doc.export` call that the dialog makes when it is opened and "Export" is clicked at once.
fn dialog_png_call(f: Fixture) -> Call {
    let mut h = open(f);
    export_call(&mut h)
}

/// The `doc.export` call of the menu row "Export as PNG".
fn menu_png_call(mut f: Fixture) -> Call {
    f.state.menu_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h.state_mut().clear();
    click(&mut h, "Export as PNG");
    let all = calls(&h);
    let export: Vec<&Call> = all.iter().filter(|c| c.id == "doc.export").collect();
    assert_eq!(export.len(), 1, "{all:?}");
    export[0].clone()
}

#[test]
fn the_menu_row_export_as_png_makes_the_call_of_the_dialog() {
    for select_first in [false, true] {
        let dialog_call = dialog_png_call(drawing(select_first));
        let menu_call = menu_png_call(drawing(select_first));
        assert_eq!(menu_call.origin, Origin::Menu);
        assert_eq!((&menu_call.id, &menu_call.args), (&dialog_call.id, &dialog_call.args), "selection: {select_first}");
        assert_eq!(menu_call.args["to"], json!({ "download": "Plan.png" }), "a direct export has a place to go");
    }
}

#[test]
fn the_shortcut_and_the_palette_make_the_call_of_the_dialog_too() {
    use drafftink_ui::keymap::{KeyContext, Pointer, resolve_keys};
    let want = dialog_png_call(drawing(false));
    let f = drawing(false);

    let ctrl_e = egui::Event::Key {
        key: egui::Key::E,
        physical_key: Some(egui::Key::E),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
    };
    let ctx = KeyContext {
        model: &f.model,
        state: &f.state,
        egui_wants_keyboard: false,
        pointer: Pointer::default(),
        host_clipboard: false,
    };
    let actions = resolve_keys(&[ctrl_e], &ctx);
    let call = actions.first().and_then(drafftink_ui::UiAction::as_call).expect("Ctrl+E is a command");
    assert_eq!(call.origin, Origin::Shortcut);
    assert_eq!((&call.id, &call.args), (&want.id, &want.args));

    let mut f = f;
    f.state.palette = Some(drafftink_ui::state::PaletteState { query: "Export as PNG".to_owned(), selected: 0 });
    let mut h = dialog::harness(f);
    h.run();
    h.state_mut().clear();
    h.key_press(egui::Key::Enter);
    h.run();
    let all = calls(&h);
    let call = all.iter().find(|c| c.id == "doc.export").expect("the palette ran the export");
    assert_eq!(call.origin, Origin::Palette);
    assert_eq!((&call.id, &call.args), (&want.id, &want.args));
}

#[test]
fn the_menu_row_writes_the_png_download_through_the_real_command() {
    let mut f = drawing(false);
    f.state.menu_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h.state_mut().clear();
    click(&mut h, "Export as PNG");
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
    let written = h.state().fakes.files.bytes(&FileRef::Download { download: "Plan.png".to_owned() });
    let written = written.expect("the PNG went to the files port as a download");
    assert!(written.starts_with(b"\x89PNG"), "{:?}", &written[..written.len().min(12)]);
    assert_eq!(h.state().fakes.renderer.calls(), 1);
}

#[test]
fn a_direct_export_that_is_refused_shows_the_engines_words_also_from_the_keyboard() {
    use drafftink_ui::{CommandSink, dispatch};
    // Nothing to export: from a menu row, from a shortcut and from the palette the person reads why.
    for origin in [Origin::Menu, Origin::Shortcut, Origin::Palette] {
        let mut f = dialog::fixture();
        dialog::run(&mut f, "doc.rename", json!({ "name": "Empty" }));
        let mut state = drafftink_ui::UiState::new();
        let want = f.session.run(origin, "doc.export", json!({ "format": "png", "to": { "download": "Empty.png" } }));
        let want = want.expect_err("an empty document is refused").message;
        let call = Call::new(origin, "doc.export", json!({ "format": "png", "to": { "download": "Empty.png" } }));
        dispatch(&mut f.session, &mut state, vec![drafftink_ui::UiAction::Command(call)], 0.0);
        let shown: Vec<_> = state.toasts.iter().map(|t| t.text.clone()).collect();
        assert_eq!(shown, vec![want], "{origin:?}");
    }
}

#[test]
fn a_direct_export_whose_picture_is_late_runs_again_with_the_same_arguments() {
    let mut f = drawing(false);
    f.state.menu_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h.state_mut().clear();
    click(&mut h, "Export as PNG");
    let first = calls(&h).into_iter().find(|c| c.id == "doc.export").expect("the export call");

    let mut state = drafftink_ui::UiState::new();
    let mut sink = SlowPicture { not_ready: 1, runs: 0 };
    drafftink_ui::dispatch(&mut sink, &mut state, vec![drafftink_ui::UiAction::Command(first.clone())], 0.0);
    assert!(state.waiting_for_picture());
    assert_eq!(state.retries(), vec![first], "the retry is the very call, `to` included");
}
