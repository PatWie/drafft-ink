//! Test support for the browser tests (`examples/web_host_probe.rs`, `scripts/web_host_test.py`).
//!
//! `WebHost` and `HostSink` are crate-private, so an example cannot build them. [`ProbeHost`] installs a
//! host whose sink turns every event into one JSON line and hands it to a JS callback. Not part of the
//! app.

use std::fmt::Write as _;

use wasm_bindgen::JsValue;
use web_sys::HtmlCanvasElement;

use super::WebHost;
use crate::events::{ClipboardContent, ImageBytes, PointerPoint, ShellEvent, TextInput};
use crate::host::{HostEvent, HostSink};

/// Find the canvas with this element id.
pub fn lookup_canvas(id: &str) -> Result<HtmlCanvasElement, String> {
    super::lookup_canvas(id)
}

/// A [`WebHost`] reporting events to a JS function as JSON text.
pub struct ProbeHost {
    host: Option<WebHost>,
}

impl ProbeHost {
    /// Install the host on `canvas`. `callback(json: string)` runs for every event.
    pub fn install(canvas: &HtmlCanvasElement, callback: js_sys::Function) -> Result<Self, String> {
        let sink = HostSink::new(move |event| {
            let json = event_json(&event);
            if let Err(e) = callback.call1(&JsValue::NULL, &JsValue::from_str(&json)) {
                log::warn!("probe callback failed: {e:?}");
            }
        });
        Ok(Self { host: Some(WebHost::install(canvas, sink)?) })
    }

    /// Remove every listener and the textarea (drops the host).
    pub fn uninstall(&mut self) {
        self.host = None;
    }

    pub fn is_installed(&self) -> bool {
        self.host.is_some()
    }

    pub fn write_clipboard(&self, text: Option<String>, png: Option<Vec<u8>>, custom: Option<Vec<u8>>) {
        if let Some(host) = &self.host {
            let images = png.map(|bytes| ImageBytes { mime: "image/png".to_owned(), bytes }).into_iter().collect();
            host.write_clipboard(ClipboardContent { text, images, custom });
        }
    }

    pub fn read_clipboard(&self) {
        if let Some(host) = &self.host {
            host.read_clipboard();
        }
    }

    /// `Some((x, y, w, h))` in CSS pixels relative to the canvas, or `None` to hide.
    pub fn set_text_input(&self, rect: Option<(f32, f32, f32, f32)>) {
        if let Some(host) = &self.host {
            host.set_text_input(rect.map(|(x, y, w, h)| egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))));
        }
    }

    /// The hidden textarea has keyboard focus.
    pub fn text_input_focused(&self) -> bool {
        self.host.as_ref().is_some_and(WebHost::text_input_focused)
    }

    pub fn open_url(&self, url: &str, new_tab: bool) {
        if let Some(host) = &self.host {
            host.open_url(url, new_tab);
        }
    }
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(value: f64) -> String {
    if value.is_finite() { format!("{value}") } else { "null".to_owned() }
}

fn opt<T>(value: Option<T>, f: impl FnOnce(T) -> String) -> String {
    value.map_or_else(|| "null".to_owned(), f)
}

fn mods(m: egui::Modifiers) -> String {
    format!(
        "{{\"alt\":{},\"ctrl\":{},\"shift\":{},\"mac_cmd\":{},\"command\":{}}}",
        m.alt, m.ctrl, m.shift, m.mac_cmd, m.command
    )
}

fn point(p: &PointerPoint) -> String {
    format!(
        "{{\"x\":{},\"y\":{},\"pressure\":{},\"time_ms\":{}}}",
        num(f64::from(p.pos.x)),
        num(f64::from(p.pos.y)),
        opt(p.pressure, |v| num(f64::from(v))),
        num(p.time_ms)
    )
}

fn bytes_summary(bytes: &[u8]) -> String {
    let head: String = bytes.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("\"len\":{},\"head\":\"{head}\"", bytes.len())
}

