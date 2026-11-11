//! `window.drafftink`: the page's scripting API.
//!
//! Every method returns a promise. The methods that talk to the document (`execute`, `batch`, `commands`,
//! `describe`, `recent`, `state`) queue a request for the app, which runs it between frames with origin
//! `agent`, exactly as the desktop bridge does. A refusal rejects the promise with `{code, message, data}`.

use drafftink_bridge::host::JobSender;
use drafftink_bridge::rpc::{INVALID_PARAMS, RpcError};
use js_sys::{Object, Reflect};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

use crate::{agent, js, webmcp_page};

/// A JS function with two optional arguments.
type Fn2 = Closure<dyn Fn(JsValue, JsValue) -> JsValue>;
type Fn0 = Closure<dyn Fn() -> JsValue>;

fn set(target: &Object, name: &str, value: &JsValue) {
    // Setting a plain property on a fresh object cannot fail.
    let _ = Reflect::set(target, &JsValue::from_str(name), value);
}

fn method2(target: &Object, name: &str, f: impl Fn(JsValue, JsValue) -> JsValue + 'static) {
    let closure: Fn2 = Closure::new(f);
    set(target, name, closure.as_ref());
    // The API lives as long as the page.
    closure.forget();
}

fn method0(target: &Object, name: &str, f: impl Fn() -> JsValue + 'static) {
    let closure: Fn0 = Closure::new(f);
    set(target, name, closure.as_ref());
    closure.forget();
}

fn bad(text: &str) -> JsValue {
    js::rejected(&RpcError::new(INVALID_PARAMS, text)).into()
}

/// Parameters from an optional options argument: `undefined` is `{}`; anything else must be an object.
fn options(value: &JsValue) -> Result<Value, RpcError> {
    match js::to_json(value)? {
        Value::Null => Ok(json!({})),
        v @ Value::Object(_) => Ok(v),
        _ => Err(RpcError::new(INVALID_PARAMS, "the options must be an object like {limit: 10}")),
    }
}

/// Builds `window.drafftink` and installs it.
pub(crate) fn install(jobs: &JobSender) -> Result<(), JsValue> {
    let api = Object::new();

    let j = jobs.clone();
    method2(&api, "execute", move |id, args| {
        let Some(id) = id.as_string() else {
            return bad("`execute` needs the command id as a string, like execute('element.create', {...})");
        };
        match js::to_json(&args) {
            Ok(args) => js::call(&j, "execute", json!({ "id": id, "args": args })).into(),
            Err(e) => js::rejected(&e).into(),
        }
    });

    let j = jobs.clone();
    method2(&api, "batch", move |steps, opts| {
        let (steps, opts) = match (js::to_json(&steps), js::to_json(&opts)) {
            (Ok(s), Ok(o)) => (s, o),
            (Err(e), _) | (_, Err(e)) => return js::rejected(&e).into(),
        };
        let mut params = json!({ "steps": steps });
        if let (Value::Object(map), false) = (&mut params, opts.is_null()) {
            map.insert("opts".to_owned(), opts);
        }
        js::call(&j, "batch", params).into()
    });

    let j = jobs.clone();
    method2(&api, "commands", move |opts, _| match options(&opts) {
        Ok(p) => js::call(&j, "commands", p).into(),
        Err(e) => js::rejected(&e).into(),
    });

    let j = jobs.clone();
    method2(&api, "describe", move |id, _| {
        let Some(id) = id.as_string() else {
            return bad("`describe` needs the command id as a string, like describe('element.create')");
        };
        js::call(&j, "describe", json!({ "id": id })).into()
    });

    let j = jobs.clone();
    method2(&api, "recent", move |opts, _| match options(&opts) {
        Ok(p) => js::call(&j, "recent", p).into(),
        Err(e) => js::rejected(&e).into(),
    });

    let j = jobs.clone();
    method0(&api, "state", move || js::call(&j, "state", json!({})).into());

    // Measurements for tests and for the curious. They read process-wide counters; nothing queues.
    method0(&api, "perf", || match serde_json::to_value(drafftink_profile::snapshot()) {
        Ok(v) => js::from_json(&v),
        Err(_) => JsValue::NULL,
    });
    method0(&api, "perf_reset", || {
        drafftink_profile::reset();
        JsValue::UNDEFINED
    });
    method2(&api, "perf_enable", |on, _| {
        drafftink_profile::set_enabled(on.is_truthy());
        JsValue::from_bool(drafftink_profile::enabled())
    });
    method0(&api, "frames", || {
        js::from_json(&json!({
            "frames": drafftink_gpu::frames_drawn(),
            "vello_runs": drafftink_gpu::vello_runs(),
        }))
    });

    set(&api, "agent", &agent::api(jobs));
    set(&api, "webmcp", &webmcp_page::api(jobs));
    set(&api, "version", &JsValue::from_str(env!("CARGO_PKG_VERSION")));

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    Reflect::set(&window, &JsValue::from_str("drafftink"), &api)?;
    Ok(())
}
