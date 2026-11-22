//! `version.*`: names the state, lists, restores as one undo step, renames and deletes, and what survives a save, a
//! restart, a duplicate and a merge. The exact texts of the refusals are part of the contract.

use drafftink_scene::limits::{MAX_VERSIONS, MAX_VERSION_NAME_CHARS};
use drafftink_scene::{ElementId, SceneRead};
use serde_json::{Value, json};

use crate::ports::fake::Fakes;
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    s.execute_from(Origin::Test, id, args).unwrap_or_else(|r| panic!("{id} refused: {} ({})", r.message, r.code))
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

/// A session with two rectangles `e1` and `e2`, saved as a document that has no history.
fn two() -> (Session, Fakes) {
    let (mut s, f) = testing::session();
    testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    testing::add_rect(&mut s, 100.0, 0.0, 50.0, 50.0).unwrap();
    (s, f)
}

fn names(s: &mut Session) -> Vec<String> {
    let out = run(s, "version.list", json!({}));
    out["versions"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap().to_owned()).collect()
}

fn live(s: &Session) -> Vec<String> {
    s.store().iter().map(|e| e.id.to_string()).collect()
}

fn save(s: &mut Session, name: &str) -> String {
    run(s, "version.save", json!({"name": name}))["version"]["id"].as_str().unwrap().to_owned()
}

// ---- save and list ----

#[test]
fn save_names_the_current_state_with_time_and_author_and_list_shows_it_newest_first() {
    let (mut s, f) = two();
    run(&mut s, "settings.set", json!({"key": "collab.user_name", "value": "Ana"}));
    f.clock.advance_ms(5_000);
    let out = run(&mut s, "version.save", json!({"name": "  Before review  "}));
    assert_eq!(out["changed"], true);
    assert_eq!((out["count"].clone(), out["limit"].clone()), (json!(1), json!(MAX_VERSIONS)));
    let v = &out["version"];
    assert_eq!(v["name"], "Before review", "the name is trimmed");
    assert_eq!((v["author"].clone(), v["elements"].clone()), (json!("Ana"), json!(2)));
    assert_eq!(v["created_ms"], json!(crate::ports::Clock::now_ms(&f.clock)));
    assert!(v["bytes"].as_u64().unwrap() > 0);

    f.clock.advance_ms(60_000);
    run(&mut s, "settings.set", json!({"key": "collab.user_name", "value": ""}));
    save(&mut s, "Second");
    let list = run(&mut s, "version.list", json!({}));
    assert_eq!(list["changed"], false);
    assert_eq!(list["count"], 2);
    let first = &list["versions"][1];
    assert_eq!(list["versions"][0]["name"], "Second", "newest first");
    assert_eq!(list["versions"][0].get("author"), None, "no name set, no author");
    assert_eq!((first["name"].clone(), first["author"].clone(), first["id"].clone()), (json!("Before review"), json!("Ana"), v["id"].clone()));
}

#[test]
fn saving_is_not_an_undo_step_and_the_document_counts_as_changed_until_it_is_saved() {
    let (mut s, _f) = two();
    s.tab_mut().mark_saved();
    assert!(!s.tab().dirty());
    save(&mut s, "Named");
    assert!(!s.tab().doc.can_undo(), "naming a version is no undo step");
    assert!(s.tab().dirty(), "a new name is a change of the document: the file does not have it yet");
    run(&mut s, "doc.save", json!({"to": {"stored": "plan"}}));
    assert!(!s.tab().dirty());
    run(&mut s, "version.rename", json!({"id": "v1", "name": "Renamed"}));
    assert!(s.tab().dirty(), "a rename is a change too");
}

#[test]
fn an_empty_name_is_refused_in_plain_words() {
    let (mut s, _) = two();
    for name in ["", "   ", "\t"] {
        let r = refuse(&mut s, "version.save", json!({"name": name}));
        assert_eq!(r.code, "empty_name");
        assert_eq!(
            r.message,
            format!("The version needs a name of 1 to {MAX_VERSION_NAME_CHARS} characters without line breaks, for example \"Before review\".")
        );
    }
    assert!(names(&mut s).is_empty(), "a refusal changes nothing");
}

