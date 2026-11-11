//! drafftink-edit (L4): the editor's domain rules, with typed errors.
//!
//! Commands in `engine` validate their arguments and delegate here. A rule lives in exactly one
//! module of this crate; commands and UI never keep a second copy.
//!
//! Every mutating function takes a transaction (`T: SceneTx`) so the caller decides the commit
//! boundary (one commit per command or batch). Where each rule lives:
//! - `binding`, `elbow`, `linear`: arrow bindings, elbow routing, line/arrow point editing.
//! - `arrow_defaults`: the path type and heads a new arrow gets from the tool.
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
//! - `text_edit`: the model of the on-canvas text editor (text, colour runs, selection, motions, the editor's own
//!   undo, input-method composition). It owns no document data; the result goes back through `text.edit_end`.
//! - `link`: hyperlinks and element links (`element.set_link`, `element.open_link`).
//! - `library`: fragments (library items, merged files): take out, insert with new ids, tell apart.
//! - `paste`, `preflight`: pasting copies into a scene, and the checks that run before a structural change. The
//!   rule for copying elements as one block is in the private `copy` module; `lifecycle::duplicate` and `paste`
//!   both use it.
//! - `sticky`: the preset and fit of a sticky note (`element.create_sticky`).
//! - `repair`: the fixes for what a merge of concurrent remote edits leaves broken (dangling references, group
//!   nesting), as deterministic, idempotent functions.
//! - `convert`: changing the type of shapes, lines and arrows in place (`element.convert`).
//!
//! Every rule reports a typed error. An element that is missing, deleted or locked is the shared
//! [`TargetError`], held as the `Target` variant of each error enum. The errors carry plain-words text and no
//! command names; the engine adds the advice that belongs to a command.

#![warn(missing_docs, unreachable_pub)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod arrange;
pub mod arrow;
pub mod arrow_defaults;
pub mod binding;
pub mod brush;
pub mod convert;
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
pub mod link;
pub mod paste;
pub mod preflight;
pub mod query;
pub mod repair;
pub mod sticky;
pub mod style;
mod target;
pub mod text_bind;
pub mod text_edit;
pub mod transform;
pub mod zorder;

pub use copy::CopyError;
pub use ids::unused_element_id;
pub use target::TargetError;
