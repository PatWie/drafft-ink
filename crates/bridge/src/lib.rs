//! drafftink-bridge (L6): the agent bridge.
//!
//! An agent drives the editor through the same `Session::execute` as a button does. This crate carries the
//! calls. See `docs/agents.md` for the protocol, the security model and the setup.
//!
//! * [`rpc`], [`core`], [`host`], [`guard`]: JSON-RPC 2.0 messages, the methods, the queue the shell drains
//!   and the token, `Host` and `Origin` checks. They hold no socket and build for every target, so the web
//!   page can answer the relay with the same [`core::dispatch`].
//! * [`tools`]: the agent tools, generated from the registry and run on any [`tools::Caller`]. Builds everywhere:
//!   the web page registers the same tools with WebMCP.
//! * [`mcp`] (native): the `rmcp` handler for the tools, and MCP over streamable HTTP for the desktop app.
//! * [`server`] (native): the loopback WebSocket server of the desktop app.
//! * [`client`] (native): a small client for the server, used by the MCP app in attached mode and by tests.
//! * [`relay`] (native): the server that the web page connects out to, and the fake page that plays the
//!   page's side in tests.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
// The native modules read the wall clock for deadlines. Nothing below L6 does.
#![allow(clippy::disallowed_methods)]

pub mod core;
pub mod guard;
pub mod host;
pub mod rpc;
pub mod tools;

#[cfg(not(target_arch = "wasm32"))]
pub mod client;
#[cfg(not(target_arch = "wasm32"))]
pub mod files;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp;
#[cfg(not(target_arch = "wasm32"))]
pub mod page_sim;
#[cfg(not(target_arch = "wasm32"))]
pub mod relay;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
#[cfg(not(target_arch = "wasm32"))]
pub mod shell_sim;
#[cfg(not(target_arch = "wasm32"))]
mod ws;

pub use host::{BridgeHost, Polled, Waker};
pub use rpc::RpcError;
#[cfg(not(target_arch = "wasm32"))]
pub use server::{BridgeConfig, BridgeError, BridgeServer};