#[test]
fn a_name_with_a_line_break_or_over_the_limit_is_refused() {
    let (mut s, _) = two();
    assert_eq!(refuse(&mut s, "version.save", json!({"name": "two\nlines"})).code, "empty_name");
    let long = "x".repeat(MAX_VERSION_NAME_CHARS + 1);
    assert_eq!(refuse(&mut s, "version.save", json!({"name": long})).code, "empty_name");
    run(&mut s, "version.save", json!({"name": "x".repeat(MAX_VERSION_NAME_CHARS)}));
}

#[test]
fn the_same_name_may_be_used_twice() {
    // Versions are addressed by id, two peers can name a version alike at once, and a user who saves "Draft" a
    // second time wants both. The refusal would be a courtesy that a merge can undo.
    let (mut s, _) = two();
    let a = save(&mut s, "Draft");
    let b = save(&mut s, "Draft");
    assert_ne!(a, b);
    assert_eq!(names(&mut s), vec!["Draft", "Draft"]);
}

#[test]
fn the_cap_on_the_number_of_versions_is_a_refusal_that_says_what_to_do() {
    let (mut s, _) = two();
    for i in 0..MAX_VERSIONS {
        save(&mut s, &format!("v {i}"));
    }
    let r = refuse(&mut s, "version.save", json!({"name": "one too many"}));
    assert_eq!(r.code, "too_many_versions");
    assert_eq!(
        r.message,
        "This document already has 50 named versions, which is the limit. Delete one you no longer need with `version.delete`, then save again."
    );
    assert_eq!(r.suggest.first().map(|s| s.command.as_str()), Some("version.list"));
    assert_eq!(names(&mut s).len(), MAX_VERSIONS);
    let oldest = run(&mut s, "version.list", json!({}))["versions"][MAX_VERSIONS - 1]["id"].as_str().unwrap().to_owned();
    run(&mut s, "version.delete", json!({"id": oldest}));
    run(&mut s, "version.save", json!({"name": "now it fits"}));
}

#[test]
fn a_scene_too_large_for_one_version_is_refused_with_its_size() {
    use drafftink_scene::limits::{MAX_TEXT_CHARS, MAX_VERSION_STATE_BYTES};
    use drafftink_scene::{ElementKind, Rng64, SplitMix64, TextData};
    let mut s = testing::headless_session();
    // Random hex compresses to half its size at best, so five texts of 900,000 characters are over two MiB.
    let mut rng = SplitMix64::new(7);
    for _ in 0..5 {
        let mut text = String::with_capacity(MAX_TEXT_CHARS);
        while text.len() < 900_000 {
            text.push_str(&format!("{:016x}", rng.next_u64()));
        }
        testing::add(&mut s, ElementKind::Text(TextData::new(&text)), 0.0, 0.0, 10.0, 10.0).unwrap();
    }
    let r = refuse(&mut s, "version.save", json!({"name": "huge"}));
    assert_eq!(r.code, "version_too_large");
    assert!(
        r.message.starts_with("The scene is too large to keep as a named version: it takes ")
            && r.message.contains(&format!("a version may take {} KiB", MAX_VERSION_STATE_BYTES / 1024)),
        "{}",
        r.message
    );
    assert!(names(&mut s).is_empty());
}

// ---- restore ----

#[test]
fn restore_makes_the_state_current_as_one_undo_step_and_keeps_every_version() {
    let (mut s, _) = two();
    let v1 = save(&mut s, "Two shapes");
    run(&mut s, "element.delete", json!({"ids": ["e1"]}));
    run(&mut s, "element.create", json!({"kind": "ellipse", "bounds": {"x": 300, "y": 0, "w": 40, "h": 40}}));
    let v2 = save(&mut s, "Changed");
    assert_eq!(live(&s), vec!["e2", "e3"]);
    let depth = s.tab().doc.undo_depth();

    let out = run(&mut s, "version.restore", json!({"id": v1}));
    assert_eq!(out["changed"], true);
    assert_eq!((out["brought_back"].clone(), out["deleted"].clone(), out["name"].clone()), (json!(1), json!(1), json!("Two shapes")));
    assert_eq!(live(&s), vec!["e1", "e2"]);
    assert_eq!(s.tab().doc.undo_depth(), depth + 1, "one undo step");
    assert_eq!(s.tab().doc.undo_label().as_deref(), Some("Restore version"));
    assert_eq!(names(&mut s), vec!["Changed", "Two shapes"], "both versions are still there");

    run(&mut s, "history.undo", json!({}));
    assert_eq!(live(&s), vec!["e2", "e3"], "one undo takes the whole restore back");
    assert_eq!(names(&mut s).len(), 2, "undo does not touch versions");
    run(&mut s, "history.redo", json!({}));
    assert_eq!(live(&s), vec!["e1", "e2"]);
    // The other version restores too, and the one before it is still reachable.
    run(&mut s, "version.restore", json!({"id": v2}));
    assert_eq!(live(&s), vec!["e2", "e3"]);
}

