//! The agent relay client: `window.drafftink.agent` (docs/agents.md, section 5.2).
//!
//! The relay (`drafftink mcp --relay`) is the WebSocket server on `127.0.0.1`. The page connects out, says
//! `hello` with a token, and from then on answers the relay's JSON-RPC requests. Each request goes through
//! the same job queue as `window.drafftink.execute`, so it runs between frames on the session with origin
//! `agent`. The rules that need no browser are in [`crate::pairing`].

use std::cell::RefCell;

use drafftink_bridge::host::{Job, JobSender};
use drafftink_bridge::rpc::{self, Incoming, RpcError};
use js_sys::{Function, Object, Promise, Reflect};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

use crate::js;
use crate::pairing::{
    AgentUrl, Backoff, CLOSE_POLICY, HelloAnswer, PairingError, Remembered, check_agent_url, hello_frame, is_token,
    parse_hello_answer, relay_url, take_pairing,
};

const STORAGE_KEY: &str = "drafftink-agent";

/// Where the connection stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Connecting,
    Connected,
    /// Lost; the next attempt is scheduled.
    Waiting,
    /// The relay refused the token. A new link is needed.
    Refused,
    /// Reconnecting went on for ten minutes without success.
    GaveUp,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Connecting => "connecting",
            State::Connected => "connected",
            State::Waiting => "waiting",
            State::Refused => "refused",
            State::GaveUp => "gave_up",
        }
    }
}

/// The closures of one socket. Dropping them frees them, so they stay with the socket.
struct Link {
    generation: u64,
    socket: WebSocket,
    _open: Closure<dyn FnMut()>,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _close: Closure<dyn FnMut(CloseEvent)>,
}

struct Agent {
    jobs: JobSender,
    state: State,
    message: Option<String>,
    port: Option<u16>,
    /// The token the next `hello` carries: the pairing token first, the resume token after.
    token: Option<String>,
    greeted: bool,
    backoff: Backoff,
    generation: u64,
    link: Option<Link>,
    timer: Option<i32>,
    /// The promise of `pair`, until the first `hello` is answered.
    waiter: Option<(Function, Function)>,
}

thread_local! {
    static AGENT: RefCell<Option<Agent>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Agent) -> R) -> Option<R> {
    AGENT.with(|a| a.borrow_mut().as_mut().map(f))
}

fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

fn remember(port: u16, resume_token: &str) {
    if let Some(s) = session_storage() {
        // A tab that cannot store simply does not resume after a reload.
        let _ = s.set_item(STORAGE_KEY, &Remembered { port, resume_token: resume_token.to_owned() }.to_text());
    }
}

fn forget() {
    if let Some(s) = session_storage() {
        let _ = s.remove_item(STORAGE_KEY);
    }
}

/// Builds `window.drafftink.agent`, and starts what the address or the tab remembers.
pub(crate) fn api(jobs: &JobSender) -> JsValue {
    AGENT.with(|a| {
        *a.borrow_mut() = Some(Agent {
            jobs: jobs.clone(),
            state: State::Idle,
            message: None,
            port: None,
            token: None,
            greeted: false,
            backoff: Backoff::new(),
            generation: 0,
            link: None,
            timer: None,
            waiter: None,
        });
    });
    let object = Object::new();
    let pair = Closure::<dyn Fn(JsValue, JsValue) -> JsValue>::new(|url, token| pair(&url, &token).into());
    // `connect` is the name docs/agents.md uses; `pair` is the name of the flow. Same function.
    let _ = Reflect::set(&object, &"pair".into(), pair.as_ref());
    let _ = Reflect::set(&object, &"connect".into(), pair.as_ref());
    pair.forget();
    let disconnect = Closure::<dyn Fn() -> JsValue>::new(|| {
        disconnect();
        JsValue::UNDEFINED
    });
    let _ = Reflect::set(&object, &"disconnect".into(), disconnect.as_ref());
    disconnect.forget();
    let status = Closure::<dyn Fn() -> JsValue>::new(status);
    let _ = Reflect::set(&object, &"status".into(), status.as_ref());
    status.forget();
    object.into()
}

