//! Files dropped on the window, through the shell's own path: a `DroppedFile` as the desktop and the browser
//! deliver it, `App::handle_files_dropped`, the arguments `drop_args` builds, the schema check of `doc.drop`, and
//! the real command. A mismatch between what the shell sends and what the schema lists fails here.

use base64::Engine as _;
use drafftink_engine::PortError;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::ports::{RenderRequest, Renderer, SceneView};
use drafftink_scene::Kind;
use serde_json::json;

use super::*;
use crate::platform::{NoHost, Platform, SharedClipboard};

/// A 1x1 PNG.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR4nGP4DwQACfsD/fteaysAAAAASUVORK5CYII=";
const EXCALIDRAW: &[u8] = include_bytes!("../../../../tests/fixtures/excalidraw/basic_shapes.excalidraw");
/// A PNG exported by Excalidraw (its own test file): one text, a smiley, in a compressed `bstring` chunk.
const EXCALIDRAW_PNG: &[u8] = include_bytes!("../../../../tests/fixtures/excalidraw/smiley_embedded_v2.png");
const LIBRARY: &[u8] = include_bytes!("../../../../tests/fixtures/excalidraw/fixture_library.excalidrawlib");

fn png_1x1() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(PNG_1X1).expect("a valid base64 text")
}

struct OnePixel;

impl Renderer for OnePixel {
    fn render_png(&self, _: &SceneView, _: &RenderRequest) -> Result<Vec<u8>, PortError> {
        Ok(png_1x1())
    }
}

fn app() -> App {
    let fakes = Fakes::new();
    let mut ports = fakes.ports();
    ports.renderer = Some(Box::new(OnePixel));
    let platform = Platform {
        ports,
        host: Box::new(NoHost),
        wake: WakeCell::new(),
        clipboard: SharedClipboard::new(),
        typesetter: drafftink_engine::services::LazyTypesetter::shared(),
        bridge: None,
        mcp_http: None,
        is_mac: false,
    };
    let mut app = App::new(platform, AppConfig::default());
    app.session.execute("doc.clear", json!({})).expect("an empty document");
    app
}

fn file(name: &str, mime: &str, bytes: &[u8]) -> DroppedFile {
    DroppedFile { name: name.to_owned(), mime: mime.to_owned(), bytes: bytes.to_vec() }
}

/// A drafft.ink document with one rectangle, as the app saves it.
fn drafftink_json() -> Vec<u8> {
    let mut a = app();
    draw_a_rectangle(&mut a);
    export(&mut a, json!({ "format": "json", "scope": "all" }))
}

fn export(app: &mut App, args: serde_json::Value) -> Vec<u8> {
    let out = app.session.execute("doc.export", args).expect("an export");
    let text = out["bytes"]["base64"].as_str().expect("inline bytes");
    base64::engine::general_purpose::STANDARD.decode(text).expect("valid base64")
}

fn draw_a_rectangle(app: &mut App) {
    let bounds = json!({ "x": 0, "y": 0, "w": 50, "h": 40 });
    app.session.execute("element.create", json!({ "kind": "rectangle", "bounds": bounds })).expect("a rectangle");
}

/// A PNG that carries a scene: the app's own export (drafft.ink JSON inside).
fn drafftink_scene_png() -> Vec<u8> {
    let mut a = app();
    draw_a_rectangle(&mut a);
    export(&mut a, json!({ "format": "png", "embed_scene": true }))
}

/// A PNG with one text chunk, written as Excalidraw's `png-chunk-text` writes: `tEXt`, Latin-1.
fn png_with_text_chunk(keyword: &str, text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 1, 1);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.add_text_chunk(keyword.to_owned(), text.to_owned()).expect("a Latin-1 text");
    let mut writer = encoder.write_header().expect("a PNG header");
    writer.write_image_data(&[0]).expect("one pixel");
    drop(writer);
    out
}

/// A PNG that carries an Excalidraw scene in its metadata, in Excalidraw's envelope (not compressed).
fn excalidraw_scene_png() -> Vec<u8> {
    let bstring: String = EXCALIDRAW.iter().copied().map(char::from).collect();
    let envelope = json!({ "version": "1", "encoding": "bstring", "compressed": false, "encoded": bstring });
    png_with_text_chunk(drafftink_io::png_meta::EXCALIDRAW_KEYWORD, &envelope.to_string())
}

