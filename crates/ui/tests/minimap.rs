#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The minimap: off by default and free while off, a card at the bottom right that clears the other chrome, a
//! rectangle for the camera, click and drag to move the view, the wheel to zoom, the verb, the setting, the menu
//! rows and the key.

mod common;
mod dialog;
mod support_a;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::testing::add_rect;
use drafftink_scene::{Color, Element, ElementId, ElementKind, MemoryStore, SceneRead, SceneStore, SceneTx};
use drafftink_ui::keymap::{KeyContext, Pointer, resolve_keys, verb_key_label};
use drafftink_ui::minimap::{Look, MAX_MARKS, Mapping, Scene, WorldRect, visible};
use drafftink_ui::model::ViewView;
use drafftink_ui::surfaces::minimap::INSET;
use drafftink_ui::{Call, CollabView, Peer, Routed, UiAction, Verb, route};
use egui::{Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Rect, TouchPhase, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

type H = Harness<'static, Fixture>;

const WINDOW: egui::Vec2 = vec2(1280.0, 800.0);

/// A fixture whose minimap is on, with a document of a few rectangles and the camera at the origin (the window is
/// 1280 by 720 points for the session, zoom 1: it sees x -640..640 and y -360..360).
fn on() -> Fixture {
    let mut f = dialog::fixture();
    for (x, y) in [(0.0, 0.0), (400.0, 100.0), (900.0, 400.0)] {
        add_rect(&mut f.session, x, y, 100.0, 80.0).unwrap();
    }
    dialog::run(&mut f, "view.set", json!({ "scroll": [0.0, 0.0], "zoom": 1.0 }));
    dialog::run(&mut f, "settings.set", json!({ "key": "view.minimap", "value": true }));
    f
}

fn open(f: Fixture) -> H {
    let mut h = dialog::sized(f, WINDOW, egui::Theme::Light);
    h.run();
    h
}

fn card(h: &H) -> Rect {
    h.get_by_label("Minimap").rect()
}

/// The mapping the card draws with now, from what the model holds.
fn mapping(h: &H) -> Mapping {
    let snapshot = &h.state().model.minimap;
    Mapping::fit(snapshot.scene.extent(&snapshot.camera), card(h).shrink(INSET)).unwrap()
}

fn press(h: &mut H, at: Pos2) {
    h.event(Event::PointerMoved(at));
    h.run();
    h.event(Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.run();
}

fn release(h: &mut H, at: Pos2) {
    h.event(Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
}

fn drag_to(h: &mut H, at: Pos2) {
    h.event(Event::PointerMoved(at));
    h.run();
}

/// The `view.set` calls among the recorded actions, with their scroll.
fn scrolls(h: &H) -> Vec<[f64; 2]> {
    h.state()
        .calls()
        .iter()
        .filter(|c| c.id == "view.set")
        .map(|c| serde_json::from_value(c.args["scroll"].clone()).unwrap())
        .collect()
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 0.5 && (a[1] - b[1]).abs() < 0.5
}

fn camera_scroll(f: &mut Fixture) -> [f64; 2] {
    let got = dialog::run(f, "view.get", json!({}));
    serde_json::from_value(got["scroll"].clone()).unwrap()
}

// ---- off, and what off costs ----

#[test]
fn it_is_off_by_default_and_draws_and_builds_nothing() {
    let mut f = dialog::fixture();
    add_rect(&mut f.session, 0.0, 0.0, 100.0, 80.0).unwrap();
    f.refresh();
    assert!(!f.model.setting_bool("view.minimap"), "off in a new session");
    assert!(!f.model.minimap.on);
    assert!(f.model.minimap.scene.marks().is_empty() && f.model.minimap.scene.elements() == 0, "nothing was built");
    let mut h = open(f);
    assert!(h.query_by_label("Minimap").is_none(), "no card");
    assert!(h.state().state.chrome.minimap.is_none());
    h.run();
}

#[test]
fn turning_it_off_drops_what_it_built() {
    let mut f = on();
    f.refresh();
    assert!(f.model.minimap.scene.elements() == 3);
    dialog::run(&mut f, "settings.toggle", json!({ "key": "view.minimap" }));
    assert!(!f.model.minimap.on && f.model.minimap.scene.marks().is_empty());
}

// ---- what it shows ----

#[test]
fn it_shows_the_document_and_follows_its_changes() {
    let mut f = on();
    f.refresh();
    let scene = &f.model.minimap.scene;
    assert_eq!(scene.elements(), 3);
    assert_eq!(scene.content(), Some(WorldRect { min: [0.0, 0.0], max: [1000.0, 480.0] }));
    let id = add_rect(&mut f.session, 2000.0, 0.0, 50.0, 50.0).unwrap();
    f.refresh();
    assert_eq!(f.model.minimap.scene.elements(), 4, "a new element is in the next frame");
    assert_eq!(f.model.minimap.scene.content().unwrap().max[0], 2050.0);
    dialog::run(&mut f, "element.delete", json!({ "ids": [id.as_str()] }));
    assert_eq!(f.model.minimap.scene.elements(), 3, "a deleted one is gone");
    dialog::run(&mut f, "history.undo", json!({}));
    assert_eq!(f.model.minimap.scene.elements(), 4, "undo brings it back");
}

#[test]
fn another_tab_has_its_own_picture() {
    let mut f = on();
    f.refresh();
    dialog::run(&mut f, "tab.new", json!({}));
    assert_eq!(f.model.minimap.scene.elements(), 0, "the new tab is empty");
    dialog::run(&mut f, "tab.switch", json!({ "tab": 0 }));
    assert_eq!(f.model.minimap.scene.elements(), 3);
}

#[test]
fn the_rectangle_follows_the_camera_and_the_apps_live_camera_wins() {
    let mut f = on();
    dialog::run(&mut f, "view.set", json!({ "scroll": [300.0, 200.0], "zoom": 2.0 }));
    assert_eq!(f.model.minimap.camera.scroll, [300.0, 200.0]);
    assert_eq!(f.model.minimap.camera.zoom, 2.0);
    let live = ViewView { scroll: [-50.0, 60.0], zoom: 0.5, width: 1000.0, height: 500.0 };
    f.model.live_camera = Some(live);
    f.refresh();
    assert_eq!(f.model.minimap.camera, live, "the canvas is ahead of the session while it pans");
    assert_eq!(visible(&live).size(), [2000.0, 1000.0]);
}

#[test]
fn a_filled_shape_is_filled_and_an_empty_one_is_an_outline() {
    let mut store = MemoryStore::new();
    put(
        &mut store,
        &[("a", [0.0, 0.0, 100.0, 50.0], Some(Color::rgb(200, 30, 30))), ("b", [200.0, 100.0, 100.0, 50.0], None)],
    );
    let scene = Scene::build(&store);
    assert!(matches!(scene.marks()[0].look, Look::Fill(_)));
    assert!(matches!(scene.marks()[1].look, Look::Outline(_)));
    assert_eq!(scene.content(), Some(WorldRect { min: [0.0, 0.0], max: [300.0, 150.0] }));
}

#[test]
fn the_dark_theme_shows_the_colours_the_dark_canvas_draws() {
    let stored = Color::rgb(200, 30, 30);
    let mut store = MemoryStore::new();
    put(&mut store, &[("a", [0.0, 0.0, 100.0, 50.0], Some(stored))]);
    let Look::Fill(tint) = Scene::build(&store).marks()[0].look else { panic!("a filled shape is filled") };
    assert_eq!(tint.pick(false), drafftink_ui::colors::to_egui(stored));
    assert_eq!(tint.pick(true), drafftink_ui::colors::to_egui(stored.for_dark_theme()));
    assert_ne!(tint.pick(true), tint.pick(false), "the dark canvas recolours it");
}

#[test]
fn a_large_document_is_merged_to_a_bounded_number_of_marks_and_still_knows_its_size() {
    let mut store = MemoryStore::new();
    let n = MAX_MARKS + 500;
    let items: Vec<_> = (0..n)
        .map(|i| {
            (
                format!("e{i}"),
                [(i % 100) as f64 * 30.0, (i / 100) as f64 * 30.0, 20.0, 20.0],
                Some(Color::rgb(10, 120, 200)),
            )
        })
        .collect();
    put_owned(&mut store, &items);
    let scene = Scene::build(&store);
    assert_eq!(scene.elements(), n);
    assert!(scene.marks().len() < n && scene.marks().len() <= 48 * 32 + 512, "{} marks", scene.marks().len());
    let content = scene.content().unwrap();
    assert!(scene.marks().iter().all(|m| content.contains(m.bounds.center())));
}

#[test]
fn a_merged_document_keeps_its_large_shapes_and_a_cell_takes_the_topmost_colour() {
    let mut store = MemoryStore::new();
    let mut items = vec![("frame".to_owned(), [0.0, 0.0, 10_000.0, 10_000.0], None)];
    for i in 0..MAX_MARKS {
        // All in one corner cell: red, with a blue one on top.
        let colour = if i + 1 == MAX_MARKS { Color::rgb(0, 0, 255) } else { Color::rgb(255, 0, 0) };
        items.push((format!("e{i}"), [1.0, 1.0, 2.0, 2.0], Some(colour)));
    }
    put_owned(&mut store, &items);
    let scene = Scene::build(&store);
    assert_eq!(scene.marks().len(), 2, "the big shape and one cell");
    assert!(matches!(scene.marks()[0].look, Look::Outline(_)));
    let Look::Fill(tint) = scene.marks()[1].look else { panic!("a cell is filled") };
    assert_eq!(tint.pick(false), drafftink_ui::colors::to_egui(Color::rgb(0, 0, 255)));
}

#[test]
fn the_extent_holds_the_content_and_the_camera() {
    let mut store = MemoryStore::new();
    put(&mut store, &[("a", [0.0, 0.0, 100.0, 100.0], None)]);
    let view = ViewView { scroll: [5000.0, 0.0], zoom: 1.0, width: 800.0, height: 600.0 };
    let extent = Scene::build(&store).extent(&view);
    assert!(extent.contains([0.0, 0.0]) && extent.contains([5000.0, 0.0]) && extent.contains([5400.0, 300.0]));
    let empty = Scene::default().extent(&view);
    assert_eq!(empty, visible(&view), "an empty document shows the camera alone");
}

fn put(store: &mut MemoryStore, items: &[(&str, [f64; 4], Option<Color>)]) {
    let owned: Vec<_> = items.iter().map(|(id, b, c)| ((*id).to_owned(), *b, *c)).collect();
    put_owned(store, &owned);
}

fn put_owned(store: &mut MemoryStore, items: &[(String, [f64; 4], Option<Color>)]) {
    let mut tx = store.begin();
    for (id, b, fill) in items {
        let index = tx.next_top_index().unwrap();
        let mut e = Element::new(ElementId::new(id).unwrap(), ElementKind::Rectangle, index)
            .with_bounds(b[0], b[1], b[2], b[3]);
        e.style.fill_color = *fill;
        tx.insert(e).unwrap();
    }
    tx.commit().unwrap();
}

// ---- where the card is ----

#[test]
fn the_card_is_at_the_bottom_right_corner() {
    let h = open(on());
    let panel = h.state().state.chrome.minimap.unwrap();
    assert!((WINDOW.x - panel.right() - 12.0).abs() < 1.5, "the window margin on the right: {panel:?}");
    assert!((WINDOW.y - panel.bottom() - 12.0).abs() < 1.5, "and at the bottom: {panel:?}");
    assert!(panel.contains_rect(card(&h)));
}

/// The rectangles of the other chrome in this frame.
fn others(h: &H) -> Vec<(&'static str, Rect)> {
    let c = h.state().state.chrome;
    [("strip", c.strip), ("tools", c.tools), ("bottom bar", c.bottom_bar), ("tabs", c.tabs), ("library", c.library)]
        .into_iter()
        .filter_map(|(name, r)| r.map(|r| (name, r)))
        .collect()
}

#[test]
fn the_card_stays_in_the_window_and_clear_of_the_other_chrome_at_every_size_and_theme() {
    let sizes = [(1280.0, 800.0), (1024.0, 640.0), (820.0, 1180.0), (390.0, 844.0), (1180.0, 820.0)];
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        for (w, height) in sizes {
            for library in [false, true] {
                let mut f = on();
                for name in ["Second", "Third"] {
                    dialog::run(&mut f, "tab.new", json!({}));
                    dialog::run(&mut f, "tab.rename", json!({ "name": name }));
                }
                if library {
                    f.state.apply_verb(Verb::ToggleLibrary, &mut Vec::new());
                    f.model.wants_library = true;
                    f.refresh();
                }
                let mut h = dialog::sized(f, vec2(w, height), theme);
                h.run();
                let panel = h.state().state.chrome.minimap.unwrap();
                let screen = Rect::from_min_size(Pos2::ZERO, vec2(w, height));
                let here = format!("{theme:?} {w}x{height} library {library}: {panel:?}");
                assert!(screen.contains_rect(panel), "{here} is inside the window");
                for (name, r) in others(&h) {
                    assert!(!panel.intersects(r), "{here} overlaps the {name} {r:?}");
                }
            }
        }
    }
}

#[test]
fn the_presence_panel_stands_above_the_card() {
    let mut f = on();
    f.model.collab = CollabView {
        room: Some("r1".into()),
        peers: vec![Peer {
            id: "p1".into(),
            name: "Ana".into(),
            color: "#ef4444".into(),
            has_cursor: false,
            followed: false,
        }],
        ..CollabView::default()
    };
    let h = open(f);
    let panel = h.state().state.chrome.minimap.unwrap();
    let ana = h.get_by_label("Follow Ana").rect();
    assert!(ana.bottom() < panel.top(), "Ana's row {ana:?} is above the card {panel:?}");
}

#[test]
fn it_is_part_of_the_chrome_so_zen_mode_hides_it() {
    let mut f = on();
    f.state.apply_verb(Verb::ToggleZen, &mut Vec::new());
    let h = open(f);
    assert!(h.query_by_label("Minimap").is_none());
}

#[test]
fn the_touch_layout_has_a_larger_card_and_a_phone_a_smaller_one() {
    let desktop = card(&open(on())).size();
    let mut tablet_f = on();
    tablet_f.state.set_touch_first(true);
    let mut tablet = dialog::sized(tablet_f, vec2(1180.0, 820.0), egui::Theme::Light);
    tablet.run();
    let mut phone = dialog::sized(on(), vec2(390.0, 844.0), egui::Theme::Light);
    phone.run();
    assert!(card(&tablet).width() > desktop.x, "{:?} against {desktop:?}", card(&tablet).size());
    assert!(card(&phone).width() < desktop.x, "{:?}", card(&phone).size());
}

// ---- moving the view ----

#[test]
fn a_click_outside_the_rectangle_centres_the_view_there() {
    let mut h = open(on());
    let m = mapping(&h);
    let target = [900.0, 400.0]; // right of what the camera sees (x up to 640)
    let at = m.to_card(target);
    press(&mut h, at);
    let calls = scrolls(&h);
    assert_eq!(calls.len(), 1, "one call for one click: {calls:?}");
    assert!(near(calls[0], target), "{:?} is where the click was", calls[0]);
    h.state_mut().dispatch();
    assert!(near(camera_scroll(h.state_mut()), target), "the session's camera is there");
    release(&mut h, at);
    assert_eq!(scrolls(&h).len(), 0, "the release adds nothing (the calls were dispatched)");
}

#[test]
fn a_click_on_the_rectangle_does_not_jump_the_view() {
    let mut h = open(on());
    let m = mapping(&h);
    // A little off the camera's centre, inside its rectangle.
    press(&mut h, m.to_card([100.0, 50.0]));
    assert!(scrolls(&h).is_empty(), "the view stays where it is: {:?}", scrolls(&h));
}

#[test]
fn a_drag_moves_the_view_with_the_pointer_in_the_mapping_of_the_press() {
    let mut h = open(on());
    let m = mapping(&h);
    let (a, b, c) = ([900.0, 400.0], [950.0, 300.0], [700.0, 100.0]);
    press(&mut h, m.to_card(a));
    h.state_mut().dispatch(); // the camera is now at `a`, and the extent the card would fit has changed
    for (point, want) in [(b, b), (c, c)] {
        h.state_mut().clear();
        drag_to(&mut h, m.to_card(point));
        let calls = scrolls(&h);
        assert_eq!(calls.len(), 1, "one call per move: {calls:?}");
        assert!(
            near(calls[0], want),
            "{:?} is the world point under the pointer in the mapping of the press",
            calls[0]
        );
        h.state_mut().dispatch();
    }
    release(&mut h, m.to_card(c));
    assert!(near(camera_scroll(h.state_mut()), c));
    h.state_mut().clear();
    drag_to(&mut h, m.to_card(a));
    assert!(scrolls(&h).is_empty(), "after the release a move does nothing");
}

#[test]
fn a_drag_that_starts_on_the_rectangle_keeps_its_grip() {
    let mut h = open(on());
    let m = mapping(&h);
    let grip = m.to_card([160.0, 0.0]); // inside the rectangle, right of its centre
    press(&mut h, grip);
    h.state_mut().clear();
    let moved = grip + vec2(10.0, 0.0);
    drag_to(&mut h, moved);
    let calls = scrolls(&h);
    let want = [m.to_world(moved)[0] - 160.0, 0.0];
    assert_eq!(calls.len(), 1);
    assert!(
        near(calls[0], want),
        "{:?} moves by what the pointer moved, from the camera's own place: {want:?}",
        calls[0]
    );
}

#[test]
fn a_drag_past_the_card_holds_the_pointer_on_its_edge() {
    let mut h = open(on());
    let m = mapping(&h);
    let edge = Pos2::new(card(&h).right(), m.to_card([0.0, 0.0]).y);
    press(&mut h, m.to_card([900.0, 400.0]));
    h.state_mut().clear();
    drag_to(&mut h, edge);
    let at_edge = scrolls(&h);
    h.state_mut().clear();
    drag_to(&mut h, edge + vec2(400.0, 0.0));
    assert!(scrolls(&h).is_empty() || scrolls(&h) == at_edge, "beyond the edge is the edge: {:?}", scrolls(&h));
}

#[test]
fn a_click_on_the_card_is_not_a_click_on_the_canvas() {
    // The minimap's presses are commands for the camera; no tool or selection command comes out of them.
    let mut h = open(on());
    let m = mapping(&h);
    press(&mut h, m.to_card([900.0, 400.0]));
    release(&mut h, m.to_card([900.0, 400.0]));
    assert!(h.state().calls().iter().all(|c| c.id == "view.set"), "{:?}", h.state().calls());
    assert!(h.state().calls().iter().all(|c| c.origin == Origin::Ui));
}

// ---- zoom ----

fn wheel(h: &mut H, over: Pos2, y: f32) {
    h.event(Event::PointerMoved(over));
    h.run();
    h.event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, y),
        phase: TouchPhase::Move,
        source: MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    });
    h.run();
}

