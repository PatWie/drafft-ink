//! JSON-RPC 2.0 messages, one per WebSocket text frame.
//!
//! Only single messages are accepted (no batch arrays): a frame is one request, notification or response.
//! Parsing is strict about shape and lenient about nothing else, and it never panics on hostile input.

use serde_json::{Value, json};

/// The frame is not JSON.
pub const PARSE_ERROR: i64 = -32700;
/// The JSON is not a valid JSON-RPC message.
pub const INVALID_REQUEST: i64 = -32600;
/// No such method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The parameters do not fit the method.
pub const INVALID_PARAMS: i64 = -32602;
/// The bridge failed.
pub const INTERNAL_ERROR: i64 = -32603;
/// The token was wrong.
pub const UNAUTHORIZED: i64 = -32001;
/// The app did not answer in time. The request is not cancelled.
pub const TIMED_OUT: i64 = -32002;
/// A call before `hello`, or a second `hello`.
pub const NOT_AUTHENTICATED: i64 = -32003;
/// Too many requests are waiting.
pub const BUSY: i64 = -32004;
/// The command refused. `message` is the refusal's plain text, `data` the whole refusal.
pub const REFUSED: i64 = -32010;

/// An error object.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    /// The code; see the constants of this module.
    pub code: i64,
    /// Plain words.
    pub message: String,
    /// Extra data. For [`REFUSED`] this is the refusal: `code`, `command`, `message`, `suggest`, `details`.
    pub data: Option<Value>,
}

impl RpcError {
    /// An error without data.
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    /// An error with data.
    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// The error object as JSON.
    pub fn to_value(&self) -> Value {
        let mut v = json!({ "code": self.code, "message": self.message });
        if let (Some(d), Some(m)) = (&self.data, v.as_object_mut()) {
            m.insert("data".to_owned(), d.clone());
        }
        v
    }

    /// Reads an error object. A malformed object becomes an internal error that says so.
    pub fn from_value(v: &Value) -> Self {
        let code = v.get("code").and_then(Value::as_i64).unwrap_or(INTERNAL_ERROR);
        let message = v.get("message").and_then(Value::as_str).unwrap_or("the peer sent an error without a message");
        Self { code, message: message.to_owned(), data: v.get("data").cloned() }
    }

    /// True when this is a command refusal.
    pub fn is_refusal(&self) -> bool {
        self.code == REFUSED
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RpcError {}

/// One incoming message.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A call that wants an answer.
    Request {
        /// The id to echo.
        id: Value,
        /// The method.
        method: String,
        /// The parameters (`{}` when absent).
        params: Value,
    },
    /// A call without an id: no answer is sent.
    Notification {
        /// The method.
        method: String,
        /// The parameters (`{}` when absent).
        params: Value,
    },
    /// An answer to a request we sent.
    Response {
        /// The id of the request.
        id: Value,
        /// The result or the error.
        outcome: Result<Value, RpcError>,
    },
}

/// A message that could not be read. `id` is the request id when it could be found, so the answer can echo it.
#[derive(Debug, Clone, PartialEq)]
pub struct Malformed {
    /// The id, if the frame had a usable one.
    pub id: Option<Value>,
    /// The error to send back.
    pub error: RpcError,
}

fn usable_id(v: &Value) -> bool {
    matches!(v, Value::String(_) | Value::Number(_))
}

/// Parses one frame.
pub fn parse(text: &str) -> Result<Incoming, Malformed> {
    let bad = |id: Option<Value>, code: i64, msg: &str| Malformed { id, error: RpcError::new(code, msg) };
    let value: Value = serde_json::from_str(text).map_err(|e| {
        bad(None, PARSE_ERROR, &format!("the frame is not valid JSON ({e}). Send one JSON-RPC 2.0 object per frame"))
    })?;
    let Some(obj) = value.as_object() else {
        return Err(bad(
            None,
            INVALID_REQUEST,
            "a frame must be one JSON object. Batch arrays are not supported: send one message per frame",
        ));
    };
    let id = obj.get("id").cloned();
    let echo_id = id.clone().filter(usable_id);
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(bad(echo_id, INVALID_REQUEST, "`jsonrpc` must be the string \"2.0\""));
    }
    if let Some(method) = obj.get("method") {
        let Some(method) = method.as_str() else {
            return Err(bad(echo_id, INVALID_REQUEST, "`method` must be a string"));
        };
        let params = match obj.get("params") {
            None | Some(Value::Null) => json!({}),
            Some(p) if p.is_object() => p.clone(),
            Some(_) => {
                return Err(bad(echo_id, INVALID_PARAMS, "`params` must be an object (by-name parameters only)"));
            }
        };
        return match id {
            None => Ok(Incoming::Notification { method: method.to_owned(), params }),
            Some(i) if usable_id(&i) => Ok(Incoming::Request { id: i, method: method.to_owned(), params }),
            Some(_) => Err(bad(None, INVALID_REQUEST, "`id` must be a string or a number")),
        };
    }
    match (id, obj.get("result"), obj.get("error")) {
        (Some(i), Some(r), None) if usable_id(&i) => Ok(Incoming::Response { id: i, outcome: Ok(r.clone()) }),
        // An error may carry a null id when the request id could not be read.
        (Some(i), None, Some(e)) if usable_id(&i) || i.is_null() => {
            Ok(Incoming::Response { id: i, outcome: Err(RpcError::from_value(e)) })
        }
        _ => Err(bad(echo_id, INVALID_REQUEST, "the frame has neither `method` nor a valid `result` or `error`")),
    }
}

