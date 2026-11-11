//! Tests of the `doc` family. No UI: a session over the fake ports. Pattern per command: availability,
//! refusal text, effect (with undo where the command is undoable), one negative case that leaves the
//! state unchanged.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use drafftink_scene::{ElementKind, FrameData, SceneRead, SceneTx};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::files::{AUTOSAVE_NAME, use_stored_default_target};
use crate::cmd::tab::{MAX_TABS, NEVER_SAVED, RoomBinding};
use crate::ports::fake::Fakes;
use crate::ports::{FileRef, Ns, PortError, RenderRequest, Renderer, SceneView, WriteOutcome, WriteToken};
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, Redraw, RingMode, Selection, TextGate, Typed, Undo};
use crate::state::TextEdit;
use crate::testing;

/// A valid 1x1 PNG.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR4nGP4DwQACfsD/fteaysAAAAASUVORK5CYII=";

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => {
            assert_eq!(s.fingerprint(), before, "{id} refused with `{}` but changed state", r.code);
            r
        }
    }
}

fn path(p: &str) -> FileRef {
    FileRef::Path { path: p.to_owned() }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap()).unwrap()
}

fn live(s: &Session) -> usize {
    s.store().len()
}

fn name(s: &Session) -> String {
    s.store().meta().name.clone()
}

fn rects(s: &mut Session, n: usize) {
    for i in 0..n {
        testing::add_rect(s, i as f64 * 150.0, 0.0, 100.0, 60.0).unwrap();
    }
}

/// Runs `doc.open` on inline bytes, with extra arguments.
fn open_inline(s: &mut Session, bytes: &[u8], extra: Value) -> Result<Value, Refusal> {
    let mut args = json!({"source": {"base64": b64(bytes), "name": "x.json"}});
    if let (Value::Object(a), Value::Object(e)) = (&mut args, extra) {
        a.extend(e);
    }
    s.execute_from(Origin::Test, "doc.open", args)
}

/// A session with `n` rectangles that was saved to nowhere: dirty.
fn drawn(n: usize) -> (Session, Fakes) {
    let (mut s, f) = testing::session();
    rects(&mut s, n);
    (s, f)
}

/// The document of the session as native JSON bytes.
fn native(s: &mut Session) -> Vec<u8> {
    let v = run(s, "doc.export", json!({"format": "json", "scope": "all"}));
    unb64(&v["bytes"]["base64"])
}

/// A renderer that returns a valid PNG and records the last request.
#[derive(Clone, Default)]
struct PngRenderer {
    seen: Arc<Mutex<Vec<RenderRequest>>>,
}

impl Renderer for PngRenderer {
    fn render_png(&self, _: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(base64::engine::general_purpose::STANDARD.decode(PNG_1X1).unwrap())
    }
}

fn with_png(s: &mut Session) -> PngRenderer {
    let r = PngRenderer::default();
    s.ports.renderer = Some(Box::new(r.clone()));
    r
}

/// A renderer that is not ready for the first calls and then returns a picture, as the browser's is.
struct LateRenderer(std::cell::Cell<u32>);

impl Renderer for LateRenderer {
    fn render_png(&self, _: &SceneView, _: &RenderRequest) -> Result<Vec<u8>, PortError> {
        let left = self.0.get();
        if left == 0 {
            return Ok(base64::engine::general_purpose::STANDARD.decode(PNG_1X1).unwrap());
        }
        self.0.set(left - 1);
        Err(PortError::NotReady)
    }
}

#[test]
fn export_png_with_a_renderer_that_is_not_ready_refuses_with_render_pending_and_succeeds_when_run_again() {
    let (mut s, _f) = drawn(2);
    s.ports.renderer = Some(Box::new(LateRenderer(std::cell::Cell::new(1))));
    let r = refuse(&mut s, "doc.export", json!({"format": "png", "scope": "all"}));
    assert_eq!(r.code, "render_pending");
    assert!(r.message.contains("run"), "the words say how to go on: {}", r.message);
    let v = run(&mut s, "doc.export", json!({"format": "png", "scope": "all"}));
    assert_eq!(v["mime"], "image/png");
}

/// A batch that moves the first rectangle and then exports a PNG, for a renderer that needs `late` calls.
fn move_then_export(s: &mut Session, late: u32, opts: &crate::batch::BatchOpts) -> crate::batch::BatchOutput {
    s.ports.renderer = Some(Box::new(LateRenderer(std::cell::Cell::new(late))));
    let first = s.store().iter().next().unwrap().id.to_string();
    let steps = [
        crate::batch::BatchStep::new("element.move", json!({"ids": [first], "dx": 25, "dy": 0})),
        crate::batch::BatchStep::new("doc.export", json!({"format": "png", "scope": "all"})),
    ];
    s.execute_batch(Origin::Test, &steps, opts)
}

#[test]
fn a_batch_with_a_picture_step_last_completes_as_a_whole_and_the_picture_follows() {
    for atomic in [true, false] {
        let (mut s, _f) = drawn(2);
        let opts = crate::batch::BatchOpts { atomic, ..Default::default() };
        let out = move_then_export(&mut s, 2, &opts);
        // The batch is done: nothing is taken back, the move is applied as one undo step.
        assert!(out.ok() && !out.rolled_back && !out.pending_render(), "atomic={atomic}: {out:?}");
        assert!(out.changed, "atomic={atomic}: the move stays");
        assert_eq!(out.completed, 2);
        assert_eq!(s.tab().doc.undo_depth(), 1);
        assert_eq!(out.pending.len(), 1);
        let (step, token) = (&out.pending[0], out.pending[0].token);
        assert_eq!((step.index, step.id.as_str()), (1, "doc.export"));
        assert_eq!(out.results[1], json!({ "pending_render": token }), "a placeholder until the picture is in");
        assert_eq!(out.to_json()["pending"][0]["token"], json!(token));

        // The picture is not ready for one more call; then the step runs and gives its own output.
        let before = s.fingerprint();
        assert_eq!(s.finish_render(token).unwrap(), crate::Finished::Waiting);
        assert_eq!(s.tab().doc.undo_depth(), 1, "waiting changes nothing");
        let crate::Finished::Done(export) = s.finish_render(token).unwrap() else { panic!("the picture is ready") };
        assert_eq!(export["mime"], "image/png");
        assert_eq!(s.tab().doc.undo_depth(), 1, "the picture is not an undo step");
        assert_eq!(s.fingerprint().split("saved=").next(), before.split("saved=").next());
        assert_eq!(s.finish_render(token).unwrap_err().code, "render_unknown", "a token is used once");
    }
}

