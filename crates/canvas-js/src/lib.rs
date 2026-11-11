//! drafftink-canvas-js: the canvas library for JavaScript.
//!
//! `drafftink-canvas` (L3) is a Rust library: a spatial index, exact hit testing, gesture recognizers, a transformer
//! and Konva-style events over any scene store. This crate puts it in a browser. It adds what a JS developer
//! expects from a canvas library and the canvas crate does not have: a WebGPU surface on an `HTMLCanvasElement`,
//! a typed shape model with CRUD and transactions, built-in selecting, dragging and transforming, and export to
//! JSON, SVG and PNG. The npm package `drafftink-canvas` (`packages/drafftink-canvas`) wraps the wasm module.
//!
//! The crate is split so that everything but the browser glue runs and is tested natively:
//!
//! | Module | Role |
//! |---|---|
//! | [`core`] | [`Core`]: canvas, store, typesetter, listeners, camera, queries |
//! | `ops` | the write operations and their batches |
//! | `image`, `remote` | adding a picture from bytes; collaborators (pointer, selection, trail) |
//! | `interact` | select, drag, transform and rubber band from the canvas crate's intents |
//! | `frame`, `overlay` | the CPU half of a frame: the store and the overlay as a Vello scene |
//! | `export`, `load` | JSON, SVG and PNG out; the formats that come in |
//! | `target` | the GPU half: a surface (browser) or an offscreen image |
//! | `web` | wasm-bindgen: `Stage`, DOM glue (wasm32 only) |
//!
//! It depends on `canvas`, `edit`, `gpu`, `io`, `render`, `scene` and `typeset` and nothing depends on it, so it is
//! an app-class crate. It uses the in-memory store: no Loro, no `doc` crate. `docs/canvas-api.md` is the guide.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod core;
mod dto;
mod error;
mod export;
mod fonts;
mod frame;
mod image;
mod interact;
mod json;
mod load;
mod ops;
mod options;
mod overlay;
mod remote;
mod target;
#[cfg(target_arch = "wasm32")]
mod web;

pub use core::Core;

pub use dto::{CameraDto, EventDto, HitDto, OverlayDto, event_name, parse_event_kind};
pub use error::ApiError;
pub use export::{PngOptions, PngPlan, SvgExport, SvgLook};
pub use image::ImageSpec;
pub use options::{GridKind, GridOptions, InteractionOptions, MarqueeOption, StageOptions, ThemeOption};
pub use remote::{RemoteUserSpec, TrailSpec};
pub use target::Renderer;
