//! drafftink-mcp: the MCP server for drafft.ink. `docs/agents.md` is the user guide.
//!
//! Three modes share one set of tools. The tools are generated from the command registry
//! ([`catalog::Catalog`]), run through [`service::Service`] on a [`backend::Backend`], and served by
//! [`server::DrafftHandler`] over stdio.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
// An app may read the clock and spawn threads; the rule against it is for the crates below L6.
#![allow(clippy::disallowed_methods)]

pub mod backend;
pub mod catalog;
pub mod cli;
pub mod guide;
pub mod outcome;
pub mod ports;
pub mod server;
pub mod service;
pub mod signature;
