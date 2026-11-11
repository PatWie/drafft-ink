//! [`WebHost`]: pickers through a hidden file input, the save target as a download name, the async clipboard.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{File, HtmlInputElement};

use super::dom::{document, js_error};
use super::pure::{ACCEPT_DOCUMENT, ACCEPT_IMAGES, ACCEPT_LIBRARY, MAX_PICKED_BYTES, download_name};
use crate::platform::{Host, HostJob, Inbound, Inbox, PickKind, PickedFile};
use drafftink_engine::ports::FileRef;

/// The browser's [`Host`].
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct WebHost;

impl Host for WebHost {
    fn start(&mut self, job: HostJob, reply: Inbox) {
        match job {
            HostJob::PickDocument => pick(PickKind::Document, ACCEPT_DOCUMENT, false, reply),
            HostJob::PickImages => pick(PickKind::Images, ACCEPT_IMAGES, true, reply),
            HostJob::PickLibrary => pick(PickKind::Library, ACCEPT_LIBRARY, false, reply),
            HostJob::PickSaveTarget { suggested } => {
                // A browser asks for the place at download time; the name is all the app needs now.
                reply.post(Inbound::SaveTarget(FileRef::Download { download: download_name(&suggested) }));
            }
            HostJob::ReadClipboardText => read_clipboard_text(reply),
        }
    }
}

fn notice(reply: &Inbox, text: String) {
    reply.post(Inbound::Notice(text));
}

/// Opens the browser's file chooser. The input is removed again whatever happens: a chosen file, a cancel,
/// or a failure.
fn pick(kind: PickKind, accept: &'static str, multiple: bool, reply: Inbox) {
    if let Err(why) = open_chooser(kind, accept, multiple, &reply) {
        notice(&reply, format!("The file chooser could not open ({why}). Try again, or drop the file onto the canvas."));
    }
}

fn open_chooser(kind: PickKind, accept: &'static str, multiple: bool, reply: &Inbox) -> Result<(), String> {
    let document = document().map_err(|e| e.to_string())?;
    let body = document.body().ok_or("the page has no body")?;
    let input = document
        .create_element("input")
        .map_err(|e| js_error(&e))?
        .dyn_into::<HtmlInputElement>()
        .map_err(|_| "the browser made no file input")?;
    input.set_type("file");
    input.set_accept(accept);
    input.set_multiple(multiple);
    input.set_attribute("style", "display:none").map_err(|e| js_error(&e))?;
    body.append_child(&input).map_err(|e| js_error(&e))?;

    // Whichever of `change` and `cancel` comes first takes the cleanup; the other one finds it done.
    let slot: Rc<RefCell<Option<HtmlInputElement>>> = Rc::new(RefCell::new(Some(input.clone())));
    let on_change = {
        let slot = Rc::clone(&slot);
        let reply = reply.clone();
        Closure::once_into_js(move || {
            let Some(input) = slot.borrow_mut().take() else { return };
            let files: Vec<File> = input
                .files()
                .map(|list| (0..list.length()).filter_map(|i| list.item(i)).collect())
                .unwrap_or_default();
            input.remove();
            spawn_local(read_files(kind, files, reply));
        })
    };
    let on_cancel = {
        let slot = Rc::clone(&slot);
        Closure::once_into_js(move || {
            if let Some(input) = slot.borrow_mut().take() {
                input.remove();
            }
        })
    };
    input
        .add_event_listener_with_callback("change", on_change.unchecked_ref())
        .map_err(|e| js_error(&e))?;
    input
        .add_event_listener_with_callback("cancel", on_cancel.unchecked_ref())
        .map_err(|e| js_error(&e))?;
    input.click();
    Ok(())
}

/// Reads the chosen files and posts them. A file that cannot be read becomes a notice; the others still arrive.
pub(crate) async fn read_files(kind: PickKind, files: Vec<File>, reply: Inbox) {
    let mut picked = Vec::with_capacity(files.len());
    for file in files {
        let name = file.name();
        if file.size() > MAX_PICKED_BYTES {
            notice(&reply, format!("`{name}` is larger than 256 MB, which is more than the browser tab can hold."));
            continue;
        }
        match JsFuture::from(file.array_buffer()).await {
            Ok(buffer) => picked.push(PickedFile::Bytes { name, bytes: Uint8Array::new(&buffer).to_vec() }),
            Err(e) => notice(&reply, format!("`{name}` could not be read ({}).", js_error(&e))),
        }
    }
    if !picked.is_empty() {
        reply.post(Inbound::Picked { kind, files: picked });
    }
}

fn read_clipboard_text(reply: Inbox) {
    let Some(window) = web_sys::window() else { return };
    let promise = window.navigator().clipboard().read_text();
    spawn_local(async move {
        match JsFuture::from(promise).await {
            Ok(text) => reply.post(Inbound::ClipboardText(text.as_string().unwrap_or_default())),
            Err(_) => notice(
                &reply,
                "The browser did not let the page read the clipboard. Allow clipboard access for this site, or paste with Ctrl+V."
                    .to_owned(),
            ),
        }
    });
}
