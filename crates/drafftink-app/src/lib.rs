//! DrafftInk Application
//!
//! The main application shell providing windowing, input handling,
//! and integration of all components.

mod app;
mod event_handler;
mod shortcuts;
mod ui;

pub use app::{App, AppConfig};
pub use shortcuts::{Shortcut, ShortcutRegistry};
pub use ui::{UiAction, UiState, render_ui};

#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
pub use web::run_wasm;
