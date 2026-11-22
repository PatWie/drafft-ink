//! Tests of the pictures of encrypted rooms: the glue between the sealed client, the tab's stores, the blob cache and
//! the workspace (docs/design/collab-e2e.md section 9). Sessions talk through the fake network to a [`FakeRelay`]
//! that serves the picture routes with the real relay core.

use drafftink_protocol::{MAX_IMAGE_BYTES, MAX_ROOM_IMAGES};
use drafftink_relay::RelayConfig;
use drafftink_scene::{ElementKind, FileEntry, FileId, SceneRead, SceneStore, SceneTx};
use serde_json::{Value, json};

use super::sealed_tests::{URL, World, elements, refuse, reload, run};
use super::state::CollabState;
use super::view::Loading;
use crate::cmd::tab::SealedBinding;
use crate::ports::{Net, Ns, Storage};
use crate::session::Session;
use crate::testing;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use drafftink_collab::ConnState;
use drafftink_doc::FilePolicy;

/// A picture that does not compress, so its bytes can be searched for.
fn noise_png(seed: u32, w: u32, h: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(88_172_645);
    let data: Vec<u8> = (0..w * h * 3)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect();
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("png header");
        writer.write_image_data(&data).expect("png data");
    }
    out
}

/// Inserts a picture with `image.insert` and returns its element and file id.
fn insert(s: &mut Session, bytes: &[u8], at: [f64; 2]) -> (String, FileId) {
    let out = run(s, "image.insert", json!({"images": [{"base64": STANDARD.encode(bytes)}], "at": at, "downscale": false}));
    let image = &out["images"][0];
    let file = FileId::new(image["file_id"].as_str().expect("a file id")).expect("a valid file id");
    (image["id"].as_str().expect("an element id").to_owned(), file)
}

fn has_bytes(s: &Session, id: &FileId) -> bool {
    s.store().file(id).is_some()
}

fn images_of(s: &mut Session) -> Value {
    run(s, "collab.status", json!({}))["images"].clone()
}

fn reconciles(s: &Session) -> u32 {
    s.ext.get::<CollabState>().and_then(|c| c.live.as_ref()).map_or(0, |l| l.pictures.runs)
}

fn cached(w: &World, i: usize, id: &FileId) -> Option<Vec<u8>> {
    w.c[i].1.storage.bytes(Ns::Blobs, id.as_str())
}

fn notices(s: &mut Session) -> Vec<String> {
    use crate::cmd::collab::CollabNotice;
    s.take_collab_notices()
        .into_iter()
        .map(|n| match n {
            CollabNotice::Info(t) => t,
            CollabNotice::Problem(p) => p.text,
        })
        .collect()
}

/// Session 1 has joined a new room of session 0.
fn two_in_a_room() -> World {
    let mut w = World::new(2);
    let link = w.create(0);
    w.join(1, &link, json!({}));
    w
}

#[test]
fn a_picture_inserted_in_one_session_reaches_the_other_and_is_cached_on_both() {
    let mut w = two_in_a_room();
    let bytes = noise_png(1, 40, 30);
    let (element, id) = insert(w.s(0), &bytes, [0.0, 0.0]);
    w.settle();
    assert!(has_bytes(w.s(1), &id), "the picture arrived");
    assert_eq!(&*w.s(1).store().file(&id).unwrap().bytes, &bytes[..]);
    assert!(w.s(1).store().get(&element.parse().unwrap()).is_some_and(|e| matches!(e.kind, ElementKind::Image(_))));
    assert_eq!(cached(&w, 0, &id).as_deref(), Some(&bytes[..]), "the inserting side caches it at once");
    assert_eq!(cached(&w, 1, &id).as_deref(), Some(&bytes[..]), "the downloading side caches it");
    assert_eq!(w.relay.sealed_pictures().len(), 1);
    let sealed = &w.relay.sealed_pictures()[0].1;
    assert!(!sealed.windows(24).any(|win| bytes.windows(24).any(|b| b == win)), "the relay holds no readable picture");
    // The status says what moved.
    let (a, b) = (images_of(w.s(0)), images_of(w.s(1)));
    assert_eq!(a["room_images"], 1, "{a}");
    assert_eq!((b["uploading"].clone(), b["downloading"].clone(), b["failed"].clone()), (json!(0), json!(0), json!(0)), "{b}");
    assert_eq!(run(w.s(0), "collab.status", json!({}))["unsynced"], false);
    // The room's document holds the manifest and no bytes.
    let manifest = w.s(1).tab().doc.replicated().manifest(&id).cloned().expect("the manifest");
    assert_eq!(manifest.size, bytes.len() as u64);
    assert!(w.relay.store().dump().iter().all(|record| !record.windows(24).any(|win| bytes.windows(24).any(|b| b == win))));
}

