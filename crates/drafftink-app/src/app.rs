//! Core application state and lifecycle.

use drafftink_core::canvas::Canvas;
use drafftink_core::collaboration::CollaborationManager;
use drafftink_core::input::InputState;
use drafftink_core::shapes::Shape;
use drafftink_core::sync::{AwarenessState, ConnectionState, SyncEvent};
use drafftink_core::tools::ToolKind;
#[cfg(not(target_arch = "wasm32"))]
use drafftink_render::PngRenderResult;
use drafftink_render::{
    AngleSnapInfo, GridStyle, RenderContext, Renderer, TextEditResult, TextEditState, TextKey,
    TextModifiers, VelloRenderer,
};
use kurbo::{Point, Size, Vec2};
use peniko::Color;
use std::sync::Arc;
use vello::util::RenderSurface;
use vello::wgpu::PresentMode;
use vello::{AaConfig, RenderParams, RendererOptions, Scene};
use winit::application::ApplicationHandler;
#[cfg(not(target_arch = "wasm32"))]
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::event_handler::EventHandler;
use crate::ui::{SelectedShapeProps, UiAction, UiState, render_ui};

#[cfg(feature = "native")]
pub mod file_ops {
    use drafftink_core::canvas::CanvasDocument;
    use std::sync::Mutex;

    // Channel for receiving async file operation results
    static PENDING_DOCUMENT: Mutex<Option<CanvasDocument>> = Mutex::new(None);

    /// Save document to a JSON file using native file dialog (async, non-blocking).
    pub fn save_document(document: &CanvasDocument, name: &str) {
        let doc = document.clone();
        let default_name = format!("{}.json", name);
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new()
                .set_title("Save Document")
                .set_file_name(&default_name)
                .add_filter("DrafftInk Document", &["json"]);

            if let Some(path) = dialog.save_file() {
                match doc.to_json() {
                    Ok(json) => {
                        if let Err(e) = std::fs::write(&path, &json) {
                            log::error!("Failed to write file: {}", e);
                        } else {
                            log::info!("Saved document to: {:?}", path);
                        }
                    }
                    Err(e) => log::error!("Failed to serialize document: {}", e),
                }
            }
        });
    }

    /// Load document from a JSON file using native file dialog (async, non-blocking).
    /// Use `take_pending_document()` to retrieve the result.
    pub fn load_document() {
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new()
                .set_title("Open Document")
                .add_filter("DrafftInk Document", &["json"])
                .add_filter("Excalidraw", &["excalidraw"]);

            if let Some(path) = dialog.pick_file() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let is_excalidraw = path
                            .extension()
                            .map(|e| e.to_string_lossy().to_lowercase() == "excalidraw")
                            .unwrap_or(false);

                        let result = if is_excalidraw {
                            CanvasDocument::from_excalidraw(&content).map_err(|e| e.to_string())
                        } else {
                            CanvasDocument::from_json(&content).map_err(|e| e.to_string())
                        };

                        match result {
                            Ok(doc) => {
                                log::info!("Loaded document from: {:?}", path);
                                if let Ok(mut pending) = PENDING_DOCUMENT.lock() {
                                    *pending = Some(doc);
                                }
                            }
                            Err(e) => log::error!("Failed to parse document: {}", e),
                        }
                    }
                    Err(e) => log::error!("Failed to read file: {}", e),
                }
            }
        });
    }

    /// Take pending document from async load operation.
    pub fn take_pending_document() -> Option<CanvasDocument> {
        PENDING_DOCUMENT.lock().ok().and_then(|mut p| p.take())
    }

    /// Load document by name (for native, just calls load_document).
    pub fn load_document_by_name(_name: &str) {
        load_document()
    }

    /// Export PNG to file using native file dialog (async, non-blocking).
    pub fn export_png(png_data: &[u8], name: &str) {
        let data = png_data.to_vec();
        let default_name = format!("{}.png", name);
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new()
                .set_title("Export PNG")
                .set_file_name(&default_name)
                .add_filter("PNG Image", &["png"]);

            if let Some(path) = dialog.save_file() {
                if let Err(e) = std::fs::write(&path, &data) {
                    log::error!("Failed to write PNG: {}", e);
                } else {
                    log::info!("Exported PNG to: {:?}", path);
                }
            }
        });
    }

    /// Copy PNG to clipboard.
    pub fn copy_png_to_clipboard(png_data: &[u8], width: u32, height: u32) {
        // arboard expects RGBA pixel data, not PNG encoded data
        // We need to decode if it's PNG, or pass raw RGBA
        let image_data = arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: std::borrow::Cow::Borrowed(png_data),
        };

        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                if let Err(e) = clipboard.set_image(image_data) {
                    log::error!("Failed to copy PNG to clipboard: {}", e);
                } else {
                    log::info!("PNG copied to clipboard ({}x{})", width, height);
                }
            }
            Err(e) => log::error!("Failed to access clipboard: {}", e),
        }
    }

    /// Paste image from clipboard as a new Image shape.
    /// Returns None if no image is in the clipboard.
    pub fn paste_image_from_clipboard(
        canvas: &drafftink_core::canvas::Canvas,
    ) -> Option<drafftink_core::shapes::Shape> {
        use drafftink_core::shapes::{Image, ImageFormat, Shape};
        use kurbo::Point;

        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                match clipboard.get_image() {
                    Ok(img_data) => {
                        // img_data contains RGBA pixel data
                        let width = img_data.width as u32;
                        let height = img_data.height as u32;

                        // Encode as PNG for storage (more compact than raw RGBA)
                        let mut png_data = Vec::new();
                        {
                            let mut encoder = ::png::Encoder::new(&mut png_data, width, height);
                            encoder.set_color(::png::ColorType::Rgba);
                            encoder.set_depth(::png::BitDepth::Eight);
                            if let Ok(mut writer) = encoder.write_header() {
                                if writer.write_image_data(&img_data.bytes).is_err() {
                                    log::error!("Failed to encode clipboard image as PNG");
                                    return None;
                                }
                            }
                        }

                        // Position at viewport center
                        let viewport_center = canvas.camera.screen_to_world(Point::new(
                            canvas.viewport_size.width / 2.0,
                            canvas.viewport_size.height / 2.0,
                        ));
                        let position = Point::new(
                            viewport_center.x - width as f64 / 2.0,
                            viewport_center.y - height as f64 / 2.0,
                        );

                        // Create image shape, scaled to fit if too large
                        let max_size = 800.0;
                        let mut image =
                            Image::new(position, &png_data, width, height, ImageFormat::Png);
                        if width as f64 > max_size || height as f64 > max_size {
                            image = image.fit_within(max_size, max_size);
                            // Re-center after fitting
                            image.position = Point::new(
                                viewport_center.x - image.width / 2.0,
                                viewport_center.y - image.height / 2.0,
                            );
                        }

                        log::info!("Pasted image from clipboard: {}x{}", width, height);
                        Some(Shape::Image(image))
                    }
                    Err(_) => {
                        // No image in clipboard
                        None
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to access clipboard: {}", e);
                None
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub mod file_ops {
    use drafftink_core::canvas::CanvasDocument;
    use drafftink_core::storage::{IndexedDbStorage, Storage};
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::prelude::*;

    thread_local! {
        static STORAGE: Rc<IndexedDbStorage> = Rc::new(IndexedDbStorage::new());
        static PENDING_DOCUMENT: RefCell<Option<CanvasDocument>> = const { RefCell::new(None) };
        static PENDING_DOCUMENT_LIST: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
        static PENDING_CLIPBOARD_TEXT: RefCell<Option<String>> = const { RefCell::new(None) };
        static PENDING_MATH_CLIPBOARD: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    /// Request clipboard text read (async). Result will be available via take_pending_clipboard_text().
    pub fn request_clipboard_text() {
        wasm_bindgen_futures::spawn_local(async {
            if let Some(text) = read_clipboard_text_async().await {
                PENDING_CLIPBOARD_TEXT.with(|cell| {
                    *cell.borrow_mut() = Some(text);
                });
            }
        });
    }

    /// Request clipboard text for math editor (async).
    pub fn request_clipboard_text_for_math() {
        wasm_bindgen_futures::spawn_local(async {
            if let Some(text) = read_clipboard_text_async().await {
                PENDING_MATH_CLIPBOARD.with(|cell| {
                    *cell.borrow_mut() = Some(text);
                });
            }
        });
    }

    /// Take pending math clipboard text if available.
    pub fn take_pending_math_clipboard() -> Option<String> {
        PENDING_MATH_CLIPBOARD.with(|cell| cell.borrow_mut().take())
    }

    /// Take pending clipboard text if available.
    pub fn take_pending_clipboard_text() -> Option<String> {
        PENDING_CLIPBOARD_TEXT.with(|cell| cell.borrow_mut().take())
    }

    async fn read_clipboard_text_async() -> Option<String> {
        let window = web_sys::window()?;
        let clipboard = window.navigator().clipboard();
        let promise = clipboard.read_text();
        wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .ok()?
            .as_string()
    }

    /// Copy text to clipboard (fire and forget).
    pub fn copy_text_to_clipboard(text: &str) {
        let text = text.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(window) = web_sys::window() {
                let clipboard = window.navigator().clipboard();
                let promise = clipboard.write_text(&text);
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            }
        });
    }

    /// Save document to IndexedDB (real persistence).
    pub fn save_document(document: &CanvasDocument, name: &str) {
        let doc_id = name.to_string();
        let doc_clone = document.clone();

        STORAGE.with(|storage| {
            let storage = storage.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let future = storage.save(&doc_id, &doc_clone);
                match future.await {
                    Ok(()) => {
                        log::info!("Document saved to IndexedDB: {}", doc_id);
                        // Also save as "last document" for auto-restore
                        let last_future = storage.save("__last__", &doc_clone);
                        if let Err(e) = last_future.await {
                            log::warn!("Failed to save as last document: {:?}", e);
                        }
                    }
                    Err(e) => log::error!("Failed to save document: {:?}", e),
                }
            });
        });
    }

    /// Auto-save document to IndexedDB (silent, no logging).
    pub fn autosave_document(document: &CanvasDocument) {
        let doc_clone = document.clone();

        STORAGE.with(|storage| {
            let storage = storage.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let future = storage.save("__last__", &doc_clone);
                if let Err(e) = future.await {
                    log::warn!("Autosave failed: {:?}", e);
                }
            });
        });
    }

    /// Load document from IndexedDB - triggers async load.
    /// Use `take_pending_document()` to retrieve the loaded document.
    pub fn load_document_async() {
        STORAGE.with(|storage| {
            let storage = storage.clone();
            wasm_bindgen_futures::spawn_local(async move {
                // Try to load the last saved document
                let future = storage.load("__last__");
                match future.await {
                    Ok(doc) => {
                        log::info!("Document loaded from IndexedDB: {}", doc.name);
                        set_pending_document(doc);
                    }
                    Err(_) => {
                        // No saved document - load intro as default
                        static INTRO_JSON: &str = include_str!("../assets/intro.json");
                        if let Ok(doc) =
                            drafftink_core::canvas::CanvasDocument::from_json(INTRO_JSON)
                        {
                            log::info!("Loading intro document as default");
                            set_pending_document(doc);
                        }
                    }
                }
            });
        });
    }

    /// Try to load the last document on startup.
    pub fn try_load_last_document() {
        load_document_async();
    }

    /// Load document by name from IndexedDB - triggers async load.
    pub fn load_document_by_name_async(name: &str) {
        let doc_name = name.to_string();
        STORAGE.with(|storage| {
            let storage = storage.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let future = storage.load(&doc_name);
                match future.await {
                    Ok(doc) => {
                        log::info!("Document loaded from IndexedDB: {}", doc.name);
                        set_pending_document(doc);
                    }
                    Err(e) => {
                        log::error!("Failed to load document '{}': {:?}", doc_name, e);
                    }
                }
            });
        });
    }

    /// List all documents in IndexedDB - triggers async list.
    pub fn list_documents_async() {
        STORAGE.with(|storage| {
            let storage = storage.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let future = storage.list();
                match future.await {
                    Ok(docs) => {
                        let filtered: Vec<String> =
                            docs.into_iter().filter(|id| id != "__last__").collect();
                        PENDING_DOCUMENT_LIST.with(|list| {
                            *list.borrow_mut() = Some(filtered);
                        });
                    }
                    Err(e) => {
                        log::error!("Failed to list documents: {:?}", e);
                    }
                }
            });
        });
    }

    /// Take the pending document list.
    pub fn take_pending_document_list() -> Option<Vec<String>> {
        PENDING_DOCUMENT_LIST.with(|list| list.borrow_mut().take())
    }

    /// Download document as JSON file (export).
    pub fn download_document(document: &CanvasDocument, name: &str) {
        match document.to_json() {
            Ok(json) => download_file(&format!("{}.json", name), &json, "application/json"),
            Err(e) => log::error!("Failed to serialize document: {}", e),
        }
    }

    /// Upload document from file input - triggers async file picker.
    /// Use `take_pending_document()` to retrieve the loaded document.
    pub fn upload_document_async() {
        trigger_file_input_async();
    }

    /// Take the pending document loaded from async operations.
    /// Returns Some if a document was loaded, None otherwise.
    pub fn take_pending_document() -> Option<CanvasDocument> {
        PENDING_DOCUMENT.with(|cell| cell.borrow_mut().take())
    }

    fn set_pending_document(doc: CanvasDocument) {
        PENDING_DOCUMENT.with(|cell| {
            *cell.borrow_mut() = Some(doc);
        });
    }

    /// Export PNG (triggers browser download).
    pub fn export_png(png_data: &[u8], name: &str) {
        download_binary_file(&format!("{}.png", name), png_data, "image/png");
    }

    /// Copy PNG to clipboard using the async Clipboard API.
    /// This spawns an async task since clipboard.write() returns a Promise.
    pub fn copy_png_to_clipboard(png_data: Vec<u8>) {
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = copy_png_to_clipboard_async(png_data).await {
                log::error!("Failed to copy to clipboard: {:?}", e);
                // Could fall back to download here if needed
            }
        });
    }

    async fn copy_png_to_clipboard_async(png_data: Vec<u8>) -> Result<(), JsValue> {
        let window = web_sys::window().ok_or("No window")?;
        let navigator = window.navigator();
        let clipboard = navigator.clipboard();

        // Create a Blob from the PNG data
        let uint8_array = js_sys::Uint8Array::from(png_data.as_slice());
        let blob_parts = js_sys::Array::new();
        blob_parts.push(&uint8_array);

        let options = web_sys::BlobPropertyBag::new();
        options.set_type("image/png");

        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&blob_parts, &options)?;

        // Create ClipboardItem with the PNG blob
        let item_options = js_sys::Object::new();
        js_sys::Reflect::set(&item_options, &"image/png".into(), &blob)?;

        let clipboard_item = web_sys::ClipboardItem::new_with_record_from_str_to_blob_promise(
            &item_options.unchecked_into(),
        )?;

        let items = js_sys::Array::new();
        items.push(&clipboard_item);

        // Write to clipboard (returns a Promise)
        let promise = clipboard.write(&items);
        wasm_bindgen_futures::JsFuture::from(promise).await?;

        log::info!("PNG copied to clipboard ({} bytes)", png_data.len());
        Ok(())
    }

    fn download_file(filename: &str, content: &str, mime_type: &str) {
        let window = web_sys::window().expect("No window");
        let document = window.document().expect("No document");

        // Create blob
        let blob_parts = js_sys::Array::new();
        blob_parts.push(&JsValue::from_str(content));

        let options = web_sys::BlobPropertyBag::new();
        options.set_type(mime_type);

        let blob = web_sys::Blob::new_with_str_sequence_and_options(&blob_parts, &options)
            .expect("Failed to create blob");

        // Create download URL
        let url = web_sys::Url::create_object_url_with_blob(&blob).expect("Failed to create URL");

        // Create and click download link
        let a = document
            .create_element("a")
            .expect("Failed to create element")
            .dyn_into::<web_sys::HtmlAnchorElement>()
            .expect("Failed to cast to anchor");

        a.set_href(&url);
        a.set_download(filename);
        a.click();

        // Clean up
        web_sys::Url::revoke_object_url(&url).ok();
    }

    fn download_binary_file(filename: &str, data: &[u8], mime_type: &str) {
        let window = web_sys::window().expect("No window");
        let document = window.document().expect("No document");

        // Create Uint8Array from data
        let uint8_array = js_sys::Uint8Array::from(data);
        let blob_parts = js_sys::Array::new();
        blob_parts.push(&uint8_array);

        let options = web_sys::BlobPropertyBag::new();
        options.set_type(mime_type);

        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&blob_parts, &options)
            .expect("Failed to create blob");

        // Create download URL
        let url = web_sys::Url::create_object_url_with_blob(&blob).expect("Failed to create URL");

        // Create and click download link
        let a = document
            .create_element("a")
            .expect("Failed to create element")
            .dyn_into::<web_sys::HtmlAnchorElement>()
            .expect("Failed to cast to anchor");

        a.set_href(&url);
        a.set_download(filename);
        a.click();

        // Clean up
        web_sys::Url::revoke_object_url(&url).ok();
    }

    fn trigger_file_input_async() {
        wasm_bindgen_futures::spawn_local(async {
            if let Err(e) = trigger_file_input_impl().await {
                log::error!("Failed to load file: {:?}", e);
            }
        });
    }

    async fn trigger_file_input_impl() -> Result<(), JsValue> {
        let window = web_sys::window().ok_or("No window")?;
        let document = window.document().ok_or("No document")?;

        let input: web_sys::HtmlInputElement = document.create_element("input")?.dyn_into()?;
        input.set_type("file");
        input.set_accept(".json,.excalidraw");
        input.style().set_property("display", "none").ok();

        document.body().ok_or("No body")?.append_child(&input)?;

        // Wait for file selection via Promise
        let file = wait_for_file_selection(&input).await;
        input.remove();

        let file = file?;
        let filename = file.name();
        let is_excalidraw = filename.to_lowercase().ends_with(".excalidraw");

        // Read file content using File.text() which returns a Promise
        let text: String = wasm_bindgen_futures::JsFuture::from(file.text())
            .await?
            .as_string()
            .ok_or("Failed to read file as text")?;

        let doc_result = if is_excalidraw {
            CanvasDocument::from_excalidraw(&text).map_err(|e| e.to_string())
        } else {
            CanvasDocument::from_json(&text).map_err(|e| e.to_string())
        };

        match doc_result {
            Ok(doc) => {
                log::info!("Document loaded: {}", doc.name);
                set_pending_document(doc);
                Ok(())
            }
            Err(e) => Err(JsValue::from_str(&format!("Failed to parse document: {}", e))),
        }
    }

    async fn wait_for_file_selection(
        input: &web_sys::HtmlInputElement,
    ) -> Result<web_sys::File, JsValue> {
        use wasm_bindgen::closure::Closure;

        let input_clone = input.clone();
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let input_inner = input_clone.clone();
            let onchange = Closure::once(Box::new(move |_: web_sys::Event| {
                let file = input_inner.files().and_then(|f| f.get(0));
                resolve.call1(&JsValue::NULL, &file.into()).ok();
            }) as Box<dyn FnOnce(_)>);
            input_clone.set_onchange(Some(onchange.as_ref().unchecked_ref()));
            onchange.forget();
        });

        input.click();

        let result = wasm_bindgen_futures::JsFuture::from(promise).await?;
        result.dyn_into::<web_sys::File>()
    }

    // Thread-local storage for pending pasted image
    thread_local! {
        static PENDING_IMAGE: RefCell<Option<drafftink_core::shapes::Shape>> = const { RefCell::new(None) };
    }

    /// Try to paste an image from the clipboard (async).
    /// The result will be available via `take_pending_image()`.
    pub fn paste_image_from_clipboard_async(
        viewport_width: f64,
        viewport_height: f64,
        camera_offset_x: f64,
        camera_offset_y: f64,
        camera_zoom: f64,
    ) {
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = paste_image_async(
                viewport_width,
                viewport_height,
                camera_offset_x,
                camera_offset_y,
                camera_zoom,
            )
            .await
            {
                log::debug!("No image in clipboard or paste failed: {:?}", e);
            }
        });
    }

    async fn paste_image_async(
        viewport_width: f64,
        viewport_height: f64,
        camera_offset_x: f64,
        camera_offset_y: f64,
        camera_zoom: f64,
    ) -> Result<(), JsValue> {
        use drafftink_core::shapes::{Image, ImageFormat, Shape};
        use kurbo::Point;

        let window = web_sys::window().ok_or("No window")?;
        let navigator = window.navigator();
        let clipboard = navigator.clipboard();

        // Read clipboard items
        let promise = clipboard.read();
        let result = wasm_bindgen_futures::JsFuture::from(promise).await?;
        let items: js_sys::Array = result.dyn_into()?;

        for i in 0..items.length() {
            let item: web_sys::ClipboardItem = items.get(i).dyn_into()?;
            let types = item.types();

            // Check for image types
            for j in 0..types.length() {
                if let Some(mime_type) = types.get(j).as_string() {
                    if mime_type.starts_with("image/") {
                        // Get the blob for this type
                        let blob_promise = item.get_type(&mime_type);
                        let blob: web_sys::Blob =
                            wasm_bindgen_futures::JsFuture::from(blob_promise)
                                .await?
                                .dyn_into()?;

                        // Read blob as array buffer
                        let array_buffer_promise = blob.array_buffer();
                        let array_buffer =
                            wasm_bindgen_futures::JsFuture::from(array_buffer_promise).await?;
                        let uint8_array = js_sys::Uint8Array::new(&array_buffer);
                        let data = uint8_array.to_vec();

                        // Determine format from mime type
                        let format = match mime_type.as_str() {
                            "image/png" => ImageFormat::Png,
                            "image/jpeg" => ImageFormat::Jpeg,
                            "image/webp" => ImageFormat::WebP,
                            _ => ImageFormat::Png, // Default to PNG
                        };

                        // Decode image to get dimensions
                        // We need to use the browser's Image API or decode in Rust
                        // For simplicity, we'll try to decode using the image crate via a simple approach
                        // But image crate is not available in WASM by default, so we'll use a different method

                        // Create an HTML Image element to decode
                        let blob_url = web_sys::Url::create_object_url_with_blob(&blob)?;

                        let (width, height) = decode_image_dimensions(&blob_url).await?;

                        web_sys::Url::revoke_object_url(&blob_url)?;

                        // Calculate viewport center in world coordinates
                        let viewport_center_x =
                            (viewport_width / 2.0 - camera_offset_x) / camera_zoom;
                        let viewport_center_y =
                            (viewport_height / 2.0 - camera_offset_y) / camera_zoom;

                        let position = Point::new(
                            viewport_center_x - width as f64 / 2.0,
                            viewport_center_y - height as f64 / 2.0,
                        );

                        // Create image shape, scaled to fit if too large
                        let max_size = 800.0;
                        let mut image = Image::new(position, &data, width, height, format);
                        if width as f64 > max_size || height as f64 > max_size {
                            image = image.fit_within(max_size, max_size);
                            image.position = Point::new(
                                viewport_center_x - image.width / 2.0,
                                viewport_center_y - image.height / 2.0,
                            );
                        }

                        log::info!("Pasted image from clipboard: {}x{}", width, height);
                        set_pending_image(Shape::Image(image));
                        return Ok(());
                    }
                }
            }
        }

        Err("No image found in clipboard".into())
    }

    async fn decode_image_dimensions(blob_url: &str) -> Result<(u32, u32), JsValue> {
        let window = web_sys::window().ok_or("No window")?;
        let document = window.document().ok_or("No document")?;
        let img: web_sys::HtmlImageElement = document.create_element("img")?.dyn_into()?;

        let promise = wait_for_image_load(&img);
        img.set_src(blob_url);
        promise.await?;

        Ok((img.natural_width(), img.natural_height()))
    }

    async fn wait_for_image_load(img: &web_sys::HtmlImageElement) -> Result<(), JsValue> {
        use wasm_bindgen::closure::Closure;

        let img_clone = img.clone();
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let onload = Closure::once(Box::new(move |_: web_sys::Event| {
                resolve.call0(&JsValue::NULL).ok();
            }) as Box<dyn FnOnce(_)>);
            let onerror = Closure::once(Box::new(move |_: web_sys::Event| {
                reject.call1(&JsValue::NULL, &"Failed to load image".into()).ok();
            }) as Box<dyn FnOnce(_)>);
            img_clone.set_onload(Some(onload.as_ref().unchecked_ref()));
            img_clone.set_onerror(Some(onerror.as_ref().unchecked_ref()));
            onload.forget();
            onerror.forget();
        });

        wasm_bindgen_futures::JsFuture::from(promise).await?;
        Ok(())
    }

    /// Yield to the browser event loop via requestAnimationFrame.
    /// This allows WebGPU callbacks and other browser tasks to run.
    pub async fn yield_to_browser() {
        use wasm_bindgen::closure::Closure;

        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let window = web_sys::window().expect("no window");
            let closure = Closure::once_into_js(move || {
                let _ = resolve.call0(&JsValue::NULL);
            });
            let _ = window.request_animation_frame(closure.unchecked_ref());
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    fn set_pending_image(shape: drafftink_core::shapes::Shape) {
        PENDING_IMAGE.with(|cell| {
            *cell.borrow_mut() = Some(shape);
        });
    }

    /// Take the pending image from clipboard paste.
    pub fn take_pending_image() -> Option<drafftink_core::shapes::Shape> {
        PENDING_IMAGE.with(|cell| cell.borrow_mut().take())
    }

    // Thread-local storage for pending dropped images
    thread_local! {
        static PENDING_DROPPED_IMAGES: RefCell<Vec<drafftink_core::shapes::Shape>> = const { RefCell::new(Vec::new()) };
    }

    /// Setup drag-drop handlers on the canvas element.
    /// Call this once after the canvas is created.
    pub fn setup_drag_drop_handlers(
        viewport_width: f64,
        viewport_height: f64,
        camera_offset_x: f64,
        camera_offset_y: f64,
        camera_zoom: f64,
    ) {
        use wasm_bindgen::closure::Closure;

        let window = web_sys::window().expect("No window");
        let document = window.document().expect("No document");

        // Find the canvas element
        if let Some(canvas) = document.query_selector("canvas").ok().flatten() {
            // Prevent default drag behavior
            let ondragover = Closure::wrap(Box::new(move |event: web_sys::DragEvent| {
                event.prevent_default();
            }) as Box<dyn Fn(_)>);

            canvas
                .add_event_listener_with_callback("dragover", ondragover.as_ref().unchecked_ref())
                .ok();
            ondragover.forget();

            // Handle drop
            let vw = viewport_width;
            let vh = viewport_height;
            let cox = camera_offset_x;
            let coy = camera_offset_y;
            let cz = camera_zoom;

            let ondrop = Closure::wrap(Box::new(move |event: web_sys::DragEvent| {
                event.prevent_default();

                if let Some(data_transfer) = event.data_transfer() {
                    if let Some(files) = data_transfer.files() {
                        for i in 0..files.length() {
                            if let Some(file) = files.get(i) {
                                let file_name = file.name();
                                let file_type = file.type_();

                                // Check if it's an image
                                if file_type.starts_with("image/") {
                                    handle_dropped_file(file, vw, vh, cox, coy, cz);
                                } else if let Some(ext) = file_name.split('.').last() {
                                    if matches!(
                                        ext.to_lowercase().as_str(),
                                        "png" | "jpg" | "jpeg" | "webp"
                                    ) {
                                        handle_dropped_file(file, vw, vh, cox, coy, cz);
                                    }
                                }
                            }
                        }
                    }
                }
            }) as Box<dyn Fn(_)>);

            canvas
                .add_event_listener_with_callback("drop", ondrop.as_ref().unchecked_ref())
                .ok();
            ondrop.forget();
        }
    }

    fn handle_dropped_file(
        file: web_sys::File,
        viewport_width: f64,
        viewport_height: f64,
        camera_offset_x: f64,
        camera_offset_y: f64,
        camera_zoom: f64,
    ) {
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = process_dropped_file(
                file,
                viewport_width,
                viewport_height,
                camera_offset_x,
                camera_offset_y,
                camera_zoom,
            )
            .await
            {
                log::error!("Failed to process dropped file: {:?}", e);
            }
        });
    }

    async fn process_dropped_file(
        file: web_sys::File,
        viewport_width: f64,
        viewport_height: f64,
        camera_offset_x: f64,
        camera_offset_y: f64,
        camera_zoom: f64,
    ) -> Result<(), JsValue> {
        use drafftink_core::shapes::{Image, ImageFormat, Shape};
        use kurbo::Point;

        let file_name = file.name();
        let file_type = file.type_();

        // Read file as array buffer
        let array_buffer_promise = file.array_buffer();
        let array_buffer = wasm_bindgen_futures::JsFuture::from(array_buffer_promise).await?;
        let uint8_array = js_sys::Uint8Array::new(&array_buffer);
        let data = uint8_array.to_vec();

        // Check for embedded scene data in PNG files
        if file_type == "image/png" || file_name.ends_with(".png") {
            if let Some(json) = super::extract_scene_from_png(&data) {
                log::info!("Found embedded scene in PNG, loading as document");
                add_pending_dropped_document(json);
                return Ok(());
            }
        }

        // Determine format
        let format = if file_type == "image/jpeg"
            || file_name.ends_with(".jpg")
            || file_name.ends_with(".jpeg")
        {
            ImageFormat::Jpeg
        } else if file_type == "image/webp" || file_name.ends_with(".webp") {
            ImageFormat::WebP
        } else {
            ImageFormat::Png
        };

        // Create blob URL to decode dimensions
        let blob: web_sys::Blob = file.into();
        let blob_url = web_sys::Url::create_object_url_with_blob(&blob)?;
        let (width, height) = decode_image_dimensions(&blob_url).await?;
        web_sys::Url::revoke_object_url(&blob_url)?;

        // Calculate viewport center in world coordinates
        let viewport_center_x = (viewport_width / 2.0 - camera_offset_x) / camera_zoom;
        let viewport_center_y = (viewport_height / 2.0 - camera_offset_y) / camera_zoom;

        let position = Point::new(
            viewport_center_x - width as f64 / 2.0,
            viewport_center_y - height as f64 / 2.0,
        );

        // Create image shape, scaled to fit if too large
        let max_size = 800.0;
        let mut image = Image::new(position, &data, width, height, format);
        if width as f64 > max_size || height as f64 > max_size {
            image = image.fit_within(max_size, max_size);
            image.position = Point::new(
                viewport_center_x - image.width / 2.0,
                viewport_center_y - image.height / 2.0,
            );
        }

        log::info!("Dropped image: {} ({}x{})", file_name, width, height);
        add_pending_dropped_image(Shape::Image(image));

        Ok(())
    }

    /// Queue a dropped document (from PNG with embedded scene) for loading.
    fn add_pending_dropped_document(json: String) {
        PENDING_DROPPED_DOCUMENT.with(|cell| {
            cell.borrow_mut().replace(json);
        });
    }

    thread_local! {
        static PENDING_DROPPED_DOCUMENT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }

    /// Take any pending dropped document JSON.
    pub fn take_pending_dropped_document() -> Option<String> {
        PENDING_DROPPED_DOCUMENT.with(|cell| cell.borrow_mut().take())
    }

    fn add_pending_dropped_image(shape: drafftink_core::shapes::Shape) {
        PENDING_DROPPED_IMAGES.with(|cell| {
            cell.borrow_mut().push(shape);
        });
    }

    /// Take all pending dropped images.
    pub fn take_pending_dropped_images() -> Vec<drafftink_core::shapes::Shape> {
        PENDING_DROPPED_IMAGES.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
    }
}

