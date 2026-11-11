//! The list of pictures to render, read from `docs/media/shots.json`.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

/// Why the list could not be read.
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("cannot read {path}: {source}")]
    Read { path: String, source: std::io::Error },
    #[error("{path} is not a valid shot list: {source}")]
    Parse { path: String, source: serde_json::Error },
}

/// The theme of the app in a shot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    /// The value of the `theme` setting.
    pub fn setting(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// One command to run on the session before the picture is taken.
#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    pub id: String,
    #[serde(default)]
    pub args: Value,
}

/// What kind of picture a shot is, and its settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Output {
    /// The whole window: the UI over the canvas, as the app draws it.
    Window {
        /// Window size in logical pixels.
        size: [f32; 2],
        /// Physical pixels per logical pixel; the PNG is `size * scale`.
        scale: f32,
        theme: Theme,
        /// Start in the touch layout (a phone or a tablet).
        #[serde(default)]
        touch: bool,
    },
    /// The document alone, drawn by the PNG export (`doc.export`).
    Export {
        /// Width of the PNG in pixels; the export scale is chosen to reach it.
        width: u32,
        /// World units kept free around the content.
        #[serde(default = "default_padding")]
        padding: f64,
        theme: Theme,
        /// Export only this frame (an id or `@name`) instead of all content, and size the PNG by its width.
        #[serde(default)]
        frame: Option<String>,
    },
}

fn default_padding() -> f64 {
    40.0
}

/// One picture.
#[derive(Debug, Clone, Deserialize)]
pub struct Shot {
    /// File name without `.png`.
    pub name: String,
    /// Commands to run after the intro document is shown and the first frames have run.
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(flatten)]
    pub output: Output,
}

#[derive(Debug, Deserialize)]
struct File {
    shots: Vec<Shot>,
}

/// Reads the shot list.
pub fn load(path: &Path) -> Result<Vec<Shot>, SpecError> {
    let shown = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|source| SpecError::Read { path: shown.clone(), source })?;
    let file: File = serde_json::from_str(&text).map_err(|source| SpecError::Parse { path: shown, source })?;
    Ok(file.shots)
}