/// The shell's drop. With `pointer` it is over the canvas, as in the browser (where it always is); without it the
/// position is unknown, as in a native drag that began outside the window.
fn drop_files(app: &mut App, files: Vec<DroppedFile>, pointer: bool) {
    if pointer {
        app.drop_files_at(&files, Some(Point::new(120.0, 80.0)));
    } else {
        app.handle_files_dropped(files);
    }
}

/// What a drop did to the app: tabs, elements in the active tab, toast codes.
#[derive(Debug, PartialEq)]
struct Effect {
    tabs: usize,
    elements: usize,
    toasts: Vec<String>,
}

fn drop_one(f: DroppedFile, pointer: bool) -> (Effect, App) {
    let mut app = app();
    drop_files(&mut app, vec![f], pointer);
    let toasts = app.ui.state.toasts.iter().map(|t| t.code.clone()).collect();
    let effect = Effect { tabs: app.session.tabs.len(), elements: app.session.store().len(), toasts };
    (effect, app)
}

fn no_schema_error(app: &App) {
    for t in &app.ui.state.toasts {
        assert_ne!(t.code, "bad_args", "the arguments did not match the schema: {}", t.text);
    }
}

/// Both ways the shells call: the pointer over the canvas (the browser always has one) and unknown.
const POINTER: [bool; 2] = [true, false];

#[test]
fn a_dropped_excalidraw_file_opens_whatever_media_type_the_system_gave() {
    for pointer in POINTER {
        // Chromium reports no type for `.excalidraw`; some systems say JSON.
        for mime in ["", "application/json", "application/vnd.excalidraw+json"] {
            let (effect, app) = drop_one(file("sketch.excalidraw", mime, EXCALIDRAW), pointer);
            no_schema_error(&app);
            assert_eq!(effect.toasts, Vec::<String>::new(), "mime {mime:?}, pointer {pointer}");
            assert_eq!(effect.tabs, 2, "a new tab holds the drawing (mime {mime:?}, pointer {pointer})");
        }
    }
}

#[test]
fn a_dropped_drafftink_document_opens_in_a_new_tab() {
    for pointer in POINTER {
        for name in ["plan.json", "plan.drafftink"] {
            let (effect, app) = drop_one(file(name, "application/json", &drafftink_json()), pointer);
            no_schema_error(&app);
            assert_eq!((effect.tabs, effect.toasts.is_empty()), (2, true), "{name}, pointer {pointer}");
        }
    }
}

#[test]
fn a_dropped_library_file_is_imported() {
    for pointer in POINTER {
        let (effect, mut app) = drop_one(file("shapes.excalidrawlib", "", LIBRARY), pointer);
        no_schema_error(&app);
        assert!(effect.toasts.is_empty(), "{:?}", effect.toasts);
        assert_eq!(effect.tabs, 1, "a library adds no tab");
        let list = app.session.execute("library.list", json!({})).expect("the library list");
        assert!(list["items"].as_array().is_some_and(|items| !items.is_empty()), "{list}");
    }
}

fn kinds(app: &App) -> Vec<Kind> {
    app.session.store().iter().map(|e| e.tag()).collect()
}

#[test]
fn a_dropped_png_with_a_scene_opens_the_scene_in_a_new_tab_like_a_document() {
    // What a dropped `.excalidraw` file yields is what the same scene inside a PNG must yield.
    let (by_file, _) = drop_one(file("sketch.excalidraw", "", EXCALIDRAW), true);
    for pointer in POINTER {
        let (effect, app) = drop_one(file("scene.png", "image/png", &drafftink_scene_png()), pointer);
        no_schema_error(&app);
        assert_eq!(effect.toasts, Vec::<String>::new(), "drafft.ink scene, pointer {pointer}");
        assert_eq!((effect.tabs, effect.elements), (2, 1), "drafft.ink scene: a new tab holds the rectangle");
        assert_eq!(kinds(&app), [Kind::Rectangle]);

        let (effect, app) = drop_one(file("scene.png", "image/png", &excalidraw_scene_png()), pointer);
        no_schema_error(&app);
        assert_eq!(effect.toasts, Vec::<String>::new(), "Excalidraw scene, pointer {pointer}");
        assert_eq!(effect, by_file, "Excalidraw scene: the same as the .excalidraw file");
        assert!(effect.elements > 1, "{effect:?}");

        // Excalidraw's own export, compressed.
        let (effect, app) = drop_one(file("smiley.png", "image/png", EXCALIDRAW_PNG), pointer);
        assert_eq!((effect.tabs, effect.elements, effect.toasts.is_empty()), (2, 1, true), "pointer {pointer}");
        assert_eq!(kinds(&app), [Kind::Text]);
    }
}

