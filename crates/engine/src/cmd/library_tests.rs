//! Tests of the `library` family. No UI: a session over the fake ports, with the storage shared between
//! sessions to check that the library survives a restart.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use drafftink_scene::{ElementKind, SceneRead};
use serde_json::{Value, json};

use super::store::{INDEX_KEY, item_key};
use crate::ports::fake::Fakes;
use crate::ports::{FileRef, Ns, PortError, RenderRequest, Renderer, SceneView, Storage};
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::services::Services;
use crate::session::Session;
use crate::spec::Undo;
use crate::testing;

const LIB_V2: &[u8] = include_bytes!("../../../../tests/fixtures/excalidraw/fixture_library.excalidrawlib");
const LIB_V1: &[u8] = include_bytes!("../../../../tests/fixtures/excalidraw/v1_library.excalidrawlib");
const LIB_GROUPED: &[u8] = include_bytes!("../../../../tests/fixtures/excalidraw/grouped_library.excalidrawlib");
const NOT_A_LIB: &[u8] = include_bytes!("../../../../tests/fixtures/excalidraw/not_a_library.excalidrawlib");

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

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn import(s: &mut Session, bytes: &[u8], extra: Value) -> Value {
    let mut args = json!({"source": {"base64": b64(bytes), "name": "shapes.excalidrawlib"}});
    if let (Value::Object(a), Value::Object(e)) = (&mut args, extra) {
        a.extend(e);
    }
    run(s, "library.import", args)
}

/// A session with a group of two rectangles with a label, and an arrow between them. The group is selected.
fn drawn() -> (Session, Fakes) {
    let (mut s, f) = testing::session();
    let a = testing::add_rect(&mut s, 100.0, 100.0, 80.0, 40.0).unwrap();
    let b = testing::add_rect(&mut s, 300.0, 100.0, 80.0, 40.0).unwrap();
    testing::select(&mut s, &[a, b]);
    (s, f)
}

fn ids_of(v: &Value) -> Vec<String> {
    v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_owned()).collect()
}

fn count(s: &Session) -> usize {
    s.store().len()
}

// ---- needs storage ----

#[test]
fn every_library_command_needs_storage_and_says_so_before_running() {
    let mut s = testing::headless_session();
    for id in ["library.add", "library.insert", "library.remove", "library.rename", "library.list", "library.thumbnail", "library.import", "library.export"] {
        let a = s.availability(id).unwrap_err();
        assert_eq!(a.code, "port_unavailable", "{id}");
        assert_eq!(
            a.message,
            format!("`{id}` needs persistent storage, which this session does not provide (storage port missing). The shape library is kept in storage."),
        );
        let r = refuse(&mut s, id, json!({}));
        assert_eq!(r.code, "port_unavailable", "{id}");
    }
}

// ---- library.add ----

#[test]
fn add_stores_the_selection_as_an_item_at_the_origin() {
    let (mut s, f) = drawn();
    let out = run(&mut s, "library.add", json!({"name": "  Two boxes "}));
    let item = &out["item"];
    assert_eq!((item["name"].as_str(), item["element_count"].as_u64()), (Some("Two boxes"), Some(2)));
    assert_eq!(item["bounds"], json!([0.0, 0.0, 280.0, 40.0]));
    assert_eq!(item["created"], 1_730_000_000_000_u64);
    // The document is untouched and nothing is undoable.
    assert_eq!(count(&s), 2);
    assert_eq!(s.tab().doc.undo_depth(), 0);
    // Storage holds the index and the item.
    let id = item["id"].as_str().unwrap();
    let index: Value = serde_json::from_slice(&f.storage.bytes(Ns::Library, INDEX_KEY).unwrap()).unwrap();
    assert_eq!(index["order"], json!([id]));
    assert!(f.storage.bytes(Ns::Library, &item_key(id)).is_some());
}

#[test]
fn add_takes_bound_text_with_its_container_and_the_children_of_a_frame() {
    let (mut s, _f) = testing::session();
    let frame = testing::add(&mut s, ElementKind::Frame(Default::default()), 0.0, 0.0, 400.0, 300.0).unwrap();
    let inner = testing::add_rect(&mut s, 20.0, 20.0, 100.0, 50.0).unwrap();
    s.tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| {
        use drafftink_scene::SceneTx;
        tx.update(&inner, |e| e.frame_id = Some(frame.clone()))
    }).unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    let out = run(&mut s, "library.add", json!({"ids": [frame.as_str()]}));
    assert_eq!(out["item"]["element_count"], 2);
}

#[test]
fn add_refuses_without_a_target_and_with_a_bad_name() {
    let (mut s, _f) = testing::session();
    let a = s.availability("library.add").unwrap_err();
    assert_eq!(a.code, "no_target");
    assert_eq!(refuse(&mut s, "library.add", json!({})).code, "no_target");
    let (mut s, _f) = drawn();
    let r = refuse(&mut s, "library.add", json!({"name": " "}));
    assert_eq!(r.code, "empty_name");
    assert_eq!(refuse(&mut s, "library.add", json!({"ids": ["ghost"]})).code, "not_found");
}

#[test]
fn add_allows_locked_elements_and_view_only_mode() {
    let (mut s, _f) = drawn();
    s.view_only = true;
    run(&mut s, "library.add", json!({}));
    assert_eq!(s.spec("library.add").unwrap().undo, Undo::NoUndo);
}

// ---- library.list ----

#[test]
fn list_shows_items_and_filters_by_search() {
    let (mut s, _f) = drawn();
    run(&mut s, "library.add", json!({"name": "Alpha box"}));
    run(&mut s, "library.add", json!({"name": "Beta"}));
    let all = run(&mut s, "library.list", json!({}));
    assert_eq!(all["items"].as_array().unwrap().len(), 2);
    assert_eq!(all["items"][0]["name"], "Alpha box");
    let found = run(&mut s, "library.list", json!({"search": "ALPHA"}));
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    assert!(run(&mut s, "library.list", json!({"search": "zzz"}))["items"].as_array().unwrap().is_empty());
}

