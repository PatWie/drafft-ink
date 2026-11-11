//! JSON between JavaScript values and `serde_json`, and the promises that wait for the app.
//!
//! A request to the app goes through the bridge queue and is answered between frames. The reply closure must
//! be `Send`, but a promise's `resolve` function is not, so the functions wait in a table on this thread and
//! the closure carries only a number. (The page has one thread, so the closure always runs where the table is.)

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use drafftink_bridge::host::{Job, JobSender};
use drafftink_bridge::rpc::{INTERNAL_ERROR, INVALID_PARAMS, RpcError};
use js_sys::{Function, Promise};
use serde_json::Value;
use wasm_bindgen::prelude::*;

/// A JS value as JSON. `undefined` and `null` are `null`.
pub(crate) fn to_json(value: &JsValue) -> Result<Value, RpcError> {
    if value.is_undefined() || value.is_null() {
        return Ok(Value::Null);
    }
    let text = js_sys::JSON::stringify(value).ok().and_then(|s| s.as_string()).ok_or_else(|| {
        RpcError::new(INVALID_PARAMS, "the argument cannot be turned into JSON (a function, a cycle or a BigInt?)")
    })?;
    serde_json::from_str(&text)
        .map_err(|e| RpcError::new(INVALID_PARAMS, format!("the argument is not valid JSON ({e})")))
}

/// JSON as a JS value.
pub(crate) fn from_json(value: &Value) -> JsValue {
    js_sys::JSON::parse(&value.to_string()).unwrap_or(JsValue::NULL)
}

/// The object a rejected promise carries: `{code, message, data?}`.
pub(crate) fn error_object(error: &RpcError) -> JsValue {
    from_json(&error.to_value())
}

/// A promise that is already rejected with `error`.
pub(crate) fn rejected(error: &RpcError) -> Promise {
    Promise::reject(&error_object(error))
}

type Settle = (Function, Function);

thread_local! {
    static WAITING: RefCell<HashMap<u64, Settle>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u64> = const { Cell::new(1) };
}

fn settle(id: u64, outcome: Result<Value, RpcError>) {
    let Some((resolve, reject)) = WAITING.with(|w| w.borrow_mut().remove(&id)) else { return };
    // A throwing `then` handler is the caller's problem; there is nobody to tell.
    let _ = match outcome {
        Ok(value) => resolve.call1(&JsValue::NULL, &from_json(&value)),
        Err(error) => reject.call1(&JsValue::NULL, &error_object(&error)),
    };
}

/// A promise for `next` once the command schemas are in. If they could not be loaded, it rejects with that
/// reason as an error object, and `next` does not run.
pub(crate) fn after_schemas(next: impl FnOnce() -> Promise + 'static) -> Promise {
    wasm_bindgen_futures::future_to_promise(async move {
        let loaded = wasm_bindgen_futures::JsFuture::from(drafftink_app::web::schemas_ready()).await;
        if let Err(why) = loaded {
            let why = why.as_string().unwrap_or_else(|| "unknown reason".to_owned());
            return Err(error_object(&RpcError::new(
                INTERNAL_ERROR,
                format!("the argument schemas of the commands could not be loaded ({why}). Reload the page."),
            )));
        }
        wasm_bindgen_futures::JsFuture::from(next()).await
    })
}

/// Queues `method` for the app and returns a promise for its answer. The app runs it between frames.
pub(crate) fn call(jobs: &JobSender, method: &str, params: Value) -> Promise {
    let mut once = Some((method.to_owned(), params));
    Promise::new(&mut |resolve, reject| {
        let Some((method, params)) = once.take() else { return };
        let id = NEXT.with(|n| {
            let id = n.get();
            n.set(id.wrapping_add(1));
            id
        });
        WAITING.with(|w| w.borrow_mut().insert(id, (resolve, reject)));
        let job = Job { method, params, reply: Box::new(move |outcome| settle(id, outcome)) };
        if let Err((error, reply)) = jobs.submit(job) {
            reply(Err(error));
        }
    })
}
