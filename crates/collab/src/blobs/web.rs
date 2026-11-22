//! The browser picture transport: `fetch` through `web-sys`.
//!
//! Each request runs as a local future on the page's thread (`wasm-bindgen-futures`); its answer goes into a plain
//! queue and the wake hook is called, like the WebSocket transport. `fetch` is found on the global object, so the
//! transport works in a page and in a worker. Redirects are an error (the `Authorization` header would go along), the
//! browser's HTTP cache is bypassed (`no-store`), and a request ends after [`REQUEST_TIMEOUT_MS`].

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use js_sys::{Function, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortSignal, Headers, Request, RequestCache, RequestInit, RequestRedirect, Response};

use super::{
    BlobOp, BlobReply, BlobRequest, BlobResponse, BlobTransport, BlobTransportError, MAX_BLOB_BODY_BYTES,
    retry_after_ms,
};
use crate::transport::WakeFn;

/// A request that has not finished after this long fails. The client gives up a little later (120 s).
const REQUEST_TIMEOUT_MS: u32 = 100_000;

type Answers = Rc<RefCell<VecDeque<BlobResponse>>>;

/// Picture requests through the browser's `fetch`. Not `Send`: it lives on the page's thread.
pub struct WebBlobTransport {
    wake: WakeFn,
    answers: Answers,
}

impl WebBlobTransport {
    /// A transport that calls `wake` whenever it has queued an answer.
    pub fn new(wake: WakeFn) -> Self {
        WebBlobTransport { wake, answers: Rc::default() }
    }
}

impl BlobTransport for WebBlobTransport {
    fn send(&mut self, request: BlobRequest) -> Result<(), BlobTransportError> {
        let fetch = fetch_function()?;
        let built = build_request(&request)
            .map_err(|_| BlobTransportError::Refused("the browser refused the picture request".into()))?;
        let answers = Rc::clone(&self.answers);
        let wake = std::sync::Arc::clone(&self.wake);
        let id = request.id;
        let head = matches!(request.op, BlobOp::Head);
        wasm_bindgen_futures::spawn_local(async move {
            let result = run(&fetch, &built, head).await.map_err(describe);
            answers.borrow_mut().push_back(BlobResponse { id, result });
            wake();
        });
        Ok(())
    }

    fn poll(&mut self) -> Option<BlobResponse> {
        self.answers.borrow_mut().pop_front()
    }
}

fn fetch_function() -> Result<Function, BlobTransportError> {
    let global = js_sys::global();
    Reflect::get(&global, &JsValue::from_str("fetch"))
        .ok()
        .and_then(|f| f.dyn_into::<Function>().ok())
        .ok_or_else(|| BlobTransportError::Failed("this browser has no fetch".into()))
}

fn build_request(request: &BlobRequest) -> Result<Request, JsValue> {
    let headers = Headers::new()?;
    headers.append("authorization", request.auth.header_value())?;
    let init = RequestInit::new();
    init.set_method(request.op.method());
    init.set_headers(&headers);
    init.set_cache(RequestCache::NoStore);
    init.set_redirect(RequestRedirect::Error);
    init.set_signal(Some(&AbortSignal::timeout_with_u32(REQUEST_TIMEOUT_MS)));
    if let BlobOp::Put(body) = &request.op {
        init.set_body(&Uint8Array::from(body.as_slice()));
    }
    Request::new_with_str_and_init(&request.url, &init)
}

async fn run(fetch: &Function, request: &Request, head: bool) -> Result<BlobReply, JsValue> {
    let promise: Promise = fetch.call1(&js_sys::global(), request)?.dyn_into()?;
    let response: Response = JsFuture::from(promise).await?.dyn_into()?;
    let headers = response.headers();
    let text = |name: &str| headers.get(name).ok().flatten();
    let content_length = text("content-length").and_then(|v| v.trim().parse::<u64>().ok());
    let retry_after_ms = text("retry-after").as_deref().and_then(retry_after_ms);
    let mut body = Vec::new();
    if !head {
        if content_length.is_some_and(|n| n > MAX_BLOB_BODY_BYTES as u64) {
            return Err(JsValue::from_str("the answer is larger than any picture"));
        }
        let buffer = JsFuture::from(response.array_buffer()?).await?;
        body = Uint8Array::new(&buffer).to_vec();
        if body.len() > MAX_BLOB_BODY_BYTES {
            return Err(JsValue::from_str("the answer is larger than any picture"));
        }
    }
    Ok(BlobReply { status: response.status(), body, content_length, retry_after_ms })
}

/// A short text for a failed fetch. A browser says little ("Failed to fetch") on purpose.
fn describe(error: JsValue) -> String {
    error
        .as_string()
        .or_else(|| Reflect::get(&error, &JsValue::from_str("message")).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| "the request failed".to_owned())
}