#[test]
fn restore_resets_what_moved_and_the_background_and_drops_selected_ids_that_are_gone() {
    let (mut s, _) = two();
    let v = save(&mut s, "Start");
    run(&mut s, "element.move", json!({"ids": ["e1"], "dx": 40, "dy": 0}));
    run(&mut s, "doc.set_background", json!({"color": "#ff0000"}));
    let e3 = run(&mut s, "element.create", json!({"kind": "rectangle", "bounds": {"x": 9, "y": 9, "w": 5, "h": 5}}));
    let e3 = ElementId::new(e3["id"].as_str().unwrap()).unwrap();
    testing::select(&mut s, std::slice::from_ref(&e3));
    run(&mut s, "version.restore", json!({"id": v}));
    assert_eq!(s.element(&ElementId::new("e1").unwrap()).unwrap().transform.x, 0.0);
    assert_eq!(s.store().meta().background.to_hex(), "#ffffff");
    assert!(s.tab().observed.selection.is_empty(), "the selected element was made after the version, so it is deleted");
}

#[test]
fn restoring_the_state_that_is_already_current_changes_nothing() {
    let (mut s, _) = two();
    let v = save(&mut s, "Now");
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "version.restore", json!({"id": v}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), depth, "no step for nothing");
}

#[test]
fn restore_of_a_version_whose_picture_is_gone_still_restores_and_says_so() {
    let (mut s, _) = two();
    let v = save(&mut s, "Start");
    // Make the stored scene name a picture the document does not hold.
    let mut doc = s.tab().doc.snapshot();
    let mut image = doc.elements[0].clone();
    image.id = ElementId::new("pic").unwrap();
    image.index = drafftink_scene::ZIndex::between(doc.elements.last().map(|e| &e.index), None).unwrap();
    image.kind = drafftink_scene::ElementKind::Image(drafftink_scene::ImageData {
        file_id: Some(drafftink_scene::FileId::of(b"never stored")),
        natural_width: 10,
        natural_height: 10,
        crop: None,
        paint: Default::default(),
    });
    doc.elements.push(image);
    let with_pic = drafftink_scene::NamedVersion::capture(
        drafftink_scene::VersionId::new("vpic").unwrap(),
        "Has a picture",
        1,
        None,
        &drafftink_scene::MemoryStore::from_document(&doc).unwrap(),
    )
    .unwrap();
    s.tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| drafftink_scene::SceneTx::put_version(tx, with_pic)).unwrap();
    let out = run(&mut s, "version.restore", json!({"id": "vpic"}));
    assert_eq!(out["missing_pictures"], 1);
    let warnings = out["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|w| w.as_str().is_some_and(|w| w.contains("no longer in the document"))), "{warnings:?}");
    assert_eq!(live(&s).len(), 3, "the element is back; its picture shows as missing");
    run(&mut s, "version.restore", json!({"id": v}));
    assert_eq!(live(&s).len(), 2);
}

#[test]
fn restore_refuses_an_unknown_id_and_an_unreadable_version() {
    let (mut s, _) = two();
    let r = refuse(&mut s, "version.restore", json!({"id": "nope"}));
    assert_eq!(r.code, "no_such_version");
    assert_eq!(r.message, "There is no version `nope` in this document. List the versions with `version.list`.");
    let r = refuse(&mut s, "version.restore", json!({"id": "has space"}));
    assert_eq!(r.code, "no_such_version", "an id that cannot exist is just unknown");
    // A version whose stored scene is garbage.
    let good = save(&mut s, "Good");
    let broken = {
        let v = s.store().version(&drafftink_scene::VersionId::new(&good).unwrap()).unwrap().clone();
        drafftink_scene::NamedVersion {
            id: drafftink_scene::VersionId::new("broken").unwrap(),
            name: "Broken".into(),
            state: std::sync::Arc::from(vec![1u8, 2, 3]),
            ..v
        }
    };
    s.tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| drafftink_scene::SceneTx::put_version(tx, broken)).unwrap();
    let before = s.fingerprint();
    let r = refuse(&mut s, "version.restore", json!({"id": "broken"}));
    assert_eq!(r.code, "version_unreadable");
    assert!(r.message.starts_with("The version \"Broken\" cannot be read:"), "{}", r.message);
    assert!(r.message.ends_with("Nothing was changed. Delete it with `version.delete`, or use another version."), "{}", r.message);
    assert_eq!(s.fingerprint(), before);
    assert_eq!(refuse(&mut s, "version.open", json!({"id": "broken"})).code, "version_unreadable");
}

