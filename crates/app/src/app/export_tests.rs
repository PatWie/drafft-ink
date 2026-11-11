//! Exporting a PNG without the dialog (`Mod+E`, the same call as the menu row and the palette row) in the app:
//! the keys go through the UI shell and the real session, the picture is drawn by the renderer port and the file
//! lands in the files port, also when the picture is late (the browser's renderer), and a refusal is a toast.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use drafftink_engine::PortError;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::ports::{FileRef, RenderRequest, Renderer, SceneView};
use serde_json::json;

use super::*;
use crate::platform::{NoHost, Platform, SharedClipboard};

/// A renderer that answers `NotReady` for its first calls, as the browser's does while it draws.
struct Late {
    not_ready: AtomicU32,
    calls: Arc<AtomicU32>,
}

impl Renderer for Late {
    fn render_png(&self, _: &SceneView, _: &RenderRequest) -> Result<Vec<u8>, PortError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.not_ready.load(Ordering::SeqCst) > 0 {
            self.not_ready.fetch_sub(1, Ordering::SeqCst);
            return Err(PortError::NotReady);
        }
        Ok(b"\x89PNG-late".to_vec())
    }
}

fn app_with(fakes: &Fakes, late: Option<Late>) -> App {
    let mut ports = fakes.ports();
    if let Some(late) = late {
        ports.renderer = Some(Box::new(late));
    }
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
    app.session.execute("doc.rename", json!({ "name": "Plan" })).expect("a name");
    app
}

fn draw_a_rectangle(app: &mut App) {
    let bounds = json!({ "x": 0, "y": 0, "w": 50, "h": 40 });
    app.session.execute("element.create", json!({ "kind": "rectangle", "bounds": bounds })).expect("a rectangle");
}

/// One egui frame of the UI shell, with these events, as `App::frame` runs it.
fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    let input = egui::RawInput { events, ..egui::RawInput::default() };
    let mut out = ctx.run_ui(input, |ui| {
        let input = FrameInput { pointer: Pointer::default(), host_clipboard: true };
        let _ = app.ui.frame(ui, &mut app.session, input);
    });
    out.textures_delta.clear();
}

fn ctrl_e() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::E,
        physical_key: Some(egui::Key::E),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
    }
}

fn download(fakes: &Fakes, name: &str) -> Option<Vec<u8>> {
    fakes.files.bytes(&FileRef::Download { download: name.to_owned() })
}

#[test]
fn mod_e_writes_the_picture_as_a_png_download_named_after_the_document() {
    let fakes = Fakes::new();
    let mut app = app_with(&fakes, None);
    draw_a_rectangle(&mut app);
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![ctrl_e()]);
    let bytes = download(&fakes, "Plan.png").expect("Plan.png was written to the files port");
    assert!(bytes.starts_with(b"\x89PNG"), "{:?}", &bytes[..bytes.len().min(10)]);
    assert!(app.ui.state.toasts.is_empty(), "{:?}", app.ui.state.toasts);
}

#[test]
fn a_picture_that_is_late_is_written_when_it_is_ready_without_a_second_key_press() {
    let fakes = Fakes::new();
    let calls = Arc::new(AtomicU32::new(0));
    let late = Late { not_ready: AtomicU32::new(2), calls: Arc::clone(&calls) };
    let mut app = app_with(&fakes, Some(late));
    draw_a_rectangle(&mut app);
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![ctrl_e()]);
    assert!(download(&fakes, "Plan.png").is_none(), "the picture is not there yet");
    assert!(app.ui.state.waiting_for_picture());
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    assert!(!app.ui.state.waiting_for_picture());
    assert_eq!(calls.load(Ordering::SeqCst), 3, "the same export ran again until the picture was ready");
    assert!(download(&fakes, "Plan.png").is_some_and(|b| b.starts_with(b"\x89PNG")));
    let texts: Vec<_> = app.ui.state.toasts.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts.len(), 1, "one note that the picture is being drawn, no error: {texts:?}");
}

#[test]
fn mod_e_on_an_empty_drawing_says_why_nothing_was_exported() {
    let fakes = Fakes::new();
    let mut app = app_with(&fakes, None);
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![ctrl_e()]);
    assert!(download(&fakes, "Plan.png").is_none());
    let texts: Vec<_> = app.ui.state.toasts.iter().map(|t| t.text.clone()).collect();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].contains("Nothing to export"), "{}", texts[0]);
}
