//! What the launcher tells the app at start.

use drafftink_engine::cmd::collab::RoomLink;

/// A room to join at start: from the command line, or from the page address on the web.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupLink {
    /// The room and the server, as `RoomLink::parse` read them.
    pub link: RoomLink,
}

impl StartupLink {
    /// Reads `room` and `server` from a page address, a fragment or a query string.
    pub fn parse(text: &str) -> Self {
        Self { link: RoomLink::parse(text) }
    }

    /// True when there is a room to join.
    pub fn is_empty(&self) -> bool {
        self.link.room.is_none()
    }
}

/// Start-up choices of the app.
#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    /// Join this room after the workspace is restored.
    pub join: StartupLink,
    /// Server to use when the link names none (`/ws` on the web means "the server that served the page").
    pub default_server: Option<String>,
    /// A document to open at start (native: the file argument). Replaces the restored workspace's active tab.
    pub open: Option<drafftink_engine::ports::FileRef>,
    /// Show the performance overlay (frame times and redraw reasons) over the canvas.
    pub perf_overlay: bool,
    /// Make `doc.save` write to a stored name when there is no save target (the web has no save dialog).
    pub stored_default_target: bool,
}