#[test]
fn the_library_survives_a_new_session_over_the_same_storage() {
    let (mut s, f) = drawn();
    run(&mut s, "library.add", json!({"name": "Kept"}));
    let mut again = Session::with_services(f.ports(), Services::fake());
    let list = run(&mut again, "library.list", json!({}));
    assert_eq!(list["items"][0]["name"], "Kept");
    assert_eq!(list["items"][0]["element_count"], 2);
}

#[test]
fn a_damaged_item_in_storage_is_skipped_with_a_warning() {
    let (mut s, f) = drawn();
    let id = run(&mut s, "library.add", json!({"name": "Good"}))["item"]["id"].as_str().unwrap().to_owned();
    let id2 = run(&mut s, "library.add", json!({"name": "Bad"}))["item"]["id"].as_str().unwrap().to_owned();
    f.storage.clone().put(Ns::Library, &item_key(&id2), b"{not json").unwrap();
    let mut again = Session::with_services(f.ports(), Services::fake());
    let list = run(&mut again, "library.list", json!({}));
    assert_eq!(ids_of(&list), vec![id]);
    assert!(list["warnings"][0].as_str().unwrap().contains(&id2), "{list}");
}

// ---- library.thumbnail ----

/// A renderer that keeps what it was asked for and answers with `answer`.
#[derive(Clone)]
struct Recorder {
    asked: Arc<Mutex<Vec<RenderRequest>>>,
    answer: Result<Vec<u8>, PortError>,
}

impl Recorder {
    fn new(answer: Result<Vec<u8>, PortError>) -> Self {
        Self { asked: Arc::default(), answer }
    }

    fn asked(&self) -> Vec<RenderRequest> {
        self.asked.lock().unwrap().clone()
    }
}

impl Renderer for Recorder {
    fn render_png(&self, _: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError> {
        self.asked.lock().unwrap().push(req.clone());
        self.answer.clone()
    }
}

/// Like [`drawn`], with `renderer` instead of the fake one, and the selection stored as the item "Pair".
fn with_renderer(renderer: &Recorder) -> (Session, String) {
    let fakes = Fakes::new();
    let mut ports = fakes.ports();
    ports.renderer = Some(Box::new(renderer.clone()));
    let mut s = Session::with_services(ports, Services::fake());
    let a = testing::add_rect(&mut s, 100.0, 100.0, 80.0, 40.0).unwrap();
    let b = testing::add_rect(&mut s, 300.0, 100.0, 80.0, 40.0).unwrap();
    testing::select(&mut s, &[a, b]);
    let id = run(&mut s, "library.add", json!({"name": "Pair"}))["item"]["id"].as_str().unwrap().to_owned();
    (s, id)
}

fn picture(out: &Value) -> Vec<u8> {
    assert_eq!(out["bytes"]["mime"], "image/png");
    base64::engine::general_purpose::STANDARD.decode(out["bytes"]["base64"].as_str().unwrap()).unwrap()
}

#[test]
fn thumbnail_draws_the_item_through_the_renderer_with_its_longer_side_at_max_side() {
    let (mut s, f) = drawn();
    let id = run(&mut s, "library.add", json!({"name": "Pair"}))["item"]["id"].as_str().unwrap().to_owned();
    let out = run(&mut s, "library.thumbnail", json!({"item": "Pair", "max_side": 128}));
    // 280 by 40 units plus 14 units of air on every side is 308 by 68; 128 / 308 of that is 128 by 28 pixels.
    assert_eq!((out["width"].as_u64(), out["height"].as_u64()), (Some(128), Some(28)));
    assert_eq!((out["item"].as_str(), out["changed"].as_bool()), (Some(id.as_str()), Some(false)));
    assert_eq!(picture(&out), b"\x89PNG-fake 128x28 elements=2");
    assert_eq!(f.renderer.calls(), 1);
}

#[test]
fn thumbnail_asks_for_a_transparent_picture_in_the_palette_that_was_chosen() {
    let renderer = Recorder::new(Ok(b"png".to_vec()));
    let (mut s, id) = with_renderer(&renderer);
    run(&mut s, "library.thumbnail", json!({"item": id}));
    run(&mut s, "library.thumbnail", json!({"item": id, "dark": true}));
    let asked = renderer.asked();
    assert_eq!(asked.len(), 2);
    assert!(asked.iter().all(|r| !r.background && !r.overlay && r.selection.is_empty()), "{asked:?}");
    assert_eq!(asked.iter().map(|r| r.dark).collect::<Vec<_>>(), [false, true]);
    // The region is the item at the origin with its air: 280 by 40 units, 14 units around.
    assert_eq!(asked[0].region, (-14.0, -14.0, 308.0, 68.0));
    // The default is 256 pixels on the longer side.
    assert_eq!(asked[0].size_px.0, 256);
}

#[test]
fn thumbnail_enlarges_a_small_item_at_most_eight_times() {
    let (mut s, _f) = testing::session();
    let a = testing::add_rect(&mut s, 5.0, 5.0, 10.0, 10.0).unwrap();
    testing::select(&mut s, &[a]);
    run(&mut s, "library.add", json!({"name": "Dot"}));
    // 10 units and 4 of air on both sides is 18 units; eight times that is 144 pixels, not 256.
    let out = run(&mut s, "library.thumbnail", json!({"item": "Dot"}));
    assert_eq!((out["width"].as_u64(), out["height"].as_u64()), (Some(144), Some(144)));
}

#[test]
fn thumbnail_changes_nothing_and_leaves_no_undo_step() {
    let (mut s, _f) = drawn();
    run(&mut s, "library.add", json!({"name": "Pair"}));
    let before = s.fingerprint();
    run(&mut s, "library.thumbnail", json!({"item": "Pair"}));
    assert_eq!(s.fingerprint(), before);
    assert_eq!(s.tab().doc.undo_depth(), 0);
    let spec = s.spec("library.thumbnail").unwrap();
    assert!(spec.reads_only);
    assert_eq!(spec.undo, Undo::NoUndo);
}

#[test]
fn thumbnail_works_as_a_query_for_the_ui() {
    let (mut s, _f) = drawn();
    run(&mut s, "library.add", json!({"name": "Pair"}));
    let out = s.query("library.thumbnail", json!({"item": "Pair", "max_side": 64})).unwrap();
    assert_eq!(out["width"], 64);
}

#[test]
fn thumbnail_refuses_in_words_and_changes_nothing() {
    let (mut s, _f) = drawn();
    run(&mut s, "library.add", json!({"name": "Pair"}));
    let r = refuse(&mut s, "library.thumbnail", json!({"item": "ghost"}));
    assert_eq!(r.code, "no_such_item");
    assert!(r.message.contains("library.list"), "{}", r.message);
    for side in [0, 15, 1025, 100_000] {
        assert_eq!(refuse(&mut s, "library.thumbnail", json!({"item": "Pair", "max_side": side})).code, "bad_args", "{side}");
    }
    assert_eq!(refuse(&mut s, "library.thumbnail", json!({"item": "Pair", "max_side": -1})).code, "bad_args");
    assert_eq!(refuse(&mut s, "library.thumbnail", json!({"item": "Pair", "extra": 1})).code, "bad_args");
}

#[test]
fn thumbnail_needs_the_renderer_and_says_so() {
    let fakes = Fakes::new();
    let mut ports = fakes.ports();
    ports.renderer = None;
    let mut s = Session::with_services(ports, Services::fake());
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    testing::select(&mut s, &[a]);
    run(&mut s, "library.add", json!({"name": "Box"}));
    let a = s.availability("library.thumbnail").unwrap_err();
    assert_eq!(a.code, "port_unavailable");
    assert!(a.message.contains("renderer"), "{}", a.message);
    assert_eq!(refuse(&mut s, "library.thumbnail", json!({"item": "Box"})).code, "port_unavailable");
}

#[test]
fn thumbnail_is_render_pending_while_the_browsers_renderer_draws_and_a_failure_is_io_failed() {
    let pending = Recorder::new(Err(PortError::NotReady));
    let (mut s, id) = with_renderer(&pending);
    let r = refuse(&mut s, "library.thumbnail", json!({"item": id}));
    assert_eq!(r.code, "render_pending");
    let failing = Recorder::new(Err(PortError::Failed("no device".to_owned())));
    let (mut s, id) = with_renderer(&failing);
    let r = refuse(&mut s, "library.thumbnail", json!({"item": id}));
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("no device"), "{}", r.message);
}