#[test]
fn a_batch_with_steps_that_cannot_be_rolled_back_completes_and_keeps_them_while_the_picture_waits() {
    let (mut s, _f) = drawn(1);
    s.ports.renderer = Some(Box::new(LateRenderer(std::cell::Cell::new(1))));
    let steps = [
        crate::batch::BatchStep::new("settings.set", json!({"key": "snap.grid", "value": true})),
        crate::batch::BatchStep::new("doc.export", json!({"format": "png", "scope": "all"})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &crate::batch::BatchOpts { atomic: false, ..Default::default() });
    assert!(out.ok() && !out.pending_render(), "{out:?}");
    assert_eq!(out.pending.len(), 1);
    assert_eq!(out.not_rolled_back.len(), 1, "the setting is named as outside the transaction");
    assert!(matches!(s.finish_render(out.pending[0].token).unwrap(), crate::Finished::Done(_)));
}

#[test]
fn a_picture_step_followed_by_a_step_that_changes_the_picture_still_waits_the_old_way() {
    for atomic in [true, false] {
        let (mut s, _f) = drawn(2);
        s.ports.renderer = Some(Box::new(LateRenderer(std::cell::Cell::new(1))));
        let first = s.store().iter().next().unwrap().id.to_string();
        let steps = [
            crate::batch::BatchStep::new("doc.export", json!({"format": "png", "scope": "all"})),
            crate::batch::BatchStep::new("element.move", json!({"ids": [first], "dx": 25, "dy": 0})),
        ];
        let opts = crate::batch::BatchOpts { atomic, ..Default::default() };
        let before = s.fingerprint();
        let waiting = s.execute_batch(Origin::Test, &steps, &opts);
        assert!(waiting.pending_render() && waiting.pending.is_empty(), "atomic={atomic}: {waiting:?}");
        assert!(waiting.rolled_back && !waiting.changed);
        assert_eq!(s.fingerprint(), before, "a waiting batch changes nothing");
        // Run again when the picture is ready: the picture is of the document before the move.
        let done = s.execute_batch(Origin::Test, &steps, &opts);
        assert!(done.ok() && done.pending.is_empty(), "atomic={atomic}: {done:?}");
        assert_eq!(done.results[0]["mime"], "image/png");
        assert!(done.changed);
    }
}

#[test]
fn a_picture_whose_document_changed_while_it_was_drawn_is_refused_not_made_for_the_wrong_state() {
    let (mut s, _f) = drawn(2);
    let out = move_then_export(&mut s, 1, &crate::batch::BatchOpts::default());
    let token = out.pending[0].token;
    // Someone edits before the picture is read back.
    testing::add_rect(&mut s, 900.0, 0.0, 10.0, 10.0).unwrap();
    let r = s.finish_render(token).unwrap_err();
    assert_eq!(r.code, "render_stale");
    assert!(r.message.contains("doc.export") && r.message.contains("again"), "{}", r.message);
    assert_eq!(s.finish_render(token).unwrap_err().code, "render_unknown");
}

#[test]
fn a_dry_run_with_a_picture_step_changes_nothing_and_keeps_no_token() {
    let (mut s, _f) = drawn(2);
    let before = s.fingerprint();
    let out = move_then_export(&mut s, 1, &crate::batch::BatchOpts { dry_run: true, ..Default::default() });
    assert!(out.rolled_back && out.pending.is_empty(), "{out:?}");
    assert_eq!(s.fingerprint(), before);
}

// ---- doc.new ----

#[test]
fn new_replaces_the_document_and_resets_everything_around_it() {
    let (mut s, _f) = drawn(3);
    s.tab_mut().saved_revision = s.tab().doc.revision();
    let first = s.store().iter().next().unwrap().id.clone();
    s.tab_mut().observed.selection.insert(first);
    s.tab_mut().view.zoom = 5.0;
    set_target(&mut s, path("/a.json"));
    let out = run(&mut s, "doc.new", json!({"name": "  Fresh  "}));
    assert_eq!(out["name"], "Fresh");
    assert_eq!(out["mode"], "replace");
    assert_eq!((live(&s), name(&s)), (0, "Fresh".to_owned()));
    assert!(s.selection().is_empty());
    assert!(!s.tab().dirty(), "a blank document is not dirty");
    assert_eq!(s.tab().doc.undo_depth(), 0, "no undo step for a replaced document");
    assert!(super::save_target(&s).is_none());
    assert_eq!(s.view().zoom, crate::state::BASE_ZOOM);
    assert_eq!(s.store().meta().background, drafftink_scene::Color::WHITE);
    assert_eq!(s.tabs.len(), 1);
}

fn set_target(s: &mut Session, f: FileRef) {
    super::set_save_target(s, Some(f));
}

#[test]
fn new_uses_the_default_background_setting() {
    let (mut s, _f) = testing::session();
    run(&mut s, "settings.set", json!({"key": "doc.default_background", "value": "#fafafa"}));
    run(&mut s, "doc.new", json!({}));
    assert_eq!(s.store().meta().background.to_hex(), "#fafafa");
}

#[test]
fn new_refuses_unsaved_changes_and_says_how_to_go_on() {
    let (mut s, _f) = drawn(2);
    let r = refuse(&mut s, "doc.new", json!({}));
    assert_eq!(r.code, "unsaved_changes");
    assert!(r.message.contains("on_unsaved=\"discard\""), "{}", r.message);
    assert_eq!(live(&s), 2);
    run(&mut s, "doc.new", json!({"on_unsaved": "discard"}));
    assert_eq!(live(&s), 0);
}

#[test]
fn new_with_on_unsaved_save_writes_the_old_document_first() {
    let (mut s, f) = drawn(2);
    let r = refuse(&mut s, "doc.new", json!({"on_unsaved": "save"}));
    assert_eq!(r.code, "no_save_target");
    assert!(r.message.contains("Pass `to`"), "{}", r.message);
    set_target(&mut s, path("/plan.json"));
    run(&mut s, "doc.new", json!({"on_unsaved": "save"}));
    let saved = f.files.bytes(&path("/plan.json")).expect("the old document was saved");
    assert!(String::from_utf8(saved).unwrap().contains("\"rectangle\""));
    assert_eq!(live(&s), 0);
}

#[test]
fn new_in_a_new_tab_keeps_the_old_document() {
    let (mut s, _f) = drawn(2);
    let out = run(&mut s, "doc.new", json!({"in_new_tab": true, "name": "Second"}));
    assert_eq!(out["mode"], "new_tab");
    assert_eq!(s.tabs.len(), 2);
    assert_eq!((s.active_index(), name(&s)), (1, "Second".to_owned()));
    assert_eq!(s.tabs[0].doc.store().len(), 2);
    assert!(s.tabs[0].dirty() && !s.tab().dirty());
}

#[test]
fn new_in_a_room_refuses_replace_but_not_a_new_tab() {
    let (mut s, _f) = testing::session();
    s.tab_mut().ext.set(RoomBinding(Some("r-42".to_owned())));
    let r = refuse(&mut s, "doc.new", json!({}));
    assert_eq!(r.code, "in_room");
    assert!(r.message.starts_with("This tab is connected to room r-42, and a new document would replace the shared one."), "{}", r.message);
    assert!(r.message.ends_with("or pass in_new_tab=true to open the new document in a new tab."), "{}", r.message);
    run(&mut s, "doc.new", json!({"in_new_tab": true}));
    assert_eq!(s.tabs.len(), 2);
}

#[test]
fn new_refuses_a_bad_name_and_a_full_tab_bar() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "doc.new", json!({"name": "   "}));
    assert_eq!(r.code, "empty_name");
    assert_eq!(r.message, "The name is empty. Give a name of 1 to 256 characters.");
    for _ in 1..MAX_TABS {
        run(&mut s, "doc.new", json!({"in_new_tab": true}));
    }
    let r = refuse(&mut s, "doc.new", json!({"in_new_tab": true}));
    assert_eq!(r.code, "too_many_tabs");
}

#[test]
fn new_is_refused_in_view_only_and_during_a_drag_and_commits_a_text_edit() {
    let (mut s, _f) = testing::session();
    s.view_only = true;
    assert_eq!(refuse(&mut s, "doc.new", json!({})).code, "view_only");
    s.view_only = false;
    assert_eq!(s.spec("doc.new").unwrap().batch, Batch::Last);
    assert_eq!(s.spec("doc.new").unwrap().while_text_edit, TextGate::Commit);
    s.text_edit = Some(TextEdit { element: None, draft: true });
    run(&mut s, "doc.new", json!({}));
    assert!(s.text_edit.is_none());
}

// ---- doc.open ----

#[test]
fn open_reads_a_native_document_from_a_path_and_makes_it_the_save_target() {
    let (mut a, _) = drawn(3);
    let bytes = native(&mut a);
    let (mut s, f) = testing::session();
    f.files.insert(&path("/home/me/plan.json"), &bytes, "application/json");
    let out = run(&mut s, "doc.open", json!({"source": {"path": "/home/me/plan.json"}}));
    assert_eq!((out["elements"].as_u64(), out["format"].as_str()), (Some(3), Some("drafftink")));
    assert_eq!(live(&s), 3);
    assert!(!s.tab().dirty());
    assert_eq!(s.tab().doc.undo_depth(), 0);
    assert_eq!(super::save_target(&s), Some(path("/home/me/plan.json")));
    assert_eq!(super::files::recent(&s), vec![path("/home/me/plan.json")]);
    assert_eq!(s.settings.get("doc.recent"), Some(&json!([{"path": "/home/me/plan.json"}])));
}

#[test]
fn opening_a_foreign_file_sets_no_save_target_so_save_never_overwrites_it() {
    let (mut a, _) = drawn(2);
    with_png(&mut a);
    let png = unb64(&run(&mut a, "doc.export", json!({"format": "png"}))["bytes"]["base64"]);
    let svg = unb64(&run(&mut a, "doc.export", json!({"format": "svg"}))["bytes"]["base64"]);
    let foreign: [(&str, &[u8]); 4] = [
        ("/w/basic.excalidraw", include_bytes!("../../../../tests/fixtures/excalidraw/basic_shapes.excalidraw")),
        ("/w/old.json", include_bytes!("../../../../tests/fixtures/legacy/all_shapes.json")),
        ("/w/scene.png", &png),
        ("/w/scene.svg", &svg),
    ];
    for mode in ["replace", "new_tab"] {
        for (p, bytes) in foreign {
            let (mut s, f) = testing::session();
            f.files.insert(&path(p), bytes, "application/octet-stream");
            run(&mut s, "doc.open", json!({"source": {"path": p}, "mode": mode}));
            assert_eq!(super::save_target(&s), None, "{p} ({mode}) must not become the save target");
            assert_eq!(super::files::recent(&s), vec![path(p)], "{p} is still a recent file");
            let r = refuse(&mut s, "doc.save", json!({}));
            assert_eq!(r.code, "no_save_target", "{p} ({mode})");
            assert_eq!(f.files.bytes(&path(p)).as_deref(), Some(bytes), "{p} ({mode}) was overwritten");
            run(&mut s, "doc.save", json!({"to": {"path": "/w/mine.json"}}));
            assert_eq!(super::save_target(&s), Some(path("/w/mine.json")));
        }
    }
}

#[test]
fn open_fits_the_camera_to_the_content() {
    let (mut a, _) = drawn(3);
    let bytes = native(&mut a);
    let (mut s, _f) = testing::session();
    s.tab_mut().view.scroll = drafftink_geom::kurbo::Point::new(9000.0, 9000.0);
    open_inline(&mut s, &bytes, json!({})).unwrap();
    let v = s.view();
    assert!(v.scroll.x.abs() < 1000.0 && v.scroll.y.abs() < 1000.0, "{v:?}");
}

#[test]
fn open_refuses_unsaved_changes_in_replace_mode_only() {
    let (mut a, _) = drawn(1);
    let bytes = native(&mut a);
    let (mut s, _f) = drawn(2);
    let before = s.fingerprint();
    let r = open_inline(&mut s, &bytes, json!({})).unwrap_err();
    assert_eq!(r.code, "unsaved_changes");
    assert_eq!(s.fingerprint(), before);
    open_inline(&mut s, &bytes, json!({"mode": "new_tab"})).unwrap();
    assert_eq!(s.tabs.len(), 2);
    assert_eq!(s.tabs[0].doc.store().len(), 2);
    open_inline(&mut s, &bytes, json!({"mode": "merge"})).unwrap();
    open_inline(&mut s, &bytes, json!({"on_unsaved": "discard"})).unwrap();
    assert_eq!(live(&s), 1);
}

#[test]
fn open_in_a_room_refuses_replace_with_the_new_tab_hint() {
    let (mut a, _) = drawn(1);
    let bytes = native(&mut a);
    let (mut s, _f) = testing::session();
    s.tab_mut().ext.set(RoomBinding(Some("r-9".to_owned())));
    let r = open_inline(&mut s, &bytes, json!({})).unwrap_err();
    assert_eq!(r.code, "in_room");
    assert!(r.message.ends_with("or pass mode=new_tab to open the file in a new tab."), "{}", r.message);
    open_inline(&mut s, &bytes, json!({"mode": "merge"})).unwrap();
}

#[test]
fn open_merge_adds_new_elements_selects_them_and_undoes_in_one_step() {
    let (mut a, _) = drawn(2);
    let bytes = native(&mut a);
    let (mut s, _f) = drawn(1);
    let own = s.store().iter().next().unwrap().id.clone();
    let out = open_inline(&mut s, &bytes, json!({"mode": "merge", "at": [1000.0, 1000.0]})).unwrap();
    assert_eq!((out["elements"].as_u64(), out["selected"].as_u64()), (Some(2), Some(2)));
    assert_eq!(live(&s), 3);
    assert_eq!(s.selection().len(), 2);
    assert!(!s.selection().contains(&own));
    let b = drafftink_edit::query::element_bounds(s.element(s.selection().iter().next().unwrap()).unwrap());
    assert!(b.x0 > 800.0, "{b:?}");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(live(&s), 1);
}

