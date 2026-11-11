//! Why a native e2e step failed.

use std::path::PathBuf;

use drafftink_bridge::client::ClientError;

/// An error of the native e2e support.
#[derive(Debug, thiserror::Error)]
pub enum E2eError {
    /// The app binary is missing.
    #[error(
        "the desktop app is not built: {0} does not exist. Run `cargo build --release -p drafftink`, or set DRAFFTINK_BIN"
    )]
    NoBinary(PathBuf),
    /// The app could not be started.
    #[error("could not start {path}: {source}")]
    Spawn {
        /// The binary.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// The app ended before its bridge was up.
    #[error("the app exited ({status}) before its bridge was up. Its log:\n{log}")]
    Exited {
        /// The exit status, as text.
        status: String,
        /// The tail of the app's stderr.
        log: String,
    },
    /// The bridge did not appear in time.
    #[error("the app's bridge did not come up in {seconds} s. Its log:\n{log}")]
    NoBridge {
        /// How long was waited.
        seconds: u64,
        /// The tail of the app's stderr.
        log: String,
    },
    /// The token file could not be used.
    #[error(transparent)]
    Token(#[from] drafftink_bridge::files::FindError),
    /// The bridge client failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// A file or directory operation failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The tokio runtime could not be built.
    #[error("could not build the async runtime: {0}")]
    Runtime(std::io::Error),
    /// A reply did not have the expected shape.
    #[error("unexpected reply: {0}")]
    Shape(String),
    /// This machine cannot run the native app, and `DRAFFTINK_REQUIRE_GPU` says it must.
    #[error("{0} (DRAFFTINK_REQUIRE_GPU is set, so this machine must be able to run the app)")]
    Required(String),
    /// A PNG could not be read.
    #[error("could not read the screenshot: {0}")]
    Png(String),
}