#[test]
fn a_download_is_no_edit_and_the_picture_is_drawn_without_one() {
    let mut w = two_in_a_room();
    w.relay.hold_blobs(true);
    let (element, id) = insert(w.s(0), &noise_png(2, 30, 30), [0.0, 0.0]);
    w.settle();
    let element: drafftink_scene::ElementId = element.parse().unwrap();
    assert!(w.s(1).store().get(&element).is_some(), "the element arrived with the manifest");
    assert!(!has_bytes(w.s(1), &id), "the bytes did not: the element shows the empty picture");
    assert_eq!(w.s(1).collab_view().loading, Some(Loading::Images { done: 0, total: 1 }), "the visible picture is on its way");
    w.s(1).tab_mut().mark_saved();
    let _ = w.s(1).tab().doc.take_touched();
    let (revision, print, work_revision) = {
        let t = w.s(1).tab();
        (t.doc.revision(), t.doc.fingerprint(), t.doc.store().revision())
    };
    assert!(!w.s(1).tab().dirty());
    w.relay.hold_blobs(false);
    w.settle();
    assert!(has_bytes(w.s(1), &id));
    let t = w.s(1).tab();
    assert_eq!((t.doc.revision(), t.doc.fingerprint()), (revision, print), "the document did not change");
    assert!(!t.dirty(), "a picture that arrives is not an edit");
    assert_ne!(t.doc.store().revision(), work_revision, "the frame is rebuilt: the working store moved");
    assert!(t.doc.take_touched().contains(&element), "the paint index re-reads the element that shows the picture");
    assert_eq!(w.s(1).collab_view().loading, None);
    assert_eq!(w.s(1).tab().doc.undo_depth(), 0, "no undo step");
}

#[test]
fn a_picture_whose_upload_is_late_is_asked_for_again_and_drawn_when_it_is_there() {
    let mut w = two_in_a_room();
    w.relay.hold_puts(true);
    let (_, id) = insert(w.s(0), &noise_png(3, 30, 30), [0.0, 0.0]);
    w.settle_for(4000);
    assert!(!has_bytes(w.s(1), &id), "the relay answered 404 so far");
    assert_eq!(run(w.s(0), "collab.status", json!({}))["unsynced"], true, "the uploader shows the dot until the upload is done");
    w.relay.hold_puts(false);
    w.settle_for(20_000);
    assert!(has_bytes(w.s(1), &id), "a retry found it");
    assert_eq!(run(w.s(0), "collab.status", json!({}))["unsynced"], false);
}

#[test]
fn a_far_picture_is_downloaded_when_the_camera_comes_near_and_not_before() {
    let mut w = two_in_a_room();
    let (_, near) = insert(w.s(0), &noise_png(4, 30, 30), [0.0, 0.0]);
    let (_, far) = insert(w.s(0), &noise_png(5, 30, 30), [90_000.0, 0.0]);
    w.settle();
    assert!(has_bytes(w.s(1), &near));
    assert!(!has_bytes(w.s(1), &far), "nobody looks there");
    assert!(cached(&w, 1, &far).is_none());
    run(w.s(1), "view.set", json!({"scroll": [90_000.0, 0.0], "zoom": 1.0}));
    w.settle();
    assert!(has_bytes(w.s(1), &far), "the camera came near");
}