/// At page start: pairs from the address (and removes the pairing from it), else resumes what this tab had.
pub(crate) fn start_from_page() {
    let Some(window) = web_sys::window() else { return };
    let location = window.location();
    let hash = location.hash().unwrap_or_default();
    let (pairing, rest) = take_pairing(&hash);
    if hash.trim_start_matches('#') != rest {
        scrub_address(&window, &rest);
    }
    if let Some(p) = pairing {
        // The promise is for scripts. A page that pairs from its address reports through `status()`.
        let promise = start_pairing(&relay_url(p.port), p.port, &p.token);
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = wasm_bindgen_futures::JsFuture::from(promise).await {
                log::warn!("agent pairing from the address failed: {}", describe(&e));
            }
        });
        return;
    }
    let remembered =
        session_storage().and_then(|s| s.get_item(STORAGE_KEY).ok().flatten()).and_then(|t| Remembered::from_text(&t));
    if let Some(r) = remembered {
        with(|a| {
            a.port = Some(r.port);
            a.token = Some(r.resume_token);
            a.greeted = true;
        });
        connect_now();
    }
}

fn describe(e: &JsValue) -> String {
    js_sys::Reflect::get(e, &"message".into()).ok().and_then(|m| m.as_string()).unwrap_or_else(|| format!("{e:?}"))
}

/// Replaces the address with itself minus the pairing, without adding a history entry.
fn scrub_address(window: &web_sys::Window, rest_of_hash: &str) {
    let location = window.location();
    let (Ok(path), Ok(search)) = (location.pathname(), location.search()) else { return };
    let url =
        if rest_of_hash.is_empty() { format!("{path}{search}") } else { format!("{path}{search}#{rest_of_hash}") };
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url));
    }
}

fn refusal(error: &PairingError) -> Promise {
    js::rejected(&RpcError::new(rpc::INVALID_PARAMS, error.to_string()))
}

fn pair(url: &JsValue, token: &JsValue) -> Promise {
    let (Some(url), Some(token)) = (url.as_string(), token.as_string()) else {
        return js::rejected(&RpcError::new(
            rpc::INVALID_PARAMS,
            "pair needs two strings: pair('ws://127.0.0.1:<port>/', '<64 hex token>')",
        ));
    };
    let AgentUrl { url, port } = match check_agent_url(&url) {
        Ok(u) => u,
        Err(e) => return refusal(&e),
    };
    let token = token.trim().to_owned();
    if !is_token(&token) {
        return refusal(&PairingError::BadToken);
    }
    start_pairing(&url, port, &token)
}

fn start_pairing(url: &str, port: u16, token: &str) -> Promise {
    disconnect();
    let url = url.to_owned();
    let token = token.to_owned();
    Promise::new(&mut |resolve, reject| {
        let started = with(|a| {
            a.port = Some(port);
            a.token = Some(token.clone());
            a.greeted = false;
            a.waiter = Some((resolve.clone(), reject.clone()));
            a.backoff.reset();
        });
        if started.is_none() {
            let _ = reject.call1(
                &JsValue::NULL,
                &js::error_object(&RpcError::new(rpc::INTERNAL_ERROR, "the agent API is not ready")),
            );
            return;
        }
        connect(&url);
    })
}

fn connect_now() {
    let Some(port) = with(|a| a.port).flatten() else { return };
    connect(&relay_url(port));
}

