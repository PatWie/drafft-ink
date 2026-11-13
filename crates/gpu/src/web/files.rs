//! File drop on the canvas. winit's web backend sends no `DroppedFile`, so the host listens for the
//! drag events itself and reads every dropped file completely.

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen_futures::spawn_local;
use web_sys::{DataTransfer, DragEvent, EventTarget, HtmlCanvasElement};

use super::dom::{self, ListenOptions, Listener};
use crate::events::{DroppedFile, ShellEvent};
use crate::host::HostSink;

/// Listeners for `dragenter`, `dragover`, `dragleave` and `drop` on the canvas, plus window-level guards
/// so a file dropped beside the canvas does not make the browser navigate to it.
pub(super) fn install(canvas: &HtmlCanvasElement, sink: &HostSink) -> Result<Vec<Listener>, String> {
    let target: &EventTarget = canvas.as_ref();
    let opts = ListenOptions::default();
    let hovering = Rc::new(Cell::new(false));
    let mut out = Vec::new();

    let set_hover = {
        let sink = sink.clone();
        let hovering = Rc::clone(&hovering);
        move |on: bool| {
            if hovering.replace(on) != on {
                sink.app(ShellEvent::FileHover(on));
            }
        }
    };

    let hover = set_hover.clone();
    out.push(Listener::new(target, "dragenter", opts, move |e: DragEvent| {
        if carries_files(&e) {
            e.prevent_default();
            hover(true);
        }
    })?);
    let hover = set_hover.clone();
    out.push(Listener::new(target, "dragover", opts, move |e: DragEvent| {
        if carries_files(&e) {
            // Without this the browser refuses the drop.
            e.prevent_default();
            if let Some(data) = e.data_transfer() {
                data.set_drop_effect("copy");
            }
            hover(true);
        }
    })?);
    let hover = set_hover.clone();
    out.push(Listener::new(target, "dragleave", opts, move |e: DragEvent| {
        if carries_files(&e) {
            hover(false);
        }
    })?);
    let sink = sink.clone();
    out.push(Listener::new(target, "drop", opts, move |e: DragEvent| {
        if !carries_files(&e) {
            return;
        }
        e.prevent_default();
        set_hover(false);
        let Some(data) = e.data_transfer() else { return };
        // The list is only valid during the event; the File objects stay valid.
        let files = collect_files(&data);
        if files.is_empty() {
            return;
        }
        let sink = sink.clone();
        spawn_local(async move {
            let mut dropped = Vec::with_capacity(files.len());
            for file in files {
                let name = file.name();
                match dom::read_blob_bytes(file.as_ref()).await {
                    Ok(bytes) => {
                        let mime = match file.type_() {
                            m if m.is_empty() => guess_mime(&name).to_owned(),
                            m => m,
                        };
                        dropped.push(DroppedFile { name, mime, bytes });
                    }
                    Err(err) => log::warn!("skipping dropped file {name}: {err}"),
                }
            }
            if !dropped.is_empty() {
                sink.app(ShellEvent::FilesDropped(dropped));
            }
        });
    })?);

    // Outside the canvas the default action of a dropped file is to open it in the tab.
    let window = dom::window()?;
    let window_target: &EventTarget = window.as_ref();
    for name in ["dragover", "drop"] {
        out.push(Listener::new(window_target, name, opts, |e: DragEvent| {
            if carries_files(&e) {
                e.prevent_default();
            }
        })?);
    }
    Ok(out)
}

fn carries_files(event: &DragEvent) -> bool {
    event.data_transfer().is_some_and(|data| data.types().iter().any(|t| t.as_string().is_some_and(|t| t == "Files")))
}

fn collect_files(data: &DataTransfer) -> Vec<web_sys::File> {
    let mut files = Vec::new();
    if let Some(list) = data.files() {
        for i in 0..list.length() {
            if let Some(file) = list.item(i) {
                files.push(file);
            }
        }
    }
    files
}

/// A MIME type from the file name, for browsers or files that give none.
fn guess_mime(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        "txt" => "text/plain",
        _ => "",
    }
}