#[test]
fn an_export_can_ask_for_a_far_picture() {
    let mut w = two_in_a_room();
    let (_, far) = insert(w.s(0), &noise_png(6, 30, 30), [90_000.0, 0.0]);
    w.settle();
    assert!(!has_bytes(w.s(1), &far));
    w.s(1).want_room_images(std::slice::from_ref(&far));
    w.settle();
    assert!(has_bytes(w.s(1), &far));
}

#[test]
fn a_reload_shows_the_pictures_offline_from_the_workspace_and_the_blob_cache() {
    let mut w = two_in_a_room();
    w.relay.hold_blobs(true);
    let (_, id) = insert(w.s(0), &noise_png(7, 30, 30), [0.0, 0.0]);
    w.settle();
    // The record is written while the picture is still missing...
    w.s(1).save_workspace().unwrap();
    w.relay.hold_blobs(false);
    w.settle();
    assert!(has_bytes(w.s(1), &id));
    // ... and is not written again for a picture that arrives: that is no edit. The blob cache has it.
    let written = w.s(1).save_workspace().unwrap().documents_written;
    assert_eq!(written, 0);
    let fakes = w.c[1].1.clone();
    fakes.net.set_fail_open(true);
    let mut reloaded = reload(&fakes);
    reloaded.restore_workspace().unwrap();
    assert!(reloaded.tab().ext.get::<SealedBinding>().is_some_and(|b| b.0.is_some()), "still the room's tab");
    assert!(has_bytes(&reloaded, &id), "offline, the picture is there");
    assert_eq!(reloaded.tab().doc.replicated().file_policy(), FilePolicy::External);
    assert_eq!(elements(&reloaded), 1);
}

#[test]
fn a_reload_online_gets_a_missing_picture_from_the_relay() {
    let mut w = two_in_a_room();
    let (_, id) = insert(w.s(0), &noise_png(8, 30, 30), [0.0, 0.0]);
    w.settle();
    w.s(1).save_workspace().unwrap();
    let fakes = w.c[1].1.clone();
    // The cache and the embedded copy are gone, as on a device that lost them.
    fakes.storage.clone().delete(Ns::Blobs, id.as_str()).unwrap();
    let mut reloaded = reload(&fakes);
    reloaded.restore_workspace().unwrap();
    w.c[1].0 = reloaded;
    w.s(1).tab_mut().doc.store();
    w.settle();
    assert!(has_bytes(w.s(1), &id));
}

#[test]
fn the_pictures_of_a_local_tab_leave_the_old_history_and_go_to_the_room_sealed() {
    let mut w = World::new(2);
    let (one, two) = (noise_png(10, 40, 40), noise_png(11, 40, 40));
    let (_, a) = insert(w.s(0), &one, [0.0, 0.0]);
    let (_, b) = insert(w.s(0), &two, [200.0, 0.0]);
    // The local copy embeds them in its Loro history.
    let before = w.s(0).tab().doc.replicated().export_snapshot().unwrap();
    assert!(before.windows(24).any(|win| one.windows(24).any(|p| p == win)), "a local document embeds its pictures");
    run(w.s(0), "collab.connect", json!({"url": URL}));
    run(w.s(0), "collab.join_room", json!({"on_local": "merge"}));
    w.settle();
    let link = run(w.s(0), "collab.share_link", json!({"copy": false, "reveal": true}))["link"].as_str().unwrap().to_owned();
    assert_eq!(w.s(0).tab().doc.replicated().file_policy(), FilePolicy::External);
    // Neither the snapshot nor the sealed log nor the updates this peer would send hold a byte of them.
    for blob in [w.s(0).tab().doc.replicated().export_snapshot().unwrap()].iter().chain(w.relay.store().dump().iter()) {
        for p in [&one, &two] {
            assert!(!blob.windows(24).any(|win| p.windows(24).any(|q| q == win)), "plaintext picture bytes in the room's data");
        }
    }
    assert_eq!(w.relay.sealed_pictures().len(), 2, "the relay holds both, sealed");
    assert!(w.relay.sealed_pictures().iter().all(|(_, s)| !s.windows(24).any(|win| one.windows(24).any(|p| p == win))));
    assert_eq!(w.s(0).tab().doc.undo_depth(), 0, "the history of the local document is gone");
    assert!(has_bytes(w.s(0), &a) && has_bytes(w.s(0), &b), "the tab still shows them");
    // The other person sees both.
    w.join(1, &link, json!({}));
    w.settle();
    assert!(has_bytes(w.s(1), &a) && has_bytes(w.s(1), &b));
    assert_eq!(elements(w.s(1)), 2);
}

