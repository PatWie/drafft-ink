//! Clipboard for the web host: the DOM `paste`, `copy` and `cut` events, and the async Clipboard API
//! for writing and for an explicit read.
//!
//! The `ClipboardItem` and `Clipboard` types in web-sys are behind `web_sys_unstable_apis`, which the
//! workspace does not set, so the async API is called through `js_sys::Reflect`.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{ClipboardEvent, DataTransfer, EventTarget};

use super::dom::{self, ListenOptions, Listener};
use crate::events::{ClipboardContent, ImageBytes, ShellEvent};
use crate::host::HostSink;

/// The custom clipboard format for app payloads. The `web ` prefix is how the async Clipboard API names
/// custom formats (Chromium; other engines reject it and the host falls back to text).
pub(super) const CUSTOM_FORMAT: &str = "web application/x-drafftink";

pub(super) struct Clipboard {
    shared: Rc<Shared>,
    _listeners: Vec<Listener>,
}

struct Shared {
    sink: HostSink,
    textarea: web_sys::HtmlTextAreaElement,
    /// The `copy` or `cut` event being dispatched right now. Safari lets a page write the clipboard only
    /// inside such an event, so a `write_clipboard` call made while the sink delivers the request uses
    /// it. Any later call uses the async API.
    current: RefCell<Option<ClipboardEvent>>,
}

impl Clipboard {
    pub(super) fn install(sink: HostSink, textarea: web_sys::HtmlTextAreaElement) -> Result<Self, String> {
        let document = dom::document()?;
        let target: &EventTarget = document.as_ref();
        let shared = Rc::new(Shared { sink, textarea, current: RefCell::new(None) });
        let opts = ListenOptions::default();
        let mut listeners = Vec::new();

        let s = Rc::clone(&shared);
        listeners.push(Listener::new(target, "paste", opts, move |e: ClipboardEvent| s.on_paste(&e))?);
        let s = Rc::clone(&shared);
        listeners.push(Listener::new(target, "copy", opts, move |e: ClipboardEvent| s.on_copy(e, false))?);
        let s = Rc::clone(&shared);
        listeners.push(Listener::new(target, "cut", opts, move |e: ClipboardEvent| s.on_copy(e, true))?);
        Ok(Self { shared, _listeners: listeners })
    }

    /// Put content on the system clipboard.
    pub(super) fn write(&self, content: ClipboardContent) {
        if content.is_empty() {
            log::debug!("write_clipboard: nothing to write");
            return;
        }
        // Inside a copy/cut event, plain text goes through the event, which works in every browser.
        let event = self.shared.current.borrow().clone();
        if let (Some(event), Some(text), true) =
            (event, content.text.as_deref(), content.images.is_empty() && content.custom.is_none())
            && let Some(data) = event.clipboard_data()
            && data.set_data("text/plain", text).is_ok()
        {
            event.prevent_default();
            return;
        }
        spawn_local(async move {
            if let Err(e) = write_async(content).await {
                log::warn!("clipboard write failed: {e}");
            }
        });
    }

    /// Read the system clipboard with the async API (needs a user gesture and, in most browsers, a
    /// permission) and send the result as `ShellEvent::Paste`.
    pub(super) fn read(&self) {
        let sink = self.shared.sink.clone();
        spawn_local(async move {
            match read_async().await {
                Ok(content) if content.is_empty() => log::debug!("read_clipboard: clipboard is empty"),
                Ok(content) => sink.app(ShellEvent::Paste(content)),
                Err(e) => log::warn!("clipboard read failed: {e}"),
            }
        });
    }
}

