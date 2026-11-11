//! [`WebFiles`]: the engine's `Files` port in a browser.
//!
//! A browser has no paths. A download hands the bytes to the user through an anchor click; a stored name
//! lives in the `Documents` namespace of the storage. Paths and File System Access handles are not offered.

use drafftink_engine::PortError;
use drafftink_engine::ports::{FileRef, Files, Ns, Storage, WriteOutcome};
use js_sys::{Array, Uint8Array};
use wasm_bindgen::JsCast;
use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, Url};

use super::dom::{after, document, js_error};
use super::pure::download_name;
use super::storage::WebStorage;

/// Files for the web: downloads and named documents in browser storage.
#[derive(Debug, Clone)]
pub(crate) struct WebFiles {
    storage: WebStorage,
}

impl WebFiles {
    pub(crate) fn new(storage: WebStorage) -> Self {
        Self { storage }
    }
}

fn unsupported(what: &str) -> PortError {
    PortError::Unsupported(format!(
        "{what} are not available in the browser. Use a download to save a copy, or open a file with the Open button"
    ))
}

/// Offers `bytes` to the user as a file called `name`.
fn download(name: &str, bytes: &[u8], mime: &str) -> Result<(), PortError> {
    let fail = |e: wasm_bindgen::JsValue| PortError::Failed(format!("the download could not start ({})", js_error(&e)));
    let parts = Array::of1(&Uint8Array::from(bytes));
    let options = BlobPropertyBag::new();
    options.set_type(mime);
    let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options).map_err(fail)?;
    let url = Url::create_object_url_with_blob(&blob).map_err(fail)?;
    let document = document().map_err(|e| PortError::Failed(e.to_string()))?;
    let anchor = document
        .create_element("a")
        .map_err(fail)?
        .dyn_into::<HtmlAnchorElement>()
        .map_err(|_| PortError::Failed("the browser could not make a download link".to_owned()))?;
    anchor.set_href(&url);
    anchor.set_download(name);
    let _ = anchor.set_attribute("style", "display:none");
    let body = document.body().ok_or_else(|| PortError::Failed("the page has no body".to_owned()))?;
    body.append_child(&anchor).map_err(fail)?;
    anchor.click();
    anchor.remove();
    // The browser has started reading the blob by then; release it a little later.
    after(30_000, move || {
        let _ = Url::revoke_object_url(&url);
    });
    Ok(())
}

impl Files for WebFiles {
    fn read(&self, file: &FileRef) -> Result<Vec<u8>, PortError> {
        match file {
            FileRef::Stored { stored } => self
                .storage
                .get(Ns::Documents, stored)?
                .ok_or_else(|| PortError::NotFound(format!("the stored document `{stored}`"))),
            FileRef::Download { download } => {
                Err(PortError::NotFound(format!("`{download}` was downloaded; the browser keeps no copy to read back")))
            }
            FileRef::Path { .. } => Err(unsupported("File paths")),
            FileRef::Handle { .. } => Err(unsupported("File handles")),
        }
    }

    fn exists(&self, file: &FileRef) -> bool {
        match file {
            FileRef::Stored { stored } => self.storage.get(Ns::Documents, stored).is_ok_and(|b| b.is_some()),
            _ => false,
        }
    }

    fn write(&mut self, file: &FileRef, bytes: &[u8], mime: &str) -> Result<WriteOutcome, PortError> {
        match file {
            FileRef::Download { download: name } => {
                let name = download_name(name);
                download(&name, bytes, mime)?;
                Ok(WriteOutcome::Done(FileRef::Download { download: name }))
            }
            FileRef::Stored { stored } => {
                self.storage.put(Ns::Documents, stored, bytes)?;
                Ok(WriteOutcome::Done(file.clone()))
            }
            FileRef::Path { .. } => Err(unsupported("File paths")),
            FileRef::Handle { .. } => Err(unsupported("File handles")),
        }
    }
}
