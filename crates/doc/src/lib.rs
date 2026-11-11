//! drafftink-doc (L3): the Loro-backed [`SceneStore`](drafftink_scene::SceneStore).
//!
//! * [`LoroStore`] implements the scene crate's store contract on top of a Loro document. Local transactions
//!   write only the fields that changed, in one Loro commit. Remote updates are applied incrementally
//!   ([`LoroStore::apply_remote`]): Loro's diff events name the touched elements and only those are re-read.
//! * Undo and redo ([`LoroStore::undo`]) use Loro's `UndoManager`: one step per commit, this peer's edits only,
//!   with the observed editor state (selection, editing group) stored in each step.
//! * Export ([`LoroStore::export_updates_since`], [`LoroStore::export_snapshot`],
//!   [`LoroStore::export_shallow_snapshot`]) and history trimming ([`LoroStore::trim_history`]).
//! * The schema is in `docs/crdt.md`; its version and the migration hook are in [`schema`].
//!
//! Builds for wasm32. Hostile bytes return errors and never panic.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod codec;
pub mod error;
mod history;
pub mod schema;
mod store;
mod sync;
mod text;
mod tx;
mod undo;

pub use error::DocError;
pub use history::{HistoryStats, TrimOptions, TrimReport};
pub use schema::{MIGRATIONS, Migration, SCHEMA_VERSION};
pub use store::{LoroConfig, LoroStore, WorkStats};
pub use tx::LoroTx;
pub use undo::{ObservedState, UndoInfo, UndoStep};