#[test]
fn a_tab_with_pictures_the_room_cannot_hold_is_refused_before_anything_is_sent() {
    let mut w = World::new(1);
    let (_, small) = insert(w.s(0), &noise_png(12, 30, 30), [0.0, 0.0]);
    // A picture over the limit, put into the document as an import would.
    let big: Vec<u8> = (0..MAX_IMAGE_BYTES + 1).map(|i| (i % 251) as u8).collect();
    let big_id = {
        let entry = FileEntry { mime: "image/png".to_owned(), bytes: big.into() };
        let id = FileId::of(&entry.bytes);
        let element = testing::add(w.s(0), ElementKind::Image(drafftink_scene::ImageData { file_id: Some(id.clone()), natural_width: 10, natural_height: 10, crop: None, paint: Default::default() }), 300.0, 0.0, 50.0, 50.0).unwrap();
        w.s(0).tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| tx.put_file(entry).map(|_| ())).unwrap();
        (id, element)
    };
    assert!(has_bytes(w.s(0), &small) && has_bytes(w.s(0), &big_id.0));
    run(w.s(0), "collab.connect", json!({"url": URL}));
    let before = w.s(0).fingerprint();
    let r = refuse(w.s(0), "collab.join_room", json!({"on_local": "merge"}));
    assert_eq!(r.code, "local_too_large_for_room");
    assert!(r.message.contains(&format!("element `{}`", big_id.1)) && r.message.contains("at most 4 MB"), "{}", r.message);
    assert!(r.message.contains("Nothing was sent"), "{}", r.message);
    assert_eq!(w.s(0).fingerprint(), before, "nothing changed");
    assert!(w.c[0].1.net.servers().is_empty(), "no connection was opened");
    assert_eq!(w.s(0).tab().doc.replicated().file_policy(), FilePolicy::Embedded);
    assert!(w.s(0).tab().ext.get::<SealedBinding>().is_none_or(|b| b.0.is_none()));
    // Discarding the local content skips the check: the tab is replaced anyway.
    run(w.s(0), "collab.join_room", json!({"on_local": "discard"}));
}