fn connect(url: &str) {
    let Some((generation, jobs)) = with(|a| {
        a.generation += 1;
        a.state = State::Connecting;
        a.message = None;
        (a.generation, a.jobs.clone())
    }) else {
        return;
    };
    let socket = match WebSocket::new(url) {
        Ok(s) => s,
        Err(_) => {
            failed(generation, "The browser refused to open the agent address.");
            return;
        }
    };
    let open = Closure::<dyn FnMut()>::new(move || on_open(generation));
    let message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(text) = e.data().as_string() {
            on_message(generation, &text, &jobs);
        }
    });
    let close = Closure::<dyn FnMut(CloseEvent)>::new(move |e: CloseEvent| on_close(generation, e.code()));
    socket.set_onopen(Some(open.as_ref().unchecked_ref()));
    socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
    socket.set_onclose(Some(close.as_ref().unchecked_ref()));
    let old =
        with(|a| a.link.replace(Link { generation, socket, _open: open, _message: message, _close: close })).flatten();
    drop_link(old);
}

fn drop_link(link: Option<Link>) {
    if let Some(l) = link {
        l.socket.set_onopen(None);
        l.socket.set_onmessage(None);
        l.socket.set_onclose(None);
        let _ = l.socket.close();
    }
}

fn send(generation: u64, frame: &str) {
    with(|a| {
        if let Some(l) =
            a.link.as_ref().filter(|l| l.generation == generation && l.socket.ready_state() == WebSocket::OPEN)
        {
            let _ = l.socket.send_with_str(frame);
        }
    });
}

fn on_open(generation: u64) {
    let frame = with(|a| {
        if a.generation != generation {
            return None;
        }
        let token = a.token.clone()?;
        let registry = drafftink_engine::Registry::global();
        Some(hello_frame(
            &token,
            env!("CARGO_PKG_VERSION"),
            registry.len(),
            &drafftink_bridge::core::registry_digest(&registry),
        ))
    })
    .flatten();
    if let Some(frame) = frame {
        send(generation, &frame);
    }
}

fn on_message(generation: u64, text: &str, jobs: &JobSender) {
    let Some((current, greeted)) = with(|a| (a.generation, a.greeted)) else { return };
    if current != generation {
        return;
    }
    if !greeted {
        on_hello_answer(generation, text);
        return;
    }
    match rpc::parse(text) {
        Ok(Incoming::Request { id, method, params }) => {
            let reply_id = id.clone();
            let job = Job {
                method,
                params,
                reply: Box::new(move |outcome| {
                    let frame = match outcome {
                        Ok(v) => rpc::response_ok(&reply_id, &v),
                        Err(e) => rpc::response_err(Some(&reply_id), &e),
                    };
                    send(generation, &frame);
                }),
            };
            if let Err((error, reply)) = jobs.submit(job) {
                reply(Err(error));
            }
        }
        // The relay sends requests only. Notifications and answers need nothing from the page.
        Ok(Incoming::Notification { .. } | Incoming::Response { .. }) => {}
        Err(m) => send(generation, &rpc::response_err(m.id.as_ref(), &m.error)),
    }
}

fn on_hello_answer(generation: u64, text: &str) {
    match parse_hello_answer(text) {
        HelloAnswer::Welcome { resume_token } => {
            let (port, waiter) = with(|a| {
                a.greeted = true;
                a.state = State::Connected;
                a.message = None;
                a.token = Some(resume_token.clone());
                a.backoff.reset();
                (a.port, a.waiter.take())
            })
            .unwrap_or((None, None));
            if let Some(port) = port {
                remember(port, &resume_token);
            }
            if let Some((resolve, _)) = waiter {
                let _ = resolve.call1(&JsValue::NULL, &js::from_json(&status_json()));
            }
        }
        HelloAnswer::Unauthorized => refused(generation, "The relay did not accept the token. Ask for a new link."),
        HelloAnswer::Failed(why) => end(generation, State::Refused, &format!("The relay refused the hello: {why}")),
    }
}

/// The relay said no for good: forget the token, close, tell whoever waits.
fn refused(generation: u64, why: &str) {
    forget();
    end(generation, State::Refused, why);
}