fn zoom_factors(h: &H) -> Vec<f64> {
    h.state().calls().iter().filter(|c| c.id == "view.zoom_at").map(|c| c.args["factor"].as_f64().unwrap()).collect()
}

#[test]
fn the_wheel_over_the_card_zooms_the_view_about_its_centre() {
    let mut h = open(on());
    let centre = card(&h).center();
    wheel(&mut h, centre, 50.0);
    let up = zoom_factors(&h);
    assert!(!up.is_empty() && up.iter().all(|f| *f > 1.0), "scrolling up zooms in: {up:?}");
    assert!(h.state().calls().iter().all(|c| c.id != "view.zoom_at" || c.args.get("at").is_none()), "about the centre");
    h.state_mut().clear();
    wheel(&mut h, centre, -50.0);
    let down = zoom_factors(&h);
    assert!(!down.is_empty() && down.iter().all(|f| *f < 1.0), "scrolling down zooms out: {down:?}");
}

#[test]
fn the_wheel_runs_the_real_command() {
    let mut h = open(on());
    let before = h.state().model.minimap.camera.zoom;
    let centre = card(&h).center();
    wheel(&mut h, centre, 50.0);
    h.state_mut().dispatch();
    assert!(h.state().model.minimap.camera.zoom > before, "the session zoomed in");
}

