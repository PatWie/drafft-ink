//! The agent tools, shared by every way of serving them: stdio MCP (`apps/drafftink-mcp`, run by `drafftink mcp` and `drafftink-headless mcp`), MCP over HTTP in
//! the desktop app ([`crate::mcp`]) and WebMCP in the web page (`apps/drafftink-web`).
//!
//! The tool list is generated from the command registry ([`catalog::Catalog`]). A call is turned into bridge
//! methods and the answer into text and image blocks by [`service::Service`], which runs on a [`Caller`]: the
//! thing that answers `execute`, `batch`, `commands`, `describe`, `recent` and `state`. Nothing here touches a
//! socket or the MCP library, so all of it builds for `wasm32-unknown-unknown`.

pub mod catalog;
pub mod guide;
pub mod outcome;
pub mod service;
pub mod signature;

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

use crate::rpc::RpcError;

/// The mode a server runs in. The instructions of the `initialize` result and the `status` tool say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Own session, no window (`drafftink-headless mcp`).
    Headless,
    /// Connected to the desktop app over its bridge (`drafftink mcp`, `--attach`).
    Attached,
    /// Listening for the browser tab (`drafftink mcp --relay`).
    Relay,
    /// Served by the desktop app itself, over streamable HTTP (`drafftink --mcp-http`).
    Desktop,
    /// Registered by the web page itself with WebMCP (`navigator.modelContext`).
    Page,
}

impl Mode {
    /// The name used in `status` and in the docs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::Attached => "attached",
            Self::Relay => "relay",
            Self::Desktop => "desktop",
            Self::Page => "page",
        }
    }
}

/// A boxed future. It is `Send` where threads exist; the browser has one thread and its futures wait on
/// promises, which are not `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A boxed future. It is `Send` where threads exist; the browser has one thread and its futures wait on
/// promises, which are not `Send`.
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// What answers the bridge methods for a [`service::Service`]: an own session, the desktop app, a browser tab,
/// or the app that serves the tools itself.
#[cfg(not(target_arch = "wasm32"))]
pub trait Caller: Send + Sync {
    /// The mode.
    fn mode(&self) -> Mode;
    /// Runs one bridge method (`execute`, `batch`, `commands`, `describe`, `recent`, `state`).
    fn call<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>>;
    /// The answer of the `status` tool.
    fn status(&self) -> BoxFuture<'_, Value>;
}

/// What answers the bridge methods for a [`service::Service`]: an own session, the desktop app, a browser tab,
/// or the app that serves the tools itself.
#[cfg(target_arch = "wasm32")]
pub trait Caller {
    /// The mode.
    fn mode(&self) -> Mode;
    /// Runs one bridge method (`execute`, `batch`, `commands`, `describe`, `recent`, `state`).
    fn call<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>>;
    /// The answer of the `status` tool.
    fn status(&self) -> BoxFuture<'_, Value>;
}
