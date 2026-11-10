//! drafftink-edit (L4): the editor's domain rules, with typed errors.
//!
//! Commands in `engine` validate their arguments and delegate here. A rule lives in exactly one
//! module of this crate; commands and UI never keep a second copy.
//!
//! Every mutating function takes a transaction (`T: SceneTx`) so the caller decides the commit
//! boundary (one commit per command or batch). Module ownership (one agent each while porting):
//! - `binding`, `elbow`, `linear`: arrow bindings, elbow routing, line/arrow point editing.
//! - `arrow`: operations of the `arrow` commands that combine them (bind an end to a named target, reroute,
//!   snapping of a dragged point).
//! - `group`, `frame`, `zorder`, `arrange`, `transform`, `lifecycle`: groups, frames, z-order,
//!   align/distribute/flip, move/resize/rotate of selections, duplicate/delete/lock rules.
//! - `style`: style properties, applicability, defaults and themes (`style.*` commands).
//! - `text_bind`, `flowchart`, `layout`, `query`: bound text, flowchart helpers, auto-layout for
//!   `diagram.create`, element queries and the document outline.

pub mod arrange;
pub mod arrow;
pub mod binding;
pub mod elbow;
pub mod flowchart;
pub mod frame;
pub mod group;
pub mod layout;
pub mod lifecycle;
pub mod paste;
pub mod linear;
pub mod preflight;
pub mod query;
pub mod style;
pub mod text_bind;
pub mod transform;
pub mod zorder;
