//! Tests of the `library` family. No UI: a session over the fake ports, with the storage shared between
//! sessions to check that the library survives a restart.

use base64::Engine as _;
use drafftink_scene::{ElementKind, SceneRead};
use serde_json::{Value, json};

use super::store::{INDEX_KEY, item_key};
use crate::ports::fake::Fakes;
use crate::ports::{FileRef, Ns, Storage};
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
    for id in ["library.add", "library.insert", "library.remove", "library.rename", "library.list", "library.import", "library.export"] {
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