// ---- rename and delete ----

#[test]
fn rename_and_delete() {
    let (mut s, _) = two();
    let a = save(&mut s, "One");
    let b = save(&mut s, "Two");
    let out = run(&mut s, "version.rename", json!({"id": a, "name": "  First  "}));
    assert_eq!((out["changed"].clone(), out["name"].clone(), out["count"].clone()), (json!(true), json!("First"), json!(2)));
    assert_eq!(names(&mut s), vec!["Two", "First"]);
    assert_eq!(run(&mut s, "version.rename", json!({"id": a, "name": "First"}))["changed"], false);
    assert_eq!(refuse(&mut s, "version.rename", json!({"id": a, "name": " "})).code, "empty_name");
    assert_eq!(refuse(&mut s, "version.rename", json!({"id": "zzz", "name": "x"})).code, "no_such_version");
    let out = run(&mut s, "version.delete", json!({"id": b}));
    assert_eq!((out["name"].clone(), out["count"].clone()), (json!("Two"), json!(1)));
    assert_eq!(names(&mut s), vec!["First"]);
    assert_eq!(refuse(&mut s, "version.delete", json!({"id": b})).code, "no_such_version");
    assert!(!s.tab().doc.can_undo(), "neither is an undo step");
    assert_eq!(live(&s), vec!["e1", "e2"], "the scene is untouched");
}

// ---- open ----

#[test]
fn open_makes_an_independent_copy_in_a_new_tab_named_after_both() {
    let (mut s, _) = two();
    run(&mut s, "doc.rename", json!({"name": "Plan"}));
    let v = save(&mut s, "Two shapes");
    run(&mut s, "element.delete", json!({"ids": ["e1", "e2"]}));
    assert!(live(&s).is_empty());
    let out = run(&mut s, "version.open", json!({"id": v, "mode": "new_tab"}));
    assert_eq!((out["changed"].clone(), out["name"].clone()), (json!(true), json!("Plan — Two shapes")));
    assert_eq!(s.tabs.len(), 2);
    assert_eq!(s.store().meta().name, "Plan — Two shapes");
    assert_eq!(live(&s), vec!["e1", "e2"], "the copy shows the state of the version");
    assert!(names(&mut s).is_empty(), "a copy starts its own history of versions");
    assert!(!s.tab().dirty(), "it is a document as it was opened");
    run(&mut s, "element.delete", json!({"ids": ["e1"]}));
    run(&mut s, "tab.switch", json!({"tab": 0}));
    assert!(live(&s).is_empty(), "editing the copy changes nothing in the original");
    assert_eq!(names(&mut s), vec!["Two shapes"]);
    assert_eq!(refuse(&mut s, "version.open", json!({"id": "nope"})).code, "no_such_version");
}

#[test]
fn a_long_name_is_cut_to_fit_the_tab_name_limit() {
    use drafftink_scene::limits::MAX_NAME_CHARS;
    let (mut s, _) = two();
    run(&mut s, "doc.rename", json!({"name": "d".repeat(MAX_NAME_CHARS)}));
    let v = save(&mut s, &"v".repeat(MAX_VERSION_NAME_CHARS));
    let out = run(&mut s, "version.open", json!({"id": v}));
    assert!(out["name"].as_str().unwrap().chars().count() <= MAX_NAME_CHARS);
}

#[test]
fn open_refuses_when_the_tabs_are_full() {
    let (mut s, _) = two();
    let v = save(&mut s, "x");
    while s.tabs.len() < crate::cmd::tab::MAX_TABS {
        run(&mut s, "tab.new", json!({}));
    }
    run(&mut s, "tab.switch", json!({"tab": 0}));
    assert_eq!(refuse(&mut s, "version.open", json!({"id": v})).code, "too_many_tabs");
}

// ---- view-only, gestures ----

