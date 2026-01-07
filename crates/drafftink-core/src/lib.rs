//! DrafftInk Core Library
//!
//! Platform-agnostic core data structures and logic for the DrafftInk whiteboard.

pub mod camera;
pub mod canvas;
pub mod collaboration;
pub mod crdt;
pub mod elbow;
pub mod input;
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
pub use input::InputState;
pub use selection::{ManipulationState, MultiMoveState};
pub use snap::{
    GRID_SIZE, SnapMode, SnapResult, SnapTarget, SnapTargetKind, snap_point, snap_to_grid,
};
pub use sync::{ConnectionState, PlatformWebSocket, SyncEvent};
pub use widget::{EditingKind, Handle, HandleKind, HandleShape, WidgetManager, WidgetState};
