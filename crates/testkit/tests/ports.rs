//! Rust end-to-end tests that need the fake ports themselves, which a JSON scenario cannot reach: what the
//! session wrote to storage, how often the renderer was asked, what time the ring recorded.

use drafftink_engine::ports::{Ns, Storage};
use drafftink_testkit::Harness;
use serde_json::json;

#[test]
fn a_setting_is_written_to_the_storage_port_and_only_when_it_changed() {
    let mut h = Harness::new();
    assert!(h.fakes.storage.list(Ns::Settings).unwrap().is_empty());
    h.session.execute("settings.set", json!({ "key": "snap.grid", "value": true })).unwrap();
    let entries = h.fakes.storage.list(Ns::Settings).unwrap();
    assert_eq!(entries.len(), 1, "the change was not persisted");
    let stamp = entries[0].modified_ms;

    h.advance_clock(5_000).unwrap();
    h.session.execute("settings.set", json!({ "key": "snap.grid", "value": true })).unwrap();
    assert_eq!(h.fakes.storage.list(Ns::Settings).unwrap()[0].modified_ms, stamp, "the same value was written again");
}

#[test]
fn a_screenshot_asks_the_renderer_port_once_and_a_headless_session_cannot_take_one() {
    let mut h = Harness::new();
    h.load_generated(6, 1).unwrap();
    let out = h.session.execute("view.screenshot", json!({ "region": "all" })).unwrap();
    assert_eq!(out["bytes"]["mime"], json!("image/png"));
    assert_eq!(h.fakes.renderer.calls(), 1);

    let mut bare = Harness::headless();
    let refusal = bare.session.execute("view.screenshot", json!({})).unwrap_err();
    assert_eq!(refusal.code, "port_unavailable");
    assert_eq!(h.fakes.renderer.calls(), 1);
}

#[test]
fn the_command_ring_stamps_the_fake_clock() {
    let mut h = Harness::new();
    h.session.execute("view.zoom_in", json!({})).unwrap();
    let first = h.advance_clock(0).unwrap()["now_ms"].as_u64().unwrap();
    h.advance_clock(1_000).unwrap();
    h.session.execute("view.zoom_out", json!({})).unwrap();
    let recent = h.session.execute("engine.recent", json!({ "limit": 2 })).unwrap();
    let times: Vec<u64> = recent["entries"].as_array().unwrap().iter().map(|e| e["t_ms"].as_u64().unwrap()).collect();
    assert_eq!(times, [first, first + 1_000]);
}