#[test]
fn view_only_documents_refuse_what_writes_and_still_open_and_list() {
    let (mut s, _) = two();
    let v = save(&mut s, "x");
    s.view_only = true;
    for (id, args) in [
        ("version.save", json!({"name": "y"})),
        ("version.restore", json!({"id": v})),
        ("version.rename", json!({"id": v, "name": "z"})),
        ("version.delete", json!({"id": v})),
    ] {
        assert_eq!(refuse(&mut s, id, args).code, "view_only", "{id}");
    }
    run(&mut s, "version.list", json!({}));
    run(&mut s, "version.open", json!({"id": v}));
}

// ---- what survives ----

#[test]
fn versions_survive_a_save_and_an_open() {
    let (mut s, _) = two();
    let a = save(&mut s, "Kept");
    run(&mut s, "doc.save", json!({"to": {"stored": "plan"}}));
    let bytes = s.ports.storage.as_ref().unwrap().get(crate::ports::Ns::Documents, "plan").unwrap().unwrap();
    let (mut t, _) = testing::session();
    t.ports.storage.as_mut().unwrap().put(crate::ports::Ns::Documents, "plan", &bytes).unwrap();
    run(&mut t, "doc.open", json!({"source": {"stored": "plan"}}));
    assert_eq!(names(&mut t), vec!["Kept"]);
    run(&mut t, "element.delete", json!({"ids": ["e1"]}));
    run(&mut t, "version.restore", json!({"id": a}));
    assert_eq!(live(&t), vec!["e1", "e2"], "a version that came through a file restores");
}

#[test]
fn versions_survive_the_workspace_autosave_and_a_restart() {
    let (mut s, f) = two();
    save(&mut s, "Before restart");
    s.save_workspace().unwrap();
    let mut again = Session::with_registry(f.ports(), s.registry_arc());
    again.restore_workspace().unwrap();
    assert_eq!(names(&mut again), vec!["Before restart"]);
    assert!(!again.tab().dirty() || s.tab().dirty(), "the unsaved state is kept the way it was");
}

#[test]
fn a_duplicated_tab_keeps_the_versions_and_the_copies_are_independent() {
    let (mut s, _) = two();
    save(&mut s, "Shared start");
    run(&mut s, "tab.duplicate", json!({}));
    assert_eq!(names(&mut s), vec!["Shared start"]);
    save(&mut s, "Only in the copy");
    run(&mut s, "tab.switch", json!({"tab": 0}));
    assert_eq!(names(&mut s), vec!["Shared start"]);
}

#[test]
fn a_remote_update_brings_versions_into_the_working_store() {
    let (mut a, _) = two();
    let (mut b, _) = testing::session();
    let first = a.tab_mut().doc.replicated_mut().export_snapshot().unwrap();
    b.tab_mut().doc.apply_remote(&first).unwrap();
    save(&mut a, "From a peer");
    let vv = b.tab().doc.replicated().version_vector().unwrap();
    let update = a.tab().doc.replicated().export_updates_since(&vv).unwrap();
    b.tab_mut().doc.apply_remote(&update).unwrap();
    assert_eq!(names(&mut b), vec!["From a peer"]);
    assert_eq!(live(&b), vec!["e1", "e2"]);
}

#[test]
fn exports_of_the_scene_carry_no_versions_and_excalidraw_says_it_dropped_them() {
    let (mut s, _) = two();
    save(&mut s, "Not in an export");
    save(&mut s, "Nor this one");
    run(&mut s, "doc.export", json!({"format": "json", "to": {"stored": "scene"}}));
    let json = String::from_utf8(s.ports.storage.as_ref().unwrap().get(crate::ports::Ns::Documents, "scene").unwrap().unwrap()).unwrap();
    assert!(!json.contains("\"versions\"") && json.contains("\"version\": 1"), "an export of the scene is a version 1 file: {json}");
    let out = run(&mut s, "doc.save", json!({"to": {"stored": "xl"}, "format": "excalidraw"}));
    let warnings = out["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w.as_str().is_some_and(|w| w.starts_with("2 named versions were left out: an Excalidraw file has no named versions."))),
        "{warnings:?}"
    );
    let text = String::from_utf8(s.ports.storage.as_ref().unwrap().get(crate::ports::Ns::Documents, "xl").unwrap().unwrap()).unwrap();
    assert!(!text.contains("Not in an export"));
}