#[test]
fn open_merge_without_at_keeps_the_coordinates_and_without_select_keeps_the_selection() {
    let (mut a, _) = drawn(1);
    let bytes = native(&mut a);
    let (mut s, _f) = drawn(1);
    let own = s.store().iter().next().unwrap().id.clone();
    testing::select(&mut s, std::slice::from_ref(&own));
    open_inline(&mut s, &bytes, json!({"mode": "merge", "select": false})).unwrap();
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), vec![&own]);
    let xs: Vec<f64> = s.store().iter().map(|e| e.transform.x).collect();
    assert_eq!(xs, vec![0.0, 0.0]);
}

#[test]
fn open_reads_old_drafft_ink_json_and_excalidraw_scenes() {
    let legacy = include_bytes!("../../../../tests/fixtures/legacy/all_shapes.json");
    let (mut s, _f) = testing::session();
    let out = open_inline(&mut s, legacy, json!({})).unwrap();
    assert_eq!(out["format"], "drafftink");
    assert!(live(&s) > 3);
    let ex = include_bytes!("../../../../tests/fixtures/excalidraw/basic_shapes.excalidraw");
    let out = open_inline(&mut s, ex, json!({"format": "excalidraw"})).unwrap();
    assert_eq!(out["format"], "excalidraw");
    assert!(live(&s) >= 3);
}

#[test]
fn open_with_the_wrong_format_word_refuses() {
    let ex = include_bytes!("../../../../tests/fixtures/excalidraw/basic_shapes.excalidraw");
    let (mut s, _f) = testing::session();
    let before = s.fingerprint();
    let r = open_inline(&mut s, ex, json!({"format": "png"})).unwrap_err();
    assert_eq!(r.code, "parse_failed");
    assert!(r.message.contains("format=png"), "{}", r.message);
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn open_restores_the_scene_of_a_png_and_of_an_svg() {
    let (mut a, _) = drawn(2);
    with_png(&mut a);
    let png = unb64(&run(&mut a, "doc.export", json!({"format": "png"}))["bytes"]["base64"]);
    let svg = unb64(&run(&mut a, "doc.export", json!({"format": "svg"}))["bytes"]["base64"]);
    assert!(String::from_utf8_lossy(&svg).starts_with("<svg"));
    for (bytes, fmt) in [(png, "png"), (svg, "png")] {
        let (mut s, _f) = testing::session();
        let out = open_inline(&mut s, &bytes, json!({"format": fmt})).unwrap();
        assert_eq!(live(&s), 2, "{out}");
    }
}

#[test]
fn open_says_what_is_wrong_with_a_file_that_is_not_a_document() {
    let (mut s, _f) = testing::session();
    let r = open_inline(&mut s, b"hello there", json!({})).unwrap_err();
    assert_eq!(r.code, "parse_failed");
    assert!(r.message.starts_with("The file is not a drafft.ink or Excalidraw document:"), "{}", r.message);
    assert!(r.message.ends_with("or a PNG exported with the scene embedded."), "{}", r.message);
    let lib = include_bytes!("../../../../tests/fixtures/excalidraw/fixture_library.excalidrawlib");
    let r = open_inline(&mut s, lib, json!({})).unwrap_err();
    assert_eq!(r.code, "is_library");
    assert_eq!(
        r.message,
        "`x.json` is an Excalidraw library, not a document. Add it to the library with `library.import`."
    );
    assert_eq!(r.suggest[0].command, "library.import");
    let r = open_inline(&mut s, b"flowchart TD\n  A --> B\n", json!({})).unwrap_err();
    assert_eq!(r.code, "parse_failed");
    assert_eq!(r.suggest[0].command, "diagram.import_mermaid");
}

#[test]
fn open_refuses_a_file_from_a_newer_version() {
    let (mut s, _f) = testing::session();
    let newer = br#"{"format":"drafftink","version":999,"name":"x","elements":[]}"#;
    let r = open_inline(&mut s, newer, json!({})).unwrap_err();
    assert_eq!(r.code, "newer_version");
    assert!(r.message.starts_with("This file was saved by a newer drafft.ink (format 999), and this one reads up to format"), "{}", r.message);
    assert!(r.message.ends_with("Update the app to open it."), "{}", r.message);
}

#[test]
fn open_refuses_missing_files_ports_and_bad_base64() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "doc.open", json!({"source": {"path": "/nope.json"}}));
    assert_eq!(r.code, "io_failed");
    assert!(r.message.starts_with("Could not read nope.json:"), "{}", r.message);
    let r = refuse(&mut s, "doc.open", json!({"source": {"base64": "!!!"}}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("source.base64"), "{}", r.message);
    let r = refuse(&mut s, "doc.open", json!({"source": {"stored": "nothing"}}));
    assert_eq!(r.code, "io_failed");
    let mut h = testing::headless_session();
    let r = refuse(&mut h, "doc.open", json!({"source": {"path": "/a.json"}}));
    assert_eq!(r.code, "port_unavailable");
    assert!(r.message.contains("files port missing"), "{}", r.message);
}

#[test]
fn open_a_stored_document_by_name() {
    let (mut a, _) = drawn(2);
    let (mut s, _f) = testing::session();
    run(&mut a, "doc.rename", json!({"name": "stored one"}));
    run(&mut a, "doc.save", json!({"to": {"stored": "stored one"}}));
    let bytes = native(&mut a);
    s.ports.storage.as_mut().unwrap().put(Ns::Documents, "stored one", &bytes).unwrap();
    run(&mut s, "doc.open", json!({"source": {"stored": "stored one"}}));
    assert_eq!((live(&s), name(&s)), (2, "stored one".to_owned()));
    assert_eq!(super::save_target(&s), Some(FileRef::Stored { stored: "stored one".to_owned() }));
}

#[test]
fn opening_the_autosave_restores_the_active_tab_as_unsaved_work() {
    let (mut s, _f) = drawn(2);
    let out = run(&mut s, "doc.autosave", json!({}));
    assert_eq!(out["saved"], true);
    s.tab_mut().saved_revision = s.tab().doc.revision();
    run(&mut s, "doc.new", json!({}));
    assert_eq!(live(&s), 0);
    run(&mut s, "doc.open", json!({"source": {"stored": AUTOSAVE_NAME}}));
    assert_eq!(live(&s), 2);
    assert_eq!(s.tab().saved_revision, NEVER_SAVED);
    assert!(s.tab().dirty());
    assert!(super::save_target(&s).is_none());
    assert!(super::files::recent(&s).is_empty(), "the autosave is not a recent file");
}

#[test]
fn opening_a_missing_autosave_is_an_io_failure() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "doc.open", json!({"source": {"stored": AUTOSAVE_NAME}}));
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("nothing has been autosaved yet"), "{}", r.message);
}

#[test]
fn open_has_the_facts_of_its_row() {
    let spec = s_spec("doc.open");
    assert_eq!((spec.undo, spec.batch, spec.view_only), (Undo::Undoable, Batch::Last, Gate::Refused));
}

fn s_spec(id: &str) -> &'static CommandSpec {
    testing::headless_session().spec(id).unwrap()
}

// ---- doc.show_intro ----

#[test]
fn show_intro_replaces_the_document_with_the_welcome_document() {
    let (mut s, _f) = testing::session();
    let out = run(&mut s, "doc.show_intro", json!({}));
    assert!(out["elements"].as_u64().unwrap() > 20, "{out}");
    assert_eq!(name(&s), "Welcome");
    assert!(!s.tab().dirty());
    assert_eq!(s.tab().doc.undo_depth(), 0);
    assert!(live(&s) > 20);
}

#[test]
fn show_intro_refuses_unsaved_changes_and_can_use_a_new_tab() {
    let (mut s, _f) = drawn(1);
    let r = refuse(&mut s, "doc.show_intro", json!({}));
    assert_eq!(r.code, "unsaved_changes");
    run(&mut s, "doc.show_intro", json!({"mode": "new_tab"}));
    assert_eq!(s.tabs.len(), 2);
    assert_eq!(s.tabs[0].doc.store().len(), 1);
    s.tab_mut().ext.set(RoomBinding(Some("r".to_owned())));
    let r = refuse(&mut s, "doc.show_intro", json!({}));
    assert_eq!(r.code, "in_room");
    assert!(r.message.ends_with("or pass mode=new_tab to open the file in a new tab."), "{}", r.message);
}

// ---- doc.drop ----

#[test]
fn drop_with_nothing_usable_says_what_is_supported() {
    let (mut s, _f) = testing::session();
    let r = refuse(
        &mut s,
        "doc.drop",
        json!({"items": [{"name": "a.txt", "bytes": {"base64": b64(b"just text")}}, {"name": "b.bin", "bytes": {"base64": b64(&[1, 2, 3])}}]}),
    );
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(
        r.message,
        "Nothing to open: none of the 2 dropped files is an image, document or library. Supported: PNG, JPEG, WebP, .json, .drafftink, .excalidraw, .excalidrawlib."
    );
}

#[test]
fn drop_refuses_more_than_fifty_files() {
    let (mut s, _f) = testing::session();
    let item = json!({"name": "a", "bytes": {"base64": "AAAA"}});
    let r = refuse(&mut s, "doc.drop", json!({"items": vec![item; 51]}));
    assert_eq!(r.code, "limit_exceeded");
    assert!(r.message.contains("limit is 50"), "{}", r.message);
}

