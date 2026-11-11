//! The picture of the editor is the picture of the committed text: the same scenes, rendered on a GPU, are equal
//! pixel for pixel. The editor is drawn through the same fragment code as a document element, so these tests catch a
//! different position, wrap, colour or size, and a caret or selection that leaks into the picture.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use drafftink_engine::interact::{InteractionOverlay, UiRequest};
use drafftink_geom::kurbo::{Point, Size};
use drafftink_gpu::{Compositor, Gpu, GpuOptions, Image};
use drafftink_render::{Overlay, Scene, Theme};
use drafftink_scene::{ElementId, SceneStore};
use serde_json::{Value, json};
use vello::peniko::Color;
use vello::wgpu;

use super::tests::Rig;
use crate::view::CanvasView;

const SIZE: [u32; 2] = [480, 320];

struct Device {
    gpu: Gpu,
    compositor: Compositor,
}

/// The one device of this test binary, shared and never dropped: tearing devices down while others are created
/// can deadlock the NVIDIA Vulkan driver (see `crates/gpu/tests/compositor.rs`). `None` on a machine without an
/// adapter; set `DRAFFTINK_REQUIRE_GPU=1` to make that a failure.
fn device() -> Option<MutexGuard<'static, Device>> {
    static DEVICE: OnceLock<Option<Mutex<Device>>> = OnceLock::new();
    let shared = DEVICE.get_or_init(|| match Gpu::headless_blocking(&GpuOptions::default()) {
        Ok(gpu) => {
            let compositor = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).expect("a compositor");
            Some(Mutex::new(Device { gpu, compositor }))
        }
        Err(e) => {
            assert!(
                std::env::var_os("DRAFFTINK_REQUIRE_GPU").is_none(),
                "no GPU adapter and DRAFFTINK_REQUIRE_GPU is set: {e}"
            );
            eprintln!("skipped: no GPU adapter on this machine");
            None
        }
    });
    shared.as_ref().map(|m| m.lock().unwrap_or_else(PoisonError::into_inner))
}

impl Device {
    fn render(&mut self, scene: &Scene) -> Image {
        self.compositor
            .render_to_image_blocking(&self.gpu, SIZE, Some((scene, Color::WHITE)), None)
            .expect("a rendered image")
    }
}

fn view(zoom: f64) -> drafftink_render::View {
    CanvasView::view_of(Point::new(150.0, 100.0), zoom, Size::new(f64::from(SIZE[0]), f64::from(SIZE[1])), 1.0)
}

/// The scene of the canvas while the editor is open (caret blinked off, nothing selected).
fn editing(r: &mut Rig, zoom: f64) -> Scene {
    let mut canvas = CanvasView::new(r.ts.clone());
    let ts = r.ts.clone();
    let editor = r.editor.as_mut().expect("an editor");
    let arranged = ts.with(|ts| editor.paint(ts).clone());
    canvas.set_hidden(editor.hidden());
    let (tab, revision) = (r.s.tab().id, r.s.store().revision());
    canvas.sync_index(tab, revision, r.s.store(), &[]);
    let view = view(zoom);
    canvas.compose_world(r.s.store(), &view);
    let overlay =
        Overlay { text_edit: Some(ts.with(|ts| editor.overlay(ts, Theme::Light, 600.0))), ..Overlay::default() };
    canvas.compose_overlay(r.s.store(), &InteractionOverlay::default(), &overlay, Some(&arranged), &view);
    let mut scene = Scene::new();
    canvas.write_to(&mut scene);
    scene
}

/// The scene of the document alone.
fn committed(r: &mut Rig, zoom: f64) -> Scene {
    let mut canvas = CanvasView::new(r.ts.clone());
    let (tab, revision) = (r.s.tab().id, r.s.store().revision());
    canvas.sync_index(tab, revision, r.s.store(), &[]);
    canvas.compose_world(r.s.store(), &view(zoom));
    let mut scene = Scene::new();
    canvas.write_to(&mut scene);
    scene
}

fn differing(a: &Image, b: &Image) -> usize {
    a.rgba.chunks(4).zip(b.rgba.chunks(4)).filter(|(p, q)| p != q).count()
}

fn inked(img: &Image) -> usize {
    img.rgba.chunks(4).filter(|p| p[0] < 240 || p[1] < 240 || p[2] < 240).count()
}

struct Case {
    name: &'static str,
    setup: fn(&mut Rig) -> (Value, UiRequest),
    zoom: f64,
}

fn request(id: &ElementId) -> UiRequest {
    UiRequest::OpenTextEditor { element: Some(id.clone()), at: Point::ZERO, width: None }
}

