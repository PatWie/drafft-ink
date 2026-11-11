//! End-to-end support for the native app (`docs/testing.md`, "Browser and native e2e").
//!
//! [`McpHttpApp`] starts the same app with MCP over HTTP instead, for an MCP client.
//!
//! [`NativeApp`] starts `apps/drafftink` with `--bridge` on whatever X display the environment names, finds it
//! through the token file as an agent does, and talks to it with the bridge's own client. A test drives the
//! app through [`NativeApp::exec`] and [`NativeApp::batch`] only: the same commands, refusals and undo steps a
//! user's buttons and an agent's calls reach.
//!
//! No display or no GPU is a *skip* with a message ([`Launch::Skipped`]); `DRAFFTINK_REQUIRE_GPU=1` turns the
//! skip into a failure, for the machine that is supposed to have both.
//!
//! The tests of this crate read the wall clock to wait for the app, so the crate opts out of the
//! `disallowed-methods` rule that protects wasm targets.
#![allow(clippy::disallowed_methods)]

mod app;
mod error;
mod image;

pub use app::{Launch, McpHttpApp, NativeApp, Refusal, require_gpu, start_mcp_http_or_skip, start_or_skip};
pub use error::E2eError;
pub use image::{Rgb, Screenshot, close, hex};