impl Shared {
    /// A form field of the page around the canvas (not our textarea) owns its own clipboard events.
    fn is_foreign_field(&self, target: Option<EventTarget>) -> bool {
        let Some(element) = target.and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return false };
        let ours: &web_sys::Element = self.textarea.as_ref();
        if element == *ours {
            return false;
        }
        let tag = element.tag_name();
        if tag.eq_ignore_ascii_case("input")
            || tag.eq_ignore_ascii_case("textarea")
            || tag.eq_ignore_ascii_case("select")
        {
            return true;
        }
        element.dyn_ref::<web_sys::HtmlElement>().is_some_and(web_sys::HtmlElement::is_content_editable)
    }

    fn on_paste(&self, event: &ClipboardEvent) {
        if self.is_foreign_field(event.target()) {
            return;
        }
        let Some(data) = event.clipboard_data() else { return };
        // The text arrives through this event. Do not let the textarea insert it as well.
        event.prevent_default();

        let text = data.get_data("text/plain").ok().filter(|t| !t.is_empty()).map(|t| t.replace("\r\n", "\n"));
        let custom = has_type(&data, CUSTOM_FORMAT)
            .then(|| data.get_data(CUSTOM_FORMAT).ok())
            .flatten()
            .filter(|c| !c.is_empty())
            .map(String::into_bytes);
        let mut files = Vec::new();
        if let Some(list) = data.files() {
            for i in 0..list.length() {
                if let Some(file) = list.item(i)
                    && file.type_().starts_with("image/")
                {
                    files.push(file);
                }
            }
        }
        let sink = self.sink.clone();
        if files.is_empty() {
            let content = ClipboardContent { text, images: Vec::new(), custom };
            if !content.is_empty() {
                sink.app(ShellEvent::Paste(content));
            }
            return;
        }
        // File objects stay valid after the event. Reading them is asynchronous.
        spawn_local(async move {
            let mut images = Vec::new();
            for file in files {
                match dom::read_blob_bytes(file.as_ref()).await {
                    Ok(bytes) => images.push(ImageBytes { mime: file.type_(), bytes }),
                    Err(e) => log::warn!("could not read pasted image {}: {e}", file.name()),
                }
            }
            let content = ClipboardContent { text, images, custom };
            if !content.is_empty() {
                sink.app(ShellEvent::Paste(content));
            }
        });
    }

    fn on_copy(&self, event: ClipboardEvent, cut: bool) {
        if self.is_foreign_field(event.target()) {
            return;
        }
        *self.current.borrow_mut() = Some(event);
        self.sink.app(ShellEvent::CopyRequested { cut });
        self.current.borrow_mut().take();
    }
}

fn has_type(data: &DataTransfer, wanted: &str) -> bool {
    data.types().iter().any(|t| t.as_string().is_some_and(|t| t == wanted))
}

fn record_set(record: &js_sys::Object, key: &str, value: &web_sys::Blob) -> Result<(), String> {
    js_sys::Reflect::set(record, &JsValue::from_str(key), value.as_ref()).map(|_| ()).map_err(|e| dom::js_error(&e))
}

/// `navigator.clipboard.write([new ClipboardItem(record)])`.
async fn write_item(clipboard: &JsValue, record: &js_sys::Object) -> Result<(), String> {
    let global = js_sys::global();
    let ctor: js_sys::Function = js_sys::Reflect::get(&global, &JsValue::from_str("ClipboardItem"))
        .map_err(|e| dom::js_error(&e))?
        .dyn_into()
        .map_err(|_| "ClipboardItem is not supported".to_owned())?;
    // The constructor throws for formats the browser does not know.
    let item = js_sys::Reflect::construct(&ctor, &js_sys::Array::of1(record)).map_err(|e| dom::js_error(&e))?;
    let promise =
        dom::call_method(clipboard, "write", &[js_sys::Array::of1(&item).into()]).map_err(|e| dom::js_error(&e))?;
    dom::await_value(promise).await.map(|_| ()).map_err(|e| dom::js_error(&e))
}

async fn write_text(clipboard: &JsValue, text: &str) -> Result<(), String> {
    let promise =
        dom::call_method(clipboard, "writeText", &[JsValue::from_str(text)]).map_err(|e| dom::js_error(&e))?;
    dom::await_value(promise).await.map(|_| ()).map_err(|e| dom::js_error(&e))
}