#[test]
fn the_wheel_elsewhere_does_nothing_here() {
    let mut h = open(on());
    wheel(&mut h, Pos2::new(640.0, 400.0), 50.0);
    assert!(zoom_factors(&h).is_empty());
}

// ---- the verb, the setting, the menus, the key ----

#[test]
fn the_verb_ends_with_the_setting_toggle_and_the_choice_is_kept() {
    let st = drafftink_ui::UiState::new();
    let routed = route(&UiAction::Verb(Verb::ToggleMinimap), &st);
    let call = Call::new(Origin::Ui, "settings.toggle", json!({ "key": "view.minimap" }));
    assert_eq!(routed, Routed::Command(call));
    assert_eq!(Verb::ToggleMinimap.id(), "ui.toggle_minimap");
    assert_eq!(Verb::ToggleMinimap.setting_toggle(), Some("view.minimap"));

    let mut f = dialog::fixture();
    f.actions.push(UiAction::Verb(Verb::ToggleMinimap));
    f.dispatch();
    assert!(f.model.minimap.on, "the verb turned it on");
    assert_eq!(f.session.settings.get("view.minimap"), Some(&json!(true)), "a setting, so the next run has it");
    f.actions.push(UiAction::Verb(Verb::ToggleMinimap));
    f.dispatch();
    assert!(!f.model.minimap.on);
}