#[test]
fn drop_opens_documents_in_new_tabs_and_imports_libraries() {
    let (mut a, _) = drawn(2);
    let doc = native(&mut a);
    let lib = include_bytes!("../../../../tests/fixtures/excalidraw/fixture_library.excalidrawlib");
    let (mut s, _f) = testing::session();
    let out = run(
        &mut s,
        "doc.drop",
        json!({"items": [
            {"name": "plan.json", "bytes": {"base64": b64(&doc)}},
            {"name": "shapes.excalidrawlib", "bytes": {"base64": b64(lib)}},
            {"name": "notes.txt", "bytes": {"base64": b64(b"hello")}},
        ]}),
    );
    assert_eq!((out["documents"].as_u64(), out["skipped"].as_u64()), (Some(1), Some(1)));
    assert!(out["library_items"].as_u64().unwrap() > 0, "{out}");
    assert_eq!(s.tabs.len(), 2);
    assert_eq!(live(&s), 2);
    assert!(out["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("notes.txt")));
}

#[test]
fn drop_documents_replace_needs_the_unsaved_decision() {
    let (mut a, _) = drawn(1);
    let doc = native(&mut a);
    let (mut s, _f) = drawn(3);
    let args = json!({"items": [{"name": "p.json", "bytes": {"base64": b64(&doc)}}], "documents": "replace"});
    let r = refuse(&mut s, "doc.drop", args.clone());
    assert_eq!(r.code, "unsaved_changes");
    let mut ok = args;
    ok["on_unsaved"] = json!("discard");
    run(&mut s, "doc.drop", ok);
    assert_eq!(live(&s), 1);
}

#[test]
fn drop_merge_adds_the_elements_to_the_active_document() {
    let (mut a, _) = drawn(2);
    let doc = native(&mut a);
    let (mut s, _f) = drawn(1);
    run(&mut s, "doc.drop", json!({"items": [{"name": "p.json", "bytes": {"base64": b64(&doc)}}], "documents": "merge", "at": [500.0, 500.0]}));
    assert_eq!((live(&s), s.tabs.len()), (3, 1));
}

#[test]
fn drop_merge_without_at_centres_the_block_on_the_viewport() {
    let (mut a, _) = drawn(1);
    let doc = native(&mut a);
    let (mut s, _f) = testing::session();
    s.tab_mut().view.scroll = drafftink_geom::kurbo::Point::new(4000.0, 3000.0);
    let items = json!([{"name": "p.json", "bytes": {"base64": b64(&doc)}}]);
    run(&mut s, "doc.drop", json!({"items": items, "documents": "merge"}));
    let r = s.store().iter().next().map(|e| (e.transform.x + e.transform.w / 2.0, e.transform.y + e.transform.h / 2.0)).unwrap();
    assert_eq!(r, (4000.0, 3000.0));
    // The word `$cursor` is accepted like in a paste; without a pointer it means the viewport centre.
    run(&mut s, "doc.drop", json!({"items": items, "documents": "merge", "at": "$cursor"}));
    assert_eq!(live(&s), 2);
}

#[test]
fn drop_reads_a_path_item_and_a_native_file_becomes_its_save_target() {
    let (mut a, _) = drawn(2);
    let doc = native(&mut a);
    let (mut s, f) = testing::session();
    f.files.insert(&path("/home/me/dropped.json"), &doc, "application/json");
    run(&mut s, "doc.drop", json!({"items": [{"name": "dropped.json", "bytes": {"path": "/home/me/dropped.json"}}]}));
    assert_eq!(s.tabs.len(), 2);
    assert_eq!(super::save_target(&s), Some(path("/home/me/dropped.json")));
    // A path that cannot be read is skipped with a reason; damaged base64 is a bad argument.
    let r = refuse(&mut s, "doc.drop", json!({"items": [{"name": "gone.json", "bytes": {"path": "/nope.json"}}]}));
    assert_eq!(r.code, "nothing_to_do");
    let r = refuse(&mut s, "doc.drop", json!({"items": [{"name": "x.json", "bytes": {"base64": "***"}}]}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("items[0].bytes.base64"), "{}", r.message);
    // Only `{base64}` and `{path}` are bytes; a stored name is not.
    let r = refuse(&mut s, "doc.drop", json!({"items": [{"name": "x.json", "bytes": {"stored": "Notes"}}]}));
    assert_eq!(r.code, "bad_args");
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImagesArgs {
    images: Vec<Value>,
    at: [f64; 2],
    offset: f64,
}

static SEEN_IMAGES: Mutex<Vec<Value>> = Mutex::new(Vec::new());

fn image_insert(_: &mut Session, a: ImagesArgs) -> Result<Value, Refusal> {
    SEEN_IMAGES.lock().unwrap().push(json!({"n": a.images.len(), "at": a.at, "offset": a.offset, "first": a.images[0]["name"]}));
    Ok(json!({}))
}

const STAND_IN: &[CommandSpec] = &[CommandSpec::new(
    "image.insert",
    "image",
    "Insert image",
    "A stand-in for the real image command, recording its arguments.",
    &Typed::new(image_insert),
    Facts {
        undo: Undo::Undoable,
        reads_only: false,
        mutates: Mutates::DOC,
        needs_selection: Selection::No,
        ports: crate::spec::PortSet::NONE,
        batch: Batch::Allowed,
        view_only: Gate::Refused,
        while_gesture: Gate::Refused,
        while_text_edit: TextGate::Commit,
        redraw: Redraw::Scene,
        ring: RingMode::Always,
        palette: false,
        agent: false,
    doc_write: crate::spec::DocWrite::Edit,
    },
)];

#[test]
fn drop_hands_images_to_image_insert_and_warns_when_it_is_missing() {
    let png = base64::engine::general_purpose::STANDARD.decode(PNG_1X1).unwrap();
    let items = json!({"items": [
        {"name": "one.png", "bytes": {"base64": b64(&png)}, "mime": "image/png"},
        {"name": "two.jpg", "bytes": {"base64": b64(&[0xff, 0xd8, 0xff, 0xe0, 0, 0])}},
    ], "at": [10.0, 20.0]});
    // Without the image command (a registry of this family alone): nothing can be opened, and it says why.
    let reg = Arc::new(Registry::from_families(&[super::COMMANDS]));
    let mut s = Session::with_registry(Fakes::new().ports(), reg);
    let r = refuse(&mut s, "doc.drop", items.clone());
    assert_eq!(r.code, "nothing_to_do");
    assert!(r.message.contains("`image.insert` is not available in this session"), "{}", r.message);
    // With it: one call, both images, a 20 unit cascade.
    let fakes = Fakes::new();
    let reg = Arc::new(Registry::from_families(&[super::COMMANDS, STAND_IN]));
    let mut s = Session::with_registry(fakes.ports(), reg);
    SEEN_IMAGES.lock().unwrap().clear();
    let out = run(&mut s, "doc.drop", items);
    assert_eq!(out["images"], 2);
    let seen = SEEN_IMAGES.lock().unwrap().clone();
    assert_eq!(seen, vec![json!({"n": 2, "at": [10.0, 20.0], "offset": 20.0, "first": "one.png"})]);
}

#[test]
fn drop_with_a_scene_png_inserts_it_as_an_image_unless_asked_to_open_it() {
    let (mut a, _) = drawn(2);
    with_png(&mut a);
    let png = unb64(&run(&mut a, "doc.export", json!({"format": "png"}))["bytes"]["base64"]);
    let item = json!({"name": "scene.png", "bytes": {"base64": b64(&png)}});
    let (mut s, _f) = testing::session();
    let out = run(&mut s, "doc.drop", json!({"items": [item.clone()]}));
    assert_eq!((out["images"].as_u64(), out["documents"].as_u64()), (Some(1), Some(0)));
    assert_eq!(live(&s), 1, "the real `image.insert` made an image element");
    assert_eq!(s.store().iter().next().map(|e| e.tag()), Some(drafftink_scene::Kind::Image));
    let out = run(&mut s, "doc.drop", json!({"items": [item], "scene_png": "open", "documents": "replace", "on_unsaved": "discard"}));
    assert_eq!(out["documents"], 1);
    assert_eq!(live(&s), 2, "the scene replaced the image");
}

#[test]
fn drop_with_an_image_that_cannot_be_read_says_why() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "doc.drop", json!({"items": [{"name": "bad.jpg", "bytes": {"base64": b64(&[0xff, 0xd8, 0xff, 0xe0, 0, 0])}}]}));
    assert_eq!(r.code, "nothing_to_do");
    assert!(r.message.contains("none of the 1 dropped files could be opened") && r.message.contains("bad.jpg"), "{}", r.message);
}

// ---- doc.save ----

#[test]
fn save_without_a_target_says_how_to_give_one() {
    let (mut s, _f) = drawn(1);
    let r = refuse(&mut s, "doc.save", json!({}));
    assert_eq!(r.code, "no_save_target");
    assert_eq!(r.message, "This document has no save location yet. Pass `to` (a path, a stored name or a download name).");
}

#[test]
fn save_writes_the_document_marks_it_saved_and_remembers_the_target() {
    let (mut s, f) = drawn(2);
    assert!(s.tab().dirty());
    let out = run(&mut s, "doc.save", json!({"to": {"path": "/work/plan.json"}}));
    assert_eq!(out["saved_to"], json!({"path": "/work/plan.json"}));
    assert_eq!(out["format"], "drafftink");
    let bytes = f.files.bytes(&path("/work/plan.json")).unwrap();
    assert_eq!(out["bytes"].as_u64(), Some(bytes.len() as u64));
    assert!(!s.tab().dirty());
    assert_eq!(s.tab().doc.undo_depth(), 0, "saving is not an undo step");
    assert_eq!(super::save_target(&s), Some(path("/work/plan.json")));
    assert_eq!(super::files::recent(&s), vec![path("/work/plan.json")]);
    // The second save needs no argument and the file reads back.
    rects(&mut s, 1);
    assert!(s.tab().dirty());
    run(&mut s, "doc.save", json!({}));
    assert!(!s.tab().dirty());
    let (mut b, fb) = testing::session();
    fb.files.insert(&path("/x.json"), &f.files.bytes(&path("/work/plan.json")).unwrap(), "application/json");
    run(&mut b, "doc.open", json!({"source": {"path": "/x.json"}}));
    assert_eq!(live(&b), 3);
}

#[test]
fn any_path_back_to_the_saved_content_reads_clean_not_only_undo() {
    let (mut s, _f) = drawn(2);
    run(&mut s, "doc.save", json!({"to": {"path": "/work/plan.json"}}));
    assert!(!s.tab().dirty());
    let first = s.store().iter().next().unwrap().id.clone();
    let before = s.tab().doc.revision();
    // The same edit made again by hand: move away and move back. No undo step is taken.
    run(&mut s, "element.move", json!({"ids": [first.clone()], "dx": 25, "dy": 0}));
    assert!(s.tab().dirty());
    run(&mut s, "element.move", json!({"ids": [first.clone()], "dx": -25, "dy": 0}));
    assert_ne!(s.tab().doc.revision(), before, "the document went through new revisions");
    assert!(!s.tab().dirty(), "the content is the saved content");
    // A different move that does not come back stays dirty, and so does a rename; renaming back is clean.
    run(&mut s, "element.move", json!({"ids": [first], "dx": 3, "dy": 0}));
    assert!(s.tab().dirty());
    run(&mut s, "history.undo", json!({}));
    assert!(!s.tab().dirty());
    let old = name(&s);
    run(&mut s, "doc.rename", json!({"name": "Another"}));
    assert!(s.tab().dirty());
    run(&mut s, "doc.rename", json!({"name": old}));
    assert!(!s.tab().dirty());
}

#[test]
fn moving_away_and_back_gives_the_same_fingerprint_though_the_version_counter_differs() {
    let (mut s, _f) = drawn(1);
    run(&mut s, "doc.save", json!({"to": {"path": "/work/plan.json"}}));
    let id = s.store().iter().next().unwrap().id.clone();
    // Each move raises the element version; the version is not content.
    let print = s.tab().doc.fingerprint();
    run(&mut s, "element.move", json!({"ids": [id.clone()], "dx": 1, "dy": 0}));
    assert_ne!(s.tab().doc.fingerprint(), print, "moving changes the fingerprint");
    run(&mut s, "element.move", json!({"ids": [id], "dx": -1, "dy": 0}));
    assert_eq!(s.tab().doc.fingerprint(), print, "the version counter is not content");
}

/// A files port whose writes fail.
struct BrokenFiles;

impl crate::ports::Files for BrokenFiles {
    fn read(&self, f: &FileRef) -> Result<Vec<u8>, PortError> {
        Err(PortError::NotFound(format!("{f:?}")))
    }
    fn exists(&self, _: &FileRef) -> bool {
        false
    }
    fn write(&mut self, _: &FileRef, _: &[u8], _: &str) -> Result<WriteOutcome, PortError> {
        Err(PortError::Failed("disk full".to_owned()))
    }
}

/// A files port that accepts every write and finishes it later, as the browser does.
struct LaterFiles(u64);

impl crate::ports::Files for LaterFiles {
    fn read(&self, f: &FileRef) -> Result<Vec<u8>, PortError> {
        Err(PortError::NotFound(format!("{f:?}")))
    }
    fn exists(&self, _: &FileRef) -> bool {
        false
    }
    fn write(&mut self, _: &FileRef, _: &[u8], _: &str) -> Result<WriteOutcome, PortError> {
        self.0 += 1;
        Ok(WriteOutcome::Pending(WriteToken(self.0)))
    }
}

#[test]
fn a_failed_write_is_a_refusal_and_the_document_stays_unsaved() {
    let (mut s, _f) = drawn(1);
    s.ports.files = Some(Box::new(BrokenFiles));
    let r = refuse(&mut s, "doc.save", json!({"to": {"path": "/work/plan.json"}}));
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("NOT saved"), "{}", r.message);
    assert!(r.message.contains("disk full"), "{}", r.message);
    assert!(s.tab().dirty(), "a failed write must leave the document dirty");
    assert_eq!(super::save_target(&s), None);
    assert!(super::files::recent(&s).is_empty());
}

#[test]
fn a_failed_write_inside_a_batch_is_named_in_the_note_and_leaves_the_document_dirty() {
    let (mut s, _f) = drawn(1);
    s.ports.files = Some(Box::new(BrokenFiles));
    let steps = [crate::batch::BatchStep::new("doc.save", json!({"to": {"path": "/b.json"}}))];
    let out = s.execute_batch(Origin::Test, &steps, &crate::batch::BatchOpts::default());
    assert!(out.note.as_deref().is_some_and(|n| n.contains("NOT saved")), "{:?}", out.note);
    assert!(s.tab().dirty());
}

#[test]
fn a_write_that_finishes_later_keeps_the_document_dirty_until_it_is_confirmed() {
    let (mut s, _f) = drawn(1);
    s.ports.files = Some(Box::new(LaterFiles(0)));
    let out = run(&mut s, "doc.save", json!({"to": {"path": "/work/plan.json"}}));
    assert_eq!(out["pending_write"], json!([1]));
    assert_eq!(out["changed"], json!(false));
    assert!(s.tab().dirty(), "not saved before the port confirms");
    assert_eq!(super::save_target(&s), None);
    // Edited while the write was open: the confirmation does not make the new edit saved.
    rects(&mut s, 1);
    s.on_write_done(WriteToken(1), Ok(())).unwrap();
    assert!(s.tab().dirty(), "the edit after the snapshot is unsaved");
    assert_eq!(super::save_target(&s), Some(path("/work/plan.json")));
    assert_eq!(super::files::recent(&s), vec![path("/work/plan.json")]);
    // The token is used up.
    assert!(s.on_write_done(WriteToken(1), Ok(())).is_err());
}

#[test]
#[allow(non_snake_case)]
fn exception_N1_a_finished_write_marks_the_tab_saved() {
    let (mut s, _f) = drawn(1);
    s.ports.files = Some(Box::new(LaterFiles(0)));
    run(&mut s, "doc.save", json!({"to": {"path": "/work/plan.json"}}));
    s.on_write_done(WriteToken(1), Ok(())).unwrap();
    assert!(!s.tab().dirty());
}

#[test]
fn a_pending_write_that_fails_leaves_the_document_dirty_and_says_so() {
    let (mut s, _f) = drawn(1);
    s.ports.files = Some(Box::new(LaterFiles(0)));
    run(&mut s, "doc.save", json!({"to": {"path": "/work/plan.json"}}));
    let r = s.on_write_done(WriteToken(1), Err(PortError::Failed("permission denied".to_owned()))).unwrap_err();
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("NOT saved") && r.message.contains("permission denied"), "{}", r.message);
    assert!(s.tab().dirty());
    assert_eq!(super::save_target(&s), None);
}

