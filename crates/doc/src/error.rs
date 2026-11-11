//! Errors of the document layer. Every one has a plain-words message that says what to do next.

use std::error::Error;
use std::sync::Arc;

use drafftink_scene::{ElementError, ElementId, StoreError};

/// A failure of the CRDT library, kept as the source of the error. `LoroError` is neither `Clone` nor
/// `PartialEq`, so two of these compare by their message.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct BackendError(#[source] Arc<dyn Error + Send + Sync>);

impl BackendError {
    /// Keeps `error` (any error, or a plain message) as the source.
    pub fn new(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self(Arc::from(error.into()))
    }
}

impl PartialEq for BackendError {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_string() == other.0.to_string()
    }
}

impl Eq for BackendError {}

/// Why a document operation failed. None of these leaves the store half-changed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DocError {
    /// The bytes are not a Loro update or snapshot, or Loro refused them.
    #[error("the data is not a valid update: {0}")]
    Import(#[source] BackendError),
    /// The bytes exceed the configured limit and were not looked at.
    #[error("the update is {size} bytes, over the limit of {limit} bytes")]
    TooLarge {
        /// Size of the input.
        size: usize,
        /// The configured limit.
        limit: usize,
    },
    /// The document was written by a newer schema version.
    #[error("this document uses schema version {found} and this build reads up to {max}. Update the app to open it")]
    NewerSchema {
        /// Version in the document.
        found: u32,
        /// Newest version this build reads.
        max: u32,
    },
    /// The data is Loro, but not a drafft.ink document of any schema version we can migrate.
    #[error("this is not a drafft.ink document: {0}")]
    NotADocument(String),
    /// A migration step failed.
    #[error("migrating the document from schema version {from} failed: {reason}")]
    Migration {
        /// The version that was being migrated.
        from: u32,
        /// What went wrong.
        reason: String,
    },
    /// The peer's version is older than the start of the kept history, so no update can bring it up to date.
    #[error("the history that peer needs was trimmed. Send a snapshot instead of an update")]
    HistoryTrimmed,
    /// A trimmed snapshot starts after the history this replica has, so it cannot be merged.
    #[error(
        "this snapshot starts after the history this copy has, so it cannot be merged. Replace the document from \
         the snapshot instead; edits that were never sent would be lost"
    )]
    SnapshotStartsLater,
    /// The version vector bytes do not decode.
    #[error("the version vector is not valid")]
    BadVersion,
    /// Trimming would lose local changes that no peer has acknowledged.
    #[error("there are local changes the server has not acknowledged, so the history cannot be trimmed yet")]
    UnsyncedChanges,
    /// The CRDT library panicked on some input. Its internal state can no longer be trusted, so every operation that
    /// needs it is refused until the document is replaced (`replace_from_snapshot`). Reads of the scene still work.
    #[error(
        "the collaboration data hit an internal error and cannot be used any more. Reload the document from the \
         server or from a saved copy"
    )]
    Poisoned,
    /// Loro failed in a way that is not the caller's fault.
    #[error("the CRDT layer failed: {0}")]
    Backend(#[source] BackendError),
    /// Loro refuses this peer id (it reserves `u64::MAX`).
    #[error("{0} cannot be used as a peer id")]
    BadPeerId(u64),
    /// A stored value cannot be read back (a wrong type, a number out of range, a broken nesting).
    #[error("a stored value is malformed: {0}")]
    Malformed(String),
    /// A stored element breaks a rule that cannot be repaired. The element is quarantined.
    #[error("element {id} is invalid: {error}")]
    BadElement {
        /// The element.
        id: ElementId,
        /// The broken rule.
        error: ElementError,
    },
}

impl From<loro::LoroError> for DocError {
    fn from(e: loro::LoroError) -> Self {
        DocError::Backend(BackendError::new(e))
    }
}

impl DocError {
    /// A CRDT-layer failure from any error or message.
    pub fn backend(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        DocError::Backend(BackendError::new(error))
    }

    /// The data was rejected on import.
    pub fn import(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        DocError::Import(BackendError::new(error))
    }
}

impl From<DocError> for StoreError {
    fn from(e: DocError) -> Self {
        StoreError::backend(e)
    }
}