#[test]
fn list_gives_a_content_fingerprint_that_follows_the_drawing_and_not_the_name() {
    let (mut s, _f) = drawn();
    let first = run(&mut s, "library.add", json!({"name": "Pair"}))["item"].clone();
    let content = first["content"].as_str().unwrap().to_owned();
    assert_eq!(content.len(), 16);
    assert!(content.bytes().all(|b| b.is_ascii_hexdigit()), "{content}");
    // Another name, the same drawing: the same fingerprint.
    run(&mut s, "library.rename", json!({"item": "Pair", "name": "Twins"}));
    let list = run(&mut s, "library.list", json!({}));
    assert_eq!(list["items"][0]["content"], content.as_str());
    // A different drawing: another fingerprint.
    let c = testing::add_rect(&mut s, 0.0, 300.0, 50.0, 50.0).unwrap();
    testing::select(&mut s, &[c]);
    let other = run(&mut s, "library.add", json!({"name": "Square"}))["item"]["content"].clone();
    assert_ne!(other, json!(content));
    assert_eq!(run(&mut s, "library.list", json!({}))["items"][1]["content"], other);
}

// ---- library.insert ----

#[test]
fn insert_puts_a_copy_centred_on_the_point_selects_it_and_undoes_in_one_step() {
    let (mut s, _f) = drawn();
    let item = run(&mut s, "library.add", json!({"name": "Pair"}))["item"]["id"].as_str().unwrap().to_owned();
    s.tab_mut().doc.clear_history();
    let out = run(&mut s, "library.insert", json!({"item": "Pair", "at": [1000.0, 500.0]}));
    assert_eq!(out["item"], item.as_str());
    assert_eq!(count(&s), 4);
    let new: Vec<String> = out["ids"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    assert_eq!(new.len(), 2);
    assert_eq!(s.selection().len(), 2);
    let b = new
        .iter()
        .map(|i| drafftink_edit::query::element_bounds(s.element(&drafftink_scene::ElementId::new(i).unwrap()).unwrap()))
        .reduce(|a, b| a.union(b))
        .unwrap();
    assert!((b.center().x - 1000.0).abs() < 1e-9 && (b.center().y - 500.0).abs() < 1e-9, "{b:?}");
    assert_eq!(out["undo_label"], "Insert from library");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(count(&s), 2);
}

/// docs/exceptions.md E14: releasing a dragged library item is one plain `library.insert` at the release point.
/// No gesture runs, so nothing is half-done before the release, and one release is one ring entry.
#[test]
#[allow(non_snake_case)]
fn exception_E14_a_library_drop_is_one_plain_command() {
    let (mut s, _f) = drawn();
    run(&mut s, "library.add", json!({"name": "Pair"}));
    s.tab_mut().doc.clear_history();
    let ring_before = s.ring().iter().count();
    assert!(!s.spec("library.insert").unwrap().is_gesture(), "a drop is not a gesture command");
    run(&mut s, "library.insert", json!({"item": "Pair", "at": [400.0, 300.0]}));
    assert!(!s.gesture_running());
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert_eq!(s.ring().iter().count(), ring_before + 1, "one ring entry for the release");
    assert_eq!(s.ring().iter().last().map(|e| e.id.as_str()), Some("library.insert"));
}

#[test]
fn insert_twice_gives_distinct_ids_and_defaults_to_the_viewport_centre() {
    let (mut s, _f) = drawn();
    run(&mut s, "library.add", json!({"name": "Pair"}));
    s.tab_mut().view.scroll = drafftink_geom::kurbo::Point::new(-500.0, 700.0);
    let one = run(&mut s, "library.insert", json!({"item": "Pair", "select": false}));
    let two = run(&mut s, "library.insert", json!({"item": "Pair"}));
    assert_ne!(one["ids"], two["ids"]);
    assert_eq!(count(&s), 6);
    assert_eq!(s.selection().len(), 2, "the first insert left the selection alone, the second replaced it");
    let id = drafftink_scene::ElementId::new(one["ids"][0].as_str().unwrap()).unwrap();
    assert!(drafftink_edit::query::element_bounds(s.element(&id).unwrap()).x0 < -400.0);
}

#[test]
fn insert_of_a_grouped_item_makes_a_new_group_and_selects_it_whole() {
    let (mut s, _f) = testing::session();
    import(&mut s, LIB_GROUPED, json!({}));
    let list = run(&mut s, "library.list", json!({}));
    let id = list["items"][0]["id"].as_str().unwrap();
    run(&mut s, "library.insert", json!({"item": id}));
    run(&mut s, "library.insert", json!({"item": id}));
    let groups: std::collections::BTreeSet<_> = s.store().iter().flat_map(|e| e.group_ids.iter().cloned()).collect();
    let per_insert = s.store().iter().filter(|e| !e.group_ids.is_empty()).count() / 2;
    assert!(per_insert >= 2, "the fixture has a group");
    assert!(groups.len() >= 2, "each insert gets its own group ids");
    assert!(!s.tab().observed.selected_groups.is_empty());
}

#[test]
fn insert_refuses_an_unknown_item_in_view_only_mode_and_an_ambiguous_name() {
    let (mut s, _f) = drawn();
    let r = refuse(&mut s, "library.insert", json!({"item": "ghost"}));
    assert_eq!(r.code, "no_such_item");
    assert_eq!(r.message, "There is no library item `ghost`. List items with `library.list`.");
    run(&mut s, "library.add", json!({"name": "Same"}));
    run(&mut s, "library.add", json!({"name": "Same"}));
    let r = refuse(&mut s, "library.insert", json!({"item": "Same"}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("several items are named `Same`"), "{}", r.message);
    s.view_only = true;
    assert_eq!(refuse(&mut s, "library.insert", json!({"item": "Same"})).code, "view_only");
}

// ---- library.rename / remove ----

#[test]
fn rename_changes_the_name_in_memory_and_in_storage() {
    let (mut s, f) = drawn();
    let id = run(&mut s, "library.add", json!({}))["item"]["id"].as_str().unwrap().to_owned();
    let out = run(&mut s, "library.rename", json!({"item": id, "name": "Pair"}));
    assert_eq!(out["item"]["name"], "Pair");
    let stored: Value = serde_json::from_slice(&f.storage.bytes(Ns::Library, &item_key(&id)).unwrap()).unwrap();
    assert_eq!(stored["name"], "Pair");
    assert_eq!(run(&mut s, "library.rename", json!({"item": "Pair", "name": "Pair"}))["changed"], false);
    assert_eq!(refuse(&mut s, "library.rename", json!({"item": "ghost", "name": "x"})).code, "no_such_item");
    assert_eq!(refuse(&mut s, "library.rename", json!({"item": "Pair", "name": ""})).code, "empty_name");
}

#[test]
fn remove_by_id_or_name_deletes_from_storage_and_all_needs_to_be_said() {
    let (mut s, f) = drawn();
    let a = run(&mut s, "library.add", json!({"name": "A"}))["item"]["id"].as_str().unwrap().to_owned();
    run(&mut s, "library.add", json!({"name": "B"}));
    run(&mut s, "library.add", json!({"name": "C"}));
    let out = run(&mut s, "library.remove", json!({"items": ["B", a]}));
    assert_eq!(out["removed"].as_array().unwrap().len(), 2);
    assert!(f.storage.bytes(Ns::Library, &item_key(&a)).is_none());
    assert_eq!(run(&mut s, "library.list", json!({}))["items"].as_array().unwrap().len(), 1);
    assert_eq!(refuse(&mut s, "library.remove", json!({})).message, "Invalid arguments for `library.remove`: Give either `items` or all=true. See `engine.describe` for the schema.");
    assert_eq!(refuse(&mut s, "library.remove", json!({"items": ["C"], "all": true})).code, "bad_args");
    assert_eq!(refuse(&mut s, "library.remove", json!({"items": ["ghost"]})).code, "no_such_item");
    let out = run(&mut s, "library.remove", json!({"all": true}));
    assert_eq!(out["removed"].as_array().unwrap().len(), 1);
    assert!(run(&mut s, "library.list", json!({}))["items"].as_array().unwrap().is_empty());
    assert_eq!(run(&mut s, "library.remove", json!({"all": true}))["changed"], false);
    let mut again = Session::with_services(f.ports(), Services::fake());
    assert!(run(&mut again, "library.list", json!({}))["items"].as_array().unwrap().is_empty());
}

// ---- library.import ----

#[test]
fn import_reads_version_2_and_version_1_files() {
    let (mut s, _f) = testing::session();
    let out = import(&mut s, LIB_V2, json!({}));
    let added2 = out["added"].as_u64().unwrap();
    assert!(added2 > 0, "{out}");
    assert_eq!(out["total"].as_u64(), Some(added2));
    let out = import(&mut s, LIB_V1, json!({}));
    assert!(out["added"].as_u64().unwrap() > 0, "{out}");
    assert!(out["total"].as_u64().unwrap() > added2);
    let list = run(&mut s, "library.list", json!({}));
    assert!(list["items"].as_array().unwrap().iter().all(|i| i["element_count"].as_u64().unwrap() > 0));
    assert!(list["items"].as_array().unwrap().iter().all(|i| i["bounds"][0] == 0.0 && i["bounds"][1] == 0.0));
}

#[test]
fn importing_the_same_file_again_skips_every_item() {
    let (mut s, _f) = testing::session();
    let first = import(&mut s, LIB_V2, json!({}));
    let again = import(&mut s, LIB_V2, json!({}));
    assert_eq!((again["added"].as_u64(), again["skipped"].as_u64()), (Some(0), first["added"].as_u64()));
    assert_eq!(again["changed"], false);
    assert_eq!(again["total"], first["total"]);
}

#[test]
fn import_replace_replaces_the_whole_library_and_clears_storage_of_the_old_items() {
    let (mut s, f) = drawn();
    let old = run(&mut s, "library.add", json!({"name": "Old"}))["item"]["id"].as_str().unwrap().to_owned();
    let out = import(&mut s, LIB_V2, json!({"mode": "replace"}));
    assert!(out["added"].as_u64().unwrap() > 0);
    assert_eq!(out["total"], out["added"]);
    assert!(f.storage.bytes(Ns::Library, &item_key(&old)).is_none());
    assert!(!ids_of(&run(&mut s, "library.list", json!({}))).contains(&old));
}

#[test]
fn import_open_as_tab_lays_the_items_out_in_a_new_tab_and_leaves_the_library_alone() {
    let (mut s, _f) = testing::session();
    let out = import(&mut s, LIB_V2, json!({"mode": "open_as_tab"}));
    assert_eq!((out["added"].as_u64(), out["total"].as_u64()), (Some(0), Some(0)));
    assert_eq!(s.tabs.len(), 2);
    assert_eq!(s.store().meta().name, "shapes");
    assert!(count(&s) > 0);
    assert!(run(&mut s, "library.list", json!({}))["items"].as_array().unwrap().is_empty());
}

#[test]
fn import_from_a_path_and_from_a_file_that_is_not_a_library() {
    let (mut s, f) = testing::session();
    f.files.insert(&FileRef::Path { path: "/lib/shapes.excalidrawlib".to_owned() }, LIB_V2, "application/json");
    run(&mut s, "library.import", json!({"source": {"path": "/lib/shapes.excalidrawlib"}}));
    let r = refuse(&mut s, "library.import", json!({"source": {"base64": b64(NOT_A_LIB)}}));
    assert_eq!(r.code, "parse_failed");
    assert!(r.message.starts_with("The file is not an Excalidraw library:"), "{}", r.message);
    assert!(r.message.ends_with("Libraries are .excalidrawlib files."), "{}", r.message);
    let r = refuse(&mut s, "library.import", json!({"source": {"base64": b64(b"{}")}}));
    assert_eq!(r.code, "parse_failed");
    assert_eq!(refuse(&mut s, "library.import", json!({"source": {"path": "/missing"}})).code, "io_failed");
}

#[test]
fn a_drawing_is_not_a_library() {
    let (mut s, _f) = testing::session();
    let drawing = include_bytes!("../../../../tests/fixtures/excalidraw/basic_shapes.excalidraw");
    let r = refuse(&mut s, "library.import", json!({"source": {"base64": b64(drawing)}}));
    assert_eq!(r.code, "parse_failed");
    assert!(r.message.contains("To open a drawing, use Open instead"), "{}", r.message);
}

// ---- library.export ----

#[test]
fn export_inline_round_trips_through_import() {
    let (mut s, _f) = testing::session();
    import(&mut s, LIB_V2, json!({}));
    let before = run(&mut s, "library.list", json!({}));
    let out = run(&mut s, "library.export", json!({}));
    assert_eq!(out["items"].as_u64(), Some(before["items"].as_array().unwrap().len() as u64));
    assert_eq!(out["bytes"]["mime"], "application/vnd.excalidrawlib+json");
    let bytes = base64::engine::general_purpose::STANDARD.decode(out["bytes"]["base64"].as_str().unwrap()).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("excalidrawlib") && text.contains("\"version\": 2") || text.contains("\"version\":2"), "{text}");
    let (mut t, _f) = testing::session();
    let imported = import(&mut t, &bytes, json!({}));
    assert_eq!(imported["added"], out["items"]);
    let after = run(&mut t, "library.list", json!({}));
    let names = |v: &Value| v["items"].as_array().unwrap().iter().map(|i| (i["name"].clone(), i["element_count"].clone())).collect::<Vec<_>>();
    assert_eq!(names(&before), names(&after));
}

#[test]
fn export_some_items_to_a_file() {
    let (mut s, f) = drawn();
    run(&mut s, "library.add", json!({"name": "A"}));
    run(&mut s, "library.add", json!({"name": "B"}));
    let out = run(&mut s, "library.export", json!({"items": ["B"], "to": {"download": "b.excalidrawlib"}}));
    assert_eq!((out["items"].as_u64(), out["saved_to"].clone()), (Some(1), json!({"download": "b.excalidrawlib"})));
    assert!(out.get("bytes").is_none());
    assert!(f.files.bytes(&FileRef::Download { download: "b.excalidrawlib".to_owned() }).is_some());
    assert_eq!(refuse(&mut s, "library.export", json!({"items": ["ghost"]})).code, "no_such_item");
}

#[test]
fn export_of_an_empty_library_says_so() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "library.export", json!({}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "The library is empty, so there is nothing to export.");
}