#[test]
fn replacing_after_on_unsaved_save_does_not_replace_when_the_write_failed() {
    let (mut s, _f) = drawn(2);
    set_target(&mut s, path("/work/plan.json"));
    s.ports.files = Some(Box::new(BrokenFiles));
    let r = refuse(&mut s, "doc.new", json!({"on_unsaved": "save"}));
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("NOT saved"), "{}", r.message);
    assert_eq!(live(&s), 2, "the document must still be there");
    assert!(s.tab().dirty());
}

#[test]
fn replacing_after_on_unsaved_save_does_not_replace_while_the_write_is_open() {
    let (mut s, _f) = drawn(2);
    set_target(&mut s, path("/work/plan.json"));
    s.ports.files = Some(Box::new(LaterFiles(0)));
    let r = refuse(&mut s, "doc.new", json!({"on_unsaved": "save"}));
    assert_eq!(r.code, "io_failed");
    assert_eq!(live(&s), 2);
    s.on_write_done(WriteToken(1), Ok(())).unwrap();
    run(&mut s, "doc.new", json!({}));
    assert_eq!(live(&s), 0, "saved, so the new document may replace it");
}

#[test]
fn save_with_remember_false_leaves_the_target_alone() {
    let (mut s, f) = drawn(1);
    run(&mut s, "doc.save", json!({"to": {"download": "copy.json"}}));
    assert!(f.files.bytes(&FileRef::Download { download: "copy.json".to_owned() }).is_some());
    assert!(super::save_target(&s).is_none(), "a download is not a location");
    assert!(super::files::recent(&s).is_empty());
    run(&mut s, "doc.save", json!({"to": {"path": "/b.json"}, "remember": false}));
    assert!(super::save_target(&s).is_none());
    assert!(!s.tab().dirty());
}

#[test]
fn save_refuses_to_overwrite_a_file_that_is_not_this_documents() {
    let (mut s, f) = drawn(1);
    f.files.insert(&path("/other.json"), b"precious", "application/json");
    let r = refuse(&mut s, "doc.save", json!({"to": {"path": "/other.json"}}));
    assert_eq!(r.code, "would_overwrite");
    assert_eq!(r.message, "`/other.json` already exists and is not this document's file. Pass overwrite=true to replace it.");
    assert_eq!(f.files.bytes(&path("/other.json")).unwrap(), b"precious");
    run(&mut s, "doc.save", json!({"to": {"path": "/other.json"}, "overwrite": true}));
    assert_ne!(f.files.bytes(&path("/other.json")).unwrap(), b"precious");
    // Its own file never asks.
    run(&mut s, "doc.save", json!({"to": {"path": "/other.json"}}));
}

#[test]
fn save_as_excalidraw_writes_a_scene_and_lists_what_it_dropped() {
    let (mut s, f) = drawn(2);
    run(&mut s, "doc.save", json!({"to": {"path": "/a.excalidraw"}, "format": "excalidraw"}));
    let text = String::from_utf8(f.files.bytes(&path("/a.excalidraw")).unwrap()).unwrap();
    assert!(text.contains("\"type\": \"excalidraw\"") || text.contains("\"type\":\"excalidraw\""), "{text}");
}

#[test]
fn save_uses_the_stored_default_target_when_the_shell_turns_it_on() {
    let (mut s, f) = drawn(1);
    run(&mut s, "doc.rename", json!({"name": "Notes"}));
    use_stored_default_target(&mut s, true);
    let out = run(&mut s, "doc.save", json!({}));
    assert_eq!(out["saved_to"], json!({"stored": "Notes"}));
    assert!(f.storage.bytes(Ns::Documents, "Notes").is_some());
    assert_eq!(super::save_target(&s), Some(FileRef::Stored { stored: "Notes".to_owned() }));
}