const LONG: &str = "the quick brown fox jumps over the lazy dog and keeps on running";

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "free text",
            zoom: 1.0,
            setup: |r| {
                let id = r.create(json!({ "kind": "text", "text": "colour me", "at": [40, 30] }));
                (json!({ "id": id.as_str() }), request(&id))
            },
        },
        Case {
            name: "fixed width draft",
            zoom: 1.5,
            setup: |r| {
                let req = UiRequest::OpenTextEditor { element: None, at: Point::new(30.0, 20.0), width: Some(170.0) };
                let _ = r;
                (json!({ "at": [30, 20], "width": 170 }), req)
            },
        },
        Case {
            name: "rectangle label that grows",
            zoom: 1.0,
            setup: |r| {
                let id = r.create(json!({ "kind": "rectangle", "bounds": { "x": 40, "y": 40, "w": 120, "h": 50 } }));
                (json!({ "id": id.as_str() }), request(&id))
            },
        },
        Case {
            name: "ellipse label",
            zoom: 1.25,
            setup: |r| {
                let id = r.create(json!({ "kind": "ellipse", "bounds": { "x": 40, "y": 40, "w": 200, "h": 110 } }));
                (json!({ "id": id.as_str() }), request(&id))
            },
        },
        Case {
            name: "turned rectangle label",
            zoom: 1.0,
            setup: |r| {
                let id = r.create(json!({ "kind": "rectangle", "bounds": { "x": 60, "y": 50, "w": 160, "h": 70 } }));
                r.run("element.rotate", json!({ "ids": [id.as_str()], "mode": "to", "angle": 25 }));
                (json!({ "id": id.as_str() }), request(&id))
            },
        },
        Case {
            name: "arrow label",
            zoom: 1.0,
            setup: |r| {
                let id = r.create(json!({ "kind": "arrow", "points": [[20, 30], [280, 110]] }));
                (json!({ "id": id.as_str() }), request(&id))
            },
        },
    ]
}

#[test]
fn the_editor_draws_the_pixels_of_the_committed_text() {
    let Some(mut dev) = device() else { return };
    for case in cases() {
        let mut r = Rig::new();
        let (begin, req) = (case.setup)(&mut r);
        r.begin(begin, req);
        r.type_text(LONG);
        // A coloured word, chosen as the style panel does.
        r.editor().model.select_chars(4..9);
        assert!(r.editor().add_style(&json!({ "stroke_color": "#d6336c" }), 0.0), "{}", case.name);
        r.editor().model.select_chars(9..9);

        let while_editing = dev.render(&editing(&mut r, case.zoom));
        r.commit();
        let after = dev.render(&committed(&mut r, case.zoom));
        assert!(inked(&after) > 300, "{}: the text is on the canvas", case.name);
        assert_eq!(differing(&while_editing, &after), 0, "{}: the editor and the commit differ", case.name);
    }
}

#[test]
fn the_caret_and_the_selection_are_drawn_over_the_text_and_nothing_else_changes() {
    let Some(mut dev) = device() else { return };
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "select some of this", "at": [40, 30] }));
    r.begin(json!({ "id": id.as_str() }), request(&id));
    let plain = dev.render(&editing(&mut r, 1.0));

    // The caret shows on the on-phase of the blink.
    let ts = r.ts.clone();
    let mut with_caret = |r: &mut Rig, now: f64| {
        let mut canvas = CanvasView::new(r.ts.clone());
        let editor = r.editor.as_mut().expect("an editor");
        let arranged = ts.with(|ts| editor.paint(ts).clone());
        canvas.set_hidden(editor.hidden());
        let (tab, revision) = (r.s.tab().id, r.s.store().revision());
        canvas.sync_index(tab, revision, r.s.store(), &[]);
        canvas.compose_world(r.s.store(), &view(1.0));
        let overlay =
            Overlay { text_edit: Some(ts.with(|ts| editor.overlay(ts, Theme::Light, now))), ..Overlay::default() };
        canvas.compose_overlay(r.s.store(), &InteractionOverlay::default(), &overlay, Some(&arranged), &view(1.0));
        let mut scene = Scene::new();
        canvas.write_to(&mut scene);
        dev.render(&scene)
    };
    let on = with_caret(&mut r, 100.0);
    let n = differing(&plain, &on);
    assert!((1..200).contains(&n), "a caret is a thin bar: {n} pixels");

    r.editor().model.select_chars(7..11);
    let selected = with_caret(&mut r, 100.0);
    let m = differing(&plain, &selected);
    assert!(m > n * 3, "a selection is a block of colour: {m} pixels against the caret's {n}");
    // A selection has no caret: the bar of the caret is not drawn on top of it.
    let blinked_off = with_caret(&mut r, 600.0);
    assert_eq!(differing(&selected, &blinked_off), 0);
}

#[test]
fn a_formula_under_the_caret_shows_its_source_and_a_formula_elsewhere_is_typeset() {
    let Some(mut dev) = device() else { return };
    let mut r = Rig::new();
    let id = r.create(json!({ "kind": "text", "text": "area $\\frac{a}{b}$ done", "at": [40, 30] }));
    r.begin(json!({ "id": id.as_str() }), request(&id));
    let typeset = dev.render(&editing(&mut r, 1.0));
    let committed_before = dev.render(&committed(&mut r, 1.0));
    // Nothing is hidden by the editor's formula handling while the caret is outside it.
    assert!(differing(&typeset, &committed_before) == 0 || inked(&typeset) > 100);

    // The caret inside the formula's source: the layout shows the source characters.
    r.editor().model.select_chars(8..8);
    let source = dev.render(&editing(&mut r, 1.0));
    assert!(differing(&typeset, &source) > 200, "the source differs from the typeset formula");
    let (drawn, _) = r.drawn();
    let text = drawn.text_data().expect("text").content.clone();
    assert_eq!(text, "area $\\frac{a}{b}$ done", "the content keeps its dollar signs");
    r.commit();
    let after = dev.render(&committed(&mut r, 1.0));
    assert_eq!(differing(&after, &committed_before), 0, "ending the edit typesets the formula again");
}
