//! drafftink-canvas (L3): the reusable canvas library.
//!
//! Konva's ideas (a scene, a camera, events with bubbling, a transformer) on a spatial index with exact hit
//! testing, pointer gesture recognizers and a description of the overlay, over any [`SceneStore`](scene::SceneStore). It has no UI
//! toolkit, no clock, and no knowledge of commands: it turns input into typed [`Intent`]s and the editor above it
//! decides what they mean. A third party can use it alone with the in-memory store.
//!
//! | You want | Use |
//! |---|---|
//! | Everything wired together | [`Canvas`] |
//! | World/screen transforms, zoom, pan, fit | [`Viewport`], [`Camera`] |
//! | Which element is under the pointer, which are in a rectangle | [`SceneIndex`], [`HitOptions`], [`MarqueeMode`] |
//! | Pointer, touch and wheel input to intents | [`Recognizer`], [`InputEvent`], [`Intent`] |
//! | Handles for the selection | [`SelectionFrame`], [`TransformerConfig`] |
//! | What to draw above the scene | [`Overlay`], [`OverlaySink`] |
//! | Konva-style listeners with bubbling | [`Dispatcher`], [`Selector`], [`EventKind`] |
//!
//! `docs/canvas-api.md` is the guide, including the planned JS API.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod canvas;
pub mod config;
pub mod dispatch;
pub mod index;
pub mod input;
pub mod intent;
pub mod overlay;
pub mod recognizer;
pub mod shape;
pub mod transformer;
pub mod viewport;

/// The geometry crate this crate is built on (same `kurbo` version).
pub use drafftink_geom as geom;
/// The scene crate this crate is built on.
pub use drafftink_scene as scene;

pub use canvas::{Canvas, Dirty, StoreMut};
pub use config::{CanvasConfig, InputConfig, PointerMode};
pub use dispatch::{Dispatcher, ElementEvent, EventKind, Listener, ListenerId, Node, Selector, SelectorError};
pub use index::{
    ApplyStats, Hit, HitOptions, HitPart, IndexMismatch, MarqueeMode, SceneIndex, hit_element, unit_group, z_cmp,
};
pub use input::{
    Button, Buttons, InputEvent, Modifiers, PointerEvent, PointerId, PointerKind, PointerPhase, Tilt, WheelEvent,
    WheelUnit,
};
pub use intent::{Cursor, Handle, Intent, Meta, Pos, Target, ViewSource};
pub use overlay::{
    BindCandidate, HoverOutline, MarqueeOverlay, Overlay, OverlayInputs, OverlaySink, RemoteCursor, RemoteSelection,
    RemoteUser, SelectionBox, TransformerOverlay,
};
pub use recognizer::{Picker, Recognizer};
pub use transformer::{HandlePos, SelectionFrame, TransformerConfig, handle_set_for};
pub use viewport::{Camera, MAX_ZOOM, MIN_ZOOM, Viewport, clamp_zoom};