fn end(generation: u64, state: State, why: &str) {
    let parts = with(|a| {
        if a.generation != generation {
            return None;
        }
        a.state = state;
        a.message = Some(why.to_owned());
        a.generation += 1; // late events of the old socket no longer count
        Some((a.link.take(), a.waiter.take()))
    })
    .flatten();
    if let Some((link, waiter)) = parts {
        drop_link(link);
        if let Some((_, reject)) = waiter {
            let _ = reject.call1(&JsValue::NULL, &js::error_object(&RpcError::new(rpc::UNAUTHORIZED, why)));
        }
    }
}

fn on_close(generation: u64, code: u16) {
    if with(|a| a.generation != generation).unwrap_or(true) {
        return;
    }
    if code == CLOSE_POLICY {
        refused(generation, "The relay did not accept the token. Ask for a new link.");
        return;
    }
    failed(generation, "The connection to the agent relay closed.");
}

/// The socket failed or closed. A pairing that never worked is reported to the caller; a connection that
/// had worked (or a resume after a reload) is retried on the schedule.
fn failed(generation: u64, why: &str) {
    let first_try = with(|a| a.generation == generation && a.waiter.is_some()).unwrap_or(false);
    if first_try {
        let port = with(|a| a.port).flatten().unwrap_or(0);
        let text = format!(
            "Could not reach the agent relay on 127.0.0.1:{port}. Is `drafftink mcp --relay` running, and did the browser allow the local connection?"
        );
        end(generation, State::Idle, &text);
        return;
    }
    let delay = with(|a| {
        if a.generation != generation {
            return None;
        }
        let old = a.link.take();
        a.message = Some(why.to_owned());
        let delay = a.backoff.next_delay_ms(now_ms());
        a.state = if delay.is_some() { State::Waiting } else { State::GaveUp };
        Some((old, delay))
    })
    .flatten();
    let Some((old, delay)) = delay else { return };
    drop_link(old);
    match delay {
        Some(ms) => schedule(generation, ms),
        None => {
            forget();
            with(|a| {
                a.message =
                    Some("The agent relay did not come back for ten minutes. Pair again with a new link.".to_owned())
            });
        }
    }
}

fn schedule(generation: u64, ms: u64) {
    let Some(window) = web_sys::window() else { return };
    let callback = Closure::once_into_js(move || {
        let still_waiting = with(|a| a.generation == generation && a.state == State::Waiting).unwrap_or(false);
        if still_waiting {
            with(|a| a.timer = None);
            connect_now();
        }
    });
    let ms = i32::try_from(ms).unwrap_or(30_000);
    let handle = window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), ms).ok();
    with(|a| a.timer = handle);
}

/// Closes with 1000 and forgets the resume token.
fn disconnect() {
    let parts = with(|a| {
        a.generation += 1;
        a.state = State::Idle;
        a.message = None;
        a.token = None;
        a.greeted = false;
        a.backoff.reset();
        (a.link.take(), a.timer.take(), a.waiter.take())
    });
    forget();
    if let Some((link, timer, waiter)) = parts {
        if let (Some(handle), Some(window)) = (timer, web_sys::window()) {
            window.clear_timeout_with_handle(handle);
        }
        if let Some(l) = &link {
            let _ = l.socket.close_with_code_and_reason(1000, "disconnect");
        }
        drop_link(link);
        if let Some((_, reject)) = waiter {
            let _ = reject.call1(
                &JsValue::NULL,
                &js::error_object(&RpcError::new(rpc::INTERNAL_ERROR, "pairing was cancelled by disconnect()")),
            );
        }
    }
}

fn status_json() -> Value {
    with(|a| {
        json!({
            "state": a.state.name(),
            "port": a.port,
            "message": a.message,
            "attempts": a.backoff.attempts(),
        })
    })
    .unwrap_or_else(|| json!({ "state": "idle" }))
}

fn status() -> JsValue {
    js::from_json(&status_json())
}