async fn write_async(content: ClipboardContent) -> Result<(), String> {
    let Some(clipboard) = dom::navigator_clipboard() else {
        return Err("navigator.clipboard is not available (insecure context or old browser)".to_owned());
    };
    let png = content.images.iter().find(|i| i.mime == "image/png");
    if content.images.len() > usize::from(png.is_some()) {
        log::warn!("the clipboard accepts PNG images only; other image formats are not written");
    }
    let text = content.text.as_deref().filter(|t| !t.is_empty());

    let build = |with_custom: bool| -> Result<js_sys::Object, String> {
        let record = js_sys::Object::new();
        if let Some(text) = text {
            record_set(&record, "text/plain", &dom::blob_from_bytes(text.as_bytes(), "text/plain")?)?;
        }
        if let Some(png) = png {
            record_set(&record, "image/png", &dom::blob_from_bytes(&png.bytes, "image/png")?)?;
        }
        if let (true, Some(custom)) = (with_custom, content.custom.as_deref()) {
            record_set(&record, CUSTOM_FORMAT, &dom::blob_from_bytes(custom, CUSTOM_FORMAT)?)?;
        }
        Ok(record)
    };

    // Most capable first; each step drops what the browser may refuse.
    let rich = png.is_some() || content.custom.is_some();
    let mut last_error = None;
    if rich {
        for with_custom in [true, false] {
            if !with_custom && content.custom.is_none() {
                continue;
            }
            match write_item(&clipboard, &build(with_custom)?).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::debug!("clipboard.write (custom format: {with_custom}) failed: {e}");
                    last_error = Some(e);
                }
            }
        }
    }
    match text {
        Some(text) => {
            if let Some(e) = last_error {
                log::warn!("falling back to text only: {e}");
            }
            write_text(&clipboard, text).await
        }
        None => Err(last_error.unwrap_or_else(|| "nothing to write".to_owned())),
    }
}

async fn read_async() -> Result<ClipboardContent, String> {
    let Some(clipboard) = dom::navigator_clipboard() else {
        return Err("navigator.clipboard is not available (insecure context or old browser)".to_owned());
    };
    let mut content = ClipboardContent::default();
    let mut read_error = None;
    if dom::has_property(&clipboard, "read")
        && let Err(e) = read_items(&clipboard, &mut content).await
    {
        read_error = Some(e);
    }
    if content.is_empty() {
        // `read` is missing (older Firefox) or refused: text only.
        if let Some(e) = read_error {
            log::debug!("clipboard.read failed, trying readText: {e}");
        }
        let promise = dom::call_method(&clipboard, "readText", &[]).map_err(|e| dom::js_error(&e))?;
        let text = dom::await_value(promise).await.map_err(|e| dom::js_error(&e))?;
        content.text = text.as_string().map(|t| t.replace("\r\n", "\n")).filter(|t| !t.is_empty());
    }
    Ok(content)
}

async fn read_items(clipboard: &JsValue, content: &mut ClipboardContent) -> Result<(), String> {
    let promise = dom::call_method(clipboard, "read", &[]).map_err(|e| dom::js_error(&e))?;
    let items: js_sys::Array = dom::await_value(promise)
        .await
        .map_err(|e| dom::js_error(&e))?
        .dyn_into()
        .map_err(|_| "clipboard.read() did not give an array".to_owned())?;
    for item in items.iter() {
        let types: js_sys::Array = js_sys::Reflect::get(&item, &JsValue::from_str("types"))
            .map_err(|e| dom::js_error(&e))?
            .dyn_into()
            .map_err(|_| "ClipboardItem.types is not an array".to_owned())?;
        for kind in types.iter().filter_map(|t| t.as_string()) {
            let wanted = kind == "text/plain" || kind == CUSTOM_FORMAT || kind.starts_with("image/");
            if !wanted || (kind == "text/plain" && content.text.is_some()) {
                continue;
            }
            let promise =
                dom::call_method(&item, "getType", &[JsValue::from_str(&kind)]).map_err(|e| dom::js_error(&e))?;
            let blob = dom::await_value(promise).await.map_err(|e| dom::js_error(&e))?;
            if kind == "text/plain" {
                let text = dom::read_blob_text(&blob).await?;
                content.text = Some(text.replace("\r\n", "\n")).filter(|t| !t.is_empty());
            } else if kind == CUSTOM_FORMAT {
                content.custom = Some(dom::read_blob_bytes(&blob).await?);
            } else {
                match dom::read_blob_bytes(&blob).await {
                    Ok(bytes) => content.images.push(ImageBytes { mime: kind, bytes }),
                    Err(e) => log::warn!("could not read clipboard image {kind}: {e}"),
                }
            }
        }
    }
    Ok(())
}