/// Render a Vello scene to PNG bytes (native version - blocking).
#[cfg(not(target_arch = "wasm32"))]
fn render_scene_to_png(
    device: &vello::wgpu::Device,
    queue: &vello::wgpu::Queue,
    vello_renderer: &mut vello::Renderer,
    scene: &Scene,
    width: u32,
    height: u32,
) -> Option<PngRenderResult> {
    if width == 0 || height == 0 {
        return None;
    }

    // Create offscreen texture for rendering
    let texture = device.create_texture(&vello::wgpu::TextureDescriptor {
        label: Some("png export texture"),
        size: vello::wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: vello::wgpu::TextureDimension::D2,
        format: vello::wgpu::TextureFormat::Rgba8Unorm,
        usage: vello::wgpu::TextureUsages::STORAGE_BINDING
            | vello::wgpu::TextureUsages::COPY_SRC
            | vello::wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let texture_view = texture.create_view(&vello::wgpu::TextureViewDescriptor::default());

    // Render the scene
    let params = RenderParams {
        base_color: Color::WHITE,
        width,
        height,
        antialiasing_method: AaConfig::Area,
    };

    if let Err(e) = vello_renderer.render_to_texture(device, queue, scene, &texture_view, &params) {
        log::error!("Failed to render scene for PNG export: {:?}", e);
        return None;
    }

    // Create buffer to read back pixels
    let bytes_per_row = (width * 4).next_multiple_of(256); // wgpu alignment requirement
    let buffer_size = (bytes_per_row * height) as u64;

    let readback_buffer = device.create_buffer(&vello::wgpu::BufferDescriptor {
        label: Some("png readback buffer"),
        size: buffer_size,
        usage: vello::wgpu::BufferUsages::COPY_DST | vello::wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // Copy texture to buffer
    let mut encoder = device.create_command_encoder(&vello::wgpu::CommandEncoderDescriptor {
        label: Some("png copy encoder"),
    });

    encoder.copy_texture_to_buffer(
        vello::wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: vello::wgpu::Origin3d::ZERO,
            aspect: vello::wgpu::TextureAspect::All,
        },
        vello::wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: vello::wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        vello::wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(std::iter::once(encoder.finish()));

    // Map buffer and read pixels
    let buffer_slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer_slice.map_async(vello::wgpu::MapMode::Read, move |result| {
        tx.send(result).ok();
    });

    // Wait for GPU to finish (blocking - native only)
    let _ = device.poll(vello::wgpu::PollType::wait_indefinitely());

    if rx.recv().ok()?.is_err() {
        log::error!("Failed to map buffer for PNG readback");
        return None;
    }

    let data = buffer_slice.get_mapped_range();

    // Remove row padding if any
    let mut rgba_data = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let row_start = (row * bytes_per_row) as usize;
        let row_end = row_start + (width * 4) as usize;
        rgba_data.extend_from_slice(&data[row_start..row_end]);
    }

    drop(data);
    readback_buffer.unmap();

    Some(PngRenderResult {
        rgba_data,
        width,
        height,
    })
}

/// Async PNG export for WASM - renders scene and triggers download when complete.
/// Takes references and clones internally to avoid lifetime issues.
#[cfg(target_arch = "wasm32")]
pub fn spawn_png_export_async(
    device: &vello::wgpu::Device,
    queue: &vello::wgpu::Queue,
    scene: Scene,
    width: u32,
    height: u32,
    filename: String,
    is_copy: bool,
    scene_json: Option<String>,
) {
    use std::sync::atomic::{AtomicBool, Ordering};

    if width == 0 || height == 0 {
        log::warn!("Cannot export empty scene");
        return;
    }

    log::info!("Starting async PNG export: {}x{}", width, height);

    // Create a new Vello renderer for the export
    let mut vello_renderer = match vello::Renderer::new(device, vello::RendererOptions::default()) {
        Ok(r) => r,
        Err(e) => {
            log::error!("Failed to create Vello renderer for export: {:?}", e);
            return;
        }
    };

    // Create offscreen texture for rendering
    let texture = device.create_texture(&vello::wgpu::TextureDescriptor {
        label: Some("png export texture"),
        size: vello::wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: vello::wgpu::TextureDimension::D2,
        format: vello::wgpu::TextureFormat::Rgba8Unorm,
        usage: vello::wgpu::TextureUsages::STORAGE_BINDING
            | vello::wgpu::TextureUsages::COPY_SRC
            | vello::wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let texture_view = texture.create_view(&vello::wgpu::TextureViewDescriptor::default());

    // Render the scene
    let params = RenderParams {
        base_color: Color::WHITE,
        width,
        height,
        antialiasing_method: AaConfig::Area,
    };

    if let Err(e) = vello_renderer.render_to_texture(device, queue, &scene, &texture_view, &params)
    {
        log::error!("Failed to render scene for PNG export: {:?}", e);
        return;
    }

    // Create buffer to read back pixels
    let bytes_per_row = (width * 4).next_multiple_of(256); // wgpu alignment requirement
    let buffer_size = (bytes_per_row * height) as u64;

    let readback_buffer = device.create_buffer(&vello::wgpu::BufferDescriptor {
        label: Some("png readback buffer"),
        size: buffer_size,
        usage: vello::wgpu::BufferUsages::COPY_DST | vello::wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // Copy texture to buffer
    let mut encoder = device.create_command_encoder(&vello::wgpu::CommandEncoderDescriptor {
        label: Some("png copy encoder"),
    });

    encoder.copy_texture_to_buffer(
        vello::wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: vello::wgpu::Origin3d::ZERO,
            aspect: vello::wgpu::TextureAspect::All,
        },
        vello::wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: vello::wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        vello::wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(std::iter::once(encoder.finish()));

    // Use Arc<AtomicBool> for the callback (it's Send)
    let mapped = Arc::new(AtomicBool::new(false));
    let mapped_clone = mapped.clone();

    // Start the async mapping - the callback will set mapped to true when done
    {
        let buffer_slice = readback_buffer.slice(..);
        buffer_slice.map_async(vello::wgpu::MapMode::Read, move |result| {
            if result.is_ok() {
                mapped_clone.store(true, Ordering::SeqCst);
            } else {
                log::error!("Buffer mapping failed: {:?}", result);
            }
        });
    }

    // Spawn async task to poll and wait for mapping
    // Move readback_buffer into the task so we can access it after mapping completes
    wasm_bindgen_futures::spawn_local(async move {
        // Poll and yield until the callback fires
        let mut attempts = 0u32;
        const MAX_ATTEMPTS: u32 = 600; // ~10 seconds at 60fps

        loop {
            if mapped.load(Ordering::SeqCst) {
                log::info!("Buffer mapping completed after {} frames", attempts);
                break;
            }

            attempts += 1;
            if attempts >= MAX_ATTEMPTS {
                log::error!(
                    "Timeout waiting for buffer mapping after {} frames",
                    attempts
                );
                return;
            }

            // Yield to browser event loop using requestAnimationFrame
            // This is crucial - Promise.resolve() creates a microtask that doesn't
            // actually yield to the browser's task queue where WebGPU callbacks run
            let _ = file_ops::yield_to_browser().await;
        }

        // Now that mapping is complete, we can access the data
        let buffer_slice = readback_buffer.slice(..);
        let data = buffer_slice.get_mapped_range();

        // Remove row padding if any
        let mut rgba_data = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let row_start = (row * bytes_per_row) as usize;
            let row_end = row_start + (width * 4) as usize;
            rgba_data.extend_from_slice(&data[row_start..row_end]);
        }

        drop(data);
        readback_buffer.unmap();

        // Encode to PNG
        let png_data = match encode_png(&rgba_data, width, height, scene_json.as_deref()) {
            Some(data) => data,
            None => {
                log::error!("Failed to encode PNG");
                return;
            }
        };

        // Either copy to clipboard or trigger download
        if is_copy {
            file_ops::copy_png_to_clipboard(png_data);
        } else {
            file_ops::export_png(&png_data, &filename.trim_end_matches(".png"));
            log::info!("PNG export complete: {} bytes", png_data.len());
        }
    });
}

/// MIME type keyword for embedded scene data in PNG tEXt chunks.
const PNG_METADATA_KEYWORD: &str = "application/vnd.drafftink+json";

/// Encode RGBA pixel data to PNG bytes with optional embedded scene JSON.
fn encode_png(rgba_data: &[u8], width: u32, height: u32, scene_json: Option<&str>) -> Option<Vec<u8>> {
    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);

        // Embed scene data as compressed zTXt chunk
        if let Some(json) = scene_json {
            if let Err(e) = encoder.add_ztxt_chunk(PNG_METADATA_KEYWORD.to_string(), json.to_string()) {
                log::warn!("Failed to add metadata chunk: {:?}", e);
            }
        }

        let mut writer = match encoder.write_header() {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to write PNG header: {:?}", e);
                return None;
            }
        };

        if let Err(e) = writer.write_image_data(rgba_data) {
            log::error!("Failed to write PNG data: {:?}", e);
            return None;
        }
    }

    Some(png_data)
}

/// Extract embedded scene JSON from PNG data, if present.
pub fn extract_scene_from_png(png_data: &[u8]) -> Option<String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_data));
    let reader = decoder.read_info().ok()?;
    
    for chunk in &reader.info().compressed_latin1_text {
        if chunk.keyword == PNG_METADATA_KEYWORD {
            return chunk.get_text().ok();
        }
    }
    None
}