#[test]
fn a_version_whose_scene_breaks_a_rule_is_refused_by_restore_and_open_and_changes_nothing() {
    use drafftink_scene::{
        BindMode, Binding, Element, ElementKind, LinearKind, MemoryStore, NamedVersion, SceneStore, SceneTx, VersionId,
        ZIndex,
    };
    // A peer's version: an arrow bound to an element the scene does not hold. Each element is valid alone.
    let mut arrow = Element::linear(
        ElementId::new("arrow").unwrap(),
        LinearKind::Arrow,
        ZIndex::first(),
        &[drafftink_geom::kurbo::Point::new(0.0, 0.0), drafftink_geom::kurbo::Point::new(50.0, 20.0)],
    )
    .unwrap();
    if let ElementKind::Arrow(d) = &mut arrow.kind {
        d.start_binding =
            Some(Binding { target: ElementId::new("ghost").unwrap(), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] });
    }
    let mut peer = MemoryStore::new();
    let mut tx = peer.begin();
    tx.insert(arrow).unwrap();
    tx.commit().unwrap();
    let bad = NamedVersion::capture(VersionId::new("vbad").unwrap(), "Broken scene", 1, None, &peer).unwrap();

    let (mut s, _) = two();
    s.tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| tx.put_version(bad)).unwrap();
    let before = s.fingerprint();
    let depth = s.tab().doc.undo_depth();
    for id in ["version.restore", "version.open"] {
        let r = refuse(&mut s, id, json!({"id": "vbad"}));
        assert_eq!(r.code, "version_unreadable", "{id}");
        assert!(r.message.contains("Broken scene") && r.message.contains("binding"), "{}", r.message);
    }
    assert_eq!(s.fingerprint(), before, "the document is as it was");
    assert_eq!((s.tabs.len(), s.tab().doc.undo_depth()), (1, depth), "no tab, no step");
}

#[test]
fn an_author_the_model_would_refuse_is_left_out() {
    let (mut s, _) = two();
    // The setting accepts text with a control character; a version author may not.
    crate::cmd::settings::set_internal(&mut s, "collab.user_name", &json!("A\nna")).unwrap();
    let out = run(&mut s, "version.save", json!({"name": "x"}));
    assert_eq!(out["version"].get("author"), None, "a name with a line break is no author");
}

#[test]
fn two_sessions_that_name_a_version_at_the_same_time_both_keep_both() {
    let (mut a, _) = two();
    let (mut b, _) = testing::session();
    // The fake ids are sequential in every session, so b would make the id a makes. Real ids are random.
    b.ports.ids.version_id();
    let snapshot = a.tab_mut().doc.replicated_mut().export_snapshot().unwrap();
    b.tab_mut().doc.apply_remote(&snapshot).unwrap();
    save(&mut a, "From a");
    save(&mut b, "From b");
    // Neither has seen the other's write. Now they exchange what the other lacks.
    let from_a = a.tab().doc.replicated().export_updates_since(&b.tab().doc.replicated().version_vector().unwrap()).unwrap();
    let from_b = b.tab().doc.replicated().export_updates_since(&a.tab().doc.replicated().version_vector().unwrap()).unwrap();
    b.tab_mut().doc.apply_remote(&from_a).unwrap();
    a.tab_mut().doc.apply_remote(&from_b).unwrap();
    for s in [&mut a, &mut b] {
        let mut now = names(s);
        now.sort();
        assert_eq!(now, ["From a", "From b"]);
    }
    assert_eq!(a.tab().doc.snapshot().versions, b.tab().doc.snapshot().versions, "the same list in the same order");
    // A restore in one is a normal edit that the other receives, and it keeps both versions.
    let id = run(&mut a, "version.list", json!({}))["versions"][0]["id"].as_str().unwrap().to_owned();
    run(&mut a, "element.delete", json!({"ids": ["e1"]}));
    run(&mut a, "version.restore", json!({"id": id}));
    let update = a.tab().doc.replicated().export_updates_since(&b.tab().doc.replicated().version_vector().unwrap()).unwrap();
    b.tab_mut().doc.apply_remote(&update).unwrap();
    assert_eq!(live(&b), live(&a));
    assert_eq!(names(&mut b).len(), 2);
}

// ---- pictures of an encrypted room (docs/design/collab-e2e.md section 15, rows Images and Schema) ----

mod room {
    use super::*;
    use crate::cmd::image::room_tests::{Room, png_of_size};
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use drafftink_scene::{ElementKind, FileId};

