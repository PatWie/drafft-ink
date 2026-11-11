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
//! - `text_bind`, `text_cmd`, `flowchart`, `layout`, `query`: bound text and text editing, flowchart
//!   helpers, auto-layout for `diagram.create`, element queries and the document outline.
//! - `link`: hyperlinks and element links (`element.set_link`, `element.open_link`).
//! - `library`: fragments (library items, merged files): take out, insert with new ids, tell apart.
//! - `paste`, `preflight`: pasting copies into a scene, and the checks that run before a structural change.
//! - `element_cmd`, `selection_cmd`, `query_cmd`, `text_cmd`: rules the `element.*`, `selection.*` and
//!   `text.*` commands need that no domain module above owns (common bounds, eraser hits, picking many targets,
//!   references by `@name`).

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod arrange;
pub mod arrow;
pub mod binding;
pub mod brush;
pub mod create;
pub mod elbow;
pub mod element_cmd;
pub mod flowchart;
pub mod frame;
pub mod group;
mod ids;
pub mod layout;
pub mod library;
pub mod lifecycle;
pub mod linear;
pub mod link;
pub mod paste;
pub mod preflight;
pub mod query;
pub mod query_cmd;
pub mod selection_cmd;
pub mod style;
pub mod text_bind;
pub mod text_cmd;
pub mod transform;
pub mod zorder;