/// A storage whose document writes finish later, as IndexedDB does in the browser.
struct LaterStorage {
    inner: crate::ports::fake::FakeStorage,
    next: u64,
}

impl crate::ports::Storage for LaterStorage {
    fn get(&self, ns: Ns, key: &str) -> Result<Option<Vec<u8>>, PortError> {
        self.inner.get(ns, key)
    }
    fn put(&mut self, ns: Ns, key: &str, bytes: &[u8]) -> Result<(), PortError> {
        self.inner.put(ns, key, bytes)
    }
    fn put_document(&mut self, _: &str, _: &[u8]) -> Result<crate::ports::PutOutcome, PortError> {
        self.next += 1;
        Ok(crate::ports::PutOutcome::Pending(WriteToken(self.next)))
    }
    fn delete(&mut self, ns: Ns, key: &str) -> Result<(), PortError> {
        self.inner.delete(ns, key)
    }
    fn list(&self, ns: Ns) -> Result<Vec<crate::ports::StoredInfo>, PortError> {
        self.inner.list(ns)
    }
}

fn with_later_storage(s: &mut Session, f: &Fakes) {
    s.ports.storage = Some(Box::new(LaterStorage { inner: f.storage.clone(), next: 0 }));
    use_stored_default_target(s, true);
}

#[test]
fn a_stored_save_that_finishes_later_keeps_the_document_dirty_until_it_is_confirmed() {
    let (mut s, f) = drawn(1);
    run(&mut s, "doc.rename", json!({"name": "Notes"}));
    with_later_storage(&mut s, &f);
    let out = run(&mut s, "doc.save", json!({}));
    assert_eq!(out["pending_write"], json!([1]));
    assert_eq!(out["changed"], json!(false));
    assert!(s.tab().dirty(), "not saved before the storage confirms");
    assert_eq!(super::save_target(&s), None);
    assert!(super::files::recent(&s).is_empty());
    s.on_write_done(WriteToken(1), Ok(())).unwrap();
    assert!(!s.tab().dirty());
    assert_eq!(super::save_target(&s), Some(FileRef::Stored { stored: "Notes".to_owned() }));
    assert_eq!(super::files::recent(&s), vec![FileRef::Stored { stored: "Notes".to_owned() }]);
}

#[test]
fn a_stored_save_that_fails_later_leaves_the_document_dirty_and_says_so() {
    let (mut s, f) = drawn(1);
    with_later_storage(&mut s, &f);
    run(&mut s, "doc.save", json!({}));
    let r = s.on_write_done(WriteToken(1), Err(PortError::Failed("quota exceeded".to_owned()))).unwrap_err();
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("NOT saved") && r.message.contains("quota exceeded"), "{}", r.message);
    assert!(s.tab().dirty());
    assert_eq!(super::save_target(&s), None);
}

#[test]
fn closing_a_tab_with_a_pending_stored_save_refuses_until_it_is_confirmed() {
    let (mut s, f) = drawn(1);
    run(&mut s, "tab.new", json!({}));
    run(&mut s, "tab.switch", json!({"tab": 0}));
    with_later_storage(&mut s, &f);
    let r = refuse(&mut s, "tab.close", json!({"on_unsaved": "save"}));
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("not confirmed yet"), "{}", r.message);
    assert_eq!(s.tabs.len(), 2, "the tab with the unsaved work must stay");
    s.on_write_done(WriteToken(1), Ok(())).unwrap();
    run(&mut s, "tab.close", json!({"on_unsaved": "save"}));
}

#[test]
fn save_after_a_rename_in_one_batch_writes_the_new_name() {
    let (mut s, f) = drawn(1);
    let steps = [
        crate::batch::BatchStep::new("doc.rename", json!({"name": "Final"})),
        crate::batch::BatchStep::new("doc.save", json!({"to": {"stored": "Final"}})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &crate::batch::BatchOpts::default());
    assert!(out.failed.is_none(), "{:?}", out.failed);
    let text = String::from_utf8(f.storage.bytes(Ns::Documents, "Final").unwrap()).unwrap();
    assert!(text.contains("\"Final\""), "{text}");
}

#[test]
fn a_batch_that_fails_after_a_save_writes_nothing() {
    let (mut s, f) = drawn(1);
    let steps = [
        crate::batch::BatchStep::new("doc.save", json!({"to": {"path": "/never.json"}})),
        crate::batch::BatchStep::new("doc.rename", json!({"name": ""})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &crate::batch::BatchOpts::default());
    assert!(out.failed.is_some());
    assert!(f.files.bytes(&path("/never.json")).is_none());
}

#[test]
fn save_needs_the_port_of_its_target() {
    let mut s = testing::headless_session();
    rects(&mut s, 1);
    let r = refuse(&mut s, "doc.save", json!({"to": {"path": "/a.json"}}));
    assert_eq!(r.code, "port_unavailable");
    let r = refuse(&mut s, "doc.save", json!({"to": {"stored": "a"}}));
    assert_eq!(r.code, "port_unavailable");
    assert!(r.message.contains("storage port missing"), "{}", r.message);
}

#[test]
fn save_is_allowed_in_view_only_mode_and_has_the_key() {
    let (mut s, _f) = drawn(1);
    s.view_only = true;
    run(&mut s, "doc.save", json!({"to": {"path": "/v.json"}}));
    let spec = s.spec("doc.save").unwrap();
    assert_eq!(spec.shortcuts[0].keys, ["Mod+S"]);
}

// ---- doc.autosave ----

#[test]
fn autosave_needs_storage() {
    let mut s = testing::headless_session();
    let r = refuse(&mut s, "doc.autosave", json!({}));
    assert_eq!(r.code, "port_unavailable");
    assert_eq!(r.message, "`doc.autosave` needs persistent storage, which this session does not provide (storage port missing).");
    assert_eq!(s.availability("doc.autosave").unwrap_err().message, r.message);
}

#[test]
fn autosave_writes_the_workspace_only_for_documents_that_changed() {
    let (mut s, f) = drawn(2);
    let first = run(&mut s, "doc.autosave", json!({}));
    assert_eq!((first["saved"].clone(), first["tabs"].clone()), (json!(true), json!(1)));
    assert!(f.storage.bytes(Ns::Autosave, "workspace").is_some());
    let again = run(&mut s, "doc.autosave", json!({}));
    assert_eq!(again["saved"], false);
    rects(&mut s, 1);
    assert_eq!(run(&mut s, "doc.autosave", json!({}))["saved"], true);
}

#[test]
fn autosave_runs_during_a_drag_and_in_view_only_mode() {
    let (mut s, _f) = drawn(1);
    s.view_only = true;
    run(&mut s, "doc.autosave", json!({}));
    let spec = s.spec("doc.autosave").unwrap();
    assert_eq!((spec.while_gesture, spec.view_only, spec.while_text_edit), (Gate::Allowed, Gate::Allowed, TextGate::Allowed));
}

// ---- doc.rename ----

#[test]
fn rename_sets_the_name_in_one_undo_step() {
    let (mut s, _f) = drawn(1);
    s.tab_mut().doc.clear_history();
    let out = run(&mut s, "doc.rename", json!({"name": "  Architecture  "}));
    assert_eq!(out["name"], "Architecture");
    assert_eq!(out["undo_label"], "Rename document");
    assert_eq!(name(&s), "Architecture");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(name(&s), "Untitled");
}

#[test]
fn rename_to_the_same_name_changes_nothing() {
    let (mut s, _f) = testing::session();
    run(&mut s, "doc.rename", json!({"name": "A"}));
    let out = run(&mut s, "doc.rename", json!({"name": "A"}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), 1);
}

#[test]
fn rename_refuses_an_empty_name_and_a_line_break() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "doc.rename", json!({"name": "  "}));
    assert_eq!(r.code, "empty_name");
    assert_eq!(r.message, "The name is empty. Give a name of 1 to 256 characters.");
    assert_eq!(refuse(&mut s, "doc.rename", json!({"name": "a\nb"})).code, "bad_args");
    let out = run(&mut s, "doc.rename", json!({"name": "x".repeat(300)}));
    assert_eq!(out["name"].as_str().map(|n| n.chars().count()), Some(drafftink_scene::limits::MAX_NAME_CHARS));
}

// ---- doc.set_background ----

#[test]
fn set_background_is_saved_undoable_and_accepts_palette_names() {
    let (mut s, _f) = testing::session();
    let out = run(&mut s, "doc.set_background", json!({"color": "#FAFAFA"}));
    assert_eq!((out["background"].as_str(), out["changed"].as_bool()), (Some("#fafafa"), Some(true)));
    assert_eq!(s.store().meta().background.to_hex(), "#fafafa");
    s.undo_steps(1).unwrap();
    assert_eq!(s.store().meta().background, drafftink_scene::Color::WHITE);
    run(&mut s, "doc.set_background", json!({"color": "transparent"}));
    assert_eq!(s.store().meta().background, drafftink_scene::Color::TRANSPARENT);
    run(&mut s, "doc.set_background", json!({"color": "gray-100"}));
    assert_ne!(s.store().meta().background, drafftink_scene::Color::TRANSPARENT);
    rects(&mut s, 1);
    let json = String::from_utf8(native(&mut s)).unwrap();
    assert!(json.contains("\"background\""), "the colour is part of the file");
}

#[test]
fn set_background_to_the_same_colour_is_ok_and_a_bad_colour_is_refused() {
    let (mut s, _f) = testing::session();
    let out = run(&mut s, "doc.set_background", json!({"color": "#ffffff"}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), 0);
    let r = refuse(&mut s, "doc.set_background", json!({"color": "not-a-colour"}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("is not a colour"), "{}", r.message);
}

// ---- doc.clear ----

#[test]
fn clear_is_unavailable_on_an_empty_document_with_the_catalog_text() {
    let (mut s, _f) = testing::session();
    let a = s.availability("doc.clear").unwrap_err();
    assert_eq!(a.code, "empty_document");
    assert_eq!(a.message, "The document is already empty, so there is nothing to clear.");
    assert_eq!(refuse(&mut s, "doc.clear", json!({})).message, a.message);
}

#[test]
fn clear_deletes_everything_locked_ones_too_and_undoes_in_one_step() {
    let (mut s, _f) = drawn(3);
    let first = s.store().iter().next().unwrap().id.clone();
    s.tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| tx.update(&first, |e| e.locked = true)).unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    let frame = testing::add(&mut s, ElementKind::Frame(FrameData::default()), -50.0, -50.0, 600.0, 200.0).unwrap();
    testing::select(&mut s, &[frame]);
    s.tab_mut().doc.clear_history();
    let out = run(&mut s, "doc.clear", json!({}));
    assert_eq!(out["removed"], 4);
    assert_eq!(live(&s), 0);
    assert!(s.selection().is_empty());
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert_eq!(out["undo_label"], "Clear document");
    s.undo_steps(1).unwrap();
    assert_eq!(live(&s), 4);
    assert!(s.element(&first).unwrap().locked, "undo restores the lock");
}