/// A success response.
pub fn response_ok(id: &Value, result: &Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// An error response. `id` is `null` when the request id was unreadable.
pub fn response_err(id: Option<&Value>, error: &RpcError) -> String {
    json!({ "jsonrpc": "2.0", "id": id.cloned().unwrap_or(Value::Null), "error": error.to_value() }).to_string()
}

/// A request frame.
pub fn request_text(id: &Value, method: &str, params: &Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// A notification frame.
pub fn notification_text(method: &str, params: &Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests_notifications_and_responses() {
        let r = parse(r#"{"jsonrpc":"2.0","id":1,"method":"execute","params":{"id":"view.get"}}"#).unwrap();
        assert!(matches!(r, Incoming::Request { ref method, .. } if method == "execute"));
        let n = parse(r#"{"jsonrpc":"2.0","method":"changed"}"#).unwrap();
        assert_eq!(n, Incoming::Notification { method: "changed".into(), params: json!({}) });
        let ok = parse(r#"{"jsonrpc":"2.0","id":"a","result":{"x":1}}"#).unwrap();
        assert_eq!(ok, Incoming::Response { id: json!("a"), outcome: Ok(json!({"x":1})) });
        let err = parse(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32010,"message":"no"}}"#).unwrap();
        assert!(matches!(err, Incoming::Response { outcome: Err(ref e), .. } if e.is_refusal()));
    }

    #[test]
    fn hostile_frames_are_errors_not_panics() {
        for t in [
            "",
            "{",
            "[]",
            "[{}]",
            "null",
            "1",
            "\"x\"",
            r#"{"jsonrpc":"1.0"}"#,
            r#"{"jsonrpc":"2.0"}"#,
            r#"{"jsonrpc":"2.0","method":5}"#,
            r#"{"jsonrpc":"2.0","id":null,"method":"x"}"#,
            r#"{"jsonrpc":"2.0","id":{},"method":"x"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"x","params":[1]}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":1,"error":{}}"#,
        ] {
            assert!(parse(t).is_err(), "{t}");
        }
    }

    #[test]
    fn a_bad_request_keeps_its_id_for_the_answer() {
        let m = parse(r#"{"jsonrpc":"2.0","id":7,"method":"x","params":[1]}"#).unwrap_err();
        assert_eq!(m.id, Some(json!(7)));
        assert_eq!(m.error.code, INVALID_PARAMS);
    }
}