#[test]
fn too_many_pictures_or_too_many_bytes_are_refused_naming_the_first_that_does_not_fit() {
    let mut w = World::new(1);
    // MAX_ROOM_IMAGES + 1 tiny pictures.
    let first_over = {
        let mut over = None;
        for i in 0..=MAX_ROOM_IMAGES {
            let entry = FileEntry { mime: "image/png".to_owned(), bytes: vec![1, 2, 3, (i % 256) as u8, (i / 256) as u8].into() };
            let id = FileId::of(&entry.bytes);
            let kind = ElementKind::Image(drafftink_scene::ImageData { file_id: Some(id), natural_width: 1, natural_height: 1, crop: None, paint: Default::default() });
            let element = testing::add(w.s(0), kind, f64::from(i) * 20.0, 0.0, 10.0, 10.0).unwrap();
            w.s(0).tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| tx.put_file(entry).map(|_| ())).unwrap();
            if i == MAX_ROOM_IMAGES {
                over = Some(element);
            }
        }
        over.unwrap()
    };
    run(w.s(0), "collab.connect", json!({"url": URL}));
    let r = refuse(w.s(0), "collab.join_room", json!({"on_local": "merge"}));
    assert_eq!(r.code, "local_too_large_for_room");
    assert!(r.message.contains(&format!("{} pictures", MAX_ROOM_IMAGES + 1)), "{}", r.message);
    assert!(r.message.contains(&format!("element `{first_over}`")), "{}", r.message);

    let mut w = World::new(1);
    let mut last = None;
    for i in 0..13u32 {
        let bytes: Vec<u8> = (0..MAX_IMAGE_BYTES).map(|k| ((k as u32).wrapping_mul(7).wrapping_add(i * 31)) as u8).collect();
        let entry = FileEntry { mime: "image/png".to_owned(), bytes: bytes.into() };
        let id = FileId::of(&entry.bytes);
        let kind = ElementKind::Image(drafftink_scene::ImageData { file_id: Some(id), natural_width: 1, natural_height: 1, crop: None, paint: Default::default() });
        last = Some(testing::add(w.s(0), kind, f64::from(i) * 20.0, 0.0, 10.0, 10.0).unwrap());
        w.s(0).tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| tx.put_file(entry).map(|_| ())).unwrap();
    }
    run(w.s(0), "collab.connect", json!({"url": URL}));
    let r = refuse(w.s(0), "collab.join_room", json!({"on_local": "merge"}));
    assert_eq!(r.code, "local_too_large_for_room");
    assert!(r.message.contains("more than 50 MB"), "{}", r.message);
    assert!(r.message.contains(&format!("element `{}`", last.unwrap())), "{}", r.message);
}

#[test]
fn leaving_the_room_keeps_every_picture_and_a_save_embeds_them() {
    let mut w = two_in_a_room();
    let (_, near) = insert(w.s(0), &noise_png(20, 30, 30), [0.0, 0.0]);
    let (_, far) = insert(w.s(0), &noise_png(21, 30, 30), [90_000.0, 0.0]);
    w.settle();
    assert!(has_bytes(w.s(1), &near) && !has_bytes(w.s(1), &far));
    // This device saw the far picture earlier: it is in its blob cache.
    w.c[1].1.storage.clone().put(Ns::Blobs, far.as_str(), &noise_png(21, 30, 30)).unwrap();
    run(w.s(1), "collab.leave_room", json!({}));
    assert_eq!(w.s(1).tab().doc.replicated().file_policy(), FilePolicy::Embedded);
    assert!(has_bytes(w.s(1), &near) && has_bytes(w.s(1), &far), "the local copy keeps what the device holds");
    run(w.s(1), "doc.save", json!({"to": {"stored": "copy"}}));
    let saved = w.c[1].1.storage.bytes(Ns::Documents, "copy").expect("saved");
    let doc = drafftink_scene::Document::from_json(std::str::from_utf8(&saved).unwrap()).unwrap();
    assert!(doc.files.contains_key(&near) && doc.files.contains_key(&far), "the file embeds the pictures");
    // Opened again in a plain session, they are there.
    let (mut s, _) = testing::session();
    s.ports.storage.as_mut().unwrap().put(Ns::Documents, "copy", &saved).unwrap();
    run(&mut s, "doc.open", json!({"source": {"stored": "copy"}}));
    assert!(has_bytes(&s, &near) && has_bytes(&s, &far));
}

#[test]
fn a_damaged_download_says_so_and_leaves_the_empty_picture() {
    let mut w = World::new(2);
    let link = w.create(0);
    let (element, id) = insert(w.s(0), &noise_png(30, 30, 30), [0.0, 0.0]);
    w.settle();
    w.relay.damage_pictures();
    w.join(1, &link, json!({}));
    w.settle();
    assert!(!has_bytes(w.s(1), &id));
    assert!(cached(&w, 1, &id).is_none(), "damaged bytes are not kept");
    assert!(w.s(1).store().get(&element.parse().unwrap()).is_some(), "the element stays");
    let told = notices(w.s(1));
    assert!(told.iter().any(|t| t.contains("damaged")), "{told:?}");
    assert_eq!(w.s(1).collab_view().loading, None, "a damaged picture is not waited for");
    assert_eq!(images_of(w.s(1))["failed"], 1);
}

