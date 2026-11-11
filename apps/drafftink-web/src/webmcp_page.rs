//! WebMCP in the browser: registers the generated tools with `document.modelContext` (or, in the first
//! previews, `navigator.modelContext`) when the browser has it, and does nothing when it does not. The rules
//! that need no browser are in [`crate::webmcp`].
//!
//! Every tool call goes through [`drafftink_bridge::tools::service::Service`] on a [`PageCaller`], which puts the
//! bridge method on the same job queue as `window.drafftink.execute`: it runs between frames on the session with
//! origin `agent`, is recorded in `recent`, and is one undo step like any other command.

use std::cell::RefCell;
use std::rc::Rc;

use drafftink_bridge::host::JobSender;
use drafftink_bridge::rpc::{INTERNAL_ERROR, RpcError};
use drafftink_bridge::tools::outcome::Outcome;
use drafftink_bridge::tools::service::Service;
use drafftink_bridge::tools::{BoxFuture, Caller, Mode};
use js_sys::{Array, Function, Object, Promise, Reflect};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, future_to_promise};

use crate::js;
use crate::webmcp::{arguments, descriptor, result, wanted};

/// Where the browser offers the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// `document.modelContext`: the draft of November 2026.
    Document,
    /// `navigator.modelContext`: the first previews.
    Navigator,
}

impl Surface {
    fn name(self) -> &'static str {
        match self {
            Self::Document => "document.modelContext",
            Self::Navigator => "navigator.modelContext",
        }
    }
}

/// Answers the bridge methods of the tools by queueing them for the app.
struct PageCaller {
    jobs: JobSender,
}

fn rpc_error(value: &JsValue) -> RpcError {
    match js::to_json(value) {
        Ok(v @ Value::Object(_)) => RpcError::from_value(&v),
        _ => RpcError::new(INTERNAL_ERROR, "the app answered with an error that could not be read"),
    }
}

impl Caller for PageCaller {
    fn mode(&self) -> Mode {
        Mode::Page
    }

    fn call<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let answer = JsFuture::from(js::call(&self.jobs, method, params)).await.map_err(|e| rpc_error(&e))?;
            js::to_json(&answer)
        })
    }

    fn status(&self) -> BoxFuture<'_, Value> {
        Box::pin(async move {
            let surface = find().map(|(s, _)| s.name());
            json!({ "mode": Mode::Page.name(), "connected": true, "registered_with": surface })
        })
    }
}

fn get(target: &JsValue, key: &str) -> Option<JsValue> {
    Reflect::get(target, &JsValue::from_str(key)).ok().filter(|v| !v.is_undefined() && !v.is_null())
}

/// The browser's model context, preferring the current API over the preview one.
fn find() -> Option<(Surface, JsValue)> {
    let window = web_sys::window()?;
    let candidates = [
        (Surface::Document, window.document().map(JsValue::from)),
        (Surface::Navigator, Some(JsValue::from(window.navigator()))),
    ];
    candidates.into_iter().find_map(|(surface, owner)| {
        let context = get(&owner?, "modelContext")?;
        get(&context, "registerTool").filter(|f| f.is_function()).map(|_| (surface, context))
    })
}

fn call_method(target: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let f: Function = get(target, name).ok_or_else(|| JsValue::from_str(name))?.dyn_into()?;
    f.apply(target, &args.iter().collect::<Array>())
}

/// What `registerTool` returned for one tool, kept so that a failure is reported once and nowhere else.
fn warn_on_failure(registered: Result<JsValue, JsValue>, name: &str) {
    let name = name.to_owned();
    let report = move |why: JsValue| log::warn!("WebMCP: could not register the tool `{name}`: {why:?}");
    match registered {
        Ok(v) => {
            if let Ok(p) = v.dyn_into::<Promise>() {
                let on_error = Closure::once(report);
                let _ = p.catch(&on_error);
                on_error.forget();
            }
        }
        Err(why) => report(why),
    }
}

/// The tools of this page, built once, and what registering them has left to undo.
struct WebMcp {
    tools: Vec<(String, JsValue)>,
    controller: Option<JsValue>,
}

thread_local! {
    static WEBMCP: RefCell<Option<WebMcp>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut WebMcp) -> R) -> Option<R> {
    WEBMCP.with(|w| w.borrow_mut().as_mut().map(f))
}

fn set(target: &Object, name: &str, value: &JsValue) {
    let _ = Reflect::set(target, &JsValue::from_str(name), value);
}