#[test]
fn a_dropped_png_whose_scene_cannot_be_read_is_inserted_and_a_toast_says_why() {
    let mut zlib_broken = drafftink_scene_png();
    let at = zlib_broken.windows(4).position(|w| w == b"iTXt").expect("the scene chunk");
    let len = u32::from_be_bytes([zlib_broken[at - 4], zlib_broken[at - 3], zlib_broken[at - 2], zlib_broken[at - 1]]);
    let end = at + 4 + usize::try_from(len).expect("a small chunk");
    zlib_broken[end - 6..end].fill(0xff);
    let not_a_document = drafftink_io::png_meta::embed_scene(&png_1x1(), "not a scene").expect("a chunk");
    for (what, png) in [("a broken chunk", zlib_broken), ("text that is no document", not_a_document)] {
        for pointer in POINTER {
            let (effect, app) = drop_one(file("broken.png", "image/png", &png), pointer);
            no_schema_error(&app);
            assert_eq!((effect.tabs, effect.elements), (1, 1), "{what}: the picture, no new tab (pointer {pointer})");
            assert_eq!(kinds(&app), [Kind::Image], "{what}");
            assert_eq!(effect.toasts, ["note"], "{what}");
            let text = &app.ui.state.toasts[0].text;
            assert!(text.contains("`broken.png`") && text.contains("inserted as a picture only"), "{what}: {text}");
        }
    }
}

#[test]
fn a_dropped_plain_png_is_inserted_and_a_broken_jpeg_is_refused_in_words() {
    for pointer in POINTER {
        let (effect, app) = drop_one(file("photo.png", "image/png", &png_1x1()), pointer);
        no_schema_error(&app);
        assert_eq!((effect.tabs, effect.elements, effect.toasts.is_empty()), (1, 1, true), "pointer {pointer}");
        // A JPEG reaches the image path; this one is cut off, so the refusal says it could not be opened.
        let (effect, app) = drop_one(file("photo.jpg", "image/jpeg", &[0xff, 0xd8, 0xff, 0xe0, 0, 0]), pointer);
        no_schema_error(&app);
        assert_eq!(effect.toasts, ["nothing_to_do"], "pointer {pointer}");
    }
}

#[test]
fn a_file_that_is_not_supported_is_refused_in_plain_words() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
    for pointer in POINTER {
        for f in [
            file("notes.txt", "text/plain", b"hello"),
            file("picture.svg", "image/svg+xml", svg),
            file("archive.zip", "", &[0x50, 0x4b, 3, 4]),
            file("empty.json", "application/json", b""),
        ] {
            let name = f.name.clone();
            let (_, app) = drop_one(f, pointer);
            no_schema_error(&app);
            let toasts = &app.ui.state.toasts;
            assert_eq!(toasts.len(), 1, "{name}: {toasts:?}");
            assert_eq!(toasts[0].code, "nothing_to_do", "{name}");
            assert!(toasts[0].text.starts_with("Nothing to open"), "{name}: {}", toasts[0].text);
            assert!(toasts[0].text.contains("Supported: PNG"), "it says what would work: {}", toasts[0].text);
        }
    }
}

#[test]
fn several_files_in_one_drop_are_one_command() {
    let mut app = app();
    let files = vec![
        file("a.excalidraw", "", EXCALIDRAW),
        file("b.png", "image/png", &png_1x1()),
        file("c.excalidrawlib", "", LIBRARY),
        file("d.txt", "text/plain", b"x"),
    ];
    drop_files(&mut app, files, true);
    assert!(app.ui.state.toasts.is_empty(), "{:?}", app.ui.state.toasts);
    assert_eq!(app.session.tabs.len(), 2);
}
