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

/// The access token for the collaboration server, as the launcher got it (a file or the environment, never the
/// command line). `Debug` prints a placeholder, so a logged [`AppConfig`] cannot leak it.
#[derive(Clone, PartialEq, Eq)]
pub struct StartupToken(String);

impl std::fmt::Debug for StartupToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StartupToken(<hidden>)")
    }
}

impl StartupToken {
    /// The token with surrounding white space removed; `None` when nothing is left.
    pub fn new(text: &str) -> Option<Self> {
        let text = text.trim();
        (!text.is_empty()).then(|| Self(text.to_owned()))
    }

    /// The token, to pass to `collab.connect`. Do not log it.
    fn expose(&self) -> &str {
        &self.0
    }
}

/// Start-up choices of the app.
#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    /// Join this room after the workspace is restored.
    pub join: StartupLink,
    /// Server to use when the link names none (`/ws` on the web means "the server that served the page").
    pub default_server: Option<String>,
    /// The access token for the server when the room is joined at start. Omitted when the server asks for none.
    pub collab_token: Option<StartupToken>,
    /// A document to open at start (native: the file argument). Replaces the restored workspace's active tab.
    pub open: Option<drafftink_engine::ports::FileRef>,
    /// Open the debug panel at start (`--perf-overlay`, `?perf=overlay`). It is the same panel as `Alt+D`.
    pub perf_overlay: bool,
    /// Make `doc.save` write to a stored name when there is no save target (the web has no save dialog).
    pub stored_default_target: bool,
}

impl AppConfig {
    /// The token as the argument of `collab.connect`.
    pub(crate) fn token_arg(&self) -> Option<&str> {
        self.collab_token.as_ref().map(StartupToken::expose)
    }
}