#[test]
fn clear_has_its_key_and_is_refused_in_view_only_mode() {
    let (mut s, _f) = drawn(1);
    assert_eq!(s.spec("doc.clear").unwrap().shortcuts[0].keys, ["Mod+Delete"]);
    s.view_only = true;
    assert_eq!(refuse(&mut s, "doc.clear", json!({})).code, "view_only");
}

// ---- doc.export ----

#[test]
fn export_svg_inline_embeds_the_scene_and_changes_nothing() {
    let (mut s, _f) = drawn(2);
    let before = s.fingerprint();
    let out = run(&mut s, "doc.export", json!({"format": "svg"}));
    assert_eq!((out["format"].as_str(), out["mime"].as_str(), out["elements"].as_u64()), (Some("svg"), Some("image/svg+xml"), Some(2)));
    assert_eq!(out["scene_embedded"], true);
    let svg = String::from_utf8(unb64(&out["bytes"]["base64"])).unwrap();
    assert!(svg.starts_with("<svg") && svg.contains("drafftink-scene"), "{}", &svg[..svg.len().min(200)]);
    assert_eq!(out["size"].as_u64(), Some(svg.len() as u64));
    assert_eq!(s.fingerprint(), before);
    let off = run(&mut s, "doc.export", json!({"format": "svg", "embed_scene": false}));
    assert!(!String::from_utf8(unb64(&off["bytes"]["base64"])).unwrap().contains("drafftink-scene"));
}

#[test]
fn export_to_a_file_that_cannot_be_written_is_refused_with_the_reason_and_writes_nothing() {
    let (mut s, f) = drawn(2);
    f.files.fail_writes(true);
    let target = path("/out/plan.json");
    let args = json!({"format": "json", "scope": "all", "to": {"path": "/out/plan.json"}});
    let r = refuse(&mut s, "doc.export", args.clone());
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("did NOT happen") && r.message.contains("the disk is full"), "{}", r.message);
    assert!(r.message.contains("Nothing was written"), "{}", r.message);
    assert!(f.files.bytes(&target).is_none(), "nothing reached the port");
    // Once the port works the same call succeeds and reports where it wrote.
    f.files.fail_writes(false);
    let out = run(&mut s, "doc.export", args);
    assert_eq!(out["saved_to"], json!({"path": "/out/plan.json"}));
    assert!(f.files.bytes(&target).is_some());
}

/// An svg export of `args` over two texts (GelPen Serif and Vanilla Extract, heavy) in a session with the real typesetter.
fn svg_of(args: Value, texts: &[(&str, &str)]) -> (String, Value) {
    let (mut s, _f) = testing::session_typeset();
    for (i, (family, weight)) in texts.iter().enumerate() {
        let at = [10.0, 10.0 + 40.0 * i as f64];
        run(&mut s, "element.create", json!({"kind": "text", "text": "hello", "at": at,
            "style": {"font_family": family, "font_weight": weight, "stroke_color": "#102030"}}));
    }
    let mut a = args;
    a["format"] = json!("svg");
    a["scope"] = json!("all");
    let out = run(&mut s, "doc.export", a);
    (String::from_utf8(unb64(&out["bytes"]["base64"])).unwrap(), out)
}

#[test]
fn export_svg_embeds_the_font_files_of_the_texts_it_draws_and_no_others() {
    let (svg, _) = svg_of(json!({}), &[("gelpen_serif", "regular"), ("gelpen_serif", "heavy")]);
    assert_eq!(svg.matches("@font-face{font-family:'GelPen Serif';font-weight:").count(), 2, "regular and heavy");
    assert!(svg.contains("font-weight:400;src:url(data:font/ttf;base64,"));
    assert!(svg.contains("font-weight:700;src:url(data:font/ttf;base64,"));
    assert!(!svg.contains("font-family:'GelPen';"), "a family no text uses is not embedded");
    assert!(!svg.contains("Vanilla Extract'"));
    let (plain, _) = svg_of(json!({}), &[("vanilla_extract", "regular")]);
    assert_eq!(plain.matches("@font-face").count(), 1);
    assert!(plain.contains("font-family:'Vanilla Extract';font-weight:400;"));
}

#[test]
fn export_svg_dark_uses_the_canvas_palette_and_no_longer_warns() {
    let (light, lo) = svg_of(json!({}), &[("gelpen", "regular")]);
    let (dark, dout) = svg_of(json!({"dark": true}), &[("gelpen", "regular")]);
    assert_eq!(dout["warnings"], json!([]), "dark is supported for svg now");
    assert_eq!(lo["warnings"], json!([]));
    // The same mapping as the dark PNG: white paper becomes #121212, the ink #102030 is mapped too.
    assert!(light.contains("fill=\"#ffffff\""), "{light}");
    assert!(dark.contains("fill=\"#121212\""), "{dark}");
    let ink = drafftink_scene::Color::rgb(0x10, 0x20, 0x30);
    assert!(light.contains("fill=\"#102030\"") && !dark.contains("fill=\"#102030\""));
    assert!(dark.contains(&format!("fill=\"{}\"", &ink.for_dark_theme().to_hex()[..7])), "{dark}");
    // The scene inside stays as drawn, so the picture opens as the same document.
    let scene = drafftink_io::svg::extract_scene(&dark).unwrap().unwrap();
    assert!(scene.contains("#102030") && !scene.contains(&ink.for_dark_theme().to_hex()[..7]), "{scene}");
}

#[test]
fn export_draws_inline_math_in_svg_and_keeps_the_source_in_excalidraw_and_json() {
    let (mut s, _f) = testing::session_typeset();
    run(&mut s, "element.create", json!({"kind": "text", "text": "Energy $E = mc^2$ here", "at": [10, 10]}));
    let svg = run(&mut s, "doc.export", json!({"format": "svg", "scope": "all"}));
    let svg = String::from_utf8(unb64(&svg["bytes"]["base64"])).unwrap();
    assert!(svg.contains("data-text=\"Energy $E = mc^2$ here\""), "the source is on the group");
    assert!(!svg.contains("<text "), "the glyphs are outlines, formula included");
    for fmt in ["excalidraw", "json"] {
        let out = run(&mut s, "doc.export", json!({"format": fmt, "scope": "all"}));
        let bytes = String::from_utf8(unb64(&out["bytes"]["base64"])).unwrap();
        assert!(bytes.contains("Energy $E = mc^2$ here"), "{fmt}: the source text with its delimiters");
        let (mut t, _) = testing::session();
        open_inline(&mut t, bytes.as_bytes(), json!({})).unwrap();
        let e = t.store().iter().next().unwrap();
        assert_eq!(e.text_data().unwrap().content, "Energy $E = mc^2$ here", "{fmt}");
    }
}

#[test]
fn export_json_and_excalidraw_round_trip_through_open() {
    let (mut s, _f) = drawn(3);
    for fmt in ["json", "excalidraw"] {
        let out = run(&mut s, "doc.export", json!({"format": fmt, "scope": "all"}));
        let (mut t, _) = testing::session();
        open_inline(&mut t, &unb64(&out["bytes"]["base64"]), json!({})).unwrap();
        assert_eq!(live(&t), 3, "{fmt}");
    }
}

#[test]
fn export_png_goes_through_the_renderer_with_the_scene_embedded() {
    let (mut s, _f) = drawn(2);
    let r = with_png(&mut s);
    let out = run(&mut s, "doc.export", json!({"format": "png", "scale": 2, "padding": 10, "dark": true}));
    assert_eq!(out["scene_embedded"], true);
    let png = unb64(&out["bytes"]["base64"]);
    assert!(drafftink_io::png_meta::extract_scene(&png).unwrap().is_some());
    let req = r.seen.lock().unwrap().last().cloned().unwrap();
    // Two 100x60 rectangles 150 apart: content 250 x 60, plus 10 on each side, times 2.
    assert_eq!(req.size_px, (540, 160));
    assert_eq!((out["width"].as_u64(), out["height"].as_u64()), (Some(540), Some(160)));
    assert!(req.background && req.dark && !req.overlay);
    assert_eq!(req.region, (-10.0, -10.0, 270.0, 80.0));
    let plain = run(&mut s, "doc.export", json!({"embed_scene": false, "background": false}));
    assert_eq!(plain["scene_embedded"], false);
    assert!(drafftink_io::png_meta::extract_scene(&unb64(&plain["bytes"]["base64"])).unwrap().is_none());
    assert!(!r.seen.lock().unwrap().last().unwrap().background);
}

#[test]
fn export_png_with_a_renderer_that_returns_no_png_warns_and_still_returns_the_bytes() {
    let (mut s, _f) = drawn(1);
    let out = run(&mut s, "doc.export", json!({}));
    assert_eq!(out["scene_embedded"], false);
    assert!(out["warnings"][0].as_str().unwrap().contains("could not be embedded"), "{out}");
}

#[test]
fn export_png_without_a_display_names_the_other_formats() {
    let mut s = testing::headless_session();
    rects(&mut s, 1);
    let r = refuse(&mut s, "doc.export", json!({"format": "png"}));
    assert_eq!(r.code, "port_unavailable");
    assert_eq!(
        r.message,
        "`doc.export` with format png needs the renderer, which this session does not provide (headless without a GPU). Use format svg or json."
    );
    run(&mut s, "doc.export", json!({"format": "svg"}));
}

