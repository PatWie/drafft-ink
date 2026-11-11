//! The error type shared by every parser in this crate, and the report of an import that succeeded with caveats.

use drafftink_scene::{Document, DocumentError};

/// The file format an error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatName {
    /// Excalidraw scene.
    Excalidraw,
    /// Excalidraw library.
    ExcalidrawLibrary,
    /// Excalidraw clipboard data.
    ExcalidrawClipboard,
    /// Plain text input.
    Text,
    /// An old collaboration snapshot.
    LoroSnapshot,
    /// An old drafft.ink document.
    LegacyDocument,
    /// An old shape list from the clipboard.
    LegacyClipboard,
    /// gzip compression.
    Gzip,
    /// SVG.
    Svg,
    /// PNG.
    Png,
    /// A picture of any kind.
    Image,
    /// drafft.ink clipboard data.
    Clipboard,
    /// Mermaid text.
    Mermaid,
}

impl std::fmt::Display for FormatName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FormatName::Excalidraw => "excalidraw",
            FormatName::ExcalidrawLibrary => "excalidraw library",
            FormatName::ExcalidrawClipboard => "excalidraw clipboard",
            FormatName::Text => "text",
            FormatName::LoroSnapshot => "loro snapshot",
            FormatName::LegacyDocument => "legacy document",
            FormatName::LegacyClipboard => "legacy clipboard",
            FormatName::Gzip => "gzip",
            FormatName::Svg => "svg",
            FormatName::Png => "png",
            FormatName::Image => "image",
            FormatName::Clipboard => "clipboard",
            FormatName::Mermaid => "mermaid",
        })
    }
}

/// What a caller asked for when the input turned out to be another kind of file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expected {
    /// An old collaboration snapshot.
    LegacySnapshot,
    /// An old shape list.
    LegacyShapeList,
    /// An old drafft.ink document.
    LegacyDocument,
    /// An Excalidraw library.
    ExcalidrawLibrary,
    /// An Excalidraw scene.
    ExcalidrawScene,
    /// Excalidraw clipboard data.
    ExcalidrawClipboard,
    /// A drafft.ink document.
    NativeDocument,
    /// Any file this app opens.
    AnyFile,
    /// drafft.ink clipboard data.
    NativeClipboard,
    /// A PNG image.
    Png,
    /// A Mermaid flowchart.
    MermaidFlowchart,
    /// A Mermaid sequence diagram.
    MermaidSequence,
    /// A Mermaid flowchart or sequence diagram.
    MermaidAny,
}

impl std::fmt::Display for Expected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Expected::LegacySnapshot => "an old drafft.ink collaboration snapshot",
            Expected::LegacyShapeList => "an old shape list",
            Expected::LegacyDocument => "an old drafft.ink document",
            Expected::ExcalidrawLibrary => "an Excalidraw library",
            Expected::ExcalidrawScene => "an Excalidraw scene",
            Expected::ExcalidrawClipboard => "Excalidraw clipboard data",
            Expected::NativeDocument => "a drafft.ink document",
            Expected::AnyFile => "a supported file",
            Expected::NativeClipboard => "drafft.ink clipboard data",
            Expected::Png => "a PNG image",
            Expected::MermaidFlowchart => "a Mermaid flowchart",
            Expected::MermaidSequence => "a Mermaid sequence diagram",
            Expected::MermaidAny => "a Mermaid flowchart or sequence diagram",
        })
    }
}

/// Why bytes could not be turned into a document (or a document into bytes).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IoError {
    /// The input is not valid text or JSON for this format.
    #[error("could not read the {format} data: {message}")]
    Parse {
        /// The format.
        format: FormatName,
        /// What went wrong, in plain words.
        message: String,
    },
    /// The input is well-formed but is another kind of file.
    #[error("this is not {expected}: {hint}")]
    WrongFormat {
        /// What the caller asked for.
        expected: Expected,
        /// What to do about it.
        hint: String,
    },
    /// The input is bigger than a hard limit. See [`crate::limits`].
    #[error("the {what} is too large: the limit is {max}")]
    Limit {
        /// What is too large.
        what: &'static str,
        /// The limit, with its unit.
        max: String,
    },
    /// The input parsed but the content cannot be represented.
    #[error("the {format} data is not usable: {message}")]
    Invalid {
        /// The format.
        format: FormatName,
        /// What is wrong.
        message: String,
    },
    /// The produced or given native document breaks a rule of the model.
    #[error(transparent)]
    Document(#[from] DocumentError),
}

impl IoError {
    /// A parse error.
    pub fn parse(format: FormatName, message: impl Into<String>) -> Self {
        IoError::Parse { format, message: message.into() }
    }

    /// An invalid-content error.
    pub fn invalid(format: FormatName, message: impl Into<String>) -> Self {
        IoError::Invalid { format, message: message.into() }
    }
}

/// Things an import dropped, defaulted or repaired. The caller may show them; an import still succeeded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    /// One plain sentence per problem, in input order. Capped at [`crate::limits::MAX_WARNINGS`].
    pub warnings: Vec<String>,
    /// Warnings that were not recorded because of the cap.
    pub omitted: usize,
}

impl ImportReport {
    /// Records a warning, or counts it as omitted past the cap.
    pub fn warn(&mut self, message: impl Into<String>) {
        if self.warnings.len() < crate::limits::MAX_WARNINGS {
            self.warnings.push(message.into());
        } else {
            self.omitted += 1;
        }
    }

    /// True when nothing was reported.
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty() && self.omitted == 0
    }
}

/// A successfully imported document and what the import had to drop or repair.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// The document. It validates.
    pub document: Document,
    /// Caveats.
    pub report: ImportReport,
}
