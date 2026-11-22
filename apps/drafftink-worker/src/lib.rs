//! The Rust core of the Cloudflare Worker host (design `docs/design/collab-e2e.md`, sections 14 and 19). The
//! TypeScript shell in this directory owns the sockets, the clock and the storage calls; the rules live in
//! `drafftink-relay`.
//!
//! - [`config`] reads the relay policy from the environment variables of the Worker.
//! - [`edge`] holds what the Worker decides before it calls a Durable Object: room id, picture name, membership
//!   token. It hashes 32 bytes and never reads a payload (the Free plan gives a Worker 10 ms of CPU).
//! - [`host`] turns the synchronous SQL calls of the shell into the store ports of the relay.
//! - [`driver`] is the relay as the shell calls it, with plain numbers and an [`driver::Effects`] list.
//! - `wasm` (on `wasm32` only) puts the JavaScript types on all of it with wasm-bindgen.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod config;
pub mod driver;
pub mod edge;
pub mod host;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod flow_tests;
#[cfg(target_arch = "wasm32")]
mod wasm;