fn app_json(event: &ShellEvent) -> String {
    match event {
        ShellEvent::Pointer(s) => {
            let coalesced: Vec<String> = s.coalesced.iter().map(point).collect();
            format!(
                "{{\"t\":\"pointer\",\"phase\":\"{:?}\",\"kind\":\"{:?}\",\"id\":{},\"point\":{},\"tilt\":{},\"button\":{},\"mods\":{},\"coalesced\":[{}]}}",
                s.phase,
                s.kind,
                s.id,
                point(&s.point),
                opt(s.tilt, |t| format!("[{},{}]", num(f64::from(t[0])), num(f64::from(t[1])))),
                opt(s.button, |b| b.to_string()),
                mods(s.modifiers),
                coalesced.join(",")
            )
        }
        ShellEvent::Paste(c) => {
            let images: Vec<String> = c
                .images
                .iter()
                .map(|i| format!("{{\"mime\":{},{}}}", quote(&i.mime), bytes_summary(&i.bytes)))
                .collect();
            format!(
                "{{\"t\":\"paste\",\"text\":{},\"images\":[{}],\"custom\":{}}}",
                opt(c.text.as_deref(), quote),
                images.join(","),
                opt(c.custom.as_deref(), |b| format!(
                    "{{{},\"text\":{}}}",
                    bytes_summary(b),
                    quote(&String::from_utf8_lossy(b))
                ))
            )
        }
        ShellEvent::CopyRequested { cut } => format!("{{\"t\":\"copy\",\"cut\":{cut}}}"),
        ShellEvent::FilesDropped(files) => {
            let files: Vec<String> = files
                .iter()
                .map(|f| {
                    format!("{{\"name\":{},\"mime\":{},{}}}", quote(&f.name), quote(&f.mime), bytes_summary(&f.bytes))
                })
                .collect();
            format!("{{\"t\":\"files_dropped\",\"files\":[{}]}}", files.join(","))
        }
        ShellEvent::FileHover(on) => format!("{{\"t\":\"file_hover\",\"on\":{on}}}"),
        ShellEvent::TextInput(TextInput::Preedit { text, cursor }) => {
            format!(
                "{{\"t\":\"text_input\",\"kind\":\"preedit\",\"text\":{},\"cursor\":{}}}",
                quote(text),
                opt(*cursor, |(a, b)| format!("[{a},{b}]"))
            )
        }
        ShellEvent::TextInput(TextInput::Commit(text)) => {
            format!("{{\"t\":\"text_input\",\"kind\":\"commit\",\"text\":{}}}", quote(text))
        }
        ShellEvent::TextInput(TextInput::Cancel) => "{\"t\":\"text_input\",\"kind\":\"cancel\"}".to_owned(),
        ShellEvent::Visibility(on) => format!("{{\"t\":\"visibility\",\"visible\":{on}}}"),
        ShellEvent::Focus(on) => format!("{{\"t\":\"focus\",\"focused\":{on}}}"),
        other => format!("{{\"t\":\"other\",\"debug\":{}}}", quote(&format!("{other:?}"))),
    }
}

#[allow(deprecated)] // the deprecated Ime variants are still sent
fn egui_json(event: &egui::Event) -> String {
    use egui::{Event, ImeEvent};
    match event {
        Event::Text(text) => format!("{{\"t\":\"egui_text\",\"text\":{}}}", quote(text)),
        Event::Key { key, physical_key, pressed, repeat, modifiers } => format!(
            "{{\"t\":\"egui_key\",\"key\":\"{key:?}\",\"physical\":{},\"pressed\":{pressed},\"repeat\":{repeat},\"mods\":{}}}",
            opt(*physical_key, |k| format!("\"{k:?}\"")),
            mods(*modifiers)
        ),
        Event::Ime(ImeEvent::Enabled) => "{\"t\":\"egui_ime\",\"kind\":\"enabled\"}".to_owned(),
        Event::Ime(ImeEvent::Disabled) => "{\"t\":\"egui_ime\",\"kind\":\"disabled\"}".to_owned(),
        Event::Ime(ImeEvent::Preedit { text, .. }) => {
            format!("{{\"t\":\"egui_ime\",\"kind\":\"preedit\",\"text\":{}}}", quote(text))
        }
        Event::Ime(ImeEvent::Commit(text)) => {
            format!("{{\"t\":\"egui_ime\",\"kind\":\"commit\",\"text\":{}}}", quote(text))
        }
        Event::Ime(ImeEvent::DeleteSurrounding { before_chars, after_chars }) => {
            format!("{{\"t\":\"egui_ime\",\"kind\":\"delete\",\"before\":{before_chars},\"after\":{after_chars}}}")
        }
        Event::WindowFocused(on) => format!("{{\"t\":\"egui_window_focused\",\"focused\":{on}}}"),
        other => format!("{{\"t\":\"egui_other\",\"debug\":{}}}", quote(&format!("{other:?}"))),
    }
}

fn event_json(event: &HostEvent) -> String {
    match event {
        HostEvent::Egui(e) => egui_json(e),
        HostEvent::App(e) => app_json(e),
    }
}
