//! Where the desktop app keeps its files, by platform convention.

use std::path::PathBuf;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The folder for settings, documents, the workspace and the library:
/// `$XDG_DATA_HOME/drafftink` or `~/.local/share/drafftink` (Linux), `~/Library/Application Support/drafftink`
/// (macOS), `%APPDATA%\drafftink` (Windows). `DRAFFTINK_DATA_DIR` overrides it.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = env_path("DRAFFTINK_DATA_DIR") {
        return Some(dir);
    }
    if cfg!(target_os = "windows") {
        return env_path("APPDATA").map(|p| p.join("drafftink"));
    }
    let home = env_path("HOME")?;
    if cfg!(target_os = "macos") {
        return Some(home.join("Library").join("Application Support").join("drafftink"));
    }
    Some(env_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share")).join("drafftink"))
}

/// The folder of the agent bridge's token files (`docs/agents.md` section 5.3). Same rule as the bridge.
pub fn bridge_dir() -> Option<PathBuf> {
    drafftink_bridge::files::default_dir()
}