#[test]
fn an_upload_resumes_after_the_connection_dropped() {
    let mut w = World::new(1);
    w.create(0);
    w.relay.hold_blobs(true);
    let (_, id) = insert(w.s(0), &noise_png(40, 30, 30), [0.0, 0.0]);
    w.settle_for(400);
    assert_eq!(w.relay.sealed_pictures().len(), 0);
    assert_eq!(run(w.s(0), "collab.status", json!({}))["unsynced"], true);
    // The connection drops with the upload under way.
    w.c[0].1.net.last_server().unwrap().drop_connection("the network went away");
    w.settle_for(400);
    w.relay.hold_blobs(false);
    w.settle_for(20_000);
    assert_eq!(w.relay.sealed_pictures().len(), 1, "uploaded when the connection was back");
    assert_eq!(run(w.s(0), "collab.status", json!({}))["state"], "connected");
    assert_eq!(run(w.s(0), "collab.status", json!({}))["unsynced"], false);
    assert!(has_bytes(w.s(0), &id));
}

#[test]
fn after_the_relay_forgot_the_room_the_pictures_are_offered_again() {
    let mut w = World::new(1);
    let link = w.create(0);
    let (_, id) = insert(w.s(0), &noise_png(41, 30, 30), [0.0, 0.0]);
    w.settle();
    assert_eq!(w.relay.sealed_pictures().len(), 1);
    w.s(0).save_workspace().unwrap();
    // The room expired; this tab restores it from its copy and offers what it has.
    w.relay = super::fake_relay::FakeRelay::new(RelayConfig::self_host());
    let mut s = reload(&w.c[0].1);
    s.restore_workspace().unwrap();
    w.c[0].0 = s;
    w.settle();
    run(w.s(0), "collab.leave_room", json!({}));
    run(w.s(0), "collab.join_room", json!({"room": link, "on_local": "merge", "if_missing": "restore"}));
    w.settle();
    assert_eq!(w.relay.sealed_pictures().len(), 1, "the relay has the picture again");
    assert!(has_bytes(w.s(0), &id));
}

#[test]
fn the_blob_cache_is_cleaned_with_the_workspace() {
    let mut w = two_in_a_room();
    let (_, id) = insert(w.s(0), &noise_png(50, 30, 30), [0.0, 0.0]);
    w.settle();
    let garbage = "f".repeat(64);
    w.c[0].1.storage.clone().put(Ns::Blobs, &garbage, b"old").unwrap();
    assert!(cached(&w, 0, &id).is_some());
    w.s(0).save_workspace().unwrap();
    assert!(w.c[0].1.storage.bytes(Ns::Blobs, &garbage).is_none(), "what no tab names is deleted");
    assert!(cached(&w, 0, &id).is_some(), "what a tab names stays");
}

#[test]
fn the_blob_cache_is_cleaned_after_a_tab_was_closed() {
    let mut w = World::new(1);
    w.create(0);
    let (_, id) = insert(w.s(0), &noise_png(51, 30, 30), [0.0, 0.0]);
    w.settle();
    w.s(0).save_workspace().unwrap();
    run(w.s(0), "tab.new", json!({}));
    run(w.s(0), "tab.close", json!({"tab": "t1", "leave_room": true, "on_unsaved": "discard"}));
    w.s(0).save_workspace().unwrap();
    assert!(cached(&w, 0, &id).is_none(), "the picture of the closed tab is gone from the cache");
}