/// Parse a CSS color string like "#ff0000" or "rgb(255, 0, 0)".
fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if s.starts_with('#') && s.len() == 7 {
        let r = u8::from_str_radix(&s[1..3], 16).ok()?;
        let g = u8::from_str_radix(&s[3..5], 16).ok()?;
        let b = u8::from_str_radix(&s[5..7], 16).ok()?;
        Some(Color::from_rgba8(r, g, b, 255))
    } else {
        None
    }
}

/// Application configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub grid_style: GridStyle,
    pub background_color: Color,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: "DrafftInk".to_string(),
            width: 1280,
            height: 800,
            grid_style: GridStyle::Lines,
            background_color: Color::from_rgba8(250, 250, 250, 255),
        }
    }
}

/// Remote peer state for rendering cursors.
#[derive(Debug, Clone)]
pub struct RemotePeer {
    pub peer_id: String,
    pub awareness: AwarenessState,
}

/// Runtime state for the application.
struct AppState {
    // Windowing
    window: Arc<Window>,
    surface: RenderSurface<'static>,

    // Rendering
    vello_renderer: vello::Renderer,
    shape_renderer: VelloRenderer,
    /// Texture blitter for RGBA->surface format conversion (needed for WebGPU/WASM)
    texture_blitter: vello::wgpu::util::TextureBlitter,

    // egui
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    ui_state: UiState,

    // State
    canvas: Canvas,
    input: InputState,
    config: AppConfig,

    // Event handling
    event_handler: EventHandler,

    // Text editing state (when editing a text shape)
    text_edit_state: Option<TextEditState>,

    // Collaboration
    collab: CollaborationManager,
    #[cfg(target_arch = "wasm32")]
    websocket: Option<drafftink_core::sync::WasmWebSocket>,
    #[cfg(not(target_arch = "wasm32"))]
    websocket: Option<drafftink_core::sync::NativeWebSocket>,
    /// Remote peers in the current room (for cursor rendering).
    remote_peers: std::collections::HashMap<String, RemotePeer>,

    // Auto-save (WASM only)
    #[cfg(target_arch = "wasm32")]
    last_autosave: web_time::Instant,
    #[cfg(target_arch = "wasm32")]
    last_doc_version: u64,
}

/// Main application struct.
pub struct App {
    config: AppConfig,
    state: Option<AppState>,
    render_cx: Option<vello::util::RenderContext>,
    /// Window waiting for async surface creation (WASM only)
    pending_window: Option<Arc<Window>>,
    /// Flag to indicate async init is in progress
    #[cfg(target_arch = "wasm32")]
    init_in_progress: std::cell::Cell<bool>,
}

impl App {
    /// Create a new application with default configuration.
    pub fn new() -> Self {
        Self::with_config(AppConfig::default())
    }

    /// Create a new application with custom configuration.
    pub fn with_config(config: AppConfig) -> Self {
        Self {
            config,
            state: None,
            render_cx: None,
            pending_window: None,
            #[cfg(target_arch = "wasm32")]
            init_in_progress: std::cell::Cell::new(false),
        }
    }

    /// Run the application.
    pub async fn run() {
        let event_loop = EventLoop::new().expect("Failed to create event loop");
        let app = App::new();

        #[cfg(target_arch = "wasm32")]
        {
            use winit::platform::web::EventLoopExtWebSys;
            event_loop.spawn_app(app);
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut app = app;
            event_loop.run_app(&mut app).expect("Event loop error");
        }
    }

    /// Finish initialization after surface is created.
    fn finish_init(&mut self, window: Arc<Window>, surface: RenderSurface<'static>) {
        let render_cx = self
            .render_cx
            .as_ref()
            .expect("RenderContext not initialized");
        let device = &render_cx.devices[surface.dev_id].device;

        let vello_renderer = vello::Renderer::new(device, RendererOptions::default())
            .expect("Failed to create Vello renderer");

        // Create texture blitter for RGBA->surface format conversion
        // This is needed because Vello renders to Rgba8Unorm (for compute shader compatibility)
        // but the surface format on WebGPU is typically Bgra8Unorm
        let texture_blitter = vello::wgpu::util::TextureBlitter::new(device, surface.config.format);

        // Initialize egui
        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            device,
            surface.config.format,
            egui_wgpu::RendererOptions::default(),
        );

        let mut canvas = Canvas::new();
        canvas.set_viewport_size(surface.config.width as f64, surface.config.height as f64);

        // Load intro as default for native (WASM handles this in load_document_async)
        #[cfg(not(target_arch = "wasm32"))]
        {
            static INTRO_JSON: &str = include_str!("../assets/intro.json");
            if let Ok(doc) = drafftink_core::canvas::CanvasDocument::from_json(INTRO_JSON) {
                canvas.document = doc;
            }
        }

        log::info!(
            "DrafftInk initialized - {}x{}",
            surface.config.width,
            surface.config.height
        );
        log::info!(
            "Keyboard shortcuts: V=Select, H=Pan, R=Rectangle, E=Ellipse, L=Line, A=Arrow, P=Pen"
        );

        self.state = Some(AppState {
            window: window.clone(),
            surface,
            vello_renderer,
            shape_renderer: VelloRenderer::new(),
            texture_blitter,
            egui_ctx,
            egui_state,
            egui_renderer,
            ui_state: UiState::default(),
            canvas,
            input: InputState::new(),
            config: self.config.clone(),
            event_handler: EventHandler::new(),
            text_edit_state: None,
            collab: CollaborationManager::new(),
            websocket: None,
            remote_peers: std::collections::HashMap::new(),
            #[cfg(target_arch = "wasm32")]
            last_autosave: web_time::Instant::now(),
            #[cfg(target_arch = "wasm32")]
            last_doc_version: 0,
        });

        self.pending_window = None;

        // Try to restore last saved document on WASM
        #[cfg(target_arch = "wasm32")]
        {
            file_ops::try_load_last_document();
        }

        // Setup drag-drop handlers on WASM
        #[cfg(target_arch = "wasm32")]
        if let Some(ref state) = self.state {
            let vw = state.canvas.viewport_size.width;
            let vh = state.canvas.viewport_size.height;
            let cox = state.canvas.camera.offset.x;
            let coy = state.canvas.camera.offset.y;
            let cz = state.canvas.camera.zoom;
            file_ops::setup_drag_drop_handlers(vw, vh, cox, coy, cz);
        }

        // Auto-join room from URL if specified (WASM only)
        #[cfg(target_arch = "wasm32")]
        {
            self.try_auto_join_room();
        }