#[test]
fn the_key_is_alt_m_and_runs_the_verb() {
    assert_eq!(verb_key_label(Verb::ToggleMinimap, false).as_deref(), Some("Alt+M"));
    let f = dialog::fixture();
    let event = Event::Key {
        key: Key::M,
        physical_key: Some(Key::M),
        pressed: true,
        repeat: false,
        modifiers: Modifiers { alt: true, ..Modifiers::NONE },
    };
    let actions = resolve_keys(
        &[event],
        &KeyContext {
            model: &f.model,
            state: &f.state,
            egui_wants_keyboard: false,
            pointer: Pointer::default(),
            host_clipboard: false,
        },
    );
    assert_eq!(actions, vec![UiAction::Verb(Verb::ToggleMinimap)]);
}

#[test]
fn the_view_menu_has_a_row_and_it_runs_the_verb() {
    for on_now in [false, true] {
        let mut f = dialog::fixture();
        dialog::run(&mut f, "settings.set", json!({ "key": "view.minimap", "value": on_now }));
        f.state.menu_open = true;
        let mut h = support_a::themed_lenient(f, false);
        h.run();
        let rows = support_a::menu::rows_in(&h, "Main menu");
        let at = |text: &str| {
            rows.iter().position(|r| r.starts_with(text)).unwrap_or_else(|| panic!("no row {text}: {rows:?}"))
        };
        assert!(
            at("Minimap") > at("Library") && at("Minimap") < at("Debug panel"),
            "under View, by the other panels: {rows:?}"
        );
        h.state_mut().clear();
        support_a::menu::row_in(&h, "Main menu", "Minimap").click();
        h.run();
        assert_eq!(dialog::verbs(h.state()), vec![Verb::ToggleMinimap]);
    }
}

