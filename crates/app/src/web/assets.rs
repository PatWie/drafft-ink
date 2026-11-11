//! Static files the page serves next to the wasm module: the big fonts (docs/web-app.md, "Size").
//!
//! `scripts/build-web.sh` copies each font to `fonts/<name>.<hash>.<ext>` and writes the map from the plain
//! file name to that address into `window.__assets` (in `index.html`), with a `<link rel="preload">` for the
//! font the first frame needs. The hash is in the address, so a cached font is valid forever and the service
//! worker (`web/sw.js`) keeps it with the first answer.

use js_sys::{ArrayBuffer, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::Response;

use super::dom::{js_error, window};
use crate::platform::FontFile;

/// What can go wrong while fetching a font.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AssetError {
    /// The page has no address for the file (a page not made by the build script).
    #[error("the page does not list {0}")]
    Unlisted(&'static str),
    /// The request failed.
    #[error("{0}")]
    Browser(String),
    /// The server answered with an error status.
    #[error("the server answered {0}")]
    Status(u16),
}

fn browser(e: &JsValue) -> AssetError {
    AssetError::Browser(js_error(e))
}

/// The address of `file_name` in the page's asset map.
fn address(file_name: &'static str) -> Result<String, AssetError> {
    let window = window().map_err(|e| AssetError::Browser(e.to_string()))?;
    let map = Reflect::get(&window, &JsValue::from_str("__assets")).map_err(|e| browser(&e))?;
    Reflect::get(&map, &JsValue::from_str(file_name))
        .ok()
        .and_then(|v| v.as_string())
        .ok_or(AssetError::Unlisted(file_name))
}

/// Fetches the bytes of a font.
pub(crate) async fn fetch_font(file: FontFile) -> Result<Vec<u8>, AssetError> {
    let url = address(file.file_name())?;
    let window = window().map_err(|e| AssetError::Browser(e.to_string()))?;
    let response = JsFuture::from(window.fetch_with_str(&url)).await.map_err(|e| browser(&e))?;
    let response: Response = response.dyn_into().map_err(|e| browser(&e))?;
    if !response.ok() {
        return Err(AssetError::Status(response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|e| browser(&e))?).await.map_err(|e| browser(&e))?;
    let buffer: ArrayBuffer = buffer.dyn_into().map_err(|e| browser(&e))?;
    Ok(Uint8Array::new(&buffer).to_vec())
}
