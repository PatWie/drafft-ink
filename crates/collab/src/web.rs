//! The browser WebSocket transport (`web-sys`, binary type `arraybuffer`).
//!
//! Browser callbacks run on the page's single thread, so events go into a plain queue and the wake hook is called
//! from the callback. The page then runs `CollabSession::pump` from its own callback, never from a timer.
//!
//! A browser behind Cloudflare Access sends its sign-in cookie with the upgrade, and a `WebSocket` cannot carry other
//! headers, so `set_access` is ignored and a service token is never used here. The page cannot see the status of a
//! failed upgrade either: it is a close with code 1006. Telling an ended sign-in, a host that has no relay and a relay
//! over its daily limit from a server that is down needs the probe of `docs/design/collab-e2e.md` sections 11.6 and
//! 19.6: `GET /api/status` on the server's own origin, with `X-Requested-With: XMLHttpRequest`. A close before any
//! `Opened` is therefore answered by the probe: a `401` ends the connection as [`TransportEvent::Denied`] with
//! `AccessDenial::SignInExpired`, a `404` as [`TransportEvent::Unavailable`] (`NoRelay`), a spent quota as
//! `DailyLimit`. Any other answer, or none, leaves the plain close.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use js_sys::{ArrayBuffer, Function, Promise, Reflect, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortSignal, BinaryType, CloseEvent, Event, Headers, MessageEvent, Request, RequestCache, RequestInit,
    RequestRedirect, Response, WebSocket,
};

use crate::access::AccessDenial;
use crate::probe::unavailable_of;

use crate::transport::{Transport, TransportError, TransportEvent, WakeFn};
use crate::url::ServerUrl;

type Queue = Rc<RefCell<VecDeque<(u64, TransportEvent)>>>;

struct Handlers {
    socket: WebSocket,
    _open: Closure<dyn FnMut(Event)>,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _error: Closure<dyn FnMut(Event)>,
    _close: Closure<dyn FnMut(CloseEvent)>,
}

impl Handlers {
    /// Stops the callbacks and closes the socket. Frames already passed to `send` are still written by the browser.
    fn shut(self) {
        self.socket.set_onopen(None);
        self.socket.set_onmessage(None);
        self.socket.set_onerror(None);
        self.socket.set_onclose(None);
        let _ = self.socket.close();
    }
}

/// A WebSocket in the browser. Not `Send`: it lives on the page's thread.
pub struct WebTransport {
    wake: WakeFn,
    queue: Queue,
    generation: u64,
    current: Option<Handlers>,
}

impl WebTransport {
    /// A transport that calls `wake` whenever it queues an event.
    pub fn new(wake: WakeFn) -> Self {
        WebTransport { wake, queue: Rc::default(), generation: 0, current: None }
    }
}

