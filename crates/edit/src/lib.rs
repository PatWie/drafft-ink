//! drafftink-edit (L4): the editor's domain rules, with typed errors.
//!
//! Commands in `engine` validate their arguments and delegate here. A rule lives in exactly one
//! module of this crate; commands and UI never keep a second copy.
//!
//! Every mutating function takes a transaction (`T: SceneTx`) so the caller decides the commit
//! boundary (one commit per command or batch). Where each rule lives:
//! - `binding`, `elbow`, `linear`: arrow bindings, elbow routing, line/arrow point editing.
//! - `arrow`: operations of the `arrow` commands that combine them (bind an end to a named target, reroute,
//!   snapping of a dragged point).
//! - `brush`: how the samples of a freehand stroke are captured (calligraphy brush, simulated pressure).
//! - `create`: the rules of `element.create`, `element.connect` and `element.set_name` (sizes, names, new
//!   elements of every kind, binding of new arrow ends, placement).
//! - `group`, `frame`, `zorder`, `arrange`, `transform`, `lifecycle`: groups, frames, z-order,
//!   align/distribute/flip, move/resize/rotate of selections, duplicate/delete/lock rules.
//! - `style`: style properties, applicability, defaults and themes (`style.*` commands).
//! - `text_bind`, `flowchart`, `layout`, `query`: text measuring and wrapping, bound text, font steps, formulas and
//!   colour runs; flowchart helpers, auto-layout for `diagram.create`, element queries (also by `@name`), hits
//!   along a path and the document outline.
//! - `library`: fragments (library items, merged files): take out, insert with new ids, tell apart.
//! - `paste`, `preflight`: pasting copies into a scene, and the checks that run before a structural change. The
//!   rule for copying elements as one block is in the private `copy` module; `lifecycle::duplicate` and `paste`
//!   both use it.
//!
//! Every rule reports a typed error. An element that is missing, deleted or locked is the shared
//! [`TargetError`], held as the `Target` variant of each error enum. The errors carry plain-words text and no
//! command names; the engine adds the advice that belongs to a command.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod arrange;
pub mod arrow;
pub mod binding;
pub mod brush;
mod copy;
pub mod create;
pub mod elbow;
pub mod flowchart;
pub mod frame;
pub mod group;
mod ids;
pub mod layout;
pub mod library;
pub mod lifecycle;
pub mod linear;
pub mod paste;
pub mod preflight;
pub mod query;
pub mod style;
mod target;
pub mod text_bind;
pub mod transform;
pub mod zorder;

pub use copy::CopyError;
pub use ids::unused_element_id;
pub use target::TargetError;