#[test]
fn export_of_an_empty_document_and_of_an_empty_selection_refuse() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "doc.export", json!({"format": "svg"}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "Nothing to export: the document has no elements. Draw something first, or pass `ids`.");
    rects(&mut s, 1);
    let r = refuse(&mut s, "doc.export", json!({"format": "svg", "scope": "selection"}));
    assert_eq!(r.code, "no_target");
}

#[test]
fn export_auto_takes_the_selection_if_there_is_one_else_everything() {
    let (mut s, _f) = drawn(3);
    assert_eq!(run(&mut s, "doc.export", json!({"format": "json"}))["elements"], 3);
    let first = s.store().iter().next().unwrap().id.clone();
    testing::select(&mut s, std::slice::from_ref(&first));
    assert_eq!(run(&mut s, "doc.export", json!({"format": "json"}))["elements"], 1);
    assert_eq!(run(&mut s, "doc.export", json!({"format": "json", "scope": "all"}))["elements"], 3);
    let ids = s.store().iter().skip(1).map(|e| e.id.as_str().to_owned()).collect::<Vec<_>>();
    assert_eq!(run(&mut s, "doc.export", json!({"format": "json", "scope": "ids", "ids": ids}))["elements"], 2);
    assert_eq!(refuse(&mut s, "doc.export", json!({"scope": "ids"})).code, "bad_args");
    assert_eq!(refuse(&mut s, "doc.export", json!({"scope": "ids", "ids": ["nope"]})).code, "not_found");
}

#[test]
fn export_frame_takes_the_frame_with_its_children() {
    let (mut s, _f) = drawn(1);
    let frame = testing::add(&mut s, ElementKind::Frame(FrameData::default()), 0.0, 0.0, 500.0, 300.0).unwrap();
    let child = testing::add_rect(&mut s, 10.0, 10.0, 40.0, 40.0).unwrap();
    s.tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| tx.update(&child, |e| e.frame_id = Some(frame.clone()))).unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    let out = run(&mut s, "doc.export", json!({"format": "json", "scope": "frame", "frame": frame.as_str()}));
    assert_eq!(out["elements"], 2);
    let r = refuse(&mut s, "doc.export", json!({"scope": "frame", "frame": child.as_str()}));
    assert_eq!(r.code, "wrong_kind");
    assert_eq!(refuse(&mut s, "doc.export", json!({"scope": "frame"})).code, "bad_args");
}

#[test]
fn export_to_a_file_writes_through_the_files_port() {
    let (mut s, f) = drawn(1);
    let out = run(&mut s, "doc.export", json!({"format": "svg", "to": {"download": "pic.svg"}}));
    assert_eq!(out["saved_to"], json!({"download": "pic.svg"}));
    assert!(out.get("bytes").is_none());
    let bytes = f.files.bytes(&FileRef::Download { download: "pic.svg".to_owned() }).unwrap();
    assert!(bytes.starts_with(b"<svg"));
    let mut h = testing::headless_session();
    rects(&mut h, 1);
    assert_eq!(refuse(&mut h, "doc.export", json!({"format": "svg", "to": {"path": "/x.svg"}})).code, "port_unavailable");
}

#[test]
fn export_checks_its_numbers() {
    let (mut s, _f) = drawn(1);
    with_png(&mut s);
    assert_eq!(refuse(&mut s, "doc.export", json!({"scale": 9})).code, "bad_args");
    assert_eq!(refuse(&mut s, "doc.export", json!({"scale": 0.1})).code, "bad_args");
    assert_eq!(refuse(&mut s, "doc.export", json!({"padding": -1})).code, "bad_args");
    testing::add_rect(&mut s, 0.0, 0.0, 9000.0, 10.0).unwrap();
    let r = refuse(&mut s, "doc.export", json!({"scale": 4}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("Lower `scale`"), "{}", r.message);
}

#[test]
fn export_defaults_come_from_the_settings() {
    let (mut s, _f) = drawn(1);
    let r = with_png(&mut s);
    run(&mut s, "settings.set", json!({"key": "export.scale", "value": 1}));
    run(&mut s, "settings.set", json!({"key": "export.padding", "value": 0}));
    run(&mut s, "settings.set", json!({"key": "export.dark", "value": true}));
    run(&mut s, "doc.export", json!({}));
    let req = r.seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!((req.size_px, req.dark), ((100, 60), true));
}

#[test]
fn export_has_its_key() {
    let s = testing::headless_session();
    let b = &s.spec("doc.export").unwrap().shortcuts[0];
    assert_eq!((b.keys, b.args), (["Mod+E"].as_slice(), r#"{"format":"png"}"#));
}

// ---- doc.list ----

#[test]
fn list_shows_recent_stored_and_autosaved_documents() {
    let (mut s, f) = drawn(1);
    assert!(run(&mut s, "doc.list", json!({}))["documents"].as_array().unwrap().is_empty());
    run(&mut s, "doc.save", json!({"to": {"stored": "one"}}));
    run(&mut s, "doc.save", json!({"to": {"path": "/two.json"}}));
    let recent = run(&mut s, "doc.list", json!({}));
    let docs = recent["documents"].as_array().unwrap();
    assert_eq!(docs[0]["name"], "two.json");
    assert_eq!(docs[0]["ref"], json!({"path": "/two.json"}));
    assert_eq!(docs[1]["ref"], json!({"stored": "one"}));
    assert!(docs[1]["size"].as_u64().unwrap() > 0);
    assert!(docs[0]["size"].is_null());
    let stored = run(&mut s, "doc.list", json!({"from": "stored"}));
    assert_eq!(stored["documents"].as_array().unwrap().len(), 1);
    assert!(run(&mut s, "doc.list", json!({"from": "autosave"}))["documents"].as_array().unwrap().is_empty());
    run(&mut s, "doc.autosave", json!({}));
    let auto = run(&mut s, "doc.list", json!({"from": "autosave"}));
    assert_eq!(auto["documents"][0]["ref"], json!({"stored": "autosave"}));
    let _ = f;
    let mut h = testing::headless_session();
    assert_eq!(refuse(&mut h, "doc.list", json!({"from": "stored"})).code, "port_unavailable");
    run(&mut h, "doc.list", json!({}));
}

#[test]
fn the_recent_list_keeps_ten_and_moves_a_reopened_file_to_the_top() {
    let (mut s, _f) = drawn(1);
    for i in 0..12 {
        run(&mut s, "doc.save", json!({"to": {"path": format!("/f{i}.json")}}));
    }
    let list = super::files::recent(&s);
    assert_eq!(list.len(), 10);
    assert_eq!(list[0], path("/f11.json"));
    run(&mut s, "doc.save", json!({"to": {"path": "/f5.json"}, "overwrite": true}));
    assert_eq!(super::files::recent(&s)[0], path("/f5.json"));
    assert_eq!(super::files::recent(&s).len(), 10);
}

// ---- facts and registry ----

#[test]
fn every_doc_command_is_registered_with_the_expected_undo_class() {
    let s = testing::headless_session();
    let want = [
        ("doc.new", Undo::NoUndo),
        ("doc.open", Undo::Undoable),
        ("doc.drop", Undo::Undoable),
        ("doc.save", Undo::NoUndo),
        ("doc.rename", Undo::Undoable),
        ("doc.set_background", Undo::Undoable),
        ("doc.export", Undo::NoUndo),
        ("doc.clear", Undo::Undoable),
        ("doc.show_intro", Undo::NoUndo),
        ("doc.autosave", Undo::NoUndo),
        ("doc.list", Undo::NoUndo),
    ];
    for (id, undo) in want {
        assert_eq!(s.spec(id).unwrap_or_else(|| panic!("{id} is not registered")).undo, undo, "{id}");
    }
    assert_eq!(super::COMMANDS.len(), want.len());
}

#[test]
fn opening_the_intro_twice_in_a_row_keeps_the_session_consistent() {
    let (mut s, _f) = testing::session();
    run(&mut s, "doc.show_intro", json!({}));
    run(&mut s, "doc.show_intro", json!({}));
    assert_eq!(s.tabs.len(), 1);
    assert!(s.take_dirty().is_some());
    assert!(s.tab().doc.undo_depth() == 0);
}

// ---- hostile input with every port present ----

/// The registry-wide hostile test runs without storage, files and renderer, so most of these commands
/// refuse early there. This one gives them every port and a drawing.
#[test]
fn hostile_arguments_never_break_a_doc_or_library_command_that_has_every_port() {
    let values = [
        json!(null),
        json!(1e308),
        json!(-1),
        json!(""),
        json!("x".repeat(5000)),
        json!("../../etc/passwd"),
        json!([]),
        json!([1e308, -1e308]),
        json!({}),
        json!({"path": "/nope"}),
        json!({"base64": "AAAA"}),
        json!({"base64": "!!"}),
        json!({"stored": "autosave"}),
        json!({"download": ""}),
    ];
    for spec in super::COMMANDS.iter().chain(crate::cmd::library::COMMANDS) {
        let mut keys = vec!["ids".to_owned(), "x".to_owned()];
        keys.extend(crate::args::schema_keys(&(spec.args)()));
        for key in &keys {
            for v in &values {
                let (mut s, _f) = drawn(3);
                with_png(&mut s);
                let first = s.store().iter().next().unwrap().id.clone();
                testing::select(&mut s, &[first]);
                run(&mut s, "library.add", json!({"name": "seed"}));
                let before = s.fingerprint();
                match s.execute_from(Origin::Test, spec.id, json!({ key.clone(): v.clone() })) {
                    Ok(_) => {}
                    Err(r) => {
                        assert_ne!(r.code, "internal", "{} {key}={v}: {}", spec.id, r.message);
                        assert_eq!(r.command, spec.id);
                        assert!(
                            crate::refusal::is_shared_code(r.code) || spec.refusals.contains(&r.code),
                            "{} {key}={v}: undeclared code {}",
                            spec.id,
                            r.code
                        );
                        assert_eq!(s.fingerprint(), before, "{} {key}={v} refused with `{}` but changed state", spec.id, r.code);
                    }
                }
            }
        }
    }
}