    /// A room with one picture. Session 0 holds the bytes, session 1 the manifest only.
    fn two() -> (Room, Vec<u8>, FileId) {
        let mut w = Room::new(2);
        let link = w.create(0);
        let bytes = png_of_size(60_000, 5);
        run(w.s(0), "image.insert", json!({"images": [{"base64": STANDARD.encode(&bytes)}], "at": [0, 0]}));
        w.settle();
        w.join(1, &link);
        let id = FileId::of(&bytes);
        assert!(w.s(1).store().file(&id).is_none());
        (w, bytes, id)
    }

    fn picture_elements(s: &Session) -> Vec<(String, Option<FileId>)> {
        s.store()
            .iter()
            .filter_map(|e| match &e.kind {
                ElementKind::Image(i) => Some((e.id.to_string(), i.file_id.clone())),
                _ => None,
            })
            .collect()
    }

    fn manifests(s: &Session) -> Vec<FileId> {
        s.tab().doc.replicated().manifests().map(|(id, _)| id.clone()).collect()
    }

    #[test]
    fn a_version_saved_in_a_room_names_the_pictures_by_id_and_copies_no_bytes() {
        let (mut w, bytes, id) = two();
        let vid = save(w.s(0), "With picture");
        let v = w.s(0).store().version(&drafftink_scene::VersionId::new(&vid).unwrap()).cloned().unwrap();
        assert_eq!(v.files, vec![id.clone()]);
        assert!(v.stored_bytes() < bytes.len() / 4, "the version holds the scene ({} bytes), not the picture ({})", v.stored_bytes(), bytes.len());
        // The version reaches the other session, which has the same ids.
        w.settle();
        let other = w.s(1).store().version(&drafftink_scene::VersionId::new(&vid).unwrap()).cloned().unwrap();
        assert_eq!(other.files, vec![id]);
    }

    #[test]
    fn opening_a_version_takes_the_bytes_the_tab_holds() {
        let (mut w, bytes, id) = two();
        let vid = save(w.s(0), "With picture");
        let out = run(w.s(0), "version.open", json!({"id": vid}));
        assert_eq!(out["changed"], true);
        assert_eq!(w.s(0).store().file(&id).map(|f| f.bytes.len()), Some(bytes.len()));
        assert_eq!(picture_elements(w.s(0)), vec![("e1".to_owned(), Some(id))]);
        assert!(w.s(0).room_images().is_none(), "the opened version is a local copy, not a room");
    }

    #[test]
    fn opening_a_version_whose_picture_is_not_downloaded_shows_the_placeholder_and_does_not_fail() {
        let (mut w, _, id) = two();
        w.settle();
        let vid = save(w.s(1), "Manifest only");
        let out = run(w.s(1), "version.open", json!({"id": vid}));
        let warnings: Vec<String> = out["warnings"].as_array().unwrap().iter().filter_map(|n| n.as_str().map(str::to_owned)).collect();
        assert!(warnings.iter().any(|n| n.contains("1 picture(s) of this version have not been downloaded yet")), "{warnings:?}");
        assert!(w.s(1).store().file(&id).is_none());
        assert_eq!(picture_elements(w.s(1)).len(), 1, "the element is there, drawn as the missing picture");
        let report = w.s(1).tab().doc.snapshot().validate();
        assert!(!report.has_errors(), "a missing blob is a warning: {:?}", report.errors().collect::<Vec<_>>());
        assert!(report.issues.iter().any(|i| i.error.to_string().contains("not in the files table")), "and it is reported as one");
    }

    #[test]
    fn opening_a_version_takes_a_not_yet_downloaded_picture_from_the_blob_cache() {
        let (mut w, bytes, id) = two();
        let vid = save(w.s(1), "Cached");
        w.cache(1, &bytes);
        let out = run(w.s(1), "version.open", json!({"id": vid}));
        assert!(out["warnings"].as_array().is_none_or(|w| w.is_empty()), "{out}");
        assert_eq!(w.s(1).store().file(&id).map(|f| f.bytes.len()), Some(bytes.len()));
    }

