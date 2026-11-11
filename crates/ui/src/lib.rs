//! drafftink-ui (L6): the egui user interface. It is thin on purpose.
//!
//! * It reads the command registry and the session through a [`Model`] snapshot.
//! * Every control builds a [`UiAction`]. [`action::route`] is the one function that maps an action to a
//!   command id and arguments, and [`action::dispatch`] executes them with `Session::execute_from`.
//! * One keymap dispatcher ([`keymap::resolve_keys`]) turns key events into the same actions, driven by
//!   the shortcuts registered on the commands.
//! * A frame returns every action of the frame, not the first.
//! * Nothing here changes the document, selection, tool, view or settings except by executing a command.
//!   A test (`tests/thin.rs`) scans this crate for calls that would.
//!
//! `docs/ui.md` describes the surfaces, the routing and what stays UI-only.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod action;
pub mod args_form;
pub mod colors;
pub mod icons;
pub mod keymap;
pub mod model;
pub mod shell;
pub mod state;
pub mod surfaces;
pub mod theme;
pub mod widgets;

pub use action::{Call, CommandSink, HostRequest, Report, Routed, Scrub, ScrubPhase, UiAction, Verb, dispatch, route};
pub use keymap::{Chord, Pointer, help_rows, parse_chord, resolve_keys};
pub use model::{
    AboutInfo, AgentServerView, BoundsView, CollabStatus, CollabView, Model, Peer, RoomLoading, TextFaces,
};
pub use shell::{FrameInput, FrameResult, UiShell, draw};
pub use state::{ColorTarget, LastPicked, Toast, UiState};
