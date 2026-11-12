//! # drafftink-core (deprecated)
//!
//! This crate has no code. Version 0.1.0 was the core of the first Drafft.ink app; the project was rewritten as a
//! set of smaller crates and this one was split up. Use the new crates instead:
//!
//! - [`drafftink-engine`](https://crates.io/crates/drafftink-engine): the editor, with its command registry and
//!   `Session`.
//! - [`drafftink-scene`](https://crates.io/crates/drafftink-scene): the element model and the scene store trait.
//! - [`drafftink-geom`](https://crates.io/crates/drafftink-geom) and
//!   [`drafftink-canvas`](https://crates.io/crates/drafftink-canvas): geometry, snapping and hit testing.
//! - [`drafftink-doc`](https://crates.io/crates/drafftink-doc),
//!   [`drafftink-collab`](https://crates.io/crates/drafftink-collab) and
//!   [`drafftink-protocol`](https://crates.io/crates/drafftink-protocol): the CRDT document and collaboration.
//! - [`drafftink-typeset`](https://crates.io/crates/drafftink-typeset): text and math layout.
//! - [`drafftink-io`](https://crates.io/crates/drafftink-io): file formats.
//! - [`drafftink-render`](https://crates.io/crates/drafftink-render) and
//!   [`drafftink-gpu`](https://crates.io/crates/drafftink-gpu): rendering.
//! - The desktop app: `cargo install drafftink`.
//!
//! The layering is described in the
//! [architecture notes](https://github.com/PatWie/drafft-ink/blob/main/docs/architecture.md).

#![forbid(unsafe_code)]
