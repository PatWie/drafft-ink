//! The bridge's methods, transport-neutral. [`dispatch`] runs one call against a [`Session`] with the origin
//! `agent`. The native server, the headless MCP server and the web page all use it, so they answer alike.
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | `execute` | `{id, args?}` | the command's output (`changed`, `created`, ...) |
//! | `batch` | `{steps: [{id, args?, as?}], opts?: {label?, atomic?, dry_run?}}` | the batch output; `failed` is set when a step refused |
//! | `commands` | `{family?, search?, available_only?, include_schemas?}` | the registry with live availability |
//! | `describe` | `{id}` | schemas, facts, example, refusals of one command |
//! | `recent` | `{limit?, mutating_only?, since_seq?}` | the newest ring entries |
//! | `state` | `{}` | `engine.state` |
//! | `ping` | `{}` | `{}` |
//!
//! `hello`, `subscribe` and `unsubscribe` belong to the transport (they are about the connection, not the
//! document). A command refusal is a JSON-RPC error with code [`REFUSED`] whose `message` is the plain text.

use drafftink_engine::{Origin, Refusal, Registry, Session};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::rpc::{INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND, REFUSED, RpcError};

/// The version of this protocol. It rises when a method changes incompatibly.
pub const PROTOCOL: u32 = 1;

/// Methods that run against the session.
pub const SESSION_METHODS: &[&str] = &["execute", "batch", "commands", "describe", "recent", "state", "ping"];

/// A refusal as a JSON-RPC error.
pub fn refusal_error(r: &Refusal) -> RpcError {
    RpcError { code: REFUSED, message: r.message.clone(), data: serde_json::to_value(r).ok() }
}

/// A short fingerprint of the command ids of a registry. Peers compare it to notice that they were built
/// from different registries.
pub fn registry_digest(reg: &Registry) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for spec in reg.all() {
        for b in spec.id.bytes().chain(std::iter::once(b'\n')) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// The answer to `hello`.
pub fn hello_result(reg: &Registry, max_frame_bytes: usize) -> Value {
    json!({
        "protocol": PROTOCOL,
        "server": "drafftink",
        "version": env!("CARGO_PKG_VERSION"),
        "origin": Origin::Agent.name(),
        "registry": { "count": reg.len(), "digest": registry_digest(reg) },
        "methods": ["hello", "execute", "batch", "commands", "describe", "recent", "state", "ping", "subscribe", "unsubscribe"],
        "limits": { "max_frame_bytes": max_frame_bytes },
    })
}

fn invalid(method: &str, why: impl std::fmt::Display, shape: &str) -> RpcError {
    RpcError::new(INVALID_PARAMS, format!("invalid params for `{method}`: {why}. Expected {shape}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteParams {
    id: String,
    #[serde(default)]
    args: Value,
}

/// Commands that only make sense for a person at the mouse or keyboard are not offered to agents.
fn ensure_agent(s: &Session, id: &str) -> Result<(), Refusal> {
    match s.spec(id) {
        Some(spec) if !spec.agent => Err(Refusal::custom(
            "not_for_agents",
            spec.id,
            format!(
                "`{}` is not available to agents: it exists for a person using the mouse or keyboard. List what you can use with `commands`.",
                spec.id
            ),
        )),
        _ => Ok(()),
    }
}

fn run(s: &mut Session, id: &str, args: Value) -> Result<Value, RpcError> {
    ensure_agent(s, id).map_err(|r| refusal_error(&r))?;
    s.execute_from(Origin::Agent, id, args).map_err(|r| refusal_error(&r))
}

/// Runs one call. Never panics; a hostile parameter is a refusal or an `invalid params` error.
pub fn dispatch(s: &mut Session, method: &str, params: &Value) -> Result<Value, RpcError> {
    match method {
        "ping" => Ok(json!({})),
        "execute" => {
            let p: ExecuteParams = serde_json::from_value(params.clone())
                .map_err(|e| invalid(method, e, r#"{"id": "<command id>", "args": {...}}"#))?;
            let args = if p.args.is_null() { json!({}) } else { p.args };
            run(s, &p.id, args)
        }
        "batch" => {
            if let Some(steps) = params.get("steps").and_then(Value::as_array) {
                for step in steps {
                    if let Some(id) = step.get("id").and_then(Value::as_str) {
                        ensure_agent(s, id).map_err(|r| refusal_error(&r))?;
                    }
                }
            }
            s.execute_batch_json(Origin::Agent, params).map_err(|r| refusal_error(&r))
        }
        "commands" => run(s, "engine.commands", params.clone()),
        "describe" => run(s, "engine.describe", params.clone()),
        "recent" => run(s, "engine.recent", params.clone()),
        "state" => run(s, "engine.state", json!({})),
        other => Err(RpcError::new(
            METHOD_NOT_FOUND,
            format!("unknown method `{other}`. Methods: hello, execute, batch, commands, describe, recent, state, ping, subscribe, unsubscribe"),
        )),
    }
}

/// Parses one frame, runs it and returns the frame to send back, if any. This is the whole bridge for a
/// peer that has no authentication step of its own (the web page answering the relay, a test).
pub fn handle_text(s: &mut Session, frame: &str) -> Option<String> {
    use crate::rpc::{Incoming, parse, response_err, response_ok};
    match parse(frame) {
        Err(m) => Some(response_err(m.id.as_ref(), &m.error)),
        Ok(Incoming::Request { id, method, params }) => Some(match dispatch(s, &method, &params) {
            Ok(v) => response_ok(&id, &v),
            Err(e) => response_err(Some(&id), &e),
        }),
        Ok(_) => None,
    }
}

/// Internal failure helper for transports.
pub fn internal(why: impl Into<String>) -> RpcError {
    RpcError::new(INTERNAL_ERROR, why)
}
