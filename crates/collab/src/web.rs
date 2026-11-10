//! The browser WebSocket transport (`web-sys`, binary type `arraybuffer`).
//!
//! Browser callbacks run on the page's single thread, so events go into a plain queue and the wake hook is called
//! from the callback. The page then runs `CollabSession::pump` from its own callback, never from a timer.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use js_sys::{ArrayBuffer, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

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
        let open = {
            let push = push.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| push(TransportEvent::Opened))
        };
        let message = {
            let push = push.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                // The server sends binary frames only; text is ignored.
                if let Ok(buffer) = e.data().dyn_into::<ArrayBuffer>() {
                    push(TransportEvent::Message(Uint8Array::new(&buffer).to_vec()));
                }
            })
        };
        // A browser gives no detail on errors; the close event that follows ends the connection.
        let error = Closure::<dyn FnMut(Event)>::new(|_| {});
        let close = {
            let push = push.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |e: CloseEvent| {
                let error = (!e.was_clean()).then(|| format!("closed with code {}", e.code()));
                push(TransportEvent::Closed { error });
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