// ---- storage from a newer build ----

#[test]
fn a_library_from_a_newer_build_is_read_but_never_written() {
    let (mut s, f) = drawn();
    run(&mut s, "library.add", json!({"name": "Mine"}));
    let mut st = f.storage.clone();
    let mut index: Value = serde_json::from_slice(&st.bytes_of(INDEX_KEY)).unwrap();
    index["schema"] = json!(99);
    st.put(Ns::Library, INDEX_KEY, index.to_string().as_bytes()).unwrap();
    let mut newer = Session::with_services(f.ports(), Services::fake());
    assert_eq!(run(&mut newer, "library.list", json!({}))["items"][0]["name"], "Mine");
    let only = testing::add_rect(&mut newer, 0.0, 0.0, 10.0, 10.0).unwrap();
    testing::select(&mut newer, &[only]);
    let r = refuse(&mut newer, "library.add", json!({}));
    assert_eq!(r.code, "library_newer");
    assert!(r.message.contains("format 99"), "{}", r.message);
    assert_eq!(refuse(&mut newer, "library.remove", json!({"all": true})).code, "library_newer");
}

trait StorageExt {
    fn bytes_of(&self, key: &str) -> Vec<u8>;
}

impl StorageExt for crate::ports::fake::FakeStorage {
    fn bytes_of(&self, key: &str) -> Vec<u8> {
        self.bytes(Ns::Library, key).unwrap()
    }
}