        // Request initial redraw
        window.request_redraw();
    }

    /// Try to auto-join a room from URL parameters (WASM only).
    #[cfg(target_arch = "wasm32")]
    fn try_auto_join_room(&mut self) {
        use crate::web::{get_server_url, get_url_params};
        use drafftink_core::sync::ConnectionState;

        let params = get_url_params();

        let room = match params.room {
            Some(r) => r,
            None => return,
        };

        let state = match self.state.as_mut() {
            Some(s) => s,
            None => return,
        };

        // Get server URL from params or origin
        let server_url = get_server_url(params.server.as_deref())
            .unwrap_or_else(|| state.ui_state.server_url.clone());

        log::info!("Auto-joining room '{}' via {}", room, server_url);

        // Update UI state
        state.ui_state.server_url = server_url.clone();
        state.ui_state.room_input = room.clone();

        // Connect to WebSocket
        let mut ws = drafftink_core::sync::WasmWebSocket::new();
        match ws.connect(&server_url) {
            Ok(()) => {
                log::info!("WebSocket connecting to {}", server_url);
                state.websocket = Some(ws);
                state.ui_state.connection_state = ConnectionState::Connecting;

                // Queue the join request (will be sent once connected)
                state.collab.join_room(&room);
            }
            Err(e) => {
                log::error!("WebSocket connect failed: {}", e);
                state.ui_state.connection_state = ConnectionState::Error;
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() || self.pending_window.is_some() {
            return;
        }

        log::info!("Creating window...");

        // Create window attributes - native gets fixed size, WASM will use viewport
        #[cfg(not(target_arch = "wasm32"))]
        let window_attrs = Window::default_attributes()
            .with_title(&self.config.title)
            .with_inner_size(LogicalSize::new(self.config.width, self.config.height));

        // On WASM, attach canvas to DOM and use full viewport
        #[cfg(target_arch = "wasm32")]
        let window_attrs = {
            use wasm_bindgen::JsCast;
            use winit::platform::web::WindowAttributesExtWebSys;

            let web_window = web_sys::window().expect("No window");
            let document = web_window.document().expect("No document");

            // Get actual viewport dimensions
            let viewport_width = web_window
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(self.config.width as f64);
            let viewport_height = web_window
                .inner_height()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(self.config.height as f64);

            // Remove loading indicator
            if let Some(loading) = document.get_element_by_id("loading") {
                let _ = loading.remove();
            }

            // Create canvas
            let canvas = document
                .get_element_by_id("drafftink-canvas")
                .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
                .or_else(|| {
                    let app_div = document.get_element_by_id("app")?;
                    let canvas = document.create_element("canvas").ok()?;
                    canvas.set_id("drafftink-canvas");
                    app_div.append_child(&canvas).ok()?;
                    canvas.dyn_into::<web_sys::HtmlCanvasElement>().ok()
                })
                .expect("Failed to create canvas");

            // Set canvas to fill viewport with actual pixel dimensions
            // Account for device pixel ratio for sharp rendering
            let dpr = web_window.device_pixel_ratio();
            let physical_width = (viewport_width * dpr) as u32;
            let physical_height = (viewport_height * dpr) as u32;

            canvas.set_width(physical_width);
            canvas.set_height(physical_height);
            let style = canvas.style();
            let _ = style.set_property("width", "100%");
            let _ = style.set_property("height", "100%");
            let _ = style.set_property("display", "block");
            let _ = style.set_property("position", "fixed");
            let _ = style.set_property("top", "0");
            let _ = style.set_property("left", "0");

            log::info!(
                "Canvas created: {}x{} (physical: {}x{}, dpr: {})",
                viewport_width,
                viewport_height,
                physical_width,
                physical_height,
                dpr
            );

            // Use viewport size for window, not fixed config size
            Window::default_attributes()
                .with_title(&self.config.title)
                .with_canvas(Some(canvas))
        };

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .expect("Failed to create window"),
        );

        log::info!("Window created, initializing renderer...");

        let size = window.inner_size();
        let (width, height) = if size.width == 0 || size.height == 0 {
            (self.config.width, self.config.height)
        } else {
            (size.width, size.height)
        };

        log::info!("Surface size: {}x{}", width, height);

        // On native, block on async surface creation
        #[cfg(not(target_arch = "wasm32"))]
        {
            let render_cx = self
                .render_cx
                .get_or_insert_with(vello::util::RenderContext::new);

            let surface = pollster::block_on(render_cx.create_surface(
                window.clone(),
                width,
                height,
                PresentMode::AutoVsync,
            ))
            .expect("Failed to create surface");

            // Transmute lifetime to 'static - safe because App owns everything
            let surface: RenderSurface<'static> = unsafe { std::mem::transmute(surface) };
            self.finish_init(window, surface);
        }

        // On WASM, store window for later async initialization
        #[cfg(target_arch = "wasm32")]
        {
            self.pending_window = Some(window);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // On WASM, handle async initialization
        #[cfg(target_arch = "wasm32")]
        if self.state.is_none() {
            if let Some(window) = self.pending_window.clone() {
                if !self.init_in_progress.get() {
                    self.init_in_progress.set(true);

                    // Get actual viewport size from browser
                    let web_window = web_sys::window().expect("No window");
                    let dpr = web_window.device_pixel_ratio();
                    let viewport_width = web_window
                        .inner_width()
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(self.config.width as f64);
                    let viewport_height = web_window
                        .inner_height()
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(self.config.height as f64);

                    let width = (viewport_width * dpr) as u32;
                    let height = (viewport_height * dpr) as u32;

                    // Get raw pointer to self for async callback
                    let self_ptr = self as *mut Self;
                    let window_clone = window.clone();

                    wasm_bindgen_futures::spawn_local(async move {
                        log::info!("Creating surface asynchronously...");

                        // Create a new RenderContext for the async operation
                        let mut render_cx = vello::util::RenderContext::new();

                        match render_cx
                            .create_surface(
                                window_clone.clone(),
                                width,
                                height,
                                PresentMode::AutoVsync,
                            )
                            .await
                        {
                            Ok(surface) => {
                                log::info!("Surface created successfully");

                                // Transmute lifetime to 'static
                                let surface: RenderSurface<'static> =
                                    unsafe { std::mem::transmute(surface) };

                                // SAFETY: We're on the same thread (WASM is single-threaded)
                                // and the App is kept alive by the event loop
                                let app = unsafe { &mut *self_ptr };
                                app.render_cx = Some(render_cx);
                                app.finish_init(window_clone, surface);
                            }
                            Err(e) => {
                                log::error!("Failed to create surface: {:?}", e);
                                let app = unsafe { &mut *self_ptr };
                                app.init_in_progress.set(false);
                            }
                        }
                    });
                }

                // Request redraw to keep the event loop running
                window.request_redraw();
            }
            return;
        }

        let Some(state) = &mut self.state else {
            return;
        };

        // Process input events through WinitInputHelper
        state.input.process_window_event(&event);

        // Let egui process the event first
        let egui_response = state.egui_state.on_window_event(&state.window, &event);

        // If egui wants this event exclusively, don't process it for canvas
        // Check both: if egui consumed the event OR if the pointer is over an egui area
        let egui_wants_input = egui_response.consumed
            || state.egui_ctx.is_pointer_over_area()
            || state.egui_ctx.wants_pointer_input()
            || state.egui_ctx.wants_keyboard_input();

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                if size.width == 0 || size.height == 0 {
                    return;
                }

                state
                    .canvas
                    .set_viewport_size(size.width as f64, size.height as f64);

                if let Some(render_cx) = self.render_cx.as_mut() {
                    render_cx.resize_surface(&mut state.surface, size.width, size.height);
                }

                state.window.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                // Update laser trail (fade out)
                state.event_handler.update_laser_trail(1.0 / 60.0);

                // Check for pending document from async file load
                if let Some(doc) = file_ops::take_pending_document() {
                    state.canvas.document = doc;
                    state.canvas.clear_selection();
                }

                // Check for pending document list (WASM)
                #[cfg(target_arch = "wasm32")]
                if let Some(docs) = file_ops::take_pending_document_list() {
                    state.ui_state.recent_documents = docs;
                }

                // Check for pending pasted image (WASM)
                #[cfg(target_arch = "wasm32")]
                if let Some(image_shape) = file_ops::take_pending_image() {
                    state.canvas.document.push_undo();
                    state.canvas.clear_selection();
                    let new_id = image_shape.id();
                    state.canvas.document.add_shape(image_shape.clone());
                    state.canvas.add_to_selection(new_id);
                    if state.collab.is_in_room() {
                        let _ = state.collab.crdt_mut().add_shape(&image_shape);
                    }
                }

                // Check for pending dropped images (WASM)
                #[cfg(target_arch = "wasm32")]
                for image_shape in file_ops::take_pending_dropped_images() {
                    state.canvas.document.push_undo();
                    state.canvas.clear_selection();
                    let new_id = image_shape.id();
                    state.canvas.document.add_shape(image_shape.clone());
                    state.canvas.add_to_selection(new_id);
                    if state.collab.is_in_room() {
                        let _ = state.collab.crdt_mut().add_shape(&image_shape);
                    }
                }

                // Check for pending dropped document (PNG with embedded scene)
                #[cfg(target_arch = "wasm32")]
                if let Some(json) = file_ops::take_pending_dropped_document() {
                    use drafftink_core::canvas::CanvasDocument;
                    match CanvasDocument::from_json(&json) {
                        Ok(doc) => {
                            log::info!("Loaded document from dropped PNG");
                            state.canvas.document = doc;
                            state.canvas.clear_selection();
                            state.canvas.camera.reset();
                        }
                        Err(e) => log::error!("Failed to parse embedded document: {}", e),
                    }
                }

                // Check for pending clipboard text paste (WASM async)
                #[cfg(target_arch = "wasm32")]
                if let Some(clipboard_text) = file_ops::take_pending_clipboard_text() {
                    if let Some(text_id) = state.event_handler.editing_text {
                        if let Some(edit_state) = &mut state.text_edit_state {
                            // Capture state before paste
                            let old_text = edit_state.text();
                            let old_char_count = old_text.chars().count();
                            let cursor_byte = edit_state.cursor_byte_offset();
                            let edit_char_pos =
                                old_text[..cursor_byte.min(old_text.len())].chars().count();

                            let (font_cx, layout_cx) = state.shape_renderer.contexts_mut();
                            let _ = edit_state.handle_key(
                                TextKey::Paste(clipboard_text),
                                TextModifiers::default(),
                                font_cx,
                                layout_cx,
                            );
                            let new_text = edit_state.text();
                            if let Some(Shape::Text(text)) =
                                state.canvas.document.get_shape_mut(text_id)
                            {
                                text.content = new_text;
                                text.sync_char_colors_after_edit(edit_char_pos, old_char_count);
                            }
                        }
                    }
                }

                // Check for pending math clipboard paste (WASM async)
                #[cfg(target_arch = "wasm32")]
                if let Some(clipboard_text) = file_ops::take_pending_math_clipboard() {
                    if let Some((_, ref mut latex)) = state.ui_state.math_editor {
                        *latex = clipboard_text;
                    }
                }

                // Poll WebSocket events
                if let Some(ref mut ws) = state.websocket {
                    let events = ws.poll_events();
                    state.ui_state.connection_state = ws.state();

                    for event in events {
                        match event {
                            SyncEvent::Connected => {
                                log::info!("WebSocket connected");
                                state.collab.enable();
                            }
                            SyncEvent::Disconnected => {
                                log::info!("WebSocket disconnected");
                                state.collab.set_room(None);
                                state.collab.disable();
                                state.ui_state.current_room = None;
                                state.ui_state.peer_count = 0;
                                state.remote_peers.clear();
                            }
                            SyncEvent::JoinedRoom {
                                room,
                                peer_count,
                                initial_sync,
                            } => {
                                log::info!("Joined room: {} ({} peers)", room, peer_count);
                                state.ui_state.current_room = Some(room.clone());
                                state.ui_state.peer_count = peer_count;

                                // Update collaboration manager state
                                state.collab.set_room(Some(room));

                                // Import initial state if provided
                                if let Some(data) = initial_sync {
                                    if state.collab.import_updates(&data) {
                                        state.collab.sync_from_crdt(&mut state.canvas.document);
                                        log::info!("Imported initial sync data");
                                    }
                                }

                                // Broadcast our current state
                                state.collab.sync_to_crdt(&state.canvas.document);
                                state.collab.broadcast_sync();
                                for msg in state.collab.take_outgoing() {
                                    let _ = ws.send(&msg);
                                }
                            }
                            SyncEvent::PeerJoined { peer_id } => {
                                log::info!("Peer joined: {}", peer_id);
                                state.ui_state.peer_count += 1;
                            }
                            SyncEvent::PeerLeft { peer_id } => {
                                log::info!("Peer left: {}", peer_id);
                                state.ui_state.peer_count =
                                    state.ui_state.peer_count.saturating_sub(1);
                                state.remote_peers.remove(&peer_id);
                            }
                            SyncEvent::SyncReceived { from, data } => {
                                log::debug!("Sync from {}: {} bytes", from, data.len());
                                if state.collab.import_updates(&data) {
                                    state.collab.sync_from_crdt(&mut state.canvas.document);
                                }
                            }
                            SyncEvent::AwarenessReceived {
                                from,
                                peer_id: _,
                                state: awareness,
                            } => {
                                state.remote_peers.insert(
                                    from.clone(),
                                    RemotePeer {
                                        peer_id: from,
                                        awareness,
                                    },
                                );
                            }
                            SyncEvent::Error { message } => {
                                log::error!("Sync error: {}", message);
                            }
                        }
                    }

                    // Send any pending outgoing messages
                    if state.collab.has_outgoing() {
                        for msg in state.collab.take_outgoing() {
                            let _ = ws.send(&msg);
                        }
                    }
                }

                // Sync UI state with canvas
                state.ui_state.current_tool = state.canvas.tool_manager.current_tool;
                state.ui_state.selection_count = state.canvas.selection.len();
                state.ui_state.zoom_level = state.canvas.camera.zoom;
                state.ui_state.grid_style = state.config.grid_style;

                // Update UI state from first selected shape's style
                if let Some(&shape_id) = state.canvas.selection.first() {
                    if let Some(shape) = state.canvas.document.get_shape(shape_id) {
                        state.ui_state.update_from_style(shape.style());
                    }
                }

                // Sync current style to tool manager for preview shapes
                state.canvas.tool_manager.current_style = state.ui_state.to_shape_style();
                state.canvas.tool_manager.corner_radius = state.ui_state.corner_radius as f64;

                // Get selected shape properties for the right panel
                let selection_count = state.canvas.selection.len();
                let mut selected_props = if selection_count >= 1 {
                    let shape_id = state.canvas.selection[0];
                    if let Some(shape) = state.canvas.document.get_shape(shape_id) {
                        SelectedShapeProps::from_shape_with_count(shape, selection_count)
                    } else {
                        SelectedShapeProps::default()
                    }
                } else {
                    SelectedShapeProps::default()
                };

                // If a drawing tool is active (not Select/Pan/Text), show the panel
                // for setting properties of shapes that will be created
                use drafftink_core::tools::ToolKind;
                let current_tool = state.canvas.tool_manager.current_tool;
                let is_drawing_tool = matches!(
                    current_tool,
                    ToolKind::Rectangle
                        | ToolKind::Ellipse
                        | ToolKind::Line
                        | ToolKind::Arrow
                        | ToolKind::Freehand
                        | ToolKind::Highlighter
                );

                if is_drawing_tool && !selected_props.has_selection {
                    selected_props = SelectedShapeProps::for_tool(
                        current_tool,
                        &state.ui_state,
                        state.canvas.tool_manager.calligraphy_mode,
                        state.canvas.tool_manager.pressure_simulation,
                    );
                }

                // Update peer info for presence panel
                state.ui_state.peers = state
                    .remote_peers
                    .values()
                    .map(|peer| {
                        crate::ui::PeerInfo {
                            peer_id: peer.peer_id.clone(),
                            name: peer.awareness.user.as_ref().map(|u| u.name.clone()),
                            color: peer
                                .awareness
                                .user
                                .as_ref()
                                .map(|u| u.color.clone())
                                .unwrap_or_else(|| "#6366f1".to_string()), // Default indigo
                            has_cursor: peer.awareness.cursor.is_some(),
                        }
                    })
                    .collect();

                // Auto-save to IndexedDB (WASM only) - every 5 seconds if document changed
                #[cfg(target_arch = "wasm32")]
                {
                    let doc_version = state.canvas.document.shapes.len() as u64
                        + state.canvas.document.z_order.len() as u64;
                    if doc_version != state.last_doc_version
                        && state.last_autosave.elapsed().as_secs() >= 5
                    {
                        file_ops::autosave_document(&state.canvas.document);
                        state.last_autosave = web_time::Instant::now();
                        state.last_doc_version = doc_version;
                    }
                }

                // Capture text selection state BEFORE egui processing
                // (mouse events may clear it during egui run)
                let text_selection_state: Option<(
                    drafftink_core::shapes::ShapeId,
                    std::ops::Range<usize>,
                )> = if let (Some(text_id), Some(edit_state)) =
                    (state.event_handler.editing_text, &state.text_edit_state)
                {
                    edit_state.selection_range().map(|r| (text_id, r))
                } else {
                    None
                };

                // Run egui and get any actions
                let egui_input = state.egui_state.take_egui_input(&state.window);
                let mut deferred_action: Option<UiAction> = None;
                let egui_output = state.egui_ctx.run(egui_input, |ctx| {
                    if let Some(action) = render_ui(ctx, &mut state.ui_state, &selected_props) {
                        match action.clone() {
                            UiAction::SetTool(tool) => {
                                state.canvas.set_tool(tool);
                            }
                            UiAction::SetStrokeColor(color) => {
                                // Update UI state
                                state.ui_state.stroke_color = color;

                                // Use captured text selection state (before mouse events cleared it)
                                let mut applied_to_text_range = false;
                                if let Some((text_id, byte_range)) = &text_selection_state {
                                    if let Some(Shape::Text(text)) =
                                        state.canvas.document.get_shape_mut(*text_id)
                                    {
                                        // Convert byte range to char indices
                                        let start_char =
                                            text.content[..byte_range.start].chars().count();
                                        let end_char =
                                            text.content[..byte_range.end].chars().count();
                                        let style = state.ui_state.to_shape_style();
                                        text.apply_color_to_range(
                                            start_char,
                                            end_char,
                                            style.stroke_color,
                                        );
                                        applied_to_text_range = true;
                                        log::info!(
                                            "Applied color to char range {}..{}",
                                            start_char,
                                            end_char
                                        );
                                    }
                                }

                                // If not applied to text range, apply to whole shapes
                                if !applied_to_text_range {
                                    let style = state.ui_state.to_shape_style();
                                    for &shape_id in &state.canvas.selection.clone() {
                                        if let Some(shape) =
                                            state.canvas.document.get_shape_mut(shape_id)
                                        {
                                            shape.style_mut().stroke_color = style.stroke_color;
                                        }
                                    }
                                }

                                // Sync property changes
                                let has_changes =
                                    applied_to_text_range || !state.canvas.selection.is_empty();
                                if has_changes && state.collab.is_in_room() {
                                    state.collab.sync_to_crdt(&state.canvas.document);
                                    state.collab.broadcast_sync();
                                    if let Some(ref ws) = state.websocket {
                                        for msg in state.collab.take_outgoing() {
                                            let _ = ws.send(&msg);
                                        }
                                    }
                                }
                            }
                            UiAction::SetFillColor(color) => {
                                state.ui_state.fill_color = color;
                                let style = state.ui_state.to_shape_style();
                                let has_selection = !state.canvas.selection.is_empty();
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(shape) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        shape.style_mut().fill_color = style.fill_color;
                                    }
                                }
                                // Sync property changes
                                if has_selection && state.collab.is_in_room() {
                                    state.collab.sync_to_crdt(&state.canvas.document);
                                    state.collab.broadcast_sync();
                                    if let Some(ref ws) = state.websocket {
                                        for msg in state.collab.take_outgoing() {
                                            let _ = ws.send(&msg);
                                        }
                                    }
                                }
                            }
                            UiAction::SetStrokeWidth(width) => {
                                state.ui_state.stroke_width = width;
                                let has_selection = !state.canvas.selection.is_empty();
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(shape) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        shape.style_mut().stroke_width = width as f64;
                                    }
                                }
                                // Sync property changes
                                if has_selection && state.collab.is_in_room() {
                                    state.collab.sync_to_crdt(&state.canvas.document);
                                    state.collab.broadcast_sync();
                                    if let Some(ref ws) = state.websocket {
                                        for msg in state.collab.take_outgoing() {
                                            let _ = ws.send(&msg);
                                        }
                                    }
                                }
                            }
                            UiAction::SaveLocal => {
                                // Save with current document name
                                file_ops::save_document(
                                    &state.canvas.document,
                                    &state.canvas.document.name,
                                );
                            }
                            UiAction::SaveLocalAs => {
                                // Open save dialog
                                state.ui_state.save_name_input = state.canvas.document.name.clone();
                                state.ui_state.save_dialog_open = true;
                            }
                            UiAction::ShowOpenDialog => {
                                // Open document selection dialog
                                state.ui_state.open_dialog_open = true;
                                #[cfg(target_arch = "wasm32")]
                                {
                                    file_ops::list_documents_async();
                                }
                            }
                            UiAction::ShowOpenRecentDialog => {
                                // Open recent documents dialog
                                state.ui_state.open_recent_dialog_open = true;
                                #[cfg(target_arch = "wasm32")]
                                {
                                    file_ops::list_documents_async();
                                }
                            }
                            UiAction::SaveLocalWithName(name) => {
                                // Save with specified name
                                state.canvas.document.name = name.clone();
                                #[cfg(target_arch = "wasm32")]
                                {
                                    // For WASM, save with name as ID
                                    let doc_id = state.canvas.document.id.clone();
                                    state.canvas.document.id = name.clone();
                                    file_ops::save_document(&state.canvas.document, &name);
                                    state.canvas.document.id = doc_id; // Restore original ID
                                }
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    file_ops::save_document(&state.canvas.document, &name);
                                }
                                // Add to recent documents if not already there
                                if !state.ui_state.recent_documents.contains(&name) {
                                    state.ui_state.recent_documents.insert(0, name);
                                    if state.ui_state.recent_documents.len() > 10 {
                                        state.ui_state.recent_documents.truncate(10);
                                    }
                                }
                            }
                            UiAction::LoadLocal(name) => {
                                // Load document by name from local storage
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    file_ops::load_document_by_name(&name);
                                }
                                #[cfg(target_arch = "wasm32")]
                                {
                                    file_ops::load_document_by_name_async(&name);
                                }
                            }
                            UiAction::SaveDocument => {
                                file_ops::save_document(
                                    &state.canvas.document,
                                    &state.canvas.document.name,
                                );
                            }
                            UiAction::LoadDocument => {
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    file_ops::load_document();
                                }
                                #[cfg(target_arch = "wasm32")]
                                {
                                    file_ops::load_document_async();
                                }
                            }
                            UiAction::DownloadDocument => {
                                // Download as file (WASM: triggers browser download, Native: same as save)
                                #[cfg(target_arch = "wasm32")]
                                file_ops::download_document(
                                    &state.canvas.document,
                                    &state.canvas.document.name,
                                );
                                #[cfg(not(target_arch = "wasm32"))]
                                file_ops::save_document(
                                    &state.canvas.document,
                                    &state.canvas.document.name,
                                );
                            }
                            UiAction::UploadDocument => {
                                // Upload from file (WASM: triggers file picker, Native: same as load)
                                #[cfg(target_arch = "wasm32")]
                                file_ops::upload_document_async();
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    file_ops::load_document();
                                }
                            }
                            UiAction::ClearDocument => {
                                if !state.canvas.document.is_empty() {
                                    state.canvas.document.push_undo();
                                    state.canvas.document.clear();
                                    state.canvas.clear_selection();
                                    log::info!("Document cleared");
                                    // Sync changes to collaborators
                                    if state.collab.is_in_room() {
                                        state.collab.sync_to_crdt(&state.canvas.document);
                                        state.collab.broadcast_sync();
                                        if let Some(ref ws) = state.websocket {
                                            for msg in state.collab.take_outgoing() {
                                                let _ = ws.send(&msg);
                                            }
                                        }
                                    }
                                }
                            }
                            UiAction::ShowIntro => {
                                static INTRO_JSON: &str = include_str!("../assets/intro.json");
                                if let Ok(doc) =
                                    drafftink_core::canvas::CanvasDocument::from_json(INTRO_JSON)
                                {
                                    state.canvas.document = doc;
                                    state.canvas.clear_selection();
                                    state.canvas.camera.reset();
                                    log::info!("Loaded intro document");
                                }
                            }
                            UiAction::ExportPng | UiAction::CopyPng => {
                                // Deferred - handled after egui run (needs render_cx access)
                                deferred_action = Some(action);
                            }
                            UiAction::ToggleGrid => {
                                state.config.grid_style = state.config.grid_style.next();
                                state.ui_state.grid_style = state.config.grid_style;
                            }
                            UiAction::ZoomIn => {
                                let center = kurbo::Point::new(
                                    state.canvas.viewport_size.width / 2.0,
                                    state.canvas.viewport_size.height / 2.0,
                                );
                                state.canvas.camera.zoom_at(center, 1.25);
                                state.ui_state.zoom_level = state.canvas.camera.zoom;
                            }
                            UiAction::ZoomOut => {
                                let center = kurbo::Point::new(
                                    state.canvas.viewport_size.width / 2.0,
                                    state.canvas.viewport_size.height / 2.0,
                                );
                                state.canvas.camera.zoom_at(center, 0.8);
                                state.ui_state.zoom_level = state.canvas.camera.zoom;
                            }
                            UiAction::ZoomReset => {
                                state.canvas.camera.zoom = drafftink_core::camera::BASE_ZOOM;
                                state.ui_state.zoom_level = drafftink_core::camera::BASE_ZOOM;
                            }
                            UiAction::CenterCanvas => {
                                // Reset camera offset to default (origin at top-left)
                                state.canvas.camera.offset = kurbo::Vec2::ZERO;
                            }
                            UiAction::ToggleSnap => {
                                state.ui_state.snap_mode = state.ui_state.snap_mode.next();
                            }
                            UiAction::ToggleAngleSnap => {
                                state.ui_state.angle_snap_enabled =
                                    !state.ui_state.angle_snap_enabled;
                                log::info!(
                                    "Angle snap: {}",
                                    if state.ui_state.angle_snap_enabled {
                                        "ON"
                                    } else {
                                        "OFF"
                                    }
                                );
                            }
                            UiAction::SetFontSize(size) => {
                                use drafftink_core::shapes::Shape;
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(Shape::Text(text)) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        text.font_size = size as f64;
                                    }
                                }
                            }
                            UiAction::SetMathFontSize(size) => {
                                use drafftink_core::shapes::Shape;
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(Shape::Math(math)) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        math.font_size = size as f64;
                                        math.invalidate_cache();
                                    }
                                }
                            }
                            UiAction::SetFontFamily(family_idx) => {
                                use drafftink_core::shapes::{FontFamily, Shape};
                                let family = match family_idx {
                                    0 => FontFamily::GelPen,
                                    1 => FontFamily::VanillaExtract,
                                    _ => FontFamily::GelPenSerif,
                                };
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(Shape::Text(text)) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        text.font_family = family;
                                    }
                                }
                            }
                            UiAction::SetFontWeight(weight_idx) => {
                                use drafftink_core::shapes::{FontWeight, Shape};
                                let weight = match weight_idx {
                                    0 => FontWeight::Light,
                                    1 => FontWeight::Regular,
                                    _ => FontWeight::Heavy,
                                };
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(Shape::Text(text)) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        text.font_weight = weight;
                                    }
                                }
                            }
                            UiAction::SetCornerRadius(radius) => {
                                use drafftink_core::shapes::Shape;
                                // Update UI state (for new shapes)
                                state.ui_state.corner_radius = radius;
                                let has_selection = !state.canvas.selection.is_empty();
                                // Apply to selected shapes
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(Shape::Rectangle(rect)) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        rect.corner_radius = radius as f64;
                                    }
                                }
                                // Sync property changes
                                if has_selection && state.collab.is_in_room() {
                                    state.collab.sync_to_crdt(&state.canvas.document);
                                    state.collab.broadcast_sync();
                                    if let Some(ref ws) = state.websocket {
                                        for msg in state.collab.take_outgoing() {
                                            let _ = ws.send(&msg);
                                        }
                                    }
                                }
                            }
                            UiAction::SetExportScale(scale) => {
                                state.ui_state.export_scale = scale;
                                log::info!("Export scale: {}x", scale);
                            }
                            UiAction::SetSloppiness(level) => {
                                use drafftink_core::shapes::Sloppiness;
                                let sloppiness = match level {
                                    0 => Sloppiness::Architect,
                                    1 => Sloppiness::Artist,
                                    2 => Sloppiness::Cartoonist,
                                    _ => Sloppiness::Drunk,
                                };
                                // Update UI state (for new shapes)
                                state.ui_state.sloppiness = sloppiness;
                                let has_selection = !state.canvas.selection.is_empty();
                                // Apply to selected shapes
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(shape) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        shape.style_mut().sloppiness = sloppiness;
                                    }
                                }
                                log::info!("Sloppiness: {:?}", sloppiness);
                                // Sync property changes
                                if has_selection && state.collab.is_in_room() {
                                    state.collab.sync_to_crdt(&state.canvas.document);
                                    state.collab.broadcast_sync();
                                    if let Some(ref ws) = state.websocket {
                                        for msg in state.collab.take_outgoing() {
                                            let _ = ws.send(&msg);
                                        }
                                    }
                                }
                            }
                            UiAction::SetFillPattern(level) => {
                                use drafftink_core::shapes::FillPattern;
                                let fill_pattern = match level {
                                    0 => FillPattern::Solid,
                                    1 => FillPattern::Hachure,
                                    2 => FillPattern::ZigZag,
                                    3 => FillPattern::CrossHatch,
                                    4 => FillPattern::Dots,
                                    5 => FillPattern::Dashed,
                                    _ => FillPattern::ZigZagLine,
                                };
                                state.ui_state.fill_pattern = fill_pattern;
                                let has_selection = !state.canvas.selection.is_empty();
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(shape) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        shape.style_mut().fill_pattern = fill_pattern;
                                    }
                                }
                                log::info!("Fill pattern: {:?}", fill_pattern);
                                if has_selection && state.collab.is_in_room() {
                                    state.collab.sync_to_crdt(&state.canvas.document);
                                    state.collab.broadcast_sync();
                                    if let Some(ref ws) = state.websocket {
                                        for msg in state.collab.take_outgoing() {
                                            let _ = ws.send(&msg);
                                        }
                                    }
                                }
                            }
                            UiAction::SetPathStyle(level) => {
                                use drafftink_core::shapes::PathStyle;
                                let path_style = match level {
                                    0 => PathStyle::Direct,
                                    1 => PathStyle::Flowing,
                                    _ => PathStyle::Angular,
                                };
                                // Always update UI state for new shapes
                                state.ui_state.path_style = level;
                                let has_selection = !state.canvas.selection.is_empty();
                                // Apply to selected lines/arrows
                                for &shape_id in &state.canvas.selection.clone() {
                                    if let Some(shape) =
                                        state.canvas.document.get_shape_mut(shape_id)
                                    {
                                        match shape {
                                            Shape::Line(line) => {
                                                line.path_style = path_style;
                                                // Angular recomputes path, clear intermediate points
                                                if path_style == PathStyle::Angular {
                                                    line.intermediate_points.clear();
                                                }
                                            }
                                            Shape::Arrow(arrow) => {
                                                arrow.path_style = path_style;
                                                if path_style == PathStyle::Angular {
                                                    arrow.intermediate_points.clear();
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                log::info!("PathStyle: {:?}", path_style);
                                // Sync property changes
                                if has_selection && state.collab.is_in_room() {
                                    state.collab.sync_to_crdt(&state.canvas.document);
                                    state.collab.broadcast_sync();
                                    if let Some(ref ws) = state.websocket {
                                        for msg in state.collab.take_outgoing() {
                                            let _ = ws.send(&msg);
                                        }
                                    }
                                }
                            }
                            UiAction::Undo => {
                                if state.canvas.document.undo() {
                                    state.canvas.clear_selection();
                                    log::info!("Undo performed");
                                    // Sync changes to collaborators
                                    if state.collab.is_in_room() {
                                        state.collab.sync_to_crdt(&state.canvas.document);
                                        state.collab.broadcast_sync();
                                        if let Some(ref ws) = state.websocket {
                                            for msg in state.collab.take_outgoing() {
                                                let _ = ws.send(&msg);
                                            }
                                        }
                                    }
                                } else {
                                    log::info!("Nothing to undo");
                                }
                            }
                            UiAction::Redo => {
                                if state.canvas.document.redo() {
                                    state.canvas.clear_selection();
                                    log::info!("Redo performed");
                                    // Sync changes to collaborators
                                    if state.collab.is_in_room() {
                                        state.collab.sync_to_crdt(&state.canvas.document);
                                        state.collab.broadcast_sync();
                                        if let Some(ref ws) = state.websocket {
                                            for msg in state.collab.take_outgoing() {
                                                let _ = ws.send(&msg);
                                            }
                                        }
                                    }
                                } else {
                                    log::info!("Nothing to redo");
                                }
                            }
                            // Collaboration actions
                            UiAction::Connect(url) => {
                                log::info!("Connect requested to: {}", url);
                                #[cfg(target_arch = "wasm32")]
                                let mut ws = drafftink_core::sync::WasmWebSocket::new();
                                #[cfg(not(target_arch = "wasm32"))]
                                let mut ws = drafftink_core::sync::NativeWebSocket::new();

                                match ws.connect(&url) {
                                    Ok(()) => {
                                        log::info!("WebSocket connecting to {}", url);
                                        state.websocket = Some(ws);
                                        state.ui_state.connection_state =
                                            ConnectionState::Connecting;
                                    }
                                    Err(e) => {
                                        log::error!("WebSocket connect failed: {}", e);
                                        state.ui_state.connection_state = ConnectionState::Error;
                                    }
                                }
                            }
                            UiAction::Disconnect => {
                                log::info!("Disconnect requested");
                                if let Some(ref mut ws) = state.websocket {
                                    ws.disconnect();
                                }
                                state.websocket = None;
                                state.collab.set_room(None);
                                state.collab.disable();
                                state.remote_peers.clear();
                                state.ui_state.connection_state = ConnectionState::Disconnected;
                                state.ui_state.current_room = None;
                            }
                            UiAction::JoinRoom(room) => {
                                log::info!("Join room requested: {}", room);
                                state.collab.join_room(&room);
                                // Send queued messages
                                if let Some(ref ws) = state.websocket {
                                    for msg in state.collab.take_outgoing() {
                                        let _ = ws.send(&msg);
                                    }
                                }
                            }
                            UiAction::LeaveRoom => {
                                log::info!("Leave room requested");
                                state.collab.leave_room();
                                // Send queued messages
                                if let Some(ref ws) = state.websocket {
                                    for msg in state.collab.take_outgoing() {
                                        let _ = ws.send(&msg);
                                    }
                                }
                                state.collab.set_room(None);
                                state.remote_peers.clear();
                                state.ui_state.current_room = None;
                            }
                            UiAction::SetUserName(name) => {
                                log::info!("Set user name: {}", name);
                                let color = state.ui_state.user_color.clone();
                                state.collab.set_user_info(name, color);
                                // Send awareness update
                                if let Some(ref ws) = state.websocket {
                                    for msg in state.collab.take_outgoing() {
                                        let _ = ws.send(&msg);
                                    }
                                }
                            }
                            UiAction::SetUserColor(color) => {
                                log::info!("Set user color: {}", color);
                                let name = state.ui_state.user_name.clone();
                                state.collab.set_user_info(name, color);
                                // Send awareness update
                                if let Some(ref ws) = state.websocket {
                                    for msg in state.collab.take_outgoing() {
                                        let _ = ws.send(&msg);
                                    }
                                }
                            }
                            UiAction::SetBgColor(color) => {
                                state.ui_state.bg_color = color;
                                // Convert egui Color32 to peniko Color for renderer
                                state.config.background_color =
                                    Color::from_rgba8(color.r(), color.g(), color.b(), color.a());
                            }
                            UiAction::BringToFront => {
                                if !state.canvas.selection.is_empty() {
                                    state.canvas.document.push_undo();
                                    for &id in &state.canvas.selection.clone() {
                                        state.canvas.document.bring_to_front(id);
                                        // Sync to CRDT if connected
                                        if state.collab.is_in_room() {
                                            let _ = state
                                                .collab
                                                .crdt_mut()
                                                .bring_to_front(&id.to_string());
                                        }
                                    }
                                }
                            }
                            UiAction::SendToBack => {
                                if !state.canvas.selection.is_empty() {
                                    state.canvas.document.push_undo();
                                    // Send in reverse order to maintain relative order
                                    for &id in state.canvas.selection.clone().iter().rev() {
                                        state.canvas.document.send_to_back(id);
                                        // Sync to CRDT if connected
                                        if state.collab.is_in_room() {
                                            let _ = state
                                                .collab
                                                .crdt_mut()
                                                .send_to_back(&id.to_string());
                                        }
                                    }
                                }
                            }
                            UiAction::BringForward => {
                                if !state.canvas.selection.is_empty() {
                                    state.canvas.document.push_undo();
                                    // Bring forward in reverse order (frontmost first) to avoid conflicts
                                    let mut selection = state.canvas.selection.clone();
                                    // Sort by z-order (back to front)
                                    selection.sort_by_key(|id| {
                                        state
                                            .canvas
                                            .document
                                            .z_order
                                            .iter()
                                            .position(|z| z == id)
                                            .unwrap_or(0)
                                    });
                                    for &id in selection.iter().rev() {
                                        state.canvas.document.bring_forward(id);
                                        // Sync to CRDT if connected
                                        if state.collab.is_in_room() {
                                            let _ = state
                                                .collab
                                                .crdt_mut()
                                                .bring_forward(&id.to_string());
                                        }
                                    }
                                }
                            }
                            UiAction::SendBackward => {
                                if !state.canvas.selection.is_empty() {
                                    state.canvas.document.push_undo();
                                    // Send backward in order (backmost first) to avoid conflicts
                                    let mut selection = state.canvas.selection.clone();
                                    // Sort by z-order (back to front)
                                    selection.sort_by_key(|id| {
                                        state
                                            .canvas
                                            .document
                                            .z_order
                                            .iter()
                                            .position(|z| z == id)
                                            .unwrap_or(0)
                                    });
                                    for &id in &selection {
                                        state.canvas.document.send_backward(id);
                                        // Sync to CRDT if connected
                                        if state.collab.is_in_room() {
                                            let _ = state
                                                .collab
                                                .crdt_mut()
                                                .send_backward(&id.to_string());
                                        }
                                    }
                                }
                            }
                            UiAction::ZoomToFit => {
                                // Fit to selection if any, otherwise fit to all shapes
                                let bounds = if state.canvas.selection.is_empty() {
                                    state.canvas.document.bounds()
                                } else {
                                    // Calculate bounds of selected shapes
                                    let mut result: Option<kurbo::Rect> = None;
                                    for &id in &state.canvas.selection {
                                        if let Some(shape) = state.canvas.document.get_shape(id) {
                                            let b = shape.bounds();
                                            result = Some(match result {
                                                Some(r) => r.union(b),
                                                None => b,
                                            });
                                        }
                                    }
                                    result
                                };
                                if let Some(bounds) = bounds {
                                    state.canvas.camera.fit_to_bounds(
                                        bounds,
                                        state.canvas.viewport_size,
                                        50.0,
                                    );
                                }
                            }
                            UiAction::Duplicate => {
                                if !state.canvas.selection.is_empty() {
                                    state.canvas.document.push_undo();
                                    let mut new_selection = Vec::new();
                                    for &id in &state.canvas.selection.clone() {
                                        if let Some(shape) = state.canvas.document.get_shape(id) {
                                            let mut new_shape = shape.clone();
                                            // Generate a new unique ID for the duplicate
                                            new_shape.regenerate_id();
                                            // Offset slightly down-right
                                            new_shape.transform(kurbo::Affine::translate(
                                                kurbo::Vec2::new(20.0, 20.0),
                                            ));
                                            let new_id = new_shape.id();
                                            state.canvas.document.add_shape(new_shape.clone());
                                            new_selection.push(new_id);
                                            // Sync to CRDT
                                            if state.collab.is_in_room() {
                                                let _ =
                                                    state.collab.crdt_mut().add_shape(&new_shape);
                                            }
                                        }
                                    }
                                    // Select the new shapes
                                    state.canvas.clear_selection();
                                    for id in new_selection {
                                        state.canvas.add_to_selection(id);
                                    }
                                }
                            }
                            UiAction::CopyShapes => {
                                if !state.canvas.selection.is_empty() {
                                    let shapes: Vec<Shape> = state
                                        .canvas
                                        .selection
                                        .iter()
                                        .filter_map(|&id| {
                                            state.canvas.document.get_shape(id).cloned()
                                        })
                                        .collect();
                                    if let Ok(json) = serde_json::to_string(&shapes) {
                                        state.ui_state.clipboard_shapes = Some(json);
                                        log::info!("Copied {} shapes to clipboard", shapes.len());
                                    }
                                }
                            }
                            UiAction::CutShapes => {
                                if !state.canvas.selection.is_empty() {
                                    let shapes: Vec<Shape> = state
                                        .canvas
                                        .selection
                                        .iter()
                                        .filter_map(|&id| {
                                            state.canvas.document.get_shape(id).cloned()
                                        })
                                        .collect();
                                    if let Ok(json) = serde_json::to_string(&shapes) {
                                        state.ui_state.clipboard_shapes = Some(json);
                                        log::info!("Cut {} shapes to clipboard", shapes.len());
                                        // Delete the shapes
                                        state.canvas.document.push_undo();
                                        for &id in &state.canvas.selection.clone() {
                                            state.canvas.document.remove_shape(id);
                                            if state.collab.is_in_room() {
                                                let _ = state
                                                    .collab
                                                    .crdt_mut()
                                                    .remove_shape(&id.to_string());
                                            }
                                        }
                                        state.canvas.clear_selection();
                                    }
                                }
                            }
                            UiAction::PasteShapes => {
                                if let Some(json) = &state.ui_state.clipboard_shapes.clone() {
                                    if let Ok(shapes) = serde_json::from_str::<Vec<Shape>>(json) {
                                        state.canvas.document.push_undo();
                                        state.canvas.clear_selection();
                                        for shape in shapes {
                                            // Create new shape with a new unique ID
                                            let mut new_shape = shape.clone();
                                            new_shape.regenerate_id();
                                            // Offset slightly down-right
                                            new_shape.transform(kurbo::Affine::translate(
                                                kurbo::Vec2::new(20.0, 20.0),
                                            ));
                                            let new_id = new_shape.id();
                                            state.canvas.document.add_shape(new_shape.clone());
                                            state.canvas.add_to_selection(new_id);
                                            if state.collab.is_in_room() {
                                                let _ =
                                                    state.collab.crdt_mut().add_shape(&new_shape);
                                            }
                                        }
                                        log::info!("Pasted shapes from clipboard");
                                    }
                                }
                            }
                            UiAction::AlignLeft => {
                                if state.canvas.selection.len() >= 2 {
                                    state.canvas.document.push_undo();
                                    // Find leftmost x
                                    let min_x = state
                                        .canvas
                                        .selection
                                        .iter()
                                        .filter_map(|&id| state.canvas.document.get_shape(id))
                                        .map(|s| s.bounds().x0)
                                        .fold(f64::INFINITY, f64::min);
                                    // Align all shapes to left
                                    for &id in &state.canvas.selection {
                                        if let Some(shape) = state.canvas.document.get_shape_mut(id)
                                        {
                                            let bounds = shape.bounds();
                                            let delta = min_x - bounds.x0;
                                            shape.transform(kurbo::Affine::translate(
                                                kurbo::Vec2::new(delta, 0.0),
                                            ));
                                        }
                                    }
                                }
                            }
                            UiAction::AlignRight => {
                                if state.canvas.selection.len() >= 2 {
                                    state.canvas.document.push_undo();
                                    let max_x = state
                                        .canvas
                                        .selection
                                        .iter()
                                        .filter_map(|&id| state.canvas.document.get_shape(id))
                                        .map(|s| s.bounds().x1)
                                        .fold(f64::NEG_INFINITY, f64::max);
                                    for &id in &state.canvas.selection {
                                        if let Some(shape) = state.canvas.document.get_shape_mut(id)
                                        {
                                            let bounds = shape.bounds();
                                            let delta = max_x - bounds.x1;
                                            shape.transform(kurbo::Affine::translate(
                                                kurbo::Vec2::new(delta, 0.0),
                                            ));
                                        }
                                    }
                                }
                            }
                            UiAction::AlignTop => {
                                if state.canvas.selection.len() >= 2 {
                                    state.canvas.document.push_undo();
                                    let min_y = state
                                        .canvas
                                        .selection
                                        .iter()
                                        .filter_map(|&id| state.canvas.document.get_shape(id))
                                        .map(|s| s.bounds().y0)
                                        .fold(f64::INFINITY, f64::min);
                                    for &id in &state.canvas.selection {
                                        if let Some(shape) = state.canvas.document.get_shape_mut(id)
                                        {
                                            let bounds = shape.bounds();
                                            let delta = min_y - bounds.y0;
                                            shape.transform(kurbo::Affine::translate(
                                                kurbo::Vec2::new(0.0, delta),
                                            ));
                                        }
                                    }
                                }
                            }
                            UiAction::AlignBottom => {
                                if state.canvas.selection.len() >= 2 {
                                    state.canvas.document.push_undo();
                                    let max_y = state
                                        .canvas
                                        .selection
                                        .iter()
                                        .filter_map(|&id| state.canvas.document.get_shape(id))
                                        .map(|s| s.bounds().y1)
                                        .fold(f64::NEG_INFINITY, f64::max);
                                    for &id in &state.canvas.selection {
                                        if let Some(shape) = state.canvas.document.get_shape_mut(id)
                                        {
                                            let bounds = shape.bounds();
                                            let delta = max_y - bounds.y1;
                                            shape.transform(kurbo::Affine::translate(
                                                kurbo::Vec2::new(0.0, delta),
                                            ));
                                        }
                                    }
                                }
                            }
                            UiAction::AlignCenterH => {
                                if state.canvas.selection.len() >= 2 {
                                    state.canvas.document.push_undo();
                                    // Calculate combined bounds center Y
                                    let mut combined: Option<kurbo::Rect> = None;
                                    for &id in &state.canvas.selection {
                                        if let Some(shape) = state.canvas.document.get_shape(id) {
                                            let b = shape.bounds();
                                            combined = Some(match combined {
                                                Some(r) => r.union(b),
                                                None => b,
                                            });
                                        }
                                    }
                                    if let Some(bounds) = combined {
                                        let center_y = bounds.center().y;
                                        for &id in &state.canvas.selection {
                                            if let Some(shape) =
                                                state.canvas.document.get_shape_mut(id)
                                            {
                                                let shape_center_y = shape.bounds().center().y;
                                                let delta = center_y - shape_center_y;
                                                shape.transform(kurbo::Affine::translate(
                                                    kurbo::Vec2::new(0.0, delta),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                            UiAction::AlignCenterV => {
                                if state.canvas.selection.len() >= 2 {
                                    state.canvas.document.push_undo();
                                    // Calculate combined bounds center X
                                    let mut combined: Option<kurbo::Rect> = None;
                                    for &id in &state.canvas.selection {
                                        if let Some(shape) = state.canvas.document.get_shape(id) {
                                            let b = shape.bounds();
                                            combined = Some(match combined {
                                                Some(r) => r.union(b),
                                                None => b,
                                            });
                                        }
                                    }
                                    if let Some(bounds) = combined {
                                        let center_x = bounds.center().x;
                                        for &id in &state.canvas.selection {
                                            if let Some(shape) =
                                                state.canvas.document.get_shape_mut(id)
                                            {
                                                let shape_center_x = shape.bounds().center().x;
                                                let delta = center_x - shape_center_x;
                                                shape.transform(kurbo::Affine::translate(
                                                    kurbo::Vec2::new(delta, 0.0),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                            UiAction::ShowShortcuts => {
                                state.ui_state.shortcuts_modal_open =
                                    !state.ui_state.shortcuts_modal_open;
                            }
                            UiAction::ToggleCalligraphy => {
                                state.canvas.tool_manager.calligraphy_mode =
                                    !state.canvas.tool_manager.calligraphy_mode;
                                log::info!(
                                    "Calligraphy mode: {}",
                                    state.canvas.tool_manager.calligraphy_mode
                                );
                            }
                            UiAction::TogglePressureSimulation => {
                                state.canvas.tool_manager.pressure_simulation =
                                    !state.canvas.tool_manager.pressure_simulation;
                                log::info!(
                                    "Pressure simulation: {}",
                                    state.canvas.tool_manager.pressure_simulation
                                );
                            }
                            UiAction::FlipHorizontal => {
                                if !state.canvas.selection.is_empty() {
                                    state.canvas.document.push_undo();
                                    state.canvas.flip_selected_horizontal();
                                    log::info!("Flipped selection horizontally");
                                }
                            }
                            UiAction::FlipVertical => {
                                if !state.canvas.selection.is_empty() {
                                    state.canvas.document.push_undo();
                                    state.canvas.flip_selected_vertical();
                                    log::info!("Flipped selection vertically");
                                }
                            }
                            UiAction::SetOpacity(opacity) => {
                                if !state.canvas.selection.is_empty() {
                                    state.canvas.document.push_undo();
                                    for &id in &state.canvas.selection {
                                        if let Some(shape) = state.canvas.document.get_shape_mut(id)
                                        {
                                            shape.style_mut().opacity = opacity as f64;
                                        }
                                    }
                                    log::info!("Set opacity to {}%", (opacity * 100.0) as i32);
                                }
                            }
                            UiAction::UpdateMathLatex(shape_id, latex) => {
                                state.canvas.document.push_undo();
                                if let Some(Shape::Math(math)) =
                                    state.canvas.document.get_shape_mut(shape_id)
                                {
                                    math.set_latex(latex);
                                    log::info!("Updated math LaTeX");
                                }
                            }
                        }
                    }
                });

                state
                    .egui_state
                    .handle_platform_output(&state.window, egui_output.platform_output);
                let egui_primitives = state
                    .egui_ctx
                    .tessellate(egui_output.shapes, egui_output.pixels_per_point);

                // Handle deferred actions that need render_cx access
                if let Some(action) = deferred_action {
                    if let Some(render_cx) = self.render_cx.as_ref() {
                        let device_handle = &render_cx.devices[state.surface.dev_id];

                        let export_scale = state.ui_state.export_scale as f64;

                        match action {
                            UiAction::ExportPng => {
                                // Build export scene (full document) with scale
                                let (scene, bounds) = state
                                    .shape_renderer
                                    .build_export_scene(&state.canvas.document, export_scale);
                                if let Some(bounds) = bounds {
                                    let width = bounds.width().ceil() as u32;
                                    let height = bounds.height().ceil() as u32;
                                    let device = &device_handle.device;
                                    let queue = &device_handle.queue;

                                    log::info!(
                                        "Exporting PNG at {}x scale: {}x{}",
                                        state.ui_state.export_scale,
                                        width,
                                        height
                                    );

                                    #[cfg(not(target_arch = "wasm32"))]
                                    {
                                        if let Some(result) = render_scene_to_png(
                                            device,
                                            queue,
                                            &mut state.vello_renderer,
                                            &scene,
                                            width,
                                            height,
                                        ) {
                                            let scene_json = state.canvas.document.to_json().ok();
                                            if let Some(png_data) = encode_png(
                                                &result.rgba_data,
                                                result.width,
                                                result.height,
                                                scene_json.as_deref(),
                                            ) {
                                                file_ops::export_png(
                                                    &png_data,
                                                    &state.canvas.document.name,
                                                );
                                            }
                                        }
                                    }

                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        let filename =
                                            format!("{}.png", state.canvas.document.name);
                                        let scene_json = state.canvas.document.to_json().ok();
                                        spawn_png_export_async(
                                            device, queue, scene, width, height, filename, false, scene_json,
                                        );
                                    }
                                } else {
                                    log::info!("Nothing to export - document is empty");
                                }
                            }
                            UiAction::CopyPng => {
                                // Build export scene (selection or full document) with scale
                                let (scene, bounds) = if state.canvas.selection.is_empty() {
                                    state
                                        .shape_renderer
                                        .build_export_scene(&state.canvas.document, export_scale)
                                } else {
                                    state.shape_renderer.build_export_scene_selection(
                                        &state.canvas.document,
                                        &state.canvas.selection,
                                        export_scale,
                                    )
                                };

                                if let Some(bounds) = bounds {
                                    let width = bounds.width().ceil() as u32;
                                    let height = bounds.height().ceil() as u32;
                                    let device = &device_handle.device;
                                    let queue = &device_handle.queue;

                                    log::info!(
                                        "Copying PNG at {}x scale: {}x{}",
                                        state.ui_state.export_scale,
                                        width,
                                        height
                                    );

                                    #[cfg(not(target_arch = "wasm32"))]
                                    {
                                        if let Some(result) = render_scene_to_png(
                                            device,
                                            queue,
                                            &mut state.vello_renderer,
                                            &scene,
                                            width,
                                            height,
                                        ) {
                                            file_ops::copy_png_to_clipboard(
                                                &result.rgba_data,
                                                result.width,
                                                result.height,
                                            );
                                        }
                                    }

                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        spawn_png_export_async(
                                            device,
                                            queue,
                                            scene,
                                            width,
                                            height,
                                            "selection.png".to_string(),
                                            true,
                                            None, // No metadata for clipboard copy
                                        );
                                    }
                                } else {
                                    log::info!("Nothing to copy - selection is empty");
                                }
                            }
                            _ => {}
                        }
                    }
                }

                // Build Vello scene
                let viewport_size = Size::new(
                    state.canvas.viewport_size.width,
                    state.canvas.viewport_size.height,
                );
                // Get selection rectangle if active
                let selection_rect = state.event_handler.selection_rect().map(|sr| sr.to_rect());

                // Get snap point for guides
                let snap_point = state.event_handler.last_snap.as_ref().map(|s| s.point);

                // Get angle snap info for visualization
                let angle_snap_info = state
                    .event_handler
                    .last_angle_snap
                    .as_ref()
                    .filter(|_| state.ui_state.angle_snap_enabled)
                    .map(|angle_snap| AngleSnapInfo {
                        start_point: state
                            .event_handler
                            .line_start_point
                            .unwrap_or(kurbo::Point::ZERO),
                        end_point: angle_snap.point,
                        angle_degrees: angle_snap.angle_degrees,
                        is_snapped: angle_snap.snapped,
                    });

                // Update and get snap targets for nearby shape indicators
                state
                    .event_handler
                    .update_snap_targets(&state.canvas, state.ui_state.snap_mode);
                let snap_targets = state.event_handler.current_snap_targets.clone();

                // Get rotation info for helper lines
                let rotation_info = state.event_handler.rotation_state.as_ref().map(|rs| {
                    drafftink_render::RotationInfo {
                        center: rs.center,
                        angle: rs.angle,
                        snapped: rs.snapped,
                    }
                });

                // Get eraser cursor info
                let eraser_cursor = if state.canvas.tool_manager.current_tool == ToolKind::Eraser {
                    state
                        .event_handler
                        .eraser_path()
                        .last()
                        .map(|p| (*p, state.event_handler.eraser_radius))
                } else {
                    None
                };

                // Get laser pointer info
                let laser_pointer =
                    if state.canvas.tool_manager.current_tool == ToolKind::LaserPointer {
                        state
                            .event_handler
                            .laser_position
                            .map(|pos| (pos, state.event_handler.laser_trail.clone()))
                    } else {
                        None
                    };

                let render_ctx = RenderContext::new(&state.canvas, viewport_size)
                    .with_scale_factor(state.window.scale_factor())
                    .with_background(state.config.background_color)
                    .with_grid(state.config.grid_style)
                    .with_selection_rect(selection_rect)
                    .with_editing_shape(state.event_handler.editing_text)
                    .with_snap_point(snap_point)
                    .with_angle_snap(angle_snap_info)
                    .with_snap_targets(snap_targets)
                    .with_rotation_info(rotation_info)
                    .with_eraser_cursor(eraser_cursor)
                    .with_laser_pointer(laser_pointer);

                state.shape_renderer.build_scene(&render_ctx);

                // Render text in edit mode (with cursor and selection)
                if let Some(text_id) = state.event_handler.editing_text {
                    if let Some(Shape::Text(text)) = state.canvas.document.get_shape(text_id) {
                        let camera_transform = state.canvas.camera.transform();

                        // Ensure edit state exists
                        if state.text_edit_state.is_none() {
                            let mut edit_state =
                                TextEditState::new(&text.content, text.font_size as f32);
                            edit_state.cursor_reset();
                            state.text_edit_state = Some(edit_state);
                        }

                        // Update cursor blinking
                        if let Some(edit_state) = &mut state.text_edit_state {
                            edit_state.cursor_blink();
                            state.shape_renderer.render_text_editing(
                                text,
                                edit_state,
                                camera_transform,
                                state.event_handler.text_edit_anchor,
                            );
                        }
                    }
                }

                // Render remote peer cursors
                {
                    let camera = &state.canvas.camera;
                    for peer in state.remote_peers.values() {
                        if let Some(ref cursor) = peer.awareness.cursor {
                            let world_pos = Point::new(cursor.x, cursor.y);
                            let screen_pos = camera.world_to_screen(world_pos);

                            // Get peer color (or use a default)
                            let color = peer
                                .awareness
                                .user
                                .as_ref()
                                .and_then(|u| parse_color(&u.color))
                                .unwrap_or(Color::from_rgba8(59, 130, 246, 255)); // Default blue

                            // Get peer name (or use "Anonymous")
                            // Draw cursor pointer
                            state.shape_renderer.draw_cursor(screen_pos, color);
                        }
                    }
                }

                let scene = state.shape_renderer.take_scene();

                // Render
                let Some(render_cx) = self.render_cx.as_ref() else {
                    return;
                };

                let device_handle = &render_cx.devices[state.surface.dev_id];
                let device = &device_handle.device;
                let queue = &device_handle.queue;

                let surface_texture = match state.surface.surface.get_current_texture() {
                    Ok(t) => t,
                    Err(e) => {
                        log::warn!("Failed to get surface texture: {:?}", e);
                        return;
                    }
                };

                let width = state.surface.config.width;
                let height = state.surface.config.height;

                let params = RenderParams {
                    base_color: state.config.background_color,
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                };

                // Create an intermediate texture with StorageBinding usage for Vello.
                // IMPORTANT: Must use Rgba8Unorm format because:
                // 1. Vello's compute shaders require StorageBinding usage
                // 2. WebGPU only supports StorageBinding for Rgba8Unorm (not Bgra8Unorm)
                // 3. We copy to the surface texture afterward (which may be Bgra8Unorm)
                let render_texture = device.create_texture(&vello::wgpu::TextureDescriptor {
                    label: Some("vello render texture"),
                    size: vello::wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: vello::wgpu::TextureDimension::D2,
                    format: vello::wgpu::TextureFormat::Rgba8Unorm,
                    usage: vello::wgpu::TextureUsages::STORAGE_BINDING
                        | vello::wgpu::TextureUsages::COPY_SRC
                        | vello::wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });

                let render_texture_view =
                    render_texture.create_view(&vello::wgpu::TextureViewDescriptor::default());

                // Render Vello to the intermediate texture
                if let Err(e) = state.vello_renderer.render_to_texture(
                    device,
                    queue,
                    &scene,
                    &render_texture_view,
                    &params,
                ) {
                    log::error!("Failed to render: {:?}", e);
                    return;
                }

                let surface_view = surface_texture
                    .texture
                    .create_view(&vello::wgpu::TextureViewDescriptor::default());

                // Blit the RGBA intermediate texture to the surface texture (which may be BGRA)
                {
                    let mut blit_encoder =
                        device.create_command_encoder(&vello::wgpu::CommandEncoderDescriptor {
                            label: Some("blit encoder"),
                        });

                    state.texture_blitter.copy(
                        device,
                        &mut blit_encoder,
                        &render_texture_view,
                        &surface_view,
                    );

                    queue.submit(std::iter::once(blit_encoder.finish()));
                }

                // Update egui textures
                for (id, image_delta) in &egui_output.textures_delta.set {
                    state
                        .egui_renderer
                        .update_texture(device, queue, *id, image_delta);
                }

                // Render egui on top
                let screen_descriptor = egui_wgpu::ScreenDescriptor {
                    size_in_pixels: [width, height],
                    pixels_per_point: egui_output.pixels_per_point,
                };

                {
                    let mut egui_encoder =
                        device.create_command_encoder(&vello::wgpu::CommandEncoderDescriptor {
                            label: Some("egui encoder"),
                        });

                    state.egui_renderer.update_buffers(
                        device,
                        queue,
                        &mut egui_encoder,
                        &egui_primitives,
                        &screen_descriptor,
                    );

                    let render_pass =
                        egui_encoder.begin_render_pass(&vello::wgpu::RenderPassDescriptor {
                            label: Some("egui render pass"),
                            color_attachments: &[Some(vello::wgpu::RenderPassColorAttachment {
                                view: &surface_view,
                                resolve_target: None,
                                ops: vello::wgpu::Operations {
                                    load: vello::wgpu::LoadOp::Load, // Keep Vello content
                                    store: vello::wgpu::StoreOp::Store,
                                },
                                depth_slice: None,
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });

                    // Use forget_lifetime to satisfy egui-wgpu's 'static requirement
                    let mut render_pass = render_pass.forget_lifetime();
                    state.egui_renderer.render(
                        &mut render_pass,
                        &egui_primitives,
                        &screen_descriptor,
                    );
                    drop(render_pass);

                    queue.submit(std::iter::once(egui_encoder.finish()));
                }

                // Free egui textures
                for id in &egui_output.textures_delta.free {
                    state.egui_renderer.free_texture(id);
                }
                surface_texture.present();
                state.window.request_redraw();
            }

            WindowEvent::CursorMoved { position, .. } => {
                let point = Point::new(position.x, position.y);

                // Skip canvas processing if egui wants the pointer
                if egui_wants_input {
                    state.window.set_cursor(CursorIcon::Default);
                    return;
                }

                let world_point = state.canvas.camera.screen_to_world(point);

                // Update cursor based on hover position (only when not dragging)
                if !state.input.is_button_pressed(MouseButton::Left) {
                    use drafftink_core::selection::{Corner, HandleKind};
                    let cursor = match state
                        .event_handler
                        .get_cursor_for_position(&state.canvas, world_point)
                    {
                        Some(Some(HandleKind::Corner(Corner::TopLeft | Corner::BottomRight))) => {
                            CursorIcon::NwseResize
                        }
                        Some(Some(HandleKind::Corner(Corner::TopRight | Corner::BottomLeft))) => {
                            CursorIcon::NeswResize
                        }
                        Some(Some(HandleKind::Edge(_))) => CursorIcon::EwResize,
                        Some(Some(
                            HandleKind::Endpoint(_)
                            | HandleKind::IntermediatePoint(_)
                            | HandleKind::SegmentMidpoint(_),
                        )) => CursorIcon::Crosshair,
                        Some(Some(HandleKind::Rotate)) => CursorIcon::Grab,
                        Some(None) => CursorIcon::Move,
                        None => CursorIcon::Default,
                    };
                    state.window.set_cursor(cursor);
                }

                // Broadcast cursor position to collaborators (throttled)
                if state.collab.is_in_room() {
                    // Simple throttling: only send every ~100ms (every 6 frames at 60fps)
                    static mut CURSOR_FRAME: u32 = 0;
                    unsafe {
                        CURSOR_FRAME = CURSOR_FRAME.wrapping_add(1);
                        if CURSOR_FRAME % 6 == 0 {
                            state.collab.set_cursor(world_point.x, world_point.y);
                            if let Some(ref ws) = state.websocket {
                                for msg in state.collab.take_outgoing() {
                                    let _ = ws.send(&msg);
                                }
                            }
                        }
                    }
                }

                // Handle dragging (manipulation or shape drawing)
                if state.input.is_button_pressed(MouseButton::Left) {
                    // Handle text selection dragging first
                    if let Some(text_id) = state.event_handler.editing_text {
                        if let Some(Shape::Text(text)) = state.canvas.document.get_shape(text_id) {
                            // Convert drag position to text-local coordinates
                            let local_x = (world_point.x - text.position.x) as f32;
                            let local_y = (world_point.y - text.position.y) as f32;

                            // Extend selection during drag using new API
                            if let Some(edit_state) = &mut state.text_edit_state {
                                let (font_cx, layout_cx) = state.shape_renderer.contexts_mut();
                                edit_state.handle_mouse_drag(local_x, local_y, font_cx, layout_cx);
                            }
                        }
                    } else if state.event_handler.is_manipulating() {
                        // Check if we're manipulating a shape (handle drag or move)
                        state.event_handler.handle_drag(
                            &mut state.canvas,
                            world_point,
                            &state.input,
                            state.ui_state.snap_mode,
                            state.ui_state.angle_snap_enabled,
                        );

                        // Sync during manipulation (throttled)
                        {
                            static mut DRAG_SYNC_FRAME: u32 = 0;
                            unsafe {
                                DRAG_SYNC_FRAME = DRAG_SYNC_FRAME.wrapping_add(1);
                                // Sync every 10 frames (~6 times per second at 60fps)
                                if DRAG_SYNC_FRAME % 10 == 0 && state.collab.is_in_room() {
                                    state.collab.sync_to_crdt(&state.canvas.document);
                                    state.collab.broadcast_sync();
                                    if let Some(ref ws) = state.websocket {
                                        for msg in state.collab.take_outgoing() {
                                            let _ = ws.send(&msg);
                                        }
                                    }
                                }
                            }
                        }
                    } else if state.event_handler.is_selecting() {
                        // Marquee selection in progress
                        state.event_handler.handle_drag(
                            &mut state.canvas,
                            world_point,
                            &state.input,
                            state.ui_state.snap_mode,
                            state.ui_state.angle_snap_enabled,
                        );
                    } else if state.canvas.tool_manager.current_tool == ToolKind::Pan {
                        // Pan with left mouse + pan tool
                        let delta = state.input.cursor_diff();
                        state.canvas.camera.pan(delta);
                    } else if matches!(
                        state.canvas.tool_manager.current_tool,
                        ToolKind::Eraser | ToolKind::LaserPointer
                    ) {
                        // Eraser and laser pointer handle their own drag
                        state.event_handler.handle_drag(
                            &mut state.canvas,
                            world_point,
                            &state.input,
                            state.ui_state.snap_mode,
                            state.ui_state.angle_snap_enabled,
                        );
                    } else if state.canvas.tool_manager.is_active() {
                        // Drawing a new shape or freehand
                        state.event_handler.handle_drag(
                            &mut state.canvas,
                            world_point,
                            &state.input,
                            state.ui_state.snap_mode,
                            state.ui_state.angle_snap_enabled,
                        );

                        // Note: We don't sync preview shapes during drawing
                        // They only get synced when finalized on mouse release
                    }
                }

                // Middle mouse button always pans
                if state.input.is_button_pressed(MouseButton::Middle) {
                    let delta = state.input.cursor_diff();
                    state.canvas.camera.pan(delta);
                }
            }

            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                // Skip canvas processing if egui wants the pointer
                if egui_wants_input {
                    return;
                }

                let mouse_btn = match button {
                    MouseButton::Left => MouseButton::Left,
                    MouseButton::Right => MouseButton::Right,
                    MouseButton::Middle => MouseButton::Middle,
                    _ => return,
                };

                let position = state.input.mouse_position();

                match btn_state {
                    ElementState::Pressed => {
                        if mouse_btn == MouseButton::Left {
                            let world_point = state.canvas.camera.screen_to_world(position);

                            // Handle text editing cursor positioning
                            if let Some(text_id) = state.event_handler.editing_text {
                                // Check if click is still on the text being edited
                                let hits = state
                                    .canvas
                                    .document
                                    .shapes_at_point(world_point, 5.0 / state.canvas.camera.zoom);
                                let clicked_on_editing =
                                    hits.first().map(|&id| id == text_id).unwrap_or(false);

                                if clicked_on_editing {
                                    if let Some(Shape::Text(text)) =
                                        state.canvas.document.get_shape(text_id)
                                    {
                                        // Convert click to text-local coordinates
                                        let local_x = (world_point.x - text.position.x) as f32;
                                        let local_y = (world_point.y - text.position.y) as f32;

                                        // Ensure edit state exists
                                        if state.text_edit_state.is_none() {
                                            let mut edit_state = TextEditState::new(
                                                &text.content,
                                                text.font_size as f32,
                                            );
                                            edit_state.cursor_reset();
                                            state.text_edit_state = Some(edit_state);
                                        }

                                        // Position cursor at click location using new API
                                        if let Some(edit_state) = &mut state.text_edit_state {
                                            let (font_cx, layout_cx) =
                                                state.shape_renderer.contexts_mut();
                                            edit_state.handle_mouse_down(
                                                local_x,
                                                local_y,
                                                state.input.shift(),
                                                font_cx,
                                                layout_cx,
                                            );
                                        }
                                    }
                                    // Handled click on editing text
                                } else {
                                    // Clicked outside editing text - exit edit mode
                                    state.event_handler.exit_text_edit(&mut state.canvas);
                                    state.text_edit_state = None;
                                    // Continue with normal press handling
                                    state.event_handler.handle_press(
                                        &mut state.canvas,
                                        world_point,
                                        &state.input,
                                        state.ui_state.snap_mode,
                                    );
                                }
                            } else {
                                state.event_handler.handle_press(
                                    &mut state.canvas,
                                    world_point,
                                    &state.input,
                                    state.ui_state.snap_mode,
                                );

                                // Check if we just entered text edit mode
                                if let Some(text_id) = state.event_handler.editing_text {
                                    if let Some(Shape::Text(text)) =
                                        state.canvas.document.get_shape(text_id)
                                    {
                                        let mut edit_state = TextEditState::new(
                                            &text.content,
                                            text.font_size as f32,
                                        );
                                        edit_state.cursor_reset();
                                        // Move cursor to end of text
                                        let (font_cx, layout_cx) =
                                            state.shape_renderer.contexts_mut();
                                        let mut drv = edit_state.driver(font_cx, layout_cx);
                                        drv.move_to_text_end();
                                        state.text_edit_state = Some(edit_state);
                                        log::info!(
                                            "Entered text edit mode for shape {:?}, content: '{}'",
                                            text_id,
                                            text.content
                                        );
                                    }
                                }

                                // Check if we need to open math editor
                                if let Some(math_id) = state.event_handler.pending_math_edit.take()
                                {
                                    if let Some(Shape::Math(math)) =
                                        state.canvas.document.get_shape(math_id)
                                    {
                                        state.ui_state.math_editor =
                                            Some((math_id, math.latex.clone()));
                                        log::info!("Opening math editor for shape {:?}", math_id);
                                    }
                                }
                            }
                        }
                    }
                    ElementState::Released => {
                        if mouse_btn == MouseButton::Left {
                            // Handle text editing mouse release
                            if let Some(edit_state) = &mut state.text_edit_state {
                                edit_state.handle_mouse_up();
                            }

                            let world_point = state.canvas.camera.screen_to_world(position);
                            let current_style = state.ui_state.to_shape_style();
                            state.event_handler.handle_release(
                                &mut state.canvas,
                                world_point,
                                &state.input,
                                &current_style,
                                state.ui_state.snap_mode,
                                state.ui_state.angle_snap_enabled,
                            );

                            // Broadcast document changes to collaborators
                            if state.collab.is_in_room() {
                                state.collab.sync_to_crdt(&state.canvas.document);
                                state.collab.broadcast_sync();
                                if let Some(ref ws) = state.websocket {
                                    for msg in state.collab.take_outgoing() {
                                        let _ = ws.send(&msg);
                                    }
                                }
                            }

                            // Clear snap guides when done dragging
                            state.event_handler.clear_snap();

                            // Check if we just entered text edit mode (for new text created on release)
                            if state.text_edit_state.is_none() {
                                if let Some(text_id) = state.event_handler.editing_text {
                                    if let Some(Shape::Text(text)) =
                                        state.canvas.document.get_shape(text_id)
                                    {
                                        let mut edit_state = TextEditState::new(
                                            &text.content,
                                            text.font_size as f32,
                                        );
                                        edit_state.cursor_reset();
                                        state.text_edit_state = Some(edit_state);
                                        log::info!(
                                            "Entered text edit mode for new text shape {:?}",
                                            text_id
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                // Skip canvas processing if egui wants the pointer
                if egui_wants_input {
                    return;
                }

                let scroll = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        Vec2::new(x as f64 * 20.0, y as f64 * 20.0)
                    }
                    MouseScrollDelta::PixelDelta(pos) => Vec2::new(pos.x, pos.y),
                };

                let position = state.input.mouse_position();

                if state.input.ctrl() {
                    // Ctrl/Cmd + scroll = zoom
                    let zoom_factor = if scroll.y > 0.0 { 1.1 } else { 0.9 };
                    state.canvas.camera.zoom_at(position, zoom_factor);
                } else {
                    // Normal scroll/trackpad = pan
                    state.canvas.camera.pan(scroll);
                }
            }

            WindowEvent::Touch(touch) => {
                // Skip if egui wants input
                if egui_wants_input {
                    return;
                }

                use winit::event::TouchPhase;
                let pos = Point::new(touch.location.x, touch.location.y);
                let world_point = state.canvas.camera.screen_to_world(pos);

                // Track previous position for pan delta
                let prev_pos = state.input.primary_touch();

                // Process touch and get gesture result
                let gesture = state.input.process_touch(&touch);
                let touch_count = state.input.touch_count();

                if let Some((pan_delta, zoom_delta, zoom_center)) = gesture {
                    // Two-finger gesture: pinch-zoom and pan
                    if (zoom_delta - 1.0).abs() > 0.001 {
                        state.canvas.camera.zoom_at(zoom_center, zoom_delta);
                    }
                    if pan_delta.length() > 0.1 {
                        state.canvas.camera.pan(pan_delta);
                    }
                } else if touch_count <= 1 {
                    // Single finger behavior depends on tool
                    let is_pan_tool = state.canvas.tool_manager.current_tool == ToolKind::Pan;

                    if is_pan_tool {
                        // Pan tool: single finger pans
                        if touch.phase == TouchPhase::Moved {
                            if let Some(prev) = prev_pos {
                                let delta = Vec2::new(pos.x - prev.x, pos.y - prev.y);
                                state.canvas.camera.pan(delta);
                            }
                        }
                    } else {
                        // Other tools: single finger draws/selects
                        match touch.phase {
                            TouchPhase::Started => {
                                state.event_handler.handle_press(
                                    &mut state.canvas,
                                    world_point,
                                    &state.input,
                                    state.ui_state.snap_mode,
                                );
                            }
                            TouchPhase::Moved => {
                                state.event_handler.handle_drag(
                                    &mut state.canvas,
                                    world_point,
                                    &state.input,
                                    state.ui_state.snap_mode,
                                    state.ui_state.angle_snap_enabled,
                                );
                            }
                            TouchPhase::Ended => {
                                let current_style = state.ui_state.to_shape_style();
                                state.event_handler.handle_release(
                                    &mut state.canvas,
                                    world_point,
                                    &state.input,
                                    &current_style,
                                    state.ui_state.snap_mode,
                                    state.ui_state.angle_snap_enabled,
                                );
                            }
                            TouchPhase::Cancelled => {
                                state.event_handler.cancel(&mut state.canvas);
                            }
                        }
                    }
                }
                state.window.request_redraw();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                // Skip canvas processing if egui wants keyboard
                if egui_wants_input {
                    return;
                }

                // Handle text editing with dedicated handler
                if let Some(text_id) = state.event_handler.editing_text {
                    if event.state == ElementState::Pressed {
                        // Initialize edit state if needed
                        if state.text_edit_state.is_none() {
                            if let Some(Shape::Text(text)) =
                                state.canvas.document.get_shape(text_id)
                            {
                                let mut edit_state =
                                    TextEditState::new(&text.content, text.font_size as f32);
                                edit_state.cursor_reset();
                                state.text_edit_state = Some(edit_state);
                            }
                        }

                        // Check for copy/paste shortcuts first
                        let has_ctrl = state.input.ctrl();
                        let text_key = if has_ctrl {
                            match &event.logical_key {
                                Key::Character(c) if c == "c" || c == "C" => Some(TextKey::Copy),
                                Key::Character(c) if c == "x" || c == "X" => Some(TextKey::Cut),
                                Key::Character(c) if c == "v" || c == "V" => {
                                    // Get text from clipboard
                                    #[cfg(not(target_arch = "wasm32"))]
                                    {
                                        arboard::Clipboard::new()
                                            .ok()
                                            .and_then(|mut cb| cb.get_text().ok())
                                            .map(TextKey::Paste)
                                    }
                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        file_ops::request_clipboard_text();
                                        return; // Don't process "v" - paste will happen on next frame
                                    }
                                }
                                // Let Ctrl+A through for select all (handled in text_editor)
                                Key::Character(c) if c == "a" || c == "A" => None,
                                // Ignore other Ctrl+key combos in text edit (don't type the letter)
                                Key::Character(_) => return,
                                _ => None,
                            }
                        } else {
                            None
                        };

                        // Convert winit key to TextKey (if not already a clipboard operation)
                        let text_key = text_key.or_else(|| match &event.logical_key {
                            Key::Named(NamedKey::Escape) => Some(TextKey::Escape),
                            Key::Named(NamedKey::Backspace) => Some(TextKey::Backspace),
                            Key::Named(NamedKey::Delete) => Some(TextKey::Delete),
                            Key::Named(NamedKey::Enter) => Some(TextKey::Enter),
                            Key::Named(NamedKey::ArrowLeft) => Some(TextKey::Left),
                            Key::Named(NamedKey::ArrowRight) => Some(TextKey::Right),
                            Key::Named(NamedKey::ArrowUp) => Some(TextKey::Up),
                            Key::Named(NamedKey::ArrowDown) => Some(TextKey::Down),
                            Key::Named(NamedKey::Home) => Some(TextKey::Home),
                            Key::Named(NamedKey::End) => Some(TextKey::End),
                            Key::Named(NamedKey::Space) => {
                                Some(TextKey::Character(" ".to_string()))
                            }
                            Key::Character(c) => Some(TextKey::Character(c.to_string())),
                            _ => None,
                        });

                        if let Some(key) = text_key {
                            log::debug!("Text edit key: {:?}", key);
                            let modifiers = TextModifiers {
                                shift: state.input.shift(),
                                ctrl: state.input.ctrl(),
                                alt: state.input.alt(),
                                meta: false, // winit_input_helper doesn't track meta separately
                            };

                            let (font_cx, layout_cx) = state.shape_renderer.contexts_mut();

                            if let Some(edit_state) = &mut state.text_edit_state {
                                // Capture state before edit for color sync
                                let old_text = edit_state.text();
                                let old_char_count = old_text.chars().count();
                                let cursor_byte = edit_state.cursor_byte_offset();
                                let edit_char_pos =
                                    old_text[..cursor_byte.min(old_text.len())].chars().count();

                                let result =
                                    edit_state.handle_key(key, modifiers, font_cx, layout_cx);
                                log::debug!(
                                    "Text edit result: {:?}, text now: '{}'",
                                    result,
                                    edit_state.text()
                                );

                                match result {
                                    TextEditResult::ExitEdit => {
                                        // Sync content before exiting
                                        let new_text = edit_state.text();
                                        if let Some(Shape::Text(text)) =
                                            state.canvas.document.get_shape_mut(text_id)
                                        {
                                            text.content = new_text;
                                            text.sync_char_colors_after_edit(
                                                edit_char_pos,
                                                old_char_count,
                                            );
                                        }
                                        state.event_handler.exit_text_edit(&mut state.canvas);
                                        state.text_edit_state = None;
                                    }
                                    TextEditResult::Handled => {
                                        // Sync content back to the text shape
                                        let new_text = edit_state.text();
                                        if let Some(Shape::Text(text)) =
                                            state.canvas.document.get_shape_mut(text_id)
                                        {
                                            text.content = new_text;
                                            text.sync_char_colors_after_edit(
                                                edit_char_pos,
                                                old_char_count,
                                            );
                                        }
                                    }
                                    TextEditResult::Copy(text_to_copy) => {
                                        // Copy text to clipboard
                                        #[cfg(not(target_arch = "wasm32"))]
                                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                            let _ = clipboard.set_text(&text_to_copy);
                                        }
                                        #[cfg(target_arch = "wasm32")]
                                        file_ops::copy_text_to_clipboard(&text_to_copy);
                                        // Sync content back (for cut operation)
                                        let new_text = edit_state.text();
                                        if let Some(Shape::Text(text)) =
                                            state.canvas.document.get_shape_mut(text_id)
                                        {
                                            text.content = new_text;
                                            text.sync_char_colors_after_edit(
                                                edit_char_pos,
                                                old_char_count,
                                            );
                                        }
                                    }
                                    TextEditResult::NotHandled => {}
                                }
                            }
                        } else {
                            log::debug!("Text edit: unhandled key {:?}", event.logical_key);
                        }
                    }
                    return;
                }

                // Regular keyboard handling (not text editing)
                let key_str = match &event.logical_key {
                    Key::Named(named) => match named {
                        NamedKey::Escape => "Escape",
                        NamedKey::Delete => "Delete",
                        NamedKey::Backspace => "Backspace",
                        NamedKey::ArrowUp => "ArrowUp",
                        NamedKey::ArrowDown => "ArrowDown",
                        NamedKey::ArrowLeft => "ArrowLeft",
                        NamedKey::ArrowRight => "ArrowRight",
                        _ => return,
                    },
                    Key::Character(c) => c.as_str(),
                    _ => return,
                };

                match event.state {
                    ElementState::Pressed => {
                        // Check for Ctrl/Cmd modifiers first for file operations
                        let has_modifier = state.input.ctrl();

                        if has_modifier {
                            let has_shift = state.input.shift();
                            match key_str {
                                "a" | "A" => {
                                    state.canvas.select_all();
                                    log::info!(
                                        "Selected all {} shapes",
                                        state.canvas.selection.len()
                                    );
                                }
                                "s" | "S" => {
                                    file_ops::save_document(
                                        &state.canvas.document,
                                        &state.canvas.document.name,
                                    );
                                }
                                "o" | "O" => {
                                    #[cfg(not(target_arch = "wasm32"))]
                                    {
                                        file_ops::load_document();
                                    }
                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        file_ops::load_document_async();
                                    }
                                }
                                // Ctrl+E = Export PNG
                                "e" | "E" => {
                                    if let Some(render_cx) = self.render_cx.as_ref() {
                                        let device_handle =
                                            &render_cx.devices[state.surface.dev_id];
                                        let device = &device_handle.device;
                                        let queue = &device_handle.queue;
                                        let export_scale = state.ui_state.export_scale as f64;

                                        let (scene, bounds) =
                                            state.shape_renderer.build_export_scene(
                                                &state.canvas.document,
                                                export_scale,
                                            );
                                        if let Some(bounds) = bounds {
                                            let width = bounds.width().ceil() as u32;
                                            let height = bounds.height().ceil() as u32;

                                            log::info!(
                                                "Exporting PNG at {}x scale: {}x{}",
                                                state.ui_state.export_scale,
                                                width,
                                                height
                                            );

                                            #[cfg(not(target_arch = "wasm32"))]
                                            {
                                                if let Some(result) = render_scene_to_png(
                                                    device,
                                                    queue,
                                                    &mut state.vello_renderer,
                                                    &scene,
                                                    width,
                                                    height,
                                                ) {
                                                    let scene_json = state.canvas.document.to_json().ok();
                                                    if let Some(png_data) = encode_png(
                                                        &result.rgba_data,
                                                        result.width,
                                                        result.height,
                                                        scene_json.as_deref(),
                                                    ) {
                                                        file_ops::export_png(
                                                            &png_data,
                                                            &state.canvas.document.name,
                                                        );
                                                    }
                                                }
                                            }

                                            #[cfg(target_arch = "wasm32")]
                                            {
                                                let filename =
                                                    format!("{}.png", state.canvas.document.name);
                                                let scene_json = state.canvas.document.to_json().ok();
                                                spawn_png_export_async(
                                                    device, queue, scene, width, height, filename,
                                                    false, scene_json,
                                                );
                                            }
                                        } else {
                                            log::info!("Nothing to export - document is empty");
                                        }
                                    }
                                }
                                // Ctrl+Z = Undo, Ctrl+Shift+Z = Redo
                                "z" | "Z" => {
                                    if has_shift {
                                        // Redo
                                        if state.canvas.document.redo() {
                                            state.canvas.clear_selection();
                                            log::info!("Redo performed");
                                        } else {
                                            log::info!("Nothing to redo");
                                        }
                                    } else {
                                        // Undo
                                        if state.canvas.document.undo() {
                                            state.canvas.clear_selection();
                                            log::info!("Undo performed");
                                        } else {
                                            log::info!("Nothing to undo");
                                        }
                                    }
                                }
                                // Ctrl+Y = Redo (alternative)
                                "y" | "Y" => {
                                    if state.canvas.document.redo() {
                                        state.canvas.clear_selection();
                                        log::info!("Redo performed");
                                    } else {
                                        log::info!("Nothing to redo");
                                    }
                                }
                                // Ctrl+G = Group, Ctrl+Shift+G = Ungroup
                                "g" | "G" => {
                                    if has_shift {
                                        // Ungroup
                                        let ungrouped = state.canvas.ungroup_selected();
                                        if !ungrouped.is_empty() {
                                            log::info!("Ungrouped {} shapes", ungrouped.len());
                                        } else {
                                            log::info!("No groups to ungroup");
                                        }
                                    } else {
                                        // Group
                                        if let Some(group_id) = state.canvas.group_selected() {
                                            log::info!("Grouped shapes into {:?}", group_id);
                                        } else {
                                            log::info!("Select at least 2 shapes to group");
                                        }
                                    }
                                }
                                // Ctrl+Shift+C = Copy PNG
                                "c" | "C" if has_shift => {
                                    if let Some(render_cx) = self.render_cx.as_ref() {
                                        let device_handle =
                                            &render_cx.devices[state.surface.dev_id];
                                        let device = &device_handle.device;
                                        let queue = &device_handle.queue;
                                        let export_scale = state.ui_state.export_scale as f64;

                                        let (scene, bounds) = if state.canvas.selection.is_empty() {
                                            state.shape_renderer.build_export_scene(
                                                &state.canvas.document,
                                                export_scale,
                                            )
                                        } else {
                                            state.shape_renderer.build_export_scene_selection(
                                                &state.canvas.document,
                                                &state.canvas.selection,
                                                export_scale,
                                            )
                                        };

                                        if let Some(bounds) = bounds {
                                            let width = bounds.width().ceil() as u32;
                                            let height = bounds.height().ceil() as u32;

                                            log::info!(
                                                "Copying PNG at {}x scale: {}x{}",
                                                state.ui_state.export_scale,
                                                width,
                                                height
                                            );

                                            #[cfg(not(target_arch = "wasm32"))]
                                            {
                                                if let Some(result) = render_scene_to_png(
                                                    device,
                                                    queue,
                                                    &mut state.vello_renderer,
                                                    &scene,
                                                    width,
                                                    height,
                                                ) {
                                                    file_ops::copy_png_to_clipboard(
                                                        &result.rgba_data,
                                                        result.width,
                                                        result.height,
                                                    );
                                                }
                                            }

                                            #[cfg(target_arch = "wasm32")]
                                            {
                                                spawn_png_export_async(
                                                    device,
                                                    queue,
                                                    scene,
                                                    width,
                                                    height,
                                                    "selection.png".to_string(),
                                                    true,
                                                    None, // No metadata for clipboard copy
                                                );
                                            }
                                        } else {
                                            log::info!("Nothing to copy - selection is empty");
                                        }
                                    }
                                }
                                // Ctrl+C = Copy shapes (without shift)
                                "c" | "C" => {
                                    if !state.canvas.selection.is_empty() {
                                        let shapes: Vec<Shape> = state
                                            .canvas
                                            .selection
                                            .iter()
                                            .filter_map(|&id| {
                                                state.canvas.document.get_shape(id).cloned()
                                            })
                                            .collect();
                                        if let Ok(json) = serde_json::to_string(&shapes) {
                                            state.ui_state.clipboard_shapes = Some(json);
                                            log::info!("Copied {} shapes", shapes.len());
                                        }
                                    }
                                }
                                // Ctrl+X = Cut shapes
                                "x" | "X" => {
                                    if !state.canvas.selection.is_empty() {
                                        let shapes: Vec<Shape> = state
                                            .canvas
                                            .selection
                                            .iter()
                                            .filter_map(|&id| {
                                                state.canvas.document.get_shape(id).cloned()
                                            })
                                            .collect();
                                        if let Ok(json) = serde_json::to_string(&shapes) {
                                            state.ui_state.clipboard_shapes = Some(json);
                                            log::info!("Cut {} shapes", shapes.len());
                                            state.canvas.document.push_undo();
                                            for &id in &state.canvas.selection.clone() {
                                                state.canvas.document.remove_shape(id);
                                                if state.collab.is_in_room() {
                                                    let _ = state
                                                        .collab
                                                        .crdt_mut()
                                                        .remove_shape(&id.to_string());
                                                }
                                            }
                                            state.canvas.clear_selection();
                                        }
                                    }
                                }
                                // Ctrl+V = Paste shapes or image
                                "v" | "V" => {
                                    // First try to paste shapes from internal clipboard
                                    let mut pasted = false;
                                    if let Some(json) = &state.ui_state.clipboard_shapes.clone() {
                                        if let Ok(shapes) = serde_json::from_str::<Vec<Shape>>(json)
                                        {
                                            state.canvas.document.push_undo();
                                            state.canvas.clear_selection();
                                            for shape in shapes {
                                                let mut new_shape = shape.clone();
                                                new_shape.regenerate_id();
                                                new_shape.transform(kurbo::Affine::translate(
                                                    kurbo::Vec2::new(20.0, 20.0),
                                                ));
                                                let new_id = new_shape.id();
                                                state.canvas.document.add_shape(new_shape.clone());
                                                state.canvas.add_to_selection(new_id);
                                                if state.collab.is_in_room() {
                                                    let _ = state
                                                        .collab
                                                        .crdt_mut()
                                                        .add_shape(&new_shape);
                                                }
                                            }
                                            log::info!("Pasted shapes");
                                            pasted = true;
                                        }
                                    }

                                    // If no shapes, try to paste image from system clipboard
                                    #[cfg(not(target_arch = "wasm32"))]
                                    if !pasted {
                                        if let Some(image_shape) =
                                            file_ops::paste_image_from_clipboard(&state.canvas)
                                        {
                                            state.canvas.document.push_undo();
                                            state.canvas.clear_selection();
                                            let new_id = image_shape.id();
                                            state.canvas.document.add_shape(image_shape.clone());
                                            state.canvas.add_to_selection(new_id);
                                            if state.collab.is_in_room() {
                                                let _ =
                                                    state.collab.crdt_mut().add_shape(&image_shape);
                                            }
                                            log::info!("Pasted image from clipboard");
                                        }
                                    }

                                    // WASM: Try async clipboard paste
                                    #[cfg(target_arch = "wasm32")]
                                    if !pasted {
                                        let vw = state.canvas.viewport_size.width;
                                        let vh = state.canvas.viewport_size.height;
                                        let cox = state.canvas.camera.offset.x;
                                        let coy = state.canvas.camera.offset.y;
                                        let cz = state.canvas.camera.zoom;
                                        file_ops::paste_image_from_clipboard_async(
                                            vw, vh, cox, coy, cz,
                                        );
                                    }
                                }
                                // Ctrl+D = Duplicate shapes
                                "d" | "D" => {
                                    if !state.canvas.selection.is_empty() {
                                        state.canvas.document.push_undo();
                                        let mut new_selection = Vec::new();
                                        for &id in &state.canvas.selection.clone() {
                                            if let Some(shape) = state.canvas.document.get_shape(id)
                                            {
                                                let mut new_shape = shape.clone();
                                                new_shape.regenerate_id();
                                                new_shape.transform(kurbo::Affine::translate(
                                                    kurbo::Vec2::new(20.0, 20.0),
                                                ));
                                                let new_id = new_shape.id();
                                                state.canvas.document.add_shape(new_shape.clone());
                                                new_selection.push(new_id);
                                                if state.collab.is_in_room() {
                                                    let _ = state
                                                        .collab
                                                        .crdt_mut()
                                                        .add_shape(&new_shape);
                                                }
                                            }
                                        }
                                        state.canvas.clear_selection();
                                        for id in new_selection {
                                            state.canvas.add_to_selection(id);
                                        }
                                        log::info!("Duplicated shapes");
                                    }
                                }
                                _ => {}
                            }
                        } else {
                            match key_str {
                                // Selection: V or 1
                                "v" | "V" | "1" => {
                                    state.canvas.set_tool(ToolKind::Select);
                                    log::info!("Tool: Select");
                                }
                                // Pan: H
                                "h" | "H" => {
                                    state.canvas.set_tool(ToolKind::Pan);
                                    log::info!("Tool: Pan");
                                }
                                // Rectangle: R or 2
                                "r" | "R" | "2" => {
                                    state.canvas.set_tool(ToolKind::Rectangle);
                                    log::info!("Tool: Rectangle");
                                }
                                // Ellipse: O or 4
                                "o" | "O" | "4" => {
                                    state.canvas.set_tool(ToolKind::Ellipse);
                                    log::info!("Tool: Ellipse");
                                }
                                // Arrow: A or 5
                                "a" | "A" | "5" => {
                                    state.canvas.set_tool(ToolKind::Arrow);
                                    log::info!("Tool: Arrow");
                                }
                                // Line: L or 6
                                "l" | "L" | "6" => {
                                    state.canvas.set_tool(ToolKind::Line);
                                    log::info!("Tool: Line");
                                }
                                // Draw/Pen: P or 7
                                "p" | "P" | "7" => {
                                    state.canvas.set_tool(ToolKind::Freehand);
                                    log::info!("Tool: Pen");
                                }
                                // Text: T or 8
                                "t" | "T" | "8" => {
                                    state.canvas.set_tool(ToolKind::Text);
                                    log::info!("Tool: Text");
                                }
                                // Math: M or 9
                                "m" | "M" | "9" => {
                                    state.canvas.set_tool(ToolKind::Math);
                                    log::info!("Tool: Math");
                                }
                                // Eraser: E
                                "e" | "E" => {
                                    state.canvas.set_tool(ToolKind::Eraser);
                                    log::info!("Tool: Eraser");
                                }
                                // Laser Pointer: Z
                                "z" | "Z" => {
                                    state.canvas.set_tool(ToolKind::LaserPointer);
                                    log::info!("Tool: Laser Pointer");
                                }
                                "Delete" | "Backspace" => {
                                    if !state.canvas.selection.is_empty() {
                                        state.canvas.document.push_undo();
                                        state.canvas.delete_selected();
                                    }
                                }
                                "Escape" => {
                                    // Skip if egui is handling this (e.g., closing its own dialogs)
                                    if egui_wants_input {
                                        return;
                                    }
                                    // First, close any open dialogs/popovers
                                    let had_open_dialog = state.ui_state.color_popover
                                        != crate::ui::ColorPopover::None
                                        || state.ui_state.menu_open
                                        || state.ui_state.collab_modal_open
                                        || state.ui_state.shortcuts_modal_open
                                        || state.ui_state.save_dialog_open
                                        || state.ui_state.open_dialog_open
                                        || state.ui_state.open_recent_dialog_open;

                                    if had_open_dialog {
                                        state.ui_state.color_popover =
                                            crate::ui::ColorPopover::None;
                                        state.ui_state.menu_open = false;
                                        state.ui_state.collab_modal_open = false;
                                        state.ui_state.shortcuts_modal_open = false;
                                        state.ui_state.save_dialog_open = false;
                                        state.ui_state.open_dialog_open = false;
                                        state.ui_state.open_recent_dialog_open = false;
                                    } else {
                                        // No dialog open - switch to Select tool
                                        state.canvas.tool_manager.cancel();
                                        state.canvas.clear_selection();
                                        state.canvas.set_tool(ToolKind::Select);
                                        state.ui_state.current_tool = ToolKind::Select;
                                    }
                                }
                                // Arrow keys: nudge selected shapes by grid size
                                "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" => {
                                    if !state.canvas.selection.is_empty() {
                                        use drafftink_core::GRID_SIZE;
                                        let delta = match key_str {
                                            "ArrowUp" => kurbo::Vec2::new(0.0, -GRID_SIZE),
                                            "ArrowDown" => kurbo::Vec2::new(0.0, GRID_SIZE),
                                            "ArrowLeft" => kurbo::Vec2::new(-GRID_SIZE, 0.0),
                                            "ArrowRight" => kurbo::Vec2::new(GRID_SIZE, 0.0),
                                            _ => return,
                                        };
                                        state.canvas.document.push_undo();
                                        let translation = kurbo::Affine::translate(delta);
                                        for &id in &state.canvas.selection {
                                            if let Some(shape) =
                                                state.canvas.document.get_shape_mut(id)
                                            {
                                                shape.transform(translation);
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    ElementState::Released => {
                        // Input state is handled by WinitInputHelper
                    }
                }
            }

            WindowEvent::ModifiersChanged(_) => {
                // Modifiers are tracked by WinitInputHelper
            }

            #[cfg(not(target_arch = "wasm32"))]
            WindowEvent::DroppedFile(path) => {
                // Handle dropped image files
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if matches!(ext_str.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                        if let Ok(data) = std::fs::read(&path) {
                            // Check for embedded scene data in PNG files
                            if ext_str == "png" {
                                if let Some(json) = extract_scene_from_png(&data) {
                                    log::info!("Found embedded scene in PNG, loading as document");
                                    use drafftink_core::canvas::CanvasDocument;
                                    match CanvasDocument::from_json(&json) {
                                        Ok(doc) => {
                                            state.canvas.document = doc;
                                            state.canvas.clear_selection();
                                            state.canvas.camera.reset();
                                            state.window.request_redraw();
                                            return;
                                        }
                                        Err(e) => log::error!("Failed to parse embedded document: {}", e),
                                    }
                                }
                            }

                            use drafftink_core::shapes::{Image, ImageFormat, Shape};
                            use kurbo::Point;

                            let format = match ext_str.as_str() {
                                "png" => ImageFormat::Png,
                                "jpg" | "jpeg" => ImageFormat::Jpeg,
                                "webp" => ImageFormat::WebP,
                                _ => ImageFormat::Png,
                            };

                            // Decode to get dimensions
                            if let Ok(decoded) = image::load_from_memory(&data) {
                                let (width, height) = (decoded.width(), decoded.height());

                                // Position at viewport center
                                let viewport_center =
                                    state.canvas.camera.screen_to_world(Point::new(
                                        state.canvas.viewport_size.width / 2.0,
                                        state.canvas.viewport_size.height / 2.0,
                                    ));
                                let position = Point::new(
                                    viewport_center.x - width as f64 / 2.0,
                                    viewport_center.y - height as f64 / 2.0,
                                );

                                // Create image shape, scaled to fit if too large
                                let max_size = 800.0;
                                let mut img = Image::new(position, &data, width, height, format);
                                if width as f64 > max_size || height as f64 > max_size {
                                    img = img.fit_within(max_size, max_size);
                                    img.position = Point::new(
                                        viewport_center.x - img.width / 2.0,
                                        viewport_center.y - img.height / 2.0,
                                    );
                                }

                                state.canvas.document.push_undo();
                                state.canvas.clear_selection();
                                let shape = Shape::Image(img);
                                let new_id = shape.id();
                                state.canvas.document.add_shape(shape.clone());
                                state.canvas.add_to_selection(new_id);
                                if state.collab.is_in_room() {
                                    let _ = state.collab.crdt_mut().add_shape(&shape);
                                }

                                log::info!(
                                    "Dropped image: {:?} ({}x{})",
                                    path.file_name(),
                                    width,
                                    height
                                );
                                state.window.request_redraw();
                            } else {
                                log::error!("Failed to decode dropped image: {:?}", path);
                            }
                        } else {
                            log::error!("Failed to read dropped file: {:?}", path);
                        }
                    }
                }
            }

            _ => {}
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: winit::event::StartCause) {
        if let Some(state) = &mut self.state {
            state.input.step();
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.state {
            state.input.end_step();
        }
    }
}
