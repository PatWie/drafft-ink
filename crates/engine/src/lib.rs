//! drafftink-engine (L5): the command registry, the `Session`, `execute` / `execute_batch`, refusals,
//! the gesture protocol and the ports.
//!
//! Everything a user can do is a command here. Buttons, shortcuts, menus, the palette, scripts, MCP and
//! tests all call [`Session::execute`] or [`Session::execute_batch`]. `docs/engine.md` is the guide for
//! shells and for writing a family; `docs/commands.md` is the contract.
//!
//! | You want | Use |
//! |---|---|
//! | Run a command | [`Session::execute`], [`Session::execute_from`], [`Session::execute_typed`] |
//! | Run many as one undo step | [`Session::execute_batch`], [`Session::execute_batch_json`] |
//! | Drag, resize, rotate | [`Session::gesture_begin_typed`], [`Session::gesture_update`], [`Session::gesture_end_typed`] |
//! | Grey out a button | [`Session::availability`], [`Session::commands`] |
//! | Add a family | `cmd/<family>.rs`, see `docs/engine.md` |
//!
//! No UI, OS or GPU crate is used here: the outside world is the traits in [`ports`], with in-memory
//! fakes in [`ports::fake`]. `tests/layering.rs` checks it.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod args;
pub mod batch;
pub mod cmd;
mod executor;
mod fingerprint;
pub mod gesture;
pub mod interact;
mod invariants;
pub mod ports;
pub mod refusal;
pub mod registry;
mod remote;
mod remote_repair;
mod renders;
pub mod ring;
mod saves;
pub mod schemas;
pub mod services;
pub mod session;
pub mod snap;
pub mod spec;
pub mod state;
pub mod store;
pub mod testing;

#[cfg(test)]
mod catalog_tests;
#[cfg(test)]
mod exception_tests;
#[cfg(test)]
mod executor_tests;
#[cfg(test)]
mod invariants_tests;
#[cfg(test)]
mod keymap_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod snap_tests;

pub use batch::{BatchFailure, BatchOpts, BatchOutput, BatchStep};
pub use fingerprint::Fingerprint;
pub use gesture::GestureId;
pub use ports::{PortError, Ports};
pub use refusal::{Refusal, Suggestion};
pub use registry::{CommandInfo, Registry};
pub use renders::{Finished, PendingRender, RenderToken};
pub use ring::{Origin, Outcome, Ring, RingEntry};
pub use services::Services;
pub use session::{Ref, Session};
pub use spec::{CommandSpec, Facts, NoArgs, Typed};
pub use store::{DocStore, TabDoc, WorkStore};