// ---- batches ----

#[test]
fn add_and_insert_work_in_one_batch() {
    let (mut s, _f) = drawn();
    let steps = [
        crate::batch::BatchStep::new("library.add", json!({"name": "Pair"})),
        crate::batch::BatchStep::new("library.insert", json!({"item": "Pair", "at": [900.0, 900.0]})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &crate::batch::BatchOpts::default());
    assert!(out.failed.is_none(), "{:?}", out.failed);
    assert_eq!(count(&s), 4);
}

// ---- sections ----

fn sections_of(list: &Value) -> Vec<(String, String, String, u64)> {
    list["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let text = |k: &str| s[k].as_str().unwrap().to_owned();
            (text("id"), text("name"), text("kind"), s["items"].as_u64().unwrap())
        })
        .collect()
}

fn import_named(s: &mut Session, bytes: &[u8], file: &str) -> Value {
    run(s, "library.import", json!({"source": {"base64": b64(bytes), "name": file}}))
}

/// A session with one item of its own, "Mine", and the fixture library imported from `shapes.excalidrawlib`.
fn with_imported() -> (Session, Fakes, String) {
    let (mut s, f) = drawn();
    run(&mut s, "library.add", json!({"name": "Mine"}));
    let out = import_named(&mut s, LIB_V2, "/home/me/shapes.excalidrawlib");
    let section = out["section"].as_str().unwrap().to_owned();
    (s, f, section)
}