#[test]
fn a_picture_the_room_refuses_raises_a_notice_in_plain_words() {
    let mut config = RelayConfig::self_host();
    config.limits.max_image_bytes = 1000;
    let mut w = World::with_config(1, config);
    w.create(0);
    insert(w.s(0), &noise_png(60, 30, 30), [0.0, 0.0]);
    w.settle();
    let told = notices(w.s(0));
    assert!(told.iter().any(|t| t.contains("Pictures in a shared room can be at most") && t.contains("insert it again")), "{told:?}");
    assert_eq!(w.relay.sealed_pictures().len(), 0);

    let mut config = RelayConfig::self_host();
    config.limits.max_room_images = 1;
    let mut w = World::with_config(1, config);
    w.create(0);
    insert(w.s(0), &noise_png(61, 30, 30), [0.0, 0.0]);
    w.settle();
    insert(w.s(0), &noise_png(62, 30, 30), [200.0, 0.0]);
    w.settle();
    let told = notices(w.s(0));
    assert!(told.iter().any(|t| t.contains("This room already holds") && t.contains("start a new room")), "{told:?}");
    assert_eq!(w.relay.sealed_pictures().len(), 1);
}

#[test]
fn an_idle_session_with_pictures_does_no_picture_work_per_poll() {
    let mut w = two_in_a_room();
    insert(w.s(0), &noise_png(70, 30, 30), [0.0, 0.0]);
    insert(w.s(0), &noise_png(71, 30, 30), [200.0, 0.0]);
    w.settle();
    let runs: Vec<u32> = (0..2).map(|i| reconciles(w.s(i))).collect();
    let requests = w.relay.blob_requests();
    let writes: Vec<usize> = w.c.iter().map(|(_, f)| f.storage.len()).collect();
    w.settle_for(10_000);
    assert_eq!((0..2).map(|i| reconciles(w.s(i))).collect::<Vec<_>>(), runs, "no look at the pictures while nothing changed");
    assert_eq!(w.relay.blob_requests(), requests, "no request");
    assert_eq!(w.c.iter().map(|(_, f)| f.storage.len()).collect::<Vec<_>>(), writes, "no write");
    // The camera moving over pictures that are all here does nothing either.
    run(w.s(1), "view.set", json!({"scroll": [5000.0, 0.0], "zoom": 1.0}));
    w.settle();
    assert_eq!(reconciles(w.s(1)), runs[1]);
    let live = w.s(1).ext.get::<CollabState>().and_then(|c| c.live.as_ref()).map(|l| l.session.client().status().state);
    assert_eq!(live, Some(ConnState::Online));
}

#[test]
fn a_plaintext_room_does_no_picture_work() {
    let mut w = World::new(1);
    run(w.s(0), "collab.connect", json!({"url": URL}));
    run(w.s(0), "collab.join_room", json!({"room": "plain-room"}));
    insert(w.s(0), &noise_png(80, 30, 30), [0.0, 0.0]);
    w.settle();
    assert_eq!(reconciles(w.s(0)), 0);
    assert_eq!(w.s(0).tab().doc.replicated().file_policy(), FilePolicy::Embedded);
    assert_eq!(images_of(w.s(0))["room_images"], 0);
}

#[test]
fn a_shell_without_http_runs_the_room_and_says_once_that_pictures_do_not_work() {
    use crate::ports::PortError;
    struct NoBlobs(crate::ports::fake::FakeNet);
    impl Net for NoBlobs {
        fn open_transport(&mut self) -> Result<Box<dyn drafftink_collab::Transport>, PortError> {
            self.0.open_transport()
        }
        fn random(&mut self) -> u128 {
            self.0.random()
        }
    }
    let mut w = World::new(1);
    let fakes = w.c[0].1.clone();
    let mut ports = fakes.ports();
    ports.net = Some(Box::new(NoBlobs(fakes.net.clone())));
    w.c[0].0 = Session::with_services(ports, crate::services::Services::fake());
    let link = w.create(0);
    let told = notices(w.s(0));
    assert_eq!(told.iter().filter(|t| t.contains("cannot load or send pictures")).count(), 1, "{told:?}");
    assert!(link.contains("room="));
    testing::add_rect(w.s(0), 0.0, 0.0, 10.0, 10.0).unwrap();
    w.settle();
    assert_eq!(run(w.s(0), "collab.status", json!({}))["state"], "connected");
}
