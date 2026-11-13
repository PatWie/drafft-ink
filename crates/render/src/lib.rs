//! drafftink-render (L2): scene elements to Vello scenes, fast.
//!
//! The pieces, from the bottom:
//!
//! * [`ElementPainter`] encodes one element into a [`Fragment`], a `vello::Scene` in the element's local frame.
//! * [`FragmentCache`] keeps fragments by element id, bounded by bytes. A moved or rotated element reuses its
//!   fragment; a resize or restyle encodes it again.
//! * [`SceneComposer`] builds a frame: it culls with a spatial index of [`paint_bounds`], sorts the visible
//!   elements by z, appends their fragments with the view transform, clips frame children, and replaces tiny
//!   elements by cheap stand-ins.
//! * [`paint_overlay`] draws selection, handles, guides and the like into a scene of its own.
//! * [`paint_grid`] draws a grid whose cost is bounded at every zoom.
//! * [`replay`] reads a finished scene back into immediate-mode calls, for renderers other than Vello's GPU one
//!   (the WebGL2 fallback, a CPU rasteriser).
//! * [`compose_export`] and [`encode_png`] make the picture of a PNG export: the scene of a region, and the file.
//! * [`ImageCache`] holds decoded pictures and lets the host decode on a worker.
//!
//! This crate never creates a GPU device or a surface; the `gpu` crate renders the scenes. It builds for wasm32.
//! See `docs/render.md`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![warn(missing_docs)]

mod cache;
mod composer;
mod crop_chrome;
mod crop_dim;
mod export;
mod fragment;
mod frame_label;
mod grid;
mod hash;
mod images;
mod index;
mod options;
mod overlay;
mod overlay_marks;
mod overlay_peers;
mod overlay_snap;
mod overlay_text;
mod painter;
pub mod replay;
mod spec;
mod theme;
mod view;
mod zorder;

pub use cache::{CacheStats, DEFAULT_CACHE_BYTES, FragmentCache, Outcome};
pub use composer::{FrameReport, SceneComposer};
pub use crop_chrome::{CropChrome, CropHandleSize, paint_crop_chrome};
pub use crop_dim::{CropDim, paint_crop_dim};
pub use export::{ExportError, ExportSpec, Exported, MAX_SIDE, compose_export, encode_png};
pub use fragment::{Fragment, scene_bytes};
pub use grid::{
    DEFAULT_SPACING, GridSpec, GridStats, GridStyle, MAX_LINES, MAX_POINTS, MIN_CELL_PX, grid_spacing, paint_grid,
};
pub use hash::paint_hash;
pub use images::{
    DecodePolicy, DecodeRequest, ImageCache, ImageError, ImageState, ImageStats, MAX_DECODE_BYTES, MAX_IMAGE_SIDE,
    decode_image,
};
pub use index::{PaintIndex, build_paint_index, update_paint_index};
pub use options::{LodConfig, RenderOptions};
pub use overlay::{
    BindingHighlight, Overlay, OverlayReport, OverlayStyle, PointHandle, PointKind, RemoteCursor, SelectionBox,
    combine, paint_overlay,
};
pub use overlay_marks::LinkMark;
pub use overlay_peers::{LaserTrail, RemoteSelection, TrailPoint};
pub use overlay_snap::{AngleRays, EraserRing, RotationGuide};
pub use overlay_text::TextEdit;
pub use painter::{ElementPainter, FileSource};
pub use spec::{
    DEFAULT_HEAD_SIZE, FRAME_LABEL_GAP, FRAME_LABEL_MIN_WIDTH, FRAME_LABEL_ROOM, FRAME_LABEL_SIZE, Margin,
    element_to_world, head_size, math_spec, paint_bounds, paint_margin, sketch_input, text_spec,
};
pub use theme::Theme;
pub use view::View;

pub use vello::Scene;