#[test]
fn an_item_added_from_the_canvas_is_in_my_items() {
    let (mut s, _f) = drawn();
    let out = run(&mut s, "library.add", json!({"name": "Box"}));
    assert_eq!(out["item"]["section"], "mine");
    let list = run(&mut s, "library.list", json!({}));
    assert_eq!(sections_of(&list), [("mine".into(), "My items".into(), "mine".into(), 1)]);
    assert_eq!(list["items"][0]["section"], "mine");
}

#[test]
fn an_empty_library_has_no_sections() {
    let (mut s, _f) = testing::session();
    assert!(run(&mut s, "library.list", json!({}))["sections"].as_array().unwrap().is_empty());
}

#[test]
fn an_import_makes_a_section_named_after_the_file_and_puts_the_new_items_in_it() {
    let (mut s, _f, section) = with_imported();
    let list = run(&mut s, "library.list", json!({}));
    let sections = sections_of(&list);
    assert_eq!(sections.len(), 2, "{sections:?}");
    assert_eq!(sections[0], ("mine".into(), "My items".into(), "mine".into(), 1));
    let (id, name, kind, items) = &sections[1];
    assert_eq!((id, name.as_str(), kind.as_str()), (&section, "shapes", "imported"));
    let in_section = list["items"].as_array().unwrap().iter().filter(|i| i["section"] == section.as_str()).count();
    assert_eq!(*items as usize, in_section);
    assert!(in_section > 0 && list["items"][0]["section"] == "mine", "the person's item stays where it was");
}

#[test]
fn importing_a_file_of_the_same_name_again_adds_to_its_section_and_a_file_of_duplicates_makes_none() {
    let (mut s, _f, section) = with_imported();
    let again = import_named(&mut s, LIB_V2, "shapes.excalidrawlib");
    assert_eq!(again["added"], 0);
    assert!(again.get("section").is_none(), "nothing new, so no section: {again}");
    let more = import_named(&mut s, LIB_V1, "shapes.excalidrawlib");
    assert_eq!(more["section"], section.as_str(), "the same file name is the same section");
    let other = import_named(&mut s, LIB_GROUPED, "more.excalidrawlib");
    assert_ne!(other["section"], section.as_str());
    let names: Vec<String> = sections_of(&run(&mut s, "library.list", json!({}))).into_iter().map(|s| s.1).collect();
    assert_eq!(names, ["My items", "shapes", "more"]);
}

#[test]
fn a_file_without_a_usable_name_is_called_library() {
    let (mut s, _f) = testing::session();
    let out = run(&mut s, "library.import", json!({"source": {"base64": b64(LIB_V2)}}));
    assert!(out["section"].is_string());
    assert_eq!(sections_of(&run(&mut s, "library.list", json!({})))[0].1, "Library");
}

#[test]
fn replace_leaves_only_the_section_of_the_new_file() {
    let (mut s, _f, _old) = with_imported();
    let args = json!({"source": {"base64": b64(LIB_V1), "name": "new.excalidrawlib"}, "mode": "replace"});
    let out = run(&mut s, "library.import", args);
    let sections = sections_of(&run(&mut s, "library.list", json!({})));
    assert_eq!(sections.len(), 1, "{sections:?}");
    assert_eq!((sections[0].0.as_str(), sections[0].1.as_str()), (out["section"].as_str().unwrap(), "new"));
}

#[test]
fn list_can_keep_one_section_by_id_or_name() {
    let (mut s, _f, section) = with_imported();
    let by_id = run(&mut s, "library.list", json!({"section": section}));
    let by_name = run(&mut s, "library.list", json!({"section": "shapes"}));
    assert_eq!(by_id, by_name);
    assert!(by_id["items"].as_array().unwrap().iter().all(|i| i["section"] == section.as_str()));
    let mine = run(&mut s, "library.list", json!({"section": "My items"}));
    assert_eq!(ids_of(&mine).len(), 1);
    assert_eq!(sections_of(&mine).len(), 2, "the sections are listed whatever the filter says");
    assert_eq!(refuse(&mut s, "library.list", json!({"section": "ghost"})).code, "no_such_section");
}