impl Transport for WebTransport {
    fn open(&mut self, url: &ServerUrl) -> Result<(), TransportError> {
        if let Some(old) = self.current.take() {
            old.shut();
        }
        self.generation += 1;
        let generation = self.generation;
        let socket = WebSocket::new(&url.to_string())
            .map_err(|_| TransportError::Failed("the browser refused the server address".to_owned()))?;
        socket.set_binary_type(BinaryType::Arraybuffer);

        let push = {
            let queue = Rc::clone(&self.queue);
            let wake = std::sync::Arc::clone(&self.wake);
            move |event: TransportEvent| {
                queue.borrow_mut().push_back((generation, event));
                wake();
            }
        };
        let opened = Rc::new(Cell::new(false));
        let open = {
            let push = push.clone();
            let opened = Rc::clone(&opened);
            Closure::<dyn FnMut(Event)>::new(move |_| {
                opened.set(true);
                push(TransportEvent::Opened);
            })
        };
        let message = {
            let push = push.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                let data = e.data();
                if let Ok(buffer) = data.clone().dyn_into::<ArrayBuffer>() {
                    push(TransportEvent::Message(Uint8Array::new(&buffer).to_vec()));
                } else if let Some(text) = data.as_string() {
                    push(TransportEvent::Text(text));
                }
            })
        };
        // A browser gives no detail on errors; the close event that follows ends the connection.
        let error = Closure::<dyn FnMut(Event)>::new(|_| {});
        let close = {
            let push = push.clone();
            let base = url.http_base();
            Closure::<dyn FnMut(CloseEvent)>::new(move |e: CloseEvent| {
                let error = (!e.was_clean()).then(|| format!("closed with code {}", e.code()));
                if opened.get() {
                    push(TransportEvent::Closed { error });
                    return;
                }
                // The socket never opened: the browser says nothing more, so ask the server's status address.
                let (push, base) = (push.clone(), base.clone());
                wasm_bindgen_futures::spawn_local(async move {
                    let event = probe(&base).await.unwrap_or(TransportEvent::Closed { error });
                    push(event);
                });
            })
        };
        socket.set_onopen(Some(open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
        socket.set_onerror(Some(error.as_ref().unchecked_ref()));
        socket.set_onclose(Some(close.as_ref().unchecked_ref()));
        self.current = Some(Handlers { socket, _open: open, _message: message, _error: error, _close: close });
        Ok(())
    }

    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        let Some(h) = &self.current else { return Err(TransportError::NotOpen) };
        if h.socket.ready_state() != WebSocket::OPEN {
            return Err(TransportError::NotOpen);
        }
        h.socket.send_with_u8_array(&frame).map_err(|_| TransportError::Failed("the browser could not send".to_owned()))
    }

    fn send_text(&mut self, text: &str) -> Result<(), TransportError> {
        let Some(h) = &self.current else { return Err(TransportError::NotOpen) };
        if h.socket.ready_state() != WebSocket::OPEN {
            return Err(TransportError::NotOpen);
        }
        h.socket.send_with_str(text).map_err(|_| TransportError::Failed("the browser could not send".to_owned()))
    }

    fn close(&mut self) {
        if let Some(h) = self.current.take() {
            h.shut();
        }
        self.generation += 1;
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        let mut q = self.queue.borrow_mut();
        while let Some((generation, event)) = q.pop_front() {
            if generation == self.generation {
                return Some(event);
            }
        }
        None
    }
}

/// How long the probe waits for the answer of the status address.
const PROBE_TIMEOUT_MS: u32 = 5_000;

/// Asks `GET <base>/api/status` why a socket did not open. `None`: the server answered nothing that explains it (it is
/// up, or unreachable, or the answer is not one this client knows), and the plain close stands.
async fn probe(base: &str) -> Option<TransportEvent> {
    let fetch: Function = Reflect::get(&js_sys::global(), &JsValue::from_str("fetch")).ok()?.dyn_into().ok()?;
    let headers = Headers::new().ok()?;
    headers.append("x-requested-with", "XMLHttpRequest").ok()?;
    let init = RequestInit::new();
    init.set_method("GET");
    init.set_headers(&headers);
    init.set_cache(RequestCache::NoStore);
    init.set_redirect(RequestRedirect::Error);
    init.set_signal(Some(&AbortSignal::timeout_with_u32(PROBE_TIMEOUT_MS)));
    let request = Request::new_with_str_and_init(&format!("{base}/api/status"), &init).ok()?;
    let promise: Promise = fetch.call1(&js_sys::global(), &request).ok()?.dyn_into().ok()?;
    let response: Response = JsFuture::from(promise).await.ok()?.dyn_into().ok()?;
    let status = response.status();
    let body = match response.array_buffer() {
        Ok(buffer) => JsFuture::from(buffer).await.ok().map(|b| Uint8Array::new(&b).to_vec()).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    if let Some(denial) = AccessDenial::of_reply(status, &body, false) {
        return Some(TransportEvent::Denied(denial.in_browser()));
    }
    unavailable_of(status, &body).map(TransportEvent::Unavailable)
}