    #[test]
    fn restoring_brings_the_picture_back_under_the_same_file_id_and_the_manifest_never_loses_it() {
        let (mut w, _, id) = two();
        let vid = save(w.s(0), "Before");
        run(w.s(0), "element.delete", json!({"ids": ["e1"]}));
        assert!(picture_elements(w.s(0)).is_empty());
        assert_eq!(manifests(w.s(0)), vec![id.clone()], "deleting the element leaves the picture in the manifest");
        let out = run(w.s(0), "version.restore", json!({"id": vid}));
        assert_eq!((out["brought_back"].clone(), out["missing_pictures"].clone()), (json!(1), json!(0)));
        assert_eq!(picture_elements(w.s(0)), vec![("e1".to_owned(), Some(id.clone()))]);
        assert_eq!(manifests(w.s(0)), vec![id]);
    }

    #[test]
    fn restoring_on_a_device_without_the_bytes_is_not_a_missing_picture_because_the_room_describes_it() {
        let (mut w, _, id) = two();
        w.settle();
        let vid = save(w.s(1), "Before");
        run(w.s(1), "element.delete", json!({"ids": ["e1"]}));
        let out = run(w.s(1), "version.restore", json!({"id": vid}));
        assert_eq!(out["missing_pictures"], 0, "{out}");
        assert!(!out["warnings"].as_array().is_some_and(|w| w.iter().any(|n| n.as_str().is_some_and(|n| n.contains("no longer in the document")))), "{out}");
        assert_eq!(picture_elements(w.s(1)), vec![("e1".to_owned(), Some(id))]);
    }

    /// A workspace record whose Loro snapshot is as the previous build wrote it: schema 1, and no `size` in the
    /// entries of `files`.
    fn downgrade_to_schema_one(snapshot: &[u8]) -> Vec<u8> {
        use loro::{ExportMode, LoroDoc, LoroValue};
        let doc = LoroDoc::new();
        doc.import(snapshot).unwrap();
        doc.get_map("meta").insert("schema", 1i64).unwrap();
        let files = doc.get_map("files");
        let keys: Vec<String> = files.keys().map(|k| k.to_string()).collect();
        for key in keys {
            let Some(loro::ValueOrContainer::Value(LoroValue::Map(entry))) = files.get(&key) else { continue };
            let old: std::collections::HashMap<String, LoroValue> =
                entry.iter().filter(|(k, _)| k.as_str() != "size").map(|(k, v)| (k.clone(), v.clone())).collect();
            files.insert(&key, LoroValue::from(old)).unwrap();
        }
        doc.commit();
        assert!(!format!("{:?}", files.get_deep_value()).contains("size"), "the entries have no size, as before");
        assert!(format!("{:?}", files.get_deep_value()).contains("bytes"), "and they hold their bytes");
        doc.export(ExportMode::Snapshot).unwrap()
    }

    #[test]
    fn a_version_made_before_the_change_still_opens_after_the_schema_migration() {
        use drafftink_doc::FilePolicy;
        // A room as the previous build kept it: the bytes of the picture are in the Loro document.
        let mut w = Room::new(1);
        w.create(0);
        w.s(0).tab_mut().doc.replicated_mut().set_file_policy(FilePolicy::Embedded);
        let bytes = png_of_size(3_000, 9);
        run(w.s(0), "image.insert", json!({"images": [{"base64": STANDARD.encode(&bytes)}]}));
        let vid = save(w.s(0), "Old version");
        w.settle();
        w.s(0).save_workspace().unwrap();
        let key = format!("workspace.t{}.loro", w.s(0).tab().id.0);
        let stored = w.fakes(0).storage.bytes(crate::ports::Ns::Autosave, &key).expect("the tab's Loro snapshot");
        let old = downgrade_to_schema_one(&stored);
        let mut storage = w.fakes(0).storage.clone();
        crate::ports::Storage::put(&mut storage, crate::ports::Ns::Autosave, &key, &old).unwrap();

        let mut reopened = Session::with_services(w.fakes(0).ports(), crate::services::Services::fake());
        assert_eq!(reopened.restore_workspace().unwrap().restored, 1);
        let id = FileId::of(&bytes);
        assert_eq!(reopened.tab().doc.replicated().schema_version(), drafftink_doc::SCHEMA_VERSION, "migrated on open");
        assert_eq!(reopened.tab().doc.replicated().manifest(&id).map(|m| m.size), Some(bytes.len() as u64), "the step wrote the size");
        let out = run(&mut reopened, "version.open", json!({"id": vid}));
        assert_eq!(out["changed"], true);
        assert_eq!(reopened.store().file(&id).map(|f| f.bytes.len()), Some(bytes.len()));
        assert_eq!(picture_elements(&reopened), vec![("e1".to_owned(), Some(id))]);
    }
}