#[test]
fn remove_section_removes_its_items_and_their_records_and_nothing_else() {
    let (mut s, f, section) = with_imported();
    let before = run(&mut s, "library.list", json!({}));
    let in_section: Vec<String> = ids_of(&run(&mut s, "library.list", json!({"section": section})));
    let own: Vec<String> = ids_of(&run(&mut s, "library.list", json!({"section": "mine"})));
    let out = run(&mut s, "library.remove_section", json!({"section": "shapes"}));
    assert_eq!((out["changed"].clone(), out["section"].clone()), (json!(true), json!(section)));
    assert_eq!(out["removed"], json!(in_section));
    for id in &in_section {
        assert!(f.storage.bytes(Ns::Library, &item_key(id)).is_none(), "{id}");
    }
    let after = run(&mut s, "library.list", json!({}));
    assert_eq!(ids_of(&after), own, "{before}");
    assert_eq!(sections_of(&after).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["mine"]);
    let index: Value = serde_json::from_slice(&f.storage.bytes(Ns::Library, INDEX_KEY).unwrap()).unwrap();
    assert_eq!(index["sections"], json!([]));
    let mut again = Session::with_services(f.ports(), Services::fake());
    assert_eq!(ids_of(&run(&mut again, "library.list", json!({}))), own, "gone from storage too");
}

#[test]
fn remove_section_by_id_and_for_my_items() {
    let (mut s, _f, section) = with_imported();
    let out = run(&mut s, "library.remove_section", json!({"section": "mine"}));
    assert_eq!(out["removed"].as_array().unwrap().len(), 1);
    assert_eq!(run(&mut s, "library.remove_section", json!({"section": "mine"}))["changed"], false, "nothing left in it");
    run(&mut s, "library.remove_section", json!({"section": section}));
    assert!(run(&mut s, "library.list", json!({}))["items"].as_array().unwrap().is_empty());
}

#[test]
fn remove_section_refuses_an_unknown_or_ambiguous_section_and_a_newer_library() {
    let (mut s, f, _section) = with_imported();
    let r = refuse(&mut s, "library.remove_section", json!({"section": "ghost"}));
    assert_eq!(r.code, "no_such_section");
    assert_eq!(r.message, "There is no library section `ghost`. List sections with `library.list`.");
    import_named(&mut s, LIB_GROUPED, "more.excalidrawlib");
    let first = sections_of(&run(&mut s, "library.list", json!({})))[1].0.clone();
    run(&mut s, "library.rename_section", json!({"section": "more", "name": "shapes"}));
    assert_eq!(refuse(&mut s, "library.remove_section", json!({"section": "shapes"})).code, "bad_args");
    run(&mut s, "library.remove_section", json!({"section": first}));
    let mut st = f.storage.clone();
    let mut index: Value = serde_json::from_slice(&st.bytes_of(INDEX_KEY)).unwrap();
    index["schema"] = json!(99);
    st.put(Ns::Library, INDEX_KEY, index.to_string().as_bytes()).unwrap();
    let mut newer = Session::with_services(f.ports(), Services::fake());
    assert_eq!(refuse(&mut newer, "library.remove_section", json!({"section": "mine"})).code, "library_newer");
    assert_eq!(refuse(&mut newer, "library.rename_section", json!({"section": "shapes", "name": "x"})).code, "library_newer");
}

#[test]
fn removing_the_last_item_of_a_section_takes_the_section_along() {
    let (mut s, _f, section) = with_imported();
    let items = ids_of(&run(&mut s, "library.list", json!({"section": section})));
    run(&mut s, "library.remove", json!({"items": items}));
    let sections = sections_of(&run(&mut s, "library.list", json!({})));
    assert_eq!(sections.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["mine"]);
}

#[test]
fn rename_section_changes_the_name_and_keeps_the_items() {
    let (mut s, f, section) = with_imported();
    let out = run(&mut s, "library.rename_section", json!({"section": "shapes", "name": "  Flowchart  "}));
    assert_eq!(out["changed"], true);
    assert_eq!((out["section"]["id"].clone(), out["section"]["name"].clone()), (json!(section), json!("Flowchart")));
    assert_eq!(run(&mut s, "library.rename_section", json!({"section": section, "name": "Flowchart"}))["changed"], false);
    let mut again = Session::with_services(f.ports(), Services::fake());
    let sections = sections_of(&run(&mut again, "library.list", json!({})));
    assert_eq!(sections[1].1, "Flowchart");
    assert_eq!(sections[1].3, out["section"]["items"].as_u64().unwrap());
    // The new name finds the section.
    assert_eq!(run(&mut s, "library.rename_section", json!({"section": "Flowchart", "name": "Shapes"}))["changed"], true);
}

#[test]
fn rename_section_refuses_in_words_and_changes_nothing() {
    let (mut s, _f, _section) = with_imported();
    assert_eq!(refuse(&mut s, "library.rename_section", json!({"section": "ghost", "name": "x"})).code, "no_such_section");
    assert_eq!(refuse(&mut s, "library.rename_section", json!({"section": "shapes", "name": " "})).code, "empty_name");
    let r = refuse(&mut s, "library.rename_section", json!({"section": "mine", "name": "Mine"}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("My items") && r.message.contains("Rename an imported library"), "{}", r.message);
}

#[test]
fn sections_survive_a_new_session_over_the_same_storage() {
    let (mut s, f, section) = with_imported();
    let before = run(&mut s, "library.list", json!({}));
    let mut again = Session::with_services(f.ports(), Services::fake());
    let after = run(&mut again, "library.list", json!({}));
    assert_eq!(sections_of(&before), sections_of(&after));
    assert_eq!(before["items"], after["items"]);
    assert!(sections_of(&after).iter().any(|s| s.0 == section));
}

/// Rewrites the storage of a library the way an earlier build wrote it (schema 1: no sections anywhere).
fn as_written_by_schema_1(f: &Fakes) {
    let mut st = f.storage.clone();
    let mut index: Value = serde_json::from_slice(&st.bytes_of(INDEX_KEY)).unwrap();
    index["schema"] = json!(1);
    index.as_object_mut().unwrap().remove("sections");
    st.put(Ns::Library, INDEX_KEY, index.to_string().as_bytes()).unwrap();
    for id in index["order"].as_array().unwrap() {
        let key = item_key(id.as_str().unwrap());
        let mut item: Value = serde_json::from_slice(&st.bytes_of(&key)).unwrap();
        item.as_object_mut().unwrap().remove("section");
        st.put(Ns::Library, &key, item.to_string().as_bytes()).unwrap();
    }
}

#[test]
fn a_library_written_before_sections_reads_as_one_my_items_section_and_can_be_changed() {
    let (mut s, f, _section) = with_imported();
    let total = run(&mut s, "library.list", json!({}))["items"].as_array().unwrap().len();
    as_written_by_schema_1(&f);
    let mut old = Session::with_services(f.ports(), Services::fake());
    let list = run(&mut old, "library.list", json!({}));
    assert!(list["warnings"].as_array().unwrap().is_empty(), "an old library is not damaged: {list}");
    assert_eq!(sections_of(&list), [("mine".into(), "My items".into(), "mine".into(), total as u64)]);
    assert!(list["items"].as_array().unwrap().iter().all(|i| i["section"] == "mine"));
    // It can be written: a new import gets its own section, and the old items stay in My items.
    let out = import_named(&mut old, LIB_GROUPED, "extra.excalidrawlib");
    assert!(out["added"].as_u64().unwrap() > 0);
    let sections = sections_of(&run(&mut old, "library.list", json!({})));
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].3, total as u64);
    let index: Value = serde_json::from_slice(&f.storage.bytes_of(INDEX_KEY)).unwrap();
    assert_eq!(index["schema"], 2);
}

