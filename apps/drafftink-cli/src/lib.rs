//! drafftink-cli: the headless command line of drafft.ink, as a library.
//!
//! This crate has no binary. `drafftink-headless` is the program: it flattens [`cli::Command`] into its own command
//! line and calls [`cli::run_command`].
//!
//! It owns an editor [`Session`](drafftink_engine::Session) without a window and drives it through the same
//! `execute` and `execute_batch` as every other door, so a script that runs here does the same in the app, over
//! MCP and in a test. Four things are added around the engine and nothing else: reading and writing files
//! ([`ports`]), reading a script ([`script`]), choosing a file format ([`format`]), and printing ([`cli`]).
//!
//! PNG needs a renderer. The `png` feature adds one that draws on the CPU ([`raster`]), so a server or a CI job
//! with no GPU and no display can write PNG. Without it `doc.export` as PNG refuses here and says so. SVG, JSON
//! and Excalidraw always work.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
// A command line tool may read the clock; the rule against it is for the crates below L6.
#![allow(clippy::disallowed_methods)]

pub mod cli;
pub mod error;
pub mod format;
pub mod ops;
pub mod ports;
#[cfg(feature = "png")]
pub mod raster;
pub mod script;
