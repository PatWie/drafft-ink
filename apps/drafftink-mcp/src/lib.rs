//! drafftink-mcp: the MCP server for drafft.ink over stdio, as a library. `docs/agents.md` is the user guide.
//!
//! This crate has no binary. `drafftink mcp` (the desktop app's launcher) serves the two modes that talk to a
//! window ([`cli::BridgeArgs`], [`cli::run_bridge`]); `drafftink-headless mcp` serves those and the headless one
//! ([`cli::Args`], [`cli::run_server`]).
//!
//! Three modes share one set of tools. The tools are generated from the command registry and served by the
//! handler in `drafftink_bridge::tools` and `drafftink_bridge::mcp` (the desktop app serves the same ones over
//! HTTP). This crate holds what is specific to a separate process: the [`backend::Backend`] each mode runs on,
//! the command line and the file scope of the headless mode.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
// An app may read the clock and spawn threads; the rule against it is for the crates below L6.
#![allow(clippy::disallowed_methods)]

pub mod backend;
pub mod cli;
pub mod ports;