#[test]
fn the_palette_lists_the_verb() {
    let mut f = dialog::fixture();
    f.refresh();
    let rows = drafftink_ui::surfaces::palette::rows(&f.model);
    assert!(rows.iter().any(|r| r.id == "ui.toggle_minimap" && r.title == "Show or hide the minimap"));
}

#[test]
fn the_preferences_have_a_switch_for_it() {
    let mut f = dialog::fixture();
    f.state.preferences_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("Minimap").click();
    h.run();
    assert!(h.state().calls().iter().any(|c| c.id == "settings.toggle" && c.args == json!({ "key": "view.minimap" })));
}

// ---- cost ----

/// What a frame costs with the minimap on and off at 1,000 elements, and what a document change costs it. Not run
/// by default (a timing needs a release build and a quiet machine):
/// `cargo test -p drafftink-ui --release --test minimap -- --ignored --nocapture`.
#[test]
#[ignore = "a timing: run it in a release build"]
#[allow(clippy::disallowed_methods)]
fn cost_of_a_frame_with_a_thousand_elements() {
    use std::time::Instant;

    const FRAMES: u32 = 300;
    let mut f = dialog::fixture();
    for i in 0..1000 {
        let (x, y) = (f64::from(i % 40) * 120.0, f64::from(i / 40) * 90.0);
        add_rect(&mut f.session, x, y, 100.0, 70.0).unwrap();
    }
    dialog::run(&mut f, "settings.set", json!({ "key": "view.minimap", "value": true }));
    dialog::run(&mut f, "view.set", json!({ "scroll": [2400.0, 1100.0], "zoom": 0.4 }));
    let ctx = egui::Context::default();
    drafftink_ui::theme::install_fonts(&ctx);
    let frame = |f: &mut Fixture| {
        let input =
            egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)), ..egui::RawInput::default() };
        let output = ctx.run_ui(input, |ui| {
            drafftink_ui::draw(ui, &f.model, &mut f.state);
        });
        std::hint::black_box(ctx.tessellate(output.shapes, output.pixels_per_point));
    };
    let timed = |f: &mut Fixture, label: &str| {
        for _ in 0..30 {
            frame(f);
        }
        let start = Instant::now();
        for _ in 0..FRAMES {
            frame(f);
        }
        let per_frame = start.elapsed().as_secs_f64() * 1e6 / f64::from(FRAMES);
        println!("{label}: {per_frame:.0} us per frame (draw and tessellate)");
        per_frame
    };
    let with = timed(&mut f, "minimap on, 1000 elements");
    dialog::run(&mut f, "settings.set", json!({ "key": "view.minimap", "value": false }));
    let without = timed(&mut f, "minimap off");
    println!("the minimap adds {:.0} us per frame", with - without);
    let mut empty = dialog::fixture();
    dialog::run(&mut empty, "settings.set", json!({ "key": "view.minimap", "value": true }));
    let card_only = timed(&mut empty, "minimap on, empty document");
    dialog::run(&mut empty, "settings.set", json!({ "key": "view.minimap", "value": false }));
    let bare = timed(&mut empty, "minimap off, empty document");
    println!(
        "the card alone adds {:.0} us per frame; the 1000 marks add {:.0} us",
        card_only - bare,
        with - without - (card_only - bare)
    );

    dialog::run(&mut f, "settings.set", json!({ "key": "view.minimap", "value": true }));
    let refresh_cost = |f: &mut Fixture| {
        let mut spent = 0.0;
        for i in 0..100 {
            // A change of the document: the model captures again, and builds the scene again when the minimap is on.
            add_rect(&mut f.session, f64::from(i) * 10.0, -500.0, 5.0, 5.0).unwrap();
            let start = Instant::now();
            f.refresh();
            spent += start.elapsed().as_secs_f64();
        }
        spent * 1e4
    };
    let on = refresh_cost(&mut f);
    dialog::run(&mut f, "settings.set", json!({ "key": "view.minimap", "value": false }));
    let off = refresh_cost(&mut f);
    println!("the model's refresh after a change, 1,000 elements: {on:.0} us with the minimap, {off:.0} us without");
    dialog::run(&mut f, "settings.set", json!({ "key": "view.minimap", "value": true }));
    assert!(f.model.minimap.scene.elements() > 1000);

    let mut store = MemoryStore::new();
    let items: Vec<_> = (0..100_000)
        .map(|i| {
            (
                format!("e{i}"),
                [f64::from(i % 400) * 30.0, f64::from(i / 400) * 30.0, 20.0, 20.0],
                Some(Color::rgb(10, 120, 200)),
            )
        })
        .collect();
    put_owned(&mut store, &items);
    let start = Instant::now();
    let scene = Scene::build(&store);
    println!(
        "building the scene of 100,000 elements: {:.1} ms, {} marks drawn",
        start.elapsed().as_secs_f64() * 1e3,
        scene.marks().len()
    );
}
