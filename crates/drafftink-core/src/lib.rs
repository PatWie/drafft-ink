//! DrafftInk Core Library
//!
//! Platform-agnostic core data structures and logic for the DrafftInk whiteboard.

pub mod camera;
pub mod canvas;
pub mod collaboration;
pub mod crdt;
pub mod elbow;
pub mod excalidraw;
pub mod input;
pub mod mermaid;
pub mod selection;
pub mod shapes;
pub mod snap;
pub mod storage;
pub mod sync;
pub mod tools;
pub mod widget;

pub use camera::Camera;
pub use canvas::Canvas;
pub use collaboration::CollaborationManager;
pub use crdt::CrdtDocument;
pub use excalidraw::{LibraryItem, library_from_excalidrawlib, library_layout_grid};
pub use input::InputState;
pub use mermaid::shapes_from_mermaid;
pub use selection::{ManipulationState, MultiMoveState};
pub use snap::{
    ENDPOINT_SNAP_RADIUS, EQUAL_SPACING_SNAP_RADIUS, GRID_SIZE, MULTI_MOVE_SNAP_RADIUS,
    SMART_GUIDE_THRESHOLD, SmartGuide, SmartGuideKind, SmartGuideResult, SnapResult,
    detect_smart_guides, detect_smart_guides_for_point, snap_point, snap_ray_to_smart_guides,
    snap_to_grid,
};
pub use sync::{ConnectionState, PlatformWebSocket, SyncEvent};
pub use widget::{EditingKind, Handle, HandleKind, HandleShape, WidgetManager, WidgetState};
