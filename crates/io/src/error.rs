//! The error type shared by every parser in this crate, and the report of an import that succeeded with caveats.

use drafftink_scene::{Document, DocumentError};

/// Why bytes could not be turned into a document (or a document into bytes).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IoError {
    /// The input is not valid text or JSON for this format.
    #[error("could not read the {format} data: {message}")]
    Parse {
        /// Name of the format, for example `"excalidraw"`.
        format: &'static str,
        /// What went wrong, in plain words.
        message: String,
    },
    /// The input is well-formed but is another kind of file.
    #[error("this is not {expected}: {hint}")]
    WrongFormat {
        /// What the caller asked for, for example `"an Excalidraw library"`.
        expected: &'static str,
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
        /// Name of the format.
        format: &'static str,
        /// What is wrong.
        message: String,
    },
    /// The produced or given native document breaks a rule of the model.
    #[error(transparent)]
    Document(#[from] DocumentError),
}

impl IoError {
    /// A parse error.
    pub fn parse(format: &'static str, message: impl Into<String>) -> Self {
        IoError::Parse { format, message: message.into() }
    }

    /// An invalid-content error.
    pub fn invalid(format: &'static str, message: impl Into<String>) -> Self {
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
