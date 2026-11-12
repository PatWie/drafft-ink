//! File formats by name and by extension.

use std::path::Path;

use clap::ValueEnum;

use crate::error::CliError;

/// A format to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InFormat {
    /// Look at the extension and the content. `.mmd` and `.mermaid` are Mermaid, `.excalidraw` is Excalidraw,
    /// `.png` is a PNG with the scene embedded, anything else is read by content.
    Auto,
    /// The drafft.ink document (`.drafftink` or `.json`), current or from the first version.
    Native,
    /// A document of the first version of drafft.ink (a `.json` without a version). It is migrated on reading,
    /// which is what `native` does too: the name only states the intent.
    Legacy,
    /// An Excalidraw scene (`.excalidraw`, or `.json` from Excalidraw).
    Excalidraw,
    /// Mermaid text: a flowchart or a sequence diagram. Subgraphs become boxes.
    Mermaid,
    /// A PNG exported with the scene embedded.
    Png,
}

/// What to say to `doc.open`, or that the file is Mermaid text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `doc.open` with this `format` argument.
    Document(&'static str),
    /// `diagram.import_mermaid` with the text of the file.
    Mermaid,
}

impl InFormat {
    /// The format for a file: the one given, or what the extension says.
    pub fn resolve(self, path: &Path) -> Source {
        let ext = extension(path);
        match (self, ext.as_str()) {
            (Self::Mermaid, _) | (Self::Auto, "mmd" | "mermaid") => Source::Mermaid,
            (Self::Excalidraw, _) | (Self::Auto, "excalidraw") => Source::Document("excalidraw"),
            (Self::Png, _) | (Self::Auto, "png") => Source::Document("png"),
            (Self::Native | Self::Legacy, _) => Source::Document("drafftink"),
            (Self::Auto, _) => Source::Document("auto"),
        }
    }
}

/// A format to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutFormat {
    /// The drafft.ink document, pretty JSON with images embedded (`.drafftink`, `.json`).
    Native,
    /// An Excalidraw scene (`.excalidraw`). Fields it has no place for are dropped, with a warning.
    Excalidraw,
    /// A vector picture with the fonts embedded (`.svg`).
    Svg,
    /// A PNG, drawn on the CPU. Only a build with the `png` feature has the renderer; the others fail with a way out.
    Png,
}

impl OutFormat {
    /// The format for a file name: the one given, or what the extension says.
    pub fn resolve(given: Option<Self>, path: &Path) -> Result<Self, CliError> {
        if let Some(f) = given {
            return Ok(f);
        }
        match extension(path).as_str() {
            "drafftink" | "json" => Ok(Self::Native),
            "excalidraw" => Ok(Self::Excalidraw),
            "svg" => Ok(Self::Svg),
            "png" => Ok(Self::Png),
            other => Err(CliError::Format(format!(
                "cannot tell the format of `{}` from {}. Name it `.drafftink`, `.excalidraw` or `.svg`, or pass --to native|excalidraw|svg.",
                path.display(),
                if other.is_empty() {
                    "a name without an extension".to_owned()
                } else {
                    format!("the extension `.{other}`")
                },
            ))),
        }
    }
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}