#[test]
fn items_of_a_section_the_index_has_lost_are_moved_to_my_items_and_not_dropped() {
    let (mut s, f, _section) = with_imported();
    let total = run(&mut s, "library.list", json!({}))["items"].as_array().unwrap().len();
    let mut st = f.storage.clone();
    let mut index: Value = serde_json::from_slice(&st.bytes_of(INDEX_KEY)).unwrap();
    index["sections"] = json!([]);
    st.put(Ns::Library, INDEX_KEY, index.to_string().as_bytes()).unwrap();
    let mut again = Session::with_services(f.ports(), Services::fake());
    let list = run(&mut again, "library.list", json!({}));
    assert_eq!(list["items"].as_array().unwrap().len(), total);
    assert_eq!(sections_of(&list).len(), 1);
    assert!(list["warnings"][0].as_str().unwrap().contains("section that is missing"), "{list}");
}

#[test]
fn the_revision_counts_writes_and_not_reads() {
    let (mut s, _f) = drawn();
    assert_eq!(s.library_revision(), 0);
    run(&mut s, "library.list", json!({}));
    assert_eq!(s.library_revision(), 0);
    run(&mut s, "library.add", json!({"name": "A"}));
    let added = s.library_revision();
    assert!(added > 0);
    run(&mut s, "library.list", json!({}));
    assert_eq!(s.library_revision(), added);
    import_named(&mut s, LIB_V2, "shapes.excalidrawlib");
    assert!(s.library_revision() > added, "an import writes the library");
    let imported = s.library_revision();
    run(&mut s, "library.remove_section", json!({"section": "shapes"}));
    assert!(s.library_revision() > imported);
}

#[test]
fn a_library_dropped_on_the_canvas_gets_a_section_named_after_the_file() {
    let (mut s, _f) = testing::session();
    let item = json!({"name": "shapes.excalidrawlib", "bytes": {"base64": b64(LIB_V2)}});
    run(&mut s, "doc.drop", json!({"items": [item]}));
    assert_eq!(sections_of(&run(&mut s, "library.list", json!({})))[0].1, "shapes");
}

// ---- a cropped picture in the library ----

/// A renderer that keeps the document it was asked to draw.
#[derive(Clone, Default)]
struct SeesScene(Arc<Mutex<Vec<(drafftink_scene::Document, RenderRequest)>>>);

impl Renderer for SeesScene {
    fn render_png(&self, scene: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError> {
        self.0.lock().unwrap().push((scene.clone(), req.clone()));
        Ok(b"\x89PNG-fake".to_vec())
    }
}

#[test]
fn a_cropped_picture_keeps_its_crop_in_the_library_and_in_its_thumbnail() {
    let seen = SeesScene::default();
    let fakes = Fakes::new();
    let mut ports = fakes.ports();
    ports.renderer = Some(Box::new(seen.clone()));
    let mut s = Session::with_services(ports, Services::fake());
    // A 200 by 100 picture, cropped to its right half: the box becomes 100 by 100.
    let bytes = crate::cmd::image::tests::png(200, 100);
    let out = run(&mut s, "image.insert", json!({"images": [{"base64": b64(&bytes)}], "at": [0, 0]}));
    let id = out["ids"][0].as_str().unwrap().to_owned();
    run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 100, "y": 0, "w": 100, "h": 100}}));
    let item = run(&mut s, "library.add", json!({"name": "Half"}))["item"]["id"].as_str().unwrap().to_owned();

    // The thumbnail is drawn from the cropped picture: its box is the crop, and the whole picture is in the file table.
    run(&mut s, "library.thumbnail", json!({"item": item, "max_side": 100}));
    let asked = seen.0.lock().unwrap().clone();
    let (doc, req) = asked.last().expect("the renderer was asked");
    let image = doc.elements.iter().find_map(|e| e.image_data().map(|d| (e, d))).expect("the picture is in the item");
    assert_eq!(image.1.crop, Some(drafftink_scene::Crop { x: 100.0, y: 0.0, width: 100.0, height: 100.0 }));
    assert_eq!((image.0.transform.w, image.0.transform.h), (100.0, 100.0));
    assert!(doc.files.values().any(|f| f.bytes[..] == bytes[..]));
    assert_eq!(req.size_px.0, req.size_px.1, "a square crop makes a square thumbnail");

    // Inserting the item gives the same crop on a new element.
    run(&mut s, "library.insert", json!({"item": item, "at": [500, 0]}));
    let crops: Vec<_> = s.store().iter().filter_map(|e| e.image_data().map(|d| d.crop.clone())).collect();
    assert_eq!(crops.len(), 2);
    assert_eq!(crops[0], crops[1]);
    assert!(crops[1].is_some());
}