/// One tool as the object `registerTool` takes: the descriptor and an `execute` that runs the tool.
fn tool_object(service: &Rc<Service>, name: &str, spec: &Value) -> JsValue {
    let object = Object::new();
    if let Some(fields) = spec.as_object() {
        for (key, value) in fields {
            set(&object, key, &js::from_json(value));
        }
    }
    let (service, tool) = (service.clone(), name.to_owned());
    let execute = Closure::<dyn Fn(JsValue, JsValue) -> JsValue>::new(move |input, _options| {
        let (service, tool) = (service.clone(), tool.clone());
        future_to_promise(async move {
            let outcome = match js::to_json(&input).map_err(|e| e.message).and_then(arguments) {
                Ok(args) => {
                    service.call(&tool, args).await.unwrap_or_else(|unknown| Outcome::error(unknown.to_string()))
                }
                Err(problem) => Outcome::error(problem),
            };
            Ok(js::from_json(&result(&outcome)))
        })
        .into()
    });
    set(&object, "execute", execute.as_ref());
    // The tool lives as long as the page.
    execute.forget();
    object.into()
}

fn abort_controller() -> Option<JsValue> {
    let ctor: Function = get(&js_sys::global(), "AbortController")?.dyn_into().ok()?;
    Reflect::construct(&ctor, &Array::new()).ok()
}

/// Registers every tool. Does nothing when the browser has no WebMCP or the tools are registered already.
fn register() -> bool {
    let Some((_, context)) = find() else { return false };
    with(|state| {
        if state.controller.is_some() {
            return true;
        }
        let controller = abort_controller();
        let options = Object::new();
        if let Some(signal) = controller.as_ref().and_then(|c| get(c, "signal")) {
            set(&options, "signal", &signal);
        }
        for (name, tool) in &state.tools {
            warn_on_failure(call_method(&context, "registerTool", &[tool.clone(), options.clone().into()]), name);
        }
        state.controller = Some(controller.unwrap_or(JsValue::TRUE));
        true
    })
    .unwrap_or(false)
}

/// Takes the tools back. The signal of the registration removes them; the preview API has `unregisterTool`.
fn unregister() {
    let context = find().map(|(_, c)| c);
    with(|state| {
        if let Some(controller) = state.controller.take()
            && controller.is_object()
        {
            let _ = call_method(&controller, "abort", &[]);
        }
        if let Some(context) = &context {
            for (name, _) in &state.tools {
                if get(context, "unregisterTool").is_some() {
                    let _ = call_method(context, "unregisterTool", &[JsValue::from_str(name)]);
                }
            }
        }
    });
}

fn status() -> JsValue {
    let found = find();
    let (registered, names): (bool, Vec<String>) =
        with(|s| (s.controller.is_some(), s.tools.iter().map(|(n, _)| n.clone()).collect())).unwrap_or_default();
    js::from_json(&json!({
        "supported": found.is_some(),
        "api": found.map(|(s, _)| s.name()),
        "registered": registered,
        "tools": names,
    }))
}

/// Builds the tools once the command schemas are in (the tool descriptions spell out each command's arguments, and
/// the module does not make the schemas itself), then registers them when the browser has WebMCP and the address
/// does not turn it off. False when the schemas could not be loaded: the page then has no tools.
async fn build(jobs: JobSender) -> bool {
    if let Err(why) = JsFuture::from(drafftink_app::web::schemas_ready()).await {
        log::warn!("WebMCP: no tools, because the command schemas could not be loaded: {why:?}");
        return false;
    }
    let service = Rc::new(Service::new(PageCaller { jobs }));
    let tools = service
        .catalog()
        .tools()
        .iter()
        .map(|tool| (tool.name.clone(), tool_object(&service, &tool.name, &descriptor(tool))))
        .collect();
    WEBMCP.with(|w| *w.borrow_mut() = Some(WebMcp { tools, controller: None }));
    let search = web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default();
    if wanted(&search) {
        register();
    }
    true
}

/// Builds `window.drafftink.webmcp`. Its `ready` promise resolves to true when the tools are built (and, when the
/// browser has WebMCP and the address does not turn it off, registered). `register()` waits for it and answers
/// with a promise of whether the tools are registered.
pub(crate) fn api(jobs: &JobSender) -> JsValue {
    let ready = future_to_promise({
        let jobs = jobs.clone();
        async move { Ok(JsValue::from_bool(build(jobs).await)) }
    });

    let object = Object::new();
    set(&object, "ready", &ready);
    let report = Closure::<dyn Fn() -> JsValue>::new(status);
    set(&object, "status", report.as_ref());
    report.forget();
    let on = Closure::<dyn Fn() -> JsValue>::new(move || {
        let ready = ready.clone();
        future_to_promise(async move {
            JsFuture::from(ready).await?;
            Ok(JsValue::from_bool(register()))
        })
        .into()
    });
    set(&object, "register", on.as_ref());
    on.forget();
    let off = Closure::<dyn Fn() -> JsValue>::new(|| {
        unregister();
        JsValue::UNDEFINED
    });
    set(&object, "unregister", off.as_ref());
    off.forget();

    object.into()
}
