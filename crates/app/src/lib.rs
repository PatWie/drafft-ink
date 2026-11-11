//! drafftink-app (L7): the application glue shared by the desktop app and the web app.
//!
//! The shell (`drafftink-gpu`) owns the window, the GPU and the event loop. The engine owns the document
//! and the commands. This crate is the part in between: [`App`] implements the shell's `ShellApp`, owns
//! the `Session`, the interaction layer, the UI and the render caches, and turns shell events into
//! commands and commands into frames. What differs per platform (clipboard, storage, files, network,
//! pickers) comes in through a [`Platform`]; `native` (desktop) and `web` (wasm) build one.
//!
//! The rules the app keeps are in `docs/shell.md` (repaint rules) and `docs/engine.md` (the session API):
//! a frame is drawn only for a recorded reason, the scene is composed only when the document or the
//! camera changed, and every wake-up the app asks for is tied to pending work.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
// `Instant` and `SystemTime` here come from `web_time` (the clock port); clippy cannot tell them apart.
#![allow(clippy::disallowed_methods)]

mod app;
mod clip;
mod clock;
mod config;
mod agent_server;
mod editor;
mod input;
mod offscreen;
mod overlay;
mod perf;
mod platform;
mod startup;
mod view;

#[cfg(not(target_arch = "wasm32"))]
pub mod native;
#[cfg(target_arch = "wasm32")]
pub mod web;

pub use app::App;
pub use clock::AppClock;
pub use config::{AppConfig, StartupLink};
#[cfg(feature = "scenarios")]
pub use perf::scene_for;
pub use perf::{Look, SceneInstall, SceneLoader};
pub use platform::{
    FontFile, Host, HostJob, Inbound, Inbox, McpHttpControl, McpHttpInfo, NoHost, PickKind, PickedFile, Platform,
    SharedClipboard, WakeCell,
};
